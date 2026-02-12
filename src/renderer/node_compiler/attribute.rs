//! Compiler for Attribute node (reads vertex attributes).

use anyhow::{Result, bail};

use super::super::types::{TypedExpr, ValueType};
use crate::dsl::Node;

/// Compile an Attribute node to WGSL.
///
/// Attribute nodes read vertex attributes by name.
///
/// # Parameters
/// - `name`: Attribute name (e.g., "uv", "position", "normal")
/// - `glslType`: Expected GLSL type (used for validation, not code gen)
///
/// # Output
/// - Type: Depends on attribute (uv = vec2f, position/normal = vec3f)
/// - Uses time: false
///
/// # Supported Attributes
/// - `uv`: Texture coordinates (vec2f) - user-facing bottom-left semantics
///
/// # Example
/// ```wgsl
/// vec2f(in.uv.x, 1.0 - in.uv.y)  // For "uv" attribute
/// ```
pub fn compile_attribute(node: &Node, _out_port: Option<&str>) -> Result<TypedExpr> {
    let name = node
        .params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("uv")
        .to_ascii_lowercase();

    match name.as_str() {
        // Common aliases from GLSL graphs (e.g. vUv)
        "uv" | "vuv" | "v_uv" => Ok(TypedExpr::new(
            "vec2f(in.uv.x, 1.0 - in.uv.y)".to_string(),
            ValueType::Vec2,
        )),
        other => bail!(
            "unsupported Attribute.name: {} (only 'uv' is currently supported)",
            other
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_attribute_uv() {
        let node = Node {
            id: "attr1".to_string(),
            node_type: "Attribute".to_string(),
            params: HashMap::from([("name".to_string(), serde_json::json!("uv"))]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_attribute(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec2);
        assert_eq!(result.expr, "vec2f(in.uv.x, 1.0 - in.uv.y)");
        assert!(!result.uses_time);
    }

    #[test]
    fn test_attribute_uv_default() {
        // Default should be "uv" when name is not specified
        let node = Node {
            id: "attr1".to_string(),
            node_type: "Attribute".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_attribute(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec2);
        assert_eq!(result.expr, "vec2f(in.uv.x, 1.0 - in.uv.y)");
    }

    #[test]
    fn test_attribute_uv_case_insensitive() {
        let node = Node {
            id: "attr1".to_string(),
            node_type: "Attribute".to_string(),
            params: HashMap::from([("name".to_string(), serde_json::json!("UV"))]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_attribute(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec2);
        assert_eq!(result.expr, "vec2f(in.uv.x, 1.0 - in.uv.y)");
    }

    #[test]
    fn test_attribute_uv_expr_flips_y_for_bottom_left_semantics() {
        let node = Node {
            id: "attr1".to_string(),
            node_type: "Attribute".to_string(),
            params: HashMap::from([("name".to_string(), serde_json::json!("uv"))]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_attribute(&node, None).unwrap();
        assert!(result.expr.contains("1.0 - in.uv.y"));
    }

    #[test]
    fn test_attribute_unsupported() {
        let node = Node {
            id: "attr1".to_string(),
            node_type: "Attribute".to_string(),
            params: HashMap::from([("name".to_string(), serde_json::json!("position"))]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        assert!(compile_attribute(&node, None).is_err());
    }
}
