// Box-blur transition fragment shader with per-frame amount.
//
// The blur radius is driven by `u.params.x`, which the composer
// writes per frame by evaluating a `ParamCurve`. This is the Phase 4
// demonstrator: a primitive whose parameter actually animates over
// the transition window, not just a scalar held constant.
//
// Cross-dissolve is layered on top so the transition still completes
// at progress = 1.0 (incoming clip wins). A 9-tap box sample is more
// than enough to show measurable Laplacian-variance differences in
// the goal-contract render test.

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
    let amount = clamp(u.params.x, 0.0, 1.0);
    // Cap the radius at 5% of frame width to avoid massive blur that
    // dwarfs the test signal.
    let radius = amount * 0.05;
    var sum = vec4<f32>(0.0);
    var count = 0.0;
    for (var dx = -1; dx <= 1; dx = dx + 1) {
        for (var dy = -1; dy <= 1; dy = dy + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * radius;
            let sample_uv = clamp(uv + offset, vec2<f32>(0.0), vec2<f32>(1.0));
            let a = textureSample(t_from, samp, sample_uv);
            let b = textureSample(t_to, samp, sample_uv);
            sum = sum + mix(a, b, u.progress);
            count = count + 1.0;
        }
    }
    return sum / count;
}
