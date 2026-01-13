//! Compilers for trigonometry nodes (Sin, Cos) and Time node.

use std::collections::HashMap;
use anyhow::{anyhow, Result};

use crate::dsl::{incoming_connection, Node, SceneDSL};
use super::super::types::{TypedExpr, MaterialCompileContext};

/// Compile a Sin node.
/// 
/// Computes the sine of the input value.
pub fn compile_sin<F>(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: F,
) -> Result<TypedExpr>
where
    F: Fn(&str, Option<&str>, &mut MaterialCompileContext, &mut HashMap<(String, String), TypedExpr>) -> Result<TypedExpr>,
{
    let input = incoming_connection(scene, &node.id, "x")
        .or_else(|| incoming_connection(scene, &node.id, "value"))
        .or_else(|| incoming_connection(scene, &node.id, "in"))
        .ok_or_else(|| anyhow!("Sin missing input"))?;
    
    let x = compile_fn(&input.from.node_id, Some(&input.from.port_id), ctx, cache)?;
    
    Ok(TypedExpr::with_time(
        format!("sin({})", x.expr),
        x.ty,
        x.uses_time,
    ))
}

/// Compile a Cos node.
/// 
/// Computes the cosine of the input value.
pub fn compile_cos<F>(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: F,
) -> Result<TypedExpr>
where
    F: Fn(&str, Option<&str>, &mut MaterialCompileContext, &mut HashMap<(String, String), TypedExpr>) -> Result<TypedExpr>,
{
    let input = incoming_connection(scene, &node.id, "x")
        .or_else(|| incoming_connection(scene, &node.id, "value"))
        .or_else(|| incoming_connection(scene, &node.id, "in"))
        .ok_or_else(|| anyhow!("Cos missing input"))?;
    
    let x = compile_fn(&input.from.node_id, Some(&input.from.port_id), ctx, cache)?;
    
    Ok(TypedExpr::with_time(
        format!("cos({})", x.expr),
        x.ty,
        x.uses_time,
    ))
}

/// Compile a Time node.
/// 
/// Returns the current time value from the uniform parameters.
pub fn compile_time(
    _scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    _node: &Node,
    _out_port: Option<&str>,
    _ctx: &mut MaterialCompileContext,
    _cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    Ok(TypedExpr::with_time("params.time".to_string(), super::super::types::ValueType::F32, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::super::types::ValueType;
    use super::super::test_utils::{test_scene, test_connection};

    fn make_test_scene() -> (SceneDSL, HashMap<String, Node>) {
        let scene = test_scene(
            vec![
                Node {
                    id: "sin1".to_string(),
                    node_type: "Sin".to_string(),
                    params: HashMap::new(),
                    inputs: Vec::new(),
                },
                Node {
                    id: "input1".to_string(),
                    node_type: "FloatInput".to_string(),
                    params: HashMap::from([("value".to_string(), serde_json::json!(0.5))]),
                    inputs: Vec::new(),
                },
            ],
            vec![test_connection("input1", "value", "sin1", "x")],
        );
        let nodes_by_id = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();
        (scene, nodes_by_id)
    }

    fn mock_compile_fn(
        _node_id: &str,
        _out_port: Option<&str>,
        _ctx: &mut MaterialCompileContext,
        _cache: &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr> {
        Ok(TypedExpr::new("0.5".to_string(), ValueType::F32))
    }

    #[test]
    fn test_sin_compilation() {
        let (scene, nodes_by_id) = make_test_scene();
        let node = &scene.nodes[0];
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_sin(
            &scene,
            &nodes_by_id,
            node,
            None,
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert!(result.expr.contains("sin("));
        assert!(!result.uses_time);
    }

    #[test]
    fn test_cos_compilation() {
        let connections = vec![
            test_connection("input1", "value", "cos1", "value"),
        ];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "cos1".to_string(),
            node_type: "Cos".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_cos(
            &scene,
            &nodes_by_id,
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert!(result.expr.contains("cos("));
        assert!(!result.uses_time);
    }

    #[test]
    fn test_time_compilation() {
        use super::super::test_utils::test_connection;
        let connections = vec![
            test_connection("input", "value", "cos1", "value"),
        ];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "time1".to_string(),
            node_type: "Time".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_time(&scene, &nodes_by_id, &node, None, &mut ctx, &mut cache).unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, "params.time");
        assert!(result.uses_time); // Time node always depends on time
    }
}
