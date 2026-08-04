
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
    // Public material UV: bottom-left origin, Y increasing upward.
    uv: vec2f,
    // Public scene/target pixel coordinate: bottom-left origin, Y increasing upward.
    frag_coord: vec2f,
    // Public geometry-local pixel coordinate: bottom-left origin, Y increasing upward.
    local_position: vec3f,
    geometry_size: vec2f,
    target_size: vec2f,
    time: f32,
};

// Renderer-owned texture boundary. ShaderMaterial authors provide LocalUV and never
// convert to WebGPU's private raster-texture convention themselves.
fn sample_texture_local_uv(
    source: texture_2d<f32>,
    source_sampler: sampler,
    local_uv: vec2f,
) -> vec4f {
    return textureSample(source, source_sampler, vec2f(local_uv.x, 1.0 - local_uv.y));
}

fn shader_material_GroupInstance_33_ShaderMaterial_BackgroundDarken(
    in: ShaderMaterialInput,
    content_tex: texture_2d<f32>,
    content_sampler: sampler,
    darken_alpha: f32,
) -> vec4f {
    let content = sample_texture_local_uv(content_tex, content_sampler, in.uv);
    let a = clamp(darken_alpha, 0.0, 1.0);
    return vec4f(content.rgb * (1.0 - a), a + content.a * (1.0 - a));
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Shader Material GroupInstance_33/ShaderMaterial_BackgroundDarken.material
    let background_darken_alpha_material = shader_material_GroupInstance_33_ShaderMaterial_BackgroundDarken(
        ShaderMaterialInput(vec2f(in.uv.x, 1.0 - in.uv.y), in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        pass_tex_GroupInstance_33_PassTexture_BackgroundBlur,
        pass_samp_GroupInstance_33_PassTexture_BackgroundBlur,
        (graph_inputs.float_input_44).x,
    );
    // Final composite
    let _frag_out = background_darken_alpha_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
