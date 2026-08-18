
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
    // Node: ColorInput_IntelligentLightParticleColor
    node_ColorInput_IntelligentLightParticleColor_4b6e0648: vec4f,
    // Node: ColorInput_IntelligentLightParticleNoiseColor
    node_ColorInput_IntelligentLightParticleNoiseColor_799010c1: vec4f,
    // Node: FloatInput_37
    node_FloatInput_37_0eaa0821: vec4f,
    // Node: FloatInput_45
    node_FloatInput_45_c714f420: vec4f,
    // Node: FloatInput_46
    node_FloatInput_46_7a16f420: vec4f,
    // Node: FloatInput_47
    node_FloatInput_47_2d18f420: vec4f,
    // Node: FloatInput_GlowBloomSigmaPx
    node_FloatInput_GlowBloomSigmaPx_274fe8f8: vec4f,
    // Node: FloatInput_GlowCoreSigmaPx
    node_FloatInput_GlowCoreSigmaPx_9500a558: vec4f,
    // Node: FloatInput_GlowMidSigmaPx
    node_FloatInput_GlowMidSigmaPx_c42032aa: vec4f,
    // Node: FloatInput_GlowWideSigmaPx
    node_FloatInput_GlowWideSigmaPx_59b0574d: vec4f,
    // Node: FloatInput_IntelligentLightParticleGain
    node_FloatInput_IntelligentLightParticleGain_de7846b4: vec4f,
    // Node: FloatInput_IntelligentLightParticleOpacity
    node_FloatInput_IntelligentLightParticleOpacity_c60851cb: vec4f,
    // Node: FloatInput_IntelligentLightParticlePointerWarpProgress
    node_FloatInput_IntelligentLightParticlePointerWarpProgress_73a49be9: vec4f,
    // Node: FloatInput_LightBloomMinSigmaPx
    node_FloatInput_LightBloomMinSigmaPx_2483ee02: vec4f,
    // Node: FloatInput_LightClipBloomProgress
    node_FloatInput_LightClipBloomProgress_3a773115: vec4f,
    // Node: FloatInput_LightHardClipFeatherPx
    node_FloatInput_LightHardClipFeatherPx_fa3b19bf: vec4f,
    // Node: FloatInput_ParticleEdgeSigmaPx
    node_FloatInput_ParticleEdgeSigmaPx_72957d4d: vec4f,
    // Node: FloatInput_ParticleInnerSizePx
    node_FloatInput_ParticleInnerSizePx_6f57ef00: vec4f,
    // Node: FloatInput_ParticleMinSizePx
    node_FloatInput_ParticleMinSizePx_d3ca865c: vec4f,
    // Node: FloatInput_ParticleOriginYPx
    node_FloatInput_ParticleOriginYPx_c355d815: vec4f,
    // Node: FloatInput_ParticleOuterSizePx
    node_FloatInput_ParticleOuterSizePx_04da7239: vec4f,
    // Node: FloatInput_ParticleWarpPx
    node_FloatInput_ParticleWarpPx_14892f46: vec4f,
    // Node: FloatInput_ParticleWarpRadiusPx
    node_FloatInput_ParticleWarpRadiusPx_aa7919d1: vec4f,
    // Node: FloatInput_SoundBarInnerFalloffPx
    node_FloatInput_SoundBarInnerFalloffPx_4a41e9e1: vec4f,
    // Node: FloatInput_SoundBarOffsetPx
    node_FloatInput_SoundBarOffsetPx_b77fb052: vec4f,
    // Node: FloatInput_SoundBarOuterFalloffPx
    node_FloatInput_SoundBarOuterFalloffPx_735b9fa8: vec4f,
    // Node: FloatInput_SoundBarRadiusPx
    node_FloatInput_SoundBarRadiusPx_0a5bf97c: vec4f,
    // Node: FloatInput_TotalEnergy
    node_FloatInput_TotalEnergy_56fa9c42: vec4f,
    // Node: Vector2Input_38
    node_Vector2Input_38_ba4f3fbd: vec4f,
    // Node: Vector2Input_LightEffectCanvasSizePx
    node_Vector2Input_LightEffectCanvasSizePx_9b9ae39c: vec4f,
    // Node: Vector2Input_LightEffectFrameSizePx
    node_Vector2Input_LightEffectFrameSizePx_0e8bf3b1: vec4f,
    // Node: Vector2Input_LightEffectPositionPx
    node_Vector2Input_LightEffectPositionPx_a9a5e69f: vec4f,
    // Node: Vector2Input_PointerLightEffectLocalPx
    node_Vector2Input_PointerLightEffectLocalPx_a618ee3f: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_GroupInstance_32_IntelligentLight_30: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_GroupInstance_32_IntelligentLight_30: sampler;


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

// Port of intelligent_light_upsample.agsl's processed IntelligentLight layer.
// Node Forge supplies ShaderMaterialInput in linear extended-sRGB coordinates.

// The original particle treatment intentionally mixed and boosted authored colors in encoded
// sRGB before decoding the result to scene-linear. Canonical color inputs are now linear and
// premultiplied, so reconstruct that legacy artistic domain locally instead of changing the
// SceneDSL color ABI or decoding every material color input.
fn legacy_particle_linear_to_srgb_channel_GroupInstance_32_ShaderMaterial_32(value: f32) -> f32 {
    let nonnegative = max(value, 0.0);
    let low = nonnegative * 12.92;
    let high = 1.055 * pow(nonnegative, 1.0 / 2.4) - 0.055;
    return select(high, low, nonnegative <= 0.0031308);
}

fn legacy_particle_linear_to_srgb_GroupInstance_32_ShaderMaterial_32(value: vec3f) -> vec3f {
    return vec3f(
        legacy_particle_linear_to_srgb_channel_GroupInstance_32_ShaderMaterial_32(value.x),
        legacy_particle_linear_to_srgb_channel_GroupInstance_32_ShaderMaterial_32(value.y),
        legacy_particle_linear_to_srgb_channel_GroupInstance_32_ShaderMaterial_32(value.z),
    );
}

fn legacy_particle_srgb_to_linear_channel_GroupInstance_32_ShaderMaterial_32(value: f32) -> f32 {
    let nonnegative = max(value, 0.0);
    let low = nonnegative / 12.92;
    let high = pow((nonnegative + 0.055) / 1.055, 2.4);
    return select(high, low, nonnegative <= 0.04045);
}

fn legacy_particle_srgb_to_linear_GroupInstance_32_ShaderMaterial_32(value: vec3f) -> vec3f {
    return vec3f(
        legacy_particle_srgb_to_linear_channel_GroupInstance_32_ShaderMaterial_32(value.x),
        legacy_particle_srgb_to_linear_channel_GroupInstance_32_ShaderMaterial_32(value.y),
        legacy_particle_srgb_to_linear_channel_GroupInstance_32_ShaderMaterial_32(value.z),
    );
}

fn legacy_particle_linear_premul_to_srgb_premul_GroupInstance_32_ShaderMaterial_32(value: vec4f) -> vec4f {
    let alpha = clamp(value.a, 0.0, 1.0);
    let straight_linear = select(
        vec3f(0.0),
        max(value.rgb / max(alpha, 0.000001), vec3f(0.0)),
        alpha > 0.000001,
    );
    return vec4f(legacy_particle_linear_to_srgb_GroupInstance_32_ShaderMaterial_32(straight_linear) * alpha, alpha);
}

fn sd_rounded_box_GroupInstance_32_ShaderMaterial_32(point: vec2f, half_size: vec2f, radius: f32) -> f32 {
    let q = abs(point) - half_size + vec2f(radius);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2f(0.0))) - radius;
}

fn supercircle_sdf_GroupInstance_32_ShaderMaterial_32(
    point: vec2f,
    center: vec2f,
    radius: f32,
    axis_mix: vec2f,
) -> f32 {
    let abs_radius = max(abs(radius), 0.0001);
    let scaled_radius = 1.5286649465560913 * abs_radius;
    let blended_radius = mix(scaled_radius, radius, max(axis_mix.x, axis_mix.y));
    let offset = point - center;
    let shifted_pos = vec2f(scaled_radius) + offset;
    let normalized_pos = max(vec2f(0.0), shifted_pos / scaled_radius);
    let abs_norm_pos = abs(normalized_pos);
    let max_norm = max(abs_norm_pos.x, abs_norm_pos.y);
    var axis_ratio = 0.0;
    if (max_norm > 0.0001) {
        axis_ratio = clamp(min(abs_norm_pos.x, abs_norm_pos.y) / max_norm, 0.0, 1.0);
    }
    let len_norm = length(abs_norm_pos);
    let poly_fit = ((((-0.7391197269 * axis_ratio + 2.4034927648) * axis_ratio
        - 2.4907319173) * axis_ratio + 0.4768708960) * axis_ratio + 0.4747847594);
    let denominator = max(
        1.0 - axis_ratio * axis_ratio * clamp(len_norm, 0.0, 1.0) * poly_fit,
        0.0001,
    );
    let dist_base = (len_norm + 1.0) - 1.0 / denominator;
    let dist_alt = 0.6541655659675598
        * length(max(
            vec2f(0.0),
            1.5286649465560913 * abs_norm_pos - vec2f(0.5286650061607361),
        ))
        + 0.3458344340324402;
    let dist_mix_x = mix(dist_base, dist_alt, axis_mix.x);
    let dist_mix_y = mix(dist_base, dist_alt, axis_mix.y);
    let axis_sign = select(-1.0, 1.0, abs_norm_pos.y > abs_norm_pos.x);
    let final_mix = mix(
        dist_mix_x,
        dist_mix_y,
        clamp(0.5 - axis_sign + axis_sign * axis_ratio, 0.0, 1.0),
    );
    let radial_pos = vec2f(blended_radius) + offset;
    return min(max(radial_pos.x, radial_pos.y), 0.0)
        + scaled_radius * (final_mix - 1.0);
}

fn smooth_corner_sdf_GroupInstance_32_ShaderMaterial_32(point: vec2f, half_size: vec2f, radius: f32) -> f32 {
    let safe_half_size = max(half_size, vec2f(0.0001));
    if (radius <= 0.0001) {
        return sd_rounded_box_GroupInstance_32_ShaderMaterial_32(point, safe_half_size, 0.0);
    }
    let radius_ratio = clamp(vec2f(radius) / safe_half_size, vec2f(0.0), vec2f(1.0));
    let ratio = clamp((radius_ratio - vec2f(0.6)) / 0.4, vec2f(0.0), vec2f(1.0));
    return supercircle_sdf_GroupInstance_32_ShaderMaterial_32(abs(point), safe_half_size, radius, ratio);
}

fn glass_frame_sdf_GroupInstance_32_ShaderMaterial_32(point: vec2f, half_size: vec2f, radius: f32) -> f32 {
    let shape_sdf = smooth_corner_sdf_GroupInstance_32_ShaderMaterial_32(point, half_size, radius);
    let layer_bounds_sdf = sd_rounded_box_GroupInstance_32_ShaderMaterial_32(point, half_size, 0.0);
    return max(shape_sdf, layer_bounds_sdf);
}

fn erf_approx_GroupInstance_32_ShaderMaterial_32(value: f32) -> f32 {
    let absolute = abs(value);
    let t = 1.0 / (1.0 + 0.3275911 * absolute);
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t
        + 1.421413741) * t - 0.284496736) * t + 0.254829592)
        * t * exp(-absolute * absolute));
    return select(-y, y, value >= 0.0);
}

fn gaussian_edge_GroupInstance_32_ShaderMaterial_32(sdf: f32, sigma: f32) -> f32 {
    return 0.5 - 0.5 * erf_approx_GroupInstance_32_ShaderMaterial_32(sdf / (sigma * 1.4142135));
}

fn gaussian_interval_GroupInstance_32_ShaderMaterial_32(position: f32, half_extent: f32, inverse_sigma: f32) -> f32 {
    return 0.5 * (
        erf_approx_GroupInstance_32_ShaderMaterial_32((position + half_extent) * inverse_sigma)
        - erf_approx_GroupInstance_32_ShaderMaterial_32((position - half_extent) * inverse_sigma)
    );
}

fn analytic_box_bloom_alpha_GroupInstance_32_ShaderMaterial_32(point: vec2f, half_size: vec2f, sigma: f32) -> f32 {
    let inverse_sigma = 1.0 / max(sigma * 1.4142135, 0.0001);
    let bloom_x = gaussian_interval_GroupInstance_32_ShaderMaterial_32(point.x, half_size.x, inverse_sigma);
    let bloom_y = gaussian_interval_GroupInstance_32_ShaderMaterial_32(point.y, half_size.y, inverse_sigma);
    let center_alpha = max(
        erf_approx_GroupInstance_32_ShaderMaterial_32(half_size.x * inverse_sigma)
            * erf_approx_GroupInstance_32_ShaderMaterial_32(half_size.y * inverse_sigma),
        0.0001,
    );
    let sigma_ratio = sigma / max(min(half_size.x, half_size.y), 0.0001);
    let target_peak = mix(1.0, 0.8, smoothstep(1.0, 4.0, sigma_ratio));
    return clamp(bloom_x * bloom_y * target_peak / center_alpha, 0.0, 1.0);
}

fn light_hard_clip_alpha_GroupInstance_32_ShaderMaterial_32(sdf: f32, feather_px: f32) -> f32 {
    return 1.0 - smoothstep(-max(feather_px, 0.0001), 0.0, sdf);
}

fn light_bloom_alpha_GroupInstance_32_ShaderMaterial_32(
    sdf: f32,
    point: vec2f,
    half_size: vec2f,
    bloom_half_size: vec2f,
    progress: f32,
    min_sigma_px: f32,
) -> f32 {
    let t = clamp(progress, 0.0, 1.0);
    let min_sigma = max(min_sigma_px, 0.0001);
    let sigma = mix(min_sigma, max(min_sigma, min(bloom_half_size.x, bloom_half_size.y)), t);
    let min_half_extent = min(half_size.x, half_size.y);
    let switch_start = max(min_sigma, min_half_extent * 0.35);
    let switch_end = max(switch_start + 0.0001, min_half_extent);
    if (sigma <= switch_start) {
        return clamp(1.6 * gaussian_edge_GroupInstance_32_ShaderMaterial_32(sdf, sigma), 0.0, 1.0);
    }
    let box_alpha = analytic_box_bloom_alpha_GroupInstance_32_ShaderMaterial_32(point, half_size, sigma);
    if (sigma >= switch_end) {
        return box_alpha;
    }
    let sdf_alpha = clamp(1.6 * gaussian_edge_GroupInstance_32_ShaderMaterial_32(sdf, sigma), 0.0, 1.0);
    return mix(sdf_alpha, box_alpha, smoothstep(switch_start, switch_end, sigma));
}

const PARTICLE_TAU: f32 = 6.28318;
const PARTICLE_DENSITY: f32 = 32.0;
const PARTICLE_GOLDEN_ANGLE: f32 = 0.618034;
const PARTICLE_EMIT_SPEED: f32 = 0.012;
const PARTICLE_JITTER_SPEED: f32 = 2.0;
const PARTICLE_JITTER_AMOUNT: f32 = 0.15;
const PARTICLE_TWINKLE_SPEED: f32 = 2.0;
fn particle_hash2_GroupInstance_32_ShaderMaterial_32(point: vec2f) -> vec2f {
    let hashed = vec2f(
        dot(point, vec2f(127.1, 311.7)),
        dot(point, vec2f(269.5, 183.3)),
    );
    return fract(sin(hashed) * 43758.5453);
}

fn particle_hash3_GroupInstance_32_ShaderMaterial_32(point: vec3f) -> vec3f {
    return -1.0 + 2.0 * fract(sin(vec3f(
        dot(point, vec3f(127.1, 311.7, 74.7)),
        dot(point, vec3f(269.5, 183.3, 246.1)),
        dot(point, vec3f(113.5, 271.9, 124.6)),
    )) * 43758.5453);
}

fn particle_simplex3_GroupInstance_32_ShaderMaterial_32(point: vec3f) -> f32 {
    let skew = 0.3333333;
    let unskew = 0.1666667;
    let cell = floor(point + dot(point, vec3f(skew)));
    let origin = cell - dot(cell, vec3f(unskew));
    let c0 = point - origin;
    let order = step(c0.yzx, c0.xyz);
    let offset1 = order * (vec3f(1.0) - order.zxy);
    let offset2 = vec3f(1.0) - order.zxy * (vec3f(1.0) - order);
    let c1 = c0 - offset1 + vec3f(unskew);
    let c2 = c0 - offset2 + vec3f(unskew * 2.0);
    let c3 = c0 - vec3f(1.0) + vec3f(unskew * 3.0);
    let weights = max(
        vec4f(0.6) - vec4f(dot(c0, c0), dot(c1, c1), dot(c2, c2), dot(c3, c3)),
        vec4f(0.0),
    );
    let contributions = vec4f(
        dot(particle_hash3_GroupInstance_32_ShaderMaterial_32(cell), c0),
        dot(particle_hash3_GroupInstance_32_ShaderMaterial_32(cell + offset1), c1),
        dot(particle_hash3_GroupInstance_32_ShaderMaterial_32(cell + offset2), c2),
        dot(particle_hash3_GroupInstance_32_ShaderMaterial_32(cell + vec3f(1.0)), c3),
    ) * weights * weights * weights * weights;
    return dot(vec4f(32.0), contributions);
}

fn particle_noise_mask_GroupInstance_32_ShaderMaterial_32(point: vec2f, canvas_size: vec2f, time: f32) -> f32 {
    let noise_uv = point / max(canvas_size.y, 1.0);
    let noise = particle_simplex3_GroupInstance_32_ShaderMaterial_32(vec3f(noise_uv * exp(0.1), time * 0.5));
    return clamp((noise + 0.5) * 0.5 + 0.5, 0.0, 1.0);
}

fn particle_point_GroupInstance_32_ShaderMaterial_32(
    grid: vec2f,
    sqrt_radius: f32,
    size_px: f32,
    min_size_px: f32,
    radius: f32,
    time: f32,
) -> f32 {
    let id = floor(grid);
    let local = fract(grid) - 0.5;
    let jitter = (particle_hash2_GroupInstance_32_ShaderMaterial_32(id) - 0.5) * 0.6
        + sin(time * (particle_hash2_GroupInstance_32_ShaderMaterial_32(id + 0.5) - 0.5) * PARTICLE_JITTER_SPEED)
            * PARTICLE_JITTER_AMOUNT;
    let distance_px = length(
        (local - jitter) * vec2f(
            sqrt_radius * sqrt_radius * PARTICLE_TAU / PARTICLE_DENSITY,
            2.0 * sqrt_radius / PARTICLE_DENSITY,
        ),
    ) * radius;
    var alpha = step(distance_px, max(size_px, min_size_px) * 0.6);
    alpha *= sin(
        time * PARTICLE_TWINKLE_SPEED + particle_hash2_GroupInstance_32_ShaderMaterial_32(id).x * PARTICLE_TAU,
    ) * 0.5 + 0.5;
    return alpha;
}

fn particle_mask_GroupInstance_32_ShaderMaterial_32(
    point: vec2f,
    pointer_point: vec2f,
    canvas_size: vec2f,
    time: f32,
    pointer_warp_progress: f32,
    particle_inner_size_px: f32,
    particle_outer_size_px: f32,
    particle_min_size_px: f32,
    particle_warp_px: f32,
    particle_warp_radius_px: f32,
    particle_edge_sigma_px: f32,
) -> f32 {
    let radius = canvas_size.x * 0.8;
    let pointer_direction = point - pointer_point;
    let pointer_distance_squared = dot(pointer_direction, pointer_direction);
    let warp_radius = max(particle_warp_radius_px, 0.0001);
    let warped_point = point
        - pointer_direction * particle_warp_px
            * clamp(pointer_warp_progress, 0.0, 1.0)
            * exp(-2.0 * pointer_distance_squared / (warp_radius * warp_radius))
            / warp_radius;
    let polar_offset = warped_point / max(radius, 1.0);
    let angle = atan2(polar_offset.y, polar_offset.x) / PARTICLE_TAU + 0.5;
    let radial_distance = length(polar_offset);
    let sqrt_radius = sqrt(max(radial_distance, 0.0));
    let polar_grid = vec2f(angle, sqrt_radius) * PARTICLE_DENSITY;
    let size_px = mix(
        particle_inner_size_px,
        particle_outer_size_px,
        clamp(radial_distance, 0.0, 1.0),
    );
    var total = 0.0;
    var layer_alpha = 1.0;
    for (var index = 0; index < 4; index += 1) {
        let layer = f32(index);
        let scroll = vec2f(
            layer * PARTICLE_GOLDEN_ANGLE,
            -PARTICLE_EMIT_SPEED * time * (1.0 + layer * 0.3),
        );
        let offset = particle_hash2_GroupInstance_32_ShaderMaterial_32(vec2f(layer, layer * 7.919)) * 10.0;
        total += particle_point_GroupInstance_32_ShaderMaterial_32(
            polar_grid * (1.0 + layer * 0.05)
                + scroll * PARTICLE_DENSITY
                + offset,
            sqrt_radius,
            size_px,
            particle_min_size_px,
            radius,
            time,
        ) * layer_alpha;
        layer_alpha *= 0.9;
    }
    let sigma = max(particle_edge_sigma_px, 0.0001);
    let edge_distance = length(point) - radius;
    let shape = 0.5 - 0.5 * erf_approx_GroupInstance_32_ShaderMaterial_32(edge_distance / (sigma * 1.4142135));
    return min(total, 1.0) * shape;
}

fn apply_particles_GroupInstance_32_ShaderMaterial_32(
    current_color: vec4f,
    coord: vec2f,
    canvas_size: vec2f,
    time: f32,
    pointer_position: vec2f,
    pointer_warp_progress: f32,
    particle_color: vec4f,
    particle_noise_color: vec4f,
    particle_gain: f32,
    particle_opacity: f32,
    particle_inner_size_px: f32,
    particle_outer_size_px: f32,
    particle_min_size_px: f32,
    particle_warp_px: f32,
    particle_warp_radius_px: f32,
    particle_edge_sigma_px: f32,
    particle_origin_y_px: f32,
) -> vec4f {
    if (particle_opacity <= 0.0001) {
        return current_color;
    }
    let origin = vec2f(canvas_size.x * 0.5, particle_origin_y_px);
    let point = coord - origin;
    let pointer_point = pointer_position - origin;
    let mask = particle_mask_GroupInstance_32_ShaderMaterial_32(
        point,
        pointer_point,
        canvas_size,
        time,
        pointer_warp_progress,
        particle_inner_size_px,
        particle_outer_size_px,
        particle_min_size_px,
        particle_warp_px,
        particle_warp_radius_px,
        particle_edge_sigma_px,
    ) * clamp(particle_opacity, 0.0, 1.0);
    let noise = particle_noise_mask_GroupInstance_32_ShaderMaterial_32(point, canvas_size, time);
    let particle_color_srgb = legacy_particle_linear_premul_to_srgb_premul_GroupInstance_32_ShaderMaterial_32(particle_color);
    let particle_noise_color_srgb = legacy_particle_linear_premul_to_srgb_premul_GroupInstance_32_ShaderMaterial_32(
        particle_noise_color,
    );
    let working = mix(particle_noise_color_srgb, particle_color_srgb, noise);
    let alpha = clamp(working.a, 0.0, 1.0);
    let linear = legacy_particle_srgb_to_linear_GroupInstance_32_ShaderMaterial_32(
        max(working.rgb * max(particle_gain, 0.0), vec3f(0.0)),
    ) * alpha;
    return mix(current_color, vec4f(linear, alpha), mask);
}

fn shader_material_GroupInstance_32_ShaderMaterial_32(
    in: ShaderMaterialInput,
    intelli_tex: texture_2d<f32>,
    intelli_sampler: sampler,
    frame_size_px: vec2f,
    light_bloom_size_px: vec2f,
    corner_radius_px: f32,
    light_hard_clip_feather_px: f32,
    light_bloom_min_sigma_px: f32,
    glow_wide_sigma_px: f32,
    glow_mid_sigma_px: f32,
    glow_core_sigma_px: f32,
    glow_bloom_sigma_px: f32,
    total_energy: f32,
    voice_opacity: f32,
    core_glow_opacity: f32,
    glow_mask_morph: f32,
    light_clip_bloom_progress: f32,
    time: f32,
    particle_pointer_position_px: vec2f,
    particle_pointer_warp_progress: f32,
    particle_color: vec4f,
    particle_noise_color: vec4f,
    particle_gain: f32,
    particle_opacity: f32,
    particle_inner_size_px: f32,
    particle_outer_size_px: f32,
    particle_min_size_px: f32,
    particle_warp_px: f32,
    particle_warp_radius_px: f32,
    particle_edge_sigma_px: f32,
    particle_origin_y_px: f32,
    sound_bar_offset_px: f32,
    sound_bar_radius_px: f32,
    sound_bar_outer_falloff_px: f32,
    sound_bar_inner_falloff_px: f32,
) -> vec4f {
    let canvas_size_px = max(in.geometry_size, vec2f(1.0));
    let size_px = clamp(frame_size_px, vec2f(0.0001), canvas_size_px);
    let canvas_center_px = canvas_size_px * 0.5;
    let point = in.local_position.xy - canvas_center_px;
    let half_size_px = size_px * 0.5;
    let bloom_size_px = clamp(light_bloom_size_px, vec2f(0.0001), canvas_size_px);
    let radius_px = clamp(corner_radius_px, 0.0, min(half_size_px.x, half_size_px.y));
    let sdf = glass_frame_sdf_GroupInstance_32_ShaderMaterial_32(point, half_size_px, radius_px);
    let bloom_progress = clamp(light_clip_bloom_progress, 0.0, 1.0);
    let hard_clip_alpha = light_hard_clip_alpha_GroupInstance_32_ShaderMaterial_32(sdf, light_hard_clip_feather_px);
    let bloom_alpha = light_bloom_alpha_GroupInstance_32_ShaderMaterial_32(
        sdf,
        point,
        half_size_px,
        bloom_size_px * 0.5,
        bloom_progress,
        light_bloom_min_sigma_px,
    );

    // IntelligentLight is already linear HDR and premultiplied in Node Forge.
    let intelligent_light = sample_texture_local_uv(intelli_tex, intelli_sampler, in.uv);
    var glow = exp(-pow(sdf / -mix(glow_wide_sigma_px, glow_bloom_sigma_px, bloom_progress), 2.0))
        * mix(1.4, 1.4, total_energy * voice_opacity);
    glow += exp(-pow(sdf / -mix(glow_mid_sigma_px, glow_bloom_sigma_px, bloom_progress), 2.0))
        * mix(1.0, 1.6, total_energy * voice_opacity);
    glow += exp(-pow(sdf / -mix(glow_core_sigma_px, glow_bloom_sigma_px, bloom_progress), 2.0))
        * mix(0.0, 3.0, core_glow_opacity);

    let sound_bar_offset = max(sound_bar_offset_px, 0.0);
    let sound_bar_radius = max(sound_bar_radius_px, 0.0);
    let sound_bar_outer = max(sound_bar_outer_falloff_px, 0.0001);
    let sound_bar_inner = max(sound_bar_inner_falloff_px, 0.0001);
    let sound_bar_distance = length(
        vec2f(abs(point.x), point.y) - vec2f(size_px.x * 0.5 + sound_bar_offset, 0.0),
    );
    var sound_bar = exp(
        -pow(max(abs(sound_bar_distance) - sound_bar_radius, 0.0) / -sound_bar_outer, 2.0),
    );
    sound_bar += 1.2 * exp(
        -pow(max(abs(sound_bar_distance) - sound_bar_radius, 0.0) / -sound_bar_inner, 2.0),
    );
    glow = mix(
        sound_bar + glow * 0.35,
        glow,
        clamp(glow_mask_morph, 0.0, 1.0),
    );

    let light_envelope = mix(
        hard_clip_alpha,
        bloom_alpha,
        smoothstep(0.0, 0.05, bloom_progress),
    );
    let light_gain = max(light_envelope * glow, 0.0);
    var color = vec4f(
        intelligent_light.rgb * light_gain,
        clamp(intelligent_light.a * light_gain, 0.0, 1.0),
    );
    color = apply_particles_GroupInstance_32_ShaderMaterial_32(
        color,
        in.local_position.xy,
        canvas_size_px,
        time,
        particle_pointer_position_px,
        particle_pointer_warp_progress,
        particle_color,
        particle_noise_color,
        particle_gain,
        particle_opacity,
        particle_inner_size_px,
        particle_outer_size_px,
        particle_min_size_px,
        particle_warp_px,
        particle_warp_radius_px,
        particle_edge_sigma_px,
        particle_origin_y_px,
    );

    color.a = clamp(color.a, 0.0, 1.0);
    return color;
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Shader Material GroupInstance_32/ShaderMaterial_32.material
    let particle_origin_y_px_material = shader_material_GroupInstance_32_ShaderMaterial_32(
        ShaderMaterialInput(vec2f(in.uv.x, 1.0 - in.uv.y), in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        pass_tex_GroupInstance_32_IntelligentLight_30,
        pass_samp_GroupInstance_32_IntelligentLight_30,
        (graph_inputs.node_Vector2Input_LightEffectFrameSizePx_0e8bf3b1).xy,
        (graph_inputs.node_Vector2Input_38_ba4f3fbd).xy,
        (graph_inputs.node_FloatInput_37_0eaa0821).x,
        (graph_inputs.node_FloatInput_LightHardClipFeatherPx_fa3b19bf).x,
        (graph_inputs.node_FloatInput_LightBloomMinSigmaPx_2483ee02).x,
        (graph_inputs.node_FloatInput_GlowWideSigmaPx_59b0574d).x,
        (graph_inputs.node_FloatInput_GlowMidSigmaPx_c42032aa).x,
        (graph_inputs.node_FloatInput_GlowCoreSigmaPx_9500a558).x,
        (graph_inputs.node_FloatInput_GlowBloomSigmaPx_274fe8f8).x,
        (graph_inputs.node_FloatInput_TotalEnergy_56fa9c42).x,
        (graph_inputs.node_FloatInput_45_c714f420).x,
        (graph_inputs.node_FloatInput_46_7a16f420).x,
        (graph_inputs.node_FloatInput_47_2d18f420).x,
        (graph_inputs.node_FloatInput_LightClipBloomProgress_3a773115).x,
        params.time,
        (graph_inputs.node_Vector2Input_PointerLightEffectLocalPx_a618ee3f).xy,
        (graph_inputs.node_FloatInput_IntelligentLightParticlePointerWarpProgress_73a49be9).x,
        vec4f((graph_inputs.node_ColorInput_IntelligentLightParticleColor_4b6e0648).rgb * (graph_inputs.node_ColorInput_IntelligentLightParticleColor_4b6e0648).a, (graph_inputs.node_ColorInput_IntelligentLightParticleColor_4b6e0648).a),
        vec4f((graph_inputs.node_ColorInput_IntelligentLightParticleNoiseColor_799010c1).rgb * (graph_inputs.node_ColorInput_IntelligentLightParticleNoiseColor_799010c1).a, (graph_inputs.node_ColorInput_IntelligentLightParticleNoiseColor_799010c1).a),
        (graph_inputs.node_FloatInput_IntelligentLightParticleGain_de7846b4).x,
        (graph_inputs.node_FloatInput_IntelligentLightParticleOpacity_c60851cb).x,
        (graph_inputs.node_FloatInput_ParticleInnerSizePx_6f57ef00).x,
        (graph_inputs.node_FloatInput_ParticleOuterSizePx_04da7239).x,
        (graph_inputs.node_FloatInput_ParticleMinSizePx_d3ca865c).x,
        (graph_inputs.node_FloatInput_ParticleWarpPx_14892f46).x,
        (graph_inputs.node_FloatInput_ParticleWarpRadiusPx_aa7919d1).x,
        (graph_inputs.node_FloatInput_ParticleEdgeSigmaPx_72957d4d).x,
        (graph_inputs.node_FloatInput_ParticleOriginYPx_c355d815).x,
        (graph_inputs.node_FloatInput_SoundBarOffsetPx_b77fb052).x,
        (graph_inputs.node_FloatInput_SoundBarRadiusPx_0a5bf97c).x,
        (graph_inputs.node_FloatInput_SoundBarOuterFalloffPx_735b9fa8).x,
        (graph_inputs.node_FloatInput_SoundBarInnerFalloffPx_4a41e9e1).x,
    );
    // Final composite
    let _frag_out = particle_origin_y_px_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
