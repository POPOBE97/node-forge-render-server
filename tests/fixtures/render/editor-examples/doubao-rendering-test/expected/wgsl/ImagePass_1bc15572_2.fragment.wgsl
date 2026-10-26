
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
    // Node: Vector2Input_1bc15572_3
    node_Vector2Input_1bc15572_3_5f48e4b9: vec4f,
    // Node: Vector2Input_f90af5a3_4
    node_Vector2Input_f90af5a3_4_5595d036: vec4f,
    // Node: sys.image-pass.ImagePass_1bc15572_2.alpha.default
    node_sys_image_pass_ImagePass_1bc15572_2_alpha_default_dcb56728: vec4f,
    // Node: sys.image-pass.ImagePass_1bc15572_2.alpha.max
    node_sys_image_pass_ImagePass_1bc15572_2_alpha_max_e373932d: vec4f,
    // Node: sys.image-pass.ImagePass_1bc15572_2.alpha.min
    node_sys_image_pass_ImagePass_1bc15572_2_alpha_min_9dddab2d: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_ImageTexture_1bc15572_1: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_ImageTexture_1bc15572_1: sampler;


// --- Extra WGSL declarations (generated) ---
fn nf_math_closure_fragment_closure(uv_2: vec2<f32>) -> vec4<f32> {
    var uv_3: vec2<f32>;
    var output: vec4<f32>;

    uv_3 = uv_2;
    let _e4: vec2<f32> = uv_3;
    let _e5: vec4<f32> = nf_math_closure_fragment_closure_sample_image_texture(_e4);
    output = _e5;
    let _e6: vec4<f32> = output;
    return _e6;
}

fn nf_math_closure_fragment_closure_sample_image_texture(uv: vec2f) -> vec4f {
    let raster_uv = vec2f(uv.x, 1.0 - uv.y);
    return textureSample(img_tex_ImageTexture_1bc15572_1, img_samp_ImageTexture_1bc15572_1, raster_uv);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    let math_closure_out = nf_math_closure_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y));
    // MathClamp sys.image-pass.ImagePass_1bc15572_2.alpha.clamp.result
    let math_clamp = clamp(
        (graph_inputs.node_sys_image_pass_ImagePass_1bc15572_2_alpha_default_dcb56728).x,
        (graph_inputs.node_sys_image_pass_ImagePass_1bc15572_2_alpha_min_9dddab2d).x,
        (graph_inputs.node_sys_image_pass_ImagePass_1bc15572_2_alpha_max_e373932d).x,
    );
    // Final composite
    let _frag_out = (math_closure_out * vec4f(math_clamp));
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
