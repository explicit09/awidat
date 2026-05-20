// Cinematic-pan fragment shader.
//
// 10-tap horizontal motion blur whose intensity bells around
// progress = 0.5. Both outgoing and incoming clips smear in the
// same direction, so the read is "the camera whip-pans across the
// cut." Distinct from `motion_blur` (omnidirectional) and
// `whip_pan_left/right` (FFmpeg `hblur` fallback — directional but
// not actually a directional blur).
//
// Uniforms:
//   params.x = direction sign. -1 = leftward sweep, +1 = rightward.
//              Other values are clamped to ±1; 0 collapses to a
//              cross-blend with no smear.
//   params.y = peak blur intensity in [0.0, 1.0] (radius at the
//              bell's apex, as fraction of frame width).

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
const TAPS: i32 = 10;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let dir = clamp(u.params.x, -1.0, 1.0);
    let max_intensity = clamp(u.params.y, 0.0, 1.0);
    // Bell curve so the smear peaks mid-transition.
    let amount = max_intensity * sin(u.progress * PI);
    let radius = amount * 0.08;

    // Asymmetric blur kernel: samples extend in the +dir direction
    // only. A leftward pan (`dir < 0`) smears trailing edges to the
    // left; a rightward pan smears them to the right. A symmetric
    // (centered) kernel would average to the same value either way,
    // so left vs right would be visually identical — exactly the
    // bug the cinematic_pan_left_and_right_produce_mirrored_blurs
    // test catches.
    var sum_a = vec4<f32>(0.0);
    var sum_b = vec4<f32>(0.0);
    let taps_f = f32(TAPS);
    for (var i = 0; i < TAPS; i = i + 1) {
        let t = f32(i) / (taps_f - 1.0); // 0..1
        let offset = vec2<f32>(dir * t * radius, 0.0);
        let sample_uv = clamp(uv + offset, vec2<f32>(0.0), vec2<f32>(1.0));
        sum_a = sum_a + textureSample(t_from, samp, sample_uv);
        sum_b = sum_b + textureSample(t_to, samp, sample_uv);
    }
    sum_a = sum_a / taps_f;
    sum_b = sum_b / taps_f;
    return mix(sum_a, sum_b, u.progress);
}
