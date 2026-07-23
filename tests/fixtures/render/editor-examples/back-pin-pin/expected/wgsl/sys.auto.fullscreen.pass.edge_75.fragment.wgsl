
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
    let _e18: vec4<f32> = c_l0_;
    let _e19: vec4<f32> = c_l1_;
    output = clamp((_e18 + _e19), vec4(0f), vec4(1f));
    let _e26: vec4<f32> = output;
    return _e26;
}

fn sample_pass_RenderPass_4_(uv_in: vec2f) -> vec4f {
    return textureSample(pass_tex_RenderPass_4, pass_samp_RenderPass_4, uv_in);
}

fn sample_pass_Upsample_41_(uv_in: vec2f) -> vec4f {
    return textureSample(pass_tex_Upsample_41, pass_samp_Upsample_41, uv_in);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_MathClosure_42_out: vec4f;
    {
        var output: vec4f;
        output = mc_MathClosure_42_(in.uv);
        mc_MathClosure_42_out = output;
    }
    return mc_MathClosure_42_out;
}
