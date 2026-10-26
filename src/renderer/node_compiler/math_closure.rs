//! Graph adapter for the shared MathClosure compiler. Language semantics live in
//! node-forge-math-closure; this module only binds graph values and GPU resources.
use crate::dsl::{Node, NodePort, SceneDSL, incoming_connection};
use crate::renderer::pass_source::{TextureSourceRef, resolve_texture_source_ref};
use crate::renderer::types::{ImageTextureRef, MaterialCompileContext, TypedExpr, ValueType};
use crate::renderer::utils::coerce_to_type;
use crate::renderer::validation::GlslShaderStage;
use anyhow::{Context, Result, anyhow, bail};
use node_forge_math_closure::{Port, Request};
use std::collections::HashMap;

fn compiler_port(port: &NodePort) -> Result<Port> {
    Ok(Port {
        name: port
            .name
            .clone()
            .ok_or_else(|| anyhow!("MathClosure input '{}' needs a name", port.id))?,
        port_type: port
            .port_type
            .clone()
            .ok_or_else(|| anyhow!("MathClosure input '{}' needs a type", port.id))?,
        array_length: port.array_length.map(u32::try_from).transpose()?,
    })
}

fn render_type(port: &Port) -> Result<ValueType> {
    let length = port.array_length.unwrap_or(0) as usize;
    Ok(match port.port_type.as_str() {
        "float" => ValueType::F32,
        "int" => ValueType::I32,
        "bool" => ValueType::Bool,
        "vector2" => ValueType::Vec2,
        "vector3" => ValueType::Vec3,
        "vector4" | "color" => ValueType::Vec4,
        "float[]" => ValueType::F32Array(length),
        "vector2[]" => ValueType::Vec2Array(length),
        "vector3[]" => ValueType::Vec3Array(length),
        "vector4[]" => ValueType::Vec4Array(length),
        other => bail!("Unsupported MathClosure render value: {other}"),
    })
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
        .ok_or_else(|| anyhow!("MathClosure {} missing source", node.id))?;
    let output_type = node
        .params
        .get("outputType")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("MathClosure {} missing outputType", node.id))?;
    let array_length = if output_type.ends_with("[]") {
        Some(
            node.params
                .get("outputArrayLength")
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| anyhow!("MathClosure {} missing outputArrayLength", node.id))?,
        )
    } else {
        None
    };
    let output = Port {
        name: "output".into(),
        port_type: output_type.into(),
        array_length,
    };
    let declared = node.outputs.first().filter(|p| p.id == "output");
    if node.outputs.len() != 1
        || declared.and_then(|p| p.port_type.as_deref()) != Some(output_type)
        || declared.and_then(|p| p.array_length) != array_length.map(|v| v as usize)
    {
        bail!(
            "MathClosure {} outputs do not match its authored output contract",
            node.id
        );
    }
    let inputs = node
        .inputs
        .iter()
        .map(compiler_port)
        .collect::<Result<Vec<_>>>()?;
    let stage_name = match stage {
        GlslShaderStage::Vertex => "vertex",
        GlslShaderStage::Fragment => "fragment",
        GlslShaderStage::Compute => "compute",
    };
    let base = format!(
        "nf_{}_{}",
        super::readable_symbol_base(node, "output"),
        stage_name
    );
    let namespace = ctx.allocate_local_name(&format!("fn:{stage_name}:{}", node.id), &base);
    let function_name = format!("{namespace}_closure");
    let compiled = node_forge_math_closure::compile(&Request {
        source: source.into(),
        inputs: inputs.clone(),
        output: output.clone(),
        function_name,
    })
    .map_err(|diagnostics| {
        anyhow!(
            "MathClosure {}: {}",
            node.id,
            diagnostics
                .iter()
                .map(|d| format!("L{}:{} {}", d.start_row + 1, d.start_column + 1, d.message))
                .collect::<Vec<_>>()
                .join("\n")
        )
    })?;

    // Public uv is LocalUV. RasterUV is private to the renderer's sampling boundary.
    let raster_uv = if matches!(stage, GlslShaderStage::Vertex) {
        "uv"
    } else {
        "in.uv"
    };
    let mut args = vec![format!("vec2f({raster_uv}.x, 1.0 - {raster_uv}.y)")];
    let mut uses_time = false;
    for (port, input) in node.inputs.iter().zip(&inputs) {
        if input.port_type == "texture" {
            let connection = incoming_connection(scene, &node.id, &port.id).ok_or_else(|| {
                anyhow!(
                    "MathClosure {} texture input {} is not connected",
                    node.id,
                    port.id
                )
            })?;
            let resource = resolve_texture_source_ref(scene, nodes_by_id, &connection.from)
                .with_context(|| format!("MathClosure texture {}", input.name))?;
            let (texture, sampler) = match resource {
                TextureSourceRef::Image {
                    binding_id,
                    image_node_id,
                    sampler_node_id,
                } => {
                    ctx.register_image_texture_ref(ImageTextureRef {
                        binding_id: binding_id.clone(),
                        image_node_id,
                        sampler_node_id,
                    });
                    (
                        MaterialCompileContext::tex_var_name(&binding_id),
                        MaterialCompileContext::sampler_var_name(&binding_id),
                    )
                }
                TextureSourceRef::Surface(reference) => {
                    let binding_id = reference.binding_id.clone();
                    ctx.register_pass_texture_ref(reference);
                    (
                        MaterialCompileContext::pass_tex_var_name(&binding_id),
                        MaterialCompileContext::pass_sampler_var_name(&binding_id),
                    )
                }
            };
            let helper = compiled
                .textures
                .iter()
                .find(|t| t.input_name == input.name)
                .ok_or_else(|| anyhow!("Compiler omitted texture binding {}", input.name))?;
            let sample = if matches!(stage, GlslShaderStage::Vertex) {
                format!("textureSampleLevel({texture}, {sampler}, raster_uv, 0.0)")
            } else {
                format!("textureSample({texture}, {sampler}, raster_uv)")
            };
            ctx.extra_wgsl_decls.insert(helper.function_name.clone(), format!(
                "fn {}(uv: vec2f) -> vec4f {{\n    let raster_uv = vec2f(uv.x, 1.0 - uv.y);\n    return {sample};\n}}", helper.function_name));
        } else {
            let ty = render_type(input)?;
            let argument = if let Some(connection) = incoming_connection(scene, &node.id, &port.id)
            {
                coerce_to_type(
                    compile_fn(
                        &connection.from.node_id,
                        Some(&connection.from.port_id),
                        ctx,
                        cache,
                    )?,
                    ty,
                )?
            } else {
                // Typed zero, including fixed-size arrays. Never infer a type from source text.
                TypedExpr::new(format!("{}()", ty.wgsl_type_string()), ty)
            };
            if argument.ty != ty {
                bail!(
                    "MathClosure {} input {} has incompatible array length",
                    node.id,
                    input.name
                );
            }
            uses_time |= argument.uses_time;
            args.push(argument.expr);
        }
    }
    let ret_ty = render_type(&output)?;
    let output_var = super::readable_node_temp_name(ctx, stage_name, node, "output", "out");
    ctx.inline_stmts.push(format!(
        "    let {output_var} = {}({});",
        compiled.function_name,
        args.join(", ")
    ));
    ctx.extra_wgsl_decls
        .insert(compiled.function_name, compiled.wgsl);
    Ok(TypedExpr::with_time(output_var, ret_ty, uses_time))
}
