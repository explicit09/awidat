//! Integration test for the `TransformCompositor` MVP.
//!
//! Constructs a 256x256 RGBA8 input, applies translate=(10,5) +
//! scale=1.5 + rotate=15deg, runs `compose`, and asserts:
//!
//! * No panic
//! * Returned texture matches the requested output size
//! * The output buffer length matches `width * height * 4`
//! * At least one sampled output pixel is non-zero (proves the
//!   transform-aware vertex+fragment shader actually ran rather than
//!   leaving the clear color everywhere).
//!
//! On environments without a GPU adapter (sandboxed CI) the test
//! skips with a `eprintln!` and exits cleanly, mirroring the
//! `renderer_or_skip` pattern in `lib.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use montage_render_gpu::{
    BlendMode, Compositor, CompositorError, CompositorFrameSpec, LayerSpec, TransformCompositor,
    TransformSpec,
};

fn checkerboard(width: u32, height: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            // 32-pixel checker, so a 256x256 image has a clear 8x8
            // grid that's easy to spot in any sampled pixel.
            let cell = ((x / 32) ^ (y / 32)) & 1;
            let v = if cell == 0 { 255 } else { 32 };
            buf.extend_from_slice(&[v, v, v, 255]);
        }
    }
    buf
}

fn make_or_skip() -> Option<TransformCompositor> {
    match TransformCompositor::new() {
        Ok(c) => Some(c),
        Err(CompositorError::NoAdapter(_)) | Err(CompositorError::NoDevice(_)) => {
            eprintln!("skipping TransformCompositor test: no adapter available");
            None
        }
        Err(other) => panic!("unexpected init error: {other}"),
    }
}

#[test]
fn transform_compositor_renders_non_empty_frame_with_translate_scale_rotate() {
    let Some(compositor) = make_or_skip() else {
        return;
    };
    let width = 256u32;
    let height = 256u32;
    let layer_rgba = checkerboard(width, height);

    let transform = TransformSpec {
        translate_px: [10.0, 5.0],
        scale: [1.5, 1.5],
        // 15 degrees in radians ≈ 0.2618.
        rotate_radians: 15.0_f32.to_radians(),
    };
    let spec = CompositorFrameSpec::single_layer(
        width,
        height,
        LayerSpec {
            width,
            height,
            rgba: &layer_rgba,
            transform,
            blend: BlendMode::Normal,
        },
    );

    let frame = match compositor.compose(&spec) {
        Ok(frame) => frame,
        Err(err) => {
            panic!("TransformCompositor::compose should succeed on synthetic input: {err:?}")
        }
    };

    assert_eq!(frame.width, width, "frame.width must match spec");
    assert_eq!(frame.height, height, "frame.height must match spec");
    assert_eq!(
        frame.texture.size().width,
        width,
        "texture.size().width must match spec",
    );
    assert_eq!(
        frame.texture.size().height,
        height,
        "texture.size().height must match spec",
    );
    assert_eq!(
        frame.rgba.len(),
        (width * height * 4) as usize,
        "rgba buffer must be tightly packed",
    );

    // The clear color is fully transparent black, so any non-zero
    // byte proves the shader ran and rasterized the transformed
    // layer onto the output.
    let any_nonzero = frame.rgba.iter().any(|&b| b != 0);
    assert!(
        any_nonzero,
        "expected at least one non-zero pixel in the composed frame",
    );
}

#[test]
fn transform_compositor_rejects_layer_size_mismatch() {
    let Some(compositor) = make_or_skip() else {
        return;
    };
    // Declare 16x16 but only provide 16 bytes.
    let too_short = vec![0u8; 16];
    let spec = CompositorFrameSpec::single_layer(
        16,
        16,
        LayerSpec {
            width: 16,
            height: 16,
            rgba: &too_short,
            transform: TransformSpec::default(),
            blend: BlendMode::Normal,
        },
    );
    match compositor.compose(&spec) {
        Err(err) => assert!(matches!(err, CompositorError::BadLayerSize { .. })),
        Ok(_) => panic!("undersized layer must be rejected"),
    }
}
