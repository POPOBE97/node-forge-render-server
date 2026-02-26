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

    // Note: keep this WGSL self-contained and avoid relying on external uniforms.
    // All numeric parameters are passed as function arguments.
    let wgsl = r#"
// ---- GlassMaterial helpers (generated) ----

fn glass_luma(color: vec3f) -> f32 {
    return dot(color, vec3f(0.2126, 0.7152, 0.0722));
}

fn glass_rgb2hsv(c: vec3f) -> vec3f {
    let K = vec4f(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    let p = select(vec4f(c.bg, K.wz), vec4f(c.gb, K.xy), c.b < c.g);
    let q = select(vec4f(p.xyw, c.r), vec4f(c.r, p.yzx), p.x < c.r);
    let d = q.x - min(q.w, q.y);
    let e = 1e-10;
    return vec3f(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

fn glass_hsv2rgb(c: vec3f) -> vec3f {
    let K = vec4f(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    let p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, vec3f(0.0), vec3f(1.0)), c.y);
}

fn glass_adjust_color(color: vec4f, saturation: f32, brightness: f32) -> vec4f {
    let luminance = dot(color.rgb, vec3f(0.2125, 0.7153, 0.0721));
    let adjusted_sat = saturation * color.rgb + (1.0 - saturation) * vec3f(luminance);
    let a = color.a;
    let adjusted_bright = adjusted_sat + vec3f(brightness * a);
    return vec4f(adjusted_bright, a);
}

fn glass_luminance_curve(color: vec4f, factors: vec4f, mix_factor: f32) -> vec4f {
    // GLSL mat4 * vec4 factors, expanded in WGSL.
    // adjustment_matrix:
    // -1  3 -3  1
    //  3 -6  3  0
    // -3  3  0  0
    //  1  0  0  0
    let factor_adjust = vec4f(
        -1.0 * factors.x + 3.0 * factors.y + -3.0 * factors.z + 1.0 * factors.w,
        3.0 * factors.x + -6.0 * factors.y + 3.0 * factors.z + 0.0 * factors.w,
        -3.0 * factors.x + 3.0 * factors.y + 0.0 * factors.z + 0.0 * factors.w,
        1.0 * factors.x + 0.0 * factors.y + 0.0 * factors.z + 0.0 * factors.w
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
    let result_rgb = mixed * alpha;
    return vec4f(result_rgb, color.a);
}

fn glass_process_color(color: vec4f, luminance_values: vec4f, luminance_amount: f32, saturation: f32, brightness: f32) -> vec4f {
    var c = glass_luminance_curve(color, luminance_values, luminance_amount);
    c = vec4f(glass_adjust_color(c, saturation, brightness).rgb, c.a);
    return c;
}

// Edge curve approximation from the existing glass test graphs (smooth7_vertical with k=0.5).
fn glass_smooth7_vertical(x: f32, k: f32) -> f32 {
    var t = pow(clamp(x, 0.0, 1.0), k);
    t = mix(0.5, 1.0, t);
    t = clamp(t, 0.0, 1.0);
    let t2 = t * t;
    let t3 = t2 * t;
    let t4 = t3 * t;
    let t5 = t4 * t;
    let t6 = t5 * t;
    let t7 = t6 * t;
    t = -20.0 * t7 + 70.0 * t6 - 84.0 * t5 + 35.0 * t4;
    t = (t - 0.5) * 2.0;
    return t;
}

fn glass_curve(x: f32, pow_ratio: f32) -> f32 {
    if (x >= 0.85) {
        return 1.0;
    }
    let circle = glass_smooth7_vertical(x, 0.5);
    let circle_pow = 1.0 - pow(1.0 - circle, pow_ratio);
    return circle_pow;
}

fn glass_box_sdf(p: vec2f, b: vec2f, r: f32) -> f32 {
    let d = abs(p) - b + vec2f(r);
    return min(max(d.x, d.y), 0.0) + length(max(d, vec2f(0.0))) - r;
}

fn glass_shape_sdf(p: vec2f, b: vec2f, r: f32, edge: f32, edge_pow: f32) -> f32 {
    var d = glass_box_sdf(p, b, r);
    if (d < -edge) {
        d = -edge;
    } else if (d < 0.0) {
        let per = (-d / edge);
        let per2 = glass_curve(per, edge_pow);
        d = -per2 * edge;
    }
    return d;
}

fn glass_calculate_normal(pos_from_center: vec2f, half_size_px: vec2f, radius_px: f32, edge: f32, edge_pow: f32) -> vec3f {
    let eps = 1.0;
    let right_sdf = glass_shape_sdf(pos_from_center + vec2f(eps, 0.0), half_size_px, radius_px, edge, edge_pow);
    let left_sdf = glass_shape_sdf(pos_from_center - vec2f(eps, 0.0), half_size_px, radius_px, edge, edge_pow);
    let top_sdf = glass_shape_sdf(pos_from_center + vec2f(0.0, eps), half_size_px, radius_px, edge, edge_pow);
    let bottom_sdf = glass_shape_sdf(pos_from_center - vec2f(0.0, eps), half_size_px, radius_px, edge, edge_pow);
    let xy_grad = vec2f((right_sdf - left_sdf) * 0.5, (top_sdf - bottom_sdf) * 0.5);
    return normalize(vec3f(xy_grad, 1.0));
}

fn glass_hsvv(col: vec3f, lighten: f32) -> vec3f {
    let v = glass_luma(col);
    let w = smoothstep(0.0, 0.5, v);
    let k = mix(1.0 - v, v, w);
    let g = 1.0 + smoothstep(0.0, 1.0, lighten) * mix(0.75, 0.4, w);
    return (col + vec3f(k)) * g - vec3f(k);
}

fn glass_dynamic_add(color: vec3f) -> f32 {
    var white_dis = distance(vec3f(1.0), color);
    white_dis = smoothstep(0.2, 1.0, white_dis);
    white_dis = mix(0.5, 1.0, white_dis);
    let lumin = glass_luma(color);
    return lumin * white_dis;
}

fn glass_add_light(color: vec3f, light_color: vec3f, light_strength: f32) -> vec3f {
    var white_dis = distance(vec3f(1.0), color);
    white_dis = smoothstep(0.2, 1.0, white_dis);
    white_dis = mix(0.3, 1.0, white_dis);
    let s = light_strength * white_dis;
    return color + light_color * s;
}

fn glass_calculate_lighting(normal: vec3f, light_dir: vec3f, intensity: f32, angle_range: f32) -> f32 {
    let nld = normalize(light_dir);
    let dp = dot(normal, nld);
    let reflection_angle = acos(clamp(dp, -1.0, 1.0));
    let angle_factor = 1.0 - (reflection_angle / (3.14159 * angle_range));
    let adjusted = max(intensity * angle_factor, 0.0);
    return max(dp, 0.0) * adjusted;
}

fn glass_texture_map(
    tex: texture_2d<f32>,
    samp: sampler,
    uv: vec2f,
    is_bg: bool,
    darker: f32,
    darker_range: vec2f,
    fg_tex: texture_2d<f32>,
    fg_samp: sampler,
) -> vec4f {
    var col = textureSample(tex, samp, uv);

    if (is_bg) {
        let lum = glass_luma(col.rgb);
        let dark = mix(0.0, darker, smoothstep(darker_range.x, darker_range.y, lum));
        col = vec4f(mix(col.rgb, vec3f(0.0), dark), col.a);
    }

    let fg_col = textureSample(fg_tex, fg_samp, uv);
    let lighten = fg_col.r;
    col = vec4f(glass_hsvv(col.rgb, lighten), col.a);
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

    // Numeric inputs: allow either incoming connection or inline params fallback.
    // For v1 we intentionally keep this permissive to make authoring easier.
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

    let u_brightness = input_expr("uBrightness", input_f32_expr(node, "uBrightness", 0.0))?;
    let u_lumin_amount = input_expr(
        "uLuminanceAmount",
        input_f32_expr(node, "uLuminanceAmount", 0.0),
    )?;
    let u_lumin_values = input_expr(
        "uLuminanceValues",
        input_vec4_expr(node, "uLuminanceValues", [0.0, 0.0, 0.0, 0.0]),
    )?;
    let u_saturation = input_expr("uSaturation", input_f32_expr(node, "uSaturation", 1.0))?;
    let u_darker = input_expr("uDarker", input_f32_expr(node, "uDarker", 0.0))?;
    let u_darker_range = input_expr(
        "uDarkerRange",
        input_vec2_expr(node, "uDarkerRange", [0.0, 1.0]),
    )?;

    let u_shape_edge_px = input_expr("uShapeEdgePx", input_f32_expr(node, "uShapeEdgePx", 30.0))?;
    let u_shape_edge_pow = input_expr("uShapeEdgePow", input_f32_expr(node, "uShapeEdgePow", 2.0))?;
    let u_shape_thickness_px = input_expr(
        "uShapeThicknessPx",
        input_f32_expr(node, "uShapeThicknessPx", 50.0),
    )?;
    let u_shape_reflect_offset_px = input_expr(
        "uShapeReflectOffsetPx",
        input_f32_expr(node, "uShapeReflectOffsetPx", 0.0),
    )?;

    let u_refract_blur_scale = input_expr(
        "uRefractBlurScale",
        input_f32_expr(node, "uRefractBlurScale", 1.0),
    )?;
    let u_refract_ior = input_expr("uRefractIOR", input_f32_expr(node, "uRefractIOR", 1.5))?;

    let u_reflection_strength = input_expr(
        "uReflectionStrength",
        input_f32_expr(node, "uReflectionStrength", 0.5),
    )?;
    let u_reflection_lighten = input_expr(
        "uReflectionLighten",
        input_f32_expr(node, "uReflectionLighten", 0.5),
    )?;

    let u_inner_bottom = input_expr("uInnerBottom", input_f32_expr(node, "uInnerBottom", 0.0))?;
    let u_inner_color_white = input_expr(
        "uInnerColorWhite",
        input_f32_expr(node, "uInnerColorWhite", 0.0),
    )?;
    let u_inner_color_mix = input_expr(
        "uInnerColorMix",
        input_f32_expr(node, "uInnerColorMix", 0.0),
    )?;

    let u_color_pow = input_expr("uColorPow", input_f32_expr(node, "uColorPow", 1.0))?;
    let u_glass_color = input_expr(
        "uGlassColor",
        input_vec4_expr(node, "uGlassColor", [1.0, 1.0, 1.0, 0.0]),
    )?;

    let u_light_dir = input_expr(
        "uDirectionalLightDirection",
        input_vec3_expr(node, "uDirectionalLightDirection", [0.5, -0.7, 0.5]),
    )?;
    let u_light_intensity = input_expr(
        "uDirectionalLightIntensity",
        input_f32_expr(node, "uDirectionalLightIntensity", 1.0),
    )?;
    let u_light_opp_intensity = input_expr(
        "uDirectionalLightOppositeIntensity",
        input_f32_expr(node, "uDirectionalLightOppositeIntensity", 0.0),
    )?;
    let u_light_angle_range = input_expr(
        "uDirectionalLightAngleRange",
        input_f32_expr(node, "uDirectionalLightAngleRange", 0.5),
    )?;
    let u_light_width = input_expr(
        "uDirectionalLightLightWidth",
        input_f32_expr(node, "uDirectionalLightLightWidth", 30.0),
    )?;
    let u_light_edge_pow = input_expr(
        "uDirectionalLightEdgePow",
        input_f32_expr(node, "uDirectionalLightEdgePow", 2.0),
    )?;

    let u_bg_color_brightness = input_expr(
        "uBgColorBrightness",
        input_f32_expr(node, "uBgColorBrightness", 0.0),
    )?;
    let u_bg_color_saturation = input_expr(
        "uBgColorSaturation",
        input_f32_expr(node, "uBgColorSaturation", 1.0),
    )?;

    let u_strength = input_expr("uStrength", input_f32_expr(node, "uStrength", 1.0))?;
    let u_highlight_strength = input_expr(
        "uHighlightStrength",
        input_f32_expr(node, "uHighlightStrength", 0.0),
    )?;
    let u_alpha = input_expr("uAlpha", input_f32_expr(node, "uAlpha", 1.0))?;

    let u_geo_px_pos = input_expr("uGeoPxPos", input_vec2_expr(node, "uGeoPxPos", [0.0, 0.0]))?;
    let u_geo_px_size = input_expr(
        "uGeoPxSize",
        input_vec3_expr(node, "uGeoPxSize", [0.0, 0.0, 20.0]),
    )?;

    let u_use_sdf_tex = input_expr("uUseSdfTex", input_bool_expr(node, "uUseSdfTex", false))?;

    // Optional SDF mask texture handle.
    // This is an opaque resource binding, so we only accept connections.
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

    // Pass texture inputs.
    let bg = resolve_pass_binding(scene, nodes_by_id, node, "uBgTex", ctx)?;
    let refract = resolve_pass_binding(scene, nodes_by_id, node, "uRefractTex", ctx)?;
    let reflect = resolve_pass_binding(scene, nodes_by_id, node, "uReflectTex", ctx)?;
    let fg_blur = resolve_pass_binding(scene, nodes_by_id, node, "uReflectForegroundBlurTex", ctx)?;
    let bg_color = resolve_pass_binding(scene, nodes_by_id, node, "uBgColorTex", ctx)?;

    // If any of the numeric inputs are time-dependent, preserve that.
    let uses_time = [
        u_brightness.uses_time,
        u_lumin_amount.uses_time,
        u_lumin_values.uses_time,
        u_saturation.uses_time,
        u_darker.uses_time,
        u_darker_range.uses_time,
        u_shape_edge_px.uses_time,
        u_shape_edge_pow.uses_time,
        u_shape_thickness_px.uses_time,
        u_shape_reflect_offset_px.uses_time,
        u_refract_blur_scale.uses_time,
        u_refract_ior.uses_time,
        u_reflection_strength.uses_time,
        u_reflection_lighten.uses_time,
        u_inner_bottom.uses_time,
        u_inner_color_white.uses_time,
        u_inner_color_mix.uses_time,
        u_color_pow.uses_time,
        u_glass_color.uses_time,
        u_light_dir.uses_time,
        u_light_intensity.uses_time,
        u_light_opp_intensity.uses_time,
        u_light_angle_range.uses_time,
        u_light_width.uses_time,
        u_light_edge_pow.uses_time,
        u_bg_color_brightness.uses_time,
        u_bg_color_saturation.uses_time,
        u_strength.uses_time,
        u_highlight_strength.uses_time,
        u_alpha.uses_time,
        u_geo_px_pos.uses_time,
        u_geo_px_size.uses_time,
        u_use_sdf_tex.uses_time,
        u_sdf_tex.as_ref().map(|x| x.uses_time).unwrap_or(false),
    ]
    .into_iter()
    .any(|v| v);

    // Validate types we rely on. Keep this strict to avoid generating invalid WGSL.
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

    expect_ty(&u_brightness, ValueType::F32, "uBrightness")?;
    expect_ty(&u_lumin_amount, ValueType::F32, "uLuminanceAmount")?;
    expect_ty(&u_lumin_values, ValueType::Vec4, "uLuminanceValues")?;
    expect_ty(&u_saturation, ValueType::F32, "uSaturation")?;
    expect_ty(&u_darker, ValueType::F32, "uDarker")?;
    expect_ty(&u_darker_range, ValueType::Vec2, "uDarkerRange")?;
    expect_ty(&u_shape_edge_px, ValueType::F32, "uShapeEdgePx")?;
    expect_ty(&u_shape_edge_pow, ValueType::F32, "uShapeEdgePow")?;
    expect_ty(&u_shape_thickness_px, ValueType::F32, "uShapeThicknessPx")?;
    expect_ty(
        &u_shape_reflect_offset_px,
        ValueType::F32,
        "uShapeReflectOffsetPx",
    )?;
    expect_ty(&u_refract_blur_scale, ValueType::F32, "uRefractBlurScale")?;
    expect_ty(&u_refract_ior, ValueType::F32, "uRefractIOR")?;
    expect_ty(
        &u_reflection_strength,
        ValueType::F32,
        "uReflectionStrength",
    )?;
    expect_ty(&u_reflection_lighten, ValueType::F32, "uReflectionLighten")?;
    expect_ty(&u_inner_bottom, ValueType::F32, "uInnerBottom")?;
    expect_ty(&u_inner_color_white, ValueType::F32, "uInnerColorWhite")?;
    expect_ty(&u_inner_color_mix, ValueType::F32, "uInnerColorMix")?;
    expect_ty(&u_color_pow, ValueType::F32, "uColorPow")?;
    expect_ty(&u_glass_color, ValueType::Vec4, "uGlassColor")?;
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
    expect_ty(
        &u_light_width,
        ValueType::F32,
        "uDirectionalLightLightWidth",
    )?;
    expect_ty(
        &u_light_edge_pow,
        ValueType::F32,
        "uDirectionalLightEdgePow",
    )?;
    expect_ty(&u_bg_color_brightness, ValueType::F32, "uBgColorBrightness")?;
    expect_ty(&u_bg_color_saturation, ValueType::F32, "uBgColorSaturation")?;
    expect_ty(&u_strength, ValueType::F32, "uStrength")?;
    expect_ty(&u_highlight_strength, ValueType::F32, "uHighlightStrength")?;
    expect_ty(&u_alpha, ValueType::F32, "uAlpha")?;
    expect_ty(&u_geo_px_pos, ValueType::Vec2, "uGeoPxPos")?;
    expect_ty(&u_geo_px_size, ValueType::Vec3, "uGeoPxSize")?;
    expect_ty(&u_use_sdf_tex, ValueType::Bool, "uUseSdfTex")?;

    if let Some(sdf_tex) = u_sdf_tex.as_ref() {
        expect_ty(sdf_tex, ValueType::Texture2D, "uSdfTex")?;
    }

    let sdf_override_stmt = if let Some(sdf_tex) = u_sdf_tex.as_ref() {
        let sdf_tex_var = MaterialCompileContext::tex_var_name(&sdf_tex.expr);
        let sdf_samp_var = MaterialCompileContext::sampler_var_name(&sdf_tex.expr);
        format!(
            r#"

    // Optional external SDF alpha override.
    // Expected: `uSdfTex` is a texture handle; `.w` contains alpha.
    if ({use_sdf}) {{
        let sdf_col = textureSample({tex}, {samp}, uv);
        final_alpha = clamp(sdf_col.w, 0.0, 1.0);
    }}
"#,
            use_sdf = u_use_sdf_tex.expr,
            tex = sdf_tex_var,
            samp = sdf_samp_var,
        )
    } else {
        String::new()
    };

    // Build the inline WGSL.
    let out_var = format!("glass_out_{}", sanitize_wgsl_ident(&node.id));

    // For pass texture sampling we need a foreground blur texture; if missing, just use bg as a fallback.
    let (fg_tex, fg_samp) = if let Some((_, t, s)) = fg_blur.clone() {
        (t, s)
    } else if let Some((_, t, s)) = bg.clone() {
        (t, s)
    } else {
        // This will produce a compile error if used; we guard by short-circuiting to transparent.
        (
            "pass_tex__missing".to_string(),
            "pass_samp__missing".to_string(),
        )
    };

    // Construct sampling expressions.
    let bg_sample = if let Some((_, t, s)) = bg {
        format!(
            "glass_texture_map({t}, {s}, uv, false, {}, {}, {fg_tex}, {fg_samp})",
            u_darker.expr, u_darker_range.expr
        )
    } else {
        "vec4f(0.0)".to_string()
    };

    let bg_color_sample = if let Some((_, t, s)) = bg_color {
        format!(
            "glass_texture_map({t}, {s}, uv, true, {}, {}, {fg_tex}, {fg_samp})",
            u_darker.expr, u_darker_range.expr
        )
    } else {
        "vec4f(0.0)".to_string()
    };

    // Refract/reflect samples are treated as background (is_bg=true) to apply the same darker/hsvv shaping.
    let refract_tex = refract.clone().map(|(_, t, s)| (t, s));
    let reflect_tex = reflect.clone().map(|(_, t, s)| (t, s));

    let refract_t = refract_tex
        .clone()
        .map(|x| x.0)
        .unwrap_or("pass_tex__missing".to_string());
    let refract_s = refract_tex
        .clone()
        .map(|x| x.1)
        .unwrap_or("pass_samp__missing".to_string());
    let reflect_t = reflect_tex
        .clone()
        .map(|x| x.0)
        .unwrap_or(refract_t.clone());
    let reflect_s = reflect_tex
        .clone()
        .map(|x| x.1)
        .unwrap_or(refract_s.clone());

    // Inline block: compute glass shading in pixel space.
    // The renderer's coordinate system:
    // - `in.frag_coord_gl` is bottom-left origin, pixel-centered.
    // - `in.local_px` is geometry-local pixel coordinate (origin at geometry bottom-left).
    //
    // NOTE: For instanced geometry, `params.center` is per-pass (not per-instance), so we must
    // derive the geometry center from `in.local_px` to keep SDFs stable per instance.
    let stmt = format!(
        r#"
 // GlassMaterial({node_id})
 var {out_var}: vec4f;
 {{
     let screen_px = in.frag_coord_gl;
     let local_px = in.local_px.xy;
     let size_px = in.geo_size_px;
     let half_size_px = size_px * 0.5;

     // Geometry center in screen pixels (works for instanced geometry too).
     let center_px = screen_px - local_px + half_size_px;

     // SDF evaluation uses geometry-local pixels (not screen pixels).
     let pos_from_center = local_px - half_size_px;

    // Use in.uv (top-left convention) for sampling pass/render-target textures.
    let uv = in.uv;
    // Bottom-left normalized UV for procedural effects (gradients, etc.).
    let gl_uv = vec2f(in.uv.x, 1.0 - in.uv.y);

    // Base background color.
    var color = {bg_sample};

    // --- Shape / SDF ---
    // NOTE: many scene exports encode scalar params as integers (e.g. `30`).
    // If we emit `let edge = 30;`, WGSL infers `i32` and later `max(edge, 1e-6)` fails.
    // Force scalar params into `f32` so type inference stays stable.
    let edge = f32({u_shape_edge_px});
    let edge_pow = f32({u_shape_edge_pow});
    // `uGeoPxSize.z` is the author-controlled corner radius in pixels.
    // For v1 we still use runtime geometry size (`in.geo_size_px`) for width/height.
    let radius_px = ({u_geo_px_size}).z;

    let box_sdf = glass_shape_sdf(pos_from_center, half_size_px, radius_px, edge, edge_pow);
    let normalized_sdf = (-box_sdf / max(edge, 1e-6));

    // Inner/edge SDFs.
    let inner_edge_width = edge * 10.0;
    let inner_edge_sdf = glass_shape_sdf(pos_from_center, half_size_px, radius_px, inner_edge_width, edge_pow);
    let inner_edge_norm = (-inner_edge_sdf / max(inner_edge_width, 1e-6));

    let edge_sdf = glass_shape_sdf(pos_from_center, half_size_px, radius_px, edge, 1.0);
    let edge_norm = (-edge_sdf / max(edge, 1e-6));

    // Light normal uses a wider edge to create a softer highlight.
    let light_width = f32({u_light_width});
    let light_edge_pow = f32({u_light_edge_pow});
    let light_sdf = glass_shape_sdf(pos_from_center, half_size_px, radius_px, light_width, light_edge_pow);
    let light_norm = (-light_sdf / max(light_width, 1e-6));

     let normal = glass_calculate_normal(pos_from_center, half_size_px, radius_px, edge, edge_pow);
     let light_normal = glass_calculate_normal(pos_from_center, half_size_px, radius_px, light_width, light_edge_pow);

    var final_alpha = smoothstep(0.0, 10.0, -edge_sdf);

{sdf_override_stmt}

    // --- Refraction / reflection sampling ---
    // Apply a mild blur scale by scaling the *offset* region.
    let display_px = (screen_px - center_px) * f32({u_refract_blur_scale}) + center_px;
    let incident = normalize(vec3f(0.0, 0.0, -1.0));
    let refractive_index = mix(f32({u_refract_ior}), f32({u_refract_ior}) + 0.2, f32({u_strength}));
    let refr_dir = refract(incident, normal, 1.0 / max(refractive_index, 1e-6));

    let refract_thickness = mix((f32({u_shape_thickness_px}) - edge) * 2.0, f32({u_shape_thickness_px}) * 2.0, clamp(normalized_sdf, 0.0, 1.0));
    let refract_px = display_px + refr_dir.xy * refract_thickness;
    let refract_uv = refract_px / params.target_size;
    let refraction = glass_texture_map({refract_t}, {refract_s}, refract_uv, true, {u_darker}, {u_darker_range}, {fg_tex}, {fg_samp});

    let refl_dir = reflect(incident, normal);
    let reflect_px = display_px + refl_dir.xy * mix(0.0, f32({u_shape_reflect_offset_px}), 1.0 - clamp(normalized_sdf, 0.0, 1.0));
    let reflect_uv = reflect_px / params.target_size;
    let reflection = glass_texture_map({reflect_t}, {reflect_s}, reflect_uv, true, {u_darker}, {u_darker_range}, {fg_tex}, {fg_samp});

    // Mix refraction/reflection.
    var glass_mat = mix(refraction, reflection, (1.0 - clamp(edge_norm, 0.0, 1.0)) * {u_reflection_strength});
    glass_mat = vec4f(glass_add_light(glass_mat.rgb, reflection.rgb, (1.0 - clamp(light_norm, 0.0, 1.0)) * {u_reflection_lighten}), glass_mat.a);

    // Color grading.
    glass_mat = glass_process_color(glass_mat, {u_lumin_values}, {u_lumin_amount}, {u_saturation}, {u_brightness});

    // Inner glass tint from blurred background color.
    var tint = {bg_color_sample};
    tint = glass_adjust_color(tint, {u_bg_color_saturation}, {u_bg_color_brightness});
    tint = vec4f(mix(tint.rgb, vec3f(1.0), {u_inner_color_white}), tint.a);

    let lum_tint = glass_luma(tint.rgb);
    var color_ratio = 1.0;
    color_ratio = mix(0.5, color_ratio, lum_tint);
    color_ratio = color_ratio * 0.8;

    var glass_color_ratio = mix(vec3f(1.0), tint.rgb, color_ratio);
    glass_color_ratio = mix(glass_color_ratio, {u_glass_color}.rgb, {u_glass_color}.a);
    glass_mat = vec4f(mix(glass_mat.rgb, glass_color_ratio, {u_inner_color_mix} * color_ratio), glass_mat.a);

    // Inner bottom gradient (uses bottom-left UV for correct visual direction).
    glass_mat = vec4f(
        glass_mat.rgb + vec3f(pow(smoothstep(1.0, 0.0, gl_uv.y), 2.0) * {u_inner_bottom}),
        glass_mat.a,
    );

    // Directional lighting.
    let light_strength = mix(f32({u_light_intensity}), f32({u_light_intensity}) + 1.0, f32({u_highlight_strength}));
    let light_strength_opp = f32({u_light_opp_intensity});
    let l1 = glass_calculate_lighting(light_normal, {u_light_dir}, light_strength, {u_light_angle_range});
    let l2 = glass_calculate_lighting(light_normal, {u_light_dir} * vec3f(-1.0, -1.0, 1.0), light_strength_opp, {u_light_angle_range});

    let plus_light = 0.0;
    let hsv_light = 1.2;
    let light_ratio = glass_dynamic_add(glass_mat.rgb);
    var hsv = glass_rgb2hsv(glass_mat.rgb);
    hsv.z = clamp(hsv.z + (l1 + l2) * hsv_light * light_ratio, 0.0, 1.0);
    glass_mat = vec4f(glass_hsv2rgb(hsv), glass_mat.a);

    // Apply power curve.
    glass_mat = pow(glass_mat, vec4f({u_color_pow}));

    // Alpha.
    glass_mat = vec4f(glass_mat.rgb, glass_mat.a * final_alpha * {u_alpha});

    // Composite over background.
    // Use premultiplied alpha output.
    let out_rgb = glass_mat.rgb;
    let out_a = glass_mat.a;
    {out_var} = vec4f(out_rgb, out_a);
}}
"#,
        node_id = node.id,
        out_var = out_var,
        bg_sample = bg_sample,
        bg_color_sample = bg_color_sample,
        u_shape_edge_px = u_shape_edge_px.expr,
        u_shape_edge_pow = u_shape_edge_pow.expr,
        u_refract_blur_scale = u_refract_blur_scale.expr,
        u_refract_ior = u_refract_ior.expr,
        u_strength = u_strength.expr,
        u_shape_thickness_px = u_shape_thickness_px.expr,
        u_shape_reflect_offset_px = u_shape_reflect_offset_px.expr,
        u_darker = u_darker.expr,
        u_darker_range = u_darker_range.expr,
        u_reflection_strength = u_reflection_strength.expr,
        u_reflection_lighten = u_reflection_lighten.expr,
        u_lumin_values = u_lumin_values.expr,
        u_lumin_amount = u_lumin_amount.expr,
        u_saturation = u_saturation.expr,
        u_brightness = u_brightness.expr,
        u_bg_color_saturation = u_bg_color_saturation.expr,
        u_bg_color_brightness = u_bg_color_brightness.expr,
        u_inner_color_white = u_inner_color_white.expr,
        u_inner_color_mix = u_inner_color_mix.expr,
        u_glass_color = u_glass_color.expr,
        u_inner_bottom = u_inner_bottom.expr,
        u_light_width = u_light_width.expr,
        u_light_edge_pow = u_light_edge_pow.expr,
        u_light_intensity = u_light_intensity.expr,
        u_highlight_strength = u_highlight_strength.expr,
        u_light_opp_intensity = u_light_opp_intensity.expr,
        u_light_dir = u_light_dir.expr,
        u_light_angle_range = u_light_angle_range.expr,
        u_color_pow = u_color_pow.expr,
        u_alpha = u_alpha.expr,
        u_geo_px_size = u_geo_px_size.expr,
        sdf_override_stmt = sdf_override_stmt,
        refract_t = refract_t,
        refract_s = refract_s,
        reflect_t = reflect_t,
        reflect_s = reflect_s,
        fg_tex = fg_tex,
        fg_samp = fg_samp,
    );

    // If there is no background pass connected, we cannot sample anything meaningful.
    // Still emit deterministic output (transparent).
    ctx.inline_stmts.push(stmt);
    Ok(TypedExpr::with_time(out_var, ValueType::Vec4, uses_time))
}
