//! Compilers for texture nodes (ImageTexture, CheckerTexture, GradientTexture, NoiseTexture).

use std::collections::HashMap;
use anyhow::{anyhow, bail, Result};

use crate::dsl::{incoming_connection, Node, SceneDSL};
use super::super::types::{TypedExpr, ValueType, MaterialCompileContext};

/// Compile an ImageTexture node.
/// 
/// Samples a texture at a given UV coordinate and returns the color or alpha channel.
/// Automatically flips the V coordinate to match WebGPU's top-left origin.
pub fn compile_image_texture<F>(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: F,
) -> Result<TypedExpr>
where
    F: Fn(&str, Option<&str>, &mut MaterialCompileContext, &mut HashMap<(String, String), TypedExpr>) -> Result<TypedExpr>,
{
    // WGSL is emitted to actually sample a bound texture. The runtime will bind the
    // texture + sampler; for headless tests we only need valid WGSL.
    let _image_index = ctx.register_image_texture(&node.id);

    // If an explicit UV input is provided, respect it; otherwise default to the fragment input uv.
    let uv_expr: TypedExpr = if let Some(conn) = incoming_connection(scene, &node.id, "uv") {
        compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?
    } else {
        TypedExpr::new("in.uv".to_string(), ValueType::Vec2)
    };
    
    if uv_expr.ty != ValueType::Vec2 {
        bail!("ImageTexture.uv must be vector2, got {:?}", uv_expr.ty);
    }

    let tex_var = MaterialCompileContext::tex_var_name(&node.id);
    let samp_var = MaterialCompileContext::sampler_var_name(&node.id);
    
    // WebGPU texture coordinates have (0,0) at the *top-left* of the image.
    // Our synthesized UV (from clip-space position) maps y=-1(bottom)->0 and y=+1(top)->1,
    // so we flip the y axis at sampling time.
    let flipped_uv = format!("vec2f(({}).x, 1.0 - ({}).y)", uv_expr.expr, uv_expr.expr);
    let sample_expr = format!("textureSample({tex_var}, {samp_var}, {flipped_uv})");

    match out_port.unwrap_or("color") {
        "color" => Ok(TypedExpr::with_time(sample_expr, ValueType::Vec4, uv_expr.uses_time)),
        "alpha" => Ok(TypedExpr::with_time(
            format!("({sample_expr}).w"),
            ValueType::F32,
            uv_expr.uses_time,
        )),
        other => bail!("unsupported ImageTexture output port: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mock_compile_fn(
        _node_id: &str,
        _out_port: Option<&str>,
        _ctx: &mut MaterialCompileContext,
        _cache: &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr> {
        // Return a default UV coordinate for testing
        Ok(TypedExpr::new("in.uv".to_string(), ValueType::Vec2))
    }

    #[test]
    fn test_image_texture_default_uv() {
        let scene = SceneDSL {
            nodes: vec![Node {
                id: "img1".to_string(),
                node_type: "ImageTexture".to_string(),
                params: HashMap::new(),
                inputs: Vec::new(),
            }],
            connections: Vec::new(),
            outputs: None,
        };
        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();
        let node = &scene.nodes[0];
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_image_texture(
            &scene,
            &nodes_by_id,
            node,
            Some("color"),
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::Vec4);
        assert!(result.expr.contains("textureSample"));
        assert!(result.expr.contains("tex_img1"));
        assert!(result.expr.contains("sampler_img1"));
        assert!(result.expr.contains("1.0 - (in.uv).y")); // V-flip
        assert!(!result.uses_time);
    }

    #[test]
    fn test_image_texture_alpha_output() {
        let scene = SceneDSL {
            nodes: vec![Node {
                id: "img1".to_string(),
                node_type: "ImageTexture".to_string(),
                params: HashMap::new(),
                inputs: Vec::new(),
            }],
            connections: Vec::new(),
            outputs: None,
        };
        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();
        let node = &scene.nodes[0];
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_image_texture(
            &scene,
            &nodes_by_id,
            node,
            Some("alpha"),
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert!(result.expr.contains(".w")); // Alpha channel
        assert!(!result.uses_time);
    }

    #[test]
    fn test_image_texture_registers_binding() {
        let scene = SceneDSL {
            nodes: vec![Node {
                id: "img1".to_string(),
                node_type: "ImageTexture".to_string(),
                params: HashMap::new(),
                inputs: Vec::new(),
            }],
            connections: Vec::new(),
            outputs: None,
        };
        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();
        let node = &scene.nodes[0];
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        compile_image_texture(
            &scene,
            &nodes_by_id,
            node,
            Some("color"),
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();

        assert_eq!(ctx.image_textures.len(), 1);
        assert_eq!(ctx.image_textures[0], "img1");
    }
}
