
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
    camera_position: vec4f,
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
var pass_tex_Downsample_10: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_Downsample_10: sampler;


// --- Extra WGSL declarations (generated) ---
fn mc_math_closure(uv: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    let _e6: vec2<f32> = uv_1;
    let _e7: vec4<f32> = sample_pass_texture(_e6);
    output = _e7;
    let _e8: vec4<f32> = output;
    return _e8;
}

fn sample_pass_texture(uv_in: vec2f) -> vec4f {
    return textureSample(pass_tex_Downsample_10, pass_samp_Downsample_10, uv_in);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    var math_closure_out: vec4f;
    {
        var output: vec4f;
        output = mc_math_closure(in.uv);
        math_closure_out = output;
    }
    // Final composite
    let _frag_out = math_closure_out;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
