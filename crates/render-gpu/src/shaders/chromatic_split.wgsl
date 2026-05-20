// Chromatic-aberration fragment shader.
//
// Separates RGB channels horizontally with an offset that bells
// around progress = 0.5. The visual read is "the picture splits into
// its color channels through the cut" — a glitch / sci-fi accent.
// Like the shake shader, this exists to express something FFmpeg's
// xfade cannot, and lands as the second proof that the data-only
// primitive vocabulary now has a real rendering backend.

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

fn bell(t: f32) -> f32 {
    let x = (t - 0.5) * 2.0;
    return max(0.0, 1.0 - x * x);
}

fn split_channels(tex: texture_2d<f32>, samp_: sampler, uv: vec2<f32>, amount: f32) -> vec4<f32> {
    let r_uv = clamp(uv + vec2<f32>(amount, 0.0), vec2<f32>(0.0), vec2<f32>(1.0));
    let b_uv = clamp(uv - vec2<f32>(amount, 0.0), vec2<f32>(0.0), vec2<f32>(1.0));
    let r = textureSample(tex, samp_, r_uv).r;
    let g = textureSample(tex, samp_, uv).g;
    let b = textureSample(tex, samp_, b_uv).b;
    let a = textureSample(tex, samp_, uv).a;
    return vec4<f32>(r, g, b, a);
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let amount = bell(u.progress) * 0.012;
    let a = split_channels(t_from, samp, uv, amount);
    let b = split_channels(t_to, samp, uv, amount);
    return mix(a, b, u.progress);
}
