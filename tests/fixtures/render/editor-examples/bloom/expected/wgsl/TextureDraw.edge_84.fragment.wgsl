
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
    // Node: sys.image-pass.TextureDraw.edge_84.alpha.default
    sys_image_pass_texture_draw_edge_84_alpha_default: vec4f,
    // Node: sys.image-pass.TextureDraw.edge_84.alpha.max
    sys_image_pass_texture_draw_edge_84_alpha_max: vec4f,
    // Node: sys.image-pass.TextureDraw.edge_84.alpha.min
    sys_image_pass_texture_draw_edge_84_alpha_min: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_TextureSampler_edge_84: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_TextureSampler_edge_84: sampler;


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
    return textureSample(pass_tex_TextureSampler_edge_84, pass_samp_TextureSampler_edge_84, raster_uv);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    let math_closure_out = nf_math_closure_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y));
    // MathClamp sys.image-pass.TextureDraw.edge_84.alpha.clamp.result
    let math_clamp = clamp(
        (graph_inputs.sys_image_pass_texture_draw_edge_84_alpha_default).x,
        (graph_inputs.sys_image_pass_texture_draw_edge_84_alpha_min).x,
        (graph_inputs.sys_image_pass_texture_draw_edge_84_alpha_max).x,
    );
    // Final composite
    let _frag_out = (math_closure_out * vec4f(math_clamp));
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
