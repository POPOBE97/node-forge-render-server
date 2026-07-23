
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
    // Node: FloatInput_42
    float_input_42: vec4f,
    // Node: Vector2Input_35
    node_Vector2Input_35_093d3fbd: vec4f,
    // Node: Vector2Input_36
    node_Vector2Input_36_f0373fbd: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_ImageTexture_InputBarUI: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_ImageTexture_InputBarUI: sampler;


// --- Extra WGSL declarations (generated) ---

struct ShaderMaterialInput {
    uv: vec2f,
    frag_coord: vec2f,
    local_position: vec3f,
    geometry_size: vec2f,
    target_size: vec2f,
    time: f32,
};

fn shader_material_ShaderMaterial_InputBarUI(
    in: ShaderMaterialInput,
    ui_color: vec4f,
    opacity: f32,
) -> vec4f {
    return ui_color * clamp(opacity, 0.0, 1.0);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // ImageTexture ImageTexture_InputBarUI.color
    let image_texture_sample = textureSample(
        img_tex_ImageTexture_InputBarUI,
        img_samp_ImageTexture_InputBarUI,
        (in.uv),
    );
    // Shader Material ShaderMaterial_InputBarUI.material
    let shader_material_material = shader_material_ShaderMaterial_InputBarUI(
        ShaderMaterialInput(in.uv, in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        image_texture_sample,
        (graph_inputs.float_input_42).x,
    );
    // Final composite
    let _frag_out = shader_material_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
