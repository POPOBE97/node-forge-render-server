//! Node compiler infrastructure and trait definition.

pub mod input_nodes;
pub mod math_nodes;
pub mod attribute;
pub mod texture_nodes;
pub mod trigonometry_nodes;
pub mod legacy_nodes;
pub mod vector_nodes;
pub mod color_nodes;

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
        
        // Texture nodes
        "ImageTexture" => texture_nodes::compile_image_texture(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        
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
        
        // Legacy nodes (for backward compatibility)
        "Float" | "Scalar" | "Constant" => legacy_nodes::compile_float_scalar_constant(scene, nodes_by_id, node, out_port, ctx, cache)?,
        "Vec2" => legacy_nodes::compile_vec2(scene, nodes_by_id, node, out_port, ctx, cache)?,
        "Vec3" => legacy_nodes::compile_vec3(scene, nodes_by_id, node, out_port, ctx, cache)?,
        "Vec4" | "Color" => legacy_nodes::compile_vec4_color(scene, nodes_by_id, node, out_port, ctx, cache)?,
        "Add" => legacy_nodes::compile_add(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "Mul" | "Multiply" => legacy_nodes::compile_mul(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "Mix" => legacy_nodes::compile_mix(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "Clamp" => legacy_nodes::compile_clamp(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "Smoothstep" => legacy_nodes::compile_smoothstep(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        
        // Unsupported node types
        other => bail!("unsupported material node type: {other}"),
    };
    
    // Cache the result
    cache.insert(key, result.clone());
    Ok(result)
}
