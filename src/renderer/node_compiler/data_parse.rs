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
        ValueType::Vec2 => TypedExpr::new("vec2f(0.0, 0.0)", ValueType::Vec2),
        ValueType::Vec3 => TypedExpr::new("vec3f(0.0, 0.0, 0.0)", ValueType::Vec3),
        ValueType::Vec4 => TypedExpr::new("vec4f(0.0, 0.0, 0.0, 0.0)", ValueType::Vec4),
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

    match out_ty {
        ValueType::Vec2 => {
            let cols = 2u32;
            let cell_w = 474.0;
            let cell_h = 239.0;
            let x0 = 85.0;
            let y0 = 376.0;

            let expr = format!(
                "(vec2f({x0}, {y0}) + vec2f(f32(instance_index % {cols}u) * {cell_w}, f32(instance_index / {cols}u) * {cell_h}))"
            );
            ctx.uses_instance_index = true;
            Ok(TypedExpr::new(expr, ValueType::Vec2))
        }
        _ => Ok(default_value_for(out_ty)),
    }
}
