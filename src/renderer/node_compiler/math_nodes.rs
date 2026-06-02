//! Compilers for math operation nodes (MathAdd, MathSubtract, MathMultiply, MathDivide, Lerp, MathClamp, MathMax, MathPower).

use anyhow::Result;
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use super::super::utils::{coerce_for_binary, coerce_to_type};
use crate::dsl::{Node, SceneDSL, incoming_connection, parse_f32};

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

    let a_conn = incoming_connection(scene, &node.id, "a");
    let b_conn = incoming_connection(scene, &node.id, "b");
    let (Some(a_conn), Some(b_conn)) = (a_conn, b_conn) else {
        return Ok(TypedExpr::new("0.0", ValueType::F32));
    };

    let a = compile_fn(&a_conn.from.node_id, Some(&a_conn.from.port_id), ctx, cache)?;
    let b = compile_fn(&b_conn.from.node_id, Some(&b_conn.from.port_id), ctx, cache)?;
    let (a, b, ty) = coerce_for_binary(a, b)?;

    let t = if let Some(t_conn) = incoming_connection(scene, &node.id, "t") {
        let t_expr = compile_fn(&t_conn.from.node_id, Some(&t_conn.from.port_id), ctx, cache)?;
        coerce_to_type(t_expr, ValueType::F32)?
    } else {
        TypedExpr::new(
            wgsl_f32_literal(parse_f32(&node.params, "t").unwrap_or(0.5)),
            ValueType::F32,
        )
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
    let a_conn = incoming_connection(scene, &node.id, "a");
    let b_conn = incoming_connection(scene, &node.id, "b");
    let (Some(a_conn), Some(b_conn)) = (a_conn, b_conn) else {
        return Ok(TypedExpr::new("0.0", ValueType::F32));
    };

    let a = compile_fn(&a_conn.from.node_id, Some(&a_conn.from.port_id), ctx, cache)?;
    let b = compile_fn(&b_conn.from.node_id, Some(&b_conn.from.port_id), ctx, cache)?;
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

    fn create_test_scene_with_connections(
        nodes: Vec<Node>,
        connections: Vec<Connection>,
    ) -> SceneDSL {
        SceneDSL {
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
