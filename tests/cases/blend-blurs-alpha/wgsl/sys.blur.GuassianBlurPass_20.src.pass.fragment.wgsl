
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,

    geo_translate: vec2f,
    geo_scale: vec2f,

    // Pack to 16-byte boundary.
    time: f32,
    _pad0: f32,

    // 16-byte aligned.
    color: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

 struct VSOut {
     @builtin(position) position: vec4f,
     @location(0) uv: vec2f,
     // GLSL-like gl_FragCoord.xy: bottom-left origin, pixel-centered.
     @location(1) frag_coord_gl: vec2f,
     // Geometry-local pixel coordinate (GeoFragcoord): origin at bottom-left.
     @location(2) local_px: vec2f,
     // Geometry size in pixels after applying geometry/instance transforms.
     @location(3) geo_size_px: vec2f,
 };
@group(1) @binding(0)
var pass_tex_node_2: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_node_2: sampler;


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    return textureSample(pass_tex_node_2, pass_samp_node_2, vec2f((in.uv).x, 1.0 - (in.uv).y));
}
