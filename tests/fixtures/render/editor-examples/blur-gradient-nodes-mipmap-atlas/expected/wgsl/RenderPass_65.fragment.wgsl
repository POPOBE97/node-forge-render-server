
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
    // Node: Vector2Input_85
    node_Vector2Input_85_1aea53bd: vec4f,
    // Node: Vector2Input_86
    node_Vector2Input_86_67e853bd: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_PassTexture_66: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_PassTexture_66: sampler;


// --- Extra WGSL declarations (generated) ---
fn mc_math_closure(uv: vec2<f32>, xy: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var size_1: vec2<f32>;
    var output: vec2<f32> = vec2(0f);

    uv_1 = uv;
    xy_1 = xy;
    size_1 = size;
    let _e9: vec2<f32> = xy_1;
    let _e10: vec2<f32> = size_1;
    output = (_e9 / _e10);
    let _e12: vec2<f32> = output;
    return _e12;
}

fn mc_math_closure_58ac04c5_(uv: vec2<f32>, uv_1: vec2<f32>) -> vec4<f32> {
    var uv_2: vec2<f32>;
    var uv_3: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_2 = uv;
    uv_3 = uv_1;
    let _e8: vec2<f32> = uv_3;
    let _e9: vec4<f32> = sample_pass_texture(_e8);
    output = _e9;
    let _e10: vec4<f32> = output;
    return _e10;
}

fn sample_pass_texture(uv_in: vec2f) -> vec4f {
    return textureSample(pass_tex_PassTexture_66, pass_samp_PassTexture_66, uv_in);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    var math_closure_out_4850b225: vec2f;
    {
        let xy = in.local_px.xy;
        let size = in.geo_size_px;
        var output: vec2f;
        output = mc_math_closure(in.uv, xy, size);
        math_closure_out_4850b225 = output;
    }
    var math_closure_out: vec4f;
    {
        let uv = math_closure_out_4850b225;
        var output: vec4f;
        output = mc_math_closure_58ac04c5_(in.uv, uv);
        math_closure_out = output;
    }
    // Final composite
    let _frag_out = math_closure_out;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
