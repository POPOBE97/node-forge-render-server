//! HDRI environment reflection with geometry-local normals.
use std::collections::HashMap;

use anyhow::{Context, Result, ensure};
use serde_json::json;

use crate::{
    dsl::{Node, SceneDSL, incoming_connection},
    renderer::{
        types::{MaterialCompileContext, TypedExpr, ValueType},
        utils::{coerce_to_type, fmt_f32},
    },
};

pub fn compile<F>(
    scene: &SceneDSL,
    node: &Node,
    out_port: Option<&str>,
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
    ensure!(
        matches!(out_port.unwrap_or("material"), "material" | "color"),
        "EnvMaterial only outputs material or color"
    );
    let mut args = Vec::new();
    let mut uses_time = false;
    for (port, ty, default) in [
        ("normal", ValueType::Vec3, json!([0.0, 0.0, 1.0])),
        ("normalStrength", ValueType::F32, json!(1.0)),
        ("view", ValueType::Vec3, json!([0.0, 0.0, 1.0])),
        ("roughness", ValueType::F32, json!(0.0)),
        ("rotation", ValueType::F32, json!(0.0)),
        ("exposure", ValueType::F32, json!(0.0)),
        ("intensity", ValueType::F32, json!(1.0)),
        ("tint", ValueType::Vec4, json!([1.0, 1.0, 1.0, 1.0])),
        ("alpha", ValueType::F32, json!(1.0)),
    ] {
        let value = if let Some(edge) = incoming_connection(scene, &node.id, port) {
            coerce_to_type(
                compile_fn(&edge.from.node_id, Some(&edge.from.port_id), ctx, cache)?,
                ty,
            )?
        } else {
            let value = node.params.get(port).unwrap_or(&default);
            let count = match ty {
                ValueType::F32 => 1,
                ValueType::Vec3 => 3,
                _ => 4,
            };
            let mut values = if count == 4 && value.is_string() {
                crate::color::hex_to_working(value.as_str().unwrap(), scene.working_color_space)
                    .context("EnvMaterial invalid tint color")?
                    .to_vec()
            } else {
                let keys = if value.get("r").is_some() {
                    ["r", "g", "b", "a"]
                } else {
                    ["x", "y", "z", "w"]
                };
                (0..count)
                    .map(|i| {
                        let component = if count == 1 {
                            Some(value)
                        } else if value.is_array() {
                            value.get(i)
                        } else {
                            value.get(keys[i])
                        };
                        component
                            .and_then(|v| v.as_f64())
                            .map(|v| v as f32)
                            .with_context(|| {
                                format!("EnvMaterial {port} requires {count} finite components")
                            })
                    })
                    .collect::<Result<Vec<_>>>()?
            };
            ensure!(
                values.iter().all(|v| v.is_finite()),
                "EnvMaterial {port} must be finite"
            );
            if count == 4 {
                for i in 0..3 {
                    values[i] *= values[3];
                }
            }
            let literal = values
                .into_iter()
                .map(fmt_f32)
                .collect::<Vec<_>>()
                .join(", ");
            TypedExpr::new(
                if count == 1 {
                    literal
                } else {
                    format!("vec{count}f({literal})")
                },
                ty,
            )
        };
        uses_time |= value.uses_time;
        args.push(value.expr);
    }
    ctx.register_image_texture(&node.id);
    ctx.needs_view_vector = true;
    ctx.extra_wgsl_decls
        .entry("env_material".into())
        .or_insert_with(|| include_str!("templates/env_material.wgsl").into());
    let texture = MaterialCompileContext::tex_var_name(&node.id);
    let address_u = sampler_option(
        node,
        "addressModeU",
        &["clamp-to-edge", "repeat", "mirror-repeat"],
        "repeat",
    )?;
    let address_v = sampler_option(
        node,
        "addressModeV",
        &["clamp-to-edge", "repeat", "mirror-repeat"],
        "clamp-to-edge",
    )?;
    let mag = sampler_option(node, "magFilter", &["nearest", "linear"], "linear")?;
    let min = sampler_option(node, "minFilter", &["nearest", "linear"], "linear")?;
    Ok(TypedExpr::with_time(
        format!(
            "nf_env_material({texture}, vec2i({address_u}, {address_v}), vec2i({mag}, {min}), in.world_pos, in.local_px.xy, {})",
            args.join(", ")
        ),
        ValueType::Vec4,
        uses_time,
    ))
}

fn sampler_option(node: &Node, key: &str, values: &[&str], default: &str) -> Result<usize> {
    let value = match node.params.get(key) {
        Some(value) => value
            .as_str()
            .with_context(|| format!("EnvMaterial {key} must be a string"))?,
        None => default,
    };
    values
        .iter()
        .position(|candidate| *candidate == value)
        .with_context(|| format!("EnvMaterial unsupported {key}: {value}"))
}
