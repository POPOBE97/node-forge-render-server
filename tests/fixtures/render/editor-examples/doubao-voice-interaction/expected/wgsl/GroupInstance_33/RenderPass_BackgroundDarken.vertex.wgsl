
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
    // Node: FloatInput_44
    float_input_44: vec4f,
    // Node: GroupInstance_33/Vector2Input_BackgroundPosition
    node_GroupInstance_33_Vector2Input_BackgroundPosition_086664a9: vec4f,
    // Node: GroupInstance_33/Vector2Input_BackgroundSize
    node_GroupInstance_33_Vector2Input_BackgroundSize_ea00ccb0: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_GroupInstance_33_PassTexture_BackgroundBlur: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_GroupInstance_33_PassTexture_BackgroundBlur: sampler;


// --- Extra WGSL declarations (generated) ---

struct ShaderMaterialInput {
    uv: vec2f,
    frag_coord: vec2f,
    local_position: vec3f,
    geometry_size: vec2f,
    target_size: vec2f,
    time: f32,
};

fn shader_material_GroupInstance_33_ShaderMaterial_BackgroundDarken(
    in: ShaderMaterialInput,
    content_tex: texture_2d<f32>,
    content_sampler: sampler,
    darken_alpha: f32,
) -> vec4f {
    let content = textureSample(content_tex, content_sampler, in.uv);
    let a = clamp(darken_alpha, 0.0, 1.0);
    return vec4f(content.rgb * (1.0 - a), a + content.a * (1.0 - a));
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

 let rect_size_px_base = (graph_inputs.node_GroupInstance_33_Vector2Input_BackgroundSize_ea00ccb0).xy;
 let rect_center_px = (graph_inputs.node_GroupInstance_33_Vector2Input_BackgroundPosition_086664a9).xy;
 let rect_dyn = vec4f(rect_center_px, rect_size_px_base);
 out.geo_size_px = rect_dyn.zw;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, 0.0);

 let p_rect_local_px = vec3f(position.xy * rect_dyn.zw, position.z);
 var p_local = p_rect_local_px;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = rect_dyn.xy + p_local.xy;

 out.position = params.camera * vec4f(p_px, p_local.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }