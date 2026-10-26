
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


struct GraphInputs {
    // Node: Vector2Input_87
    node_Vector2Input_87_b4e653bd: vec4f,
    // Node: Vector2Input_88
    node_Vector2Input_88_69d753bd: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_PassTexture_70: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_PassTexture_70: sampler;


// --- Extra WGSL declarations (generated) ---
fn nf_math_closure_fragment_b18cf88f_closure(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var size_1: vec2<f32>;
    var output: vec2<f32>;

    uv_1 = uv;
    xy_1 = xy;
    size_1 = size;
    let _e7: vec2<f32> = xy_1;
    let _e8: vec2<f32> = size_1;
    output = (_e7 / _e8);
    let _e10: vec2<f32> = output;
    return _e10;
}

fn nf_math_closure_fragment_closure(uv_2: vec2<f32>, sampleUV: vec2<f32>) -> vec4<f32> {
    var uv_3: vec2<f32>;
    var sampleUV_1: vec2<f32>;
    var output: vec4<f32>;

    uv_3 = uv_2;
    sampleUV_1 = sampleUV;
    let _e6: vec2<f32> = sampleUV_1;
    let _e7: vec4<f32> = nf_math_closure_fragment_closure_sample_texture_texture(_e6);
    output = vec4<f32>(_e7);
    let _e9: vec4<f32> = output;
    return _e9;
}

fn nf_math_closure_fragment_closure_sample_texture_texture(uv: vec2f) -> vec4f {
    let raster_uv = vec2f(uv.x, 1.0 - uv.y);
    return textureSample(pass_tex_PassTexture_70, pass_samp_PassTexture_70, raster_uv);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    let math_closure_out = nf_math_closure_fragment_b18cf88f_closure(vec2f(in.uv.x, 1.0 - in.uv.y), in.local_px.xy, in.geo_size_px);
    let math_closure_out_cea110ae = nf_math_closure_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y), math_closure_out);
    // Final composite
    let _frag_out = math_closure_out_cea110ae;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
