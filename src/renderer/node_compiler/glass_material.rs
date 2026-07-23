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

use super::super::types::{MaterialCompileContext, PassTextureRef, TypedExpr, ValueType};
use crate::dsl::{Node, SceneDSL, incoming_connection, parse_f32};
use crate::renderer::geometry_resolver::is_pass_like_node_type;
use crate::renderer::utils::fmt_f32;

fn substitute_template(template: &str, vars: &[(&str, String)]) -> String {
    let mut result = String::with_capacity(template.len());
    let mut remaining = template;

    // Strip leading comment/empty lines
    while remaining.starts_with("//") || remaining.starts_with('\n') {
        if let Some(nl) = remaining.find('\n') {
            remaining = &remaining[nl + 1..];
        } else {
            remaining = "";
        }
    }

    // Process {{#if key}}...{{/if}} blocks first
    loop {
        if let Some(if_start) = remaining.find("{{#if ") {
            result.push_str(&remaining[..if_start]);
            let after_if = &remaining[if_start + 6..];
            let key_end = after_if.find("}}").expect("unclosed {{#if}}");
            let key = &after_if[..key_end];
            let block_start = &after_if[key_end + 2..];
            // Find matching {{/if}}
            let endif = block_start.find("{{/if}}").expect("missing {{/if}}");
            let block_content = &block_start[..endif];
            remaining = &block_start[endif + 7..];

            let has_key = vars.iter().any(|(k, _)| *k == key);
            if has_key {
                // Strip leading/trailing newline from the block
                let trimmed = block_content.strip_prefix('\n').unwrap_or(block_content);
                let trimmed = trimmed.strip_suffix('\n').unwrap_or(trimmed);
                result.push_str(trimmed);
                result.push('\n');
            }
        } else {
            result.push_str(remaining);
            break;
        }
    }

    // Perform {{key}} substitutions
    for (key, value) in vars {
        let pattern = format!("{{{{{}}}}}", key);
        result = result.replace(&pattern, value);
    }

    result
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
    if !is_pass_like_node_type(&upstream.node_type) {
        bail!(
            "GlassMaterial.{port_id} must be connected to a pass node, got {}",
            upstream.node_type
        );
    }

    let texture_ref = PassTextureRef::direct(&upstream_id, &conn.from.port_id);
    ctx.register_pass_texture_ref(texture_ref.clone());
    let tex_var = MaterialCompileContext::pass_tex_var_name(&texture_ref.binding_id);
    let samp_var = MaterialCompileContext::pass_sampler_var_name(&texture_ref.binding_id);
    Ok(Some((texture_ref.binding_id, tex_var, samp_var)))
}

const GLASS_WGSL_LIB_KEY: &str = "glass_material_lib";
const GLASS_TEMPLATE_SPLIT: &str = "\n// {{BODY}}\n";

fn glass_override_path(node: &Node) -> Option<std::path::PathBuf> {
    node.wgsl_override
        .as_deref()
        .and_then(super::template_loader::resolve_override_path)
}

fn glass_template_parts(node: &Node) -> (String, String) {
    let path = glass_override_path(node);
    let full = super::template_loader::load_template_with_override(
        path.as_deref(),
        "glass_material_fragment.wgsl",
    );
    let split_pos = full
        .find(GLASS_TEMPLATE_SPLIT)
        .expect("template missing // {{BODY}} marker");
    let helpers_raw = &full[..split_pos];
    let body = full[split_pos + GLASS_TEMPLATE_SPLIT.len()..].to_owned();
    let mut helpers_start = helpers_raw;
    while helpers_start.starts_with("//") || helpers_start.starts_with('\n') {
        if let Some(nl) = helpers_start.find('\n') {
            helpers_start = &helpers_start[nl + 1..];
        } else {
            helpers_start = "";
        }
    }
    (helpers_start.to_owned(), body)
}

/// When a node has a per-node override, register its helpers under a node-scoped
/// lib key so overrides from different nodes don't stomp each other in
/// `extra_wgsl_decls`.
///
/// Note: helper *symbol* names (e.g. `fn glass_blur_*`) are NOT renamed per-node.
/// Two GlassMaterial nodes with structurally different override contents will
/// therefore surface a duplicate-symbol error at WGSL compile time. The common
/// case (one override per scene, or identical overrides shared by multiple
/// nodes) works without intervention.
fn glass_lib_key_for(node: &Node) -> String {
    if node.wgsl_override.is_some() {
        let suffix: String = node
            .id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        format!("{GLASS_WGSL_LIB_KEY}::{suffix}")
    } else {
        GLASS_WGSL_LIB_KEY.to_string()
    }
}

fn ensure_glass_wgsl_lib(ctx: &mut MaterialCompileContext, node: &Node) {
    super::sdf_nodes::ensure_default_sdf2d_wgsl_lib(ctx);

    let lib_key = glass_lib_key_for(node);
    if ctx.extra_wgsl_decls.contains_key(&lib_key) {
        return;
    }

    let (helpers, _) = glass_template_parts(node);
    let header = if node.wgsl_override.is_some() {
        format!(
            "\n// ---- GlassMaterial helpers (generated, override for {}) ----\n\n",
            node.id
        )
    } else {
        "\n// ---- GlassMaterial helpers (generated) ----\n\n".to_string()
    };
    ctx.extra_wgsl_decls
        .insert(lib_key, format!("{header}{helpers}"));
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
    ensure_glass_wgsl_lib(ctx, node);

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

    let u_shape_edge_px = input_expr("uShapeEdgePx", input_f32_expr(node, "uShapeEdgePx", 20.0))?;
    let u_shape_edge_pow = input_expr("uShapeEdgePow", input_f32_expr(node, "uShapeEdgePow", 2.0))?;
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

    let u_inner_bottom = input_expr("uInnerBottom", input_f32_expr(node, "uInnerBottom", 0.05))?;
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

    let u_bionic_burn = input_expr("uBionicBurn", input_f32_expr(node, "uBionicBurn", 11.0))?;
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
    let u_geo_px_radius = input_expr("uGeoPxRadius", input_f32_expr(node, "uGeoPxRadius", 24.0))?;
    let u_use_sdf_tex = input_expr("uUseSdfTex", input_bool_expr(node, "uUseSdfTex", false))?;

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
    let fg_blur = resolve_pass_binding(scene, nodes_by_id, node, "uLightTex", ctx)?;

    let uses_time = [
        u_blend_luminance_amount.uses_time,
        u_blend_luminance_values.uses_time,
        u_blend_saturation.uses_time,
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
        u_geo_px_radius.uses_time,
        u_use_sdf_tex.uses_time,
        u_sdf_tex
            .as_ref()
            .map(|expr| expr.uses_time)
            .unwrap_or(false),
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
    expect_ty(&u_geo_px_radius, ValueType::F32, "uGeoPxRadius")?;
    expect_ty(&u_use_sdf_tex, ValueType::Bool, "uUseSdfTex")?;

    if let Some(sdf_tex) = u_sdf_tex.as_ref() {
        expect_ty(sdf_tex, ValueType::Texture2D, "uSdfTex")?;
    }

    let out_var =
        super::readable_node_temp_name(ctx, "fs", node, _out_port.unwrap_or("color"), "out");

    let has_fg_light = fg_blur.is_some();
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
    let add_foreground = if has_fg_light { "true" } else { "false" };

    let bg_color_expr = if let Some((_, tex, samp)) = bg_color.clone().or(bg.clone()) {
        format!(
            "glass_texture_map({tex}, {samp}, screen_uv, {add_foreground}, {opacity}, {blend_mode}, {fg_tex}, {fg_samp}, screen_uv)",
            opacity = u_reflect_lighten_opacity.expr,
            blend_mode = u_reflect_lighten_blend_mode.expr,
            fg_tex = fg_tex,
            fg_samp = fg_samp,
        )
    } else {
        "vec4f(0.0)".to_string()
    };

    let refraction_expr = if let Some((_, tex, samp)) = refract.or(bg.clone()) {
        format!(
            "glass_texture_map({tex}, {samp}, refract_uv, {add_foreground}, {opacity}, {blend_mode}, {fg_tex}, {fg_samp}, screen_uv)",
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
            "glass_texture_map({tex}, {samp}, reflect_uv, {add_foreground}, {opacity}, {blend_mode}, {fg_tex}, {fg_samp}, screen_uv)",
            opacity = u_reflect_lighten_opacity.expr,
            blend_mode = u_reflect_lighten_blend_mode.expr,
            fg_tex = fg_tex,
            fg_samp = fg_samp,
        )
    } else {
        "vec4f(0.0)".to_string()
    };

    let radius_expr = if incoming_connection(scene, &node.id, "uGeoPxRadius").is_some() {
        format!("f32({})", u_geo_px_radius.expr)
    } else {
        format!("({}).z", u_geo_px_size.expr)
    };

    let mut vars: Vec<(&str, String)> = vec![
        ("node_id", node.id.clone()),
        ("out_var", out_var.clone()),
        ("uShapeEdgePx", u_shape_edge_px.expr.clone()),
        ("uShapeEdgePow", u_shape_edge_pow.expr.clone()),
        ("radius_expr", radius_expr),
        ("uRefractBlurScale", u_refract_blur_scale.expr.clone()),
        ("uRefractIOR", u_refract_ior.expr.clone()),
        ("uShapeThicknessPx", u_shape_thickness_px.expr.clone()),
        (
            "uShapeReflectOffsetPx",
            u_shape_reflect_offset_px.expr.clone(),
        ),
        ("refraction_expr", refraction_expr),
        ("reflection_expr", reflection_expr),
        ("uReflectStrength", u_reflect_strength.expr.clone()),
        ("uReflectLighten", u_reflect_lighten.expr.clone()),
        (
            "uBlendLuminanceValues",
            u_blend_luminance_values.expr.clone(),
        ),
        (
            "uBlendLuminanceAmount",
            u_blend_luminance_amount.expr.clone(),
        ),
        ("uBlendSaturation", u_blend_saturation.expr.clone()),
        ("bg_color_expr", bg_color_expr),
        ("uBgColorSaturation", u_bg_color_saturation.expr.clone()),
        ("uBgColorBrightness", u_bg_color_brightness.expr.clone()),
        ("uInnerColorWhite", u_inner_color_white.expr.clone()),
        ("uBionicBurn", u_bionic_burn.expr.clone()),
        ("uInnerGlassColor", u_inner_glass_color.expr.clone()),
        (
            "uDebugFixNeutralVibrancy",
            u_debug_fix_neutral_vibrancy.expr.clone(),
        ),
        (
            "uDebugFixNeutralVibrancyThresholdMin",
            u_debug_fix_neutral_vibrancy_threshold_min.expr.clone(),
        ),
        (
            "uDebugFixNeutralVibrancyThreshold",
            u_debug_fix_neutral_vibrancy_threshold.expr.clone(),
        ),
        ("uInnerColorMix", u_inner_color_mix.expr.clone()),
        ("uInnerBottom", u_inner_bottom.expr.clone()),
        ("uDirectionalLightDirection", u_light_dir.expr.clone()),
        ("uDirectionalLightIntensity", u_light_intensity.expr.clone()),
        (
            "uDirectionalLightOppositeIntensity",
            u_light_opp_intensity.expr.clone(),
        ),
        (
            "uDirectionalLightAngleRange",
            u_light_angle_range.expr.clone(),
        ),
        ("uInnerColorPow", u_inner_color_pow.expr.clone()),
        ("uBionicUnShade", u_bionic_unshade.expr.clone()),
        ("uAlpha", u_alpha.expr.clone()),
    ];

    if let Some(sdf_tex) = u_sdf_tex.as_ref() {
        let sdf_tex_var = MaterialCompileContext::tex_var_name(&sdf_tex.expr);
        let sdf_samp_var = MaterialCompileContext::sampler_var_name(&sdf_tex.expr);
        vars.push(("use_sdf_tex", "1".to_string()));
        vars.push(("uUseSdfTex", u_use_sdf_tex.expr.clone()));
        vars.push(("sdf_tex_var", sdf_tex_var));
        vars.push(("sdf_samp_var", sdf_samp_var));
    }

    let (_, body_template) = glass_template_parts(node);
    let stmt = substitute_template(&body_template, &vars);

    ctx.inline_stmts.push(stmt);
    Ok(TypedExpr::with_time(out_var, ValueType::Vec4, uses_time))
}
