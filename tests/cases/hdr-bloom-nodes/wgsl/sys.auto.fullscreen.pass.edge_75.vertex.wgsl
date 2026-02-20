
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
     @location(2) local_px: vec3f,
     // Geometry size in pixels after applying geometry/instance transforms.
     @location(3) geo_size_px: vec2f,
  };


@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_RenderPass_4: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_RenderPass_4: sampler;

@group(1) @binding(2)
var pass_tex_Upsample_41: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_Upsample_41: sampler;


// --- Extra WGSL declarations (generated) ---
fn mc_MathClosure_42_(uv: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);
    var c_l0_: vec4<f32>;
    var c_l1_: vec4<f32>;

    uv_1 = uv;
    let _e6: vec2<f32> = uv_1;
    let _e7: vec4<f32> = sample_pass_RenderPass_4_(_e6);
    c_l0_ = _e7;
    let _e10: vec2<f32> = uv_1;
    let _e11: vec4<f32> = sample_pass_Upsample_41_(_e10);
    c_l1_ = _e11;
    let _e13: vec4<f32> = c_l0_;
    let _e14: vec4<f32> = c_l1_;
    output = (_e13 + _e14);
    let _e16: vec4<f32> = output;
    return _e16;
}

fn sample_pass_RenderPass_4_(uv_in: vec2f) -> vec4f {
    return textureSample(pass_tex_RenderPass_4, pass_samp_RenderPass_4, uv_in);
}

fn sample_pass_Upsample_41_(uv_in: vec2f) -> vec4f {
    return textureSample(pass_tex_Upsample_41, pass_samp_Upsample_41, uv_in);
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

 let p_local = position;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = params.center + p_local.xy;

 // Convert pixels to clip space assuming bottom-left origin.
 // (0,0) => (-1,-1), (target_size) => (1,1)
 let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);
 out.position = vec4f(ndc, p_local.z / params.target_size.x, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }