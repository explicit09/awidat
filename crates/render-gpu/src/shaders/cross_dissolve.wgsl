// Cross-dissolve fragment shader.
//
// The most basic transition: linearly mix the outgoing frame into the
// incoming frame as `progress` goes from 0.0 to 1.0. This is the same
// math FFmpeg's `xfade=fade` filter performs and exists primarily to
// prove the GPU pipeline produces equivalent output to the existing
// xfade backend before we add primitives xfade can't express.

struct Uniforms {
    resolution: vec2<f32>,
    progress: f32,
    _pad: f32,
    params: vec4<f32>,
};

@group(0) @binding(0) var t_from: texture_2d<f32>;
@group(0) @binding(1) var t_to: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> u: Uniforms;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let a = textureSample(t_from, samp, uv);
    let b = textureSample(t_to, samp, uv);
    return mix(a, b, u.progress);
}
