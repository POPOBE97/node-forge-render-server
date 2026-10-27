use crate::{
    dsl::{Node, SceneDSL},
    renderer::types::{MaterialCompileContext, TypedExpr, ValueType},
};
use anyhow::{Result, bail};
use std::collections::HashMap;

pub fn compile<F>(
    scene: &SceneDSL,
    node: &Node,
    port: Option<&str>,
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
    let port = port.unwrap_or("distance");
    if !matches!(port, "distance" | "coverage") {
        bail!("TextSdf unsupported output {port}");
    }
    let position = super::sdf_nodes::resolve_input_expr_vec2_or_default(
        scene,
        node,
        "position",
        "vec2f(0.0)",
        ctx,
        cache,
        &compile_fn,
    )?;
    ctx.register_image_texture(&node.id);
    ctx.extra_wgsl_decls
        .entry("text_sdf".into())
        .or_insert_with(|| include_str!("templates/text_sdf.wgsl").into());
    let texture = MaterialCompileContext::tex_var_name(&node.id);
    let sample = if port == "coverage" { "nf_text_msdf" } else { "nf_text_sdf" };
    let distance = format!("{sample}({texture}, in.local_px.xy - ({}))", position.expr);
    let expr = if port == "coverage" {
        format!("nf_sdf_coverage({distance})")
    } else {
        distance
    };
    // Preserve LocalPx in the expression: Sdf2DBevel re-evaluates it at four offsets.
    Ok(TypedExpr::with_time(expr, ValueType::F32, position.uses_time).inline())
}
