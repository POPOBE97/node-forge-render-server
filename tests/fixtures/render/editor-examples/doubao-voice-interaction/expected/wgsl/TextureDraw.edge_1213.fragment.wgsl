
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
    // Node: FloatInput_CausticBlurRadiusPx
    node_FloatInput_CausticBlurRadiusPx_6b056aea: vec4f,
    // Node: GroupInstance_60/Vector2Input_ca3c5000_60
    node_GroupInstance_60_Vector2Input_ca3c5000_60_133ed924: vec4f,
    // Node: Vector2Input_35
    node_Vector2Input_35_093d3fbd: vec4f,
    // Node: Vector2Input_36
    node_Vector2Input_36_f0373fbd: vec4f,
    // Node: sys.image-pass.TextureDraw.edge_1213.alpha.default
    node_sys_image_pass_TextureDraw_edge_1213_alpha_default_fcc3f063: vec4f,
    // Node: sys.image-pass.TextureDraw.edge_1213.alpha.max
    node_sys_image_pass_TextureDraw_edge_1213_alpha_max_0389bfb8: vec4f,
    // Node: sys.image-pass.TextureDraw.edge_1213.alpha.min
    node_sys_image_pass_TextureDraw_edge_1213_alpha_min_bd3fd6b8: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_GroupInstance_60_GuassianBlurPass_b9da1676_59: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_GroupInstance_60_GuassianBlurPass_b9da1676_59: sampler;


// --- Extra WGSL declarations (generated) ---
fn mc_math_closure(uv: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    let _e6: vec2<f32> = uv_1;
    let _e7: vec4<f32> = sample_pass_image(_e6);
    output = _e7;
    let _e8: vec4<f32> = output;
    return _e8;
}

fn sample_pass_image(uv_in: vec2f) -> vec4f {
    return textureSample(pass_tex_GroupInstance_60_GuassianBlurPass_b9da1676_59, pass_samp_GroupInstance_60_GuassianBlurPass_b9da1676_59, uv_in);
}


// --- Extra WGSL declarations (generated) ---
fn mc_caustic_draw_size(uv: vec2<f32>, size: vec2<f32>, radius: f32) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var size_1: vec2<f32>;
    var radius_1: f32;
    var output: vec2<f32> = vec2(0f);

    uv_1 = uv;
    size_1 = size;
    radius_1 = radius;
    let _e10: vec2<f32> = size_1;
    let _e15: vec2<f32> = size_1;
    let _e23: f32 = radius_1;
    let _e28: f32 = radius_1;
    output = (max(round(_e15), vec2(1f)) + vec2((2f * ceil(max(_e28, 0f)))));
    let _e35: vec2<f32> = output;
    return _e35;
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    var math_closure_out: vec4f;
    {
        var output: vec4f;
        output = mc_math_closure(in.uv);
        math_closure_out = output;
    }
    // MathClamp sys.image-pass.TextureDraw.edge_1213.alpha.clamp.result
    let math_clamp = clamp(
        (graph_inputs.node_sys_image_pass_TextureDraw_edge_1213_alpha_default_fcc3f063).x,
        (graph_inputs.node_sys_image_pass_TextureDraw_edge_1213_alpha_min_bd3fd6b8).x,
        (graph_inputs.node_sys_image_pass_TextureDraw_edge_1213_alpha_max_0389bfb8).x,
    );
    // Final composite
    let _frag_out = (math_closure_out * vec4f(math_clamp));
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
