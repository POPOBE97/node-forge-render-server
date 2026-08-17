//! Compilers for texture nodes (ImageTexture, CheckerTexture, GradientTexture, NoiseTexture, Matcap).

use anyhow::{Result, bail};
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use crate::dsl::{Node, SceneDSL, incoming_connection};
use crate::renderer::utils::{coerce_to_type, fmt_f32};

/// Stable key for the aspect-correction WGSL helpers in `extra_wgsl_decls`.
const ASPECT_CORRECT_WGSL_LIB_KEY: &str = "aspect_correct_uv_lib";

fn texture_temp_name(
    ctx: &mut MaterialCompileContext,
    node: &Node,
    out_port: &str,
    suffix: &str,
) -> String {
    super::readable_node_temp_name(ctx, "fs", node, out_port, suffix)
}

/// Ensure the aspect-correction UV helper functions are emitted exactly once.
///
/// Both helpers rescale `uv` around (0.5, 0.5) so the texture preserves its natural pixel
/// aspect ratio when displayed on geometry whose own aspect may differ. The correction
/// factor is `image_aspect / geo_aspect`, accounting for BOTH the bound image's resolution
/// and the geometry's per-fragment pixel size (after instance transforms).
///
/// - `aspect_correct_uv_fit`  — object-fit: contain (image fully visible, empty bands handled
///                              by the configured addressMode).
/// - `aspect_correct_uv_fill` — object-fit: cover   (image fills geometry, sides cropped).
fn ensure_aspect_correct_wgsl_lib(ctx: &mut MaterialCompileContext) {
    if ctx
        .extra_wgsl_decls
        .contains_key(ASPECT_CORRECT_WGSL_LIB_KEY)
    {
        return;
    }

    let wgsl = r#"
fn aspect_correct_uv_fit(uv: vec2f, img_dim: vec2f, geo_dim: vec2f) -> vec2f {
    // r = image_aspect / geo_aspect; r > 1 means image is relatively wider than geometry.
    let r = (img_dim.x * geo_dim.y) / (img_dim.y * geo_dim.x);
    let s = vec2f(max(1.0 / r, 1.0), max(r, 1.0));
    return (uv - vec2f(0.5)) * s + vec2f(0.5);
}
fn aspect_correct_uv_fill(uv: vec2f, img_dim: vec2f, geo_dim: vec2f) -> vec2f {
    let r = (img_dim.x * geo_dim.y) / (img_dim.y * geo_dim.x);
    let s = vec2f(min(1.0 / r, 1.0), min(r, 1.0));
    return (uv - vec2f(0.5)) * s + vec2f(0.5);
}
"#;

    ctx.extra_wgsl_decls
        .insert(ASPECT_CORRECT_WGSL_LIB_KEY.to_string(), wgsl.to_string());
}

/// Compile an ImageTexture node.
///
/// Samples a texture at a given UV coordinate and returns the color or alpha channel.
///
/// Note: This renderer uses a GL-like coordinate system (origin bottom-left). We *do not* flip
/// UVs in WGSL for ImageTexture. If an image source is top-left origin, it must be flipped on
/// upload (CPU-side) so that UV space remains consistent across the graph.
///
/// `aspectCorrection` param controls UV rescaling around (0.5, 0.5) using BOTH the bound
/// texture's runtime resolution (via WGSL `textureDimensions`) AND the geometry's per-fragment
/// pixel size (`in.geo_size_px`, which respects instance transforms). The resulting correction
/// is invariant to non-square geometry — e.g. a 2:1 image on a 2:1 quad needs no correction.
///
/// - `"off"`  (default): legacy behavior, sample UV directly.
/// - `"fit"`:  object-fit: contain — preserves natural aspect, image fully visible.
/// - `"fill"`: object-fit: cover   — preserves natural aspect, fills the geometry.
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
    let port = out_port.unwrap_or("color");

    if !matches!(port, "color" | "alpha" | "texture") {
        bail!("unsupported ImageTexture output port: {port}");
    }

    if port == "texture" {
        return Ok(TypedExpr::new(node.id.clone(), ValueType::Texture2D));
    }

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

    let aspect_mode = node
        .params
        .get("aspectCorrection")
        .and_then(|v| v.as_str())
        .unwrap_or("off");

    let sample_uv = match aspect_mode {
        "fit" => {
            ensure_aspect_correct_wgsl_lib(ctx);
            format!(
                "aspect_correct_uv_fit(({}), vec2f(textureDimensions({tex_var})), in.geo_size_px)",
                uv_expr.expr
            )
        }
        "fill" => {
            ensure_aspect_correct_wgsl_lib(ctx);
            format!(
                "aspect_correct_uv_fill(({}), vec2f(textureDimensions({tex_var})), in.geo_size_px)",
                uv_expr.expr
            )
        }
        _ => format!("({})", uv_expr.expr),
    };

    let sample_uv = match aspect_mode {
        "fit" | "fill" => {
            let uv_var = texture_temp_name(ctx, node, port, "uv");
            super::push_readable_let(
                ctx,
                format!("ImageTexture {} aspect-correct uv", node.id),
                &uv_var,
                &sample_uv,
            );
            uv_var
        }
        _ => sample_uv,
    };

    // UVs here are already in the renderer's GL-like convention: (0,0) bottom-left.
    let sample_expr = format!("textureSample({tex_var}, {samp_var}, {sample_uv})");
    let sample_var = texture_temp_name(ctx, node, port, "sample");

    match port {
        "color" => {
            super::push_readable_let(
                ctx,
                format!("ImageTexture {}.color", node.id),
                &sample_var,
                &sample_expr,
            );
            Ok(TypedExpr::with_time(
                sample_var,
                ValueType::Vec4,
                uv_expr.uses_time,
            ))
        }
        "alpha" => {
            super::push_readable_let(
                ctx,
                format!("ImageTexture {}.alpha", node.id),
                &sample_var,
                &sample_expr,
            );
            Ok(TypedExpr::with_time(
                format!("({sample_var}).w"),
                ValueType::F32,
                uv_expr.uses_time,
            ))
        }
        _ => unreachable!("ImageTexture port validated above"),
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
                wgsl_override: None,
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
        assert_eq!(result.expr, "image_texture_sample");
        let stmts = ctx.inline_stmts.join("\n");
        assert!(stmts.contains("textureSample"));
        assert!(stmts.contains("img_tex_img1"));
        assert!(stmts.contains("img_samp_img1"));
        assert!(!stmts.contains("1.0 - (in.uv).y"));
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
                wgsl_override: None,
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
                wgsl_override: None,
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
        assert_eq!(ctx.image_textures[0].image_node_id, "img1");
    }

    fn aspect_scene(mode: Option<&str>) -> SceneDSL {
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        if let Some(m) = mode {
            params.insert(
                "aspectCorrection".to_string(),
                serde_json::Value::String(m.to_string()),
            );
        }
        test_scene(
            vec![Node {
                id: "img1".to_string(),
                node_type: "ImageTexture".to_string(),
                params,
                inputs: Vec::new(),
                input_bindings: Vec::new(),
                outputs: Vec::new(),
                wgsl_override: None,
            }],
            Vec::new(),
        )
    }

    #[test]
    fn test_image_texture_aspect_off_default() {
        // With no aspectCorrection param (or "off"), behavior matches legacy: no helper emitted,
        // no textureDimensions call, raw UV passed to textureSample.
        for mode in [None, Some("off")] {
            let scene = aspect_scene(mode);
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

            assert!(!result.expr.contains("aspect_correct_uv_"));
            assert!(!result.expr.contains("textureDimensions"));
            let stmts = ctx.inline_stmts.join("\n");
            assert!(!stmts.contains("aspect_correct_uv_"));
            assert!(!stmts.contains("textureDimensions"));
            assert!(
                !ctx.extra_wgsl_decls
                    .contains_key(ASPECT_CORRECT_WGSL_LIB_KEY)
            );
        }
    }

    #[test]
    fn test_image_texture_aspect_fit() {
        let scene = aspect_scene(Some("fit"));
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

        assert_eq!(result.expr, "image_texture_sample");
        let stmts = ctx.inline_stmts.join("\n");
        assert!(stmts.contains("aspect_correct_uv_fit("));
        assert!(stmts.contains("textureDimensions(img_tex_img1)"));
        assert!(stmts.contains("in.geo_size_px"));
        assert!(stmts.contains("textureSample"));

        let lib = ctx
            .extra_wgsl_decls
            .get(ASPECT_CORRECT_WGSL_LIB_KEY)
            .expect("aspect_correct_uv_lib must be emitted");
        assert!(lib.contains("aspect_correct_uv_fit"));
        // fit uses max() to expand UV (letterbox / contain).
        assert!(lib.contains("max(1.0 / r, 1.0)"));
    }

    #[test]
    fn test_image_texture_aspect_fill() {
        let scene = aspect_scene(Some("fill"));
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

        assert_eq!(result.expr, "image_texture_sample");
        let stmts = ctx.inline_stmts.join("\n");
        assert!(stmts.contains("aspect_correct_uv_fill("));
        assert!(stmts.contains("textureDimensions(img_tex_img1)"));
        assert!(stmts.contains("in.geo_size_px"));

        let lib = ctx
            .extra_wgsl_decls
            .get(ASPECT_CORRECT_WGSL_LIB_KEY)
            .expect("aspect_correct_uv_lib must be emitted");
        assert!(lib.contains("aspect_correct_uv_fill"));
        // fill uses min() to compress UV (cover / crop).
        assert!(lib.contains("min(1.0 / r, 1.0)"));
    }
}
