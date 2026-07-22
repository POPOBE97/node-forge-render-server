
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
    // Node: Vector2Input_35
    node_Vector2Input_35_093d3fbd: vec4f,
    // Node: Vector2Input_36
    node_Vector2Input_36_f0373fbd: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

struct ShaderMaterialParams {
    shader_GroupInstance_51_ShaderMaterial_InputBarUI_opacity: vec4f,
};
@group(0) @binding(3)
var<storage, read> shader_material_params: ShaderMaterialParams;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_GroupInstance_51_ImageTexture_InputBarUI: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_GroupInstance_51_ImageTexture_InputBarUI: sampler;


// --- Extra WGSL declarations (generated) ---

struct ShaderMaterialInput {
    uv: vec2f,
    frag_coord: vec2f,
    local_position: vec3f,
    geometry_size: vec2f,
    target_size: vec2f,
    time: f32,
};

fn shader_material_GroupInstance_51_ShaderMaterial_InputBarUI(
    in: ShaderMaterialInput,
    ui_color: vec4f,
    opacity: f32,
) -> vec4f {
    return ui_color * clamp(opacity, 0.0, 1.0);
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

 let rect_size_px_base = (graph_inputs.node_Vector2Input_35_093d3fbd).xy;
 let rect_center_px = (graph_inputs.node_Vector2Input_36_f0373fbd).xy;
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
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // ImageTexture GroupInstance_51/ImageTexture_InputBarUI.color
    let image_texture_sample = textureSample(
        img_tex_GroupInstance_51_ImageTexture_InputBarUI,
        img_samp_GroupInstance_51_ImageTexture_InputBarUI,
        (in.uv),
    );
    // Shader Material GroupInstance_51/ShaderMaterial_InputBarUI.material
    let shader_material_material = shader_material_GroupInstance_51_ShaderMaterial_InputBarUI(
        ShaderMaterialInput(in.uv, in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        image_texture_sample,
        (shader_material_params.shader_GroupInstance_51_ShaderMaterial_InputBarUI_opacity).x,
    );
    // Final composite
    let _frag_out = shader_material_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
