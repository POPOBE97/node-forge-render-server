
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
    camera: mat4x4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

 struct VSOut {
     @builtin(position) position: vec4f,
     @location(0) uv: vec2f,
     // GLSL-like gl_FragCoord.xy: bottom-left origin, pixel-centered.
     @location(1) frag_coord_gl: vec2f,
     // Geometry-local pixel coordinate (GeoFragcoord): origin at bottom-left.
     @location(2) local_px: vec3f,
     // Geometry size in pixels after applying geometry/instance transforms.
     @location(3) geo_size_px: vec2f,
  };


@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;

// --- Extra WGSL declarations (generated) ---
fn mc_MathClosure_8_(uv: vec2<f32>, input1_: vec2<f32>, input2_: vec2<f32>, input3_: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var input1_1: vec2<f32>;
    var input2_1: vec2<f32>;
    var input3_1: f32;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    input1_1 = input1_;
    input2_1 = input2_;
    input3_1 = input3_;
    let _e11: vec2<f32> = input1_1;
    let _e12: vec2<f32> = input2_1;
    let _e13: vec2<f32> = (_e11 / _e12);
    let _e14: f32 = input3_1;
    let _e17: f32 = input3_1;
    let _e23: f32 = input3_1;
    let _e26: f32 = input3_1;
    output = vec4<f32>(_e13.x, _e13.y, abs((fract((_e26 / 2f)) - 0.5f)), 1f);
    let _e37: vec4<f32> = output;
    return _e37;
}


 @vertex
 fn vs_main(
     @location(0) position: vec3f,
     @location(1) uv: vec2f,
 ) -> VSOut {
 var out: VSOut;

 let _unused_geo_size = params.geo_size;
 let _unused_geo_translate = params.geo_translate;
 let _unused_geo_scale = params.geo_scale;

 // UV passed as vertex attribute.
 out.uv = uv;

 out.geo_size_px = params.geo_size;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, 0.0);

 var p_local = position;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = params.center + p_local.xy;

 out.position = params.camera * vec4f(p_px, p_local.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_MathClosure_8_out: vec4f;
    {
        let input1 = in.local_px.xy;
        let input2 = in.geo_size_px;
        let input3 = params.time;
        var output: vec4f;
        output = mc_MathClosure_8_(in.uv, input1, input2, input3);
        mc_MathClosure_8_out = output;
    }
    let _frag_out = mc_MathClosure_8_out;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
