//! Compilers for input nodes (ColorInput, FloatInput, IntInput, Vector2Input, Vector3Input, TextureInput).

use anyhow::Result;
use serde_json::Value;

use crate::dsl::Node;
use super::super::types::{TypedExpr, ValueType};

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

/// Parse a vec4 array from a node's params.
fn parse_vec4_value_array(node: &Node, key: &str) -> Option<[f32; 4]> {
    let arr = node.params.get(key)?.as_array()?;
    let get = |i: usize, default: f32| -> f32 {
        arr.get(i)
            .and_then(parse_json_number_f32)
            .unwrap_or(default)
    };
    Some([get(0, 0.0), get(1, 0.0), get(2, 0.0), get(3, 1.0)])
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

/// Compile a ColorInput node to WGSL.
///
/// ColorInput nodes provide a constant RGBA color value.
///
/// # Parameters
/// - `value`: Array of 4 floats [r, g, b, a], defaults to [1.0, 0.0, 1.0, 1.0]
///
/// # Output
/// - Type: vec4f
/// - Uses time: false
pub fn compile_color_input(node: &Node, _out_port: Option<&str>) -> Result<TypedExpr> {
    let v = parse_vec4_value_array(node, "value").unwrap_or([1.0, 0.0, 1.0, 1.0]);
    // Premultiplied alpha convention: store RGB already multiplied by A.
    let a = v[3];
    Ok(TypedExpr::new(
        format!("vec4f({}, {}, {}, {})", v[0] * a, v[1] * a, v[2] * a, a),
        ValueType::Vec4,
    ))
}

/// Compile a FloatInput or IntInput node to WGSL.
///
/// These nodes provide a constant scalar value.
///
/// # Parameters
/// - `value`: Float or integer value, defaults to 0.0
///
/// # Output
/// - Type: f32
/// - Uses time: false
pub fn compile_float_or_int_input(node: &Node, _out_port: Option<&str>) -> Result<TypedExpr> {
    let v = parse_const_f32(node).unwrap_or(0.0);
    Ok(TypedExpr::new(format!("{v}"), ValueType::F32))
}

/// Compile a Vector2Input node to WGSL.
///
/// Vector2Input nodes provide a constant 2D vector value.
///
/// # Parameters
/// - `x`: X component, defaults to 0.0
/// - `y`: Y component, defaults to 0.0
///
/// # Output
/// - Type: vec2f
/// - Uses time: false
pub fn compile_vector2_input(node: &Node, _out_port: Option<&str>) -> Result<TypedExpr> {
    let v = parse_const_vec(node, ["x", "y", "z", "w"]).unwrap_or([0.0, 0.0, 0.0, 0.0]);
    Ok(TypedExpr::new(
        format!("vec2f({}, {})", v[0], v[1]),
        ValueType::Vec2,
    ))
}

/// Compile a Vector3Input node to WGSL.
///
/// Vector3Input nodes provide a constant 3D vector value.
///
/// # Parameters
/// - `x`: X component, defaults to 0.0
/// - `y`: Y component, defaults to 0.0
/// - `z`: Z component, defaults to 0.0
///
/// # Output
/// - Type: vec3f
/// - Uses time: false
pub fn compile_vector3_input(node: &Node, _out_port: Option<&str>) -> Result<TypedExpr> {
    let v = parse_const_vec(node, ["x", "y", "z", "w"]).unwrap_or([0.0, 0.0, 0.0, 0.0]);
    Ok(TypedExpr::new(
        format!("vec3f({}, {}, {})", v[0], v[1], v[2]),
        ValueType::Vec3,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_color_input_default() {
        let node = Node {
            id: "color1".to_string(),
            node_type: "ColorInput".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
        };
        
        let result = compile_color_input(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec4);
        assert_eq!(result.expr, "vec4f(1, 0, 1, 1)");
        assert!(!result.uses_time);
    }

    #[test]
    fn test_color_input_custom() {
        let node = Node {
            id: "color1".to_string(),
            node_type: "ColorInput".to_string(),
            params: HashMap::from([
                ("value".to_string(), serde_json::json!([0.5, 0.3, 0.8, 1.0]))
            ]),
            inputs: Vec::new(),
        };
        
        let result = compile_color_input(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec4);
        assert!(result.expr.contains("0.5"));
        assert!(result.expr.contains("0.3"));
    }

    #[test]
    fn test_float_input() {
        let node = Node {
            id: "float1".to_string(),
            node_type: "FloatInput".to_string(),
            params: HashMap::from([
                ("value".to_string(), serde_json::json!(3.14))
            ]),
            inputs: Vec::new(),
        };
        
        let result = compile_float_or_int_input(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, "3.14");
    }

    #[test]
    fn test_vector2_input() {
        let node = Node {
            id: "vec2_1".to_string(),
            node_type: "Vector2Input".to_string(),
            params: HashMap::from([
                ("x".to_string(), serde_json::json!(1.0)),
                ("y".to_string(), serde_json::json!(2.0)),
            ]),
            inputs: Vec::new(),
        };
        
        let result = compile_vector2_input(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec2);
        assert_eq!(result.expr, "vec2f(1, 2)");
    }

    #[test]
    fn test_vector3_input() {
        let node = Node {
            id: "vec3_1".to_string(),
            node_type: "Vector3Input".to_string(),
            params: HashMap::from([
                ("x".to_string(), serde_json::json!(1.0)),
                ("y".to_string(), serde_json::json!(2.0)),
                ("z".to_string(), serde_json::json!(3.0)),
            ]),
            inputs: Vec::new(),
        };
        
        let result = compile_vector3_input(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec3);
        assert_eq!(result.expr, "vec3f(1, 2, 3)");
    }
}
