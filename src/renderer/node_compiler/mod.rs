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
use crate::dsl::{Connection, Endpoint, Metadata, Node, SceneDSL, find_node, incoming_connection};
use crate::renderer::utils::readable_wgsl_ident;

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
    let prev = ctx.preserve_legacy_graph_input_names;
    ctx.preserve_legacy_graph_input_names = true;
    let result = compile_expr(
        scene,
        nodes_by_id,
        node_id,
        out_port,
        ctx,
        cache,
        crate::renderer::validation::GlslShaderStage::Vertex,
    );
    ctx.preserve_legacy_graph_input_names = prev;
    result
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

fn display_param<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
    node.params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn default_label_for_node_type(node_type: &str) -> String {
    node_type.to_string()
}

fn meaningful_node_label(node: &Node) -> Option<&str> {
    let label = display_param(node, "label")?;
    let readable_label = readable_wgsl_ident(label);
    let readable_type = readable_wgsl_ident(&default_label_for_node_type(&node.node_type));
    if readable_label == readable_type {
        return None;
    }
    match readable_label.as_str() {
        "multiply" | "add" | "subtract" | "divide" | "math" | "math_closure" | "image_texture"
        | "pass_texture" | "vector_math" | "remap" | "luminance" | "normalize" | "refract"
        | "dot_product" | "cross_product" | "color_mix" | "float_input" | "vector2_input"
        | "vector3_input" | "vector4_input" | "color_input" | "bool_input" | "int_input" => None,
        _ => Some(label),
    }
}

pub(super) fn readable_symbol_base(node: &Node, out_port: &str) -> String {
    let group_output_name = display_param(node, "__group_output_name").filter(|name| {
        !matches!(
            readable_wgsl_ident(name).as_str(),
            "output" | "result" | "value"
        )
    });
    let raw = group_output_name
        .or_else(|| meaningful_node_label(node))
        .or_else(|| display_param(node, "__group_input_name"))
        .unwrap_or_else(|| node.node_type.as_str());
    let fallback_label;
    let raw = if raw == node.node_type {
        fallback_label = default_label_for_node_type(&node.node_type);
        fallback_label.as_str()
    } else {
        raw
    };
    let mut base = readable_wgsl_ident(raw);
    let port = readable_wgsl_ident(out_port);
    if !matches!(port.as_str(), "result" | "value" | "output" | "color") {
        base.push('_');
        base.push_str(&port);
    }
    base
}

pub(super) fn readable_node_temp_name(
    ctx: &mut MaterialCompileContext,
    stage_tag: &str,
    node: &Node,
    out_port: &str,
    suffix: &str,
) -> String {
    let mut base = readable_symbol_base(node, out_port);
    let suffix = readable_wgsl_ident(suffix);
    if !suffix.is_empty() && suffix != "result" && suffix != "value" && suffix != "output" {
        base.push('_');
        base.push_str(&suffix);
    }
    ctx.allocate_local_name(
        &format!("{stage_tag}:{}:{out_port}:{suffix}", node.id),
        &base,
    )
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

fn outgoing_value_fanout(scene: &SceneDSL, node_id: &str, out_port: &str) -> usize {
    scene
        .connections
        .iter()
        .filter(|c| c.from.node_id == node_id && c.from.port_id == out_port)
        .count()
}

fn has_expensive_or_effect_like_call(expr: &str) -> bool {
    expr.contains("textureSample(")
        || expr.contains("textureLoad(")
        || expr.contains("textureDimensions(")
        || expr.contains("dFdx(")
        || expr.contains("dFdy(")
}

fn node_string_param<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
    node.params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn expanded_instance_id(node_id: &str) -> Option<&str> {
    node_id.split_once('/').map(|(instance_id, _)| instance_id)
}

fn value_type_from_port_type(port_type: Option<&str>) -> Option<ValueType> {
    match port_type.unwrap_or("float") {
        "float" | "f32" | "number" => Some(ValueType::F32),
        "int" | "i32" => Some(ValueType::I32),
        "uint" | "u32" => Some(ValueType::U32),
        "bool" | "boolean" => Some(ValueType::Bool),
        "vector2" | "vec2" => Some(ValueType::Vec2),
        "vector3" | "vec3" => Some(ValueType::Vec3),
        "vector4" | "vec4" | "color" => Some(ValueType::Vec4),
        _ => None,
    }
}

fn default_expr_for_type(ty: ValueType) -> TypedExpr {
    match ty {
        ValueType::F32 => TypedExpr::new("0.0", ValueType::F32),
        ValueType::I32 => TypedExpr::new("0", ValueType::I32),
        ValueType::U32 => TypedExpr::new("0u", ValueType::U32),
        ValueType::Bool => TypedExpr::new("false", ValueType::Bool),
        ValueType::Vec2 => TypedExpr::new("vec2f(0.0, 0.0)", ValueType::Vec2),
        ValueType::Vec3 => TypedExpr::new("vec3f(0.0, 0.0, 0.0)", ValueType::Vec3),
        ValueType::Vec4 => TypedExpr::new("vec4f(0.0, 0.0, 0.0, 0.0)", ValueType::Vec4),
        _ => TypedExpr::new("0.0", ValueType::F32),
    }
}

fn is_pure_group_node_type(node_type: &str) -> bool {
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
            | "MathAdd"
            | "MathSubtract"
            | "MathMultiply"
            | "MathDivide"
            | "MathClamp"
            | "MathMax"
            | "MathPower"
            | "Lerp"
            | "MathClosure"
            | "Remap"
            | "Sin"
            | "Cos"
            | "Tan"
            | "Asin"
            | "Acos"
            | "Atan"
            | "Atan2"
            | "VectorMath"
            | "DotProduct"
            | "CrossProduct"
            | "Normalize"
            | "Refract"
            | "ColorMix"
            | "ColorRamp"
            | "HSVAdjust"
            | "Luminance"
    )
}

fn is_pure_expression_group(group: &crate::dsl::GroupDSL) -> bool {
    group
        .nodes
        .iter()
        .all(|node| is_pure_group_node_type(&node.node_type))
}

fn group_function_name(
    ctx: &mut MaterialCompileContext,
    group_id: &str,
    group_port_id: &str,
    group_name: Option<&str>,
) -> String {
    let raw = group_name
        .filter(|s| !matches!(readable_wgsl_ident(s).as_str(), "group" | "node_group"))
        .unwrap_or(group_id);
    let base = readable_wgsl_ident(raw);
    ctx.allocate_local_name(&format!("group_fn:{group_id}:{group_port_id}"), &base)
}

fn group_display_name<'a>(group_id: &'a str, group_name: Option<&'a str>) -> &'a str {
    group_name
        .filter(|s| !matches!(readable_wgsl_ident(s).as_str(), "group" | "node_group"))
        .unwrap_or(group_id)
}

fn try_compile_pure_group_call(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    out_port: &str,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    stage: crate::renderer::validation::GlslShaderStage,
) -> Result<Option<TypedExpr>> {
    if !matches!(
        stage,
        crate::renderer::validation::GlslShaderStage::Fragment
    ) {
        return Ok(None);
    }

    let Some(instance_id) = expanded_instance_id(&node.id) else {
        return Ok(None);
    };
    let Some(group_id) = node_string_param(node, "__dedup_group_id")
        .or_else(|| node_string_param(node, "__group_id"))
    else {
        return Ok(None);
    };
    let Some(original_id) = node_string_param(node, "__dedup_original_id") else {
        return Ok(None);
    };
    let Some(group) = scene.groups.iter().find(|g| g.id == group_id) else {
        return Ok(None);
    };
    if !is_pure_expression_group(group) {
        return Ok(None);
    }

    let Some(output_binding) = group
        .output_bindings
        .iter()
        .find(|b| b.from.node_id == original_id && b.from.port_id == out_port)
    else {
        return Ok(None);
    };

    let mut call_args = Vec::new();
    let mut helper_params = Vec::new();
    let expanded_node_id = |local_id: &str| format!("{instance_id}/{local_id}");
    let mut helper_connections = group
        .connections
        .iter()
        .cloned()
        .map(|mut conn| {
            conn.from.node_id = expanded_node_id(&conn.from.node_id);
            conn.to.node_id = expanded_node_id(&conn.to.node_id);
            conn
        })
        .collect::<Vec<_>>();
    let mut helper_ctx = MaterialCompileContext::default();
    let mut helper_cache = HashMap::new();

    for input_port in &group.inputs {
        let Some(ty) = value_type_from_port_type(input_port.port_type.as_deref()) else {
            return Ok(None);
        };
        let raw_name = input_port
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(input_port.id.as_str());
        let param_name = helper_ctx.allocate_local_name(
            &format!("group_param:{group_id}:{}", input_port.id),
            &readable_wgsl_ident(raw_name),
        );
        helper_params.push(format!("{}: {}", param_name, ty.wgsl_type_string()));

        let mut call_expr = default_expr_for_type(ty);
        for binding in group
            .input_bindings
            .iter()
            .filter(|b| b.group_port_id == input_port.id)
        {
            let param_source_id = format!("__group_param_{}", input_port.id);
            let expanded_target_id = expanded_node_id(&binding.to.node_id);
            if let Some(conn) = incoming_connection(scene, &expanded_target_id, &binding.to.port_id)
            {
                call_expr = compile_expr(
                    scene,
                    nodes_by_id,
                    &conn.from.node_id,
                    Some(&conn.from.port_id),
                    ctx,
                    cache,
                    stage,
                )?;
            }
            helper_connections.push(Connection {
                id: format!(
                    "__group_param_edge_{}_{}_{}",
                    input_port.id, binding.to.node_id, binding.to.port_id
                ),
                from: Endpoint {
                    node_id: param_source_id.clone(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: expanded_target_id.clone(),
                    port_id: binding.to.port_id.clone(),
                },
            });
            helper_ctx.expr_overrides.insert(
                (param_source_id, "value".to_string()),
                TypedExpr::new(param_name.clone(), ty),
            );
            helper_ctx.expr_overrides.insert(
                (expanded_target_id, binding.to.port_id.clone()),
                TypedExpr::new(param_name.clone(), ty),
            );
        }
        call_args.push(call_expr);
    }

    helper_ctx.graph_input_kinds = ctx.graph_input_kinds.clone();
    helper_ctx.graph_input_field_names = ctx.graph_input_field_names.clone();
    helper_ctx.used_graph_input_field_names = ctx.used_graph_input_field_names.clone();

    let helper_nodes = group
        .nodes
        .iter()
        .map(|group_node| {
            let expanded_id = expanded_node_id(&group_node.id);
            nodes_by_id.get(&expanded_id).cloned().unwrap_or_else(|| {
                let mut node = group_node.clone();
                node.id = expanded_id;
                node
            })
        })
        .collect::<Vec<_>>();
    let helper_scene = SceneDSL {
        version: scene.version.clone(),
        metadata: Metadata {
            name: format!("group:{group_id}"),
            created: None,
            modified: None,
        },
        nodes: helper_nodes,
        connections: helper_connections,
        outputs: None,
        groups: Vec::new(),
        assets: scene.assets.clone(),
        state_machine: None,
        debug_artifacts: None,
    };
    let helper_nodes_by_id: HashMap<String, Node> = helper_scene
        .nodes
        .iter()
        .cloned()
        .map(|n| (n.id.clone(), n))
        .collect();

    let helper_output_node_id = expanded_node_id(&output_binding.from.node_id);
    let helper_result = compile_expr(
        &helper_scene,
        &helper_nodes_by_id,
        &helper_output_node_id,
        Some(&output_binding.from.port_id),
        &mut helper_ctx,
        &mut helper_cache,
        stage,
    )?;

    if !helper_ctx.image_textures.is_empty() || !helper_ctx.pass_textures.is_empty() {
        return Ok(None);
    }

    ctx.graph_input_kinds = helper_ctx.graph_input_kinds.clone();
    ctx.graph_input_field_names = helper_ctx.graph_input_field_names.clone();
    ctx.used_graph_input_field_names = helper_ctx.used_graph_input_field_names.clone();

    let fn_name = group_function_name(
        ctx,
        group_id,
        &output_binding.group_port_id,
        group.name.as_deref(),
    );
    let decl_key = format!("group_fn:{group_id}:{}", output_binding.group_port_id);
    if !ctx.extra_wgsl_decls.contains_key(&decl_key) {
        for (key, decl) in helper_ctx.extra_wgsl_decls {
            ctx.extra_wgsl_decls.entry(key).or_insert(decl);
        }

        let mut body = String::new();
        body.push_str(&format!(
            "// Group: {}\nfn {fn_name}({}) -> {} {{\n",
            group_display_name(group_id, group.name.as_deref()),
            helper_params.join(", "),
            helper_result.ty.wgsl_type_string()
        ));
        if !helper_ctx.inline_stmts.is_empty() {
            body.push_str(&helper_ctx.inline_stmts.join("\n"));
            body.push('\n');
        }
        body.push_str(&format!("    return {};\n}}\n", helper_result.expr));
        ctx.extra_wgsl_decls.insert(decl_key, body);
    }

    let uses_time = call_args.iter().any(|arg| arg.uses_time);
    let args = call_args
        .into_iter()
        .map(|arg| arg.expr)
        .collect::<Vec<_>>()
        .join(", ");
    Ok(Some(TypedExpr::with_time(
        format!("{fn_name}({args})"),
        helper_result.ty,
        uses_time || helper_result.uses_time,
    )))
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
    scene: &SceneDSL,
    stage: crate::renderer::validation::GlslShaderStage,
    ctx: &MaterialCompileContext,
    node: &Node,
    out_port: &str,
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
    if has_expensive_or_effect_like_call(&result.expr) {
        return true;
    }
    if result.expr.len() > 180 {
        return true;
    }
    if outgoing_value_fanout(scene, &node.id, out_port) > 1 && result.expr.len() > 48 {
        return true;
    }

    false
}

fn emit_readable_temp_if_needed(
    scene: &SceneDSL,
    stage_tag: &str,
    stage: crate::renderer::validation::GlslShaderStage,
    node: &Node,
    out_port: &str,
    ctx: &mut MaterialCompileContext,
    result: TypedExpr,
) -> TypedExpr {
    if !should_emit_readable_temp(scene, stage, ctx, node, out_port, &result) {
        return result;
    }

    let temp_name = readable_node_temp_name(ctx, stage_tag, node, out_port, "result");
    push_readable_let(
        ctx,
        format!(
            "{} {}.{}",
            display_param(node, "label").unwrap_or(node.node_type.as_str()),
            node.id,
            out_port
        ),
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
    if let Some(v) = ctx
        .expr_overrides
        .get(&(node_id.to_string(), out_port_name.to_string()))
    {
        return Ok(v.clone());
    }
    if let Some(v) = cache.get(&key) {
        return Ok(v.clone());
    }

    let node = find_node(nodes_by_id, node_id)?;

    if let Some(group_call) =
        try_compile_pure_group_call(scene, nodes_by_id, node, out_port_name, ctx, cache, stage)?
    {
        let result = emit_readable_temp_if_needed(
            scene,
            stage_tag,
            stage,
            node,
            out_port_name,
            ctx,
            group_call,
        );
        cache.insert(key, result.clone());
        return Ok(result);
    }

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

    let result =
        emit_readable_temp_if_needed(scene, stage_tag, stage, node, out_port_name, ctx, result);

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
            debug_artifacts: None,
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
            debug_artifacts: None,
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

#[cfg(test)]
mod readability_tests {
    use super::*;
    use crate::dsl::{
        Connection, Endpoint, GroupDSL, GroupInputBinding, GroupOutputBinding, Metadata, NodePort,
    };
    use serde_json::json;
    use std::collections::HashMap;

    fn node(id: &str, node_type: &str, label: &str) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params: HashMap::from([("label".to_string(), json!(label))]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        }
    }

    fn conn(from_node: &str, from_port: &str, to_node: &str, to_port: &str) -> Connection {
        Connection {
            id: format!("{from_node}:{from_port}->{to_node}:{to_port}"),
            from: Endpoint {
                node_id: from_node.to_string(),
                port_id: from_port.to_string(),
            },
            to: Endpoint {
                node_id: to_node.to_string(),
                port_id: to_port.to_string(),
            },
        }
    }

    fn scene(nodes: Vec<Node>, connections: Vec<Connection>, groups: Vec<GroupDSL>) -> SceneDSL {
        SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "readability".to_string(),
                created: None,
                modified: None,
            },
            nodes,
            connections,
            outputs: None,
            groups,
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        }
    }

    #[test]
    fn simple_math_nodes_stay_inline() {
        let nodes = vec![
            node("a", "FloatInput", "A"),
            node("b", "FloatInput", "B"),
            node("mul", "MathMultiply", "Multiply"),
        ];
        let scene = scene(
            nodes.clone(),
            vec![
                conn("a", "value", "mul", "a"),
                conn("b", "value", "mul", "b"),
            ],
            Vec::new(),
        );
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let expr = compile_material_expr(
            &scene,
            &nodes_by_id,
            "mul",
            Some("result"),
            &mut ctx,
            &mut cache,
        )
        .unwrap();

        assert_eq!(expr.ty, ValueType::F32);
        assert!(expr.expr.contains(" * "));
        assert!(
            ctx.inline_stmts.is_empty(),
            "simple multiply should not emit readable temps: {:#?}",
            ctx.inline_stmts
        );
    }

    #[test]
    fn readable_temp_names_prefer_node_labels_and_hash_only_on_collision() {
        let mut ctx = MaterialCompileContext::default();
        let first = node("n1", "MathClosure", "Edge Color");
        let second = node("n2", "MathClosure", "Edge Color");

        let a = readable_node_temp_name(&mut ctx, "fs", &first, "output", "out");
        let b = readable_node_temp_name(&mut ctx, "fs", &second, "output", "out");

        assert_eq!(a, "edge_color_out");
        assert!(b.starts_with("edge_color_out_"));
        assert_ne!(a, b);
    }

    #[test]
    fn pure_group_output_compiles_to_helper_call() {
        let group = GroupDSL {
            id: "group_1".to_string(),
            name: Some("Pure Multiply".to_string()),
            inputs: vec![
                NodePort {
                    id: "left".to_string(),
                    name: Some("Left".to_string()),
                    port_type: Some("float".to_string()),
                },
                NodePort {
                    id: "right".to_string(),
                    name: Some("Right".to_string()),
                    port_type: Some("float".to_string()),
                },
            ],
            outputs: vec![NodePort {
                id: "out".to_string(),
                name: Some("Output".to_string()),
                port_type: Some("float".to_string()),
            }],
            nodes: vec![
                node("ga", "FloatInput", "Float Input"),
                node("gb", "FloatInput", "Float Input"),
                node("gmul", "MathMultiply", "Multiply"),
            ],
            connections: vec![
                conn("ga", "value", "gmul", "a"),
                conn("gb", "value", "gmul", "b"),
            ],
            input_bindings: vec![
                GroupInputBinding {
                    group_port_id: "left".to_string(),
                    to: Endpoint {
                        node_id: "ga".to_string(),
                        port_id: "value".to_string(),
                    },
                },
                GroupInputBinding {
                    group_port_id: "right".to_string(),
                    to: Endpoint {
                        node_id: "gb".to_string(),
                        port_id: "value".to_string(),
                    },
                },
            ],
            output_bindings: vec![GroupOutputBinding {
                group_port_id: "out".to_string(),
                from: Endpoint {
                    node_id: "gmul".to_string(),
                    port_id: "result".to_string(),
                },
            }],
        };

        let mut expanded_a = node("GroupInstance_1/ga", "FloatInput", "Float Input");
        expanded_a
            .params
            .insert("__dedup_group_id".to_string(), json!("group_1"));
        expanded_a
            .params
            .insert("__dedup_original_id".to_string(), json!("ga"));
        let mut expanded_b = node("GroupInstance_1/gb", "FloatInput", "Float Input");
        expanded_b
            .params
            .insert("__dedup_group_id".to_string(), json!("group_1"));
        expanded_b
            .params
            .insert("__dedup_original_id".to_string(), json!("gb"));
        let mut expanded_mul = node("GroupInstance_1/gmul", "MathMultiply", "Multiply");
        expanded_mul
            .params
            .insert("__dedup_group_id".to_string(), json!("group_1"));
        expanded_mul
            .params
            .insert("__dedup_original_id".to_string(), json!("gmul"));
        expanded_mul
            .params
            .insert("__group_instance_label".to_string(), json!("Nice Group"));
        let mut expanded_a2 = node("GroupInstance_2/ga", "FloatInput", "Float Input");
        expanded_a2
            .params
            .insert("__dedup_group_id".to_string(), json!("group_1"));
        expanded_a2
            .params
            .insert("__dedup_original_id".to_string(), json!("ga"));
        let mut expanded_b2 = node("GroupInstance_2/gb", "FloatInput", "Float Input");
        expanded_b2
            .params
            .insert("__dedup_group_id".to_string(), json!("group_1"));
        expanded_b2
            .params
            .insert("__dedup_original_id".to_string(), json!("gb"));
        let mut expanded_mul2 = node("GroupInstance_2/gmul", "MathMultiply", "Multiply");
        expanded_mul2
            .params
            .insert("__dedup_group_id".to_string(), json!("group_1"));
        expanded_mul2
            .params
            .insert("__dedup_original_id".to_string(), json!("gmul"));
        expanded_mul2
            .params
            .insert("__group_instance_label".to_string(), json!("Other Label"));

        let nodes = vec![
            node("x", "FloatInput", "Input X"),
            node("y", "FloatInput", "Input Y"),
            node("x2", "FloatInput", "Input X2"),
            node("y2", "FloatInput", "Input Y2"),
            expanded_a,
            expanded_b,
            expanded_mul,
            expanded_a2,
            expanded_b2,
            expanded_mul2,
        ];
        let scene = scene(
            nodes.clone(),
            vec![
                conn("x", "value", "GroupInstance_1/ga", "value"),
                conn("y", "value", "GroupInstance_1/gb", "value"),
                conn("GroupInstance_1/ga", "value", "GroupInstance_1/gmul", "a"),
                conn("GroupInstance_1/gb", "value", "GroupInstance_1/gmul", "b"),
                conn("x2", "value", "GroupInstance_2/ga", "value"),
                conn("y2", "value", "GroupInstance_2/gb", "value"),
                conn("GroupInstance_2/ga", "value", "GroupInstance_2/gmul", "a"),
                conn("GroupInstance_2/gb", "value", "GroupInstance_2/gmul", "b"),
            ],
            vec![group],
        );
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let expr = compile_material_expr(
            &scene,
            &nodes_by_id,
            "GroupInstance_1/gmul",
            Some("result"),
            &mut ctx,
            &mut cache,
        )
        .unwrap();

        let expr2 = compile_material_expr(
            &scene,
            &nodes_by_id,
            "GroupInstance_2/gmul",
            Some("result"),
            &mut ctx,
            &mut cache,
        )
        .unwrap();

        assert!(expr.expr.starts_with("pure_multiply("), "got {}", expr.expr);
        assert!(
            expr2.expr.starts_with("pure_multiply("),
            "got {}",
            expr2.expr
        );
        let decls = ctx.extra_wgsl_decls.values().cloned().collect::<String>();
        assert!(decls.contains("fn pure_multiply(left: f32, right: f32) -> f32"));
        assert!(decls.contains("return (left * right);"));
        assert_eq!(decls.matches("fn pure_multiply(").count(), 1);
    }
}
