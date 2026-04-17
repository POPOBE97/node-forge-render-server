//! Glass material node.
//!
//! This node is a shader-like material expression that samples multiple upstream pass textures
//! (background / refraction / reflection / blur) and applies a glass shading model.
//!
//! Design notes:
//! - The render-server's material system is expression-based. `RenderPass.material` compiles to a
//!   `vec4f` expression. There is no generic full-GLSL shader node today.
//! - We therefore implement GlassMaterial as a node compiler that emits WGSL helpers + inline
//!   statements and returns a `vec4f` color expression.
//! - Pass textures are bound via `MaterialCompileContext::register_pass_texture()` and sampled
//!   using the generated `pass_tex_*` / `pass_samp_*` vars.

use anyhow::{Result, bail};
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use crate::dsl::{Node, SceneDSL, incoming_connection, parse_f32};
use crate::renderer::utils::{fmt_f32, sanitize_wgsl_ident};

fn wgsl_vec2_literal(v: [f32; 2]) -> String {
    format!("vec2f({}, {})", fmt_f32(v[0]), fmt_f32(v[1]))
}

fn wgsl_vec3_literal(v: [f32; 3]) -> String {
    format!(
        "vec3f({}, {}, {})",
        fmt_f32(v[0]),
        fmt_f32(v[1]),
        fmt_f32(v[2])
    )
}

fn wgsl_vec4_literal(v: [f32; 4]) -> String {
    format!(
        "vec4f({}, {}, {}, {})",
        fmt_f32(v[0]),
        fmt_f32(v[1]),
        fmt_f32(v[2]),
        fmt_f32(v[3])
    )
}

fn parse_inline_vec2(node: &Node, key: &str, default: [f32; 2]) -> [f32; 2] {
    match node.params.get(key) {
        Some(v) => {
            if let Some(arr) = v.as_array() {
                if arr.len() >= 2 {
                    let x = arr[0].as_f64().unwrap_or(default[0] as f64) as f32;
                    let y = arr[1].as_f64().unwrap_or(default[1] as f64) as f32;
                    return [x, y];
                }
            }
            if let Some(obj) = v.as_object() {
                let x = obj
                    .get("x")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(default[0] as f64) as f32;
                let y = obj
                    .get("y")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(default[1] as f64) as f32;
                return [x, y];
            }
            default
        }
        None => default,
    }
}

fn parse_inline_vec3(node: &Node, key: &str, default: [f32; 3]) -> [f32; 3] {
    match node.params.get(key) {
        Some(v) => {
            if let Some(arr) = v.as_array() {
                if arr.len() >= 3 {
                    let x = arr[0].as_f64().unwrap_or(default[0] as f64) as f32;
                    let y = arr[1].as_f64().unwrap_or(default[1] as f64) as f32;
                    let z = arr[2].as_f64().unwrap_or(default[2] as f64) as f32;
                    return [x, y, z];
                }
            }
            if let Some(obj) = v.as_object() {
                let x = obj
                    .get("x")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(default[0] as f64) as f32;
                let y = obj
                    .get("y")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(default[1] as f64) as f32;
                let z = obj
                    .get("z")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(default[2] as f64) as f32;
                return [x, y, z];
            }
            default
        }
        None => default,
    }
}

fn parse_inline_vec4(node: &Node, key: &str, default: [f32; 4]) -> [f32; 4] {
    match node.params.get(key) {
        Some(v) => {
            if let Some(arr) = v.as_array() {
                if arr.len() >= 4 {
                    let x = arr[0].as_f64().unwrap_or(default[0] as f64) as f32;
                    let y = arr[1].as_f64().unwrap_or(default[1] as f64) as f32;
                    let z = arr[2].as_f64().unwrap_or(default[2] as f64) as f32;
                    let w = arr[3].as_f64().unwrap_or(default[3] as f64) as f32;
                    return [x, y, z, w];
                }
            }
            default
        }
        None => default,
    }
}

fn parse_inline_bool(node: &Node, key: &str, default: bool) -> bool {
    match node.params.get(key) {
        Some(v) => {
            if let Some(b) = v.as_bool() {
                return b;
            }
            if let Some(n) = v.as_f64() {
                return n > 0.5;
            }
            default
        }
        None => default,
    }
}

fn parse_inline_i32(node: &Node, key: &str, default: i32) -> i32 {
    match node.params.get(key) {
        Some(v) => {
            if let Some(n) = v.as_i64() {
                return n as i32;
            }
            if let Some(n) = v.as_f64() {
                return n as i32;
            }
            default
        }
        None => default,
    }
}

fn input_f32_expr(node: &Node, key: &str, default: f32) -> TypedExpr {
    let v = parse_f32(&node.params, key).unwrap_or(default);
    TypedExpr::new(fmt_f32(v), ValueType::F32)
}

fn input_vec2_expr(node: &Node, key: &str, default: [f32; 2]) -> TypedExpr {
    let v = parse_inline_vec2(node, key, default);
    TypedExpr::new(wgsl_vec2_literal(v), ValueType::Vec2)
}

fn input_vec3_expr(node: &Node, key: &str, default: [f32; 3]) -> TypedExpr {
    let v = parse_inline_vec3(node, key, default);
    TypedExpr::new(wgsl_vec3_literal(v), ValueType::Vec3)
}

fn input_vec4_expr(node: &Node, key: &str, default: [f32; 4]) -> TypedExpr {
    let v = parse_inline_vec4(node, key, default);
    TypedExpr::new(wgsl_vec4_literal(v), ValueType::Vec4)
}

fn input_bool_expr(node: &Node, key: &str, default: bool) -> TypedExpr {
    let v = parse_inline_bool(node, key, default);
    TypedExpr::new(if v { "true" } else { "false" }, ValueType::Bool)
}

fn input_i32_expr(node: &Node, key: &str, default: i32) -> TypedExpr {
    let v = parse_inline_i32(node, key, default);
    TypedExpr::new(v.to_string(), ValueType::I32)
}

fn resolve_pass_binding(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    port_id: &str,
    ctx: &mut MaterialCompileContext,
) -> Result<Option<(String, String, String)>> {
    let Some(conn) = incoming_connection(scene, &node.id, port_id) else {
        return Ok(None);
    };

    let upstream_id = conn.from.node_id.clone();
    let upstream = nodes_by_id.get(&upstream_id).ok_or_else(|| {
        anyhow::anyhow!("GlassMaterial: upstream node not found for {port_id}: {upstream_id}")
    })?;

    // Only pass-producing nodes are valid.
    if !matches!(
        upstream.node_type.as_str(),
        "RenderPass"
            | "BloomNode"
            | "GuassianBlurPass"
            | "Downsample"
            | "Upsample"
            | "GradientBlur"
            | "Composite"
    ) {
        bail!(
            "GlassMaterial.{port_id} must be connected to a pass node, got {}",
            upstream.node_type
        );
    }

    ctx.register_pass_texture(&upstream_id);
    let tex_var = MaterialCompileContext::pass_tex_var_name(&upstream_id);
    let samp_var = MaterialCompileContext::pass_sampler_var_name(&upstream_id);
    Ok(Some((upstream_id, tex_var, samp_var)))
}

const GLASS_WGSL_LIB_KEY: &str = "glass_material_lib";

fn ensure_glass_wgsl_lib(ctx: &mut MaterialCompileContext) {
    if ctx.extra_wgsl_decls.contains_key(GLASS_WGSL_LIB_KEY) {
        return;
    }

    let wgsl = r#"
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
    var luminance = dot(scaled_rgb, vec3f(0.2125, 0.7153, 0.0721));
    luminance = clamp(luminance, 0.0, 1.0);

    var adj = luminance * factor_adjust.x + factor_adjust.y;
    adj = adj * luminance + factor_adjust.z;
    adj = adj * luminance + factor_adjust.w;
    adj = clamp(adj, 0.0, 1.0);

    let mixed = mix(scaled_rgb, vec3f(adj), mix_factor);
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
    let v = glass_luma(col);
    let w = smoothstep(0.0, 0.5, v);
    let k = mix(1.0 - v, v, w);
    let g = 1.0 + smoothstep(0.0, 1.0, lighten) * mix(0.75, 0.4, w);
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
    var white_dis = distance(vec3f(1.0), color);
    white_dis = smoothstep(0.2, 1.0, white_dis);
    white_dis = mix(0.5, 1.0, white_dis);
    let lumin = glass_luma(color);
    return mix(0.5, white_dis, lumin);
}

fn glass_add_light(color: vec3f, light_color: vec3f, light_strength: f32) -> vec3f {
    var white_dis = distance(vec3f(1.0), color);
    white_dis = smoothstep(0.2, 1.0, white_dis);
    white_dis = mix(0.3, 1.0, white_dis);
    return color + light_color * (light_strength * white_dis);
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
    is_bg: bool,
    add_foreground: bool,
    blend_darker: f32,
    blend_darker_range: vec2f,
    reflect_lighten_opacity: f32,
    reflect_lighten_blend_mode: i32,
    fg_tex: texture_2d<f32>,
    fg_samp: sampler,
) -> vec4f {
    var col = textureSample(tex, samp, sample_uv);

    if (is_bg) {
        let lum = glass_luma(col.rgb);
        let t = mix(0.0, blend_darker, smoothstep(blend_darker_range.x, blend_darker_range.y, lum));
        let darken_tint = 0.1 * (vec3f(1.0) - vec3f(0.2126, 0.7152, 0.0722));
        col = vec4f(mix(col.rgb, darken_tint, t), col.a);
    }

    if (add_foreground) {
        let lighten = glass_get_lighten(fg_tex, fg_samp, sample_uv);
        col = glass_blend_reflect_light(col, lighten * reflect_lighten_opacity, reflect_lighten_blend_mode);
    }

    return col;
}
"#;

    ctx.extra_wgsl_decls
        .insert(GLASS_WGSL_LIB_KEY.to_string(), wgsl.to_string());
}

pub fn compile_glass_material<F>(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: F,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    ensure_glass_wgsl_lib(ctx);

    let mut input_expr = |port_id: &str, fallback: TypedExpr| -> Result<TypedExpr> {
        if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
            Ok(compile_fn(
                &conn.from.node_id,
                Some(&conn.from.port_id),
                ctx,
                cache,
            )?)
        } else {
            Ok(fallback)
        }
    };

    let u_blend_brightness = input_expr(
        "uBlendBrightness",
        input_f32_expr(node, "uBlendBrightness", 0.0),
    )?;
    let u_blend_luminance_amount = input_expr(
        "uBlendLuminanceAmount",
        input_f32_expr(node, "uBlendLuminanceAmount", 0.0),
    )?;
    let u_blend_luminance_values = input_expr(
        "uBlendLuminanceValues",
        input_vec4_expr(node, "uBlendLuminanceValues", [0.0, 0.0, 0.0, 0.0]),
    )?;
    let u_blend_saturation = input_expr(
        "uBlendSaturation",
        input_f32_expr(node, "uBlendSaturation", 1.0),
    )?;
    let u_blend_darker = input_expr(
        "uBlendDarker",
        input_f32_expr(node, "uBlendDarker", 0.15),
    )?;
    let u_blend_darker_range = input_expr(
        "uBlendDarkerRange",
        input_vec2_expr(node, "uBlendDarkerRange", [0.6, 0.8]),
    )?;

    let u_shape_edge_px = input_expr(
        "uShapeEdgePx",
        input_f32_expr(node, "uShapeEdgePx", 20.0),
    )?;
    let u_shape_edge_pow = input_expr(
        "uShapeEdgePow",
        input_f32_expr(node, "uShapeEdgePow", 2.0),
    )?;
    let u_shape_thickness_px = input_expr(
        "uShapeThicknessPx",
        input_f32_expr(node, "uShapeThicknessPx", 200.0),
    )?;
    let u_shape_reflect_offset_px = input_expr(
        "uShapeReflectOffsetPx",
        input_f32_expr(node, "uShapeReflectOffsetPx", 400.0),
    )?;

    let u_refract_blur_scale = input_expr(
        "uRefractBlurScale",
        input_f32_expr(node, "uRefractBlurScale", 1.0),
    )?;
    let u_refract_ior = input_expr(
        "uRefractIorIOR",
        input_f32_expr(node, "uRefractIorIOR", 1.5),
    )?;

    let u_reflect_strength = input_expr(
        "uReflectStrength",
        input_f32_expr(node, "uReflectStrength", 1.0),
    )?;
    let u_reflect_lighten = input_expr(
        "uReflectLighten",
        input_f32_expr(node, "uReflectLighten", 0.2),
    )?;
    let u_reflect_lighten_opacity = input_expr(
        "uReflectLightenOpacity",
        input_f32_expr(node, "uReflectLightenOpacity", 1.0),
    )?;
    let u_reflect_lighten_blend_mode = input_expr(
        "uReflectLightenBlendMode",
        input_i32_expr(node, "uReflectLightenBlendMode", 0),
    )?;

    let u_bg_color_brightness = input_expr(
        "uBgColorBrightness",
        input_f32_expr(node, "uBgColorBrightness", 0.0),
    )?;
    let u_bg_color_saturation = input_expr(
        "uBgColorSaturation",
        input_f32_expr(node, "uBgColorSaturation", 4.0),
    )?;

    let u_inner_bottom = input_expr(
        "uInnerBottom",
        input_f32_expr(node, "uInnerBottom", 0.05),
    )?;
    let u_inner_color_white = input_expr(
        "uInnerColorWhite",
        input_f32_expr(node, "uInnerColorWhite", 0.1),
    )?;
    let u_inner_color_mix = input_expr(
        "uInnerColorMix",
        input_f32_expr(node, "uInnerColorMix", 0.7),
    )?;
    let u_inner_color_pow = input_expr(
        "uInnerColorPow",
        input_f32_expr(node, "uInnerColorPow", 1.2),
    )?;
    let u_inner_glass_color = input_expr(
        "uInnerGlassColor",
        input_vec4_expr(node, "uInnerGlassColor", [1.0, 1.0, 1.0, 0.15]),
    )?;

    let u_bionic_burn = input_expr(
        "uBionicBurn",
        input_f32_expr(node, "uBionicBurn", 11.0),
    )?;
    let u_bionic_unshade = input_expr(
        "uBionicUnShade",
        input_f32_expr(node, "uBionicUnShade", 0.0),
    )?;
    let u_debug_fix_neutral_vibrancy = input_expr(
        "uDebugFixNeutralVibrancy",
        input_f32_expr(node, "uDebugFixNeutralVibrancy", 1.0),
    )?;
    let u_debug_fix_neutral_vibrancy_threshold_min = input_expr(
        "uDebugFixNeutralVibrancyThresholdMin",
        input_f32_expr(node, "uDebugFixNeutralVibrancyThresholdMin", 0.0),
    )?;
    let u_debug_fix_neutral_vibrancy_threshold = input_expr(
        "uDebugFixNeutralVibrancyThreshold",
        input_f32_expr(node, "uDebugFixNeutralVibrancyThreshold", 0.4),
    )?;

    let u_light_dir = input_expr(
        "uDirectionalLightDirection",
        input_vec3_expr(node, "uDirectionalLightDirection", [0.0, 1.0, 0.0]),
    )?;
    let u_light_intensity = input_expr(
        "uDirectionalLightIntensity",
        input_f32_expr(node, "uDirectionalLightIntensity", 0.8),
    )?;
    let u_light_opp_intensity = input_expr(
        "uDirectionalLightOppositeIntensity",
        input_f32_expr(node, "uDirectionalLightOppositeIntensity", 0.4),
    )?;
    let u_light_angle_range = input_expr(
        "uDirectionalLightAngleRange",
        input_f32_expr(node, "uDirectionalLightAngleRange", 0.25),
    )?;

    let u_alpha = input_expr("uAlpha", input_f32_expr(node, "uAlpha", 1.0))?;
    let u_geo_px_size = input_expr(
        "uGeoPxSize",
        input_vec3_expr(node, "uGeoPxSize", [0.0, 0.0, 54.0]),
    )?;
    let u_use_sdf_tex = input_expr(
        "uUseSdfTex",
        input_bool_expr(node, "uUseSdfTex", false),
    )?;

    let u_sdf_tex = if let Some(conn) = incoming_connection(scene, &node.id, "uSdfTex") {
        Some(compile_fn(
            &conn.from.node_id,
            Some(&conn.from.port_id),
            ctx,
            cache,
        )?)
    } else {
        None
    };

    let bg = resolve_pass_binding(scene, nodes_by_id, node, "uBgTex", ctx)?;
    let bg_color = resolve_pass_binding(scene, nodes_by_id, node, "uBgColorTex", ctx)?;
    let refract = resolve_pass_binding(scene, nodes_by_id, node, "uRefractTex", ctx)?;
    let reflect = resolve_pass_binding(scene, nodes_by_id, node, "uReflectTex", ctx)?;
    let fg_blur =
        resolve_pass_binding(scene, nodes_by_id, node, "uReflectForegroundBlurTex", ctx)?;

    let uses_time = [
        u_blend_brightness.uses_time,
        u_blend_luminance_amount.uses_time,
        u_blend_luminance_values.uses_time,
        u_blend_saturation.uses_time,
        u_blend_darker.uses_time,
        u_blend_darker_range.uses_time,
        u_shape_edge_px.uses_time,
        u_shape_edge_pow.uses_time,
        u_shape_thickness_px.uses_time,
        u_shape_reflect_offset_px.uses_time,
        u_refract_blur_scale.uses_time,
        u_refract_ior.uses_time,
        u_reflect_strength.uses_time,
        u_reflect_lighten.uses_time,
        u_reflect_lighten_opacity.uses_time,
        u_reflect_lighten_blend_mode.uses_time,
        u_bg_color_brightness.uses_time,
        u_bg_color_saturation.uses_time,
        u_inner_bottom.uses_time,
        u_inner_color_white.uses_time,
        u_inner_color_mix.uses_time,
        u_inner_color_pow.uses_time,
        u_inner_glass_color.uses_time,
        u_bionic_burn.uses_time,
        u_bionic_unshade.uses_time,
        u_debug_fix_neutral_vibrancy.uses_time,
        u_debug_fix_neutral_vibrancy_threshold_min.uses_time,
        u_debug_fix_neutral_vibrancy_threshold.uses_time,
        u_light_dir.uses_time,
        u_light_intensity.uses_time,
        u_light_opp_intensity.uses_time,
        u_light_angle_range.uses_time,
        u_alpha.uses_time,
        u_geo_px_size.uses_time,
        u_use_sdf_tex.uses_time,
        u_sdf_tex.as_ref().map(|expr| expr.uses_time).unwrap_or(false),
    ]
    .into_iter()
    .any(|value| value);

    fn expect_ty(expr: &TypedExpr, expected: ValueType, name: &str) -> Result<()> {
        if expr.ty != expected {
            bail!(
                "GlassMaterial.{name} expected {:?}, got {:?}",
                expected,
                expr.ty
            );
        }
        Ok(())
    }

    expect_ty(&u_blend_brightness, ValueType::F32, "uBlendBrightness")?;
    expect_ty(
        &u_blend_luminance_amount,
        ValueType::F32,
        "uBlendLuminanceAmount",
    )?;
    expect_ty(
        &u_blend_luminance_values,
        ValueType::Vec4,
        "uBlendLuminanceValues",
    )?;
    expect_ty(&u_blend_saturation, ValueType::F32, "uBlendSaturation")?;
    expect_ty(&u_blend_darker, ValueType::F32, "uBlendDarker")?;
    expect_ty(&u_blend_darker_range, ValueType::Vec2, "uBlendDarkerRange")?;
    expect_ty(&u_shape_edge_px, ValueType::F32, "uShapeEdgePx")?;
    expect_ty(&u_shape_edge_pow, ValueType::F32, "uShapeEdgePow")?;
    expect_ty(&u_shape_thickness_px, ValueType::F32, "uShapeThicknessPx")?;
    expect_ty(
        &u_shape_reflect_offset_px,
        ValueType::F32,
        "uShapeReflectOffsetPx",
    )?;
    expect_ty(&u_refract_blur_scale, ValueType::F32, "uRefractBlurScale")?;
    expect_ty(&u_refract_ior, ValueType::F32, "uRefractIorIOR")?;
    expect_ty(&u_reflect_strength, ValueType::F32, "uReflectStrength")?;
    expect_ty(&u_reflect_lighten, ValueType::F32, "uReflectLighten")?;
    expect_ty(
        &u_reflect_lighten_opacity,
        ValueType::F32,
        "uReflectLightenOpacity",
    )?;
    expect_ty(
        &u_reflect_lighten_blend_mode,
        ValueType::I32,
        "uReflectLightenBlendMode",
    )?;
    expect_ty(&u_bg_color_brightness, ValueType::F32, "uBgColorBrightness")?;
    expect_ty(&u_bg_color_saturation, ValueType::F32, "uBgColorSaturation")?;
    expect_ty(&u_inner_bottom, ValueType::F32, "uInnerBottom")?;
    expect_ty(&u_inner_color_white, ValueType::F32, "uInnerColorWhite")?;
    expect_ty(&u_inner_color_mix, ValueType::F32, "uInnerColorMix")?;
    expect_ty(&u_inner_color_pow, ValueType::F32, "uInnerColorPow")?;
    expect_ty(&u_inner_glass_color, ValueType::Vec4, "uInnerGlassColor")?;
    expect_ty(&u_bionic_burn, ValueType::F32, "uBionicBurn")?;
    expect_ty(&u_bionic_unshade, ValueType::F32, "uBionicUnShade")?;
    expect_ty(
        &u_debug_fix_neutral_vibrancy,
        ValueType::F32,
        "uDebugFixNeutralVibrancy",
    )?;
    expect_ty(
        &u_debug_fix_neutral_vibrancy_threshold_min,
        ValueType::F32,
        "uDebugFixNeutralVibrancyThresholdMin",
    )?;
    expect_ty(
        &u_debug_fix_neutral_vibrancy_threshold,
        ValueType::F32,
        "uDebugFixNeutralVibrancyThreshold",
    )?;
    expect_ty(&u_light_dir, ValueType::Vec3, "uDirectionalLightDirection")?;
    expect_ty(
        &u_light_intensity,
        ValueType::F32,
        "uDirectionalLightIntensity",
    )?;
    expect_ty(
        &u_light_opp_intensity,
        ValueType::F32,
        "uDirectionalLightOppositeIntensity",
    )?;
    expect_ty(
        &u_light_angle_range,
        ValueType::F32,
        "uDirectionalLightAngleRange",
    )?;
    expect_ty(&u_alpha, ValueType::F32, "uAlpha")?;
    expect_ty(&u_geo_px_size, ValueType::Vec3, "uGeoPxSize")?;
    expect_ty(&u_use_sdf_tex, ValueType::Bool, "uUseSdfTex")?;

    if let Some(sdf_tex) = u_sdf_tex.as_ref() {
        expect_ty(sdf_tex, ValueType::Texture2D, "uSdfTex")?;
    }

    let out_var = format!("glass_out_{}", sanitize_wgsl_ident(&node.id));

    let (fg_tex, fg_samp) = if let Some((_, tex, samp)) = fg_blur.clone() {
        (tex, samp)
    } else if let Some((_, tex, samp)) = bg.clone() {
        (tex, samp)
    } else {
        (
            "pass_tex__missing".to_string(),
            "pass_samp__missing".to_string(),
        )
    };

    let bg_color_expr = if let Some((_, tex, samp)) = bg_color.clone().or(bg.clone()) {
        format!(
            "glass_texture_map({tex}, {samp}, screen_uv, true, false, {darker}, {darker_range}, {opacity}, {blend_mode}, {fg_tex}, {fg_samp})",
            darker = u_blend_darker.expr,
            darker_range = u_blend_darker_range.expr,
            opacity = u_reflect_lighten_opacity.expr,
            blend_mode = u_reflect_lighten_blend_mode.expr,
            fg_tex = fg_tex,
            fg_samp = fg_samp,
        )
    } else {
        "vec4f(0.0)".to_string()
    };

    let refraction_expr = if let Some((_, tex, samp)) = refract {
        format!(
            "glass_texture_map({tex}, {samp}, refract_uv, true, true, {darker}, {darker_range}, {opacity}, {blend_mode}, {fg_tex}, {fg_samp})",
            darker = u_blend_darker.expr,
            darker_range = u_blend_darker_range.expr,
            opacity = u_reflect_lighten_opacity.expr,
            blend_mode = u_reflect_lighten_blend_mode.expr,
            fg_tex = fg_tex,
            fg_samp = fg_samp,
        )
    } else {
        "vec4f(0.0)".to_string()
    };

    let reflection_expr = if let Some((_, tex, samp)) = reflect.or(bg.clone()) {
        format!(
            "glass_texture_map({tex}, {samp}, reflect_uv, true, true, {darker}, {darker_range}, {opacity}, {blend_mode}, {fg_tex}, {fg_samp})",
            darker = u_blend_darker.expr,
            darker_range = u_blend_darker_range.expr,
            opacity = u_reflect_lighten_opacity.expr,
            blend_mode = u_reflect_lighten_blend_mode.expr,
            fg_tex = fg_tex,
            fg_samp = fg_samp,
        )
    } else {
        "vec4f(0.0)".to_string()
    };

    let mut stmt = String::new();
    let mut push_line = |line: &str| {
        stmt.push_str(line);
        stmt.push('\n');
    };

    push_line(&format!(" // GlassMaterial({})", node.id));
    push_line(&format!(" var {}: vec4f;", out_var));
    push_line(" {");
    push_line("     let screen_px = in.frag_coord_gl;");
    push_line("     let local_px = in.local_px.xy;");
    push_line("     let size_px = in.geo_size_px;");
    push_line("     let safe_size_px = max(size_px, vec2f(1e-6));");
    push_line("     let half_size_px = size_px * 0.5;");
    push_line("     let pos_from_center = local_px - half_size_px;");
    push_line("     let geo_origin_px = screen_px - local_px;");
    push_line("     let screen_uv = glass_sample_screen_uv(screen_px, params.target_size);");
    push_line(&format!(
        "     let edge = f32({});",
        u_shape_edge_px.expr
    ));
    push_line(&format!(
        "     let edge_pow = f32({});",
        u_shape_edge_pow.expr
    ));
    push_line(&format!(
        "     let radius_px = ({}).z;",
        u_geo_px_size.expr
    ));
    push_line("     let safe_edge = max(edge, 1e-6);");
    push_line("     let box_sdf = glass_shape_sdf(pos_from_center, half_size_px, radius_px, edge, edge_pow);");
    push_line("     let normalized_sdf = -box_sdf / safe_edge;");
    push_line("     let edge_sdf = glass_shape_sdf(pos_from_center, half_size_px, radius_px, edge, 1.0);");
    push_line("     let edge_normalized_sdf = -edge_sdf / safe_edge;");
    push_line("     let light_width = edge;");
    push_line("     let light_edge_pow = edge_pow;");
    push_line("     let light_box_sdf = glass_shape_sdf(pos_from_center, half_size_px, radius_px, light_width, light_edge_pow);");
    push_line("     let light_normalized_sdf = -light_box_sdf / max(light_width, 1e-6);");
    push_line("     let normal = glass_calculate_normal(pos_from_center, half_size_px, radius_px, edge, edge_pow);");
    push_line("     let light_normal = glass_calculate_normal(pos_from_center, half_size_px, radius_px, light_width, light_edge_pow);");
    push_line("     var final_alpha = smoothstep(0.0, 10.0, -edge_sdf);");

    if let Some(sdf_tex) = u_sdf_tex.as_ref() {
        let sdf_tex_var = MaterialCompileContext::tex_var_name(&sdf_tex.expr);
        let sdf_samp_var = MaterialCompileContext::sampler_var_name(&sdf_tex.expr);
        push_line(&format!("     if ({}) {{", u_use_sdf_tex.expr));
        push_line(&format!(
            "         let sdf_col = textureSample({}, {}, in.uv);",
            sdf_tex_var, sdf_samp_var
        ));
        push_line("         final_alpha = clamp(sdf_col.w, 0.0, 1.0);");
        push_line("     }");
    }

    push_line(&format!(
        "     let uv_display_px = (local_px - half_size_px) * f32({}) + half_size_px;",
        u_refract_blur_scale.expr
    ));
    push_line("     let incident_ray = normalize(vec3f(0.0, 0.0, -1.0));");
    push_line(&format!(
        "     let refractive_index = f32({});",
        u_refract_ior.expr
    ));
    push_line("     let refract_dir = refract(incident_ray, normal, 1.0 / max(refractive_index, 1e-6));");
    push_line(&format!(
        "     let refract_thickness = mix((f32({}) - edge) * 2.0, f32({}) * 2.0, clamp(normalized_sdf, 0.0, 1.0));",
        u_shape_thickness_px.expr,
        u_shape_thickness_px.expr
    ));
    push_line("     let refract_local_px = uv_display_px + refract_dir.xy * refract_thickness;");
    push_line("     let refract_uv = glass_sample_screen_uv(geo_origin_px + refract_local_px, params.target_size);");
    push_line(&format!("     let refraction = {};", refraction_expr));
    push_line("     let reflect_dir = reflect(incident_ray, normal);");
    push_line(&format!(
        "     let reflect_local_px = uv_display_px + reflect_dir.xy * mix(0.0, f32({}), 1.0 - clamp(normalized_sdf, 0.0, 1.0));",
        u_shape_reflect_offset_px.expr
    ));
    push_line("     let reflect_uv = glass_sample_screen_uv(geo_origin_px + reflect_local_px, params.target_size);");
    push_line(&format!("     let reflection = {};", reflection_expr));
    push_line(&format!(
        "     var glass_mat = mix(refraction, reflection, (1.0 - edge_normalized_sdf) * {});",
        u_reflect_strength.expr
    ));
    push_line(&format!(
        "     glass_mat = vec4f(glass_add_light(glass_mat.rgb, reflection.rgb, (1.0 - light_normalized_sdf) * {}), glass_mat.a);",
        u_reflect_lighten.expr
    ));
    push_line(&format!(
        "     glass_mat = glass_process_color(glass_mat, {}, {}, {}, {});",
        u_blend_luminance_values.expr,
        u_blend_luminance_amount.expr,
        u_blend_saturation.expr,
        u_blend_brightness.expr
    ));
    push_line(&format!("     var glass_color = {};", bg_color_expr));
    push_line(&format!(
        "     glass_color = glass_adjust_color(glass_color, {}, {});",
        u_bg_color_saturation.expr,
        u_bg_color_brightness.expr
    ));
    push_line(&format!(
        "     glass_color = vec4f(mix(glass_color.rgb, vec3f(1.0), {}), glass_color.a);",
        u_inner_color_white.expr
    ));
    push_line("     let glass_color_luma = clamp(glass_luma(glass_color.rgb), 0.0, 1.0);");
    push_line("     var color_ratio = 1.0;");
    push_line(&format!(
        "     let burn_term = pow(glass_color_luma, {}) - 0.5;",
        u_bionic_burn.expr
    ));
    push_line("     let burn_mix = 1.587 * burn_term * burn_term * burn_term + 0.5;");
    push_line("     color_ratio = mix(glass_color_luma, color_ratio, burn_mix);");
    push_line("     color_ratio = color_ratio * 0.8;");
    push_line("     var glass_color_ratio = mix(vec3f(1.0), glass_color.rgb, color_ratio);");
    push_line(&format!(
        "     glass_color_ratio = mix(glass_color_ratio, {}.rgb, {}.a);",
        u_inner_glass_color.expr,
        u_inner_glass_color.expr
    ));
    push_line(&format!(
        "     if ({} > 0.0) {{",
        u_debug_fix_neutral_vibrancy.expr
    ));
    push_line("         let mean_glass_color_ratio = (glass_color_ratio.r + glass_color_ratio.g + glass_color_ratio.b) / 3.0;");
    push_line(&format!(
        "         let neutral_threshold_min = clamp({}, 0.0, 1.0);",
        u_debug_fix_neutral_vibrancy_threshold_min.expr
    ));
    push_line(&format!(
        "         var neutral_threshold = clamp({}, 0.0, 1.0);",
        u_debug_fix_neutral_vibrancy_threshold.expr
    ));
    push_line("         neutral_threshold = max(neutral_threshold, neutral_threshold_min + 0.0001);");
    push_line("         let grayness = distance(glass_color.rgb, vec3f(mean_glass_color_ratio));");
    push_line("         glass_color_ratio = mix(vec3f(mean_glass_color_ratio), glass_color_ratio, smoothstep(neutral_threshold_min, neutral_threshold, grayness));");
    push_line("     }");
    push_line("     glass_mat = vec4f(glass_mat.rgb * mix(vec3f(1.0), glass_color_ratio, 1.0), glass_mat.a);");
    push_line(&format!(
        "     glass_mat = vec4f(mix(glass_mat.rgb, glass_color_ratio, {} * color_ratio), glass_mat.a);",
        u_inner_color_mix.expr
    ));
    push_line(&format!(
        "     glass_mat = vec4f(glass_mat.rgb + vec3f(pow(smoothstep(1.0, 0.0, in.uv.y), 2.0) * {}), glass_mat.a);",
        u_inner_bottom.expr
    ));
    push_line(&format!(
        "     let lighting1 = glass_calculate_lighting(light_normal, {}, {}, {});",
        u_light_dir.expr,
        u_light_intensity.expr,
        u_light_angle_range.expr
    ));
    push_line(&format!(
        "     let lighting2 = glass_calculate_lighting(light_normal, {} * vec3f(-1.0, -1.0, 1.0), {}, {});",
        u_light_dir.expr,
        u_light_opp_intensity.expr,
        u_light_angle_range.expr
    ));
    push_line("     let light_ratio = glass_dynamic_add(glass_mat.rgb);");
    push_line("     glass_mat = vec4f(glass_hsvv(glass_mat.rgb, (lighting1 + lighting2) * 1.0 * light_ratio), glass_mat.a);");
    push_line(&format!(
        "     glass_mat = pow(glass_mat, vec4f({}));",
        u_inner_color_pow.expr
    ));
    push_line(&format!(
        "     glass_mat = vec4f(mix(glass_mat.rgb, {}.rgb, {}), glass_mat.a);",
        u_inner_glass_color.expr,
        u_bionic_unshade.expr
    ));
    push_line(&format!(
        "     glass_mat = vec4f(glass_mat.rgb, glass_mat.a * final_alpha * {});",
        u_alpha.expr
    ));
    push_line(&format!("     {} = glass_mat;", out_var));
    push_line(" }");

    ctx.inline_stmts.push(stmt);
    Ok(TypedExpr::with_time(out_var, ValueType::Vec4, uses_time))
}
