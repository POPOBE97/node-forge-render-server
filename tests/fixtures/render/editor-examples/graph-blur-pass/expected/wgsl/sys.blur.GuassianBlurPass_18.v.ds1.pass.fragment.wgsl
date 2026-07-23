
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
 let k = array<f32, 8>(0.202875838, 0.190085858, 0.0817791, 0.021707874, 0.003551335, 0, 0, 0);
 let o = array<f32, 8>(0.652716517, 2.422937155, 4.363707066, 6.308340073, 8.257984161, 0, 0, 0);
 var color = vec4f(0.0);
 for (var i: u32 = 0u; i < 8u; i = i + 1u) {
     let uv_pos = (xy + vec2f(0.0, o[i])) / original;
     let uv_neg = (xy - vec2f(0.0, o[i])) / original;
     color = color + textureSampleLevel(src_tex, src_samp, uv_pos, 0.0) * k[i];
     color = color + textureSampleLevel(src_tex, src_samp, uv_neg, 0.0) * k[i];
 }
 return color;

}
