//! Stateless radial height field and its analytic geometry-local normal.
use std::collections::HashMap;

use anyhow::{Context, Result, bail, ensure};
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
    let port = out_port.unwrap_or("height");
    let ty = match port {
        "height" => ValueType::F32,
        "dir" => ValueType::Vec2,
        "normal" => ValueType::Vec3,
        _ => bail!("Wave: unsupported output port '{port}'"),
    };
    let mut args = Vec::new();
    let mut uses_time = false;
    for (input, input_ty, default) in [
        ("progress", ValueType::F32, json!(0.0)),
        ("origin", ValueType::Vec2, json!([0.0, 0.0])),
        ("amplitude", ValueType::F32, json!(0.2)),
        ("frequency", ValueType::F32, json!(100.0)),
        ("decay", ValueType::F32, json!(27.0)),
        ("speed", ValueType::F32, json!(1.0)),
        ("damping", ValueType::F32, json!(3.7)),
        ("front_feather", ValueType::F32, json!(0.0)),
    ] {
        let value = if let Some(edge) = incoming_connection(scene, &node.id, input) {
            coerce_to_type(
                compile_fn(&edge.from.node_id, Some(&edge.from.port_id), ctx, cache)?,
                input_ty,
            )?
        } else {
            let value = node.params.get(input).unwrap_or(&default);
            let count = if input_ty == ValueType::Vec2 { 2 } else { 1 };
            let values = (0..count)
                .map(|i| {
                    let component = if count == 1 {
                        Some(value)
                    } else if value.is_array() {
                        value.get(i)
                    } else {
                        value.get(["x", "y"][i])
                    };
                    let number = component.and_then(|v| v.as_f64()).with_context(|| {
                        format!(
                            "Wave {}.{input} requires {count} finite components",
                            node.id
                        )
                    })? as f32;
                    ensure!(
                        number.is_finite(),
                        "Wave {}.{input} must be finite",
                        node.id
                    );
                    Ok(fmt_f32(number))
                })
                .collect::<Result<Vec<_>>>()?;
            TypedExpr::new(
                if count == 1 {
                    values[0].clone()
                } else {
                    format!("vec2f({})", values.join(", "))
                },
                input_ty,
            )
        };
        uses_time |= value.uses_time;
        args.push(value.expr);
    }
    ctx.extra_wgsl_decls
        .entry("wave".into())
        .or_insert_with(|| include_str!("templates/wave.wgsl").into());
    Ok(TypedExpr::with_time(
        format!(
            "nf_wave(in.local_px.xy, in.geo_size_px.y, {}).{port}",
            args.join(", ")
        ),
        ty,
        uses_time,
    ))
}
