
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
      @location(6) world_pos: vec3f,
 };


struct GraphInputs {
    // Node: Vector4Input_063772fb_9
    vector4_input_063772fb_9: vec4f,
    // Node: gauge.clock.seconds
    gauge_clock_seconds: vec4f,
    // Node: gauge.tick.count
    gauge_tick_count: vec4i,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_gauge_label50_sdf: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_gauge_label50_sdf: sampler;

@group(1) @binding(2)
var pass_tex_gauge_glass_background: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_gauge_glass_background: sampler;


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

    if (color.a <= 0.0001) {
        return color;
    }

    let rgb = color.rgb / color.a;
    let luminance = clamp(dot(rgb, vec3f(0.2125, 0.7153, 0.0721)), 0.0, 1.0);
    var target_luminance = luminance * factor_adjust.x + factor_adjust.y;
    target_luminance = target_luminance * luminance + factor_adjust.z;
    target_luminance = target_luminance * luminance + factor_adjust.w;

    let chroma = rgb - vec3f(luminance);
    let chroma_scale = clamp(target_luminance / max(luminance, 1e-6), 0.0, 1.0);
    let remapped_rgb = vec3f(target_luminance) + chroma * chroma_scale;
    let mixed = max(vec3f(0.0), mix(rgb, remapped_rgb, mix_factor));

    return vec4f(mixed * color.a, color.a);
}

fn glass_luminance_curve_lab(color: vec4f, factors: vec4f, mix_factor: f32) -> vec4f {
    let factor_adjust = vec4f(
        -1.0 * factors.x + 3.0 * factors.y - 3.0 * factors.z + 1.0 * factors.w,
        3.0 * factors.x - 6.0 * factors.y + 3.0 * factors.z,
        -3.0 * factors.x + 3.0 * factors.y,
        factors.x,
    );

    if (color.a <= 0.0001) {
        return color;
    }

    let rgb = (mat3x3f(vec3f(1.2249402, -0.042056955, -0.019637555), vec3f(-0.22494018, 1.042057, -0.07863604), vec3f(0.0, 0.0, 1.0982736)) * color.rgb / color.a);
    let lms = vec3f(
        0.4122214708 * rgb.r + 0.5363325363 * rgb.g + 0.0514459929 * rgb.b,
        0.2119034982 * rgb.r + 0.6806995451 * rgb.g + 0.1073969566 * rgb.b,
        0.0883024619 * rgb.r + 0.2817188376 * rgb.g + 0.6299787005 * rgb.b,
    );
    let lms_cbrt = sign(lms) * pow(abs(lms), vec3f(1.0 / 3.0));
    let lab = vec3f(
        0.2104542553 * lms_cbrt.x + 0.7936177850 * lms_cbrt.y - 0.0040720468 * lms_cbrt.z,
        1.9779984951 * lms_cbrt.x - 2.4285922050 * lms_cbrt.y + 0.4505937099 * lms_cbrt.z,
        0.0259040371 * lms_cbrt.x + 0.7827717662 * lms_cbrt.y - 0.8086757660 * lms_cbrt.z,
    );

    let curve_input = clamp(lab.x, 0.0, 1.0);
    var target_l = curve_input * factor_adjust.x + factor_adjust.y;
    target_l = target_l * curve_input + factor_adjust.z;
    target_l = target_l * curve_input + factor_adjust.w;
    // if (lab.x > 1.0) {
    //     target_l = lab.x + factors.w - 1.0;
    // } else if (lab.x < 0.0) {
    //     target_l = lab.x + factors.x;
    // }

    let mapped_l = mix(lab.x, target_l, mix_factor);
    let mapped_lms_cbrt = vec3f(
        mapped_l + 0.3963377774 * lab.y + 0.2158037573 * lab.z,
        mapped_l - 0.1055613458 * lab.y - 0.0638541728 * lab.z,
        mapped_l - 0.0894841775 * lab.y - 1.2914855480 * lab.z,
    );
    let mapped_lms = mapped_lms_cbrt * mapped_lms_cbrt * mapped_lms_cbrt;
    var mapped_rgb = vec3f(
        4.0767416621 * mapped_lms.x - 3.3077115913 * mapped_lms.y + 0.2309699292 * mapped_lms.z,
        -1.2684380046 * mapped_lms.x + 2.6097574011 * mapped_lms.y - 0.3413193965 * mapped_lms.z,
        -0.0041960863 * mapped_lms.x - 0.7034186147 * mapped_lms.y + 1.7076147010 * mapped_lms.z,
    );
    mapped_rgb = max(vec3f(0.0), (mat3x3f(vec3f(0.82246196, 0.0331942, 0.01708263), vec3f(0.17753804, 0.9668058, 0.07239744), vec3f(0.0, 0.0, 0.9105199)) * mapped_rgb));

    return vec4f(mapped_rgb * color.a, color.a);
}

fn glass_adjust_color(color: vec4f, saturation: f32, brightness: f32) -> vec4f {
    let luminance = dot(color.rgb, vec3f(0.2125, 0.7153, 0.0721));
    let adjusted_saturation = saturation * color.rgb + (1.0 - saturation) * vec3f(luminance);
    let alpha = color.a;
    let adjusted_brightness = adjusted_saturation + vec3f(brightness * alpha);
    return vec4f(adjusted_brightness, alpha);
}

fn glass_smooth5_map(t_in: f32) -> f32 {
    // Symmetry avoids catastrophic cancellation in finite-difference normals.
    let t = clamp(0.5 + t_in * 0.5, 0.0, 1.0);
    let q = min(t, 1.0 - t);
    let polynomial = q * q * q * (10.0 + q * (-15.0 + 6.0 * q));
    return select(2.0 * polynomial - 1.0, 1.0 - 2.0 * polynomial, t >= 0.5);
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

fn glass_shape_sdf(p: vec2f, b: vec2f, r: f32, edge: f32, edge_pow: f32) -> f32 {
    let k1 = 0.6;
    let safe_b = max(b, vec2f(1e-6));
    let ratio = clamp((clamp(vec2f(r) / safe_b, vec2f(0.0), vec2f(1.0)) - vec2f(k1)) / vec2f(1.0 - k1), vec2f(0.0), vec2f(1.0));
    let safe_edge = max(edge, 1e-6);
    var d = sdf2d_smooth_round_rect(abs(p), b, r, ratio).x;
    if (d < -safe_edge) {
        d = -safe_edge;
    } else if (d < 0.0) {
        let per = -d / safe_edge;
        d = -glass_curve(per, edge_pow) * safe_edge;
    }
    return d;
}

fn glass_calculate_normal(pos_from_center: vec2f, half_size_px: vec2f, radius_px: f32, edge: f32, edge_pow: f32) -> vec3f {
    let eps = 0.002;
    let right_sdf = glass_shape_sdf(pos_from_center + vec2f(eps, 0.0), half_size_px, radius_px, edge, edge_pow);
    let left_sdf = glass_shape_sdf(pos_from_center - vec2f(eps, 0.0), half_size_px, radius_px, edge, edge_pow);
    let top_sdf = glass_shape_sdf(pos_from_center + vec2f(0.0, eps), half_size_px, radius_px, edge, edge_pow);
    let bottom_sdf = glass_shape_sdf(pos_from_center - vec2f(0.0, eps), half_size_px, radius_px, edge, edge_pow);
    let xy_gradient = vec2f((right_sdf - left_sdf) / (2.0 * eps), (top_sdf - bottom_sdf) / (2.0 * eps));
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

fn nf_math_closure_fragment_14f5dfc3_closure(uv: vec2<f32>, pointerColor: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var pointerColor_1: vec4<f32>;
    var output: vec4<f32>;

    uv_1 = uv;
    pointerColor_1 = pointerColor;
    let _e5: vec4<f32> = pointerColor_1;
    output = _e5;
    let _e6: vec4<f32> = output;
    return _e6;
}

fn nf_math_closure_fragment_647c5198_closure(uv: vec2<f32>, percent: f32, tickCount: i32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var percent_1: f32;
    var tickCount_1: i32;
    var output: vec4<f32>;
    var nonWhiteCount: f32;
    var redCount: f32;
    var redStart: f32;
    var headIndex: f32;
    var inRed: bool;
    var local: vec4<f32>;

    uv_1 = uv;
    percent_1 = percent;
    tickCount_1 = tickCount;
    let _e7: i32 = tickCount_1;
    let _e12: i32 = tickCount_1;
    nonWhiteCount = max(f32((_e12 - 1i)), 0f);
    let _e21: i32 = tickCount_1;
    let _e25: i32 = tickCount_1;
    let _e31: i32 = tickCount_1;
    let _e35: i32 = tickCount_1;
    let _e41: f32 = nonWhiteCount;
    let _e43: i32 = tickCount_1;
    let _e47: i32 = tickCount_1;
    let _e53: i32 = tickCount_1;
    let _e57: i32 = tickCount_1;
    redCount = min(_e41, max(1f, ceil((f32(_e57) * 0.15f))));
    let _e65: f32 = nonWhiteCount;
    let _e66: f32 = redCount;
    redStart = (_e65 - _e66);
    let _e72: f32 = percent_1;
    let _e78: f32 = nonWhiteCount;
    let _e83: f32 = percent_1;
    let _e89: f32 = nonWhiteCount;
    headIndex = floor(((clamp(_e83, 0f, 100f) / 100f) * _e89));
    let _e93: f32 = headIndex;
    let _e94: f32 = redStart;
    inRed = (_e93 >= _e94);
    let _e97: bool = inRed;
    if _e97 {
        local = vec4<f32>(1f, 0f, 0f, 1f);
    } else {
        local = vec4<f32>(0f, 1f, 1f, 1f);
    }
    let _e109: vec4<f32> = local;
    output = _e109;
    let _e110: vec4<f32> = output;
    return _e110;
}

fn nf_math_closure_fragment_a36c17c0_closure(uv: vec2<f32>, phase: f32, localPx: vec2<f32>, layoutWidthPx: f32, cellWidthPx: f32, cellHeightPx: f32) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var phase_1: f32;
    var localPx_1: vec2<f32>;
    var layoutWidthPx_1: f32;
    var cellWidthPx_1: f32;
    var cellHeightPx_1: f32;
    var output: vec2<f32>;
    var value: f32;
    var local: f32;
    var local_1: f32;
    var digits: f32;
    var left: f32;
    var cell: f32;
    var place: f32;
    var digit: f32;

    uv_1 = uv;
    phase_1 = phase;
    localPx_1 = localPx;
    layoutWidthPx_1 = layoutWidthPx;
    cellWidthPx_1 = cellWidthPx;
    cellHeightPx_1 = cellHeightPx;
    let _e16: f32 = phase_1;
    let _e23: f32 = phase_1;
    value = floor(clamp(_e23, 0f, 100f));
    let _e29: f32 = value;
    if (_e29 >= 100f) {
        local_1 = 3f;
    } else {
        let _e33: f32 = value;
        if (_e33 >= 10f) {
            local = 2f;
        } else {
            local = 1f;
        }
        let _e39: f32 = local;
        local_1 = _e39;
    }
    let _e41: f32 = local_1;
    digits = _e41;
    let _e43: f32 = layoutWidthPx_1;
    let _e44: f32 = digits;
    let _e45: f32 = cellWidthPx_1;
    left = ((_e43 - (_e44 * _e45)) * 0.5f);
    let _e51: vec2<f32> = localPx_1;
    let _e53: f32 = left;
    let _e55: f32 = cellWidthPx_1;
    let _e57: vec2<f32> = localPx_1;
    let _e59: f32 = left;
    let _e61: f32 = cellWidthPx_1;
    let _e65: f32 = digits;
    let _e68: vec2<f32> = localPx_1;
    let _e70: f32 = left;
    let _e72: f32 = cellWidthPx_1;
    let _e74: vec2<f32> = localPx_1;
    let _e76: f32 = left;
    let _e78: f32 = cellWidthPx_1;
    let _e82: f32 = digits;
    cell = clamp(floor(((_e74.x - _e76) / _e78)), 0f, (_e82 - 1f));
    let _e88: f32 = digits;
    let _e89: f32 = cell;
    let _e94: f32 = digits;
    let _e95: f32 = cell;
    place = pow(10f, ((_e94 - _e95) - 1f));
    let _e101: f32 = value;
    let _e102: f32 = place;
    let _e104: f32 = value;
    let _e105: f32 = place;
    let _e109: f32 = value;
    let _e110: f32 = place;
    let _e112: f32 = value;
    let _e113: f32 = place;
    let _e115: f32 = floor((_e112 / _e113));
    digit = (_e115 - (floor((_e115 / 10f)) * 10f));
    let _e122: f32 = left;
    let _e123: f32 = cell;
    let _e124: f32 = cellWidthPx_1;
    let _e127: f32 = digit;
    let _e129: f32 = cellHeightPx_1;
    output = vec2<f32>((_e122 + (_e123 * _e124)), (-(_e127) * _e129));
    let _e132: vec2<f32> = output;
    return _e132;
}

fn nf_math_closure_fragment_closure(uv: vec2<f32>, accent: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var accent_1: vec4<f32>;
    var output: vec4<f32>;

    uv_1 = uv;
    accent_1 = accent;
    let _e5: vec4<f32> = accent_1;
    let _e8: vec3<f32> = (_e5.xyz * 0.55f);
    output = vec4<f32>(_e8.x, _e8.y, _e8.z, 1f);
    let _e14: vec4<f32> = output;
    return _e14;
}


// ---- 2D SDF bevel helpers (generated) ----
// 2D SDF bevel helper template.
//
// This file is the editable WGSL source for Sdf2DBevel curve helper functions.
// The Rust compiler wires node inputs into calls to these helpers.

fn sdf2d_bevel_smooth5_map(t_in: f32) -> f32 {
    // Evaluate the symmetric polynomial in the nearer endpoint's coordinate.
    // Expanded terms near t=1 lose precision before finite-difference normals.
    let t = clamp(0.5 + t_in * 0.5, 0.0, 1.0);
    let q = min(t, 1.0 - t);
    let polynomial = q * q * q * (10.0 + q * (-15.0 + 6.0 * q));
    return select(2.0 * polynomial - 1.0, 1.0 - 2.0 * polynomial, t >= 0.5);
}

fn sdf2d_bevel_smooth5(d_in: f32, edge: f32, cliff: f32) -> f32 {
    var d = d_in;
    if (d < -edge) {
        d = -edge;
    } else if (d < 0.0) {
        var x = -d / edge;
        if (x >= 0.85) {
            x = 1.0;
        } else {
            x = clamp(x, 0.0, 1.0);
            x = sdf2d_bevel_smooth5_map(pow(x, 0.5));
            x = 1.0 - pow(1.0 - x, cliff);
        }
        d = -x * edge;
    }
    return d;
}

fn sdf2d_bevel_smooth7_map(t_in: f32) -> f32 {
    let t = clamp(0.5 + t_in * 0.5, 0.0, 1.0);
    let q = min(t, 1.0 - t);
    let q2 = q * q;
    let polynomial = q2 * q2 * (35.0 + q * (-84.0 + q * (70.0 - 20.0 * q)));
    return select(2.0 * polynomial - 1.0, 1.0 - 2.0 * polynomial, t >= 0.5);
}

fn sdf2d_bevel_smooth7(d_in: f32, edge: f32, cliff: f32) -> f32 {
    var d = d_in;
    if (d < -edge) {
        d = -edge;
    } else if (d < 0.0) {
        var x = -d / edge;
        if (x >= 0.85) {
            x = 1.0;
        } else {
            x = clamp(x, 0.0, 1.0);
            x = sdf2d_bevel_smooth7_map(pow(x, 0.5));
            x = 1.0 - pow(1.0 - x, cliff);
        }
        d = -x * edge;
    }
    return d;
}

fn sdf2d_bevel_eps() -> f32 {
    return 0.002;
}

fn sdf2d_bevel_normal(depth_px: f32, depth_nx: f32, depth_py: f32, depth_ny: f32, eps: f32) -> vec3f {
    let safe_eps = max(abs(eps), 1e-6);
    let dx = (depth_px - depth_nx) / (2.0 * safe_eps);
    let dy = (depth_py - depth_ny) / (2.0 * safe_eps);
    return normalize(vec3f(-dx, -dy, 1.0));
}

// Note: normal reconstruction uses 4 extra evaluations (finite differences).
// Potential optimization: use `dpdx`/`dpdy` in WGSL to estimate derivatives with fewer calls.


// ---- 2D SDF helpers (generated) ----
// 2D SDF helper template.
//
// This file is the editable WGSL source for Sdf2D shape helper functions.
// The Rust compiler wires node inputs into calls to these helpers.

fn sdf2d_round_rect(p: vec2f, b: vec2f, rad4: vec4f) -> f32 {
    var r: f32 = rad4.x;
    if (p.x > 0.0 && p.y > 0.0) {
        r = rad4.y;
    } else if (p.x > 0.0 && p.y < 0.0) {
        r = rad4.z;
    } else if (p.x < 0.0 && p.y < 0.0) {
        r = rad4.w;
    }

    let q = abs(p) - b + vec2f(r, r);
    let outside = length(max(q, vec2f(0.0, 0.0)));
    let inside = min(max(q.x, q.y), 0.0);
    return outside + inside - r;
}

fn sdf2d_smooth_round_rect(point: vec2f, center: vec2f, radius: f32, axis_mix: vec2f) -> vec3f {
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

    let poly_fit_0 = -0.7391197269 * axis_ratio + 2.4034927648;
    let poly_fit_1 = poly_fit_0 * axis_ratio - 2.4907319173;
    let poly_fit_2 = poly_fit_1 * axis_ratio + 0.4768708960;
    let poly_fit = poly_fit_2 * axis_ratio + 0.4747847594;
    let len_abs = length(abs_norm_pos);
    let denom = 1.0 - axis_ratio * axis_ratio * clamp(len_abs, 0.0, 1.0) * poly_fit;
    let safe_denom = select(denom, 1e-6, abs(denom) < 1e-6);
    let dist_base = (len_abs + 1.0) - 1.0 / safe_denom;
    let dist_alt_pos = max(
        vec2f(0.0),
        1.5286649465560913 * abs_norm_pos - vec2f(0.5286650061607361),
    );
    let dist_alt = 0.6541655659675598 * length(dist_alt_pos) + 0.3458344340324402;

    let dist_mix_x = mix(dist_base, dist_alt, axis_mix.x);
    let dist_mix_y = mix(dist_base, dist_alt, axis_mix.y);
    let axis_sign = select(-1.0, 1.0, abs_norm_pos.y > abs_norm_pos.x);
    let final_mix = mix(dist_mix_x, dist_mix_y, clamp(0.5 - axis_sign + axis_sign * axis_ratio, 0.0, 1.0));

    let radial_pos = vec2f(blended_radius, blended_radius) + offset;
    let dir_norm = normalize(max(vec2f(0.0), radial_pos));
    let fallback_axis = select(vec2f(0.0, 1.0), vec2f(1.0, 0.0), radial_pos.x > radial_pos.y);
    let fallback_dir = select(fallback_axis, dir_norm, dir_norm.x + dir_norm.y > 0.0);
    let final_height = min(max(radial_pos.x, radial_pos.y), 0.0) + safe_scaled_radius * (final_mix - 1.0);

    return vec3f(final_height, fallback_dir);
}

// Private atlas rows are top-down. Public positions and distances remain LocalPx/Y-up.
// Header + instance records are separate from the four distance channels in each tile.
fn nf_text_record(atlas: texture_2d<f32>, index: u32) -> vec4f {
    let width = textureDimensions(atlas).x;
    return textureLoad(atlas, vec2i(i32(index % width), i32(index / width)), 0);
}

fn nf_text_distances(atlas: texture_2d<f32>, local_px: vec2f) -> vec2f {
    let header = nf_text_record(atlas, 0u);
    let position = clamp(local_px, vec2f(0.0), header.yz);
    var distances = vec2f(1e20);
    for (var i = 0u; i < u32(header.x); i += 1u) {
        let plane = nf_text_record(atlas, 1u + 2u * i);
        let outside_box = length(max(max(plane.xy - position, position - plane.zw), vec2f(0.0)));
        if (outside_box > 0.0 && outside_box > max(max(distances.x, distances.y), 0.0)) {
            continue;
        }
        let tile = nf_text_record(atlas, 2u + 2u * i);
        let scale = (plane.z - plane.x) / tile.z;
        let raster = vec2f(position.x - plane.x, plane.w - position.y) / scale - 0.5;
        let bounded = clamp(raster, vec2f(0.0), tile.zw - 1.0);
        let lo = vec2i(floor(bounded));
        let hi = min(lo + 1, vec2i(tile.zw) - 1);
        let origin = vec2i(tile.xy);
        let t = fract(bounded);
        let a = mix(textureLoad(atlas, origin + lo, 0), textureLoad(atlas, origin + vec2i(hi.x, lo.y), 0), t.x);
        let b = mix(textureLoad(atlas, origin + vec2i(lo.x, hi.y), 0), textureLoad(atlas, origin + hi, 0), t.x);
        let value = mix(a, b, t.y) * scale;
        let median = max(min(value.r, value.g), min(max(value.r, value.g), value.b));
        var glyph = vec2f(value.a, median);
        let outside = length(raster - bounded) * scale;
        if (outside > 0.0) {
            glyph = max(glyph, vec2f(0.0)) + outside;
        }
        distances = min(distances, glyph);
    }
    let outside_layout = length(local_px - position);
    if (outside_layout > 0.0) {
        distances = max(distances, vec2f(0.0)) + outside_layout;
    }
    // The atlas includes complete outlines; intersect with the authored layout frame.
    // This also gives a continuous distance at a frame edge that cuts through ink.
    let box_delta = abs(local_px - header.yz * 0.5) - header.yz * 0.5;
    let clip_distance = length(max(box_delta, vec2f(0.0))) + min(max(box_delta.x, box_delta.y), 0.0);
    return max(distances, vec2f(clip_distance));
}

fn nf_text_sdf(atlas: texture_2d<f32>, local_px: vec2f) -> f32 {
    return nf_text_distances(atlas, local_px).x;
}

fn nf_text_msdf(atlas: texture_2d<f32>, local_px: vec2f) -> f32 {
    return nf_text_distances(atlas, local_px).y;
}

fn nf_sdf_coverage(distance: f32) -> f32 {
    let aa = max(fwidth(distance), 0.0001);
    return clamp(0.5 - distance / aa, 0.0, 1.0);
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

 out.geo_size_px = params.geo_size;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(uv * out.geo_size_px, 0.0);

 var p_local = position;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = params.center + p_local.xy;

 out.position = params.camera * vec4f(p_px, p_local.z, 1.0);

 out.world_pos = vec3f(p_px, p_local.z);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    let math_closure_out = nf_math_closure_fragment_647c5198_closure(vec2f(in.uv.x, 1.0 - in.uv.y), (graph_inputs.gauge_clock_seconds).x, i32(f32((graph_inputs.gauge_tick_count).x)));
    let math_closure_out_5dae3c33 = nf_math_closure_fragment_14f5dfc3_closure(vec2f(in.uv.x, 1.0 - in.uv.y), math_closure_out);
    let math_closure_out_56795a5e = nf_math_closure_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y), math_closure_out_5dae3c33);
    let math_closure_out_b06a9c64 = nf_math_closure_fragment_a36c17c0_closure(vec2f(in.uv.x, 1.0 - in.uv.y), (graph_inputs.gauge_clock_seconds).x, in.local_px.xy, 450.0, 150.0, 225.0);
    // Sdf2DBevel gauge.label50.bevel.normal finite differences
    let _2d_sdf_bevel_normal_sdf_px = nf_text_sdf(
        img_tex_gauge_label50_sdf,
        (in.local_px.xy + vec2f(max(abs(1.5), 0.001), 0.0)) - (math_closure_out_b06a9c64),
    );
    let _2d_sdf_bevel_normal_sdf_nx = nf_text_sdf(
        img_tex_gauge_label50_sdf,
        (in.local_px.xy + vec2f(-(max(abs(1.5), 0.001)), 0.0)) - (math_closure_out_b06a9c64),
    );
    let _2d_sdf_bevel_normal_sdf_py = nf_text_sdf(
        img_tex_gauge_label50_sdf,
        (in.local_px.xy + vec2f(0.0, max(abs(1.5), 0.001))) - (math_closure_out_b06a9c64),
    );
    let _2d_sdf_bevel_normal_sdf_ny = nf_text_sdf(
        img_tex_gauge_label50_sdf,
        (in.local_px.xy + vec2f(0.0, -(max(abs(1.5), 0.001)))) - (math_closure_out_b06a9c64),
    );
    let _2d_sdf_bevel_normal_depth_px = sdf2d_bevel_smooth7(_2d_sdf_bevel_normal_sdf_px, 3.5, 0.45);
    let _2d_sdf_bevel_normal_depth_nx = sdf2d_bevel_smooth7(_2d_sdf_bevel_normal_sdf_nx, 3.5, 0.45);
    let _2d_sdf_bevel_normal_depth_py = sdf2d_bevel_smooth7(_2d_sdf_bevel_normal_sdf_py, 3.5, 0.45);
    let _2d_sdf_bevel_normal_depth_ny = sdf2d_bevel_smooth7(_2d_sdf_bevel_normal_sdf_ny, 3.5, 0.45);
    let _2d_sdf_bevel_normal_normal = sdf2d_bevel_normal(
        _2d_sdf_bevel_normal_depth_px,
        _2d_sdf_bevel_normal_depth_nx,
        _2d_sdf_bevel_normal_depth_py,
        _2d_sdf_bevel_normal_depth_ny,
        max(abs(1.5), 0.001),
    );
 // GlassMaterial(gauge.label50.material)
 var glass_material_material_out: vec4f;
 {
     let screen_px = in.frag_coord_gl;
     let local_px = in.local_px.xy;
     let size_px = in.geo_size_px;
     let safe_size_px = max(size_px, vec2f(1e-6));
     let half_size_px = size_px * 0.5;
     let pos_from_center = local_px - half_size_px;
     let geo_origin_px = screen_px - local_px;
     let screen_uv = glass_sample_screen_uv(screen_px, params.target_size);

     // --- Shape SDF ---
     let edge = f32(7.800000191);
     let edge_pow = f32(2.0);
     let radius_px = (vec3f(0.0, 0.0, 54.0)).z;
     let safe_edge = max(edge, 1e-6);

     let shape_distance = nf_text_sdf(img_tex_gauge_label50_sdf, in.local_px.xy - (math_closure_out_b06a9c64));
     let normalized_sdf = clamp(-shape_distance / safe_edge, 0.0, 1.0);
     let local_normal = _2d_sdf_bevel_normal_normal;
     // Transform the local bevel normal and offsets with this instance's frame.
     let qx = dpdx(in.world_pos); let qy = dpdy(in.world_pos);
     let sx = dpdx(local_px); let sy = dpdy(local_px);
     let det = sx.x * sy.y - sx.y * sy.x;
     let safe_det = select(-max(abs(det), 1e-8), max(abs(det), 1e-8), det >= 0.0);
     let glass_tangent = (qx * sy.y - qy * sx.y) / safe_det;
     let glass_bitangent = (qy * sx.x - qx * sy.x) / safe_det;
     let geometric = normalize(cross(glass_tangent, glass_bitangent));
     let normal = normalize(cross(glass_bitangent, geometric) * local_normal.x * -1.0
         + cross(geometric, glass_tangent) * local_normal.y * -1.0
         + cross(glass_tangent, glass_bitangent) * local_normal.z);
     let light_normal = normal;
     var final_alpha = clamp(0.5 - shape_distance / max(fwidth(shape_distance), 0.0001), 0.0, 1.0);




     // --- Refraction ---
     let uv_display_px = (local_px - half_size_px) * f32(1.0) + half_size_px;
     let incident_ray = normalize(vec3f(0.0, 0.0, -1.0));
     let refractive_index = f32(1.5);
     let refract_dir = refract(incident_ray, normal, 1.0 / max(refractive_index, 1e-6));
     let refract_thickness = mix((f32(8.0) - edge) * 2.0, f32(8.0) * 2.0, clamp(normalized_sdf, 0.0, 1.0));
     let refract_local_px = uv_display_px + refract_dir.xy * refract_thickness;
     let refract_uv = glass_sample_screen_uv(screen_px + glass_tangent.xy * (uv_display_px.x - local_px.x) + glass_bitangent.xy * (uv_display_px.y - local_px.y) + refract_dir.xy * refract_thickness, params.target_size);
     let refraction = textureSample(pass_tex_gauge_glass_background, pass_samp_gauge_glass_background, refract_uv);

     // --- Reflection ---
     let reflect_dir = reflect(incident_ray, normal);
     let reflect_local_px = uv_display_px + reflect_dir.xy * mix(0.0, f32(0.0), 1.0 - clamp(normalized_sdf, 0.0, 1.0));
     let reflect_uv = glass_sample_screen_uv(screen_px + glass_tangent.xy * (uv_display_px.x - local_px.x) + glass_bitangent.xy * (uv_display_px.y - local_px.y) + reflect_dir.xy * mix(0.0, f32(0.0), 1.0 - clamp(normalized_sdf, 0.0, 1.0)), params.target_size);
     let reflection = textureSample(pass_tex_gauge_glass_background, pass_samp_gauge_glass_background, reflect_uv);

     // --- Mix refraction + reflection ---
     var glass_mat = mix(refraction, reflection, clamp(1.0 + dot(normal, incident_ray) * 0.150000006, 0.0, 1.0));

     // --- Background color tinting ---
     var glass_color = textureSample(pass_tex_gauge_glass_background, pass_samp_gauge_glass_background, screen_uv);
     glass_color = glass_adjust_color(glass_color, 1.0, 0.0);
     glass_color = vec4f(mix(glass_color.rgb, vec3f(1.0), 0.0), glass_color.a);
     let glass_color_luma = clamp(glass_luma(glass_color.rgb), 0.0, 1.0);

     // --- Bionic burn color ratio ---
     var color_ratio = 1.0;
     let burn_term = pow(glass_color_luma, 1.0) - 0.5;
     let burn_mix = 1.587 * burn_term * burn_term * burn_term + 0.5;
     color_ratio = mix(glass_color_luma, color_ratio, burn_mix);
     color_ratio = color_ratio * 0.8;
     var glass_color_ratio = mix(vec3f(1.0), glass_color.rgb, color_ratio);

     // --- Neutral vibrancy fix ---
     if (1.0 > 0.0) {
         let mean_glass_mat_color = (glass_mat.r + glass_mat.g + glass_mat.b) / 3.0;
         let mean_glass_color_ratio = (glass_color_ratio.r + glass_color_ratio.g + glass_color_ratio.b) / 3.0;
         let neutral_threshold_min = clamp(0.0, 0.0, 1.0);
         var neutral_threshold = clamp(0.400000006, 0.0, 1.0);
         neutral_threshold = max(neutral_threshold, neutral_threshold_min + 0.0001);
         let grayness = distance(glass_mat.rgb, vec3f(mean_glass_mat_color));
         glass_color_ratio = mix(vec3f(mean_glass_color_ratio) * 0.5 + glass_color_ratio * 0.5, glass_color_ratio, smoothstep(neutral_threshold_min, neutral_threshold, grayness));
     }

     // --- Apply inner glass color ---
     glass_color_ratio = mix(glass_color_ratio, math_closure_out_56795a5e.rgb, math_closure_out_56795a5e.a);

     // --- Apply color ratio + inner color ---
     glass_mat = vec4f(glass_mat.rgb * mix(vec3f(1.0), glass_color_ratio, 1.0), glass_mat.a);
     glass_mat = vec4f(mix(glass_mat.rgb, glass_color_ratio, 0.100000001 * color_ratio), glass_mat.a);
     // --- Combine all light sources and the authored curve before one OKLab remap ---
     let lighting1 = glass_calculate_lighting(light_normal, vec3f(-0.400000006, 0.850000024, 0.200000003), 2.0, 0.419999987);
     let lighting2 = glass_calculate_lighting(light_normal, vec3f(-0.400000006, 0.850000024, 0.200000003) * vec3f(-1.0, -1.0, 1.0), 1.0, 0.419999987);
     let bottom_lighting = pow(smoothstep(1.0, 0.0, 1.0 - in.uv.y), 2.0) * 0.02;
     let lighting = lighting1 + lighting2 + 0.0 + bottom_lighting;
     if (lighting != 0.0 || 1.0 != 0.0) {
         let identity_curve = vec4f(0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0);
         let curve = mix(identity_curve, (graph_inputs.vector4_input_063772fb_9), 1.0) + vec4f(0.2 * lighting);
         // Material RGB is premultiplied only at the end; illumination must not change alpha.
         let lit = glass_luminance_curve_lab(vec4f(glass_mat.rgb, 1.0), curve, 1.0);
         glass_mat = vec4f(lit.rgb, glass_mat.a);
     }

     // --- Final adjustments ---
     glass_mat = vec4f(pow(glass_mat.rgb, vec3f(1.0)), glass_mat.a);

     // --- Apply unshade factor ---
     glass_mat = vec4f(mix(glass_mat.rgb, math_closure_out_56795a5e.rgb, 0.0), glass_mat.a);

     // --- Apply overall alpha ---
     glass_mat = vec4f(glass_mat.rgb, glass_mat.a * final_alpha * 0.939999998);

     // --- Premultiply alpha ---
     glass_mat = vec4f(glass_mat.rgb * glass_mat.a, glass_mat.a);
     glass_material_material_out = glass_mat;
 }

    // Final composite
    let _frag_out = glass_material_material_out;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
