// Swirl-vortex fragment shader.
//
// A radial rotation pulls UVs in a spiral around the center, with
// rotation strength growing toward the edges of the frame so the
// center stays relatively stable and the corners spin hardest. The
// outgoing clip swirls outward; the incoming clip arrives unswirled.
// Combined with the cross-blend this reads as "scene gets sucked
// into a vortex and the next scene drops in clean."
//
// Uniforms:
//   params.x = strength in [0.0, 1.0]. 0 = no swirl (degrades to
//              cross-blend), 1 = ~2π rotation at the corners.
//
// No FBM dependency — pure polar coordinate math.

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

const TAU: f32 = 6.28318530717959;

fn swirl(uv: vec2<f32>, strength: f32) -> vec2<f32> {
    let centered = uv - vec2<f32>(0.5);
    let radius = length(centered);
    // Rotation amount: strong at the edges, weak at the center.
    // The `smoothstep` band keeps the very center (where the swirl
    // would otherwise look like a pinched singularity) clean.
    let edge_weight = smoothstep(0.0, 0.6, radius);
    let theta = strength * TAU * edge_weight;
    let cs = cos(theta);
    let sn = sin(theta);
    let rotated = vec2<f32>(
        centered.x * cs - centered.y * sn,
        centered.x * sn + centered.y * cs,
    );
    return rotated + vec2<f32>(0.5);
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let strength = clamp(u.params.x, 0.0, 1.0);
    // Outgoing clip swirls increasingly as progress runs forward;
    // incoming arrives straight. Reverse swirl on the incoming side
    // would be a "drilling in" feel — saving that for a sibling
    // preset later if anyone wants it.
    let outgoing_swirl = strength * u.progress;
    let from_uv = clamp(swirl(uv, outgoing_swirl), vec2<f32>(0.0), vec2<f32>(1.0));
    let a = textureSample(t_from, samp, from_uv);
    let b = textureSample(t_to, samp, uv);
    return mix(a, b, u.progress);
}
