
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
};

@group(1) @binding(0)
var src_tex: texture_2d<f32>;
@group(1) @binding(1)
var src_samp: sampler;

 @vertex
  fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {
      var out: VSOut;
 
      let _unused_geo_size = params.geo_size;
      let _unused_geo_translate = params.geo_translate;
     let _unused_geo_scale = params.geo_scale;
 
      // UV passed as vertex attribute.
      out.uv = uv;
 
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
 
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    
let original = vec2f(textureDimensions(src_tex));
let xy = vec2f(in.position.xy);
let k = array<f32, 8>(0.236040637, 0.193577051, 0.060631454, 0.00975086, 0, 0, 0, 0);
let o = array<f32, 8>(0.647165358, 2.393475771, 4.314556122, 6.245069504, 0, 0, 0, 0);
var color = vec4f(0.0);
for (var i: u32 = 0u; i < 8u; i = i + 1u) {
    let uv_pos = (xy + vec2f(o[i], 0.0)) / original;
    let uv_neg = (xy - vec2f(o[i], 0.0)) / original;
    color = color + textureSampleLevel(src_tex, src_samp, uv_pos, 0.0) * k[i];
    color = color + textureSampleLevel(src_tex, src_samp, uv_neg, 0.0) * k[i];
}
return color;

}
