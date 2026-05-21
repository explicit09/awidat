//! Per-frame compositor scaffold for the GPU motion path.
//!
//! `awidat-render-gpu` started as a transitions-only renderer (two
//! input frames + a progress scalar → one output frame). Motion
//! (translate / scale / rotate / blur / opacity envelopes) currently
//! lowers to FFmpeg filter strings, which bypasses the GPU entirely.
//!
//! This module is the seed of the per-frame motion compositor that
//! consumes the animation graph and composes a single output frame on
//! the GPU. It deliberately starts small:
//!
//! * [`Compositor`] — the abstraction every concrete pass implements.
//!   Today's surface is `compose(&CompositorFrameSpec) -> wgpu::Texture`
//!   so callers can chain passes (transform → blur → opacity) without
//!   the trait knowing about specific motion components.
//! * [`CompositorFrameSpec`] — minimal description of one frame's
//!   inputs. Phase-1 MVP only carries a single layer + a transform; we
//!   leave room for `blur_amount`, `opacity`, additional layers, etc.
//!   without re-shaping the public trait.
//! * [`BlendMode`] — placeholder enum so multi-layer composers
//!   (future) don't break source-compatibility once they need a blend
//!   selector.
//!
//! What this module is NOT (and on purpose):
//!
//! * It does not retrofit [`crate::GpuTransitionRenderer`] onto the
//!   trait. That renderer takes two inputs, owns its own pipeline,
//!   and would balloon if we tried to force-fit it. The scaffold
//!   exists alongside it; we can collapse them later if the shapes
//!   converge.
//! * It does not implement blur, opacity, or particle composition.
//!   Those are TODOs tracked at the call site.

use thiserror::Error;

/// Errors returned by [`Compositor::compose`].
#[derive(Debug, Error)]
pub enum CompositorError {
    /// No GPU adapter is available — typically a sandboxed CI
    /// environment without a software fallback.
    #[error("could not request a GPU adapter: {0}")]
    NoAdapter(String),
    /// Adapter found, but device request failed.
    #[error("could not request a GPU device: {0}")]
    NoDevice(String),
    /// Caller supplied an input whose byte length does not match the
    /// declared `width * height * 4`.
    #[error(
        "layer buffer size mismatch: expected {expected} bytes ({width}x{height} RGBA8), got {actual}"
    )]
    BadLayerSize {
        /// Expected `width * height * 4`.
        expected: usize,
        /// Actual byte length the caller passed.
        actual: usize,
        /// Layer width in pixels.
        width: u32,
        /// Layer height in pixels.
        height: u32,
    },
    /// The compositor received an empty list of layers but at least
    /// one is required.
    #[error("compositor received zero layers")]
    NoLayers,
    /// Output dimensions must be non-zero.
    #[error("compositor output dimensions must be non-zero, got {width}x{height}")]
    BadOutputSize {
        /// Requested output width.
        width: u32,
        /// Requested output height.
        height: u32,
    },
}

/// How a layer is blended over the accumulated output. Phase-1 only
/// uses `Normal` (replace-with-alpha); the enum exists so multi-layer
/// composers can extend it without breaking the public spec shape.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlendMode {
    /// Standard alpha blend over the destination. The only mode
    /// supported by [`crate::TransformCompositor`] today.
    #[default]
    Normal,
}

/// Affine transform applied to a single layer, expressed in component
/// form. The compositor composes these into a 4x4 matrix on the CPU
/// before uploading; this struct stays component-form so the animation
/// graph can drive each channel independently from its own curve.
///
/// Conventions:
///
/// * Coordinates are in pixels of the *output frame*. `translate_px`
///   moves the layer center by that many pixels (positive `y` is
///   down, matching the rest of the crate's top-left UV convention).
/// * `scale` is multiplicative; `1.0` leaves the layer at its native
///   size in output pixels. `2.0` doubles each axis.
/// * `rotate_radians` is counter-clockwise in screen space (after the
///   y-axis flip), so a positive value rotates the right edge upward
///   when viewed on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransformSpec {
    /// Pixel offset applied after scale + rotate.
    pub translate_px: [f32; 2],
    /// Per-axis multiplicative scale `[sx, sy]`. `1.0` is identity.
    pub scale: [f32; 2],
    /// Rotation in radians.
    pub rotate_radians: f32,
}

impl Default for TransformSpec {
    fn default() -> Self {
        Self {
            translate_px: [0.0, 0.0],
            scale: [1.0, 1.0],
            rotate_radians: 0.0,
        }
    }
}

impl TransformSpec {
    /// Build the column-major 4x4 matrix that maps from layer-NDC
    /// `[-1, +1]^2` to output-NDC, applying scale, rotation, and
    /// translation in that order.
    ///
    /// `layer_size_px` is the layer's native size in pixels; the
    /// shader's local coordinates are layer-NDC so we fold the
    /// half-size into the scale row so a `scale = 1.0` layer occupies
    /// exactly its native pixel footprint in the output frame.
    /// `output_size_px` is the output's pixel size; we use it to
    /// convert pixel translations into NDC units.
    pub fn to_matrix_column_major(
        &self,
        layer_size_px: [u32; 2],
        output_size_px: [u32; 2],
    ) -> [[f32; 4]; 4] {
        let out_w = output_size_px[0].max(1) as f32;
        let out_h = output_size_px[1].max(1) as f32;
        let layer_w = layer_size_px[0].max(1) as f32;
        let layer_h = layer_size_px[1].max(1) as f32;
        // Half-size of the layer in NDC units (`layer / output`
        // because layer-NDC is `[-1, +1]` so the full extent is 2 NDC
        // units = 1 layer width).
        let half_x = (layer_w / out_w) * self.scale[0];
        let half_y = (layer_h / out_h) * self.scale[1];
        // Pixel translation in NDC: `dx/(out_w/2)` because clip-space
        // x ranges over 2 NDC units across `out_w` pixels.
        let tx = self.translate_px[0] * 2.0 / out_w;
        // Invert y because output clip-space y goes UP but pixel y
        // goes DOWN; we want `translate_px = [0, +N]` to push the
        // layer downward on screen.
        let ty = -self.translate_px[1] * 2.0 / out_h;
        let c = self.rotate_radians.cos();
        let s = self.rotate_radians.sin();
        // Compose Scale -> Rotate -> Translate, column-major.
        // [ c*sx  -s*sy   0   tx ]
        // [ s*sx   c*sy   0   ty ]
        // [  0      0     1    0 ]
        // [  0      0     0    1 ]
        [
            [c * half_x, s * half_x, 0.0, 0.0],
            [-s * half_y, c * half_y, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [tx, ty, 0.0, 1.0],
        ]
    }
}

/// One layer passed to the compositor. RGBA8, top-down, row-major.
#[derive(Debug, Clone)]
pub struct LayerSpec<'a> {
    /// Layer width in pixels.
    pub width: u32,
    /// Layer height in pixels.
    pub height: u32,
    /// RGBA8 pixels, length must equal `width * height * 4`.
    pub rgba: &'a [u8],
    /// Component-form transform applied to this layer.
    pub transform: TransformSpec,
    /// How this layer composes over the running output. Phase-1
    /// honours only [`BlendMode::Normal`].
    pub blend: BlendMode,
}

/// Description of one frame the compositor should produce.
#[derive(Debug, Clone)]
pub struct CompositorFrameSpec<'a> {
    /// Output frame width in pixels.
    pub output_width: u32,
    /// Output frame height in pixels.
    pub output_height: u32,
    /// Layers composed back-to-front. Phase-1 [`crate::TransformCompositor`]
    /// only honours the first layer; later compositors will fold the
    /// rest into the output.
    pub layers: Vec<LayerSpec<'a>>,
}

impl<'a> CompositorFrameSpec<'a> {
    /// Convenience: build a spec containing a single layer occupying
    /// the entire output frame at native size.
    pub fn single_layer(output_width: u32, output_height: u32, layer: LayerSpec<'a>) -> Self {
        Self {
            output_width,
            output_height,
            layers: vec![layer],
        }
    }
}

/// Output of a compositor pass.
///
/// We return both the [`wgpu::Texture`] (so subsequent passes can
/// consume it without a roundtrip through CPU memory) and the
/// already-read-back RGBA8 bytes (so callers that only want a single
/// final frame can take them directly). Phase-1 always materialises
/// bytes so tests can inspect them; a future variant may allow
/// GPU-only output to avoid the readback cost.
#[derive(Debug)]
pub struct CompositorFrame {
    /// Render-target texture (RGBA8Unorm, `usage` includes COPY_SRC
    /// and TEXTURE_BINDING so it can be sampled by a later pass).
    pub texture: wgpu::Texture,
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// Tightly-packed RGBA8 bytes, top-down row-major.
    pub rgba: Vec<u8>,
}

/// Abstract per-frame compositor. Implementations own their own
/// `wgpu::Device` / `wgpu::Queue` / `wgpu::RenderPipeline` and are
/// expected to be reused across many frames.
pub trait Compositor {
    /// Render one frame from `spec`. Returns the output texture plus
    /// the RGBA8 bytes for callers that only need the bytes (e.g.
    /// preview).
    fn compose(&self, spec: &CompositorFrameSpec) -> Result<CompositorFrame, CompositorError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_transform_maps_layer_to_full_output_when_sizes_match() {
        let t = TransformSpec::default();
        let m = t.to_matrix_column_major([32, 32], [32, 32]);
        // Identity scale + sizes match → layer-NDC [-1,+1] maps to
        // clip-NDC [-1,+1] (covers the whole frame).
        assert!((m[0][0] - 1.0).abs() < 1e-6, "m00 = {}", m[0][0]);
        assert!((m[1][1] - 1.0).abs() < 1e-6, "m11 = {}", m[1][1]);
        assert!(m[3][0].abs() < 1e-6, "tx = {}", m[3][0]);
        assert!(m[3][1].abs() < 1e-6, "ty = {}", m[3][1]);
    }

    #[test]
    fn translate_px_converts_to_clip_units_with_flipped_y() {
        let t = TransformSpec {
            translate_px: [50.0, 50.0],
            ..Default::default()
        };
        let m = t.to_matrix_column_major([100, 100], [100, 100]);
        // 50px on a 100px wide output is +1.0 in clip-x and -1.0 in
        // clip-y (downward-positive pixels → upward-negative NDC).
        assert!((m[3][0] - 1.0).abs() < 1e-6, "tx = {}", m[3][0]);
        assert!((m[3][1] + 1.0).abs() < 1e-6, "ty = {}", m[3][1]);
    }

    #[test]
    fn scale_multiplies_layer_extent() {
        let t = TransformSpec {
            scale: [2.0, 2.0],
            ..Default::default()
        };
        // 32px layer, 64px output, scale 2.0 → half-extent = 32/64 *
        // 2.0 = 1.0, i.e. layer covers the entire output.
        let m = t.to_matrix_column_major([32, 32], [64, 64]);
        assert!((m[0][0] - 1.0).abs() < 1e-6);
        assert!((m[1][1] - 1.0).abs() < 1e-6);
    }
}
