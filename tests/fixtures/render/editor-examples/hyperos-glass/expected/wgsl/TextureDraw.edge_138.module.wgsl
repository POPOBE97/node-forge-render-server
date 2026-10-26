
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
    // Node: sys.image-pass.TextureDraw.edge_138.alpha.default
    sys_image_pass_texture_draw_edge_138_alpha_default: vec4f,
    // Node: sys.image-pass.TextureDraw.edge_138.alpha.max
    sys_image_pass_texture_draw_edge_138_alpha_max: vec4f,
    // Node: sys.image-pass.TextureDraw.edge_138.alpha.min
    sys_image_pass_texture_draw_edge_138_alpha_min: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_sys_group_BackgroundPass_GuassianBlurPass_42: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_sys_group_BackgroundPass_GuassianBlurPass_42: sampler;


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
    return textureSample(pass_tex_sys_group_BackgroundPass_GuassianBlurPass_42, pass_samp_sys_group_BackgroundPass_GuassianBlurPass_42, raster_uv);
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
 out.uv = vec2f(uv.x, 1.0 - uv.y);

 out.geo_size_px = params.geo_size;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(uv * out.geo_size_px, 0.0);

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
    let math_closure_out = nf_math_closure_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y));
    // MathClamp sys.image-pass.TextureDraw.edge_138.alpha.clamp.result
    let math_clamp = clamp(
        (graph_inputs.sys_image_pass_texture_draw_edge_138_alpha_default).x,
        (graph_inputs.sys_image_pass_texture_draw_edge_138_alpha_min).x,
        (graph_inputs.sys_image_pass_texture_draw_edge_138_alpha_max).x,
    );
    // Final composite
    let _frag_out = (math_closure_out * vec4f(math_clamp));
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
