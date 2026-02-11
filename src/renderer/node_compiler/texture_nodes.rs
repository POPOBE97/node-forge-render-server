//! Compilers for texture nodes (ImageTexture, CheckerTexture, GradientTexture, NoiseTexture).

use anyhow::{Result, bail};
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use crate::dsl::{Node, SceneDSL, incoming_connection};

/// Compile an ImageTexture node.
///
/// Samples a texture at a given UV coordinate and returns the color or alpha channel.
///
/// Note: This renderer uses a GL-like coordinate system (origin bottom-left). We *do not* flip
/// UVs in WGSL for ImageTexture. If an image source is top-left origin, it must be flipped on
/// upload (CPU-side) so that UV space remains consistent across the graph.
pub fn compile_image_texture<F>(
    scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    out_port: Option<&str>,
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

    // UVs here are already in the renderer's GL-like convention: (0,0) bottom-left.
    let sample_expr = format!("textureSample({tex_var}, {samp_var}, ({}))", uv_expr.expr);

    // If this ImageTexture has straight alpha, wrap the sample in a premultiply operation
    // so downstream consumers always see premultiplied-alpha data.
    let needs_premultiply = ctx.premultiply_image_nodes.contains(&node.id);

    let color_expr = if needs_premultiply {
        // Inject the helper function once (deduplicated by key in extra_wgsl_decls).
        ctx.extra_wgsl_decls
            .entry("nf_premultiply".to_string())
            .or_insert_with(|| {
                "fn nf_premultiply(c: vec4f) -> vec4f {\n    return vec4f(c.rgb * c.a, c.a);\n}\n"
                    .to_string()
            });
        format!("nf_premultiply({sample_expr})")
    } else {
        sample_expr.clone()
    };

    match out_port.unwrap_or("color") {
        "color" => Ok(TypedExpr::with_time(
            color_expr,
            ValueType::Vec4,
            uv_expr.uses_time,
        )),
        // Alpha channel is the same regardless of premultiply.
        "alpha" => Ok(TypedExpr::with_time(
            format!("({sample_expr}).w"),
            ValueType::F32,
            uv_expr.uses_time,
        )),
        "texture" => Ok(TypedExpr::new(node.id.clone(), ValueType::Texture2D)),
        other => bail!("unsupported ImageTexture output port: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::test_scene;
    use super::*;

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
        let scene = test_scene(
            vec![Node {
                id: "img1".to_string(),
                node_type: "ImageTexture".to_string(),
                params: HashMap::new(),
                inputs: Vec::new(),
                input_bindings: Vec::new(),
                outputs: Vec::new(),
            }],
            Vec::new(),
        );
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
        assert!(result.expr.contains("img_tex_img1"));
        assert!(result.expr.contains("img_samp_img1"));
        assert!(!result.expr.contains("1.0 - (in.uv).y"));
        assert!(!result.uses_time);
    }

    #[test]
    fn test_image_texture_alpha_output() {
        let scene = test_scene(
            vec![Node {
                id: "img1".to_string(),
                node_type: "ImageTexture".to_string(),
                params: HashMap::new(),
                inputs: Vec::new(),
                input_bindings: Vec::new(),
                outputs: Vec::new(),
            }],
            Vec::new(),
        );
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
        let scene = test_scene(
            vec![Node {
                id: "img1".to_string(),
                node_type: "ImageTexture".to_string(),
                params: HashMap::new(),
                inputs: Vec::new(),
                input_bindings: Vec::new(),
                outputs: Vec::new(),
            }],
            Vec::new(),
        );
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

/// Compile a PassTexture node.
///
/// Samples the output texture of an upstream pass node for use in material expressions.
/// This enables chain composition where one pass can sample another pass's output.
pub fn compile_pass_texture<F>(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    out_port: Option<&str>,
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
    // Find the upstream pass node connected to the "pass" input.
    let pass_conn = incoming_connection(scene, &node.id, "pass")
        .ok_or_else(|| anyhow::anyhow!("PassTexture.pass input is not connected"))?;

    let upstream_node_id = &pass_conn.from.node_id;
    let upstream_node = nodes_by_id.get(upstream_node_id).ok_or_else(|| {
        anyhow::anyhow!("PassTexture upstream node not found: {}", upstream_node_id)
    })?;

    // Validate that upstream is a pass-producing node.
    if !matches!(
        upstream_node.node_type.as_str(),
        "RenderPass" | "GuassianBlurPass" | "Downsample"
    ) {
        bail!(
            "PassTexture.pass must be connected to a pass node, got {}",
            upstream_node.node_type
        );
    }

    // Register this pass texture for binding.
    let _pass_index = ctx.register_pass_texture(upstream_node_id);

    // If an explicit UV input is provided, use it; otherwise default to fragment input uv.
    let uv_expr: TypedExpr = if let Some(conn) = incoming_connection(scene, &node.id, "uv") {
        compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?
    } else {
        TypedExpr::new("in.uv".to_string(), ValueType::Vec2)
    };

    if uv_expr.ty != ValueType::Vec2 {
        bail!("PassTexture.uv must be vector2, got {:?}", uv_expr.ty);
    }

    let tex_var = MaterialCompileContext::pass_tex_var_name(upstream_node_id);
    let samp_var = MaterialCompileContext::pass_sampler_var_name(upstream_node_id);

    // NOTE: PassTexture is the one intentional exception where we do a fragment-space Y flip.
    // This keeps existing baseline PNGs stable while we migrate the whole graph to a GL-like
    // coordinate system and move image flipping out of WGSL.
    //
    // WGSL texture sampling uses (0,0) at top-left; our renderer's UV convention is bottom-left.
    let flipped_uv = format!("vec2f(({}).x, 1.0 - ({}).y)", uv_expr.expr, uv_expr.expr);
    let sample_expr = format!("textureSample({tex_var}, {samp_var}, {flipped_uv})");

    match out_port.unwrap_or("color") {
        "color" => Ok(TypedExpr::with_time(
            sample_expr,
            ValueType::Vec4,
            uv_expr.uses_time,
        )),
        "alpha" => Ok(TypedExpr::with_time(
            format!("({sample_expr}).w"),
            ValueType::F32,
            uv_expr.uses_time,
        )),
        other => bail!("unsupported PassTexture output port: {other}"),
    }
}

#[cfg(test)]
mod pass_texture_tests {
    use super::super::test_utils::{test_connection, test_scene};
    use super::*;
    use crate::dsl::Connection;

    fn mock_compile_uv(
        _node_id: &str,
        _out_port: Option<&str>,
        _ctx: &mut MaterialCompileContext,
        _cache: &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr> {
        Ok(TypedExpr::new("in.uv".to_string(), ValueType::Vec2))
    }

    fn scene_with_pass_texture() -> (SceneDSL, HashMap<String, Node>, Node) {
        let nodes = vec![
            Node {
                id: "up".to_string(),
                node_type: "RenderPass".to_string(),
                params: HashMap::new(),
                inputs: Vec::new(),
                input_bindings: Vec::new(),
                outputs: Vec::new(),
            },
            Node {
                id: "pt".to_string(),
                node_type: "PassTexture".to_string(),
                params: HashMap::new(),
                inputs: Vec::new(),
                input_bindings: Vec::new(),
                outputs: Vec::new(),
            },
        ];

        let connections: Vec<Connection> = vec![test_connection("up", "pass", "pt", "pass")];
        let scene = test_scene(nodes.clone(), connections);
        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();

        let node = nodes_by_id.get("pt").unwrap().clone();
        (scene, nodes_by_id, node)
    }

    #[test]
    fn test_pass_texture_color_has_v_flip() {
        let (scene, nodes_by_id, node) = scene_with_pass_texture();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_pass_texture(
            &scene,
            &nodes_by_id,
            &node,
            Some("color"),
            &mut ctx,
            &mut cache,
            mock_compile_uv,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::Vec4);
        assert!(result.expr.contains("textureSample"));
        assert!(result.expr.contains("1.0 - (in.uv).y"));
    }

    #[test]
    fn test_pass_texture_alpha_output() {
        let (scene, nodes_by_id, node) = scene_with_pass_texture();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_pass_texture(
            &scene,
            &nodes_by_id,
            &node,
            Some("alpha"),
            &mut ctx,
            &mut cache,
            mock_compile_uv,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert!(result.expr.contains(".w"));
    }
}
