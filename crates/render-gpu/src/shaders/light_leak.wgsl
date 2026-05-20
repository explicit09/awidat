// Light-leak fragment shader.
//
// A warm overexposed glow sweeps through the frame from an
// off-screen point (simulating a camera lens flare or a strip of
// film leaking). The blend between outgoing and incoming clips
// follows `progress` linearly; the leak intensity bells around the
// midpoint so the brightness peaks mid-transition.
//
// Uniforms:
//   params.x = peak intensity in [0.0, 1.0] (typically 0.6 to 1.0).
//   params.y = warmth bias in [0.0, 1.0]. 0 = neutral white,
//              1 = full sunset orange. Drives the tint of the leak.
//
// This is the "premium / cinematic warm" entry in the catalog —
// CSS can't approximate a properly-tinted off-screen glow, and
// FFmpeg's xfade has no equivalent. Pure radial falloff math + a
// small ACES-style tonemap: no FBM, no noise textures.

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

const PI: f32 = 3.14159265358979;

// Cheap ACES-like filmic tonemap; keeps the leak from clipping to
// pure white when intensity stacks above 1.0.
fn tonemap(c: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let cc = 2.43;
    let d = 0.59;
    let e = 0.14;
    let mapped = (c * (a * c + b)) / (c * (cc * c + d) + e);
    return clamp(mapped, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let intensity = clamp(u.params.x, 0.0, 2.0);
    let warmth = clamp(u.params.y, 0.0, 1.0);

    // Off-screen source position drifts across progress so the leak
    // appears to sweep through the frame. Start beyond top-right,
    // end beyond bottom-left.
    let start = vec2<f32>(1.2, -0.15);
    let end = vec2<f32>(-0.15, 1.15);
    let leak_pos = mix(start, end, u.progress);

    let dist = distance(uv, leak_pos);
    let core = exp(-dist * 3.5);
    // Bell curve so the leak peaks at progress = 0.5 instead of
    // simply ramping in.
    let bell = sin(u.progress * PI);
    let amount = core * bell * intensity;

    // Warm tint: pull the green channel down, push the red up.
    let warm = mix(
        vec3<f32>(1.0, 0.95, 0.85),
        vec3<f32>(1.0, 0.55, 0.20),
        warmth,
    );

    let a = textureSample(t_from, samp, uv).rgb;
    let b = textureSample(t_to, samp, uv).rgb;
    let base = mix(a, b, u.progress);
    let lit = base + warm * amount * 1.6;
    return vec4<f32>(tonemap(lit), 1.0);
}
