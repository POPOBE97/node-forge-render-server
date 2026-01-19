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
        other => bail!("unsupported MathClosure port type: {other}"),
    }
}

fn default_value_for(ty: ValueType) -> TypedExpr {
    match ty {
        ValueType::F32 => TypedExpr::new("0.0", ValueType::F32),
        ValueType::I32 => TypedExpr::new("0", ValueType::I32),
        ValueType::U32 => TypedExpr::new("0u", ValueType::U32),
        ValueType::Bool => TypedExpr::new("false", ValueType::Bool),
        ValueType::Vec2 => TypedExpr::new("vec2f(0.0, 0.0)", ValueType::Vec2),
        ValueType::Vec3 => TypedExpr::new("vec3f(0.0, 0.0, 0.0)", ValueType::Vec3),
        ValueType::Vec4 => TypedExpr::new("vec4f(0.0, 0.0, 0.0, 0.0)", ValueType::Vec4),
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
pub fn compile_math_closure<F>(
    scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
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
    let ret_ty = node
        .outputs
        .first()
        .and_then(|p| p.port_type.as_deref())
        .and_then(|t| map_port_type(Some(t)).ok())
        .unwrap_or_else(|| infer_output_type_from_source(source));
    let output_var = format!("mc_{}_out", sanitize_wgsl_ident(&node.id));

    // Compile inputs in declared order.
    let mut param_bindings: Vec<String> = Vec::new();
    let mut uses_time = false;

    for port in &node.inputs {
        let param_name = port_id_to_param_name(port);
        let expected_ty = promote_type_from_source_swizzles(
            &source,
            &param_name,
            map_port_type(port.port_type.as_deref())?,
        );

        let arg_expr = if let Some(conn) = incoming_connection(scene, &node.id, &port.id) {
            let compiled = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
            coerce_to_type(compiled, expected_ty)?
        } else {
            // Allow unconnected inputs; treat as zero.
            default_value_for(expected_ty)
        };

        uses_time |= arg_expr.uses_time;
        // Bind the input expression to a local variable inside the block
        param_bindings.push(format!("        let {param_name} = {};", arg_expr.expr));
    }

    // Convert the user snippet by compiling a minimal GLSL fragment module via naga.
    // We intentionally only use a GLSL "function" wrapper and then call the emitted WGSL function.
    // This avoids any string-level GLSL-ish -> WGSL rewrites.
    let fn_name = format!("mc_{}", sanitize_wgsl_ident(&node.id));
    let source = source
        .replace("vUv", "uv")
        .replace("v_UV", "uv")
        .replace("vUV", "uv");

    let mut glsl_params: Vec<String> = vec!["vec2 uv".to_string()];
    // We accept `uv` as the first argument for both fragment and vertex stages.
    // Fragment uses `in.uv`; vertex uses local variable `uv`.
    let mut wgsl_args: Vec<String> = vec![match stage {
        GlslShaderStage::Vertex => "uv".to_string(),
        _ => "in.uv".to_string(),
    }];

    for port in &node.inputs {
        let param_name = port_id_to_param_name(port);
        let expected_ty = promote_type_from_source_swizzles(
            &source,
            &param_name,
            map_port_type(port.port_type.as_deref())?,
        );
        glsl_params.push(format!("{} {}", expected_ty.glsl(), param_name));
        wgsl_args.push(param_name);
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
    })
    .map_err(|e| anyhow!("MathClosure GLSL->WGSL failed (node={}): {e:#}", node.id))?;

    ctx.extra_wgsl_decls
        .insert(compiled.wgsl_fn_name.clone(), compiled.wgsl_fn_decl);

    // Indent call-site for proper formatting inside the block.
    let call_expr = compiled.call_expr;

    // Build the inline block statement with `{ }` for scope isolation.
    // Structure:
    //     var mc_xxx_out: <type>;
    //     {
    //         let param1 = expr1;
    //         let param2 = expr2;
    //         var output: <type>;
    //         <snippet code>
    //         mc_xxx_out = output;
    //     }
    let ret_type = ret_ty.wgsl();
    let mut block = String::new();
    block.push_str(&format!("    var {output_var}: {ret_type};\n"));
    block.push_str("    {\n");

    // Add parameter bindings (if any)
    if !param_bindings.is_empty() {
        block.push_str(&param_bindings.join("\n"));
        block.push('\n');
    }

    block.push_str(&format!("        var output: {ret_type};\n"));

    // Compute via the converted closure function.
    block.push_str(&format!("        output = {call_expr};\n"));
    block.push_str(&format!("        {output_var} = output;\n"));
    block.push_str("    }");

    ctx.inline_stmts.push(block);

    Ok(TypedExpr::with_time(output_var, ret_ty, uses_time))
}
