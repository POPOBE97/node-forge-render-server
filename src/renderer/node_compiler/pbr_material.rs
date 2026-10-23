//! Metallic/roughness GGX material, with explicit world-space studio lighting.
use crate::{
    dsl::{Node, SceneDSL, incoming_connection},
    renderer::{
        types::{MaterialCompileContext, TypedExpr, ValueType},
        utils::{coerce_to_type, fmt_f32},
    },
};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::collections::HashMap;

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
        out_port.unwrap_or("material") == "material",
        "PBRMaterial only outputs material"
    );
    let mut args = Vec::new();
    let mut uses_time = false;
    for (port, ty, default) in [
        ("baseColor", ValueType::Vec4, json!([0.8, 0.8, 0.8, 1.0])),
        ("metallic", ValueType::F32, json!(0.0)),
        ("roughness", ValueType::F32, json!(0.5)),
        ("normal", ValueType::Vec3, json!([0.0, 0.0, 1.0])),
        ("normalStrength", ValueType::F32, json!(1.0)),
        ("emission", ValueType::Vec4, json!([0.0, 0.0, 0.0, 1.0])),
        ("alpha", ValueType::F32, json!(1.0)),
        ("lightDirection", ValueType::Vec3, json!([-0.4, 0.6, 1.0])),
        ("lightColor", ValueType::Vec4, json!([1.0, 1.0, 1.0, 1.0])),
        ("lightIntensity", ValueType::F32, json!(3.0)),
        (
            "ambientColor",
            ValueType::Vec4,
            json!([0.08, 0.08, 0.08, 1.0]),
        ),
        ("viewDirection", ValueType::Vec3, json!([0.0, 0.0, 1.0])),
    ] {
        let value = if let Some(edge) = incoming_connection(scene, &node.id, port) {
            coerce_to_type(
                compile_fn(&edge.from.node_id, Some(&edge.from.port_id), ctx, cache)?,
                ty,
            )?
        } else {
            let value = node.params.get(port).unwrap_or(&default);
            let n = match ty {
                ValueType::F32 => 1,
                ValueType::Vec3 => 3,
                _ => 4,
            };
            let mut values = if n == 4 && value.is_string() {
                crate::color::hex_to_working(value.as_str().unwrap(), scene.working_color_space)
                    .context("PBRMaterial invalid hex color")?
                    .to_vec()
            } else {
                let keys = if value.get("r").is_some() {
                    ["r", "g", "b", "a"]
                } else {
                    ["x", "y", "z", "w"]
                };
                (0..n)
                    .map(|i| {
                        let v = if n == 1 {
                            Some(value)
                        } else if value.is_array() {
                            value.get(i)
                        } else {
                            value.get(keys[i])
                        };
                        v.and_then(|v| v.as_f64())
                            .map(|v| v as f32)
                            .with_context(|| {
                                format!("PBRMaterial {port} requires {n} finite components")
                            })
                    })
                    .collect::<Result<Vec<_>>>()?
            };
            ensure!(
                values.iter().all(|v| v.is_finite()),
                "PBRMaterial {port} must be finite"
            );
            if n == 4 {
                for i in 0..3 {
                    values[i] *= values[3];
                }
            }
            let expr = values
                .into_iter()
                .map(fmt_f32)
                .collect::<Vec<_>>()
                .join(", ");
            TypedExpr::new(
                if n == 1 {
                    expr
                } else {
                    format!("vec{n}f({expr})")
                },
                ty,
            )
        };
        uses_time |= value.uses_time;
        args.push(value.expr);
    }
    ctx.needs_view_vector = true;
    ctx.extra_wgsl_decls
        .entry("pbr_material".into())
        .or_insert_with(|| include_str!("templates/pbr_material.wgsl").into());
    Ok(TypedExpr::with_time(
        format!("nf_pbr(in.world_pos, in.local_px.xy, {})", args.join(", ")),
        ValueType::Vec4,
        uses_time,
    ))
}
