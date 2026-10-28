//! Compilers for math operation nodes (Abs, MathAdd, MathSubtract, MathMultiply, MathDivide, Lerp, MathClamp, MathMax, MathPower).

use anyhow::Result;
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use super::super::utils::{coerce_for_binary, coerce_to_type};
use crate::dsl::{Node, SceneDSL, incoming_connection};

fn wgsl_f32_literal(v: f32) -> String {
    if v.is_finite() {
        if v.fract() == 0.0 {
            format!("{v:.1}")
        } else {
            v.to_string()
        }
    } else {
        "0.0".to_string()
    }
}

pub(super) fn inline_numeric_expr(node: &Node, port: &str, dpi: f64) -> Result<Option<TypedExpr>> {
    let Some(value) = node.params.get(port) else { return Ok(None) };
    let value = crate::input_units::inline_value(node, port, value, dpi)?;
    if let Some(value) = value.as_f64() {
        return Ok(Some(TypedExpr::new(wgsl_f32_literal(value as f32), ValueType::F32)));
    }
    if let Some(values) = value.as_array() {
        let ty = match values.len() { 2 => ValueType::Vec2, 3 => ValueType::Vec3, 4 => ValueType::Vec4,
            _ => anyhow::bail!("invalid inline math vector {}.{port}", node.id) };
        let parts = values.iter().map(|value| value.as_f64().map(|v| wgsl_f32_literal(v as f32))
            .ok_or_else(|| anyhow::anyhow!("invalid inline math value {}.{port}", node.id))).collect::<Result<Vec<_>>>()?;
        return Ok(Some(TypedExpr::new(format!("vec{}f({})", values.len(), parts.join(", ")), ty)));
    }
    anyhow::bail!("inline math input {}.{port} must be numeric", node.id)
}

/// Compile component-wise absolute value, preserving vector dimensions.
pub fn compile_math_abs<F>(
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
    let x = if let Some(conn) = incoming_connection(scene, &node.id, "x") {
        compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?
    } else {
        inline_numeric_expr(node, "x", scene.dpi)?
            .unwrap_or_else(|| TypedExpr::new("0.0", ValueType::F32))
    };
    // Generic math ports infer scalar inputs as float, including int and bool.
    let x = match x.ty {
        ValueType::F32 | ValueType::Vec2 | ValueType::Vec3 | ValueType::Vec4 => x,
        ValueType::I32 | ValueType::Bool => coerce_to_type(x, ValueType::F32)?,
        ValueType::U32 => TypedExpr::with_time(
            format!("f32({})", x.expr), ValueType::F32, x.uses_time,
        ),
        _ => anyhow::bail!("Abs '{}'.x requires a scalar or vector, got {:?}", node.id, x.ty),
    };
    Ok(TypedExpr::with_time(format!("abs({})", x.expr), x.ty, x.uses_time))
}

/// Compile a MathAdd node to WGSL.
///
/// MathAdd nodes add N values together.
///
/// The editor exports dynamic input ports in `node.inputs` (e.g. `dynamic_<ts>_<index>`).
/// Falls back to fixed `a`/`b` ports when `node.inputs` is empty.
pub fn compile_math_add<F>(
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
    let input_ports: Vec<&str> = if node.inputs.is_empty() {
        vec!["a", "b"]
    } else {
        node.inputs.iter().map(|p| p.id.as_str()).collect()
    };

    let mut resolved: Vec<TypedExpr> = Vec::new();
    for port_id in input_ports {
        if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
            let expr = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
            resolved.push(expr);
        } else if let Some(expr) = inline_numeric_expr(node, port_id, scene.dpi)? {
            resolved.push(expr);
        }
    }

    if resolved.len() < 2 {
        return Ok(TypedExpr::new("0.0", ValueType::F32));
    }

    let mut it = resolved.into_iter();
    let first = it.next().expect("len >= 2");

    let mut acc_expr = first;
    for next in it {
        let (a, b, ty) = coerce_for_binary(acc_expr, next)?;
        acc_expr = TypedExpr::with_time(
            format!("({} + {})", a.expr, b.expr),
            ty,
            a.uses_time || b.uses_time,
        );
    }

    Ok(acc_expr)
}

/// Compile a MathSubtract node to WGSL.
///
/// MathSubtract nodes subtract each subsequent connected input from the first.
///
/// The editor exports dynamic input ports in `node.inputs` (e.g. `dynamic_<ts>_<index>`).
/// Falls back to fixed `a`/`b` ports when `node.inputs` is empty.
pub fn compile_math_subtract<F>(
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
    let input_ports: Vec<&str> = if node.inputs.is_empty() {
        vec!["a", "b"]
    } else {
        node.inputs.iter().map(|p| p.id.as_str()).collect()
    };

    let mut resolved: Vec<TypedExpr> = Vec::new();
    for port_id in input_ports {
        if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
            let expr = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
            resolved.push(expr);
        } else if let Some(expr) = inline_numeric_expr(node, port_id, scene.dpi)? {
            resolved.push(expr);
        }
    }

    if resolved.len() < 2 {
        return Ok(TypedExpr::new("0.0", ValueType::F32));
    }

    let mut it = resolved.into_iter();
    let first = it.next().expect("len >= 2");

    let mut acc_expr = first;
    for next in it {
        let (a, b, ty) = coerce_for_binary(acc_expr, next)?;
        acc_expr = TypedExpr::with_time(
            format!("({} - {})", a.expr, b.expr),
            ty,
            a.uses_time || b.uses_time,
        );
    }

    Ok(acc_expr)
}

/// Compile a MathMultiply node to WGSL.
///
/// MathMultiply nodes multiply N values together.
///
/// The editor exports dynamic input ports in `node.inputs` (e.g. `dynamic_<ts>_<index>`).
/// The render server should not assume fixed ports like `a`/`b`.
pub fn compile_math_multiply<F>(
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
    let input_ports: Vec<&str> = if node.inputs.is_empty() {
        vec!["a", "b"]
    } else {
        node.inputs.iter().map(|p| p.id.as_str()).collect()
    };

    // Resolve all connected inputs and fold them with `*`.
    let mut resolved: Vec<TypedExpr> = Vec::new();
    for port_id in input_ports {
        if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
            let expr = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
            resolved.push(expr);
        } else if let Some(expr) = inline_numeric_expr(node, port_id, scene.dpi)? {
            resolved.push(expr);
        }
    }

    // The editor allows creating the node before connecting inputs; treat <2 inputs as unknown.
    if resolved.len() < 2 {
        // Not enough information to infer a type.
        // Emit a dummy expression; downstream compilers should treat this as unknown.
        return Ok(TypedExpr::new("0.0", ValueType::F32));
    }

    let mut it = resolved.into_iter();
    let first = it.next().expect("len >= 2");

    let mut acc_expr = first;
    for next in it {
        let (a, b, ty) = coerce_for_binary(acc_expr, next)?;
        acc_expr = TypedExpr::with_time(
            format!("({} * {})", a.expr, b.expr),
            ty,
            a.uses_time || b.uses_time,
        );
    }

    Ok(acc_expr)
}

/// Compile a MathDivide node to WGSL.
///
/// MathDivide nodes divide the first connected input by each subsequent connected input.
///
/// The editor exports dynamic input ports in `node.inputs` (e.g. `dynamic_<ts>_<index>`).
/// The render server should not assume fixed ports like `a`/`b`.
pub fn compile_math_divide<F>(
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
    let input_ports: Vec<&str> = if node.inputs.is_empty() {
        vec!["a", "b"]
    } else {
        node.inputs.iter().map(|p| p.id.as_str()).collect()
    };

    let mut resolved: Vec<TypedExpr> = Vec::new();
    for port_id in input_ports {
        if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
            let expr = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
            resolved.push(expr);
        } else if let Some(expr) = inline_numeric_expr(node, port_id, scene.dpi)? {
            resolved.push(expr);
        }
    }

    if resolved.len() < 2 {
        return Ok(TypedExpr::new("0.0", ValueType::F32));
    }

    let mut it = resolved.into_iter();
    let first = it.next().expect("len >= 2");

    let mut acc_expr = first;
    for next in it {
        let (a, b, ty) = coerce_for_binary(acc_expr, next)?;
        acc_expr = TypedExpr::with_time(
            format!("({} / {})", a.expr, b.expr),
            ty,
            a.uses_time || b.uses_time,
        );
    }

    Ok(acc_expr)
}

/// Compile a Lerp node to WGSL.
///
/// Lerp interpolates between A and B by scalar T. A and B are coerced with the
/// same rules as binary math nodes; T is coerced to f32 and splatted for vector
/// outputs to satisfy WGSL `mix` overloads.
pub fn compile_lerp<F>(
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
    let port = out_port.unwrap_or("result");
    if port != "result" {
        anyhow::bail!("Lerp: unsupported output port '{port}'");
    }

    let mut resolve = |port: &str| -> Result<Option<TypedExpr>> {
        if let Some(conn) = incoming_connection(scene, &node.id, port) {
            Ok(Some(compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?))
        } else {
            inline_numeric_expr(node, port, scene.dpi)
        }
    };
    let (Some(a), Some(b)) = (resolve("a")?, resolve("b")?) else {
        return Ok(TypedExpr::new("0.0", ValueType::F32));
    };
    let (a, b, ty) = coerce_for_binary(a, b)?;

    let t = if let Some(t_conn) = incoming_connection(scene, &node.id, "t") {
        let t_expr = compile_fn(&t_conn.from.node_id, Some(&t_conn.from.port_id), ctx, cache)?;
        coerce_to_type(t_expr, ValueType::F32)?
    } else {
        coerce_to_type(
            inline_numeric_expr(node, "t", scene.dpi)?
                .unwrap_or_else(|| TypedExpr::new("0.5", ValueType::F32)),
            ValueType::F32,
        )?
    };

    let t = if matches!(ty, ValueType::Vec2 | ValueType::Vec3 | ValueType::Vec4) {
        coerce_to_type(t, ty)?
    } else {
        t
    };

    Ok(TypedExpr::with_time(
        format!("mix({}, {}, {})", a.expr, b.expr, t.expr),
        ty,
        a.uses_time || b.uses_time || t.uses_time,
    ))
}

/// Compile a MathClamp node to WGSL.
///
/// MathClamp nodes clamp a value between minimum and maximum bounds.
///
/// The editor exports dynamic input ports in `node.inputs`.
/// Falls back to fixed `value`/`min`/`max` ports when `node.inputs` is empty.
/// The first three connected inputs are treated as (value, min, max).
pub fn compile_math_clamp<F>(
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
    let input_ports: Vec<&str> = if node.inputs.is_empty() {
        vec!["value", "min", "max"]
    } else {
        node.inputs.iter().map(|p| p.id.as_str()).collect()
    };

    let mut resolved: Vec<TypedExpr> = Vec::new();
    for port_id in &input_ports {
        if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
            let expr = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
            resolved.push(expr);
        } else if let Some(expr) = inline_numeric_expr(node, port_id, scene.dpi)? {
            resolved.push(expr);
        }
    }

    if resolved.len() < 3 {
        return Ok(TypedExpr::new("0.0", ValueType::F32));
    }

    let mut it = resolved.into_iter();
    let x = it.next().unwrap();
    let min_val = it.next().unwrap();
    let max_val = it.next().unwrap();

    let (x, min_val, _ty) = coerce_for_binary(x, min_val)?;
    let (min_val, max_val, ty) = coerce_for_binary(min_val, max_val)?;
    let (x, _, ty) = coerce_for_binary(x, TypedExpr::with_time("".to_string(), ty, false))?;

    Ok(TypedExpr::with_time(
        format!("clamp({}, {}, {})", x.expr, min_val.expr, max_val.expr),
        ty,
        x.uses_time || min_val.uses_time || max_val.uses_time,
    ))
}

/// Compile a MathMax node to WGSL.
///
/// MathMax nodes return the larger of two values.
pub fn compile_math_max<F>(
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
    let mut resolve = |port: &str| -> Result<Option<TypedExpr>> {
        if let Some(conn) = incoming_connection(scene, &node.id, port) {
            Ok(Some(compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?))
        } else {
            inline_numeric_expr(node, port, scene.dpi)
        }
    };
    let (Some(a), Some(b)) = (resolve("a")?, resolve("b")?) else {
        return Ok(TypedExpr::new("0.0", ValueType::F32));
    };
    let (a, b, ty) = coerce_for_binary(a, b)?;

    Ok(TypedExpr::with_time(
        format!("max({}, {})", a.expr, b.expr),
        ty,
        a.uses_time || b.uses_time,
    ))
}

/// Compile a MathPower node to WGSL.
///
/// MathPower nodes raise a base to an exponent.
///
/// The editor exports dynamic input ports in `node.inputs` (e.g. `dynamic_<ts>_<index>`).
/// Falls back to fixed `base`/`exponent` ports when `node.inputs` is empty.
pub fn compile_math_power<F>(
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
    let input_ports: Vec<&str> = if node.inputs.is_empty() {
        vec!["base", "exponent"]
    } else {
        node.inputs.iter().map(|p| p.id.as_str()).collect()
    };

    let mut resolved: Vec<TypedExpr> = Vec::new();
    for port_id in input_ports {
        if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
            let expr = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
            resolved.push(expr);
        } else if let Some(expr) = inline_numeric_expr(node, port_id, scene.dpi)? {
            resolved.push(expr);
        }
    }

    if resolved.len() < 2 {
        return Ok(TypedExpr::new("0.0", ValueType::F32));
    }

    let mut it = resolved.into_iter();
    let first = it.next().expect("len >= 2");

    let mut acc_expr = first;
    for next in it {
        let (a, b, ty) = coerce_for_binary(acc_expr, next)?;
        acc_expr = TypedExpr::with_time(
            format!("pow({}, {})", a.expr, b.expr),
            ty,
            a.uses_time || b.uses_time,
        );
    }

    Ok(acc_expr)
}

#[cfg(test)]
mod tests {
    use super::super::super::types::ValueType;
    use super::*;
    use crate::dsl::{Connection, Endpoint, Metadata, SceneDSL};
    use anyhow::bail;

    fn abs_node(params: serde_json::Value) -> Node {
        serde_json::from_value(serde_json::json!({
            "id": "abs", "type": "Abs", "params": params,
        })).unwrap()
    }

    fn validate_abs_wgsl(expr: &TypedExpr) {
        let source = format!("fn magnitude() -> {} {{ return {}; }}", expr.ty.wgsl(), expr.expr);
        let module = naga::front::wgsl::parse_str(&source).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        ).validate(&module).unwrap();
    }

    #[test]
    fn abs_inline_scalar_vectors_and_default_compile_through_dispatch() {
        use serde_json::json;
        for (params, expected, ty) in [
            (json!({}), "abs(0.0)", ValueType::F32),
            (json!({"x": -2.5}), "abs(-2.5)", ValueType::F32),
            (json!({"x": [-1, 2]}), "abs(vec2f(-1.0, 2.0))", ValueType::Vec2),
            (json!({"x": [-1, 2, -3]}), "abs(vec3f(-1.0, 2.0, -3.0))", ValueType::Vec3),
            (json!({"x": [-1, 2, -3, 4]}), "abs(vec4f(-1.0, 2.0, -3.0, 4.0))", ValueType::Vec4),
            (json!({"x": -2, "inputUnits": {"x": "dp"}}), "abs(-6.0)", ValueType::F32),
        ] {
            let node = abs_node(params);
            let scene = create_test_scene_with_connections(vec![node.clone()], vec![]);
            let result = super::super::compile_material_expr(
                &scene, &HashMap::from([(node.id.clone(), node)]), "abs", Some("result"),
                &mut MaterialCompileContext::default(), &mut HashMap::new(),
            ).unwrap();
            assert_eq!(result.ty, ty);
            assert_eq!(result.expr, expected);
            assert!(!result.uses_time);
            validate_abs_wgsl(&result);
        }
    }

    #[test]
    fn abs_connected_input_wins_and_preserves_time_with_numeric_coercion() {
        let node = abs_node(serde_json::json!({"x": "ignored invalid inline value"}));
        let scene = create_test_scene_with_connections(vec![node.clone()], vec![
            super::super::test_utils::test_connection("source", "value", "abs", "x"),
        ]);
        for (value, input_type, output_type) in [
            ("-2.5", ValueType::F32, ValueType::F32),
            ("-2", ValueType::I32, ValueType::F32),
            ("2u", ValueType::U32, ValueType::F32),
            ("true", ValueType::Bool, ValueType::F32),
            ("vec2f(-1.0, 2.0)", ValueType::Vec2, ValueType::Vec2),
            ("vec3f(-1.0, 2.0, -3.0)", ValueType::Vec3, ValueType::Vec3),
            ("vec4f(-1.0)", ValueType::Vec4, ValueType::Vec4),
        ] {
            let result = compile_math_abs(
                &scene, &HashMap::new(), &node, Some("result"),
                &mut MaterialCompileContext::default(), &mut HashMap::new(),
                |id, port, _, _| {
                    assert_eq!((id, port), ("source", Some("value")));
                    Ok(TypedExpr::with_time(value, input_type, true))
                },
            ).unwrap();
            assert_eq!(result.ty, output_type);
            assert!(result.uses_time);
            validate_abs_wgsl(&result);
        }
    }

    #[test]
    fn abs_rejects_non_numeric_connected_values() {
        let node = abs_node(serde_json::json!({}));
        let scene = create_test_scene_with_connections(vec![node.clone()], vec![
            super::super::test_utils::test_connection("source", "value", "abs", "x"),
        ]);
        for ty in [ValueType::Texture2D, ValueType::Mat4, ValueType::F32Array(2)] {
            let result = compile_math_abs(
                &scene, &HashMap::new(), &node, Some("result"),
                &mut MaterialCompileContext::default(), &mut HashMap::new(),
                |_, _, _, _| Ok(TypedExpr::new("unsupported", ty)),
            );
            assert!(result.unwrap_err().to_string().contains("requires a scalar or vector"));
        }
    }

    fn create_test_scene_with_connections(
        nodes: Vec<Node>,
        connections: Vec<Connection>,
    ) -> SceneDSL {
        SceneDSL {
            working_color_space: crate::color::WorkingColorSpace::Srgb,
            dpi: 3.0,
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "test".to_string(),
                created: None,
                modified: None,
            },
            nodes,
            connections,
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        }
    }

    fn mock_compile_fn(
        node_id: &str,
        _port: Option<&str>,
        _ctx: &mut MaterialCompileContext,
        _cache: &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr> {
        // Simple mock that returns f32 literals based on node_id
        match node_id {
            "a" => Ok(TypedExpr::new("2.0", ValueType::F32)),
            "b" => Ok(TypedExpr::new("3.0", ValueType::F32)),
            "min" => Ok(TypedExpr::new("0.0", ValueType::F32)),
            "max" => Ok(TypedExpr::new("1.0", ValueType::F32)),
            "value" => Ok(TypedExpr::new("0.5", ValueType::F32)),
            "base" => Ok(TypedExpr::new("2.0", ValueType::F32)),
            "exp" => Ok(TypedExpr::new("3.0", ValueType::F32)),
            "t" => Ok(TypedExpr::new("0.25", ValueType::F32)),
            "color_a" => Ok(TypedExpr::new("vec4f(1.0, 0.0, 0.0, 1.0)", ValueType::Vec4)),
            "color_b" => Ok(TypedExpr::new("vec4f(0.0, 0.0, 1.0, 1.0)", ValueType::Vec4)),
            _ => bail!("unknown node"),
        }
    }

    #[test]
    fn test_math_add() {
        let nodes = vec![
            Node {
                id: "add".to_string(),
                node_type: "MathAdd".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "a".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "b".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
        ];

        let connections = vec![
            Connection {
                id: "c1".to_string(),
                from: Endpoint {
                    node_id: "a".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "add".to_string(),
                    port_id: "a".to_string(),
                },
            },
            Connection {
                id: "c2".to_string(),
                from: Endpoint {
                    node_id: "b".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "add".to_string(),
                    port_id: "b".to_string(),
                },
            },
        ];

        let scene = create_test_scene_with_connections(nodes.clone(), connections);
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

        let add_node = nodes_by_id.get("add").unwrap();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_math_add(
            &scene,
            &nodes_by_id,
            add_node,
            None,
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, "(2.0 + 3.0)");
    }

    #[test]
    fn test_math_subtract() {
        let nodes = vec![
            Node {
                id: "sub".to_string(),
                node_type: "MathSubtract".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "a".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "b".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
        ];

        let connections = vec![
            Connection {
                id: "c1".to_string(),
                from: Endpoint {
                    node_id: "a".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "sub".to_string(),
                    port_id: "a".to_string(),
                },
            },
            Connection {
                id: "c2".to_string(),
                from: Endpoint {
                    node_id: "b".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "sub".to_string(),
                    port_id: "b".to_string(),
                },
            },
        ];

        let scene = create_test_scene_with_connections(nodes.clone(), connections);
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

        let sub_node = nodes_by_id.get("sub").unwrap();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_math_subtract(
            &scene,
            &nodes_by_id,
            sub_node,
            None,
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, "(2.0 - 3.0)");
    }

    #[test]
    fn test_math_multiply() {
        let nodes = vec![
            Node {
                id: "mul".to_string(),
                node_type: "MathMultiply".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "a".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "b".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
        ];

        let connections = vec![
            Connection {
                id: "c1".to_string(),
                from: Endpoint {
                    node_id: "a".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "mul".to_string(),
                    port_id: "a".to_string(),
                },
            },
            Connection {
                id: "c2".to_string(),
                from: Endpoint {
                    node_id: "b".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "mul".to_string(),
                    port_id: "b".to_string(),
                },
            },
        ];

        let scene = create_test_scene_with_connections(nodes.clone(), connections);
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

        let mul_node = nodes_by_id.get("mul").unwrap();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_math_multiply(
            &scene,
            &nodes_by_id,
            mul_node,
            None,
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, "(2.0 * 3.0)");
    }

    #[test]
    fn test_math_divide() {
        let nodes = vec![
            Node {
                id: "div".to_string(),
                node_type: "MathDivide".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "a".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "b".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
        ];

        let connections = vec![
            Connection {
                id: "c1".to_string(),
                from: Endpoint {
                    node_id: "a".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "div".to_string(),
                    port_id: "a".to_string(),
                },
            },
            Connection {
                id: "c2".to_string(),
                from: Endpoint {
                    node_id: "b".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "div".to_string(),
                    port_id: "b".to_string(),
                },
            },
        ];

        let scene = create_test_scene_with_connections(nodes.clone(), connections);
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

        let div_node = nodes_by_id.get("div").unwrap();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_math_divide(
            &scene,
            &nodes_by_id,
            div_node,
            None,
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, "(2.0 / 3.0)");
    }

    #[test]
    fn test_math_max() {
        let nodes = vec![
            Node {
                id: "max_node".to_string(),
                node_type: "MathMax".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "a".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "b".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
        ];

        let connections = vec![
            Connection {
                id: "c1".to_string(),
                from: Endpoint {
                    node_id: "a".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "max_node".to_string(),
                    port_id: "a".to_string(),
                },
            },
            Connection {
                id: "c2".to_string(),
                from: Endpoint {
                    node_id: "b".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "max_node".to_string(),
                    port_id: "b".to_string(),
                },
            },
        ];

        let scene = create_test_scene_with_connections(nodes.clone(), connections);
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

        let max_node = nodes_by_id.get("max_node").unwrap();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_math_max(
            &scene,
            &nodes_by_id,
            max_node,
            None,
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, "max(2.0, 3.0)");
    }

    #[test]
    fn test_lerp_default_t() {
        let nodes = vec![
            Node {
                id: "lerp".to_string(),
                node_type: "Lerp".to_string(),
                params: HashMap::from([("t".to_string(), serde_json::json!(0.26))]),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "a".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "b".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
        ];

        let connections = vec![
            Connection {
                id: "c1".to_string(),
                from: Endpoint {
                    node_id: "a".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "lerp".to_string(),
                    port_id: "a".to_string(),
                },
            },
            Connection {
                id: "c2".to_string(),
                from: Endpoint {
                    node_id: "b".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "lerp".to_string(),
                    port_id: "b".to_string(),
                },
            },
        ];

        let scene = create_test_scene_with_connections(nodes.clone(), connections);
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

        let lerp_node = nodes_by_id.get("lerp").unwrap();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_lerp(
            &scene,
            &nodes_by_id,
            lerp_node,
            Some("result"),
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, "mix(2.0, 3.0, 0.26)");
    }

    #[test]
    fn test_lerp_color_splats_t() {
        let nodes = vec![
            Node {
                id: "lerp".to_string(),
                node_type: "Lerp".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "color_a".to_string(),
                node_type: "ColorInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "color_b".to_string(),
                node_type: "ColorInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
            Node {
                id: "t".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            },
        ];

        let connections = vec![
            Connection {
                id: "c1".to_string(),
                from: Endpoint {
                    node_id: "color_a".to_string(),
                    port_id: "color".to_string(),
                },
                to: Endpoint {
                    node_id: "lerp".to_string(),
                    port_id: "a".to_string(),
                },
            },
            Connection {
                id: "c2".to_string(),
                from: Endpoint {
                    node_id: "color_b".to_string(),
                    port_id: "color".to_string(),
                },
                to: Endpoint {
                    node_id: "lerp".to_string(),
                    port_id: "b".to_string(),
                },
            },
            Connection {
                id: "c3".to_string(),
                from: Endpoint {
                    node_id: "t".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "lerp".to_string(),
                    port_id: "t".to_string(),
                },
            },
        ];

        let scene = create_test_scene_with_connections(nodes.clone(), connections);
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

        let lerp_node = nodes_by_id.get("lerp").unwrap();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_lerp(
            &scene,
            &nodes_by_id,
            lerp_node,
            Some("result"),
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::Vec4);
        assert_eq!(
            result.expr,
            "mix(vec4f(1.0, 0.0, 0.0, 1.0), vec4f(0.0, 0.0, 1.0, 1.0), vec4f(0.25))"
        );
    }
}
