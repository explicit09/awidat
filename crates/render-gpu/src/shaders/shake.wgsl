// Camera-shake fragment shader.
//
// Samples the two source frames with a pseudo-random UV jitter whose
// amplitude bells around progress = 0.5. The visual read is "screen
// shakes through the cut" — a stylistic emphasis transition that
// FFmpeg's xfade cannot express. The blend between outgoing and
// incoming clips uses a standard linear mix on top of the jitter so
// the transition still completes cleanly at progress = 1.0.

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

// Bell curve over [0, 1] peaking at 0.5. Avoids importing sin() to
// keep the shader portable across naga's WGSL feature set.
fn bell(t: f32) -> f32 {
    let x = (t - 0.5) * 2.0;
    return max(0.0, 1.0 - x * x);
}

// Cheap pseudo-random offset driven by progress + screen coordinates.
// Not crypto-strong; just visually noisy.
fn jitter(progress: f32, uv: vec2<f32>) -> vec2<f32> {
    let phase = progress * 30.0;
    let jx = sin(phase * 7.31 + uv.y * 13.7);
    let jy = cos(phase * 5.27 + uv.x * 11.3);
    return vec2<f32>(jx, jy);
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let amplitude = bell(u.progress) * 0.04;
    let offset = jitter(u.progress, uv) * amplitude;
    let shaken_uv = clamp(uv + offset, vec2<f32>(0.0), vec2<f32>(1.0));
    let a = textureSample(t_from, samp, shaken_uv);
    let b = textureSample(t_to, samp, shaken_uv);
    return mix(a, b, u.progress);
}
