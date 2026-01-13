//! Compilers for legacy node types that are being replaced by newer variants.
//!
//! These nodes are kept for backward compatibility with older DSL files:
//! - Float, Scalar, Constant (use FloatInput instead)
//! - Vec2, Vec3, Vec4, Color (use Vector2Input, Vector3Input, ColorInput instead)
//! - Add, Mul, Mix, Clamp, Smoothstep (use MathAdd, MathMultiply, etc. instead)

use std::collections::HashMap;
use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::dsl::{incoming_connection, Node, SceneDSL};
use super::super::types::{TypedExpr, ValueType, MaterialCompileContext};
use super::super::utils::coerce_for_binary;

/// Parse a JSON value as an f32.
fn parse_json_number_f32(v: &Value) -> Option<f32> {
    v.as_f64()
        .map(|x| x as f32)
        .or_else(|| v.as_i64().map(|x| x as f32))
        .or_else(|| v.as_u64().map(|x| x as f32))
}

/// Parse a constant float value from a node's params.
fn parse_const_f32(node: &Node) -> Option<f32> {
    parse_json_number_f32(node.params.get("value")?)
        .or_else(|| parse_json_number_f32(node.params.get("x")?))
        .or_else(|| parse_json_number_f32(node.params.get("v")?))
}

/// Parse a vector from node params using specified keys.
fn parse_const_vec(node: &Node, keys: [&str; 4]) -> Option<[f32; 4]> {
    let x = parse_json_number_f32(node.params.get(keys[0])?)?;
    let y = node
        .params
        .get(keys[1])
        .and_then(parse_json_number_f32)
        .unwrap_or(0.0);
    let z = node
        .params
        .get(keys[2])
        .and_then(parse_json_number_f32)
        .unwrap_or(0.0);
    let w = node
        .params
        .get(keys[3])
        .and_then(parse_json_number_f32)
        .unwrap_or(1.0);
    Some([x, y, z, w])
}

/// Compile Float, Scalar, or Constant nodes (legacy FloatInput).
pub fn compile_float_scalar_constant(
    _scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    _ctx: &mut MaterialCompileContext,
    _cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    let v = parse_const_f32(node).unwrap_or(0.0);
    Ok(TypedExpr::new(format!("{v}"), ValueType::F32))
}

/// Compile Vec2 nodes (legacy Vector2Input).
pub fn compile_vec2(
    _scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    _ctx: &mut MaterialCompileContext,
    _cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    let v = parse_const_vec(node, ["x", "y", "z", "w"]).unwrap_or([0.0, 0.0, 0.0, 0.0]);
    Ok(TypedExpr::new(
        format!("vec2f({}, {})", v[0], v[1]),
        ValueType::Vec2,
    ))
}

/// Compile Vec3 nodes (legacy Vector3Input).
pub fn compile_vec3(
    _scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    _ctx: &mut MaterialCompileContext,
    _cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    let v = parse_const_vec(node, ["x", "y", "z", "w"]).unwrap_or([0.0, 0.0, 0.0, 0.0]);
    Ok(TypedExpr::new(
        format!("vec3f({}, {}, {})", v[0], v[1], v[2]),
        ValueType::Vec3,
    ))
}

/// Compile Vec4 or Color nodes (legacy ColorInput).
pub fn compile_vec4_color(
    _scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    _ctx: &mut MaterialCompileContext,
    _cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    // Accept either x/y/z/w or r/g/b/a.
    let v = parse_const_vec(node, ["x", "y", "z", "w"])
        .or_else(|| parse_const_vec(node, ["r", "g", "b", "a"]))
        .unwrap_or([1.0, 0.0, 1.0, 1.0]);
    Ok(TypedExpr::new(
        format!("vec4f({}, {}, {}, {})", v[0], v[1], v[2], v[3]),
        ValueType::Vec4,
    ))
}

/// Compile Add node (legacy MathAdd).
pub fn compile_add<F>(
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
    let a_conn = incoming_connection(scene, &node.id, "a")
        .or_else(|| incoming_connection(scene, &node.id, "x"))
        .ok_or_else(|| anyhow!("Add missing input a"))?;
    let b_conn = incoming_connection(scene, &node.id, "b")
        .or_else(|| incoming_connection(scene, &node.id, "y"))
        .ok_or_else(|| anyhow!("Add missing input b"))?;
    
    let a = compile_fn(&a_conn.from.node_id, Some(&a_conn.from.port_id), ctx, cache)?;
    let b = compile_fn(&b_conn.from.node_id, Some(&b_conn.from.port_id), ctx, cache)?;
    
    let (aa, bb, ty) = coerce_for_binary(a, b)?;
    Ok(TypedExpr::with_time(
        format!("({} + {})", aa.expr, bb.expr),
        ty,
        aa.uses_time || bb.uses_time,
    ))
}

/// Compile Mul or Multiply node (legacy MathMultiply).
pub fn compile_mul<F>(
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
    let a_conn = incoming_connection(scene, &node.id, "a")
        .or_else(|| incoming_connection(scene, &node.id, "x"))
        .ok_or_else(|| anyhow!("Mul missing input a"))?;
    let b_conn = incoming_connection(scene, &node.id, "b")
        .or_else(|| incoming_connection(scene, &node.id, "y"))
        .ok_or_else(|| anyhow!("Mul missing input b"))?;
    
    let a = compile_fn(&a_conn.from.node_id, Some(&a_conn.from.port_id), ctx, cache)?;
    let b = compile_fn(&b_conn.from.node_id, Some(&b_conn.from.port_id), ctx, cache)?;
    
    let (aa, bb, ty) = coerce_for_binary(a, b)?;
    Ok(TypedExpr::with_time(
        format!("({} * {})", aa.expr, bb.expr),
        ty,
        aa.uses_time || bb.uses_time,
    ))
}

/// Compile Mix node (legacy ColorMix or similar).
pub fn compile_mix<F>(
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
    let a_conn = incoming_connection(scene, &node.id, "a")
        .or_else(|| incoming_connection(scene, &node.id, "x"))
        .ok_or_else(|| anyhow!("Mix missing input a"))?;
    let b_conn = incoming_connection(scene, &node.id, "b")
        .or_else(|| incoming_connection(scene, &node.id, "y"))
        .ok_or_else(|| anyhow!("Mix missing input b"))?;
    let t_conn = incoming_connection(scene, &node.id, "t")
        .or_else(|| incoming_connection(scene, &node.id, "alpha"))
        .or_else(|| incoming_connection(scene, &node.id, "factor"))
        .ok_or_else(|| anyhow!("Mix missing input t"))?;

    let a = compile_fn(&a_conn.from.node_id, Some(&a_conn.from.port_id), ctx, cache)?;
    let b = compile_fn(&b_conn.from.node_id, Some(&b_conn.from.port_id), ctx, cache)?;
    let t = compile_fn(&t_conn.from.node_id, Some(&t_conn.from.port_id), ctx, cache)?;
    
    if t.ty != ValueType::F32 {
        bail!("Mix.t must be f32, got {:?}", t.ty);
    }
    
    let (aa, bb, ty) = coerce_for_binary(a, b)?;
    let tt = if ty == ValueType::F32 {
        t
    } else {
        // WGSL allows vecNf(f32) splat constructors.
        TypedExpr::with_time(format!("{}({})", ty.wgsl(), t.expr), ty, t.uses_time)
    };
    
    Ok(TypedExpr::with_time(
        format!("mix({}, {}, {})", aa.expr, bb.expr, tt.expr),
        ty,
        aa.uses_time || bb.uses_time || tt.uses_time,
    ))
}

/// Compile Clamp node (legacy MathClamp).
pub fn compile_clamp<F>(
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
    let x_conn = incoming_connection(scene, &node.id, "x")
        .or_else(|| incoming_connection(scene, &node.id, "value"))
        .ok_or_else(|| anyhow!("Clamp missing input x"))?;
    let min_conn = incoming_connection(scene, &node.id, "min")
        .or_else(|| incoming_connection(scene, &node.id, "lo"))
        .ok_or_else(|| anyhow!("Clamp missing input min"))?;
    let max_conn = incoming_connection(scene, &node.id, "max")
        .or_else(|| incoming_connection(scene, &node.id, "hi"))
        .ok_or_else(|| anyhow!("Clamp missing input max"))?;
    
    let x = compile_fn(&x_conn.from.node_id, Some(&x_conn.from.port_id), ctx, cache)?;
    let minv = compile_fn(&min_conn.from.node_id, Some(&min_conn.from.port_id), ctx, cache)?;
    let maxv = compile_fn(&max_conn.from.node_id, Some(&max_conn.from.port_id), ctx, cache)?;
    
    let (xx, mn, ty) = coerce_for_binary(x, minv)?;
    let (xx2, mx, _) = coerce_for_binary(xx, maxv)?;
    
    Ok(TypedExpr::with_time(
        format!("clamp({}, {}, {})", xx2.expr, mn.expr, mx.expr),
        ty,
        xx2.uses_time || mn.uses_time || mx.uses_time,
    ))
}

/// Compile Smoothstep node.
pub fn compile_smoothstep<F>(
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
    let e0_conn = incoming_connection(scene, &node.id, "edge0")
        .or_else(|| incoming_connection(scene, &node.id, "min"))
        .ok_or_else(|| anyhow!("Smoothstep missing input edge0"))?;
    let e1_conn = incoming_connection(scene, &node.id, "edge1")
        .or_else(|| incoming_connection(scene, &node.id, "max"))
        .ok_or_else(|| anyhow!("Smoothstep missing input edge1"))?;
    let x_conn = incoming_connection(scene, &node.id, "x")
        .or_else(|| incoming_connection(scene, &node.id, "value"))
        .ok_or_else(|| anyhow!("Smoothstep missing input x"))?;
    
    let e0 = compile_fn(&e0_conn.from.node_id, Some(&e0_conn.from.port_id), ctx, cache)?;
    let e1 = compile_fn(&e1_conn.from.node_id, Some(&e1_conn.from.port_id), ctx, cache)?;
    let x = compile_fn(&x_conn.from.node_id, Some(&x_conn.from.port_id), ctx, cache)?;
    
    let (e0c, e1c, ty01) = coerce_for_binary(e0, e1)?;
    let (xc, _, ty) = coerce_for_binary(x, e0c.clone())?;
    
    if ty != ty01 {
        bail!("Smoothstep type mismatch: {:?} vs {:?}", ty01, ty);
    }
    
    Ok(TypedExpr::with_time(
        format!("smoothstep({}, {}, {})", e0c.expr, e1c.expr, xc.expr),
        ty,
        e0c.uses_time || e1c.uses_time || xc.uses_time,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_utils::test_scene;

    #[test]
    fn test_float_scalar_constant() {
        let node = Node {
            id: "f1".to_string(),
            node_type: "Float".to_string(),
            params: HashMap::from([("value".to_string(), serde_json::json!(3.14))]),
            inputs: Vec::new(),
        };
        let scene = test_scene(vec![], vec![]);
        let nodes_by_id = HashMap::new();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_float_scalar_constant(&scene, &nodes_by_id, &node, None, &mut ctx, &mut cache).unwrap();
        assert_eq!(result.ty, ValueType::F32);
        assert!(result.expr.contains("3.14"));
        assert!(!result.uses_time);
    }

    #[test]
    fn test_vec2() {
        let node = Node {
            id: "v2".to_string(),
            node_type: "Vec2".to_string(),
            params: HashMap::from([
                ("x".to_string(), serde_json::json!(1.0)),
                ("y".to_string(), serde_json::json!(2.0)),
            ]),
            inputs: Vec::new(),
        };
        let scene = test_scene(vec![], vec![]);
        let nodes_by_id = HashMap::new();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_vec2(&scene, &nodes_by_id, &node, None, &mut ctx, &mut cache).unwrap();
        assert_eq!(result.ty, ValueType::Vec2);
        assert!(result.expr.contains("vec2f"));
        assert!(!result.uses_time);
    }

    #[test]
    fn test_vec4_color() {
        let node = Node {
            id: "c1".to_string(),
            node_type: "Color".to_string(),
            params: HashMap::from([
                ("r".to_string(), serde_json::json!(0.8)),
                ("g".to_string(), serde_json::json!(0.2)),
                ("b".to_string(), serde_json::json!(0.4)),
                ("a".to_string(), serde_json::json!(1.0)),
            ]),
            inputs: Vec::new(),
        };
        let scene = test_scene(vec![], vec![]);
        let nodes_by_id = HashMap::new();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_vec4_color(&scene, &nodes_by_id, &node, None, &mut ctx, &mut cache).unwrap();
        assert_eq!(result.ty, ValueType::Vec4);
        assert!(result.expr.contains("vec4f"));
        assert!(!result.uses_time);
    }
}
