// HyperOS Glass Material — Complete Shader Template
//
// This is the single source of truth for the HyperOS glass material shader.
// It is embedded at compile time via include_str! and markers are substituted
// with resolved expressions from the node graph.
//
// Structure:
//   [HELPERS]  — static WGSL helper functions (no markers)
//   [BODY]     — fragment body with {{marker}} placeholders
//
// Markers:
//   {{name}}                — replaced with a dynamic expression
//   {{#if feature}}...{{/if}} — conditional block (included only when feature is connected)
//
// The template is split at the "// {{BODY}}" marker line:
//   - Everything above it goes into extra_wgsl_decls (helper functions)
//   - Everything below it becomes the fragment body (with substitution)

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

fn hyperos_glass_luma(color: vec3f) -> f32 {
    return dot(color, vec3f(0.2126, 0.7152, 0.0722));
}

fn hyperos_glass_blend_normal(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb * (1.0 - src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn hyperos_glass_blend_darken(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - max(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn hyperos_glass_blend_multiply(src: vec4f, dst: vec4f) -> vec4f {
    return src * (1.0 - dst.a) + dst * (1.0 - src.a) + src * dst;
}

fn hyperos_glass_blend_plus_darker(src: vec4f, dst: vec4f) -> vec4f {
    let a = src.a + (1.0 - src.a) * dst.a;
    let color = max(vec3f(0.0), vec3f(a) - (dst.a - dst.rgb) - (src.a - src.rgb));
    return vec4f(color, a);
}

fn hyperos_glass_blend_color_burn_component(src: vec2f, dst: vec2f) -> f32 {
    let t = select(0.0, dst.y, dst.y == dst.x);
    let d = select(
        t,
        dst.y - min(dst.y, (dst.y - dst.x) * src.y / (src.x + 0.001)),
        abs(src.x) > 0.0,
    );
    return (d * src.y + src.x * (1.0 - dst.y)) + dst.x * (1.0 - src.y);
}

fn hyperos_glass_blend_color_burn(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        hyperos_glass_blend_color_burn_component(src.ra, dst.ra),
        hyperos_glass_blend_color_burn_component(src.ga, dst.ga),
        hyperos_glass_blend_color_burn_component(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn hyperos_glass_blend_lighten(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - min(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn hyperos_glass_blend_screen(src: vec4f, dst: vec4f) -> vec4f {
    return vec4f(
        vec3f(1.0) - (vec3f(1.0) - src.rgb) * (vec3f(1.0) - dst.rgb),
        src.a + dst.a * (1.0 - src.a),
    );
}

fn hyperos_glass_blend_plus_lighter(src: vec4f, dst: vec4f) -> vec4f {
    let color = min(src.rgb + dst.rgb, vec3f(1.0));
    let alpha = src.a + (1.0 - src.a) * dst.a;
    return vec4f(color, alpha);
}

fn hyperos_glass_blend_color_dodge_component(src: vec2f, dst: vec2f) -> f32 {
    let dx_scale = select(1.0, 0.0, dst.x == 0.0);
    let dodge = select(
        dst.y,
        (dst.x * src.y) / ((src.y - src.x) + 0.001),
        abs(src.y - src.x) > 0.0,
    );
    let delta = dx_scale * min(dst.y, dodge);
    return (delta * src.y + src.x * (1.0 - dst.y)) + dst.x * (1.0 - src.y);
}

fn hyperos_glass_blend_color_dodge(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        hyperos_glass_blend_color_dodge_component(src.ra, dst.ra),
        hyperos_glass_blend_color_dodge_component(src.ga, dst.ga),
        hyperos_glass_blend_color_dodge_component(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn hyperos_glass_blend_overlay_component(src: vec2f, dst: vec2f) -> f32 {
    if (2.0 * dst.x <= dst.y) {
        return (2.0 * src.x) * dst.x;
    }
    return src.y * dst.y - (2.0 * (dst.y - dst.x)) * (src.y - src.x);
}

fn hyperos_glass_blend_overlay(src: vec4f, dst: vec4f) -> vec4f {
    var c = vec3f(
        hyperos_glass_blend_overlay_component(src.ra, dst.ra),
        hyperos_glass_blend_overlay_component(src.ga, dst.ga),
        hyperos_glass_blend_overlay_component(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    c = c + dst.rgb * (1.0 - src.a) + src.rgb * (1.0 - dst.a);
    return vec4f(c, a);
}

fn hyperos_glass_blend_soft_light_component(src: vec2f, dst: vec2f) -> f32 {
    let eps = 0.0;
    if (2.0 * src.x <= src.y) {
        return (((dst.x * dst.x) * (src.y - 2.0 * src.x)) / (dst.y + eps) + (1.0 - dst.y) * src.x)
            + dst.x * ((-src.y + 2.0 * src.x) + 1.0);
    }
    if (4.0 * dst.x <= dst.y) {
        let d_sqd = dst.x * dst.x;
        let d_cub = d_sqd * dst.x;
        let da_sqd = dst.y * dst.y;
        let da_cub = da_sqd * dst.y;
        return (((da_sqd * (src.x - dst.x * ((3.0 * src.y - 6.0 * src.x) - 1.0))
            + ((12.0 * dst.y) * d_sqd) * (src.y - 2.0 * src.x))
            - (16.0 * d_cub) * (src.y - 2.0 * src.x))
            - da_cub * src.x)
            / (da_sqd + eps);
    }
    return ((dst.x * ((src.y - 2.0 * src.x) + 1.0) + src.x)
        - sqrt(dst.y * dst.x) * (src.y - 2.0 * src.x))
        - dst.y * src.x;
}

fn hyperos_glass_blend_soft_light(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        hyperos_glass_blend_soft_light_component(src.ra, dst.ra),
        hyperos_glass_blend_soft_light_component(src.ga, dst.ga),
        hyperos_glass_blend_soft_light_component(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn hyperos_glass_blend_hard_light(src: vec4f, dst: vec4f) -> vec4f {
    return hyperos_glass_blend_overlay(dst, src);
}

fn hyperos_glass_blend_difference(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - 2.0 * min(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn hyperos_glass_blend_exclusion(src: vec4f, dst: vec4f) -> vec4f {
    let c = (dst.rgb + src.rgb) - (2.0 * dst.rgb * src.rgb);
    let a = src.a + (1.0 - src.a) * dst.a;
    return vec4f(c, a);
}

fn hyperos_glass_blend_color_saturation(color: vec3f) -> f32 {
    return max(max(color.x, color.y), color.z) - min(min(color.x, color.y), color.z);
}

fn hyperos_glass_blend_hsl_color(flip_sat: vec2f, src: vec4f, dst: vec4f) -> vec4f {
    let eps = 0.0;
    let min_normal_half = 6.10351562e-05;
    let alpha = dst.a * src.a;
    let sda = src.rgb * dst.a;
    let dsa = dst.rgb * src.a;
    let flip_x = flip_sat.x != 0.0;
    let flip_y = flip_sat.y != 0.0;
    var l = sda;
    var r = dsa;
    if (flip_x) {
        l = dsa;
        r = sda;
    }
    if (flip_y) {
        let mn = min(min(l.x, l.y), l.z);
        let mx = max(max(l.x, l.y), l.z);
        if (mx > mn) {
            l = ((l - vec3f(mn)) * hyperos_glass_blend_color_saturation(r)) / (mx - mn);
        } else {
            l = vec3f(0.0);
        }
        r = dsa;
    }
    let lum = dot(vec3f(0.3, 0.59, 0.11), r);
    var result = vec3f(lum) - vec3f(dot(vec3f(0.3, 0.59, 0.11), l)) + l;
    let min_comp = min(min(result.x, result.y), result.z);
    let max_comp = max(max(result.x, result.y), result.z);
    if (min_comp < 0.0 && lum != min_comp) {
        result = vec3f(lum) + (result - vec3f(lum)) * (lum / ((lum - min_comp + min_normal_half) + eps));
    }
    if (max_comp > alpha && max_comp != lum) {
        result = vec3f(lum) + ((result - vec3f(lum)) * (alpha - lum)) / ((max_comp - lum + min_normal_half) + eps);
    }
    return vec4f(((result + dst.rgb) - dsa + src.rgb) - sda, src.a + dst.a - alpha);
}

fn hyperos_glass_blend_hue(src: vec4f, dst: vec4f) -> vec4f {
    return hyperos_glass_blend_hsl_color(vec2f(0.0, 1.0), src, dst);
}

fn hyperos_glass_blend_saturation(src: vec4f, dst: vec4f) -> vec4f {
    return hyperos_glass_blend_hsl_color(vec2f(1.0, 1.0), src, dst);
}

fn hyperos_glass_blend_color(src: vec4f, dst: vec4f) -> vec4f {
    return hyperos_glass_blend_hsl_color(vec2f(0.0, 0.0), src, dst);
}

fn hyperos_glass_blend_luminance(src: vec4f, dst: vec4f) -> vec4f {
    return hyperos_glass_blend_hsl_color(vec2f(1.0, 0.0), src, dst);
}

fn hyperos_glass_luminance_curve(color: vec4f, factors: vec4f, mix_factor: f32) -> vec4f {
    let factor_adjust = vec4f(
        -1.0 * factors.x + 3.0 * factors.y - 3.0 * factors.z + 1.0 * factors.w,
        3.0 * factors.x - 6.0 * factors.y + 3.0 * factors.z,
        -3.0 * factors.x + 3.0 * factors.y,
        factors.x,
    );

    let alpha = max(color.a, 0.0001);
    let scale = 1.0 / alpha;
    let scaled_rgb = scale * color.rgb;
    let luminance = dot(scaled_rgb, vec3f(0.2125, 0.7153, 0.0721));

    var adj = luminance * factor_adjust.x + factor_adjust.y;
    adj = adj * luminance + factor_adjust.z;
    adj = adj * luminance + factor_adjust.w;

    let ray_scale = select(0.0, adj / luminance, luminance > 0.0001);
    let mapped_rgb = scaled_rgb * ray_scale;
    let mixed = mix(scaled_rgb, mapped_rgb, mix_factor);
    let result_rgb = mixed * alpha;

    return vec4f(result_rgb, color.a);
}

fn hyperos_glass_adjust_color(color: vec4f, saturation: f32, brightness: f32) -> vec4f {
    let luminance = dot(color.rgb, vec3f(0.2125, 0.7153, 0.0721));
    let adjusted_saturation = saturation * color.rgb + (1.0 - saturation) * vec3f(luminance);
    let alpha = color.a;
    let adjusted_brightness = adjusted_saturation + vec3f(brightness * alpha);
    return vec4f(adjusted_brightness, alpha);
}

fn hyperos_glass_process_color(
    color: vec4f,
    luminance_values: vec4f,
    luminance_amount: f32,
    saturation: f32,
    brightness: f32,
) -> vec4f {
    var c = hyperos_glass_luminance_curve(color, luminance_values, luminance_amount);
    c = vec4f(hyperos_glass_adjust_color(c, saturation, brightness).rgb, c.a);
    return c;
}

fn hyperos_glass_smooth5_map(t_in: f32) -> f32 {
    var t = mix(0.5, 1.0, t_in);
    t = clamp(t, 0.0, 1.0);
    t = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    return (t - 0.5) * 2.0;
}

fn hyperos_glass_smooth5_vertical(x: f32, k: f32) -> f32 {
    let t = pow(clamp(x, 0.0, 1.0), k);
    return hyperos_glass_smooth5_map(t);
}

fn hyperos_glass_curve(x_in: f32, pow_ratio: f32) -> f32 {
    if (x_in >= 0.85) {
        return 1.0;
    }
    let x = clamp(x_in, 0.0, 1.0);
    let circle = hyperos_glass_smooth5_vertical(x, 0.5);
    return 1.0 - pow(1.0 - circle, pow_ratio);
}

fn hyperos_glass_shape_sdf(p: vec2f, b: vec2f, r: f32, edge: f32, edge_pow: f32) -> f32 {
    let k1 = 0.6;
    let safe_b = max(b, vec2f(1e-6));
    let ratio = clamp((clamp(vec2f(r) / safe_b, vec2f(0.0), vec2f(1.0)) - vec2f(k1)) / vec2f(1.0 - k1), vec2f(0.0), vec2f(1.0));
    let safe_edge = max(edge, 1e-6);
    var d = sdf2d_smooth_round_rect(abs(p), b, r, ratio).x;
    if (d < -safe_edge) {
        d = -safe_edge;
    } else if (d < 0.0) {
        let per = -d / safe_edge;
        d = -hyperos_glass_curve(per, edge_pow) * safe_edge;
    }
    return d;
}

fn hyperos_glass_calculate_normal(pos_from_center: vec2f, half_size_px: vec2f, radius_px: f32, edge: f32, edge_pow: f32) -> vec3f {
    let eps = 0.001;
    let right_sdf = hyperos_glass_shape_sdf(pos_from_center + vec2f(eps, 0.0), half_size_px, radius_px, edge, edge_pow);
    let left_sdf = hyperos_glass_shape_sdf(pos_from_center - vec2f(eps, 0.0), half_size_px, radius_px, edge, edge_pow);
    let top_sdf = hyperos_glass_shape_sdf(pos_from_center + vec2f(0.0, eps), half_size_px, radius_px, edge, edge_pow);
    let bottom_sdf = hyperos_glass_shape_sdf(pos_from_center - vec2f(0.0, eps), half_size_px, radius_px, edge, edge_pow);
    let xy_gradient = vec2f((right_sdf - left_sdf) / (2.0 * eps), (top_sdf - bottom_sdf) / (2.0 * eps));
    return normalize(vec3f(xy_gradient, 1.0));
}

fn hyperos_glass_get_lighten(fg_tex: texture_2d<f32>, fg_samp: sampler, uv: vec2f) -> f32 {
    let fg_col = textureSample(fg_tex, fg_samp, uv);
    return fg_col.r * fg_col.r;
}

fn hyperos_glass_hsvv(col: vec3f, lighten: f32) -> vec3f {
    let v = hyperos_glass_luma(col);
    let w = smoothstep(0.0, 0.5, v);
    let k = mix(1.0 - v, v, w);
    let g = 1.0 + smoothstep(0.0, 1.0, lighten) * mix(0.75, 0.4, w);
    return (col + vec3f(k)) * g - vec3f(k);
}

fn hyperos_glass_blend_reflect_light(dst: vec4f, lighten: f32, blend_mode: i32) -> vec4f {
    let src = vec4f(vec3f(lighten), lighten);
    switch (blend_mode) {
        case 1: {
            return hyperos_glass_blend_normal(src, dst);
        }
        case 2: {
            return hyperos_glass_blend_darken(src, dst);
        }
        case 3: {
            return hyperos_glass_blend_multiply(src, dst);
        }
        case 4: {
            return hyperos_glass_blend_plus_darker(src, dst);
        }
        case 5: {
            return hyperos_glass_blend_color_burn(src, dst);
        }
        case 6: {
            return hyperos_glass_blend_lighten(src, dst);
        }
        case 7: {
            return hyperos_glass_blend_screen(src, dst);
        }
        case 8: {
            return hyperos_glass_blend_plus_lighter(src, dst);
        }
        case 9: {
            return hyperos_glass_blend_color_dodge(src, dst);
        }
        case 10: {
            return hyperos_glass_blend_overlay(src, dst);
        }
        case 11: {
            return hyperos_glass_blend_soft_light(src, dst);
        }
        case 12: {
            return hyperos_glass_blend_hard_light(src, dst);
        }
        case 13: {
            return hyperos_glass_blend_difference(src, dst);
        }
        case 14: {
            return hyperos_glass_blend_exclusion(src, dst);
        }
        case 15: {
            return hyperos_glass_blend_hue(src, dst);
        }
        case 16: {
            return hyperos_glass_blend_saturation(src, dst);
        }
        case 17: {
            return hyperos_glass_blend_color(src, dst);
        }
        case 18: {
            return hyperos_glass_blend_luminance(src, dst);
        }
        default: {
            return vec4f(hyperos_glass_hsvv(dst.rgb, lighten), dst.a);
        }
    }
}

fn hyperos_glass_dynamic_add(color: vec3f) -> f32 {
    var white_dis = distance(vec3f(1.0), color);
    white_dis = smoothstep(0.2, 1.0, white_dis);
    white_dis = mix(0.5, 1.0, white_dis);
    let lumin = hyperos_glass_luma(color);
    return mix(0.5, white_dis, lumin);
}

fn hyperos_glass_add_light(color: vec3f, light_color: vec3f, light_strength: f32) -> vec3f {
    var white_dis = distance(vec3f(1.0), color);
    white_dis = smoothstep(0.2, 1.0, white_dis);
    white_dis = mix(0.3, 1.0, white_dis);
    return color + light_color * (light_strength * white_dis);
}

fn hyperos_glass_calculate_lighting(normal: vec3f, light_dir: vec3f, intensity: f32, angle_range: f32) -> f32 {
    let normalized_light_dir = normalize(light_dir);
    let dot_product = dot(normal, normalized_light_dir);
    let reflection_angle = acos(clamp(dot_product, -1.0, 1.0));
    let angle_factor = 1.0 - (reflection_angle / (3.14159 * angle_range));
    let adjusted_intensity = max(intensity * angle_factor, 0.0);
    return max(dot_product, 0.0) * adjusted_intensity;
}

fn hyperos_glass_sample_screen_uv(screen_px: vec2f, resolution: vec2f) -> vec2f {
    return vec2f(screen_px.x, resolution.y - screen_px.y) / resolution;
}

fn hyperos_glass_texture_map(
    tex: texture_2d<f32>,
    samp: sampler,
    sample_uv: vec2f,
    add_foreground: bool,
    reflect_lighten_opacity: f32,
    reflect_lighten_blend_mode: i32,
    fg_tex: texture_2d<f32>,
    fg_samp: sampler,
    frag_uv: vec2f,
) -> vec4f {
    var col = textureSample(tex, samp, sample_uv);

    if (add_foreground) {
        let lighten = hyperos_glass_get_lighten(fg_tex, fg_samp, frag_uv);
        col = hyperos_glass_blend_reflect_light(col, lighten * reflect_lighten_opacity, reflect_lighten_blend_mode);
    }

    return col;
}

// {{BODY}}

// ============================================================================
// FRAGMENT BODY
// ============================================================================

 // HyperOSGlassMaterial({{node_id}})
 var {{out_var}}: vec4f;
 {
     let screen_px = in.frag_coord_gl;
     let local_px = in.local_px.xy;
     let size_px = in.geo_size_px;
     let safe_size_px = max(size_px, vec2f(1e-6));
     let half_size_px = size_px * 0.5;
     let pos_from_center = local_px - half_size_px;
     let geo_origin_px = screen_px - local_px;
     let screen_uv = hyperos_glass_sample_screen_uv(screen_px, params.target_size);

     // --- Shape SDF ---
     let edge = f32({{uShapeEdgePx}});
     let edge_pow = f32({{uShapeEdgePow}});
     let radius_px = {{radius_expr}};
     let safe_edge = max(edge, 1e-6);
     let box_sdf = hyperos_glass_shape_sdf(pos_from_center, half_size_px, radius_px, edge, edge_pow);
     let normalized_sdf = -box_sdf / safe_edge;
     let edge_sdf = hyperos_glass_shape_sdf(pos_from_center, half_size_px, radius_px, edge, 1.0);
     let edge_normalized_sdf = -edge_sdf / safe_edge;
     let light_width = edge;
     let light_edge_pow = edge_pow;
     let light_box_sdf = hyperos_glass_shape_sdf(pos_from_center, half_size_px, radius_px, light_width, light_edge_pow);
     let light_normalized_sdf = -light_box_sdf / max(light_width, 1e-6);
     let normal = hyperos_glass_calculate_normal(pos_from_center, half_size_px, radius_px, edge, edge_pow);
     let light_normal = hyperos_glass_calculate_normal(pos_from_center, half_size_px, radius_px, light_width, light_edge_pow);

     // --- Anti-aliasing ---
     let aa_depth = edge * hyperos_glass_curve(clamp(f32({{uDebugShapeExpand}}) / max(edge, 0.0001), 0.0, 1.0), 1.0);
     var final_alpha = smoothstep(0.0, aa_depth, -edge_sdf);

{{#if use_sdf_tex}}
     if ({{uUseSdfTex}}) {
         let sdf_col = textureSample({{sdf_tex_var}}, {{sdf_samp_var}}, in.uv);
         final_alpha = clamp(sdf_col.w, 0.0, 1.0);
     }
{{/if}}

     // --- Refraction ---
     let uv_display_px = (local_px - half_size_px) * f32({{uRefractBlurScale}}) + half_size_px;
     let incident_ray = normalize(vec3f(0.0, 0.0, -1.0));
     let refractive_index = f32({{uRefractIOR}});
     let refract_dir = refract(incident_ray, normal, 1.0 / max(refractive_index, 1e-6));
     let refract_thickness = mix((f32({{uShapeThicknessPx}}) - edge) * 2.0, f32({{uShapeThicknessPx}}) * 2.0, normalized_sdf);
     let refract_local_px = uv_display_px + refract_dir.xy * refract_thickness;
     let refract_uv = hyperos_glass_sample_screen_uv(geo_origin_px + refract_local_px, params.target_size);
     var refraction = {{refraction_expr}};
     {
         let l = hyperos_glass_luma(refraction.rgb);
         let dark_target = 0.1 * (vec3f(1.0) - vec3f(0.2126, 0.7152, 0.0722));
         refraction = vec4f(mix(refraction.rgb, dark_target, mix(0.0, f32({{uBlendDarker}}), smoothstep(f32({{uBlendDarkerRangeX}}), f32({{uBlendDarkerRangeY}}), l))), refraction.a);
     }

     // --- Reflection ---
     let reflect_dir = reflect(incident_ray, normal);
     let reflect_local_px = uv_display_px + reflect_dir.xy * mix(0.0, f32({{uShapeReflectOffsetPx}}), 1.0 - normalized_sdf);
     let reflect_uv = hyperos_glass_sample_screen_uv(geo_origin_px + reflect_local_px, params.target_size);
     var reflection = {{reflection_expr}};
     {
         let l = hyperos_glass_luma(reflection.rgb);
         let dark_target = 0.1 * (vec3f(1.0) - vec3f(0.2126, 0.7152, 0.0722));
         reflection = vec4f(mix(reflection.rgb, dark_target, mix(0.0, f32({{uBlendDarker}}), smoothstep(f32({{uBlendDarkerRangeX}}), f32({{uBlendDarkerRangeY}}), l))), reflection.a);
     }

     // --- Mix refraction + reflection ---
     var hyperos_glass_mat = mix(refraction, reflection, (1.0 - edge_normalized_sdf) * f32({{uReflectStrength}}));
     hyperos_glass_mat = vec4f(hyperos_glass_add_light(hyperos_glass_mat.rgb, reflection.rgb, (1.0 - light_normalized_sdf) * f32({{uReflectLighten}})), hyperos_glass_mat.a);

     // --- Process color (luminance curve + saturation + brightness) ---
     hyperos_glass_mat = hyperos_glass_luminance_curve(hyperos_glass_mat, {{uBlendLuminanceValues}}, f32({{uBlendLuminanceAmount}}));
     hyperos_glass_mat = vec4f(hyperos_glass_adjust_color(hyperos_glass_mat, f32({{uBlendSaturation}}), f32({{uBlendBrightness}})).rgb, hyperos_glass_mat.a);

     // --- Background color tinting ---
     var hyperos_glass_color = {{bg_color_expr}};
     {
         let l = hyperos_glass_luma(hyperos_glass_color.rgb);
         let dark_target = 0.1 * (vec3f(1.0) - vec3f(0.2126, 0.7152, 0.0722));
         hyperos_glass_color = vec4f(mix(hyperos_glass_color.rgb, dark_target, mix(0.0, f32({{uBlendDarker}}), smoothstep(f32({{uBlendDarkerRangeX}}), f32({{uBlendDarkerRangeY}}), l))), hyperos_glass_color.a);
     }
     hyperos_glass_color = hyperos_glass_adjust_color(hyperos_glass_color, f32({{uBgColorSaturation}}), f32({{uBgColorBrightness}}));
     hyperos_glass_color = vec4f(mix(hyperos_glass_color.rgb, vec3f(1.0), f32({{uInnerColorWhite}})), hyperos_glass_color.a);
     let hyperos_glass_color_luma = clamp(hyperos_glass_luma(hyperos_glass_color.rgb), 0.0, 1.0);

     // --- Bionic burn color ratio ---
     var color_ratio = 1.0;
     let burn_term = pow(hyperos_glass_color_luma, f32({{uBionicBurn}})) - 0.5;
     let burn_mix = 1.587 * burn_term * burn_term * burn_term + 0.5;
     color_ratio = mix(hyperos_glass_color_luma, color_ratio, burn_mix);
     color_ratio = color_ratio * 0.8;
     var hyperos_glass_color_ratio = mix(vec3f(1.0), hyperos_glass_color.rgb, color_ratio);

     // --- Neutral vibrancy fix ---
     if (f32({{uDebugFixNeutralVibrancy}}) > 0.0) {
         let mean_hyperos_glass_mat_color = (hyperos_glass_mat.r + hyperos_glass_mat.g + hyperos_glass_mat.b) / 3.0;
         let mean_hyperos_glass_color_ratio = (hyperos_glass_color_ratio.r + hyperos_glass_color_ratio.g + hyperos_glass_color_ratio.b) / 3.0;
         let neutral_threshold_min = clamp(f32({{uDebugFixNeutralVibrancyThresholdMin}}), 0.0, 1.0);
         var neutral_threshold = clamp(f32({{uDebugFixNeutralVibrancyThreshold}}), 0.0, 1.0);
         neutral_threshold = max(neutral_threshold, neutral_threshold_min + 0.0001);
         let grayness = distance(hyperos_glass_mat.rgb, vec3f(mean_hyperos_glass_mat_color));
         hyperos_glass_color_ratio = mix(vec3f(mean_hyperos_glass_color_ratio) * 0.5 + hyperos_glass_color_ratio * 0.5, hyperos_glass_color_ratio, smoothstep(neutral_threshold_min, neutral_threshold, grayness));
     }

     // --- Apply inner glass color ---
     hyperos_glass_color_ratio = mix(hyperos_glass_color_ratio, {{uInnerGlassColor}}.rgb, {{uInnerGlassColor}}.a);

     // --- Apply color ratio + inner color ---
     hyperos_glass_mat = vec4f(hyperos_glass_mat.rgb * hyperos_glass_color_ratio, hyperos_glass_mat.a);
     hyperos_glass_mat = vec4f(mix(hyperos_glass_mat.rgb, hyperos_glass_color_ratio, f32({{uInnerColorMix}}) * color_ratio), hyperos_glass_mat.a);
     hyperos_glass_mat = vec4f(hyperos_glass_mat.rgb + vec3f(pow(smoothstep(1.0, 0.0, 1.0 - in.uv.y), 2.0) * f32({{uInnerBottom}})), hyperos_glass_mat.a);

     // --- Directional lighting ---
     let lighting1 = hyperos_glass_calculate_lighting(light_normal, {{uDirectionalLightDirection}}, f32({{uDirectionalLightIntensity}}), f32({{uDirectionalLightAngleRange}}));
     let lighting2 = hyperos_glass_calculate_lighting(light_normal, {{uDirectionalLightDirection}} * vec3f(-1.0, -1.0, 1.0), f32({{uDirectionalLightOppositeIntensity}}), f32({{uDirectionalLightAngleRange}}));
     let light_ratio = hyperos_glass_dynamic_add(hyperos_glass_mat.rgb);
     hyperos_glass_mat = vec4f(hyperos_glass_hsvv(hyperos_glass_mat.rgb, (lighting1 + lighting2) * 2.0 * light_ratio), hyperos_glass_mat.a);

     // --- Final adjustments ---
     hyperos_glass_mat = vec4f(pow(hyperos_glass_mat.rgb, vec3f(f32({{uInnerColorPow}}))), hyperos_glass_mat.a);

     // --- Apply unshade factor ---
     hyperos_glass_mat = vec4f(mix(hyperos_glass_mat.rgb, {{uInnerGlassColor}}.rgb, f32({{uBionicUnShade}})), hyperos_glass_mat.a);

     // --- Apply overall alpha ---
     hyperos_glass_mat = vec4f(hyperos_glass_mat.rgb, hyperos_glass_mat.a * final_alpha * f32({{uAlpha}}));

     // --- Premultiply alpha ---
     hyperos_glass_mat = vec4f(hyperos_glass_mat.rgb * hyperos_glass_mat.a, hyperos_glass_mat.a);
     {{out_var}} = hyperos_glass_mat;
 }
