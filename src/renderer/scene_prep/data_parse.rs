use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    dsl::{InputBinding, Node, SourceBinding, find_node},
    renderer::types::{BakedValue, ValueType},
    ts_runtime::TsRuntime,
};

fn map_baked_type(s: Option<&str>) -> Result<ValueType> {
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
        "texture" => Ok(ValueType::Texture2D),
        other => bail!("unsupported DataParse port type: {other}"),
    }
}

fn string_param<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
    node.params.get(key)?.as_str()
}

fn data_node_json(nodes_by_id: &HashMap<String, Node>, id: &str) -> Result<serde_json::Value> {
    let data_node = find_node(nodes_by_id, id)?;
    let text = string_param(data_node, "text").unwrap_or("");
    if text.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(text)
        .with_context(|| format!("failed to parse DataNode.text as JSON for {id}"))
}

fn resolve_binding_value(
    nodes_by_id: &HashMap<String, Node>,
    binding: &InputBinding,
    index_value: u32,
) -> Result<serde_json::Value> {
    let Some(SourceBinding {
        node_id,
        output_port_id,
        ..
    }) = binding.source_binding.as_ref()
    else {
        return Ok(serde_json::Value::Null);
    };

    match output_port_id.as_str() {
        "data" => data_node_json(nodes_by_id, node_id),
        "index" => Ok(serde_json::json!(index_value)),
        _ => Ok(serde_json::Value::Null),
    }
}

fn baked_from_json(ty: ValueType, v: &serde_json::Value) -> Result<BakedValue> {
    match ty {
        ValueType::F32 => Ok(BakedValue::F32(
            v.as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
        )),
        ValueType::I32 => Ok(BakedValue::I32(
            v.as_i64().ok_or_else(|| anyhow!("expected int"))? as i32,
        )),
        ValueType::U32 => Ok(BakedValue::U32(
            v.as_u64().ok_or_else(|| anyhow!("expected uint"))? as u32,
        )),
        ValueType::Bool => Ok(BakedValue::Bool(
            v.as_bool().ok_or_else(|| anyhow!("expected bool"))?,
        )),
        ValueType::Vec2 => {
            let arr = v.as_array().ok_or_else(|| anyhow!("expected array"))?;
            if arr.len() != 2 {
                bail!("expected vec2 array length 2, got {}", arr.len());
            }
            Ok(BakedValue::Vec2([
                arr[0].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
                arr[1].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
            ]))
        }
        ValueType::Vec3 => {
            let arr = v.as_array().ok_or_else(|| anyhow!("expected array"))?;
            if arr.len() != 3 {
                bail!("expected vec3 array length 3, got {}", arr.len());
            }
            Ok(BakedValue::Vec3([
                arr[0].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
                arr[1].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
                arr[2].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
            ]))
        }
        ValueType::Vec4 => {
            let arr = v.as_array().ok_or_else(|| anyhow!("expected array"))?;
            if arr.len() != 4 {
                bail!("expected vec4 array length 4, got {}", arr.len());
            }
            Ok(BakedValue::Vec4([
                arr[0].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
                arr[1].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
                arr[2].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
                arr[3].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
            ]))
        }

        // DataParse outputs are baked CPU-side; GPU resources are not supported here.
        ValueType::Texture2D => bail!("cannot bake DataParse output type 'texture'"),
        _ if ty.is_array() => bail!("cannot bake DataParse output type 'array'"),
        _ => unreachable!(),
    }
}

pub(crate) fn bake_data_parse_nodes(
    nodes_by_id: &HashMap<String, Node>,
    pass_id: &str,
    instance_count: u32,
) -> Result<HashMap<(String, String, String), Vec<BakedValue>>> {
    let mut baked: HashMap<(String, String, String), Vec<BakedValue>> = HashMap::new();
    let mut rt = TsRuntime::new();

    for node in nodes_by_id.values() {
        if node.node_type != "DataParse" {
            continue;
        }

        let src = string_param(node, "source")
            .ok_or_else(|| anyhow!("DataParse missing params.source for {}", node.id))?;

        let port_types: HashMap<String, ValueType> = node
            .outputs
            .iter()
            .map(|p| {
                let ty = map_baked_type(p.port_type.as_deref()).with_context(|| {
                    format!("invalid output port type for {}.{}", node.id, p.id)
                })?;
                Ok((p.id.clone(), ty))
            })
            .collect::<Result<_>>()?;

        let capped_instance_count = instance_count.min(1024);
        for i in 0..capped_instance_count {
            let mut bindings_src = String::new();
            for b in &node.input_bindings {
                let val = resolve_binding_value(nodes_by_id, b, i).with_context(|| {
                    format!(
                        "failed to resolve input binding {} for {}",
                        b.variable_name, node.id
                    )
                })?;
                let json = serde_json::to_string(&val)?;
                bindings_src.push_str(&format!("const {} = {};\n", b.variable_name, json));
            }
            if !node
                .input_bindings
                .iter()
                .any(|b| b.variable_name == "index")
            {
                bindings_src.push_str(&format!("const index = {};\n", i));
            }

            let mut user_src = src.to_string();
            user_src = user_src.replace(" as vec2", "");
            user_src = user_src.replace(" as vec3", "");
            user_src = user_src.replace(" as vec4", "");
            user_src = user_src.replace(" as int", "");
            user_src = user_src.replace(" as i32", "");
            user_src = user_src.replace(" as uint", "");
            user_src = user_src.replace(" as u32", "");
            user_src = user_src.replace(" as float", "");
            user_src = user_src.replace(" as f32", "");
            user_src = user_src.replace(" as number", "");
            user_src = user_src.replace(" as bool", "");
            user_src = user_src.replace(" as boolean", "");

            let script_body = format!("{bindings_src}\n{user_src}\n");
            let script = format!("(function() {{\n{}\n}})()", script_body);
            let out: serde_json::Value = match rt.eval_script(&script) {
                Ok(v) => v,
                Err(_) => serde_json::Value::Object(serde_json::Map::new()),
            };
            let out_obj = out.as_object();

            for p in &node.outputs {
                let key = p.name.as_deref().unwrap_or(p.id.as_str());
                let ty = *port_types
                    .get(&p.id)
                    .ok_or_else(|| anyhow!("missing port type"))?;
                let v = out_obj
                    .and_then(|o| o.get(key))
                    .unwrap_or(&serde_json::Value::Null);
                let baked_v = baked_from_json(ty, v).unwrap_or_else(|_| match ty {
                    ValueType::F32 => BakedValue::F32(0.0),
                    ValueType::I32 => BakedValue::I32(0),
                    ValueType::U32 => BakedValue::U32(0),
                    ValueType::Bool => BakedValue::Bool(false),
                    ValueType::Vec2 => BakedValue::Vec2([0.0, 0.0]),
                    ValueType::Vec3 => BakedValue::Vec3([0.0, 0.0, 0.0]),
                    ValueType::Vec4 => BakedValue::Vec4([0.0, 0.0, 0.0, 0.0]),
                    ValueType::Texture2D => BakedValue::Vec4([0.0, 0.0, 0.0, 0.0]),
                    // Array types are not used in DataParse baking.
                    _ => BakedValue::Vec4([0.0, 0.0, 0.0, 0.0]),
                });

                baked
                    .entry((pass_id.to_string(), node.id.clone(), p.id.clone()))
                    .or_default()
                    .push(baked_v);
            }
        }
    }

    Ok(baked)
}
