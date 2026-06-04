//! Node compiler infrastructure and trait definition.

pub mod attribute;
pub mod color_nodes;
pub mod data_parse;
pub mod geometry_nodes;
pub mod glass_material;
pub mod input_nodes;
pub mod luminance_curve;
pub mod math_closure;
pub mod math_nodes;
pub mod remap_nodes;
pub mod sdf_nodes;
pub mod template_loader;
pub mod texture_nodes;
pub mod trigonometry_nodes;
pub mod vector_nodes;

use anyhow::{Result, bail};
use std::collections::HashMap;

use super::types::{ExprEmitPolicy, MaterialCompileContext, TypedExpr, ValueType};
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

fn stable_temp_hash(parts: &[&str]) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    for part in parts {
        for byte in part.as_bytes().iter().copied().chain([0xff]) {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(0x01000193);
        }
    }
    hash
}

fn readable_temp_name(stage_tag: &str, node_id: &str, out_port: &str) -> String {
    let node = crate::renderer::utils::sanitize_wgsl_ident(node_id);
    let port = crate::renderer::utils::sanitize_wgsl_ident(out_port);
    let hash = stable_temp_hash(&[stage_tag, node_id, out_port]);
    format!("nf_{stage_tag}_{node}_{port}_{hash:08x}")
}

fn is_simple_wgsl_expr(expr: &str) -> bool {
    let expr = expr.trim();
    if expr.is_empty() || expr.len() > 96 {
        return false;
    }

    if expr
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return true;
    }

    // Common graph input scalar/vector component form: `(graph_inputs.foo).x`.
    expr.starts_with("(graph_inputs.")
        && expr
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '(' | ')'))
}

fn is_input_like_node(node_type: &str) -> bool {
    matches!(
        node_type,
        "BoolInput"
            | "ColorInput"
            | "FloatInput"
            | "IntInput"
            | "Vector2Input"
            | "Vector3Input"
            | "Vector4Input"
            | "TimeInput"
            | "Time"
            | "FragCoord"
            | "GeoFragcoord"
            | "GeoSize"
            | "Index"
            | "Attribute"
            | "ResourcePool"
    )
}

fn split_top_level_args(args: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;

    for (idx, ch) in args.char_indices() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            '[' => bracket_depth += 1,
            ']' => bracket_depth -= 1,
            ',' if paren_depth == 0 && bracket_depth == 0 => {
                out.push(args[start..idx].trim().to_string());
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
        if paren_depth < 0 || bracket_depth < 0 {
            return None;
        }
    }

    if paren_depth != 0 || bracket_depth != 0 {
        return None;
    }

    out.push(args[start..].trim().to_string());
    if out.iter().any(|arg| arg.is_empty()) {
        return None;
    }
    Some(out)
}

fn find_matching_paren(expr: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (rel_idx, ch) in expr[open_idx..].char_indices() {
        let idx = open_idx + rel_idx;
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
        if depth < 0 {
            return None;
        }
    }
    None
}

fn strip_wrapping_parens(expr: &str) -> Option<&str> {
    let expr = expr.trim();
    if !expr.starts_with('(') || !expr.ends_with(')') {
        return None;
    }
    let close_idx = find_matching_paren(expr, 0)?;
    if close_idx != expr.len() - 1 {
        return None;
    }
    Some(expr[1..close_idx].trim())
}

fn format_readable_call_expr(expr: &str) -> Option<String> {
    let open_idx = expr.find('(')?;
    let callee = expr[..open_idx].trim();
    if callee.is_empty()
        || !callee
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }

    let close_idx = find_matching_paren(expr, open_idx)?;
    let suffix = expr[close_idx + 1..].trim();
    if !suffix.is_empty()
        && (!suffix.starts_with('.')
            || !suffix[1..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        return None;
    }

    let args_src = &expr[open_idx + 1..close_idx];
    let args = split_top_level_args(args_src)?;
    if args.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str(callee);
    out.push_str("(\n");
    for arg in args {
        out.push_str("        ");
        out.push_str(&arg);
        out.push_str(",\n");
    }
    out.push_str("    )");
    out.push_str(suffix);
    Some(out)
}

fn split_top_level_binary(expr: &str) -> Option<(&str, char, &str)> {
    let inner = strip_wrapping_parens(expr)?;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut found: Option<(usize, char)> = None;

    for (idx, ch) in inner.char_indices() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            '[' => bracket_depth += 1,
            ']' => bracket_depth -= 1,
            '+' | '-' | '*' | '/' if paren_depth == 0 && bracket_depth == 0 => {
                let before = inner[..idx].chars().next_back();
                let after_idx = idx + ch.len_utf8();
                let after = inner[after_idx..].chars().next();
                if before == Some(' ') && after == Some(' ') {
                    if found.is_some() {
                        return None;
                    }
                    found = Some((idx, ch));
                }
            }
            _ => {}
        }
        if paren_depth < 0 || bracket_depth < 0 {
            return None;
        }
    }

    if paren_depth != 0 || bracket_depth != 0 {
        return None;
    }

    let (idx, op) = found?;
    let lhs = inner[..idx].trim();
    let rhs = inner[idx + op.len_utf8()..].trim();
    if lhs.is_empty() || rhs.is_empty() {
        return None;
    }
    Some((lhs, op, rhs))
}

fn format_readable_binary_expr(expr: &str) -> Option<String> {
    let (lhs, op, rhs) = split_top_level_binary(expr)?;
    Some(format!("(\n        {lhs}\n        {op} {rhs}\n    )"))
}

fn format_readable_temp_expr(temp_name: &str, expr: &str) -> String {
    const MAX_ASSIGNMENT_LINE: usize = 120;

    let expr = expr.trim();
    if "    let ".len() + temp_name.len() + " = ".len() + expr.len() + 1 <= MAX_ASSIGNMENT_LINE {
        return expr.to_string();
    }

    format_readable_call_expr(expr)
        .or_else(|| format_readable_binary_expr(expr))
        .unwrap_or_else(|| expr.to_string())
}

pub(super) fn readable_let_stmt(temp_name: &str, expr: &str) -> String {
    let expr = format_readable_temp_expr(temp_name, expr);
    format!("    let {temp_name} = {expr};")
}

pub(super) fn push_readable_let(
    ctx: &mut MaterialCompileContext,
    comment: impl AsRef<str>,
    temp_name: &str,
    expr: &str,
) {
    ctx.inline_stmts.push(format!(
        "    // {}\n{}",
        comment.as_ref(),
        readable_let_stmt(temp_name, expr)
    ));
}

fn should_emit_readable_temp(
    stage: crate::renderer::validation::GlslShaderStage,
    ctx: &MaterialCompileContext,
    node: &Node,
    result: &TypedExpr,
) -> bool {
    if !matches!(
        stage,
        crate::renderer::validation::GlslShaderStage::Fragment
    ) {
        return false;
    }
    if ctx.auto_temp_suppression_depth > 0 {
        return false;
    }
    if result.emit_policy == ExprEmitPolicy::Inline {
        return false;
    }
    if result.ty == ValueType::Texture2D {
        return false;
    }
    if is_simple_wgsl_expr(&result.expr) {
        return false;
    }
    if is_input_like_node(&node.node_type) {
        return false;
    }

    true
}

fn emit_readable_temp_if_needed(
    stage_tag: &str,
    stage: crate::renderer::validation::GlslShaderStage,
    node: &Node,
    out_port: &str,
    ctx: &mut MaterialCompileContext,
    result: TypedExpr,
) -> TypedExpr {
    if !should_emit_readable_temp(stage, ctx, node, &result) {
        return result;
    }

    let temp_name = readable_temp_name(stage_tag, &node.id, out_port);
    push_readable_let(
        ctx,
        format!("{} {}.{}", node.node_type, node.id, out_port),
        &temp_name,
        &result.expr,
    );

    TypedExpr::with_time(temp_name, result.ty, result.uses_time)
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
    let out_port_name = out_port.unwrap_or("value");
    let key = (node_id.to_string(), format!("{stage_tag}:{out_port_name}"));
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
        "BoolInput" => input_nodes::compile_bool_input(node, out_port, ctx)?,
        "ColorInput" => input_nodes::compile_color_input(node, out_port, ctx)?,
        "FloatInput" | "IntInput" => input_nodes::compile_float_or_int_input(node, out_port, ctx)?,
        "Vector2Input" => input_nodes::compile_vector2_input(node, out_port, ctx)?,
        "Vector3Input" => input_nodes::compile_vector3_input(node, out_port, ctx)?,
        "Vector4Input" => input_nodes::compile_vector4_input(node, out_port, ctx)?,
        "TimeInput" => input_nodes::compile_time_input(node, out_port)?,
        "FragCoord" => input_nodes::compile_frag_coord(node, out_port)?,
        "GeoFragcoord" => input_nodes::compile_geo_fragcoord(node, out_port)?,
        "GeoSize" => input_nodes::compile_geo_size_for_stage(node, out_port, stage)?,
        "Index" => input_nodes::compile_index(node, out_port, ctx)?,
        "ResourcePool" => input_nodes::compile_resource_pool(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,

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
        "MathSubtract" => math_nodes::compile_math_subtract(
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
        "MathDivide" => math_nodes::compile_math_divide(
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
        "MathMax" => math_nodes::compile_math_max(
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
        "Lerp" => {
            math_nodes::compile_lerp(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?
        }
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
        "Matcap" => texture_nodes::compile_matcap(
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
        "Tan" => trigonometry_nodes::compile_tan(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "Asin" => trigonometry_nodes::compile_asin(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "Acos" => trigonometry_nodes::compile_acos(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "Atan" => trigonometry_nodes::compile_atan(
            scene,
            nodes_by_id,
            node,
            out_port,
            ctx,
            cache,
            compile_fn,
        )?,
        "Atan2" => trigonometry_nodes::compile_atan2(
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

        "ViewVector" => vector_nodes::compile_view_vector(node, out_port, ctx)?,

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
        "LuminanceCurve" => luminance_curve::compile_luminance_curve(
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

    let result = emit_readable_temp_if_needed(stage_tag, stage, node, out_port_name, ctx, result);

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
            assets: Default::default(),
            state_machine: None,
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
            assets: Default::default(),
            state_machine: None,
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
