//! Compilers for signed distance field (SDF) nodes.

use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use crate::dsl::{Node, SceneDSL, incoming_connection};
use crate::renderer::utils::coerce_to_type;

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
    let frag_local_px = TypedExpr::new("in.local_px".to_string(), ValueType::Vec2);

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

            ctx.extra_wgsl_decls
                .entry("sdf2d_round_rect".to_string())
                .or_insert_with(|| {
                    r#"fn sdf2d_round_rect(p: vec2f, b: vec2f, rad4: vec4f) -> f32 {
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
"#
                    .to_string()
                });

            Ok(TypedExpr::with_time(
                format!("sdf2d_round_rect({}, {}, {})", p.expr, b.expr, rad4.expr),
                ValueType::F32,
                p.uses_time || b.uses_time || rad4.uses_time,
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
        assert!(ctx.extra_wgsl_decls.contains_key("sdf2d_round_rect"));
    }
}
