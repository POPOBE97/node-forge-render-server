
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
    // Node: ColorInput_PttPromptColor
    color_input_ptt_prompt_color: vec4f,
    // Node: FloatInput_PttPromptCancel
    float_input_ptt_prompt_cancel: vec4f,
    // Node: FloatInput_PttPromptOpacity
    float_input_ptt_prompt_opacity: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_ImageTexture_PttPromptSend: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_ImageTexture_PttPromptSend: sampler;

@group(1) @binding(2)
var img_tex_ImageTexture_PttPromptCancel: texture_2d<f32>;

@group(1) @binding(3)
var img_samp_ImageTexture_PttPromptCancel: sampler;


// --- Extra WGSL declarations (generated) ---

struct ShaderMaterialInput {
    uv: vec2f,
    frag_coord: vec2f,
    local_position: vec3f,
    geometry_size: vec2f,
    target_size: vec2f,
    time: f32,
};

fn shader_material_ShaderMaterial_PttPrompt(
    in: ShaderMaterialInput,
    send_color: vec4f,
    cancel_color: vec4f,
    prompt_color: vec4f,
    prompt_opacity: f32,
    cancel_mix: f32,
) -> vec4f {
    let mask_alpha = mix(send_color.a, cancel_color.a, clamp(cancel_mix, 0.0, 1.0));
    let coverage = clamp(mask_alpha * prompt_color.a * prompt_opacity, 0.0, 1.0);
    return vec4f(prompt_color.rgb * 1.5 * coverage, coverage);
}


fn aspect_correct_uv_fit(uv: vec2f, img_dim: vec2f, geo_dim: vec2f) -> vec2f {
    // r = image_aspect / geo_aspect; r > 1 means image is relatively wider than geometry.
    let r = (img_dim.x * geo_dim.y) / (img_dim.y * geo_dim.x);
    let s = vec2f(max(1.0 / r, 1.0), max(r, 1.0));
    return (uv - vec2f(0.5)) * s + vec2f(0.5);
}
fn aspect_correct_uv_fill(uv: vec2f, img_dim: vec2f, geo_dim: vec2f) -> vec2f {
    let r = (img_dim.x * geo_dim.y) / (img_dim.y * geo_dim.x);
    let s = vec2f(min(1.0 / r, 1.0), min(r, 1.0));
    return (uv - vec2f(0.5)) * s + vec2f(0.5);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // ImageTexture ImageTexture_PttPromptSend aspect-correct uv
    let image_texture_uv = aspect_correct_uv_fill(
        (in.uv),
        vec2f(textureDimensions(img_tex_ImageTexture_PttPromptSend)),
        in.geo_size_px,
    );
    // ImageTexture ImageTexture_PttPromptSend.color
    let image_texture_sample = textureSample(
        img_tex_ImageTexture_PttPromptSend,
        img_samp_ImageTexture_PttPromptSend,
        image_texture_uv,
    );
    // ImageTexture ImageTexture_PttPromptCancel aspect-correct uv
    let image_texture_uv_f3a41dd1 = aspect_correct_uv_fill(
        (in.uv),
        vec2f(textureDimensions(img_tex_ImageTexture_PttPromptCancel)),
        in.geo_size_px,
    );
    // ImageTexture ImageTexture_PttPromptCancel.color
    let image_texture_sample_1a8f8444 = textureSample(
        img_tex_ImageTexture_PttPromptCancel,
        img_samp_ImageTexture_PttPromptCancel,
        image_texture_uv_f3a41dd1,
    );
    // Shader Material ShaderMaterial_PttPrompt.material
    let shader_material_material = shader_material_ShaderMaterial_PttPrompt(
        ShaderMaterialInput(in.uv, in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        image_texture_sample,
        image_texture_sample_1a8f8444,
        vec4f((graph_inputs.color_input_ptt_prompt_color).rgb * (graph_inputs.color_input_ptt_prompt_color).a, (graph_inputs.color_input_ptt_prompt_color).a),
        (graph_inputs.float_input_ptt_prompt_opacity).x,
        (graph_inputs.float_input_ptt_prompt_cancel).x,
    );
    // Final composite
    let _frag_out = shader_material_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
