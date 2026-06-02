//! Compilers for signed distance field (SDF) nodes.

use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use crate::dsl::{Node, SceneDSL, incoming_connection};
use crate::renderer::utils::coerce_to_type;

const SDF2D_WGSL_LIB_KEY: &str = "sdf2d_lib";
const SDF2D_BEVEL_WGSL_LIB_KEY: &str = "sdf2d_bevel_lib";
const SDF2D_ROUND_RECT_FN: &str = "sdf2d_round_rect";
const SDF2D_SMOOTH_ROUND_RECT_FN: &str = "sdf2d_smooth_round_rect";
const SDF2D_BEVEL_SMOOTH5_FN: &str = "sdf2d_bevel_smooth5";
const SDF2D_BEVEL_SMOOTH7_FN: &str = "sdf2d_bevel_smooth7";
const SDF2D_BEVEL_NORMAL_FN: &str = "sdf2d_bevel_normal";
const SDF2D_BEVEL_EPS_FN: &str = "sdf2d_bevel_eps";

struct Sdf2DLib {
    round_rect_fn: String,
    smooth_round_rect_fn: String,
}

struct Sdf2DBevelLib {
    bevel_smooth5_fn: String,
    bevel_smooth7_fn: String,
    bevel_normal_fn: String,
    bevel_eps_fn: String,
}

fn sanitize_id_suffix(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn override_path(node: &Node) -> Option<std::path::PathBuf> {
    node.wgsl_override
        .as_deref()
        .and_then(super::template_loader::resolve_override_path)
}

fn ensure_sdf2d_wgsl_lib(ctx: &mut MaterialCompileContext, node: &Node) -> Sdf2DLib {
    let path = override_path(node);
    let template =
        super::template_loader::load_template_with_override(path.as_deref(), "sdf2d.wgsl");

    if path.is_some() {
        let suffix = sanitize_id_suffix(&node.id);
        let round_rect_fn = format!("{SDF2D_ROUND_RECT_FN}__{suffix}");
        let smooth_round_rect_fn = format!("{SDF2D_SMOOTH_ROUND_RECT_FN}__{suffix}");
        let lib_key = format!("{SDF2D_WGSL_LIB_KEY}::{suffix}");
        let renamed = template
            .replace(SDF2D_ROUND_RECT_FN, &round_rect_fn)
            .replace(SDF2D_SMOOTH_ROUND_RECT_FN, &smooth_round_rect_fn);
        let block = format!(
            "\n// ---- 2D SDF helpers (generated, override for {}) ----\n{}",
            node.id, renamed
        );
        ctx.extra_wgsl_decls.entry(lib_key).or_insert(block);
        return Sdf2DLib {
            round_rect_fn,
            smooth_round_rect_fn,
        };
    }

    ensure_default_sdf2d_wgsl_lib(ctx);
    Sdf2DLib {
        round_rect_fn: SDF2D_ROUND_RECT_FN.to_string(),
        smooth_round_rect_fn: SDF2D_SMOOTH_ROUND_RECT_FN.to_string(),
    }
}

pub(crate) fn ensure_default_sdf2d_wgsl_lib(ctx: &mut MaterialCompileContext) {
    if ctx.extra_wgsl_decls.contains_key(SDF2D_WGSL_LIB_KEY) {
        return;
    }

    let template = super::template_loader::load_template("sdf2d.wgsl");
    let block = format!("\n// ---- 2D SDF helpers (generated) ----\n{}", template);
    ctx.extra_wgsl_decls
        .insert(SDF2D_WGSL_LIB_KEY.to_string(), block);
}

fn ensure_sdf2d_bevel_wgsl_lib(ctx: &mut MaterialCompileContext, node: &Node) -> Sdf2DBevelLib {
    let path = override_path(node);
    let template =
        super::template_loader::load_template_with_override(path.as_deref(), "sdf2d_bevel.wgsl");

    if path.is_some() {
        let suffix = sanitize_id_suffix(&node.id);
        let bevel_smooth5_fn = format!("{SDF2D_BEVEL_SMOOTH5_FN}__{suffix}");
        let bevel_smooth7_fn = format!("{SDF2D_BEVEL_SMOOTH7_FN}__{suffix}");
        let bevel_normal_fn = format!("{SDF2D_BEVEL_NORMAL_FN}__{suffix}");
        let bevel_eps_fn = format!("{SDF2D_BEVEL_EPS_FN}__{suffix}");
        let lib_key = format!("{SDF2D_BEVEL_WGSL_LIB_KEY}::{suffix}");
        let renamed = template
            .replace(SDF2D_BEVEL_SMOOTH5_FN, &bevel_smooth5_fn)
            .replace(SDF2D_BEVEL_SMOOTH7_FN, &bevel_smooth7_fn)
            .replace(SDF2D_BEVEL_EPS_FN, &bevel_eps_fn)
            .replace(SDF2D_BEVEL_NORMAL_FN, &bevel_normal_fn);
        let block = format!(
            "\n// ---- 2D SDF bevel helpers (generated, override for {}) ----\n{}",
            node.id, renamed
        );
        ctx.extra_wgsl_decls.entry(lib_key).or_insert(block);
        return Sdf2DBevelLib {
            bevel_smooth5_fn,
            bevel_smooth7_fn,
            bevel_normal_fn,
            bevel_eps_fn,
        };
    }

    ensure_default_sdf2d_bevel_wgsl_lib(ctx);
    Sdf2DBevelLib {
        bevel_smooth5_fn: SDF2D_BEVEL_SMOOTH5_FN.to_string(),
        bevel_smooth7_fn: SDF2D_BEVEL_SMOOTH7_FN.to_string(),
        bevel_normal_fn: SDF2D_BEVEL_NORMAL_FN.to_string(),
        bevel_eps_fn: SDF2D_BEVEL_EPS_FN.to_string(),
    }
}

fn ensure_default_sdf2d_bevel_wgsl_lib(ctx: &mut MaterialCompileContext) {
    if ctx.extra_wgsl_decls.contains_key(SDF2D_BEVEL_WGSL_LIB_KEY) {
        return;
    }

    let template = super::template_loader::load_template("sdf2d_bevel.wgsl");
    let block = format!(
        "\n// ---- 2D SDF bevel helpers (generated) ----\n{}",
        template
    );
    ctx.extra_wgsl_decls
        .insert(SDF2D_BEVEL_WGSL_LIB_KEY.to_string(), block);
}

fn parse_json_number_f32(v: &Value) -> Option<f32> {
    v.as_f64()
        .map(|x| x as f32)
        .or_else(|| v.as_i64().map(|x| x as f32))
        .or_else(|| v.as_u64().map(|x| x as f32))
}

fn parse_vec2_param(node: &Node, key: &str) -> Option<[f32; 2]> {
    let v = node.params.get(key)?;
    if let Some(arr) = v.as_array() {
        let x = arr.get(0).and_then(parse_json_number_f32).unwrap_or(0.0);
        let y = arr.get(1).and_then(parse_json_number_f32).unwrap_or(0.0);
        return Some([x, y]);
    }
    if let Some(obj) = v.as_object() {
        let x = obj.get("x").and_then(parse_json_number_f32).unwrap_or(0.0);
        let y = obj.get("y").and_then(parse_json_number_f32).unwrap_or(0.0);
        return Some([x, y]);
    }
    None
}

fn parse_vec4_param(node: &Node, key: &str) -> Option<[f32; 4]> {
    let v = node.params.get(key)?;
    if let Some(arr) = v.as_array() {
        let get = |i: usize, default: f32| -> f32 {
            arr.get(i)
                .and_then(parse_json_number_f32)
                .unwrap_or(default)
        };
        return Some([get(0, 0.0), get(1, 0.0), get(2, 0.0), get(3, 0.0)]);
    }
    if let Some(obj) = v.as_object() {
        let x = obj.get("x").and_then(parse_json_number_f32).unwrap_or(0.0);
        let y = obj.get("y").and_then(parse_json_number_f32).unwrap_or(0.0);
        let z = obj.get("z").and_then(parse_json_number_f32).unwrap_or(0.0);
        let w = obj.get("w").and_then(parse_json_number_f32).unwrap_or(0.0);
        return Some([x, y, z, w]);
    }
    None
}

fn resolve_input_expr_f32<F>(
    scene: &SceneDSL,
    node: &Node,
    port_id: &str,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: &F,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
        let v = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
        let from_ty = v.ty;
        return coerce_to_type(v, ValueType::F32)
            .map_err(|_| anyhow!("{}.{} must be f32, got {:?}", node.id, port_id, from_ty));
    }

    if let Some(v) = node.params.get(port_id).and_then(parse_json_number_f32) {
        return Ok(TypedExpr::new(format!("{v}"), ValueType::F32));
    }

    // Port-level defaults are expected to have been merged into node.params by normalize_scene_defaults.
    // Keep a small compatibility fallback for older scenes.
    if port_id == "radius" {
        return Ok(TypedExpr::new("0.5", ValueType::F32));
    }

    Err(anyhow!(
        "missing input '{}.{}' (no connection and no param)",
        node.id,
        port_id
    ))
}

fn resolve_input_expr_f32_or_default<F>(
    scene: &SceneDSL,
    node: &Node,
    port_id: &str,
    default_value: f32,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: &F,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
        let v = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
        let from_ty = v.ty;
        return coerce_to_type(v, ValueType::F32)
            .map_err(|_| anyhow!("{}.{} must be f32, got {:?}", node.id, port_id, from_ty));
    }

    if let Some(v) = node.params.get(port_id).and_then(parse_json_number_f32) {
        return Ok(TypedExpr::new(format!("{v}"), ValueType::F32));
    }

    Ok(TypedExpr::new(format!("{default_value}"), ValueType::F32))
}

fn resolve_input_expr_vec2<F>(
    scene: &SceneDSL,
    node: &Node,
    port_id: &str,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: &F,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
        let v = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
        let from_ty = v.ty;
        return coerce_to_type(v, ValueType::Vec2)
            .map_err(|_| anyhow!("{}.{} must be vec2, got {:?}", node.id, port_id, from_ty));
    }

    if let Some([x, y]) = parse_vec2_param(node, port_id) {
        return Ok(TypedExpr::new(format!("vec2f({x}, {y})"), ValueType::Vec2));
    }

    Err(anyhow!(
        "missing input '{}.{}' (no connection and no param)",
        node.id,
        port_id
    ))
}

fn resolve_input_expr_vec2_or_default<F>(
    scene: &SceneDSL,
    node: &Node,
    port_id: &str,
    default_expr: &str,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: &F,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
        let v = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
        let from_ty = v.ty;
        return coerce_to_type(v, ValueType::Vec2)
            .map_err(|_| anyhow!("{}.{} must be vec2, got {:?}", node.id, port_id, from_ty));
    }

    if let Some([x, y]) = parse_vec2_param(node, port_id) {
        return Ok(TypedExpr::new(format!("vec2f({x}, {y})"), ValueType::Vec2));
    }

    Ok(TypedExpr::new(default_expr.to_string(), ValueType::Vec2))
}

fn resolve_input_expr_vec4<F>(
    scene: &SceneDSL,
    node: &Node,
    port_id: &str,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: &F,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
        let v = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
        let from_ty = v.ty;
        return coerce_to_type(v, ValueType::Vec4)
            .map_err(|_| anyhow!("{}.{} must be vec4, got {:?}", node.id, port_id, from_ty));
    }

    if let Some([x, y, z, w]) = parse_vec4_param(node, port_id) {
        return Ok(TypedExpr::new(
            format!("vec4f({x}, {y}, {z}, {w})"),
            ValueType::Vec4,
        ));
    }

    Err(anyhow!(
        "missing input '{}.{}' (no connection and no param)",
        node.id,
        port_id
    ))
}

pub fn compile_sdf2d<F>(
    scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    out_port: Option<&str>,
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
    let out = out_port.unwrap_or("distance");
    if out != "distance" {
        bail!("Sdf2D unsupported output port: {out}");
    }

    let shape = node
        .params
        .get("shape")
        .and_then(|v| v.as_str())
        .unwrap_or("circle");

    // Evaluate SDF at the *current fragment* local position in pixels.
    // Use GeoFragcoord convention: origin at bottom-left of the geometry.
    //
    // Note: `gl_FragCoord` (render-target space) cannot be made instance/geometry-local
    // without knowing the per-geometry (or per-instance) inverse transform. So Sdf2D uses
    // the geometry-local contract instead.
    let frag_local_px = TypedExpr::new("in.local_px.xy".to_string(), ValueType::Vec2);

    // `position` is interpreted as the SDF shape center (in the same local pixel space).
    // If not provided, default to centered at origin.
    let center = resolve_input_expr_vec2_or_default(
        scene,
        node,
        "position",
        "vec2f(0.0, 0.0)",
        ctx,
        cache,
        &compile_fn,
    )?;

    let p = TypedExpr::with_time(
        format!("({} - {})", frag_local_px.expr, center.expr),
        ValueType::Vec2,
        center.uses_time,
    );

    match shape {
        "rectangle" => {
            let sdf_lib = ensure_sdf2d_wgsl_lib(ctx, node);
            // Rounded rectangle with per-corner radii.
            // Convention (quadrant -> radius4 component):
            // - p.x < 0 && p.y > 0 => radius4.x (left-top)
            // - p.x > 0 && p.y > 0 => radius4.y (right-top)
            // - p.x > 0 && p.y < 0 => radius4.z (right-bottom)
            // - p.x < 0 && p.y < 0 => radius4.w (left-bottom)
            // `size` is interpreted as full size; convert to half-extents.
            let size = resolve_input_expr_vec2(scene, node, "size", ctx, cache, &compile_fn)?;
            let b = TypedExpr::with_time(
                format!("({} * 0.5)", size.expr),
                ValueType::Vec2,
                size.uses_time,
            );
            let rad4 = resolve_input_expr_vec4(scene, node, "radius4", ctx, cache, &compile_fn)?;

            Ok(TypedExpr::with_time(
                format!(
                    "{}({}, {}, {})",
                    sdf_lib.round_rect_fn, p.expr, b.expr, rad4.expr
                ),
                ValueType::F32,
                p.uses_time || b.uses_time || rad4.uses_time,
            ))
        }
        "smooth_round_rect" => {
            let sdf_lib = ensure_sdf2d_wgsl_lib(ctx, node);
            let size = resolve_input_expr_vec2(scene, node, "size", ctx, cache, &compile_fn)?;
            let b = TypedExpr::with_time(
                format!("({} * 0.5)", size.expr),
                ValueType::Vec2,
                size.uses_time,
            );
            let radius = resolve_input_expr_f32(scene, node, "radius", ctx, cache, &compile_fn)?;
            let axis_mix = resolve_input_expr_vec2_or_default(
                scene,
                node,
                "axisMix",
                "vec2f(0.0, 0.0)",
                ctx,
                cache,
                &compile_fn,
            )?;

            Ok(TypedExpr::with_time(
                format!(
                    "{}(abs({}), {}, {}, {}).x",
                    sdf_lib.smooth_round_rect_fn, p.expr, b.expr, radius.expr, axis_mix.expr
                ),
                ValueType::F32,
                p.uses_time || b.uses_time || radius.uses_time || axis_mix.uses_time,
            ))
        }
        // Treat unknown values as circle for resilience.
        _ => {
            let r = resolve_input_expr_f32(scene, node, "radius", ctx, cache, &compile_fn)?;
            Ok(TypedExpr::with_time(
                format!("(length({}) - {})", p.expr, r.expr),
                ValueType::F32,
                p.uses_time || r.uses_time,
            ))
        }
    }
}

fn offset_in_local_px(expr: &str, dx: &str, dy: &str) -> String {
    let off = format!("(in.local_px.xy + vec2f({dx}, {dy}))");
    expr.replace("in.local_px.xy", &off)
}

pub fn compile_sdf2d_bevel<F>(
    scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    out_port: Option<&str>,
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
    let out = out_port.unwrap_or("depth");
    if out != "depth" && out != "normal" {
        bail!("Sdf2DBevel unsupported output port: {out}");
    }

    // `curve` is `any` in the scheme, so it is delivered as a string param.
    // WGSL cannot branch on strings; treat it as a compile-time choice.
    let curve = node
        .params
        .get("curve")
        .and_then(|v| v.as_str())
        .unwrap_or("smooth7");

    if incoming_connection(scene, &node.id, "curve").is_some() {
        bail!(
            "{}.curve cannot be connected (string/any ports are not shader-expressible); set node.params.curve to 'smooth5' or 'smooth7'",
            node.id
        );
    }

    let bevel_lib = ensure_sdf2d_bevel_wgsl_lib(ctx, node);
    let bevel_fn = match curve {
        "smooth5" => bevel_lib.bevel_smooth5_fn.as_str(),
        // Treat unknown values as smooth7 for resilience.
        _ => bevel_lib.bevel_smooth7_fn.as_str(),
    };

    // Inputs:
    // - sdfDistance: upstream SDF distance at current fragment
    // - width: bevel edge width in pixels
    // - cliff: exponent shaping near the edge
    let d0 = resolve_input_expr_f32_or_default(
        scene,
        node,
        "sdfDistance",
        0.0,
        ctx,
        cache,
        &compile_fn,
    )?;
    let width =
        resolve_input_expr_f32_or_default(scene, node, "width", 0.1, ctx, cache, &compile_fn)?;
    let cliff =
        resolve_input_expr_f32_or_default(scene, node, "cliff", 0.5, ctx, cache, &compile_fn)?;

    let depth0 = TypedExpr::with_time(
        format!("{bevel_fn}({}, {}, {})", d0.expr, width.expr, cliff.expr),
        ValueType::F32,
        d0.uses_time || width.uses_time || cliff.uses_time,
    );

    if out == "depth" {
        return Ok(depth0);
    }

    // Normal from depth finite differences in geometry-local pixel space.
    // We approximate depth(x, y) in a small neighborhood by re-evaluating the upstream distance
    // expression with `in.local_px` substituted by offset values.
    let normal_eps = format!("{}()", bevel_lib.bevel_eps_fn);
    let d_px = offset_in_local_px(&d0.expr, &normal_eps, "0.0");
    let d_nx = offset_in_local_px(&d0.expr, &format!("-({normal_eps})"), "0.0");
    let d_py = offset_in_local_px(&d0.expr, "0.0", &normal_eps);
    let d_ny = offset_in_local_px(&d0.expr, "0.0", &format!("-({normal_eps})"));

    let depth_px = format!("{bevel_fn}({d_px}, {}, {})", width.expr, cliff.expr);
    let depth_nx = format!("{bevel_fn}({d_nx}, {}, {})", width.expr, cliff.expr);
    let depth_py = format!("{bevel_fn}({d_py}, {}, {})", width.expr, cliff.expr);
    let depth_ny = format!("{bevel_fn}({d_ny}, {}, {})", width.expr, cliff.expr);

    let n = format!(
        "{}({}, {}, {}, {}, {})",
        bevel_lib.bevel_normal_fn, depth_px, depth_nx, depth_py, depth_ny, normal_eps
    );

    Ok(TypedExpr::with_time(
        n,
        ValueType::Vec3,
        d0.uses_time || width.uses_time || cliff.uses_time,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::node_compiler::test_utils::test_scene;

    #[test]
    fn sdf2d_circle_from_params() {
        let node = Node {
            id: "sdf".to_string(),
            node_type: "Sdf2D".to_string(),
            params: HashMap::from([
                ("shape".to_string(), serde_json::json!("circle")),
                ("position".to_string(), serde_json::json!([1.0, 0.0])),
                ("radius".to_string(), serde_json::json!(2.0)),
            ]),
            inputs: vec![],
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };

        let scene = test_scene(vec![node.clone()], vec![]);
        let nodes_by_id = HashMap::from([(node.id.clone(), node)]);
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        // Compile node output directly.
        let expr = crate::renderer::node_compiler::compile_material_expr(
            &scene,
            &nodes_by_id,
            "sdf",
            Some("distance"),
            &mut ctx,
            &mut cache,
        )
        .unwrap();

        assert_eq!(expr.ty, ValueType::F32);
        assert!(expr.expr.contains("in.local_px"));
        assert!(expr.expr.contains("length"));
        assert!(expr.expr.contains("-"));
    }

    #[test]
    fn sdf2d_rect_emits_helper() {
        let node = Node {
            id: "sdf".to_string(),
            node_type: "Sdf2D".to_string(),
            params: HashMap::from([
                ("shape".to_string(), serde_json::json!("rectangle")),
                ("position".to_string(), serde_json::json!([1.0, 2.0])),
                ("size".to_string(), serde_json::json!([10.0, 20.0])),
                (
                    "radius4".to_string(),
                    serde_json::json!([1.0, 2.0, 3.0, 4.0]),
                ),
            ]),
            inputs: vec![],
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };

        let scene = test_scene(vec![node.clone()], vec![]);
        let nodes_by_id = HashMap::from([(node.id.clone(), node)]);
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let expr = crate::renderer::node_compiler::compile_material_expr(
            &scene,
            &nodes_by_id,
            "sdf",
            Some("distance"),
            &mut ctx,
            &mut cache,
        )
        .unwrap();

        assert_eq!(expr.ty, ValueType::F32);
        assert!(expr.expr.contains("in.local_px"));
        assert!(expr.expr.contains("sdf2d_round_rect"));
        let lib = ctx.extra_wgsl_decls.get(SDF2D_WGSL_LIB_KEY).unwrap();
        assert!(lib.contains("fn sdf2d_round_rect"));
    }

    #[test]
    fn sdf2d_smooth_round_rect_emits_helper() {
        let node = Node {
            id: "sdf".to_string(),
            node_type: "Sdf2D".to_string(),
            params: HashMap::from([
                ("shape".to_string(), serde_json::json!("smooth_round_rect")),
                ("position".to_string(), serde_json::json!([1.0, 2.0])),
                ("size".to_string(), serde_json::json!([10.0, 20.0])),
                ("radius".to_string(), serde_json::json!(3.0)),
                ("axisMix".to_string(), serde_json::json!([0.25, 0.75])),
            ]),
            inputs: vec![],
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };

        let scene = test_scene(vec![node.clone()], vec![]);
        let nodes_by_id = HashMap::from([(node.id.clone(), node)]);
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let expr = crate::renderer::node_compiler::compile_material_expr(
            &scene,
            &nodes_by_id,
            "sdf",
            Some("distance"),
            &mut ctx,
            &mut cache,
        )
        .unwrap();

        assert_eq!(expr.ty, ValueType::F32);
        assert!(expr.expr.contains("sdf2d_smooth_round_rect"));
        assert!(expr.expr.contains(".x"));
        let lib = ctx.extra_wgsl_decls.get(SDF2D_WGSL_LIB_KEY).unwrap();
        assert!(lib.contains("fn sdf2d_smooth_round_rect"));
    }

    #[test]
    fn sdf2d_bevel_depth_emits_helper() {
        let node = Node {
            id: "bev".to_string(),
            node_type: "Sdf2DBevel".to_string(),
            params: HashMap::from([
                ("curve".to_string(), serde_json::json!("smooth7")),
                ("sdfDistance".to_string(), serde_json::json!(-0.25)),
                ("width".to_string(), serde_json::json!(4.0)),
                ("cliff".to_string(), serde_json::json!(0.8)),
            ]),
            inputs: vec![],
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };

        let scene = test_scene(vec![node.clone()], vec![]);
        let nodes_by_id = HashMap::from([(node.id.clone(), node)]);
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let expr = crate::renderer::node_compiler::compile_material_expr(
            &scene,
            &nodes_by_id,
            "bev",
            Some("depth"),
            &mut ctx,
            &mut cache,
        )
        .unwrap();

        assert_eq!(expr.ty, ValueType::F32);
        assert!(expr.expr.contains("sdf2d_bevel_smooth7"));
        let lib = ctx.extra_wgsl_decls.get(SDF2D_BEVEL_WGSL_LIB_KEY).unwrap();
        assert!(lib.contains("fn sdf2d_bevel_smooth7"));
    }

    #[test]
    fn sdf2d_bevel_normal_is_vec3() {
        let node = Node {
            id: "bev".to_string(),
            node_type: "Sdf2DBevel".to_string(),
            params: HashMap::from([
                ("curve".to_string(), serde_json::json!("smooth5")),
                ("sdfDistance".to_string(), serde_json::json!(-0.25)),
            ]),
            inputs: vec![],
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };

        let scene = test_scene(vec![node.clone()], vec![]);
        let nodes_by_id = HashMap::from([(node.id.clone(), node)]);
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let expr = crate::renderer::node_compiler::compile_material_expr(
            &scene,
            &nodes_by_id,
            "bev",
            Some("normal"),
            &mut ctx,
            &mut cache,
        )
        .unwrap();

        assert_eq!(expr.ty, ValueType::Vec3);
        assert!(expr.expr.contains("sdf2d_bevel_normal"));
        assert!(expr.expr.contains("sdf2d_bevel_smooth5"));
        let lib = ctx.extra_wgsl_decls.get(SDF2D_BEVEL_WGSL_LIB_KEY).unwrap();
        assert!(lib.contains("fn sdf2d_bevel_smooth5"));
        assert!(lib.contains("fn sdf2d_bevel_normal"));
    }
}
