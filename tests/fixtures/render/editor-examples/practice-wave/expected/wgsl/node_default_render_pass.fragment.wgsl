
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
    // Node: FloatInput_19ad5082_7
    float_input_19ad5082_7: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_ProceduralTestTexture_bdf48f62_1: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_ProceduralTestTexture_bdf48f62_1: sampler;


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

// Sampling stays separate from the Wave field so height, dir and normal can
// also drive other math, PBRMaterial.normal or GlassMaterial.uNormal.
fn shader_material_ShaderMaterial_bdf48f62_2(
    in: ShaderMaterialInput,
    bg_tex: texture_2d<f32>,
    bg_sampler: sampler,
    height: f32,
    dir: vec2f,
) -> vec4f {
    let coord = in.frag_coord;
    let sample_coord = coord + height * dir;
    return sample_texture_local_uv(bg_tex, bg_sampler, sample_coord / in.target_size);
}

fn nf_math_closure_fragment_closure(uv: vec2<f32>, t: f32, T: f32) -> f32 {
    var uv_1: vec2<f32>;
    var t_1: f32;
    var T_1: f32;
    var output: f32;

    uv_1 = uv;
    t_1 = t;
    T_1 = T;
    let _e7: f32 = t_1;
    let _e8: f32 = T_1;
    let _e10: f32 = t_1;
    let _e11: f32 = T_1;
    let _e14: f32 = T_1;
    output = ((fract((_e10 / _e11)) * _e14) / 3f);
    let _e18: f32 = output;
    return _e18;
}

struct NfWave {
    height: f32,
    dir: vec2f,
    normal: vec3f,
};

// All spatial values belong to the consuming geometry, in bottom-left/Y-up LocalPx.
// Progress is already authored time: looping and time scaling belong upstream.
fn nf_wave(
    coord: vec2f,
    geometry_height: f32,
    progress: f32,
    origin: vec2f,
    amplitude: f32,
    frequency: f32,
    decay: f32,
    speed: f32,
    damping: f32,
    front_feather: f32,
) -> NfWave {
    let scale = max(geometry_height, 1.0);
    let delta = coord - origin;
    let radius = length(delta);
    let dir = delta / max(radius, 0.000001);
    let speed_px = max(speed * scale, 0.001);
    let damping_px = damping / scale;
    let t = progress - radius / speed_px;
    if (t <= 0.0) {
        return NfWave(0.0, dir, vec3f(0.0, 0.0, 1.0));
    }

    let phase = frequency * t;
    let attenuation = amplitude * scale * exp(-decay * t - radius * damping_px);
    let wave = sin(phase);
    var onset = 1.0;
    var onset_dr = 0.0;
    if (front_feather > 0.0) {
        let x = clamp(t * speed_px / front_feather, 0.0, 1.0);
        onset = x * x * x * (10.0 + x * (-15.0 + 6.0 * x));
        onset_dr = -30.0 * x * x * (1.0 - x) * (1.0 - x) / front_feather;
    }
    let height = attenuation * wave * onset;
    // d(height)/d(radius), including both attenuation and the feather envelope.
    let slope = attenuation * (
        ((decay / speed_px - damping_px) * wave - frequency / speed_px * cos(phase)) * onset
        + wave * onset_dr
    );
    // The radial direction is zero at the center, giving a finite +Z normal there.
    let normal = normalize(vec3f(-slope * dir, 1.0));
    return NfWave(height, dir, normal);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    let math_closure_out = nf_math_closure_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y), params.time, (graph_inputs.float_input_19ad5082_7).x);
    // Shader Material ShaderMaterial_bdf48f62_2.material
    let shader_material_material = shader_material_ShaderMaterial_bdf48f62_2(
        ShaderMaterialInput(vec2f(in.uv.x, 1.0 - in.uv.y), in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        pass_tex_ProceduralTestTexture_bdf48f62_1,
        pass_samp_ProceduralTestTexture_bdf48f62_1,
        nf_wave(in.local_px.xy, in.geo_size_px.y, math_closure_out, vec2f(630.0, -700.0), 0.200000003, 100.0, 27.0, 1.0, 3.700000048, 0.0).height,
        nf_wave(in.local_px.xy, in.geo_size_px.y, math_closure_out, vec2f(630.0, -700.0), 0.200000003, 100.0, 27.0, 1.0, 3.700000048, 0.0).dir,
    );
    // Final composite
    let _frag_out = shader_material_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
