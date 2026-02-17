//! Compilers for math operation nodes (MathAdd, MathMultiply, MathClamp, MathPower).

use anyhow::Result;
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use super::super::utils::coerce_for_binary;
use crate::dsl::{Node, SceneDSL, incoming_connection};

/// Compile a MathAdd node to WGSL.
///
/// MathAdd nodes add two values together.
///
/// # Inputs
/// - `a` or `x`: First operand
/// - `b` or `y`: Second operand
///
/// # Output
/// - Type: Matches input types (with scalar-to-vector promotion)
/// - Uses time: true if any input uses time
///
/// # Example
/// ```wgsl
/// (a + b)
/// ```
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
    let a_conn = incoming_connection(scene, &node.id, "a")
        .or_else(|| incoming_connection(scene, &node.id, "x"))
        .ok_or_else(|| anyhow::anyhow!("MathAdd missing input a"))?;
    let b_conn = incoming_connection(scene, &node.id, "b")
        .or_else(|| incoming_connection(scene, &node.id, "y"))
        .ok_or_else(|| anyhow::anyhow!("MathAdd missing input b"))?;

    let a = compile_fn(&a_conn.from.node_id, Some(&a_conn.from.port_id), ctx, cache)?;
    let b = compile_fn(&b_conn.from.node_id, Some(&b_conn.from.port_id), ctx, cache)?;

    let (aa, bb, ty) = coerce_for_binary(a, b)?;
    Ok(TypedExpr::with_time(
        format!("({} + {})", aa.expr, bb.expr),
        ty,
        aa.uses_time || bb.uses_time,
    ))
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

/// Compile a MathClamp node to WGSL.
///
/// MathClamp nodes clamp a value between minimum and maximum bounds.
///
/// # Inputs
/// - `value` or `x`: Value to clamp
/// - `min` or `lo`: Minimum bound
/// - `max` or `hi`: Maximum bound
///
/// # Output
/// - Type: Matches input types (with scalar-to-vector promotion)
/// - Uses time: true if any input uses time
///
/// # Example
/// ```wgsl
/// clamp(value, min, max)
/// ```
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
    let x_conn = incoming_connection(scene, &node.id, "value")
        .or_else(|| incoming_connection(scene, &node.id, "x"))
        .ok_or_else(|| anyhow::anyhow!("MathClamp missing input value"))?;
    let min_conn = incoming_connection(scene, &node.id, "min")
        .or_else(|| incoming_connection(scene, &node.id, "lo"))
        .ok_or_else(|| anyhow::anyhow!("MathClamp missing input min"))?;
    let max_conn = incoming_connection(scene, &node.id, "max")
        .or_else(|| incoming_connection(scene, &node.id, "hi"))
        .ok_or_else(|| anyhow::anyhow!("MathClamp missing input max"))?;

    let x = compile_fn(&x_conn.from.node_id, Some(&x_conn.from.port_id), ctx, cache)?;
    let min = compile_fn(
        &min_conn.from.node_id,
        Some(&min_conn.from.port_id),
        ctx,
        cache,
    )?;
    let max = compile_fn(
        &max_conn.from.node_id,
        Some(&max_conn.from.port_id),
        ctx,
        cache,
    )?;

    // All three values should have the same type (or be promoted)
    let (x, min, _ty) = coerce_for_binary(x, min)?;
    let (min, max, ty) = coerce_for_binary(min, max)?;
    let (x, _, ty) = coerce_for_binary(x, TypedExpr::with_time("".to_string(), ty, false))?;

    Ok(TypedExpr::with_time(
        format!("clamp({}, {}, {})", x.expr, min.expr, max.expr),
        ty,
        x.uses_time || min.uses_time || max.uses_time,
    ))
}

/// Compile a MathPower node to WGSL.
///
/// MathPower nodes raise a base to an exponent.
///
/// # Inputs
/// - `base`: Base value
/// - `exponent`: Exponent value
///
/// # Output
/// - Type: Matches input types (with scalar-to-vector promotion)
/// - Uses time: true if any input uses time
///
/// # Example
/// ```wgsl
/// pow(base, exponent)
/// ```
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
    let base_conn = incoming_connection(scene, &node.id, "base")
        .ok_or_else(|| anyhow::anyhow!("MathPower missing input base"))?;
    let exp_conn = incoming_connection(scene, &node.id, "exponent")
        .ok_or_else(|| anyhow::anyhow!("MathPower missing input exponent"))?;

    let base = compile_fn(
        &base_conn.from.node_id,
        Some(&base_conn.from.port_id),
        ctx,
        cache,
    )?;
    let exp = compile_fn(
        &exp_conn.from.node_id,
        Some(&exp_conn.from.port_id),
        ctx,
        cache,
    )?;

    let (base, exp, ty) = coerce_for_binary(base, exp)?;
    Ok(TypedExpr::with_time(
        format!("pow({}, {})", base.expr, exp.expr),
        ty,
        base.uses_time || exp.uses_time,
    ))
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
            },
            Node {
                id: "a".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
            },
            Node {
                id: "b".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
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
    fn test_math_multiply() {
        let nodes = vec![
            Node {
                id: "mul".to_string(),
                node_type: "MathMultiply".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
            },
            Node {
                id: "a".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
            },
            Node {
                id: "b".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                input_bindings: Vec::new(),
                outputs: Vec::new(),
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
}
