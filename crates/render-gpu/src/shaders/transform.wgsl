// Single-layer transform compositor.
//
// Reads one RGBA texture (`t_layer`) and rewrites it onto the output
// surface through an affine 2D transform expressed as a 4x4 matrix in
// homogeneous clip-space coordinates. The matrix is built CPU-side from
// translate / scale / rotate components and uploaded as a uniform — the
// shader does no decomposition.
//
// The vertex shader emits a fullscreen-quad of the LAYER (not the
// viewport): four corners in normalized layer space `[-1, +1]^2` get
// pushed through `u.transform`. The viewport's clip-space matches the
// output texture, so `u.transform` maps from "layer NDC" → "output
// NDC". The accompanying fragment shader samples the layer texture
// with the interpolated UV.
//
// The UV is generated in `[0, 1]` from the same four corners. Origin
// is top-left to match the rest of `montage-render-gpu`'s convention.

struct Uniforms {
    transform: mat4x4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var t_layer: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> u: Uniforms;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // Two triangles, six vertices, covering layer-NDC [-1, +1]^2.
    // CCW order matches the pipeline's front-face setting.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
    );
    let corner = corners[vi];
    let world = u.transform * vec4<f32>(corner, 0.0, 1.0);
    var out: VsOut;
    out.pos = world;
    // UV: top-left origin → corner.y must be inverted.
    out.uv = vec2<f32>((corner.x + 1.0) * 0.5, 1.0 - (corner.y + 1.0) * 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(t_layer, samp, in.uv);
}
