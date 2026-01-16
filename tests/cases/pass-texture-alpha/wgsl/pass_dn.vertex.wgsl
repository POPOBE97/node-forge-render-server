
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,
    time: f32,
    _pad0: f32,
    color: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    // GLSL-like gl_FragCoord.xy: bottom-left origin, pixel-centered.
    @location(1) frag_coord_gl: vec2f,
};
@group(1) @binding(0)
var pass_tex_pass_up: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_pass_up: sampler;


@vertex
fn vs_main(@location(0) position: vec3f) -> VSOut {
    var out: VSOut;

    // Local UV in [0,1] based on geometry size.
    out.uv = (position.xy / params.geo_size) + vec2f(0.5, 0.5);

    // Geometry vertices are in local pixel units centered at (0,0).
    // Convert to target pixel coordinates with bottom-left origin.
    let p_px = params.center + position.xy + (params.target_size * 0.5);

    // Convert pixels to clip space assuming bottom-left origin.
    // (0,0) => (-1,-1), (target_size) => (1,1)
    let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);
    out.position = vec4f(ndc, position.z, 1.0);

    // Pixel-centered like GLSL gl_FragCoord.xy.
    out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
    return out;
}
