//! Compilers for texture nodes (ImageTexture, CheckerTexture, GradientTexture, NoiseTexture, Matcap).

use anyhow::{Result, bail};
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use crate::dsl::{Node, SceneDSL, incoming_connection};
use crate::renderer::utils::{coerce_to_type, fmt_f32};

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
        "texture" => Ok(TypedExpr::new(node.id.clone(), ValueType::Texture2D)),
        other => bail!("unsupported ImageTexture output port: {other}"),
    }
}

// ---------------------------------------------------------------------------
// Matcap
// ---------------------------------------------------------------------------

/// Stable key for the matcap WGSL helper library in `extra_wgsl_decls`.
const MATCAP_WGSL_LIB_KEY: &str = "matcap_lib";

/// Ensure the matcap UV helper function is emitted exactly once.
fn ensure_matcap_wgsl_lib(ctx: &mut MaterialCompileContext) {
    if ctx.extra_wgsl_decls.contains_key(MATCAP_WGSL_LIB_KEY) {
        return;
    }

    let wgsl = r#"
fn matcap_uv(n: vec3f, v: vec3f) -> vec2f {
    let N = normalize(n);
    let V = normalize(v);
    let x_axis = normalize(vec3f(V.z, 0.0, -V.x));
    let y_axis = normalize(cross(V, x_axis));
    let uv = vec2f(dot(N, x_axis), dot(N, y_axis)) * 0.5 + 0.5;
    return clamp(uv, vec2f(0.0), vec2f(1.0));
}
"#;

    ctx.extra_wgsl_decls
        .insert(MATCAP_WGSL_LIB_KEY.to_string(), wgsl.to_string());
}

/// Parse a vec3 default from a `{x, y, z}` JSON object in `node.params[key]`.
fn param_vec3_literal(node: &Node, key: &str, default: [f32; 3]) -> String {
    if let Some(obj) = node.params.get(key).and_then(|v| v.as_object()) {
        let x = obj
            .get("x")
            .and_then(|v| v.as_f64())
            .unwrap_or(default[0] as f64) as f32;
        let y = obj
            .get("y")
            .and_then(|v| v.as_f64())
            .unwrap_or(default[1] as f64) as f32;
        let z = obj
            .get("z")
            .and_then(|v| v.as_f64())
            .unwrap_or(default[2] as f64) as f32;
        format!("vec3f({}, {}, {})", fmt_f32(x), fmt_f32(y), fmt_f32(z))
    } else {
        format!(
            "vec3f({}, {}, {})",
            fmt_f32(default[0]),
            fmt_f32(default[1]),
            fmt_f32(default[2])
        )
    }
}

/// Compile a Matcap node.
///
/// Samples a matcap texture using view and normal vectors to compute UV coordinates.
/// The matcap UV is derived by projecting the normal into a screen-aligned basis
/// built from the view direction.
///
/// # Inputs
/// - `image`: ImageFile texture (required — registered as image texture binding)
/// - `normal`: vec3 normal direction (optional, default `(0, 0, 1)`)
/// - `view`: vec3 view direction (optional, default `(0, 0, 1)`)
///
/// # Output
/// - `color`: vec4 sampled matcap color
pub fn compile_matcap<F>(
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
    let port = out_port.unwrap_or("color");
    if port != "color" {
        bail!("Matcap: unsupported output port '{port}'");
    }

    // Register the image texture binding (same mechanism as ImageTexture).
    let _image_index = ctx.register_image_texture(&node.id);

    // Emit the matcap UV helper function.
    ensure_matcap_wgsl_lib(ctx);

    // --- normal input ---
    let normal_expr = if let Some(conn) = incoming_connection(scene, &node.id, "normal") {
        let raw = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
        coerce_to_type(raw, ValueType::Vec3)?
    } else {
        let lit = param_vec3_literal(node, "normal", [0.0, 0.0, 1.0]);
        TypedExpr::new(lit, ValueType::Vec3)
    };

    // --- view input ---
    let view_expr = if let Some(conn) = incoming_connection(scene, &node.id, "view") {
        let raw = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
        coerce_to_type(raw, ValueType::Vec3)?
    } else {
        let lit = param_vec3_literal(node, "view", [0.0, 0.0, 1.0]);
        TypedExpr::new(lit, ValueType::Vec3)
    };

    let tex_var = MaterialCompileContext::tex_var_name(&node.id);
    let samp_var = MaterialCompileContext::sampler_var_name(&node.id);

    let sample_expr = format!(
        "textureSample({tex_var}, {samp_var}, matcap_uv({}, {}))",
        normal_expr.expr, view_expr.expr
    );

    Ok(TypedExpr::with_time(
        sample_expr,
        ValueType::Vec4,
        normal_expr.uses_time || view_expr.uses_time,
    ))
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
        "RenderPass"
            | "BloomNode"
            | "GuassianBlurPass"
            | "Downsample"
            | "Upsample"
            | "GradientBlur"
            | "Composite"
    ) {
        bail!(
            "PassTexture.pass must be connected to a pass node, got {}",
            upstream_node.node_type
        );
    }

    // Register this pass texture for binding.
    let _pass_index = ctx.register_pass_texture(upstream_node_id);

    // If an explicit UV input is provided, treat it as user-facing UV semantics
    // (bottom-left origin) and convert to texture-sampling UV space (top-left origin).
    //
    // If no UV input is connected, use `in.uv` directly (already top-left origin).
    let (uv_expr, has_explicit_uv): (TypedExpr, bool) =
        if let Some(conn) = incoming_connection(scene, &node.id, "uv") {
            (
                compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?,
                true,
            )
        } else {
            (TypedExpr::new("in.uv".to_string(), ValueType::Vec2), false)
        };

    if uv_expr.ty != ValueType::Vec2 {
        bail!("PassTexture.uv must be vector2, got {:?}", uv_expr.ty);
    }

    let tex_var = MaterialCompileContext::pass_tex_var_name(upstream_node_id);
    let samp_var = MaterialCompileContext::pass_sampler_var_name(upstream_node_id);

    let sample_uv_expr = if has_explicit_uv {
        format!("vec2f(({}).x, 1.0 - ({}).y)", uv_expr.expr, uv_expr.expr)
    } else {
        uv_expr.expr.clone()
    };
    let sample_expr = format!("textureSample({tex_var}, {samp_var}, {sample_uv_expr})");

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
    fn test_pass_texture_color_sampling_uses_uv() {
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
        assert!(result.expr.contains("in.uv"));
        assert!(!result.expr.contains("1.0 -"));
    }

    fn mock_compile_uv_user_space(
        _node_id: &str,
        _out_port: Option<&str>,
        _ctx: &mut MaterialCompileContext,
        _cache: &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr> {
        Ok(TypedExpr::new("user_uv".to_string(), ValueType::Vec2))
    }

    #[test]
    fn test_pass_texture_explicit_uv_flips_y_for_sampling_space() {
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
                id: "uvsrc".to_string(),
                node_type: "MathClosure".to_string(),
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

        let connections: Vec<Connection> = vec![
            test_connection("up", "pass", "pt", "pass"),
            test_connection("uvsrc", "output", "pt", "uv"),
        ];

        let scene = test_scene(nodes, connections);
        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();

        let node = nodes_by_id.get("pt").unwrap().clone();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_pass_texture(
            &scene,
            &nodes_by_id,
            &node,
            Some("color"),
            &mut ctx,
            &mut cache,
            mock_compile_uv_user_space,
        )
        .unwrap();

        assert!(
            result
                .expr
                .contains("vec2f((user_uv).x, 1.0 - (user_uv).y)")
        );
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
