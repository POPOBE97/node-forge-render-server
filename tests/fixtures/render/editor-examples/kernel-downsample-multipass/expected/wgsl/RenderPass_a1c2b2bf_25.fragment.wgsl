
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
    // Node: FloatInput_blur_sigma_max
    float_input_blur_sigma_max: vec4f,
    // Node: FloatInput_blur_sigma_min
    float_input_blur_sigma_min: vec4f,
    // Node: FloatInput_blur_y_max
    float_input_blur_y_max: vec4f,
    // Node: FloatInput_blur_y_min
    float_input_blur_y_min: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_ImageTexture_d5db2d53_12: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_ImageTexture_d5db2d53_12: sampler;

@group(1) @binding(2)
var pass_tex_Downsample_gaussian3_d1: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_Downsample_gaussian3_d1: sampler;

@group(1) @binding(4)
var pass_tex_Downsample_gaussian3_d2: texture_2d<f32>;

@group(1) @binding(5)
var pass_samp_Downsample_gaussian3_d2: sampler;

@group(1) @binding(6)
var pass_tex_Downsample_gaussian3_d3: texture_2d<f32>;

@group(1) @binding(7)
var pass_samp_Downsample_gaussian3_d3: sampler;

@group(1) @binding(8)
var pass_tex_Downsample_gaussian3_d4: texture_2d<f32>;

@group(1) @binding(9)
var pass_samp_Downsample_gaussian3_d4: sampler;

@group(1) @binding(10)
var pass_tex_Downsample_gaussian3_d5: texture_2d<f32>;

@group(1) @binding(11)
var pass_samp_Downsample_gaussian3_d5: sampler;

@group(1) @binding(12)
var pass_tex_Downsample_gaussian3_d6: texture_2d<f32>;

@group(1) @binding(13)
var pass_samp_Downsample_gaussian3_d6: sampler;


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

// Variable blur reconstruction for Gaussian 3x3.
// Effective cumulative variances (downsampling + cubic reconstruction): 0, 2.083333, 9.083333, 37.083333, 149.083333, 597.083333, 2389.083333.
// Endpoint sigmas use log2(sigma * 1.309375844); the Y ramp interpolates LOD directly.
fn cubic_reconstruct_ShaderMaterial_gaussian3(
    tex: texture_2d<f32>,
    tex_sampler: sampler,
    uv: vec2f
) -> vec4f {
    let resolution = vec2f(textureDimensions(tex));
    let d = uv * resolution - 0.5;
    let c = floor(d);
    let x = c - d + 1.0;
    let X = d - c;
    let x3 = x * x * x;
    let coeff = 0.5 * x * x + 0.5 * x + 0.166667;
    let w1 = -0.333333 * x3 + coeff;
    let w2 = 1.0 - w1;
    let o1 = (-0.5 * x3 + coeff) / w1 + c - 0.5;
    let o2 = (X * X * X / 6.0) / w2 + c + 1.5;

    let c00 = sample_texture_local_uv(
        tex,
        tex_sampler,
        vec2f(o1.x, o1.y) / resolution
    );
    let c10 = sample_texture_local_uv(
        tex,
        tex_sampler,
        vec2f(o2.x, o1.y) / resolution
    );
    let c01 = sample_texture_local_uv(
        tex,
        tex_sampler,
        vec2f(o1.x, o2.y) / resolution
    );
    let c11 = sample_texture_local_uv(
        tex,
        tex_sampler,
        vec2f(o2.x, o2.y) / resolution
    );

    return w1.x * w1.y * c00
        + w2.x * w1.y * c10
        + w1.x * w2.y * c01
        + w2.x * w2.y * c11;
}

fn shader_material_ShaderMaterial_gaussian3(
    in: ShaderMaterialInput,
    level0: texture_2d<f32>,
    level0_sampler: sampler,
    level1: texture_2d<f32>,
    level1_sampler: sampler,
    level2: texture_2d<f32>,
    level2_sampler: sampler,
    level3: texture_2d<f32>,
    level3_sampler: sampler,
    level4: texture_2d<f32>,
    level4_sampler: sampler,
    level5: texture_2d<f32>,
    level5_sampler: sampler,
    level6: texture_2d<f32>,
    level6_sampler: sampler,
    sigma_min: f32,
    sigma_max: f32,
    y_min: f32,
    y_max: f32
) -> vec4f {
    let y_span = max(y_max - y_min, 0.000001);
    let progress = clamp((1.0 - in.uv.y - y_min) / y_span, 0.0, 1.0);
    var min_lod = 0.0;
    if (sigma_min > 0.0) {
        min_lod = clamp(log2(sigma_min * 1.309375844), 0.0, 6.0);
    }
    var max_lod = 0.0;
    if (sigma_max > 0.0) {
        max_lod = clamp(log2(sigma_max * 1.309375844), 0.0, 6.0);
    }
    let lod = mix(min_lod, max_lod, progress);

    let lower_lod = floor(lod);
    let alpha = lod - lower_lod;
    var lower_color: vec4f;
    var upper_color: vec4f;

    if (lower_lod < 0.5) {
        lower_color = sample_texture_local_uv(level0, level0_sampler, in.uv);
        upper_color = cubic_reconstruct_ShaderMaterial_gaussian3(level1, level1_sampler, in.uv);
    } else if (lower_lod < 1.5) {
        lower_color = cubic_reconstruct_ShaderMaterial_gaussian3(level1, level1_sampler, in.uv);
        upper_color = cubic_reconstruct_ShaderMaterial_gaussian3(level2, level2_sampler, in.uv);
    } else if (lower_lod < 2.5) {
        lower_color = cubic_reconstruct_ShaderMaterial_gaussian3(level2, level2_sampler, in.uv);
        upper_color = cubic_reconstruct_ShaderMaterial_gaussian3(level3, level3_sampler, in.uv);
    } else if (lower_lod < 3.5) {
        lower_color = cubic_reconstruct_ShaderMaterial_gaussian3(level3, level3_sampler, in.uv);
        upper_color = cubic_reconstruct_ShaderMaterial_gaussian3(level4, level4_sampler, in.uv);
    } else if (lower_lod < 4.5) {
        lower_color = cubic_reconstruct_ShaderMaterial_gaussian3(level4, level4_sampler, in.uv);
        upper_color = cubic_reconstruct_ShaderMaterial_gaussian3(level5, level5_sampler, in.uv);
    } else if (lower_lod < 5.5) {
        lower_color = cubic_reconstruct_ShaderMaterial_gaussian3(level5, level5_sampler, in.uv);
        upper_color = cubic_reconstruct_ShaderMaterial_gaussian3(level6, level6_sampler, in.uv);
    } else {
        lower_color = cubic_reconstruct_ShaderMaterial_gaussian3(level6, level6_sampler, in.uv);
        upper_color = lower_color;
    }

    return mix(lower_color, upper_color, alpha);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Shader Material ShaderMaterial_gaussian3.material
    let shader_material_material = shader_material_ShaderMaterial_gaussian3(
        ShaderMaterialInput(vec2f(in.uv.x, 1.0 - in.uv.y), in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        img_tex_ImageTexture_d5db2d53_12,
        img_samp_ImageTexture_d5db2d53_12,
        pass_tex_Downsample_gaussian3_d1,
        pass_samp_Downsample_gaussian3_d1,
        pass_tex_Downsample_gaussian3_d2,
        pass_samp_Downsample_gaussian3_d2,
        pass_tex_Downsample_gaussian3_d3,
        pass_samp_Downsample_gaussian3_d3,
        pass_tex_Downsample_gaussian3_d4,
        pass_samp_Downsample_gaussian3_d4,
        pass_tex_Downsample_gaussian3_d5,
        pass_samp_Downsample_gaussian3_d5,
        pass_tex_Downsample_gaussian3_d6,
        pass_samp_Downsample_gaussian3_d6,
        (graph_inputs.float_input_blur_sigma_min).x,
        (graph_inputs.float_input_blur_sigma_max).x,
        (graph_inputs.float_input_blur_y_min).x,
        (graph_inputs.float_input_blur_y_max).x,
    );
    // Final composite
    let _frag_out = shader_material_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
