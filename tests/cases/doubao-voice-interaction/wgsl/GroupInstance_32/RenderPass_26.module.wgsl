
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
    // Node: FloatInput_37
    float_input_37: vec4f,
    // Node: FloatInput_45
    float_input_45: vec4f,
    // Node: FloatInput_46
    float_input_46: vec4f,
    // Node: FloatInput_47
    float_input_47: vec4f,
    // Node: FloatInput_48
    float_input_48: vec4f,
    // Node: FloatInput_49
    float_input_49: vec4f,
    // Node: FloatInput_50
    float_input_50: vec4f,
    // Node: GroupInstance_32/Vector2Input_39
    node_GroupInstance_32_Vector2Input_39_f8d74346: vec4f,
    // Node: Vector2Input_35
    vector2_input_35: vec4f,
    // Node: Vector2Input_36
    node_Vector2Input_36_f0373fbd: vec4f,
    // Node: Vector2Input_38
    vector2_input_38: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

struct ShaderMaterialParams {
    shader_GroupInstance_32_ShaderMaterial_32_density: vec4f,
    shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies: array<vec4f, 16>,
    shader_GroupInstance_32_ShaderMaterial_32_light_clip_bloom_progress: vec4f,
    shader_GroupInstance_32_ShaderMaterial_32_total_energy: vec4f,
    shader_GroupInstance_32_ShaderMaterial_32_voice_dot_color: vec4f,
};
@group(0) @binding(3)
var<storage, read> shader_material_params: ShaderMaterialParams;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_GroupInstance_32_IntelligentLight_30: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_GroupInstance_32_IntelligentLight_30: sampler;


// --- Extra WGSL declarations (generated) ---

struct ShaderMaterialInput {
    uv: vec2f,
    frag_coord: vec2f,
    local_position: vec3f,
    geometry_size: vec2f,
    target_size: vec2f,
    time: f32,
};

// Port of voice_visualizer.agsl's processedIntelligentLight and voice-dot layers.
// Node Forge supplies ShaderMaterialInput in linear extended-sRGB coordinates.

fn catmull_segment_GroupInstance_32_ShaderMaterial_32(local_t: f32, y_im1: f32, y_i: f32, y_ip1: f32, y_ip2: f32) -> f32 {
    let m0 = 0.5 * (y_ip1 - y_im1) * 0.9;
    let m1 = 0.5 * (y_ip2 - y_i) * 0.9;
    let t2 = local_t * local_t;
    let t3 = t2 * local_t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + local_t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    return h00 * y_i + h10 * m0 + h01 * y_ip1 + h11 * m1;
}

fn catmull_values_GroupInstance_32_ShaderMaterial_32(t: f32, values: array<f32, 16>) -> f32 {
    let segment = clamp(t, 0.0, 1.0) * 14.0;
    if (segment < 1.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment, values[0], values[0], values[1], values[2]);
    }
    if (segment < 2.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 1.0, values[0], values[1], values[2], values[3]);
    }
    if (segment < 3.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 2.0, values[1], values[2], values[3], values[4]);
    }
    if (segment < 4.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 3.0, values[2], values[3], values[4], values[5]);
    }
    if (segment < 5.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 4.0, values[3], values[4], values[5], values[6]);
    }
    if (segment < 6.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 5.0, values[4], values[5], values[6], values[7]);
    }
    if (segment < 7.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 6.0, values[5], values[6], values[7], values[8]);
    }
    if (segment < 8.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 7.0, values[6], values[7], values[8], values[9]);
    }
    if (segment < 9.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 8.0, values[7], values[8], values[9], values[10]);
    }
    if (segment < 10.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 9.0, values[8], values[9], values[10], values[11]);
    }
    if (segment < 11.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 10.0, values[9], values[10], values[11], values[12]);
    }
    if (segment < 12.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 11.0, values[10], values[11], values[12], values[13]);
    }
    if (segment < 13.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 12.0, values[11], values[12], values[13], values[14]);
    }
    return catmull_segment_GroupInstance_32_ShaderMaterial_32(segment - 13.0, values[12], values[13], values[14], values[14]);
}

fn calm_human_voice_GroupInstance_32_ShaderMaterial_32(t: f32, values: array<f32, 16>) -> f32 {
    let dx = 0.75 / 14.0;
    return catmull_values_GroupInstance_32_ShaderMaterial_32(clamp(t - dx, 0.0, 1.0), values) * 0.15
        + catmull_values_GroupInstance_32_ShaderMaterial_32(t, values) * 0.70
        + catmull_values_GroupInstance_32_ShaderMaterial_32(clamp(t + dx, 0.0, 1.0), values) * 0.15;
}

fn smooth5_map_GroupInstance_32_ShaderMaterial_32(value: f32) -> f32 {
    var t = mix(0.5, 1.0, clamp(value, 0.0, 1.0));
    t = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    return (t - 0.5) * 2.0;
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

fn light_hard_clip_alpha_GroupInstance_32_ShaderMaterial_32(sdf: f32) -> f32 {
    return 1.0 - smoothstep(-2.5, 0.0, sdf);
}

fn light_bloom_alpha_GroupInstance_32_ShaderMaterial_32(
    sdf: f32,
    point: vec2f,
    half_size: vec2f,
    bloom_half_size: vec2f,
    progress: f32,
) -> f32 {
    let t = clamp(progress, 0.0, 1.0);
    let sigma = mix(2.5, max(2.5, min(bloom_half_size.x, bloom_half_size.y)), t);
    let min_half_extent = min(half_size.x, half_size.y);
    let switch_start = max(2.5, min_half_extent * 0.35);
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

fn voice_dot_sample_GroupInstance_32_ShaderMaterial_32(
    point: vec2f,
    index: f32,
    energy: f32,
    density: f32,
    progress: f32,
    response: f32,
) -> f32 {
    let center_distance = abs(index - 17.0);
    let mapped_energy = smooth5_map_GroupInstance_32_ShaderMaterial_32(clamp(energy * response * 1.5, 0.0, 1.0));
    let dot_size = vec2f(2.4, mix(7.2, 24.0, mapped_energy)) * density;
    let radius = min(dot_size.x, dot_size.y) * 0.5;
    let x = (index - 17.0) * 6.0 * density;
    let sdf = -sd_rounded_box_GroupInstance_32_ShaderMaterial_32(point - vec2f(x, 0.0), dot_size * 0.5, radius);
    let visible = smoothstep(
        (center_distance - 0.5) / 17.5,
        (center_distance + 0.5) / 17.5,
        clamp(progress, 0.0, 1.0),
    );
    return smoothstep(0.0, 1.0, sdf) * visible;
}

fn voice_dot_alpha_GroupInstance_32_ShaderMaterial_32(
    point: vec2f,
    density: f32,
    energies: array<f32, 16>,
    opacity: f32,
    progress: f32,
    response: f32,
) -> f32 {
    var alpha = 0.0;
    for (var index = 0; index < 35; index += 1) {
        let sample_t = f32(index) / 34.0;
        let energy = calm_human_voice_GroupInstance_32_ShaderMaterial_32(sample_t, energies);
        alpha = max(
            alpha,
            voice_dot_sample_GroupInstance_32_ShaderMaterial_32(
                point,
                f32(index),
                energy,
                density,
                progress,
                response,
            ),
        );
    }
    return alpha * clamp(opacity, 0.0, 1.0);
}

fn shader_material_GroupInstance_32_ShaderMaterial_32(
    in: ShaderMaterialInput,
    intelli_tex: texture_2d<f32>,
    intelli_sampler: sampler,
    frame_size_px: vec2f,
    light_bloom_size_px: vec2f,
    corner_radius_px: f32,
    density: f32,
    human_voice_energies: array<f32, 16>,
    total_energy: f32,
    voice_opacity: f32,
    core_glow_opacity: f32,
    glow_mask_morph: f32,
    light_clip_bloom_progress: f32,
    voice_dot_opacity: f32,
    voice_dot_progress: f32,
    voice_dot_response: f32,
    voice_dot_color: vec4f,
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
    let hard_clip_alpha = light_hard_clip_alpha_GroupInstance_32_ShaderMaterial_32(sdf);
    let bloom_alpha = light_bloom_alpha_GroupInstance_32_ShaderMaterial_32(
        sdf,
        point,
        half_size_px,
        bloom_size_px * 0.5,
        bloom_progress,
    );

    // IntelligentLight is already linear HDR and premultiplied in Node Forge.
    let intelligent_light = textureSample(intelli_tex, intelli_sampler, in.uv);
    var glow = exp(-pow(sdf / mix(-60.0, -2400.0, bloom_progress), 2.0))
        * mix(1.4, 1.4, total_energy * voice_opacity);
    glow += exp(-pow(sdf / mix(-20.0, -2400.0, bloom_progress), 2.0))
        * mix(1.0, 1.6, total_energy * voice_opacity);
    glow += exp(-pow(sdf / mix(-5.0, -2400.0, bloom_progress), 2.0))
        * mix(0.0, 3.0, core_glow_opacity);

    let sound_bar_distance = length(
        vec2f(abs(point.x), point.y) - vec2f(size_px.x * 0.5 + 120.0, 0.0),
    );
    var sound_bar = exp(
        -pow(max(abs(sound_bar_distance) - 80.0, 0.0) / -200.0, 2.0),
    );
    sound_bar += 1.2 * exp(
        -pow(max(abs(sound_bar_distance) - 80.0, 0.0) / -70.0, 2.0),
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

    let dot_alpha = voice_dot_alpha_GroupInstance_32_ShaderMaterial_32(
        point,
        density,
        human_voice_energies,
        voice_dot_opacity,
        voice_dot_progress,
        voice_dot_response,
    );
    let dot_coverage = clamp(dot_alpha * hard_clip_alpha, 0.0, 1.0);
    let dot_layer = vec4f(
        voice_dot_color.rgb * voice_dot_color.a * dot_coverage,
        dot_coverage,
    );
    color = dot_layer + color * (1.0 - dot_coverage);
    color.a = clamp(color.a, 0.0, 1.0);
    return color;
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

 let rect_size_px_base = (graph_inputs.node_GroupInstance_32_Vector2Input_39_f8d74346).xy;
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
    // Shader Material GroupInstance_32/ShaderMaterial_32.material
    let voice_dot_response_material = shader_material_GroupInstance_32_ShaderMaterial_32(
        ShaderMaterialInput(in.uv, in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        pass_tex_GroupInstance_32_IntelligentLight_30,
        pass_samp_GroupInstance_32_IntelligentLight_30,
        (graph_inputs.vector2_input_35).xy,
        (graph_inputs.vector2_input_38).xy,
        (graph_inputs.float_input_37).x,
        (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_density).x,
        array<f32,
        16>((shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[0]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[1]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[2]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[3]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[4]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[5]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[6]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[7]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[8]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[9]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[10]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[11]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[12]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[13]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[14]).x, (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_human_voice_energies[15]).x),
        (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_total_energy).x,
        (graph_inputs.float_input_45).x,
        (graph_inputs.float_input_46).x,
        (graph_inputs.float_input_47).x,
        (shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_light_clip_bloom_progress).x,
        (graph_inputs.float_input_48).x,
        (graph_inputs.float_input_49).x,
        (graph_inputs.float_input_50).x,
        shader_material_params.shader_GroupInstance_32_ShaderMaterial_32_voice_dot_color,
    );
    // Final composite
    let _frag_out = voice_dot_response_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
