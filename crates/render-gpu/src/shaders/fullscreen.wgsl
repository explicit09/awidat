// Fullscreen-triangle vertex shader shared by every transition shader.
//
// Three vertices cover the entire NDC quad. Vertex 0 is (-1,-1),
// vertex 1 is (3,-1), vertex 2 is (-1, 3); the two off-screen vertices
// get clipped, leaving a single big triangle that exactly fills the
// viewport. We pass UV in `[0, 1]^2` with origin at the top-left,
// matching how wgpu uploads texture data row-by-row.

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var x: f32 = -1.0;
    var y: f32 = -1.0;
    if (vi == 1u) {
        x = 3.0;
    } else if (vi == 2u) {
        y = 3.0;
    }
    var out: VsOut;
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    // Flip Y so UV (0,0) is the top-left of the texture.
    out.uv = vec2<f32>((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}
