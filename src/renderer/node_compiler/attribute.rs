//! Compiler for Attribute node (reads vertex attributes).

use anyhow::{bail, Result};

use crate::dsl::Node;
use super::super::types::{TypedExpr, ValueType};

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
/// - `uv`: Texture coordinates (vec2f) - from fragment shader input
///
/// # Example
/// ```wgsl
/// in.uv  // For "uv" attribute
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
        "uv" | "vuv" | "v_uv" => Ok(TypedExpr::new("in.uv".to_string(), ValueType::Vec2)),
        other => bail!("unsupported Attribute.name: {} (only 'uv' is currently supported)", other),
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
            params: HashMap::from([
                ("name".to_string(), serde_json::json!("uv"))
            ]),
            inputs: Vec::new(),
        };
        
        let result = compile_attribute(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec2);
        assert_eq!(result.expr, "in.uv");
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
        };
        
        let result = compile_attribute(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec2);
        assert_eq!(result.expr, "in.uv");
    }

    #[test]
    fn test_attribute_uv_case_insensitive() {
        let node = Node {
            id: "attr1".to_string(),
            node_type: "Attribute".to_string(),
            params: HashMap::from([
                ("name".to_string(), serde_json::json!("UV"))
            ]),
            inputs: Vec::new(),
        };
        
        let result = compile_attribute(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec2);
        assert_eq!(result.expr, "in.uv");
    }

    #[test]
    fn test_attribute_unsupported() {
        let node = Node {
            id: "attr1".to_string(),
            node_type: "Attribute".to_string(),
            params: HashMap::from([
                ("name".to_string(), serde_json::json!("position"))
            ]),
            inputs: Vec::new(),
        };
        
        assert!(compile_attribute(&node, None).is_err());
    }
}
