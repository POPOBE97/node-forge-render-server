
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

var src_tex: texture_2d<f32>;
@group(1) @binding(1)
var src_samp: sampler;

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    
let original = vec2f(textureDimensions(src_tex));
let xy = vec2f(in.position.xy);
let k = array<f32, 8>(0.160921663, 0.172244444, 0.102648713, 0.045504373, 0.015002778, 0.003678028, 0, 0);
let o = array<f32, 8>(0.658242106, 2.453042507, 4.41602993, 6.37994051, 8.345131874, 10.311907768, 0, 0);
var color = vec4f(0.0);
for (var i: u32 = 0u; i < 8u; i = i + 1u) {
    let uv_pos = (xy + vec2f(0.0, o[i])) / original;
    let uv_neg = (xy - vec2f(0.0, o[i])) / original;
    color = color + textureSampleLevel(src_tex, src_samp, uv_pos, 0.0) * k[i];
    color = color + textureSampleLevel(src_tex, src_samp, uv_neg, 0.0) * k[i];
}
return color;

}
