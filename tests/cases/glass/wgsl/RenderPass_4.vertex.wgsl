
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
     @location(4) instance_index: u32,
 };


struct GraphInputs {
    // Node: Vector3Input_67
    node_Vector3Input_67_698d8c66: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_GroupInstance_47_GuassianBlurPass_42: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_GroupInstance_47_GuassianBlurPass_42: sampler;


// --- Extra WGSL declarations (generated) ---

// ---- GlassMaterial helpers (generated) ----

fn glass_luma(color: vec3f) -> f32 {
    return dot(color, vec3f(0.2126, 0.7152, 0.0722));
}

fn glass_blend_normal(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb * (1.0 - src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn glass_blend_darken(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - max(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn glass_blend_multiply(src: vec4f, dst: vec4f) -> vec4f {
    return src * (1.0 - dst.a) + dst * (1.0 - src.a) + src * dst;
}

fn glass_blend_plus_darker(src: vec4f, dst: vec4f) -> vec4f {
    let a = src.a + (1.0 - src.a) * dst.a;
    let color = max(vec3f(0.0), vec3f(a) - (dst.a - dst.rgb) - (src.a - src.rgb));
    return vec4f(color, a);
}

fn glass_blend_color_burn_component(src: vec2f, dst: vec2f) -> f32 {
    let t = select(0.0, dst.y, dst.y == dst.x);
    let d = select(
        t,
        dst.y - min(dst.y, (dst.y - dst.x) * src.y / (src.x + 0.001)),
        abs(src.x) > 0.0,
    );
    return (d * src.y + src.x * (1.0 - dst.y)) + dst.x * (1.0 - src.y);
}

fn glass_blend_color_burn(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        glass_blend_color_burn_component(src.ra, dst.ra),
        glass_blend_color_burn_component(src.ga, dst.ga),
        glass_blend_color_burn_component(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn glass_blend_lighten(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - min(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn glass_blend_screen(src: vec4f, dst: vec4f) -> vec4f {
    return vec4f(
        vec3f(1.0) - (vec3f(1.0) - src.rgb) * (vec3f(1.0) - dst.rgb),
        src.a + dst.a * (1.0 - src.a),
    );
}

fn glass_blend_plus_lighter(src: vec4f, dst: vec4f) -> vec4f {
    let color = min(src.rgb + dst.rgb, vec3f(1.0));
    let alpha = src.a + (1.0 - src.a) * dst.a;
    return vec4f(color, alpha);
}

fn glass_blend_color_dodge_component(src: vec2f, dst: vec2f) -> f32 {
    let dx_scale = select(1.0, 0.0, dst.x == 0.0);
    let dodge = select(
        dst.y,
        (dst.x * src.y) / ((src.y - src.x) + 0.001),
        abs(src.y - src.x) > 0.0,
    );
    let delta = dx_scale * min(dst.y, dodge);
    return (delta * src.y + src.x * (1.0 - dst.y)) + dst.x * (1.0 - src.y);
}

fn glass_blend_color_dodge(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        glass_blend_color_dodge_component(src.ra, dst.ra),
        glass_blend_color_dodge_component(src.ga, dst.ga),
        glass_blend_color_dodge_component(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn glass_blend_overlay_component(src: vec2f, dst: vec2f) -> f32 {
    if (2.0 * dst.x <= dst.y) {
        return (2.0 * src.x) * dst.x;
    }
    return src.y * dst.y - (2.0 * (dst.y - dst.x)) * (src.y - src.x);
}

fn glass_blend_overlay(src: vec4f, dst: vec4f) -> vec4f {
    var c = vec3f(
        glass_blend_overlay_component(src.ra, dst.ra),
        glass_blend_overlay_component(src.ga, dst.ga),
        glass_blend_overlay_component(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    c = c + dst.rgb * (1.0 - src.a) + src.rgb * (1.0 - dst.a);
    return vec4f(c, a);
}

fn glass_blend_soft_light_component(src: vec2f, dst: vec2f) -> f32 {
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

fn glass_blend_soft_light(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        glass_blend_soft_light_component(src.ra, dst.ra),
        glass_blend_soft_light_component(src.ga, dst.ga),
        glass_blend_soft_light_component(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn glass_blend_hard_light(src: vec4f, dst: vec4f) -> vec4f {
    return glass_blend_overlay(dst, src);
}

fn glass_blend_difference(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - 2.0 * min(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn glass_blend_exclusion(src: vec4f, dst: vec4f) -> vec4f {
    let c = (dst.rgb + src.rgb) - (2.0 * dst.rgb * src.rgb);
    let a = src.a + (1.0 - src.a) * dst.a;
    return vec4f(c, a);
}

fn glass_blend_color_saturation(color: vec3f) -> f32 {
    return max(max(color.x, color.y), color.z) - min(min(color.x, color.y), color.z);
}

fn glass_blend_hsl_color(flip_sat: vec2f, src: vec4f, dst: vec4f) -> vec4f {
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
            l = ((l - vec3f(mn)) * glass_blend_color_saturation(r)) / (mx - mn);
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

fn glass_blend_hue(src: vec4f, dst: vec4f) -> vec4f {
    return glass_blend_hsl_color(vec2f(0.0, 1.0), src, dst);
}

fn glass_blend_saturation(src: vec4f, dst: vec4f) -> vec4f {
    return glass_blend_hsl_color(vec2f(1.0, 1.0), src, dst);
}

fn glass_blend_color(src: vec4f, dst: vec4f) -> vec4f {
    return glass_blend_hsl_color(vec2f(0.0, 0.0), src, dst);
}

fn glass_blend_luminance(src: vec4f, dst: vec4f) -> vec4f {
    return glass_blend_hsl_color(vec2f(1.0, 0.0), src, dst);
}

fn glass_luminance_curve(color: vec4f, factors: vec4f, mix_factor: f32) -> vec4f {
    let factor_adjust = vec4f(
        -1.0 * factors.x + 3.0 * factors.y - 3.0 * factors.z + 1.0 * factors.w,
        3.0 * factors.x - 6.0 * factors.y + 3.0 * factors.z,
        -3.0 * factors.x + 3.0 * factors.y,
        factors.x,
    );

    let alpha = max(color.a, 0.0001);
    let scale = 1.0 / alpha;
    let scaled_rgb = scale * color.rgb;
    let luminance = clamp(dot(scaled_rgb, vec3f(0.2125, 0.7153, 0.0721)), 0.0, 1.0);

    var adj = luminance * factor_adjust.x + factor_adjust.y;
    adj = adj * luminance + factor_adjust.z;
    adj = adj * luminance + factor_adjust.w;

    let ray_scale = select(0.0, adj / luminance, luminance > 0.0001);
    let mapped_rgb = scaled_rgb * ray_scale;
    let mixed = mix(scaled_rgb, mapped_rgb, mix_factor);

    return vec4f(mixed * alpha, color.a);
}

fn glass_adjust_color(color: vec4f, saturation: f32, brightness: f32) -> vec4f {
    let luminance = dot(color.rgb, vec3f(0.2125, 0.7153, 0.0721));
    let adjusted_saturation = saturation * color.rgb + (1.0 - saturation) * vec3f(luminance);
    let alpha = color.a;
    let adjusted_brightness = adjusted_saturation + vec3f(brightness * alpha);
    return vec4f(adjusted_brightness, alpha);
}

fn glass_process_color(
    color: vec4f,
    luminance_values: vec4f,
    luminance_amount: f32,
    saturation: f32,
    brightness: f32,
) -> vec4f {
    var c = glass_luminance_curve(color, luminance_values, luminance_amount);
    c = vec4f(glass_adjust_color(c, saturation, brightness).rgb, c.a);
    return c;
}

fn glass_smooth5_map(t_in: f32) -> f32 {
    var t = mix(0.5, 1.0, t_in);
    t = clamp(t, 0.0, 1.0);
    t = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    return (t - 0.5) * 2.0;
}

fn glass_smooth5_vertical(x: f32, k: f32) -> f32 {
    let t = pow(clamp(x, 0.0, 1.0), k);
    return glass_smooth5_map(t);
}

fn glass_curve(x_in: f32, pow_ratio: f32) -> f32 {
    if (x_in >= 0.85) {
        return 1.0;
    }
    let x = clamp(x_in, 0.0, 1.0);
    let circle = glass_smooth5_vertical(x, 0.5);
    return 1.0 - pow(1.0 - circle, pow_ratio);
}

fn glass_supercircle_sdf_height(point: vec2f, center: vec2f, radius: f32, axis_mix: vec2f) -> f32 {
    let abs_radius = abs(radius);
    let scaled_radius = 1.5286649465560913 * abs_radius;
    let safe_scaled_radius = max(scaled_radius, 1e-6);
    let blended_radius = mix(scaled_radius, radius, max(axis_mix.x, axis_mix.y));
    let offset = point - center;
    let shifted_pos = vec2f(safe_scaled_radius, safe_scaled_radius) + offset;
    let normalized_pos = max(vec2f(0.0), shifted_pos / safe_scaled_radius);
    let abs_norm_pos = abs(normalized_pos);
    let axis_denom = max(abs_norm_pos.x, abs_norm_pos.y);
    let axis_ratio = select(
        clamp(min(abs_norm_pos.x, abs_norm_pos.y) / axis_denom, 0.0, 1.0),
        0.0,
        axis_denom == 0.0,
    );
    let poly_fit = ((((-0.7391197269 * axis_ratio + 2.4034927648) * axis_ratio - 2.4907319173) * axis_ratio + 0.4768708960) * axis_ratio + 0.4747847594);
    let len_abs = length(abs_norm_pos);
    let denom = 1.0 - axis_ratio * axis_ratio * clamp(len_abs, 0.0, 1.0) * poly_fit;
    let safe_denom = select(denom, 1e-6, abs(denom) < 1e-6);
    let dist_base = (len_abs + 1.0) - 1.0 / safe_denom;
    let dist_alt = 0.6541655659675598 * length(max(vec2f(0.0), 1.5286649465560913 * abs_norm_pos - vec2f(0.5286650061607361))) + 0.3458344340324402;
    let dist_mix_x = mix(dist_base, dist_alt, axis_mix.x);
    let dist_mix_y = mix(dist_base, dist_alt, axis_mix.y);
    let axis_sign = select(-1.0, 1.0, abs_norm_pos.y > abs_norm_pos.x);
    let final_mix = mix(dist_mix_x, dist_mix_y, clamp(0.5 - axis_sign + axis_sign * axis_ratio, 0.0, 1.0));
    let radial_pos = vec2f(blended_radius, blended_radius) + offset;
    return min(max(radial_pos.x, radial_pos.y), 0.0) + safe_scaled_radius * (final_mix - 1.0);
}

fn glass_shape_sdf(p: vec2f, b: vec2f, r: f32, edge: f32, edge_pow: f32) -> f32 {
    let k1 = 0.6;
    let safe_b = max(b, vec2f(1e-6));
    let ratio = clamp((clamp(vec2f(r) / safe_b, vec2f(0.0), vec2f(1.0)) - vec2f(k1)) / vec2f(1.0 - k1), vec2f(0.0), vec2f(1.0));
    let safe_edge = max(edge, 1e-6);
    var d = glass_supercircle_sdf_height(abs(p), b, r, ratio);
    if (d < -safe_edge) {
        d = -safe_edge;
    } else if (d < 0.0) {
        let per = -d / safe_edge;
        d = -glass_curve(per, edge_pow) * safe_edge;
    }
    return d;
}

fn glass_calculate_normal(pos_from_center: vec2f, half_size_px: vec2f, radius_px: f32, edge: f32, edge_pow: f32) -> vec3f {
    let eps = 1.0;
    let right_sdf = glass_shape_sdf(pos_from_center + vec2f(eps, 0.0), half_size_px, radius_px, edge, edge_pow);
    let left_sdf = glass_shape_sdf(pos_from_center - vec2f(eps, 0.0), half_size_px, radius_px, edge, edge_pow);
    let top_sdf = glass_shape_sdf(pos_from_center + vec2f(0.0, eps), half_size_px, radius_px, edge, edge_pow);
    let bottom_sdf = glass_shape_sdf(pos_from_center - vec2f(0.0, eps), half_size_px, radius_px, edge, edge_pow);
    let xy_gradient = vec2f((right_sdf - left_sdf) * 0.5, (top_sdf - bottom_sdf) * 0.5);
    return normalize(vec3f(xy_gradient, 1.0));
}

fn glass_get_lighten(fg_tex: texture_2d<f32>, fg_samp: sampler, uv: vec2f) -> f32 {
    let fg_col = textureSample(fg_tex, fg_samp, uv);
    return fg_col.r * fg_col.r;
}

fn glass_hsvv(col: vec3f, lighten: f32) -> vec3f {
    let raw_v = glass_luma(col);
    let v = min(raw_v, 1.0);
    let w = smoothstep(0.0, 0.5, v);
    let k = mix(1.0 - v, v, w);
    let g_base = 1.0 + smoothstep(0.0, 1.0, lighten) * mix(0.75, 0.4, w);
    let g = mix(g_base, 1.0, smoothstep(1.0, 3.0, raw_v));
    return (col + vec3f(k)) * g - vec3f(k);
}

fn glass_blend_reflect_light(dst: vec4f, lighten: f32, blend_mode: i32) -> vec4f {
    let src = vec4f(vec3f(lighten), lighten);
    switch (blend_mode) {
        case 1: {
            return glass_blend_normal(src, dst);
        }
        case 2: {
            return glass_blend_darken(src, dst);
        }
        case 3: {
            return glass_blend_multiply(src, dst);
        }
        case 4: {
            return glass_blend_plus_darker(src, dst);
        }
        case 5: {
            return glass_blend_color_burn(src, dst);
        }
        case 6: {
            return glass_blend_lighten(src, dst);
        }
        case 7: {
            return glass_blend_screen(src, dst);
        }
        case 8: {
            return glass_blend_plus_lighter(src, dst);
        }
        case 9: {
            return glass_blend_color_dodge(src, dst);
        }
        case 10: {
            return glass_blend_overlay(src, dst);
        }
        case 11: {
            return glass_blend_soft_light(src, dst);
        }
        case 12: {
            return glass_blend_hard_light(src, dst);
        }
        case 13: {
            return glass_blend_difference(src, dst);
        }
        case 14: {
            return glass_blend_exclusion(src, dst);
        }
        case 15: {
            return glass_blend_hue(src, dst);
        }
        case 16: {
            return glass_blend_saturation(src, dst);
        }
        case 17: {
            return glass_blend_color(src, dst);
        }
        case 18: {
            return glass_blend_luminance(src, dst);
        }
        default: {
            return vec4f(glass_hsvv(dst.rgb, lighten), dst.a);
        }
    }
}

fn glass_dynamic_add(color: vec3f) -> f32 {
    // var white_dis = distance(vec3f(1.0), color);
    // white_dis = smoothstep(0.2, 1.0, white_dis);
    // white_dis = mix(0.5, 1.0, white_dis);
    // let lumin = glass_luma(color);
    // return mix(0.5, white_dis, lumin);
    return 1.0;
}

fn glass_add_light(color: vec3f, light_color: vec3f, light_strength: f32) -> vec3f {
    // var white_dis = distance(vec3f(1.0), color);
    // white_dis = smoothstep(0.2, 1.0, white_dis);
    // white_dis = mix(0.3, 1.0, white_dis);
    // return color + light_color * (light_strength * white_dis);
    return color + light_color * light_strength;
}

fn glass_calculate_lighting(normal: vec3f, light_dir: vec3f, intensity: f32, angle_range: f32) -> f32 {
    let normalized_light_dir = normalize(light_dir);
    let dot_product = dot(normal, normalized_light_dir);
    let reflection_angle = acos(clamp(dot_product, -1.0, 1.0));
    let angle_factor = 1.0 - (reflection_angle / (3.14159 * angle_range));
    let adjusted_intensity = max(intensity * angle_factor, 0.0);
    return max(dot_product, 0.0) * adjusted_intensity;
}

fn glass_sample_screen_uv(screen_px: vec2f, resolution: vec2f) -> vec2f {
    return vec2f(screen_px.x, resolution.y - screen_px.y) / resolution;
}

fn glass_texture_map(
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
        let lighten = glass_get_lighten(fg_tex, fg_samp, frag_uv);
        col = glass_blend_reflect_light(col, lighten * reflect_lighten_opacity, reflect_lighten_blend_mode);
    }

    return col;
}


 @vertex
 fn vs_main(
     @location(0) position: vec3f,
     @location(1) uv: vec2f,
     @location(2) i0: vec4f,
     @location(3) i1: vec4f,
     @location(4) i2: vec4f,
     @location(5) i3: vec4f,
     @builtin(instance_index) instance_index: u32,
 ) -> VSOut {
 var out: VSOut;

 out.instance_index = instance_index;

 let _unused_geo_size = params.geo_size;
 let _unused_geo_translate = params.geo_translate;
 let _unused_geo_scale = params.geo_scale;

 // UV passed as vertex attribute.
 out.uv = uv;

 let inst_m = mat4x4f(i0, i1, i2, i3);
 let geo_sx = length(inst_m[0].xy);
 let geo_sy = length(inst_m[1].xy);
 let geo_size_px = params.geo_size * vec2f(geo_sx, geo_sy);
 out.geo_size_px = geo_size_px;
 out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * geo_size_px, 0.0);

 var p_local = (inst_m * vec4f(position, 1.0)).xyz;

 let delta_t = (vec3f(baked_data_parse[(instance_index) * 5u + 0u].xy, 0.0) + (graph_inputs.node_Vector3Input_67_698d8c66).xyz);
 p_local = p_local + delta_t;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = params.center + p_local.xy;

 out.position = params.camera * vec4f(p_px, p_local.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }