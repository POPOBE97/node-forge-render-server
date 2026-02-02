//! Compilers for vector math nodes (VectorMath, CrossProduct, DotProduct, Normalize, Refract).

use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use super::super::utils::coerce_to_type;
use crate::dsl::{incoming_connection, Node, SceneDSL};

/// Compile a DotProduct node.
///
/// Computes the dot product of two vectors.
pub fn compile_dot_product<F>(
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
        .ok_or_else(|| anyhow!("DotProduct missing input a"))?;
    let b_conn = incoming_connection(scene, &node.id, "b")
        .or_else(|| incoming_connection(scene, &node.id, "y"))
        .ok_or_else(|| anyhow!("DotProduct missing input b"))?;

    let a = compile_fn(&a_conn.from.node_id, Some(&a_conn.from.port_id), ctx, cache)?;
    let b = compile_fn(&b_conn.from.node_id, Some(&b_conn.from.port_id), ctx, cache)?;

    // Ensure both inputs are vectors of the same type
    if a.ty == ValueType::F32 || b.ty == ValueType::F32 {
        bail!(
            "DotProduct requires vector inputs, got {:?} and {:?}",
            a.ty,
            b.ty
        );
    }
    if a.ty != b.ty {
        bail!(
            "DotProduct requires same type vectors, got {:?} and {:?}",
            a.ty,
            b.ty
        );
    }

    Ok(TypedExpr::with_time(
        format!("dot({}, {})", a.expr, b.expr),
        ValueType::F32,
        a.uses_time || b.uses_time,
    ))
}

/// Compile a CrossProduct node.
///
/// Computes the cross product of two vec3 vectors.
pub fn compile_cross_product<F>(
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
        .ok_or_else(|| anyhow!("CrossProduct missing input a"))?;
    let b_conn = incoming_connection(scene, &node.id, "b")
        .or_else(|| incoming_connection(scene, &node.id, "y"))
        .ok_or_else(|| anyhow!("CrossProduct missing input b"))?;

    let a = compile_fn(&a_conn.from.node_id, Some(&a_conn.from.port_id), ctx, cache)?;
    let b = compile_fn(&b_conn.from.node_id, Some(&b_conn.from.port_id), ctx, cache)?;

    // Cross product requires vec3 inputs
    if a.ty != ValueType::Vec3 {
        bail!("CrossProduct requires vec3 for first input, got {:?}", a.ty);
    }
    if b.ty != ValueType::Vec3 {
        bail!(
            "CrossProduct requires vec3 for second input, got {:?}",
            b.ty
        );
    }

    Ok(TypedExpr::with_time(
        format!("cross({}, {})", a.expr, b.expr),
        ValueType::Vec3,
        a.uses_time || b.uses_time,
    ))
}

/// Compile a Normalize node.
///
/// Normalizes a vector to unit length.
pub fn compile_normalize<F>(
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
    let input = incoming_connection(scene, &node.id, "vector")
        .or_else(|| incoming_connection(scene, &node.id, "value"))
        .or_else(|| incoming_connection(scene, &node.id, "x"))
        .ok_or_else(|| anyhow!("Normalize missing input"))?;

    let x = compile_fn(&input.from.node_id, Some(&input.from.port_id), ctx, cache)?;

    // Ensure input is a vector
    if x.ty == ValueType::F32 {
        bail!("Normalize requires vector input, got {:?}", x.ty);
    }

    Ok(TypedExpr::with_time(
        format!("normalize({})", x.expr),
        x.ty,
        x.uses_time,
    ))
}

/// Compile a Refract node.
///
/// Computes the refraction vector of an incident vector about a normal.
///
/// Semantics (editor contract):
/// - eta = 1.0 / ior
/// - offset = refract(normalize(incident_vec3), normalize(normal_vec3), eta)
/// - normal/incident are coerced to vec3 per PORT_TYPE_COMPATIBILITY before normalize.
pub fn compile_refract<F>(
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
    let port = out_port.unwrap_or("offset");
    if port != "offset" {
        bail!("Refract: unsupported output port '{port}'");
    }

    let normal_conn = incoming_connection(scene, &node.id, "normal")
        .ok_or_else(|| anyhow!("Refract missing input normal"))?;
    let incident_conn = incoming_connection(scene, &node.id, "incident")
        .ok_or_else(|| anyhow!("Refract missing input incident"))?;

    let normal_raw = compile_fn(
        &normal_conn.from.node_id,
        Some(&normal_conn.from.port_id),
        ctx,
        cache,
    )?;
    let incident_raw = compile_fn(
        &incident_conn.from.node_id,
        Some(&incident_conn.from.port_id),
        ctx,
        cache,
    )?;

    // Coerce inputs to vec3 before normalize (per editor compatibility contract).
    let normal = coerce_to_type(normal_raw, ValueType::Vec3)?;
    let incident = coerce_to_type(incident_raw, ValueType::Vec3)?;

    // ior is optional: incoming connection wins, otherwise fall back to node.params["ior"].
    let ior = if let Some(conn) = incoming_connection(scene, &node.id, "ior") {
        let expr = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
        coerce_to_type(expr, ValueType::F32)?
    } else {
        let v = crate::dsl::parse_f32(&node.params, "ior").unwrap_or(1.5);
        TypedExpr::new(crate::renderer::utils::fmt_f32(v), ValueType::F32)
    };

    let eta_expr = format!("(1.0 / ({}))", ior.expr);
    Ok(TypedExpr::with_time(
        format!(
            "refract(normalize({}), normalize({}), {})",
            incident.expr, normal.expr, eta_expr
        ),
        ValueType::Vec3,
        normal.uses_time || incident.uses_time || ior.uses_time,
    ))
}

/// Compile a VectorMath node.
///
/// Performs various vector operations based on the "operation" parameter.
pub fn compile_vector_math<F>(
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
    let operation = node
        .params
        .get("operation")
        .and_then(|v| v.as_str())
        .unwrap_or("add")
        .to_lowercase();

    match operation.as_str() {
        "add" => {
            let a_conn = incoming_connection(scene, &node.id, "a")
                .ok_or_else(|| anyhow!("VectorMath.add missing input a"))?;
            let b_conn = incoming_connection(scene, &node.id, "b")
                .ok_or_else(|| anyhow!("VectorMath.add missing input b"))?;

            let a = compile_fn(&a_conn.from.node_id, Some(&a_conn.from.port_id), ctx, cache)?;
            let b = compile_fn(&b_conn.from.node_id, Some(&b_conn.from.port_id), ctx, cache)?;

            if a.ty != b.ty {
                bail!(
                    "VectorMath.add requires same type vectors, got {:?} and {:?}",
                    a.ty,
                    b.ty
                );
            }

            Ok(TypedExpr::with_time(
                format!("({} + {})", a.expr, b.expr),
                a.ty,
                a.uses_time || b.uses_time,
            ))
        }
        "subtract" | "sub" => {
            let a_conn = incoming_connection(scene, &node.id, "a")
                .ok_or_else(|| anyhow!("VectorMath.subtract missing input a"))?;
            let b_conn = incoming_connection(scene, &node.id, "b")
                .ok_or_else(|| anyhow!("VectorMath.subtract missing input b"))?;

            let a = compile_fn(&a_conn.from.node_id, Some(&a_conn.from.port_id), ctx, cache)?;
            let b = compile_fn(&b_conn.from.node_id, Some(&b_conn.from.port_id), ctx, cache)?;

            if a.ty != b.ty {
                bail!(
                    "VectorMath.subtract requires same type vectors, got {:?} and {:?}",
                    a.ty,
                    b.ty
                );
            }

            Ok(TypedExpr::with_time(
                format!("({} - {})", a.expr, b.expr),
                a.ty,
                a.uses_time || b.uses_time,
            ))
        }
        "multiply" | "mul" => {
            let a_conn = incoming_connection(scene, &node.id, "a")
                .ok_or_else(|| anyhow!("VectorMath.multiply missing input a"))?;
            let b_conn = incoming_connection(scene, &node.id, "b")
                .ok_or_else(|| anyhow!("VectorMath.multiply missing input b"))?;

            let a = compile_fn(&a_conn.from.node_id, Some(&a_conn.from.port_id), ctx, cache)?;
            let b = compile_fn(&b_conn.from.node_id, Some(&b_conn.from.port_id), ctx, cache)?;

            if a.ty != b.ty {
                bail!(
                    "VectorMath.multiply requires same type vectors, got {:?} and {:?}",
                    a.ty,
                    b.ty
                );
            }

            Ok(TypedExpr::with_time(
                format!("({} * {})", a.expr, b.expr),
                a.ty,
                a.uses_time || b.uses_time,
            ))
        }
        "dot" => compile_dot_product(scene, nodes_by_id, node, _out_port, ctx, cache, compile_fn),
        "cross" => {
            compile_cross_product(scene, nodes_by_id, node, _out_port, ctx, cache, compile_fn)
        }
        "normalize" => {
            compile_normalize(scene, nodes_by_id, node, _out_port, ctx, cache, compile_fn)
        }
        "length" => {
            let input = incoming_connection(scene, &node.id, "a")
                .or_else(|| incoming_connection(scene, &node.id, "vector"))
                .ok_or_else(|| anyhow!("VectorMath.length missing input"))?;

            let x = compile_fn(&input.from.node_id, Some(&input.from.port_id), ctx, cache)?;

            if x.ty == ValueType::F32 {
                bail!("VectorMath.length requires vector input, got {:?}", x.ty);
            }

            Ok(TypedExpr::with_time(
                format!("length({})", x.expr),
                ValueType::F32,
                x.uses_time,
            ))
        }
        other => bail!("unsupported VectorMath operation: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::types::ValueType;
    use super::super::test_utils::test_scene;
    use super::*;

    fn mock_vec3_compile_fn(
        _node_id: &str,
        _out_port: Option<&str>,
        _ctx: &mut MaterialCompileContext,
        _cache: &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr> {
        Ok(TypedExpr::new(
            "vec3f(1.0, 0.0, 0.0)".to_string(),
            ValueType::Vec3,
        ))
    }

    #[test]
    fn test_dot_product() {
        use super::super::test_utils::test_connection;
        let connections = vec![
            test_connection("vec1", "value", "dot1", "a"),
            test_connection("vec2", "value", "dot1", "b"),
        ];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "dot1".to_string(),
            node_type: "DotProduct".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_dot_product(
            &scene,
            &nodes_by_id,
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_vec3_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert!(result.expr.contains("dot("));
    }

    #[test]
    fn test_cross_product() {
        use super::super::test_utils::test_connection;
        let connections = vec![
            test_connection("vec1", "value", "cross1", "a"),
            test_connection("vec2", "value", "cross1", "b"),
        ];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "cross1".to_string(),
            node_type: "CrossProduct".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_cross_product(
            &scene,
            &nodes_by_id,
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_vec3_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::Vec3);
        assert!(result.expr.contains("cross("));
    }

    #[test]
    fn test_normalize() {
        use super::super::test_utils::test_connection;
        let connections = vec![test_connection("vec_in", "value", "norm1", "vector")];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "norm1".to_string(),
            node_type: "Normalize".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_normalize(
            &scene,
            &nodes_by_id,
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_vec3_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::Vec3);
        assert!(result.expr.contains("normalize("));
    }

    #[test]
    fn test_vector_math_add() {
        use super::super::test_utils::test_connection;
        let connections = vec![
            test_connection("vec1", "value", "vm1", "a"),
            test_connection("vec2", "value", "vm1", "b"),
        ];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "vm1".to_string(),
            node_type: "VectorMath".to_string(),
            params: HashMap::from([("operation".to_string(), serde_json::json!("add"))]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_vector_math(
            &scene,
            &nodes_by_id,
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_vec3_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::Vec3);
        assert!(result.expr.contains("+"));
    }
}
