//! Compilers for trigonometry nodes and Time node.

use anyhow::{Result, anyhow};
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr};
use super::super::utils::coerce_for_binary;
use crate::dsl::{Node, SceneDSL, incoming_connection};

fn compile_unary_trig<F>(
    scene: &SceneDSL,
    node: &Node,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: &F,
    node_name: &str,
    wgsl_fn: &str,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    let input = incoming_connection(scene, &node.id, "x")
        .or_else(|| incoming_connection(scene, &node.id, "value"))
        .or_else(|| incoming_connection(scene, &node.id, "in"))
        .ok_or_else(|| anyhow!("{node_name} missing input"))?;

    let x = compile_fn(&input.from.node_id, Some(&input.from.port_id), ctx, cache)?;

    Ok(TypedExpr::with_time(
        format!("{wgsl_fn}({})", x.expr),
        x.ty,
        x.uses_time,
    ))
}

fn compile_binary_trig<F>(
    scene: &SceneDSL,
    node: &Node,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: &F,
    node_name: &str,
    wgsl_fn: &str,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    let y_input = incoming_connection(scene, &node.id, "y")
        .or_else(|| incoming_connection(scene, &node.id, "a"))
        .ok_or_else(|| anyhow!("{node_name} missing input y"))?;
    let x_input = incoming_connection(scene, &node.id, "x")
        .or_else(|| incoming_connection(scene, &node.id, "b"))
        .ok_or_else(|| anyhow!("{node_name} missing input x"))?;

    let y = compile_fn(
        &y_input.from.node_id,
        Some(&y_input.from.port_id),
        ctx,
        cache,
    )?;
    let x = compile_fn(
        &x_input.from.node_id,
        Some(&x_input.from.port_id),
        ctx,
        cache,
    )?;
    let (y, x, ty) = coerce_for_binary(y, x)?;

    Ok(TypedExpr::with_time(
        format!("{wgsl_fn}({}, {})", y.expr, x.expr),
        ty,
        y.uses_time || x.uses_time,
    ))
}

macro_rules! unary_trig_compiler {
    ($fn_name:ident, $node_name:literal, $wgsl_fn:literal) => {
        pub fn $fn_name<F>(
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
            compile_unary_trig(scene, node, ctx, cache, &compile_fn, $node_name, $wgsl_fn)
        }
    };
}

unary_trig_compiler!(compile_sin, "Sin", "sin");
unary_trig_compiler!(compile_cos, "Cos", "cos");
unary_trig_compiler!(compile_tan, "Tan", "tan");
unary_trig_compiler!(compile_asin, "Asin", "asin");
unary_trig_compiler!(compile_acos, "Acos", "acos");
unary_trig_compiler!(compile_atan, "Atan", "atan");

pub fn compile_atan2<F>(
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
    compile_binary_trig(scene, node, ctx, cache, &compile_fn, "Atan2", "atan2")
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
    Ok(TypedExpr::with_time(
        "params.time".to_string(),
        super::super::types::ValueType::F32,
        true,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::super::types::ValueType;
    use super::super::test_utils::{test_connection, test_scene};
    use super::*;

    fn make_test_scene() -> (SceneDSL, HashMap<String, Node>) {
        let scene = test_scene(
            vec![
                Node {
                    id: "sin1".to_string(),
                    node_type: "Sin".to_string(),
                    params: HashMap::new(),
                    inputs: Vec::new(),
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                    wgsl_override: None,
                },
                Node {
                    id: "input1".to_string(),
                    node_type: "FloatInput".to_string(),
                    params: HashMap::from([("value".to_string(), serde_json::json!(0.5))]),
                    inputs: Vec::new(),
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                    wgsl_override: None,
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
        let connections = vec![test_connection("input1", "value", "cos1", "value")];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "cos1".to_string(),
            node_type: "Cos".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
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
    fn test_added_unary_trig_compilation() {
        let cases: Vec<(
            &str,
            fn(
                &SceneDSL,
                &HashMap<String, Node>,
                &Node,
                Option<&str>,
                &mut MaterialCompileContext,
                &mut HashMap<(String, String), TypedExpr>,
                fn(
                    &str,
                    Option<&str>,
                    &mut MaterialCompileContext,
                    &mut HashMap<(String, String), TypedExpr>,
                ) -> Result<TypedExpr>,
            ) -> Result<TypedExpr>,
            &str,
        )> = vec![
            ("Tan", compile_tan, "tan("),
            ("Asin", compile_asin, "asin("),
            ("Acos", compile_acos, "acos("),
            ("Atan", compile_atan, "atan("),
        ];

        for (node_type, compile, needle) in cases {
            let node_id = format!("{}_1", node_type.to_lowercase());
            let connections = vec![test_connection("input1", "value", &node_id, "x")];
            let scene = test_scene(vec![], connections);
            let nodes_by_id = HashMap::new();
            let node = Node {
                id: node_id,
                node_type: node_type.to_string(),
                params: HashMap::new(),
                inputs: Vec::new(),
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            };
            let mut ctx = MaterialCompileContext::default();
            let mut cache = HashMap::new();

            let result = compile(
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
            assert!(result.expr.contains(needle));
            assert!(!result.uses_time);
        }
    }

    #[test]
    fn test_atan2_compilation() {
        let connections = vec![
            test_connection("input_y", "value", "atan2_1", "y"),
            test_connection("input_x", "value", "atan2_1", "x"),
        ];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "atan2_1".to_string(),
            node_type: "Atan2".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_atan2(
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
        assert!(result.expr.contains("atan2("));
        assert!(!result.uses_time);
    }

    #[test]
    fn test_time_compilation() {
        use super::super::test_utils::test_connection;
        let connections = vec![test_connection("input", "value", "cos1", "value")];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "time1".to_string(),
            node_type: "Time".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_time(&scene, &nodes_by_id, &node, None, &mut ctx, &mut cache).unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, "params.time");
        assert!(result.uses_time); // Time node always depends on time
    }
}
