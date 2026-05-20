// Luma-mask reveal shader.
//
// Computes a procedural grayscale mask value per pixel, then reveals
// the incoming clip wherever the mask value is below the current
// progress (with a soft feathered edge). The mask kind is selected by
// `params.x` (cast to integer):
//
//   0  →  clock wipe (rotating pie-slice from 12 o'clock)
//   1  →  venetian blinds horizontal (8 slats opening from top)
//   2  →  checkerboard dissolve (8×8 grid, alternating tiles)
//   3  →  spiral wipe (Archimedean spiral unwinding from center)
//   4  →  burst wipe (12 radial spokes growing from center)
//   5  →  jaws wipe (interlocking triangular teeth meeting in middle)
//
// `params.y` is the edge softness in `[0.0, 1.0]`. 0 = hard mask
// (binary on/off per pixel); higher values feather the boundary so
// the transition reads as a smooth reveal rather than a stepped one.
//
// This single shader is the entry point for the much larger Kdenlive
// luma-mask catalogue. All kinds are procedural; future image-based
// masks (organic shapes, hand-drawn patterns) will add a sampled
// texture binding alongside `params.x`.

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
const TAU: f32 = 6.28318530717959;

fn clock_mask(uv: vec2<f32>) -> f32 {
    // Angle from the center, normalized to `[0, 1]` with 12 o'clock
    // at 0. The mask grows clockwise so values further around the
    // dial reveal later in the transition.
    let centered = uv - vec2<f32>(0.5, 0.5);
    var angle = atan2(centered.x, -centered.y); // 0 at top, +CW
    if (angle < 0.0) {
        angle = angle + TAU;
    }
    return angle / TAU;
}

fn blinds_mask(uv: vec2<f32>) -> f32 {
    // Eight horizontal slats. Each slat opens from its top edge so
    // the result reads like venetian blinds tilting open. We squash
    // the per-slat coordinate into `[0, 1]` and return it as the
    // mask value, so each slat reveals top-down in unison.
    let slats = 8.0;
    return fract(uv.y * slats);
}

fn checkerboard_mask(uv: vec2<f32>) -> f32 {
    // 8×8 grid. Cells alternate between revealing at progress < 0.5
    // and progress > 0.5, so the dissolve fills in two waves rather
    // than one. The per-cell mask offset is the parity (0 or ~0.5).
    let cells = 8.0;
    let grid = floor(uv * cells);
    let parity = (grid.x + grid.y) - 2.0 * floor((grid.x + grid.y) * 0.5);
    return select(0.55, 0.0, parity == 0.0);
}

fn spiral_mask(uv: vec2<f32>) -> f32 {
    // Archimedean spiral: mask value = (angle + radius * twist) wrapped.
    // The result is a smooth radial sweep that turns inward as it
    // moves toward the center, so the reveal "unwinds" from the
    // outside.
    let centered = uv - vec2<f32>(0.5);
    let radius = length(centered);
    var angle = atan2(centered.x, -centered.y);
    if (angle < 0.0) {
        angle = angle + TAU;
    }
    let twist = 4.0;
    let raw = (angle / TAU + radius * twist) - floor(angle / TAU + radius * twist);
    return raw;
}

fn burst_mask(uv: vec2<f32>) -> f32 {
    // 12 radial spokes. Spokes are dark (small mask value) and the
    // gaps between are bright; with progress sweeping, spokes
    // reveal first, then the gaps fill in.
    let centered = uv - vec2<f32>(0.5);
    var angle = atan2(centered.x, -centered.y);
    if (angle < 0.0) {
        angle = angle + TAU;
    }
    let spokes = 12.0;
    let phase = fract(angle / TAU * spokes);
    // Distance from the nearest spoke center (0 at spoke, 0.5 between).
    let to_spoke = min(phase, 1.0 - phase) * 2.0;
    return to_spoke;
}

fn jaws_mask(uv: vec2<f32>) -> f32 {
    // 8 interlocking triangular teeth from top and bottom. The
    // top-edge teeth point down; the bottom-edge teeth point up,
    // shifted by half a tooth so they alternate. Mask value is the
    // vertical distance to the nearest tooth tip, so the reveal
    // appears to close like jaws meeting in the middle.
    let teeth = 8.0;
    let column = fract(uv.x * teeth);
    let tooth_offset = abs(column - 0.5) * 2.0; // 0 at tip, 1 at base
    // Two sets of teeth: one pointing down from top, one pointing
    // up from bottom, offset by half a tooth-column.
    let top_tip = tooth_offset * 0.5;
    let column_b = fract(uv.x * teeth + 0.5);
    let bottom_offset = abs(column_b - 0.5) * 2.0;
    let bottom_tip = 1.0 - bottom_offset * 0.5;
    // Where is the pixel along the tooth axis? Distance from the
    // closer tooth boundary so the bite closes from both edges.
    let from_top = uv.y - top_tip;
    let from_bottom = bottom_tip - uv.y;
    return clamp(min(from_top, from_bottom) * 2.0 + 0.25, 0.0, 1.0);
}

fn mask_value(uv: vec2<f32>, kind: i32) -> f32 {
    if (kind == 1) {
        return blinds_mask(uv);
    }
    if (kind == 2) {
        return checkerboard_mask(uv);
    }
    if (kind == 3) {
        return spiral_mask(uv);
    }
    if (kind == 4) {
        return burst_mask(uv);
    }
    if (kind == 5) {
        return jaws_mask(uv);
    }
    return clock_mask(uv);
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let kind = i32(u.params.x + 0.5);
    let softness = clamp(u.params.y, 0.0, 1.0);
    let mask = mask_value(uv, kind);
    // Reveal-blend: where mask <= progress we show `to`, otherwise
    // we show `from`. Feathered via smoothstep so `softness` widens
    // the transition band around `progress`.
    let edge = max(softness, 0.001);
    let blend = smoothstep(u.progress - edge, u.progress + edge, 1.0 - mask);
    let a = textureSample(t_from, samp, uv);
    let b = textureSample(t_to, samp, uv);
    return mix(a, b, blend);
}
