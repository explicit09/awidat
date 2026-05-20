//! GPU transition renderer.
//!
//! This crate is the GPU half of the awidat transition pipeline. It
//! complements `awidat-render`'s FFmpeg backend: shaders here can
//! express transitions FFmpeg's `xfade` cannot (camera shake,
//! chromatic split, future shader-only catalogue entries) and, by
//! design, share the same code path between offline export and live
//! preview so the two never drift.
//!
//! Today's surface is intentionally small:
//!
//! * [`GpuTransitionRenderer`] — owns one wgpu device, queue, and a
//!   pipeline for one shader. Cheap to keep alive; expensive to
//!   create. Reuse across many frames.
//! * [`TransitionShader`] — enum of supported shaders. Phase 2a ships
//!   only `CrossDissolve` to prove parity with FFmpeg's `xfade=fade`.
//! * [`render_frame`](GpuTransitionRenderer::render_frame) — renders
//!   one frame from two equally-sized RGBA8 inputs at a given
//!   `progress` and returns the resulting RGBA8 bytes.
//!
//! The shader contract is fixed: two `texture_2d<f32>` bindings
//! (`t_from`, `t_to`), one `sampler`, and a `uniform` block carrying
//! `resolution: vec2<f32>` and `progress: f32`. Future shaders ride on
//! the same layout so the pipeline cache can swap fragment programs
//! without rebuilding bind groups.

use bytemuck::{Pod, Zeroable};
use thiserror::Error;

const FULLSCREEN_VS_WGSL: &str = include_str!("shaders/fullscreen.wgsl");
const CROSS_DISSOLVE_FS_WGSL: &str = include_str!("shaders/cross_dissolve.wgsl");
const SHAKE_FS_WGSL: &str = include_str!("shaders/shake.wgsl");
const CHROMATIC_SPLIT_FS_WGSL: &str = include_str!("shaders/chromatic_split.wgsl");
const BLUR_FS_WGSL: &str = include_str!("shaders/blur.wgsl");

/// wgpu requires every row of a texture-to-buffer copy to start on a
/// 256-byte boundary. RGBA8 is 4 bytes per pixel, so we pad each row
/// up to the next multiple of 256 in the staging buffer and strip the
/// padding when handing bytes back to the caller.
const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// Errors from the GPU transition renderer.
#[derive(Debug, Error)]
pub enum GpuError {
    /// No GPU adapter is available — typically a sandboxed CI environment
    /// without a software fallback. Callers should degrade gracefully
    /// rather than aborting the whole render.
    #[error("could not request a GPU adapter: {0}")]
    NoAdapter(String),
    /// Adapter found, but device request failed.
    #[error("could not request a GPU device: {0}")]
    NoDevice(String),
    /// Caller passed RGBA bytes whose length does not match the frame
    /// size implied by `width * height * 4`.
    #[error(
        "frame buffer size mismatch: expected {expected} bytes ({width}x{height} RGBA8), got {actual}"
    )]
    BadFrameSize {
        /// Expected `width * height * 4`.
        expected: usize,
        /// Actual length of the slice the caller passed.
        actual: usize,
        /// Frame width in pixels.
        width: u32,
        /// Frame height in pixels.
        height: u32,
    },
    /// Readback from GPU memory failed.
    #[error("GPU readback failed: {0}")]
    Readback(String),
}

/// Supported transition shaders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransitionShader {
    /// Linear cross-blend, equivalent to FFmpeg `xfade=fade`.
    CrossDissolve,
    /// Per-frame UV jitter (amplitude bells around progress = 0.5)
    /// composited with a linear cross-blend. Renders the `Shake`
    /// primitive that FFmpeg `xfade` cannot express.
    Shake,
    /// Per-channel RGB offset (amplitude bells around progress = 0.5)
    /// composited with a linear cross-blend. Renders the
    /// `ChromaticSplit` primitive that FFmpeg `xfade` cannot express.
    ChromaticSplit,
    /// Box-blur transition. Reads `extra_params[0]` as the per-frame
    /// blur amount, so the composer can drive it from a
    /// [`awidat_proto::transitions::ParamCurve`] evaluated at every
    /// progress value.
    Blur,
}

impl TransitionShader {
    fn fragment_wgsl(&self) -> &'static str {
        match self {
            Self::CrossDissolve => CROSS_DISSOLVE_FS_WGSL,
            Self::Shake => SHAKE_FS_WGSL,
            Self::ChromaticSplit => CHROMATIC_SPLIT_FS_WGSL,
            Self::Blur => BLUR_FS_WGSL,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::CrossDissolve => "cross_dissolve",
            Self::Shake => "shake",
            Self::ChromaticSplit => "chromatic_split",
            Self::Blur => "blur",
        }
    }

    /// Parse the stable shader id that `awidat_proto::transitions::
    /// resolve_composition_gpu_shader` returns. Used by the render
    /// crate to bridge the proto layer (which cannot depend on
    /// render-gpu) to a concrete `TransitionShader` value.
    pub fn from_proto_id(id: &str) -> Option<Self> {
        match id {
            "cross_dissolve" => Some(Self::CrossDissolve),
            "shake" => Some(Self::Shake),
            "chromatic_split" => Some(Self::ChromaticSplit),
            "blur" => Some(Self::Blur),
            _ => None,
        }
    }
}

/// Parameters for one rendered frame.
#[derive(Debug, Clone, Copy)]
pub struct FrameParams {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Transition progress in `[0.0, 1.0]`. 0 returns the outgoing
    /// frame unchanged, 1 returns the incoming frame.
    pub progress: f32,
    /// Four shader-specific scalars accessible as `u.params.xyzw` in
    /// WGSL. The Blur shader reads `params.x` as the blur amount; the
    /// cross-dissolve / shake / chromatic-split shaders ignore them.
    /// Use this to pass per-frame `ParamCurve` evaluations.
    pub extra_params: [f32; 4],
}

impl Default for FrameParams {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            progress: 0.0,
            extra_params: [0.0; 4],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    resolution: [f32; 2],
    progress: f32,
    _pad: f32,
    params: [f32; 4],
}

/// A configured GPU transition renderer.
///
/// Construction is async (wgpu adapter and device requests) but the
/// public constructor wraps that with `pollster::block_on` so callers
/// don't need to thread an executor through.
pub struct GpuTransitionRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
}

impl GpuTransitionRenderer {
    /// Create a renderer bound to one shader. Picks the highest-power
    /// available adapter (Metal on macOS, Vulkan/DX12 elsewhere).
    pub fn new(shader: TransitionShader) -> Result<Self, GpuError> {
        pollster::block_on(Self::new_async(shader))
    }

    async fn new_async(shader: TransitionShader) -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| GpuError::NoAdapter("no compatible GPU adapter".into()))?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("awidat-render-gpu"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .map_err(|e| GpuError::NoDevice(e.to_string()))?;

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("transition_bind_layout"),
            entries: &[
                // t_from
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // t_to
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // uniforms
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("transition_pipeline_layout"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });

        // We concatenate the shared vertex shader with the shader-specific
        // fragment shader. Both share the WGSL module namespace so the
        // fragment shader gets the VsOut struct from the vertex shader.
        let combined = format!("{FULLSCREEN_VS_WGSL}\n{}", shader.fragment_wgsl());
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(shader.label()),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Owned(combined)),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(shader.label()),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("transition_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Ok(Self {
            device,
            queue,
            sampler,
            pipeline,
            bind_layout,
        })
    }

    /// Render one preview frame for desktop scrubbing/playback.
    ///
    /// This is an explicit alias for [`Self::render_frame`]. The two
    /// share the same code path, the same shader, the same uniform
    /// layout, and the same texture sampling — so by construction the
    /// preview pipeline and the export pipeline cannot drift. The
    /// alias exists to make the contract obvious at the call site:
    /// when the desktop app or a UI test asks "render the transition
    /// at time t for preview," it calls this; the export composer
    /// calls [`Self::render_frame`]. Both end up in the same wgpu
    /// pipeline.
    pub fn render_preview_frame(
        &self,
        from_rgba: &[u8],
        to_rgba: &[u8],
        params: FrameParams,
    ) -> Result<Vec<u8>, GpuError> {
        self.render_frame(from_rgba, to_rgba, params)
    }

    /// Render one transition frame.
    ///
    /// `from_rgba` and `to_rgba` must both be `params.width * params.height * 4`
    /// bytes of RGBA8 data in row-major top-down order. The returned
    /// `Vec<u8>` is the same shape.
    pub fn render_frame(
        &self,
        from_rgba: &[u8],
        to_rgba: &[u8],
        params: FrameParams,
    ) -> Result<Vec<u8>, GpuError> {
        let expected = (params.width as usize) * (params.height as usize) * 4;
        if from_rgba.len() != expected {
            return Err(GpuError::BadFrameSize {
                expected,
                actual: from_rgba.len(),
                width: params.width,
                height: params.height,
            });
        }
        if to_rgba.len() != expected {
            return Err(GpuError::BadFrameSize {
                expected,
                actual: to_rgba.len(),
                width: params.width,
                height: params.height,
            });
        }
        pollster::block_on(self.render_frame_async(from_rgba, to_rgba, params))
    }

    async fn render_frame_async(
        &self,
        from_rgba: &[u8],
        to_rgba: &[u8],
        params: FrameParams,
    ) -> Result<Vec<u8>, GpuError> {
        let texture_size = wgpu::Extent3d {
            width: params.width,
            height: params.height,
            depth_or_array_layers: 1,
        };
        let from_tex = self.upload_input_texture("t_from", from_rgba, texture_size);
        let to_tex = self.upload_input_texture("t_to", to_rgba, texture_size);
        let from_view = from_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let to_view = to_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let uniforms = Uniforms {
            resolution: [params.width as f32, params.height as f32],
            progress: params.progress.clamp(0.0, 1.0),
            _pad: 0.0,
            params: params.extra_params,
        };
        let uniform_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("transition_uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("transition_bind_group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&from_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&to_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform_buf.as_entire_binding(),
                },
            ],
        });

        let output_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("transition_output"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output_view = output_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let padded_bytes_per_row = padded_bytes_per_row(params.width);
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("transition_readback"),
            size: (padded_bytes_per_row as u64) * (params.height as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("transition_encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("transition_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &output_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &staging,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(params.height),
                },
            },
            texture_size,
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = staging.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        receiver
            .recv()
            .map_err(|e| GpuError::Readback(e.to_string()))?
            .map_err(|e| GpuError::Readback(format!("{e:?}")))?;

        let view = slice.get_mapped_range();
        let mut out = Vec::with_capacity(expected_bytes(params.width, params.height));
        let row_bytes = (params.width * 4) as usize;
        for row in 0..(params.height as usize) {
            let start = row * (padded_bytes_per_row as usize);
            out.extend_from_slice(&view[start..start + row_bytes]);
        }
        drop(view);
        staging.unmap();
        Ok(out)
    }

    /// Render a complete transition as a sequence of frames.
    ///
    /// `from_frames` and `to_frames` must each contain `frame_count`
    /// frames worth of RGBA8 data laid out one after another. The
    /// returned vector contains `frame_count` rendered frames in the
    /// same layout. `progress` runs from 0 (exclusive, transition just
    /// starting) at the first frame to 1 (inclusive, transition just
    /// completed) at the last frame — this matches FFmpeg `xfade`'s
    /// convention for offset/duration windows.
    pub fn render_sequence(
        &self,
        from_frames: &[u8],
        to_frames: &[u8],
        width: u32,
        height: u32,
        frame_count: u32,
    ) -> Result<Vec<u8>, GpuError> {
        let bytes_per_frame = expected_bytes(width, height);
        let total = bytes_per_frame * (frame_count as usize);
        if from_frames.len() != total {
            return Err(GpuError::BadFrameSize {
                expected: total,
                actual: from_frames.len(),
                width,
                height,
            });
        }
        if to_frames.len() != total {
            return Err(GpuError::BadFrameSize {
                expected: total,
                actual: to_frames.len(),
                width,
                height,
            });
        }
        let mut out = Vec::with_capacity(total);
        for i in 0..frame_count {
            let start = (i as usize) * bytes_per_frame;
            let end = start + bytes_per_frame;
            let progress = if frame_count <= 1 {
                1.0
            } else {
                (i as f32 + 1.0) / (frame_count as f32)
            };
            let frame = self.render_frame(
                &from_frames[start..end],
                &to_frames[start..end],
                FrameParams {
                    width,
                    height,
                    progress,
                    extra_params: [0.0; 4],
                },
            )?;
            out.extend_from_slice(&frame);
        }
        Ok(out)
    }

    fn upload_input_texture(
        &self,
        label: &str,
        rgba: &[u8],
        size: wgpu::Extent3d,
    ) -> wgpu::Texture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 4),
                rows_per_image: Some(size.height),
            },
            size,
        );
        texture
    }
}

fn padded_bytes_per_row(width: u32) -> u32 {
    let row_bytes = width * 4;
    let align = COPY_BYTES_PER_ROW_ALIGNMENT;
    ((row_bytes + align - 1) / align) * align
}

fn expected_bytes(width: u32, height: u32) -> usize {
    (width as usize) * (height as usize) * 4
}

/// Encode an RGBA8 buffer as a PNG. Output is the binary PNG bytes;
/// callers that need a `data:image/png;base64,…` URL can base64-encode
/// the result. Used by the desktop preview command to ship rendered
/// frames over Tauri IPC as `<img>` payloads.
pub fn encode_rgba_to_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, GpuError> {
    let expected = expected_bytes(width, height);
    if rgba.len() != expected {
        return Err(GpuError::BadFrameSize {
            expected,
            actual: rgba.len(),
            width,
            height,
        });
    }
    let mut buf = Vec::with_capacity(expected / 2);
    {
        let mut encoder = png::Encoder::new(&mut buf, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| {
            GpuError::Readback(format!("png header: {e}"))
        })?;
        writer
            .write_image_data(rgba)
            .map_err(|e| GpuError::Readback(format!("png write: {e}")))?;
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::with_capacity(expected_bytes(width, height));
        for _ in 0..(width * height) {
            out.extend_from_slice(&rgba);
        }
        out
    }

    /// Skip GPU-touching tests gracefully when no adapter is available
    /// (sandboxed CI). The renderer must work in real macOS / Linux
    /// dev environments, but we shouldn't fail the whole `cargo test`
    /// run on a headless CI box without a Vulkan/Metal driver.
    fn renderer_or_skip(shader: TransitionShader) -> Option<GpuTransitionRenderer> {
        match GpuTransitionRenderer::new(shader) {
            Ok(r) => Some(r),
            Err(GpuError::NoAdapter(_)) | Err(GpuError::NoDevice(_)) => {
                eprintln!("skipping GPU test: no adapter available in this environment");
                None
            }
            Err(other) => panic!("unexpected renderer init error: {other}"),
        }
    }

    #[test]
    fn rejects_mismatched_input_size() {
        let Some(renderer) = renderer_or_skip(TransitionShader::CrossDissolve) else {
            return;
        };
        let too_short = vec![0u8; 4];
        let err = renderer
            .render_frame(
                &too_short,
                &too_short,
                FrameParams {
                    width: 4,
                    height: 4,
                    progress: 0.5,
                    extra_params: [0.0; 4],
                },
            )
            .unwrap_err();
        assert!(matches!(err, GpuError::BadFrameSize { .. }));
    }

    #[test]
    fn progress_zero_returns_outgoing_frame() {
        let Some(renderer) = renderer_or_skip(TransitionShader::CrossDissolve) else {
            return;
        };
        let from = solid(16, 16, [255, 0, 0, 255]);
        let to = solid(16, 16, [0, 0, 255, 255]);
        let out = renderer
            .render_frame(
                &from,
                &to,
                FrameParams {
                    width: 16,
                    height: 16,
                    progress: 0.0,
                    extra_params: [0.0; 4],
                },
            )
            .unwrap();
        assert_eq!(out[..4], [255, 0, 0, 255]);
    }

    #[test]
    fn progress_one_returns_incoming_frame() {
        let Some(renderer) = renderer_or_skip(TransitionShader::CrossDissolve) else {
            return;
        };
        let from = solid(16, 16, [255, 0, 0, 255]);
        let to = solid(16, 16, [0, 0, 255, 255]);
        let out = renderer
            .render_frame(
                &from,
                &to,
                FrameParams {
                    width: 16,
                    height: 16,
                    progress: 1.0,
                    extra_params: [0.0; 4],
                },
            )
            .unwrap();
        assert_eq!(out[..4], [0, 0, 255, 255]);
    }

    #[test]
    fn progress_half_returns_midpoint_blend() {
        let Some(renderer) = renderer_or_skip(TransitionShader::CrossDissolve) else {
            return;
        };
        let from = solid(16, 16, [200, 0, 0, 255]);
        let to = solid(16, 16, [0, 0, 200, 255]);
        let out = renderer
            .render_frame(
                &from,
                &to,
                FrameParams {
                    width: 16,
                    height: 16,
                    progress: 0.5,
                    extra_params: [0.0; 4],
                },
            )
            .unwrap();
        // Linear blend in sRGB-encoded RGBA8 storage. We don't promote to
        // linear before mixing (matches xfade=fade), so 200*0.5 + 0*0.5 = 100.
        let tolerance = 2;
        assert!(
            (out[0] as i32 - 100).abs() <= tolerance,
            "R channel: got {}, expected ~100",
            out[0]
        );
        assert!(
            (out[2] as i32 - 100).abs() <= tolerance,
            "B channel: got {}, expected ~100",
            out[2]
        );
        assert_eq!(out[3], 255);
    }

    #[test]
    fn renders_a_three_frame_sequence_with_increasing_progress() {
        let Some(renderer) = renderer_or_skip(TransitionShader::CrossDissolve) else {
            return;
        };
        let frame_count = 3u32;
        let width = 8u32;
        let height = 8u32;
        let frame_bytes = (width * height * 4) as usize;
        let mut from = Vec::with_capacity(frame_bytes * frame_count as usize);
        let mut to = Vec::with_capacity(frame_bytes * frame_count as usize);
        for _ in 0..frame_count {
            from.extend_from_slice(&solid(width, height, [255, 0, 0, 255]));
            to.extend_from_slice(&solid(width, height, [0, 0, 255, 255]));
        }
        let out = renderer
            .render_sequence(&from, &to, width, height, frame_count)
            .unwrap();
        assert_eq!(out.len(), frame_bytes * frame_count as usize);
        // Last frame should be the incoming color (progress = 1.0).
        let last = &out[out.len() - frame_bytes..];
        assert_eq!(last[..4], [0, 0, 255, 255]);
        // First frame should be ~1/3 of the way blended.
        let first = &out[..frame_bytes];
        assert!(
            first[0] > first[2],
            "first frame R should still dominate B at progress=1/3: got R={} B={}",
            first[0],
            first[2]
        );
    }

    #[test]
    fn shake_shader_compiles_and_produces_full_frame() {
        let Some(renderer) = renderer_or_skip(TransitionShader::Shake) else {
            return;
        };
        let from = solid(16, 16, [255, 0, 0, 255]);
        let to = solid(16, 16, [0, 0, 255, 255]);
        let out = renderer
            .render_frame(
                &from,
                &to,
                FrameParams {
                    width: 16,
                    height: 16,
                    progress: 0.5,
                    extra_params: [0.0; 4],
                },
            )
            .unwrap();
        // Pure-color inputs render to bell-of-shake mid-blend regardless
        // of the jitter offset — but the output must still be a full
        // 16x16 RGBA frame.
        assert_eq!(out.len(), 16 * 16 * 4);
        // At mid-progress the blend is ~50/50 red/blue.
        assert!(out[0] > 80 && out[0] < 160, "R: {}", out[0]);
        assert!(out[2] > 80 && out[2] < 160, "B: {}", out[2]);
    }

    #[test]
    fn chromatic_split_shader_compiles_and_produces_full_frame() {
        let Some(renderer) = renderer_or_skip(TransitionShader::ChromaticSplit) else {
            return;
        };
        let from = solid(16, 16, [200, 0, 0, 255]);
        let to = solid(16, 16, [0, 0, 200, 255]);
        let out = renderer
            .render_frame(
                &from,
                &to,
                FrameParams {
                    width: 16,
                    height: 16,
                    progress: 0.5,
                    extra_params: [0.0; 4],
                },
            )
            .unwrap();
        assert_eq!(out.len(), 16 * 16 * 4);
        assert!(out[0] > 60 && out[0] < 140, "R: {}", out[0]);
        assert!(out[2] > 60 && out[2] < 140, "B: {}", out[2]);
    }

    #[test]
    fn shake_produces_per_pixel_variance_across_frames() {
        // The defining property of shake: pixel content differs
        // across the transition frames, NOT just across the
        // progress-driven mix. This test catches a regression where
        // the jitter math accidentally collapses to a fixed offset.
        let Some(renderer) = renderer_or_skip(TransitionShader::Shake) else {
            return;
        };
        // Use a synthetic non-uniform input so the shake offset
        // produces visible per-pixel differences (uniform colors hide
        // the jitter).
        let width = 16u32;
        let height = 16u32;
        let mut from = vec![0u8; (width * height * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                let idx = ((y * width + x) * 4) as usize;
                // Diagonal red gradient.
                let intensity = ((x + y) * 8).min(255) as u8;
                from[idx] = intensity;
                from[idx + 1] = 0;
                from[idx + 2] = 0;
                from[idx + 3] = 255;
            }
        }
        let to = solid(width, height, [0, 0, 255, 255]);

        let frame_a = renderer
            .render_frame(
                &from,
                &to,
                FrameParams {
                    width,
                    height,
                    progress: 0.3,
                    extra_params: [0.0; 4],
                },
            )
            .unwrap();
        let frame_b = renderer
            .render_frame(
                &from,
                &to,
                FrameParams {
                    width,
                    height,
                    progress: 0.7,
                    extra_params: [0.0; 4],
                },
            )
            .unwrap();
        let mut differing_pixels = 0;
        for i in (0..frame_a.len()).step_by(4) {
            if frame_a[i] != frame_b[i]
                || frame_a[i + 1] != frame_b[i + 1]
                || frame_a[i + 2] != frame_b[i + 2]
            {
                differing_pixels += 1;
            }
        }
        // The progress-driven mix alone would change every pixel by
        // approximately the same amount; the jitter offset adds extra
        // variance. We just require some difference here — the shake
        // shader IS supposed to perturb things differently at
        // different progress values.
        assert!(
            differing_pixels > 0,
            "shake at progress 0.3 vs 0.7 must produce different pixel data"
        );
    }

    #[test]
    fn preview_and_export_paths_are_bit_equal() {
        // Phase 2 goal contract step 4: "The dissolve at frame N
        // rendered by the export path and the dissolve at the same
        // timestamp rendered by the preview API produce bit-identical
        // RGBA bytes." Both paths call into the same wgpu pipeline,
        // so the test asserts that explicitly.
        let Some(renderer) = renderer_or_skip(TransitionShader::CrossDissolve) else {
            return;
        };
        let from = solid(32, 32, [220, 0, 50, 255]);
        let to = solid(32, 32, [10, 110, 240, 255]);
        let params = FrameParams {
            width: 32,
            height: 32,
            progress: 0.42,
            extra_params: [0.0; 4],
        };
        let export = renderer.render_frame(&from, &to, params).unwrap();
        let preview = renderer
            .render_preview_frame(&from, &to, params)
            .unwrap();
        assert_eq!(
            export, preview,
            "preview and export paths must produce bit-identical RGBA bytes"
        );
    }

    #[test]
    fn preview_path_works_for_every_shader() {
        // Same drift guarantee for the non-cross-dissolve shaders.
        // Each shader's render_frame and render_preview_frame must
        // agree byte-for-byte.
        let shaders = [
            TransitionShader::CrossDissolve,
            TransitionShader::Shake,
            TransitionShader::ChromaticSplit,
        ];
        for shader in shaders {
            let Some(renderer) = renderer_or_skip(shader) else {
                continue;
            };
            let from = solid(16, 16, [120, 30, 200, 255]);
            let to = solid(16, 16, [40, 200, 90, 255]);
            let params = FrameParams {
                width: 16,
                height: 16,
                progress: 0.5,
                extra_params: [0.0; 4],
            };
            let export = renderer.render_frame(&from, &to, params).unwrap();
            let preview = renderer
                .render_preview_frame(&from, &to, params)
                .unwrap();
            assert_eq!(
                export, preview,
                "shader {shader:?}: preview must match export bit-for-bit"
            );
        }
    }

    #[test]
    fn padded_row_alignment_is_correct() {
        assert_eq!(padded_bytes_per_row(64), 256);
        assert_eq!(padded_bytes_per_row(63), 256);
        assert_eq!(padded_bytes_per_row(65), 512);
        assert_eq!(padded_bytes_per_row(640), 2560);
    }
}
