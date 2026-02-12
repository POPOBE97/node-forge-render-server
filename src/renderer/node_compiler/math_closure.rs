//! Compiler for MathClosure node (user-provided math code snippets).
//!
//! The DSL MathClosure nodes carry a small GLSL-like snippet in `params.source`.
//! We compile each closure into an inline `{ }` block to isolate context and avoid
//! naming conflicts, rather than generating separate helper functions.

use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use crate::dsl::{Node, NodePort, SceneDSL, incoming_connection};
use crate::renderer::glsl_snippet::{GlslParam, GlslSnippetSpec, compile_glsl_snippet};
use crate::renderer::types::{MaterialCompileContext, TypedExpr, ValueType};
use crate::renderer::utils::{coerce_to_type, sanitize_wgsl_ident};
use crate::renderer::validation::GlslShaderStage;

fn map_port_type(s: Option<&str>) -> Result<ValueType> {
    let Some(s) = s else {
        return Ok(ValueType::F32);
    };
    let t = s.to_ascii_lowercase();
    match t.as_str() {
        "float" | "f32" | "number" => Ok(ValueType::F32),
        "int" | "i32" => Ok(ValueType::I32),
        "bool" | "boolean" => Ok(ValueType::Bool),
        "vector2" | "vec2" => Ok(ValueType::Vec2),
        "vector3" | "vec3" => Ok(ValueType::Vec3),
        "vector4" | "vec4" | "color" => Ok(ValueType::Vec4),
        // Pass texture reference - used for multi-tap sampling inside MathClosure.
        "pass" | "texture" => Ok(ValueType::Texture2D),
        // Array types — size 0 means "infer at compile time".
        "float[]" | "f32[]" => Ok(ValueType::F32Array(0)),
        "vector2[]" | "vec2[]" => Ok(ValueType::Vec2Array(0)),
        "vector3[]" | "vec3[]" => Ok(ValueType::Vec3Array(0)),
        "vector4[]" | "vec4[]" => Ok(ValueType::Vec4Array(0)),
        other => bail!("unsupported MathClosure port type: {other}"),
    }
}

/// Represents a pass texture input for MathClosure that enables direct sampling.
#[derive(Clone, Debug)]
struct PassTextureInput {
    /// Variable name used in the snippet (e.g., "mip0").
    var_name: String,
    /// Upstream pass node ID.
    pass_node_id: String,
}

fn default_value_for(ty: ValueType) -> TypedExpr {
    match ty {
        ValueType::F32 => TypedExpr::new("0.0", ValueType::F32),
        ValueType::I32 => TypedExpr::new("0", ValueType::I32),
        ValueType::U32 => TypedExpr::new("0u", ValueType::U32),
        ValueType::Bool => TypedExpr::new("false", ValueType::Bool),
        ValueType::Texture2D => unreachable!("MathClosure cannot produce Texture2D values"),
        ValueType::Vec2 => TypedExpr::new("vec2f(0.0, 0.0)", ValueType::Vec2),
        ValueType::Vec3 => TypedExpr::new("vec3f(0.0, 0.0, 0.0)", ValueType::Vec3),
        ValueType::Vec4 => TypedExpr::new("vec4f(0.0, 0.0, 0.0, 0.0)", ValueType::Vec4),
        _ if ty.is_array() => unreachable!("MathClosure array inputs must be connected"),
        _ => unreachable!(),
    }
}

fn infer_output_type_from_source(source: &str) -> ValueType {
    // Heuristic: detect the constructor used in the final assignment.
    // This matches the patterns used by our test scenes.
    if source.contains("output = vec4") || source.contains("output=vec4") {
        return ValueType::Vec4;
    }
    if source.contains("output = vec3") || source.contains("output=vec3") {
        return ValueType::Vec3;
    }
    if source.contains("output = vec2") || source.contains("output=vec2") {
        return ValueType::Vec2;
    }

    // Fallback: detect `output = someVar;` and infer from the var's declaration type.
    // Example: `vec2 lightCenterPx = ...; output = lightCenterPx;`
    let mut rhs_ident: Option<&str> = None;
    for line in source.lines() {
        let l = line.trim();
        let Some(pos) = l.find("output") else {
            continue;
        };
        let after = &l[pos + "output".len()..];
        let after = after.trim_start();
        if !after.starts_with('=') {
            continue;
        }
        let after = after[1..].trim_start();
        let rhs = after.split(';').next().unwrap_or(after).trim();
        if rhs.is_empty() {
            continue;
        }
        // Only consider simple identifiers (no swizzles, no function calls).
        if rhs
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            rhs_ident = Some(rhs);
            break;
        }
    }

    if let Some(name) = rhs_ident {
        if source.contains(&format!("vec4 {name}")) {
            return ValueType::Vec4;
        }
        if source.contains(&format!("vec3 {name}")) {
            return ValueType::Vec3;
        }
        if source.contains(&format!("vec2 {name}")) {
            return ValueType::Vec2;
        }
        if source.contains(&format!("float {name}")) {
            return ValueType::F32;
        }
    }

    ValueType::F32
}

fn port_id_to_param_name(port: &NodePort) -> String {
    // DSL ports often use a generated `id` (e.g. dynamic_...) while the user-provided
    // snippet references the human variable name (port.name / variableName).
    // Prefer the name when available so identifiers resolve inside the WGSL helper.
    if let Some(name) = port
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sanitize_wgsl_ident(name)
    } else {
        sanitize_wgsl_ident(&port.id)
    }
}

fn promote_type_from_source_swizzles(source: &str, param_name: &str, ty: ValueType) -> ValueType {
    // Some DSL exports mark ports as `float` but the snippet uses swizzles like `x/y`.
    // WGSL forbids swizzles on scalars, so we conservatively promote the parameter type.
    if ty != ValueType::F32 {
        return ty;
    }

    let has_x = source.contains(&format!("{param_name}.x"));
    let has_y = source.contains(&format!("{param_name}.y"));
    let has_z = source.contains(&format!("{param_name}.z"));
    let has_w = source.contains(&format!("{param_name}.w"));

    if has_w {
        ValueType::Vec4
    } else if has_z {
        ValueType::Vec3
    } else if has_x || has_y {
        ValueType::Vec2
    } else {
        ValueType::F32
    }
}

/// Compile a MathClosure node by emitting an inline `{ }` block and returning a variable reference.
///
/// Instead of generating a separate helper function, this emits the snippet code inline
/// within a `{ }` block to isolate the local variable scope and avoid naming conflicts.
/// This produces clearer generated code for small math snippets.
///
/// ## Pass Texture Inputs
///
/// MathClosure supports `pass` type inputs for direct texture sampling. When a port has
/// type "pass", the connected pass node's texture becomes available for sampling via:
/// ```glsl
/// vec4 color = samplePass(varName, uv);  // Sample at UV coordinates
/// ```
///
/// This enables Mitchell-Netravali cubic upsampling and other multi-tap filters inside
/// a MathClosure without requiring separate PassTexture nodes.
///
/// ## Array Types
///
/// MathClosure supports array input/output types (`float[]`, `vector2[]`, etc.).
/// When arrays are involved, a direct GLSL→WGSL string conversion is used instead
/// of the naga pipeline because GLSL doesn't support array function return types.

/// Infer the array length from the source code by finding array constructor patterns.
///
/// Searches for patterns like `vec2[4](...`, `float[8](...`.
fn infer_array_size_from_source(source: &str, elem_ty: ValueType) -> Option<usize> {
    let type_prefix = match elem_ty {
        ValueType::F32 => "float",
        ValueType::Vec2 => "vec2",
        ValueType::Vec3 => "vec3",
        ValueType::Vec4 => "vec4",
        _ => return None,
    };
    let pattern = format!("{type_prefix}[");
    for (pos, _) in source.match_indices(&pattern) {
        let after = &source[pos + pattern.len()..];
        if let Some(close) = after.find(']') {
            let num_str = after[..close].trim();
            if let Ok(n) = num_str.parse::<usize>() {
                // Verify it's followed by '(' (constructor, not declaration)
                let after_bracket = after[close + 1..].trim_start();
                if after_bracket.starts_with('(') {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// Check whether any input or output port of a MathClosure uses an array type.
fn has_array_ports(node: &Node) -> bool {
    node.inputs
        .iter()
        .chain(node.outputs.iter())
        .any(|p| {
            p.port_type
                .as_deref()
                .map(|t| t.ends_with("[]"))
                .unwrap_or(false)
        })
}

/// Convert a single GLSL statement/line to WGSL.
///
/// Handles variable declarations (`vec2 x = expr;` → `var x: vec2f = expr;`)
/// and expression-level type constructor replacements.
fn convert_glsl_line_to_wgsl(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Try to match a variable declaration: <type> <name> [= <expr>];
    static GLSL_TYPES: &[(&str, &str)] = &[
        ("vec4", "vec4f"),
        ("vec3", "vec3f"),
        ("vec2", "vec2f"),
        ("float", "f32"),
        ("int", "i32"),
        ("bool", "bool"),
    ];

    for &(glsl_ty, wgsl_ty) in GLSL_TYPES {
        if !trimmed.starts_with(glsl_ty) {
            continue;
        }
        let after_type = &trimmed[glsl_ty.len()..];
        // Must be followed by whitespace (declaration) or '[' (array decl/constructor).
        let first_char = after_type.chars().next().unwrap_or(' ');
        if first_char == '[' {
            // Array declaration: vec2[N] name = expr; (not a constructor on its own line)
            if let Some(close) = after_type.find(']') {
                let size_str = after_type[1..close].trim();
                let after_bracket = after_type[close + 1..].trim_start();
                // If it starts with '(' it's an expression (constructor), not a declaration
                if !after_bracket.starts_with('(') && !after_bracket.is_empty() {
                    if let Ok(_n) = size_str.parse::<usize>() {
                        if let Some(eq_pos) = after_bracket.find('=') {
                            let name = after_bracket[..eq_pos].trim();
                            let expr = after_bracket[eq_pos + 1..].trim();
                            let expr = convert_glsl_exprs_to_wgsl(expr);
                            return format!("var {name}: array<{wgsl_ty}, {size_str}> = {expr}");
                        } else {
                            let name = after_bracket.trim_end_matches(';').trim();
                            return format!("var {name}: array<{wgsl_ty}, {size_str}>;");
                        }
                    }
                }
            }
        } else if first_char.is_ascii_whitespace() {
            let rest = after_type.trim_start();
            // Check if rest starts with an identifier (variable name)
            if let Some(first) = rest.chars().next() {
                if first.is_ascii_alphabetic() || first == '_' {
                    // Find the identifier end
                    let ident_end = rest
                        .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                        .unwrap_or(rest.len());
                    let name = &rest[..ident_end];
                    let after_name = rest[ident_end..].trim_start();

                    if after_name.starts_with('=') {
                        let expr = after_name[1..].trim();
                        let expr = convert_glsl_exprs_to_wgsl(expr);
                        return format!("var {name}: {wgsl_ty} = {expr}");
                    } else if after_name.starts_with(';') || after_name.is_empty() {
                        return format!("var {name}: {wgsl_ty};");
                    }
                }
            }
        }
    }

    // Not a declaration — convert expressions in the whole line.
    convert_glsl_exprs_to_wgsl(trimmed)
}

/// Replace GLSL expression-level constructs with WGSL equivalents.
fn convert_glsl_exprs_to_wgsl(expr: &str) -> String {
    let mut result = expr.to_string();

    // Replace array constructors first: vec2[N]( → array<vec2f, N>(
    for (glsl_ty, wgsl_ty) in [
        ("vec4", "vec4f"),
        ("vec3", "vec3f"),
        ("vec2", "vec2f"),
        ("float", "f32"),
        ("int", "i32"),
    ] {
        let pattern = format!("{glsl_ty}[");
        loop {
            let Some(pos) = result.find(&pattern) else {
                break;
            };
            let after = &result[pos + pattern.len()..];
            let Some(close) = after.find(']') else { break };
            let size_str = after[..close].trim();
            if size_str.parse::<usize>().is_err() {
                break;
            }
            let after_bracket = &after[close + 1..];
            if after_bracket.starts_with('(') {
                let replacement = format!("array<{wgsl_ty}, {size_str}>");
                result = format!(
                    "{}{}{}",
                    &result[..pos],
                    replacement,
                    after_bracket
                );
                continue;
            }
            break;
        }
    }

    // Replace type constructors: vec4( → vec4f(, vec3( → vec3f(, vec2( → vec2f(
    // Process longer names first to avoid partial matches.
    result = result.replace("vec4(", "vec4f(");
    result = result.replace("vec3(", "vec3f(");
    result = result.replace("vec2(", "vec2f(");

    result
}

/// Compile a MathClosure containing array types directly to WGSL (bypassing naga).
///
/// GLSL doesn't support array return types from functions, so we can't use the naga
/// GLSL→WGSL pipeline for closures that produce/consume arrays.
fn compile_math_closure_array(
    fn_name: &str,
    source: &str,
    input_params: &[(String, ValueType)],
    ret_ty: ValueType,
    stage: GlslShaderStage,
) -> Result<(String, String, String)> {
    // Build WGSL function parameters.
    let uv_arg = match stage {
        GlslShaderStage::Vertex => "uv",
        _ => "in.uv",
    };
    let mut fn_params = vec!["uv_in: vec2f".to_string()];
    let mut var_copies = vec!["    var uv: vec2f = uv_in;".to_string()];

    for (name, ty) in input_params {
        let wgsl_ty = ty.wgsl_type_string();
        fn_params.push(format!("{name}_in: {wgsl_ty}"));
        var_copies.push(format!("    var {name}: {wgsl_ty} = {name}_in;"));
    }

    let ret_wgsl = ret_ty.wgsl_type_string();

    // Convert GLSL body to WGSL.
    let mut body_lines = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let converted = convert_glsl_line_to_wgsl(trimmed);
        body_lines.push(format!("    {converted}"));
    }

    let fn_decl = format!(
        "fn {fn_name}({params}) -> {ret_wgsl} {{\n{copies}\n    var output: {ret_wgsl};\n{body}\n    return output;\n}}",
        params = fn_params.join(", "),
        copies = var_copies.join("\n"),
        body = body_lines.join("\n"),
    );

    // Build call-site arguments.
    let mut call_args = vec![uv_arg.to_string()];
    for (name, _) in input_params {
        call_args.push(name.clone());
    }
    let call_expr = format!("{fn_name}({})", call_args.join(", "));

    Ok((fn_name.to_string(), fn_decl, call_expr))
}

pub fn compile_math_closure<F>(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: F,
    stage: GlslShaderStage,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    let source = node
        .params
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("MathClosure missing params.source (node={})", node.id))?;

    // Prefer the declared output port type (from the scene schema).
    // The old inference-from-source heuristic breaks for cases like:
    //   output = gap * vec3(x, y, 0.0);
    // where no `vecN(...)` constructor is present in the assignment.
    let mut ret_ty = node
        .outputs
        .first()
        .and_then(|p| p.port_type.as_deref())
        .and_then(|t| map_port_type(Some(t)).ok())
        .unwrap_or_else(|| infer_output_type_from_source(source));

    // For unsized array outputs, infer the actual length from the source.
    if ret_ty.is_array() && ret_ty.array_len() == 0 {
        if let Some(elem_ty) = ret_ty.array_element_type() {
            if let Some(size) = infer_array_size_from_source(source, elem_ty) {
                ret_ty = ret_ty.with_array_len(size);
            } else {
                bail!(
                    "MathClosure {} has array output but array size could not be inferred from source",
                    node.id
                );
            }
        }
    }

    let output_var = format!("mc_{}_out", sanitize_wgsl_ident(&node.id));

    // Collect pass texture inputs for direct sampling support.
    let mut pass_texture_inputs: Vec<PassTextureInput> = Vec::new();

    // Compile inputs in declared order.
    let mut param_bindings: Vec<String> = Vec::new();
    let mut uses_time = false;
    // Track actual resolved (name, type) for each non-texture input — needed by the array path.
    let mut resolved_input_types: Vec<(String, ValueType)> = Vec::new();

    for port in &node.inputs {
        let param_name = port_id_to_param_name(port);
        let port_ty = map_port_type(port.port_type.as_deref())?;

        // Handle pass texture inputs specially.
        if port_ty == ValueType::Texture2D {
            // Find the connected pass node.
            let conn = incoming_connection(scene, &node.id, &port.id)
                .ok_or_else(|| anyhow!("MathClosure pass input '{}' is not connected", port.id))?;
            let upstream_node = nodes_by_id.get(&conn.from.node_id)
                .ok_or_else(|| anyhow!("MathClosure: upstream node not found: {}", conn.from.node_id))?;

            // Validate that upstream is a pass-producing node.
            if !matches!(
                upstream_node.node_type.as_str(),
                "RenderPass" | "GuassianBlurPass" | "Downsample"
            ) {
                bail!(
                    "MathClosure pass input '{}' must be connected to a pass node, got {}",
                    param_name,
                    upstream_node.node_type
                );
            }

            // Register this pass texture for binding.
            ctx.register_pass_texture(&conn.from.node_id);

            pass_texture_inputs.push(PassTextureInput {
                var_name: param_name.clone(),
                pass_node_id: conn.from.node_id.clone(),
            });

            // Don't add a parameter binding for pass inputs - they're sampled directly.
            continue;
        }

        let expected_ty = if port_ty.is_array() {
            // Array types don't get promoted from swizzle analysis.
            port_ty
        } else {
            promote_type_from_source_swizzles(
                &source,
                &param_name,
                port_ty,
            )
        };

        let arg_expr = if let Some(conn) = incoming_connection(scene, &node.id, &port.id) {
            let compiled = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
            coerce_to_type(compiled, expected_ty)?
        } else {
            // Allow unconnected inputs; treat as zero.
            default_value_for(expected_ty)
        };

        uses_time |= arg_expr.uses_time;
        // Track the actual resolved type (with correct array size).
        resolved_input_types.push((param_name.clone(), arg_expr.ty));
        // Bind the input expression to a local variable inside the block
        param_bindings.push(format!("        let {param_name} = {};", arg_expr.expr));
    }

    // Convert the user snippet by compiling a minimal GLSL fragment module via naga.
    // We intentionally only use a GLSL "function" wrapper and then call the emitted WGSL function.
    // This avoids any string-level GLSL-ish -> WGSL rewrites.
    //
    // EXCEPTION: If any port uses an array type, we bypass naga entirely and do direct
    // GLSL→WGSL string conversion, because GLSL doesn't support array function return types.
    let fn_name = format!("mc_{}", sanitize_wgsl_ident(&node.id));
    let mut source = source
        .replace("vUv", "uv")
        .replace("v_UV", "uv")
        .replace("vUV", "uv");

    let use_array_path = has_array_ports(node);

    let (wgsl_fn_name, wgsl_fn_decl, call_expr) = if use_array_path {
        // ── Array path: direct WGSL generation ──
        // Use the resolved input types collected during the first pass (with correct array sizes).
        compile_math_closure_array(&fn_name, &source, &resolved_input_types, ret_ty, stage)?
    } else {
        // ── Normal path: naga GLSL→WGSL pipeline ──

        // Build a prefix with helper function declarations for pass texture sampling.
        let mut glsl_helper_prefix = String::new();

        for pti in &pass_texture_inputs {
            let tex_var = MaterialCompileContext::pass_tex_var_name(&pti.pass_node_id);
            let samp_var = MaterialCompileContext::pass_sampler_var_name(&pti.pass_node_id);
            let helper_name = format!("sample_pass_{}", sanitize_wgsl_ident(&pti.pass_node_id));
            let helper_name_with_suffix = format!("{helper_name}_");

            let extra_args = {
                let pat = format!("samplePass({}, ", pti.var_name);
                if let Some(pos) = source.find(&pat) {
                    let after = &source[pos + pat.len()..];
                    let mut depth = 1;
                    let mut commas = 0;
                    for ch in after.chars() {
                        match ch {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 { break; }
                            }
                            ',' if depth == 1 => commas += 1,
                            _ => {}
                        }
                    }
                    commas
                } else {
                    0
                }
            };

            let pattern = format!("samplePass({}, ", pti.var_name);
            let replacement = format!("{helper_name}(");
            source = source.replace(&pattern, &replacement);

            if extra_args >= 1 {
                glsl_helper_prefix.push_str(&format!(
                    "vec4 {helper_name}(vec2 xy_arg, vec2 res_arg) {{ return vec4(0.0); }}\n"
                ));
                let helper_fn = format!(
                    r#"fn {helper_name_with_suffix}(xy_in: vec2f, res_in: vec2f) -> vec4f {{
    let uv = xy_in / res_in;
    return textureSample({tex_var}, {samp_var}, uv);
}}
"#,
                );
                ctx.extra_wgsl_decls
                    .insert(helper_name_with_suffix, helper_fn);
            } else {
                glsl_helper_prefix.push_str(&format!(
                    "vec4 {helper_name}(vec2 uv_arg) {{ return vec4(0.0); }}\n"
                ));
                let helper_fn = format!(
                    r#"fn {helper_name_with_suffix}(uv_in: vec2f) -> vec4f {{
    return textureSample({tex_var}, {samp_var}, uv_in);
}}
"#,
                );
                ctx.extra_wgsl_decls
                    .insert(helper_name_with_suffix, helper_fn);
            }
        }

        let helper_prefix = if glsl_helper_prefix.is_empty() {
            None
        } else {
            Some(glsl_helper_prefix)
        };

        let mut glsl_params: Vec<String> = vec!["vec2 uv".to_string()];
        let mut wgsl_args: Vec<String> = vec![match stage {
            GlslShaderStage::Vertex => "uv".to_string(),
            _ => "in.uv".to_string(),
        }];

        for port in &node.inputs {
            let param_name = port_id_to_param_name(port);
            let port_ty = map_port_type(port.port_type.as_deref())?;

            if port_ty == ValueType::Texture2D {
                continue;
            }

            let expected_ty = promote_type_from_source_swizzles(
                &source,
                &param_name,
                port_ty,
            );
            glsl_params.push(format!("{} {}", expected_ty.glsl(), param_name));
            if expected_ty == ValueType::Bool {
                wgsl_args.push(format!("select(0.0, 1.0, {param_name})"));
            } else {
                wgsl_args.push(param_name);
            }
        }

        let compiled = compile_glsl_snippet(GlslSnippetSpec {
            fn_name: fn_name.clone(),
            return_type: ret_ty,
            params: glsl_params
                .iter()
                .zip(wgsl_args.iter())
                .map(|(glsl_param, wgsl_expr)| {
                    let mut parts = glsl_param.split_whitespace();
                    let ty = parts.next().unwrap_or("float");
                    let name = parts.next().unwrap_or("arg");
                    let ty = match ty {
                        "float" => ValueType::F32,
                        "int" => ValueType::I32,
                        "uint" => ValueType::U32,
                        "vec2" => ValueType::Vec2,
                        "vec3" => ValueType::Vec3,
                        "vec4" => ValueType::Vec4,
                        _ => ValueType::F32,
                    };
                    GlslParam {
                        name: name.to_string(),
                        ty,
                        wgsl_expr: wgsl_expr.clone(),
                    }
                })
                .collect(),
            body: source.clone(),
            stage,
            helper_prefix,
        })
        .map_err(|e| anyhow!("MathClosure GLSL->WGSL failed (node={}): {e:#}", node.id))?;

        (compiled.wgsl_fn_name, compiled.wgsl_fn_decl, compiled.call_expr)
    };

    ctx.extra_wgsl_decls
        .insert(wgsl_fn_name, wgsl_fn_decl);

    // Build the inline block statement with `{ }` for scope isolation.
    let ret_type = ret_ty.wgsl_type_string();
    let mut block = String::new();
    block.push_str(&format!("    var {output_var}: {ret_type};\n"));
    block.push_str("    {\n");

    if !param_bindings.is_empty() {
        block.push_str(&param_bindings.join("\n"));
        block.push('\n');
    }

    block.push_str(&format!("        var output: {ret_type};\n"));
    block.push_str(&format!("        output = {call_expr};\n"));
    block.push_str(&format!("        {output_var} = output;\n"));
    block.push_str("    }");

    ctx.inline_stmts.push(block);

    Ok(TypedExpr::with_time(output_var, ret_ty, uses_time))
}
