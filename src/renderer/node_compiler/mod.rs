//! Node compiler infrastructure and trait definition.

pub mod attribute;
pub mod color_nodes;
pub mod data_parse;
pub mod geometry_nodes;
pub mod glass_material;
pub mod input_nodes;
pub mod math_closure;
pub mod math_nodes;
pub mod remap_nodes;
pub mod sdf_nodes;
pub mod texture_nodes;
pub mod trigonometry_nodes;
pub mod vector_nodes;

use anyhow::{Result, bail};
use std::collections::HashMap;

use super::types::{MaterialCompileContext, TypedExpr};
use crate::dsl::{Node, SceneDSL, find_node};

/// Main dispatch function for compiling material expressions (fragment stage).
pub fn compile_material_expr(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    compile_expr(
        scene,
        nodes_by_id,
        node_id,
        out_port,
        ctx,
        cache,
        crate::renderer::validation::GlslShaderStage::Fragment,
    )
}

/// Compile an expression intended for the vertex stage.
pub fn compile_vertex_expr(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    compile_expr(
        scene,
        nodes_by_id,
        node_id,
        out_port,
        ctx,
        cache,
        crate::renderer::validation::GlslShaderStage::Vertex,
    )
}

/// Stage-aware node compiler.
fn compile_expr(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    stage: crate::renderer::validation::GlslShaderStage,
) -> Result<TypedExpr> {
    // Check cache first
    let stage_tag = match stage {
        crate::renderer::validation::GlslShaderStage::Vertex => "vs",
        crate::renderer::validation::GlslShaderStage::Fragment => "fs",
        crate::renderer::validation::GlslShaderStage::Compute => "cs",
    };
    let key = (
        node_id.to_string(),
        format!("{stage_tag}:{}", out_port.unwrap_or("value")),
    );
    if let Some(v) = cache.get(&key) {
        return Ok(v.clone());
    }

    let node = find_node(nodes_by_id, node_id)?;

    let compile_fn = |id: &str,
                      port: Option<&str>,
                      ctx: &mut MaterialCompileContext,
                      cache: &mut HashMap<(String, String), TypedExpr>| {
        compile_expr(scene, nodes_by_id, id, port, ctx, cache, stage)
    };

    let result = match node.node_type.as_str() {
        // Input nodes
        "BoolInput" => input_nodes::compile_bool_input(node, out_port)?,
        "ColorInput" => input_nodes::compile_color_input(node, out_port)?,
        "FloatInput" | "IntInput" => input_nodes::compile_float_or_int_input(node, out_port)?,
        "Vector2Input" => input_nodes::compile_vector2_input(node, out_port)?,
        "Vector3Input" => input_nodes::compile_vector3_input(node, out_port)?,
        "FragCoord" => input_nodes::compile_frag_coord(node, out_port)?,
        "GeoFragcoord" => input_nodes::compile_geo_fragcoord(node, out_port)?,
        "GeoSize" => input_nodes::compile_geo_size_for_stage(node, out_port, stage)?,
        "Index" => input_nodes::compile_index(node, out_port, ctx)?,

        // Attribute node
        "Attribute" => attribute::compile_attribute(node, out_port)?,

        // Math nodes
        "MathAdd" => math_nodes::compile_math_add(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "MathMultiply" => math_nodes::compile_math_multiply(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "MathClamp" => math_nodes::compile_math_clamp(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "MathPower" => math_nodes::compile_math_power(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "MathClosure" => math_closure::compile_math_closure(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
            stage,
        )?,

        "Remap" => {
            remap_nodes::compile_remap(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?
        }

        // Texture nodes
        "ImageTexture" => texture_nodes::compile_image_texture(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "PassTexture" => texture_nodes::compile_pass_texture(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,

        // Material nodes
        "GlassMaterial" => glass_material::compile_glass_material(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,

        // Trigonometry nodes
        "Sin" => trigonometry_nodes::compile_sin(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "Cos" => trigonometry_nodes::compile_cos(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "Time" => trigonometry_nodes::compile_time(scene, nodes_by_id, node, out_port, ctx, cache)?,

        // Vector nodes
        "VectorMath" => vector_nodes::compile_vector_math(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "DotProduct" => vector_nodes::compile_dot_product(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "CrossProduct" => vector_nodes::compile_cross_product(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "Normalize" => vector_nodes::compile_normalize(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,

        "Refract" => vector_nodes::compile_refract(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,

        // Color nodes
        "ColorMix" => color_nodes::compile_color_mix(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "ColorRamp" => color_nodes::compile_color_ramp(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "HSVAdjust" => color_nodes::compile_hsv_adjust(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "Luminance" => color_nodes::compile_luminance(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,

        // SDF nodes
        "Sdf2D" => {
            sdf_nodes::compile_sdf2d(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?
        }

        "Sdf2DBevel" => sdf_nodes::compile_sdf2d_bevel(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,

        "DataParse" => data_parse::compile_data_parse(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
            stage,
        )?,

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
    use crate::dsl::{Connection, Metadata, Node, SceneDSL};
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
            groups: Vec::new(),
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
            groups: Vec::new(),
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
