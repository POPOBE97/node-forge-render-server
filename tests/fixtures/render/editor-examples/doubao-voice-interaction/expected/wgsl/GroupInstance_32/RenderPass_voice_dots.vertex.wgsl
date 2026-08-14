
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
    // Node: ColorInput_VoiceDotColor
    node_ColorInput_VoiceDotColor_dfa3c7da: vec4f,
    // Node: FloatInput_37
    node_FloatInput_37_0eaa0821: vec4f,
    // Node: FloatInput_48
    node_FloatInput_48_b0fef320: vec4f,
    // Node: FloatInput_49
    node_FloatInput_49_6300f420: vec4f,
    // Node: FloatInput_50
    node_FloatInput_50_d125f720: vec4f,
    // Node: FloatInput_LightHardClipFeatherPx
    node_FloatInput_LightHardClipFeatherPx_fa3b19bf: vec4f,
    // Node: FloatInput_VoiceDotMaxHeightPx
    node_FloatInput_VoiceDotMaxHeightPx_0ea7dc97: vec4f,
    // Node: FloatInput_VoiceDotMinHeightPx
    node_FloatInput_VoiceDotMinHeightPx_487cf0b9: vec4f,
    // Node: FloatInput_VoiceDotSdfFeatherPx
    node_FloatInput_VoiceDotSdfFeatherPx_e33ad04e: vec4f,
    // Node: FloatInput_VoiceDotSpacingPx
    node_FloatInput_VoiceDotSpacingPx_6e907c9c: vec4f,
    // Node: FloatInput_VoiceDotWidthPx
    node_FloatInput_VoiceDotWidthPx_57608a19: vec4f,
    // Node: Vector2Input_LightEffectCanvasSizePx
    node_Vector2Input_LightEffectCanvasSizePx_9b9ae39c: vec4f,
    // Node: Vector2Input_LightEffectFrameSizePx
    node_Vector2Input_LightEffectFrameSizePx_0e8bf3b1: vec4f,
    // Node: Vector2Input_LightEffectPositionPx
    node_Vector2Input_LightEffectPositionPx_a9a5e69f: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

struct ShaderMaterialParams {
    shader_GroupInstance_32_ShaderMaterial_voice_dots_human_voice_energies: array<vec4f, 16>,
};
@group(0) @binding(3)
var<storage, read> shader_material_params: ShaderMaterialParams;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;

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

// Independent voice-dot layer ported from audio_bars.agsl.
// It intentionally has no texture or shader dependency on the light layer.

fn catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(local_t: f32, y_im1: f32, y_i: f32, y_ip1: f32, y_ip2: f32) -> f32 {
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

fn catmull_values_GroupInstance_32_ShaderMaterial_voice_dots(t: f32, values: array<f32, 16>) -> f32 {
    let segment = clamp(t, 0.0, 1.0) * 14.0;
    if (segment < 1.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment, values[0], values[0], values[1], values[2]);
    }
    if (segment < 2.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 1.0, values[0], values[1], values[2], values[3]);
    }
    if (segment < 3.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 2.0, values[1], values[2], values[3], values[4]);
    }
    if (segment < 4.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 3.0, values[2], values[3], values[4], values[5]);
    }
    if (segment < 5.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 4.0, values[3], values[4], values[5], values[6]);
    }
    if (segment < 6.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 5.0, values[4], values[5], values[6], values[7]);
    }
    if (segment < 7.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 6.0, values[5], values[6], values[7], values[8]);
    }
    if (segment < 8.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 7.0, values[6], values[7], values[8], values[9]);
    }
    if (segment < 9.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 8.0, values[7], values[8], values[9], values[10]);
    }
    if (segment < 10.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 9.0, values[8], values[9], values[10], values[11]);
    }
    if (segment < 11.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 10.0, values[9], values[10], values[11], values[12]);
    }
    if (segment < 12.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 11.0, values[10], values[11], values[12], values[13]);
    }
    if (segment < 13.0) {
        return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 12.0, values[11], values[12], values[13], values[14]);
    }
    return catmull_segment_GroupInstance_32_ShaderMaterial_voice_dots(segment - 13.0, values[12], values[13], values[14], values[14]);
}

fn calm_human_voice_GroupInstance_32_ShaderMaterial_voice_dots(t: f32, values: array<f32, 16>) -> f32 {
    let dx = 0.75 / 14.0;
    return catmull_values_GroupInstance_32_ShaderMaterial_voice_dots(clamp(t - dx, 0.0, 1.0), values) * 0.15
        + catmull_values_GroupInstance_32_ShaderMaterial_voice_dots(t, values) * 0.70
        + catmull_values_GroupInstance_32_ShaderMaterial_voice_dots(clamp(t + dx, 0.0, 1.0), values) * 0.15;
}

fn smooth5_map_GroupInstance_32_ShaderMaterial_voice_dots(value: f32) -> f32 {
    var t = mix(0.5, 1.0, clamp(value, 0.0, 1.0));
    t = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    return (t - 0.5) * 2.0;
}

fn sd_rounded_box_GroupInstance_32_ShaderMaterial_voice_dots(point: vec2f, half_size: vec2f, radius: f32) -> f32 {
    let q = abs(point) - half_size + vec2f(radius);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2f(0.0))) - radius;
}

fn supercircle_sdf_GroupInstance_32_ShaderMaterial_voice_dots(point: vec2f, center: vec2f, radius: f32, axis_mix: vec2f) -> f32 {
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

fn smooth_corner_sdf_GroupInstance_32_ShaderMaterial_voice_dots(point: vec2f, half_size: vec2f, radius: f32) -> f32 {
    let safe_half_size = max(half_size, vec2f(0.0001));
    if (radius <= 0.0001) {
        return sd_rounded_box_GroupInstance_32_ShaderMaterial_voice_dots(point, safe_half_size, 0.0);
    }
    let radius_ratio = clamp(vec2f(radius) / safe_half_size, vec2f(0.0), vec2f(1.0));
    let ratio = clamp((radius_ratio - vec2f(0.6)) / 0.4, vec2f(0.0), vec2f(1.0));
    return supercircle_sdf_GroupInstance_32_ShaderMaterial_voice_dots(abs(point), safe_half_size, radius, ratio);
}

fn glass_frame_sdf_GroupInstance_32_ShaderMaterial_voice_dots(point: vec2f, half_size: vec2f, radius: f32) -> f32 {
    let shape_sdf = smooth_corner_sdf_GroupInstance_32_ShaderMaterial_voice_dots(point, half_size, radius);
    let layer_bounds_sdf = sd_rounded_box_GroupInstance_32_ShaderMaterial_voice_dots(point, half_size, 0.0);
    return max(shape_sdf, layer_bounds_sdf);
}

fn voice_dot_sample_GroupInstance_32_ShaderMaterial_voice_dots(
    point: vec2f,
    index: f32,
    energy: f32,
    dot_width_px: f32,
    dot_min_height_px: f32,
    dot_max_height_px: f32,
    dot_spacing_px: f32,
    sdf_feather_px: f32,
    progress: f32,
    response: f32,
) -> f32 {
    let center_distance = abs(index - 17.0);
    let mapped_energy = smooth5_map_GroupInstance_32_ShaderMaterial_voice_dots(clamp(energy * response * 1.5, 0.0, 1.0));
    let dot_size = vec2f(dot_width_px, mix(dot_min_height_px, dot_max_height_px, mapped_energy));
    let radius = min(dot_size.x, dot_size.y) * 0.5;
    let x = (index - 17.0) * dot_spacing_px;
    let sdf = -sd_rounded_box_GroupInstance_32_ShaderMaterial_voice_dots(point - vec2f(x, 0.0), dot_size * 0.5, radius);
    let visible = smoothstep(
        (center_distance - 0.5) / 17.5,
        (center_distance + 0.5) / 17.5,
        clamp(progress, 0.0, 1.0),
    );
    return smoothstep(0.0, max(sdf_feather_px, 0.0001), sdf) * visible;
}

fn voice_dot_alpha_GroupInstance_32_ShaderMaterial_voice_dots(
    point: vec2f,
    dot_width_px: f32,
    dot_min_height_px: f32,
    dot_max_height_px: f32,
    dot_spacing_px: f32,
    sdf_feather_px: f32,
    energies: array<f32, 16>,
    opacity: f32,
    progress: f32,
    response: f32,
) -> f32 {
    var alpha = 0.0;
    for (var index = 0; index < 35; index += 1) {
        let sample_t = f32(index) / 34.0;
        let energy = calm_human_voice_GroupInstance_32_ShaderMaterial_voice_dots(sample_t, energies);
        alpha = max(
            alpha,
            voice_dot_sample_GroupInstance_32_ShaderMaterial_voice_dots(
                point,
                f32(index),
                energy,
                dot_width_px,
                dot_min_height_px,
                dot_max_height_px,
                dot_spacing_px,
                sdf_feather_px,
                progress,
                response,
            ),
        );
    }
    return alpha * clamp(opacity, 0.0, 1.0);
}

fn shader_material_GroupInstance_32_ShaderMaterial_voice_dots(
    in: ShaderMaterialInput,
    frame_size_px: vec2f,
    corner_radius_px: f32,
    hard_clip_feather_px: f32,
    dot_width_px: f32,
    dot_min_height_px: f32,
    dot_max_height_px: f32,
    dot_spacing_px: f32,
    dot_sdf_feather_px: f32,
    human_voice_energies: array<f32, 16>,
    voice_dot_opacity: f32,
    voice_dot_progress: f32,
    voice_dot_response: f32,
    voice_dot_color: vec4f,
) -> vec4f {
    let canvas_size_px = max(in.geometry_size, vec2f(1.0));
    let size_px = clamp(frame_size_px, vec2f(0.0001), canvas_size_px);
    let point = in.local_position.xy - canvas_size_px * 0.5;
    let half_size_px = size_px * 0.5;
    let radius_px = clamp(corner_radius_px, 0.0, min(half_size_px.x, half_size_px.y));
    let hard_clip_alpha = 1.0 - smoothstep(
        -max(hard_clip_feather_px, 0.0001),
        0.0,
        glass_frame_sdf_GroupInstance_32_ShaderMaterial_voice_dots(point, half_size_px, radius_px),
    );
    let dot_alpha = voice_dot_alpha_GroupInstance_32_ShaderMaterial_voice_dots(
        point,
        dot_width_px,
        dot_min_height_px,
        dot_max_height_px,
        dot_spacing_px,
        dot_sdf_feather_px,
        human_voice_energies,
        voice_dot_opacity,
        voice_dot_progress,
        voice_dot_response,
    );
    let coverage = clamp(dot_alpha * hard_clip_alpha, 0.0, 1.0);
    return vec4f(
        voice_dot_color.rgb * voice_dot_color.a * coverage,
        coverage,
    );
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
 out.uv = vec2f(uv.x, 1.0 - uv.y);

 let rect_size_px_base = (graph_inputs.node_Vector2Input_LightEffectCanvasSizePx_9b9ae39c).xy;
 let rect_center_px = (graph_inputs.node_Vector2Input_LightEffectPositionPx_a9a5e69f).xy;
 let rect_dyn = vec4f(rect_center_px, rect_size_px_base);
 out.geo_size_px = rect_dyn.zw;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(uv * out.geo_size_px, 0.0);

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