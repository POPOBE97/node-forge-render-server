//! Compile small user-authored GLSL snippets into WGSL helpers.

use anyhow::{Result, anyhow};

use crate::renderer::{
    types::ValueType,
    validation::{GlslShaderStage, glsl_to_wgsl},
};

pub struct GlslParam {
    pub name: String,
    pub ty: ValueType,
    /// WGSL expression to pass at call-site.
    pub wgsl_expr: String,
}

pub struct GlslSnippetSpec {
    pub fn_name: String,
    pub return_type: ValueType,
    pub params: Vec<GlslParam>,
    /// GLSL function body. Expected to write to `output`.
    pub body: String,
    pub stage: GlslShaderStage,
}

pub struct CompiledGlslSnippet {
    pub wgsl_fn_name: String,
    pub wgsl_fn_decl: String,
    pub call_expr: String,
}

pub fn compile_glsl_snippet(spec: GlslSnippetSpec) -> Result<CompiledGlslSnippet> {
    let ret_glsl = spec.return_type.glsl();

    let mut glsl_params: Vec<String> = Vec::new();
    let mut arg_forward: Vec<String> = Vec::new();
    let mut entry_inputs: Vec<String> = Vec::new();

    for (i, p) in spec.params.iter().enumerate() {
        glsl_params.push(format!("{} {}", p.ty.glsl(), p.name));

        // NOTE: The synthetic entrypoint exists purely to satisfy naga's GLSL frontend.
        // Some GLSL stage IO types have extra restrictions (e.g. bool varyings, integer
        // varyings in fragment stage requiring `flat`). Keep the entry IO valid, while
        // preserving the *function* signature types.
        let (entry_in_ty, forward_expr) = match p.ty {
            // GLSL does not allow boolean stage inputs/outputs in the general case.
            // Represent as int in the entrypoint, then convert to bool when calling.
            ValueType::Bool => ("int", format!("({}_in != 0)", p.name)),
            _ => (p.ty.glsl(), format!("{}_in", p.name)),
        };
        let needs_flat = matches!(spec.stage, GlslShaderStage::Fragment)
            && matches!(entry_in_ty, "int" | "uint");
        let flat = if needs_flat { "flat " } else { "" };
        entry_inputs.push(format!(
            "layout(location={i}) {flat}in {entry_in_ty} {}_in;",
            p.name
        ));
        arg_forward.push(forward_expr);
    }

    let entry_ret = match spec.return_type {
        ValueType::F32 => "float",
        ValueType::I32 => "int",
        ValueType::U32 => "uint",
        ValueType::Bool => "bool",
        ValueType::Texture2D => "sampler2D",
        ValueType::Vec2 => "vec2",
        ValueType::Vec3 => "vec3",
        ValueType::Vec4 => "vec4",
    };

    let glsl = format!(
        "#version 450\n\n{ret_glsl} {fn_name}({params}) {{\n    {ret_glsl} output = {ret_glsl}(0.0);\n{body}\n    return output;\n}}\n\n{entry_inputs}\nlayout(location=0) out {entry_ret} _sn_out;\nvoid main() {{\n    _sn_out = {fn_name}({arg_forward});\n}}\n",
        fn_name = spec.fn_name,
        params = glsl_params.join(", "),
        body = indent_glsl_body(&spec.body, 1),
        entry_inputs = entry_inputs.join("\n"),
        arg_forward = arg_forward.join(", "),
    );

    let wgsl = glsl_to_wgsl(&glsl, spec.stage)
        .or_else(|_| {
            // Some snippets are stage-agnostic; allow fallback.
            match spec.stage {
                GlslShaderStage::Fragment => glsl_to_wgsl(&glsl, GlslShaderStage::Vertex),
                GlslShaderStage::Vertex => glsl_to_wgsl(&glsl, GlslShaderStage::Fragment),
                GlslShaderStage::Compute => glsl_to_wgsl(&glsl, GlslShaderStage::Fragment),
            }
        })
        .map_err(|e| anyhow!("GLSL->WGSL failed: {e:#}\nGLSL:\n{glsl}"))?;

    let (wgsl_fn_name, wgsl_fn_decl) = extract_wgsl_fn_decl(&wgsl, &spec.fn_name)
        .map(|decl| (spec.fn_name.clone(), decl))
        .or_else(|| {
            // Naga can append '_' to avoid name collisions.
            let alt = format!("{}_", spec.fn_name);
            extract_wgsl_fn_decl(&wgsl, &alt).map(|decl| (alt, decl))
        })
        .ok_or_else(|| {
            anyhow!(
                "failed to find generated WGSL function `{}` in naga output\nWGSL:\n{}",
                spec.fn_name,
                wgsl
            )
        })?;

    let call_expr = format!(
        "{wgsl_fn_name}({})",
        spec.params
            .iter()
            .map(|p| p.wgsl_expr.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(CompiledGlslSnippet {
        wgsl_fn_name,
        wgsl_fn_decl,
        call_expr,
    })
}

fn indent_glsl_body(source: &str, indent_levels: usize) -> String {
    let indent = "    ".repeat(indent_levels);
    source
        .replace("\r\n", "\n")
        .lines()
        .map(|line| {
            let line = line.trim_end();
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_wgsl_fn_decl(source: &str, fn_name: &str) -> Option<String> {
    let needle = format!("fn {fn_name}(");
    let start = source.find(&needle)?;

    let bytes = source.as_bytes();
    let mut i = start;
    let mut brace_depth: i32 = 0;
    let mut seen_open_brace = false;

    while i < bytes.len() {
        match bytes[i] as char {
            '{' => {
                seen_open_brace = true;
                brace_depth += 1;
            }
            '}' => {
                if seen_open_brace {
                    brace_depth -= 1;
                    if brace_depth == 0 {
                        return Some(source[start..=i].to_string());
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }

    None
}
