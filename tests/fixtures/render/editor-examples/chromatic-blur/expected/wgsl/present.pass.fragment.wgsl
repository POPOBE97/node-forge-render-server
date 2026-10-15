
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
    // Node: gaussian.mask
    gaussian_mask: vec4f,
    // Node: preview.center
    preview_center: vec4f,
    // Node: radial.mask
    radial_mask: vec4f,
    // Node: rgb.mask
    rgb_mask: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

struct ShaderMaterialParams {
    shader_present_material_preview: vec4i,
};
@group(0) @binding(3)
var<storage, read> shader_material_params: ShaderMaterialParams;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_source_image: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_source_image: sampler;

@group(1) @binding(2)
var pass_tex_rgb_sampler: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_rgb_sampler: sampler;

@group(1) @binding(4)
var pass_tex_radial_sampler: texture_2d<f32>;

@group(1) @binding(5)
var pass_samp_radial_sampler: sampler;

@group(1) @binding(6)
var pass_tex_gaussian_sampler: texture_2d<f32>;

@group(1) @binding(7)
var pass_samp_gaussian_sampler: sampler;


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

// All sampled RGB and intermediate results are scene-linear, premultiplied RGBA.
// Public UV coordinates are bottom-left/Y-up. Texture storage conversion belongs
// exclusively to sample_texture_local_uv, supplied by the renderer.
fn read_border_present_material(image: texture_2d<f32>, image_sampler: sampler, uv: vec2f) -> vec4f {
  let value = sample_texture_local_uv(image, image_sampler, uv);
  return select(value, vec4f(0.0), any(uv < vec2f(0.0)) || any(uv > vec2f(1.0)));
}

fn radial_mask_present_material(uv: vec2f, center_local_uv: vec2f, mask: vec4f) -> f32 {
  let distance = length((uv - center_local_uv) * vec2f(1260.0, 2800.0));
  let outer = max(mask.x, 1.0);
  let inner = outer * (1.0 - clamp(mask.y, 0.001, 1.0));
  let outside = smoothstep(inner, outer, distance);
  return pow(select(1.0 - outside, outside, mask.w > 0.5), max(mask.z, 0.01));
}

// Present stays linear. The renderer owns the final display/export sRGB encoding.
fn shader_material_present_material(
  in: ShaderMaterialInput,
  source: texture_2d<f32>, source_sampler: sampler,
  original: texture_2d<f32>, original_sampler: sampler,
  radial: texture_2d<f32>, radial_sampler: sampler,
  gaussian: texture_2d<f32>, gaussian_sampler: sampler,
  previewCenterLocalUV: vec2f,
  radialMask: vec4f, gaussianMask: vec4f, rgbMask: vec4f,
  preview: i32,
) -> vec4f {
  let final_value = read_border_present_material(source, source_sampler, in.uv);
  let original_value = read_border_present_material(original, original_sampler, in.uv);
  let radial_value = read_border_present_material(radial, radial_sampler, in.uv);
  let gaussian_value = read_border_present_material(gaussian, gaussian_sampler, in.uv);
  switch preview {
    case 1: { return original_value; }
    case 2: { return radial_value; }
    case 3: { return gaussian_value; }
    case 4: { return vec4f(vec3f(radial_mask_present_material(in.uv, previewCenterLocalUV, radialMask)), 1.0); }
    case 5: { return vec4f(vec3f(radial_mask_present_material(in.uv, previewCenterLocalUV, gaussianMask)), 1.0); }
    case 6: { return vec4f(vec3f(radial_mask_present_material(in.uv, previewCenterLocalUV, rgbMask)), 1.0); }
    default: { return final_value; }
  }
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Shader Material present.material.material
    let shader_material_material = shader_material_present_material(
        ShaderMaterialInput(vec2f(in.uv.x, 1.0 - in.uv.y), in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        pass_tex_rgb_sampler,
        pass_samp_rgb_sampler,
        img_tex_source_image,
        img_samp_source_image,
        pass_tex_radial_sampler,
        pass_samp_radial_sampler,
        pass_tex_gaussian_sampler,
        pass_samp_gaussian_sampler,
        (graph_inputs.preview_center).xy,
        (graph_inputs.radial_mask),
        (graph_inputs.gaussian_mask),
        (graph_inputs.rgb_mask),
        (shader_material_params.shader_present_material_preview).x,
    );
    // Final composite
    let _frag_out = shader_material_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
