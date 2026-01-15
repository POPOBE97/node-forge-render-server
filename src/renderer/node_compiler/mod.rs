//! Node compiler infrastructure and trait definition.

pub mod input_nodes;
pub mod math_nodes;
pub mod math_closure;
pub mod attribute;
pub mod texture_nodes;
pub mod trigonometry_nodes;
pub mod vector_nodes;
pub mod color_nodes;
pub mod geometry_nodes;

use std::collections::HashMap;
use anyhow::{bail, Result};

use crate::dsl::{find_node, Node, SceneDSL};
use super::types::{TypedExpr, MaterialCompileContext};

/// Main dispatch function for compiling material expressions.
/// 
/// This is the modular replacement for the monolithic `compile_material_expr` function.
/// It dispatches to specific node compiler modules based on node type.
pub fn compile_material_expr(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    // Check cache first
    let key = (
        node_id.to_string(),
        out_port.unwrap_or("value").to_string(),
    );
    if let Some(v) = cache.get(&key) {
        return Ok(v.clone());
    }

    let node = find_node(nodes_by_id, node_id)?;
    
    // Create a recursive compile function for child nodes to use
    let compile_fn = |id: &str, port: Option<&str>, ctx: &mut MaterialCompileContext, cache: &mut HashMap<(String, String), TypedExpr>| {
        compile_material_expr(scene, nodes_by_id, id, port, ctx, cache)
    };
    
    // Dispatch to specific node compiler based on node type
    let result = match node.node_type.as_str() {
        // Input nodes
        "ColorInput" => input_nodes::compile_color_input(node, out_port)?,
        "FloatInput" | "IntInput" => input_nodes::compile_float_or_int_input(node, out_port)?,
        "Vector2Input" => input_nodes::compile_vector2_input(node, out_port)?,
        "Vector3Input" => input_nodes::compile_vector3_input(node, out_port)?,
        
        // Attribute node
        "Attribute" => attribute::compile_attribute(node, out_port)?,
        
        // Math nodes
        "MathAdd" => math_nodes::compile_math_add(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "MathMultiply" => math_nodes::compile_math_multiply(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "MathClamp" => math_nodes::compile_math_clamp(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "MathPower" => math_nodes::compile_math_power(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "MathClosure" => math_closure::compile_math_closure(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        
        // Texture nodes
        "ImageTexture" => texture_nodes::compile_image_texture(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "PassTexture" => texture_nodes::compile_pass_texture(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        
        // Trigonometry nodes
        "Sin" => trigonometry_nodes::compile_sin(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "Cos" => trigonometry_nodes::compile_cos(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "Time" => trigonometry_nodes::compile_time(scene, nodes_by_id, node, out_port, ctx, cache)?,
        
        // Vector nodes
        "VectorMath" => vector_nodes::compile_vector_math(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "DotProduct" => vector_nodes::compile_dot_product(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "CrossProduct" => vector_nodes::compile_cross_product(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "Normalize" => vector_nodes::compile_normalize(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        
        // Color nodes
        "ColorMix" => color_nodes::compile_color_mix(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "ColorRamp" => color_nodes::compile_color_ramp(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "HSVAdjust" => color_nodes::compile_hsv_adjust(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        
        // Unsupported node types
        other => bail!("unsupported material node type: {other}"),
    };
    
    // Cache the result
    cache.insert(key, result.clone());
    Ok(result)
}

/// Test utilities for creating test scenes.
/// 
/// TEMPORARY: These helpers exist to provide default values for SceneDSL fields
/// that are required but not relevant to unit tests. Will be kept as long as
/// unit tests need to construct SceneDSL instances directly.
#[cfg(test)]
pub mod test_utils {
    use crate::dsl::{SceneDSL, Node, Connection, Metadata};
    use std::collections::HashMap;

    /// Create a SceneDSL for testing with default metadata and version.
    pub fn test_scene(nodes: Vec<Node>, connections: Vec<Connection>) -> SceneDSL {
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
        }
    }

    /// Create a SceneDSL with custom outputs.
    pub fn test_scene_with_outputs(
        nodes: Vec<Node>,
        connections: Vec<Connection>,
        outputs: HashMap<String, String>,
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
            outputs: Some(outputs),
        }
    }

    /// Create a Connection for testing.
    pub fn test_connection(
        from_node: &str,
        from_port: &str,
        to_node: &str,
        to_port: &str,
    ) -> Connection {
        Connection {
            id: format!("{}_{}_{}", from_node, to_node, to_port),
            from: crate::dsl::Endpoint {
                node_id: from_node.to_string(),
                port_id: from_port.to_string(),
            },
            to: crate::dsl::Endpoint {
                node_id: to_node.to_string(),
                port_id: to_port.to_string(),
            },
        }
    }
}
