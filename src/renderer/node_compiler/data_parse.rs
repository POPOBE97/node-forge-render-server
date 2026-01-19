use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use crate::dsl::{Node, SceneDSL};
use crate::renderer::types::{BakedValue, MaterialCompileContext, TypedExpr, ValueType};
use crate::renderer::utils::fmt_f32;

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

    let baked = ctx.baked_data_parse.as_ref();
    if baked.is_none() {
        return Ok(default_value_for(out_ty));
    }
    let baked = baked.unwrap();

    let Some(vs) = baked.get(&("__global".to_string(), node.id.clone(), port_id.to_string()))
    else {
        return Ok(default_value_for(out_ty));
    };
    let Some(v) = vs.get(0) else {
        return Ok(default_value_for(out_ty));
    };

    let typed = match (out_ty, v) {
        (ValueType::F32, BakedValue::F32(x)) => TypedExpr::new(fmt_f32(*x), ValueType::F32),
        (ValueType::I32, BakedValue::I32(x)) => TypedExpr::new(format!("{x}"), ValueType::I32),
        (ValueType::U32, BakedValue::U32(x)) => TypedExpr::new(format!("{x}u"), ValueType::U32),
        (ValueType::Bool, BakedValue::Bool(x)) => {
            TypedExpr::new(if *x { "true" } else { "false" }, ValueType::Bool)
        }
        (ValueType::Vec2, BakedValue::Vec2([x, y])) => TypedExpr::new(
            format!("vec2f({}, {})", fmt_f32(*x), fmt_f32(*y)),
            ValueType::Vec2,
        ),
        (ValueType::Vec3, BakedValue::Vec3([x, y, z])) => TypedExpr::new(
            format!("vec3f({}, {}, {})", fmt_f32(*x), fmt_f32(*y), fmt_f32(*z)),
            ValueType::Vec3,
        ),
        (ValueType::Vec4, BakedValue::Vec4([x, y, z, w])) => TypedExpr::new(
            format!(
                "vec4f({}, {}, {}, {})",
                fmt_f32(*x),
                fmt_f32(*y),
                fmt_f32(*z),
                fmt_f32(*w)
            ),
            ValueType::Vec4,
        ),
        (expected, other) => bail!(
            "DataParse baked value type mismatch for node={} port={port_id}: expected {:?}, got {:?}",
            node.id,
            expected,
            other
        ),
    };

    Ok(typed)
}
