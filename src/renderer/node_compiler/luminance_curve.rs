//! LuminanceCurve color node.
//!
//! Applies a cubic Bezier curve to a color's luminance, in either LAB or RGB
//! space. The curve is parametrised by four normalized y-values sampled at
//! x = 0, 1/3, 2/3, 1 (wire format: `vec4`).
//!
//! The two helpers (`lc_luminance_curve_lab`, `lc_luminance_curve_rgb`) are
//! semantically identical to the inline curve currently embedded inside
//! `GlassMaterial` (`glass_luminance_curve_lab` / `glass_luminance_curve`).
//! They are renamed and registered under their own WGSL lib key so the two
//! libs can coexist in a single shader without symbol collisions.
//!
//! Each helper lives in its own template file under `templates/` and is loaded
//! lazily on demand — only the variant referenced by the compiled graph gets
//! emitted into the final WGSL.

use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use super::super::utils::{fmt_f32, to_vec4_color};
use crate::dsl::{Node, SceneDSL, incoming_connection};

const LUMINANCE_CURVE_LAB_LIB_KEY: &str = "luminance_curve_lab_lib";
const LUMINANCE_CURVE_RGB_LIB_KEY: &str = "luminance_curve_rgb_lib";
const LUMINANCE_CURVE_LAB_FN: &str = "lc_luminance_curve_lab";
const LUMINANCE_CURVE_RGB_FN: &str = "lc_luminance_curve_rgb";

/// Sanitize a node ID into a WGSL identifier suffix.
fn sanitize_id_suffix(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Resolve override absolute path for a node, if any.
fn override_path(node: &Node) -> Option<std::path::PathBuf> {
    node.wgsl_override
        .as_deref()
        .and_then(super::template_loader::resolve_override_path)
}

/// Returns (lib_key, fn_name, decl_block) for a per-node LuminanceCurve helper.
///
/// When an override is in play we suffix both the lib key and the function name
/// with the node id so two nodes with different override contents can coexist
/// in the same compiled shader without symbol collision.
fn build_lc_lib(
    node: &Node,
    template_name: &str,
    base_fn_name: &str,
    base_lib_key: &str,
    label: &str,
) -> (String, String, String) {
    let path = override_path(node);
    let template =
        super::template_loader::load_template_with_override(path.as_deref(), template_name);

    if path.is_some() {
        let suffix = sanitize_id_suffix(&node.id);
        let fn_name = format!("{base_fn_name}__{suffix}");
        let lib_key = format!("{base_lib_key}::{suffix}");
        let renamed = template.replace(base_fn_name, &fn_name);
        let block = format!(
            "\n// ---- LuminanceCurve {label} helper (generated, override for {}) ----\n{}",
            node.id, renamed
        );
        (lib_key, fn_name, block)
    } else {
        let block = format!(
            "\n// ---- LuminanceCurve {label} helper (generated) ----\n{}",
            template
        );
        (base_lib_key.to_string(), base_fn_name.to_string(), block)
    }
}

fn ensure_luminance_curve_lab_lib(ctx: &mut MaterialCompileContext, node: &Node) -> String {
    let (lib_key, fn_name, block) = build_lc_lib(
        node,
        "luminance_curve_lab.wgsl",
        LUMINANCE_CURVE_LAB_FN,
        LUMINANCE_CURVE_LAB_LIB_KEY,
        "LAB",
    );
    ctx.extra_wgsl_decls.entry(lib_key).or_insert(block);
    fn_name
}

fn ensure_luminance_curve_rgb_lib(ctx: &mut MaterialCompileContext, node: &Node) -> String {
    let (lib_key, fn_name, block) = build_lc_lib(
        node,
        "luminance_curve_rgb.wgsl",
        LUMINANCE_CURVE_RGB_FN,
        LUMINANCE_CURVE_RGB_LIB_KEY,
        "RGB",
    );
    ctx.extra_wgsl_decls.entry(lib_key).or_insert(block);
    fn_name
}

fn parse_param_vec4(node: &Node, key: &str, default: [f32; 4]) -> [f32; 4] {
    let Some(v) = node.params.get(key) else {
        return default;
    };

    if let Some(arr) = v.as_array() {
        if arr.len() >= 4 {
            let read = |i: usize| arr[i].as_f64().unwrap_or(default[i] as f64) as f32;
            return [read(0), read(1), read(2), read(3)];
        }
    }

    if let Some(obj) = v.as_object() {
        let read = |key: &str, fallback: f32| -> f32 {
            obj.get(key)
                .and_then(|x| x.as_f64())
                .unwrap_or(fallback as f64) as f32
        };
        let has_xyzw = ["x", "y", "z", "w"].iter().any(|k| obj.contains_key(*k));
        if has_xyzw {
            return [
                read("x", default[0]),
                read("y", default[1]),
                read("z", default[2]),
                read("w", default[3]),
            ];
        }
    }

    default
}

fn parse_param_f32(node: &Node, key: &str, default: f32) -> f32 {
    node.params
        .get(key)
        .and_then(|v| v.as_f64())
        .map(|x| x as f32)
        .unwrap_or(default)
}

fn vec4_literal(v: [f32; 4]) -> String {
    format!(
        "vec4f({}, {}, {}, {})",
        fmt_f32(v[0]),
        fmt_f32(v[1]),
        fmt_f32(v[2]),
        fmt_f32(v[3])
    )
}

fn expect_ty(expr: &TypedExpr, expected: ValueType, name: &str) -> Result<()> {
    if expr.ty != expected {
        bail!(
            "LuminanceCurve.{name} expected {:?}, got {:?}",
            expected,
            expr.ty
        );
    }
    Ok(())
}

pub fn compile_luminance_curve<F>(
    scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
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
    let color_conn = incoming_connection(scene, &node.id, "color")
        .ok_or_else(|| anyhow!("LuminanceCurve missing input color"))?;
    let color = compile_fn(
        &color_conn.from.node_id,
        Some(&color_conn.from.port_id),
        ctx,
        cache,
    )?;
    let color_vec4 = to_vec4_color(color);

    // values: vec4 (normalizedBezierCurve wire-compatible with vec4).
    let values = if let Some(conn) = incoming_connection(scene, &node.id, "values") {
        let expr = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
        expect_ty(&expr, ValueType::Vec4, "values")?;
        expr
    } else {
        let v = parse_param_vec4(node, "values", [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]);
        TypedExpr::new(vec4_literal(v), ValueType::Vec4)
    };

    let amount = if let Some(conn) = incoming_connection(scene, &node.id, "amount") {
        let expr = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
        expect_ty(&expr, ValueType::F32, "amount")?;
        expr
    } else {
        let v = parse_param_f32(node, "amount", 1.0);
        TypedExpr::new(fmt_f32(v), ValueType::F32)
    };

    let mode = node
        .params
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("lab");
    let fn_name = match mode {
        "rgb" => ensure_luminance_curve_rgb_lib(ctx, node),
        // Default and "lab" both map to the LAB variant.
        _ => ensure_luminance_curve_lab_lib(ctx, node),
    };

    let uses_time = color_vec4.uses_time || values.uses_time || amount.uses_time;
    Ok(TypedExpr::with_time(
        format!(
            "{}({}, {}, {})",
            fn_name, color_vec4.expr, values.expr, amount.expr
        ),
        ValueType::Vec4,
        uses_time,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::node_compiler::test_utils::{test_connection, test_scene};

    fn mock_color_compile_fn(
        _node_id: &str,
        _out_port: Option<&str>,
        _ctx: &mut MaterialCompileContext,
        _cache: &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr> {
        Ok(TypedExpr::new(
            "vec4f(0.25, 0.5, 0.75, 1.0)".to_string(),
            ValueType::Vec4,
        ))
    }

    #[test]
    fn rgb_curve_accepts_values_above_one_and_keeps_hdr_gain() {
        let scene = test_scene(
            Vec::new(),
            vec![test_connection("color", "value", "curve", "color")],
        );
        let node = Node {
            id: "curve".to_string(),
            node_type: "LuminanceCurve".to_string(),
            params: HashMap::from([
                ("mode".to_string(), serde_json::json!("rgb")),
                (
                    "values".to_string(),
                    serde_json::json!([0.0, 0.5, 1.25, 2.0]),
                ),
            ]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_luminance_curve(
            &scene,
            &HashMap::new(),
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_color_compile_fn,
        )
        .unwrap();

        assert!(result.expr.contains("lc_luminance_curve_rgb("));
        assert!(result.expr.contains("vec4f(0.0, 0.5, 1.25, 2.0)"));

        let lib = ctx
            .extra_wgsl_decls
            .get(LUMINANCE_CURVE_RGB_LIB_KEY)
            .unwrap();
        assert!(
            lib.contains("let chroma_scale = max(target_luminance / max(luminance, 1e-6), 0.0);")
        );
        assert!(!lib.contains("target_luminance / max(luminance, 1e-6), 0.0, 1.0"));
    }
}
