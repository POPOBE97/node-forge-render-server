use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use crate::dsl::{Node, SceneDSL};
use crate::renderer::types::{MaterialCompileContext, TypedExpr, ValueType};

fn map_port_type(s: Option<&str>) -> Result<ValueType> {
    let Some(s) = s else {
        return Ok(ValueType::F32);
    };
    let t = s.to_ascii_lowercase();
    match t.as_str() {
        "float" | "f32" | "number" => Ok(ValueType::F32),
        "int" | "i32" => Ok(ValueType::I32),
        "uint" | "u32" => Ok(ValueType::U32),
        "bool" | "boolean" => Ok(ValueType::Bool),
        "vector2" | "vec2" => Ok(ValueType::Vec2),
        "vector3" | "vec3" => Ok(ValueType::Vec3),
        "vector4" | "vec4" | "color" => Ok(ValueType::Vec4),
        other => bail!("unsupported DataParse port type: {other}"),
    }
}

fn default_value_for(ty: ValueType) -> TypedExpr {
    match ty {
        ValueType::F32 => TypedExpr::new("0.0", ValueType::F32),
        ValueType::I32 => TypedExpr::new("0", ValueType::I32),
        ValueType::U32 => TypedExpr::new("0u", ValueType::U32),
        ValueType::Bool => TypedExpr::new("false", ValueType::Bool),
        ValueType::Texture2D => unreachable!("DataParse cannot produce Texture2D values"),
        ValueType::Vec2 => TypedExpr::new("vec2f(0.0, 0.0)", ValueType::Vec2),
        ValueType::Vec3 => TypedExpr::new("vec3f(0.0, 0.0, 0.0)", ValueType::Vec3),
        ValueType::Vec4 => TypedExpr::new("vec4f(0.0, 0.0, 0.0, 0.0)", ValueType::Vec4),
        _ if ty.is_array() => unreachable!("DataParse cannot produce array values"),
        _ => unreachable!(),
    }
}

pub fn compile_data_parse<F>(
    _scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    _cache: &mut HashMap<(String, String), TypedExpr>,
    _compile_fn: F,
    stage: crate::renderer::validation::GlslShaderStage,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    let port_id = out_port.ok_or_else(|| anyhow!("DataParse requires an output port"))?;

    let declared = node
        .outputs
        .iter()
        .find(|p| p.id == port_id)
        .and_then(|p| p.port_type.as_deref());
    let out_ty = map_port_type(declared)?;

    let Some(meta) = ctx.baked_data_parse_meta.as_ref() else {
        return Ok(default_value_for(out_ty));
    };

    let Some(slot) = meta.slot_for(meta.pass_id.as_str(), node.id.as_str(), port_id) else {
        return Ok(default_value_for(out_ty));
    };

    ctx.uses_instance_index = true;

    let ix = match stage {
        crate::renderer::validation::GlslShaderStage::Vertex => "instance_index",
        crate::renderer::validation::GlslShaderStage::Fragment => "instance_index",
        crate::renderer::validation::GlslShaderStage::Compute => "0u",
    };

    let out = match out_ty {
        ValueType::F32 => TypedExpr::new(
            format!(
                "baked_data_parse[({ix}) * {}u + {}u].x",
                meta.outputs_per_instance, slot
            ),
            ValueType::F32,
        ),
        ValueType::I32 => TypedExpr::new(
            format!(
                "i32(baked_data_parse[({ix}) * {}u + {}u].x)",
                meta.outputs_per_instance, slot
            ),
            ValueType::I32,
        ),
        ValueType::U32 => TypedExpr::new(
            format!(
                "u32(baked_data_parse[({ix}) * {}u + {}u].x)",
                meta.outputs_per_instance, slot
            ),
            ValueType::U32,
        ),
        ValueType::Bool => TypedExpr::new(
            format!(
                "(baked_data_parse[({ix}) * {}u + {}u].x != 0.0)",
                meta.outputs_per_instance, slot
            ),
            ValueType::Bool,
        ),
        ValueType::Vec2 => TypedExpr::new(
            format!(
                "baked_data_parse[({ix}) * {}u + {}u].xy",
                meta.outputs_per_instance, slot
            ),
            ValueType::Vec2,
        ),
        ValueType::Vec3 => TypedExpr::new(
            format!(
                "baked_data_parse[({ix}) * {}u + {}u].xyz",
                meta.outputs_per_instance, slot
            ),
            ValueType::Vec3,
        ),
        ValueType::Vec4 => TypedExpr::new(
            format!(
                "baked_data_parse[({ix}) * {}u + {}u]",
                meta.outputs_per_instance, slot
            ),
            ValueType::Vec4,
        ),
        ValueType::Texture2D => unreachable!("DataParse cannot produce Texture2D values"),
        _ if out_ty.is_array() => unreachable!("DataParse cannot produce array values"),
        _ => unreachable!(),
    };

    Ok(out)
}
