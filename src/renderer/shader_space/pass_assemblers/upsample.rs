//! Upsample pass assembler.
//!
//! Handles the `"Upsample"` node type. Upsamples source pass output into a
//! `targetSize`-sized texture using bilinear filtering. Optionally adds a fit
//! pass to scale to the Composite target size.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color},
};

use crate::{
    dsl::{incoming_connection, parse_str, Node},
    renderer::{
        camera::{legacy_projection_camera_matrix, resolve_effective_camera_for_pass_node},
        graph_uniforms::graph_field_name,
        types::{MaterialCompileContext, PassOutputSpec, TypedExpr, ValueType},
        utils::{coerce_to_type, cpu_num_f32},
        wgsl::{build_fullscreen_textured_bundle, build_upsample_bilinear_bundle},
    },
};

use super::args::{BuilderState, SceneContext};
use super::super::pass_spec::{
    PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl,
    make_params,
};

/// Assemble an `"Upsample"` layer.
pub(crate) fn assemble_upsample(
    sc: &SceneContext<'_>,
    bs: &mut BuilderState<'_>,
    layer_id: &str,
    layer_node: &Node,
) -> Result<()> {
    let scene = sc.scene();
    let nodes_by_id = sc.nodes_by_id();
    let tgt_w = bs.tgt_size[0];
    let tgt_h = bs.tgt_size[1];
    let tgt_w_u = bs.tgt_size_u[0];
    let tgt_h_u = bs.tgt_size_u[1];

    let pass_name: ResourceName = format!("sys.upsample.{layer_id}.pass").into();
    let pass_blend_state =
        crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
            .with_context(|| {
                format!(
                    "invalid blend params for {}",
                    crate::dsl::node_display_label_with_id(layer_node)
                )
            })?;

    let src_conn = incoming_connection(scene, layer_id, "source")
        .ok_or_else(|| anyhow!("Upsample.source missing for {layer_id}"))?;
    let src_pass_id = src_conn.from.node_id.clone();
    let src_tex = bs
        .pass_output_registry
        .get_texture(&src_pass_id)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "Upsample.source references upstream pass {src_pass_id}, but its output texture is not registered yet"
            )
        })?;

    fn parse_json_number_f32(v: &serde_json::Value) -> Option<f32> {
        v.as_f64()
            .map(|x| x as f32)
            .or_else(|| v.as_i64().map(|x| x as f32))
            .or_else(|| v.as_u64().map(|x| x as f32))
    }

    fn to_positive_target_size(layer_id: &str, x: f32, y: f32) -> Result<(u32, u32)> {
        if !x.is_finite() || !y.is_finite() {
            bail!(
                "Upsample.targetSize must have finite components for {layer_id}, got ({x}, {y})"
            );
        }
        if x <= 0.0 || y <= 0.0 {
            bail!(
                "Upsample.targetSize must have positive components for {layer_id}, got ({x}, {y})"
            );
        }
        Ok((x.round().max(1.0) as u32, y.round().max(1.0) as u32))
    }

    let target_size_expr = if let Some(target_size_conn) =
        incoming_connection(scene, layer_id, "targetSize")
    {
        let target_size_expr = {
            let mut ctx = MaterialCompileContext::default();
            let mut cache: HashMap<(String, String), TypedExpr> = HashMap::new();
            crate::renderer::node_compiler::compile_material_expr(
                scene,
                &nodes_by_id,
                &target_size_conn.from.node_id,
                Some(&target_size_conn.from.port_id),
                &mut ctx,
                &mut cache,
            )?
        };
        coerce_to_type(target_size_expr, ValueType::Vec2)?
    } else if let Some(v) = layer_node.params.get("targetSize") {
        let (x, y) = if let Some(arr) = v.as_array() {
            let x = arr
                .first()
                .and_then(parse_json_number_f32)
                .ok_or_else(|| {
                    anyhow!(
                        "Upsample.targetSize must be an object {{x,y}} or array [x,y] in params for {layer_id}"
                    )
                })?;
            let y = arr
                .get(1)
                .and_then(parse_json_number_f32)
                .ok_or_else(|| {
                    anyhow!(
                        "Upsample.targetSize must be an object {{x,y}} or array [x,y] in params for {layer_id}"
                    )
                })?;
            (x, y)
        } else if let Some(obj) = v.as_object() {
            let x = obj
                .get("x")
                .and_then(parse_json_number_f32)
                .ok_or_else(|| {
                    anyhow!(
                        "Upsample.targetSize must be an object {{x,y}} or array [x,y] in params for {layer_id}"
                    )
                })?;
            let y = obj
                .get("y")
                .and_then(parse_json_number_f32)
                .ok_or_else(|| {
                    anyhow!(
                        "Upsample.targetSize must be an object {{x,y}} or array [x,y] in params for {layer_id}"
                    )
                })?;
            (x, y)
        } else {
            bail!(
                "Upsample.targetSize must be an object {{x,y}} or array [x,y] in params for {layer_id}"
            );
        };
        TypedExpr::new(format!("vec2f({x}, {y})"), ValueType::Vec2)
    } else {
        bail!("missing input '{layer_id}.targetSize' (no connection and no param)");
    };

    let (out_w, out_h) = {
        let s = target_size_expr.expr.replace([' ', '\n', '\t', '\r'], "");
        if let Some(inner) = s
            .strip_prefix("(graph_inputs.")
            .and_then(|x| x.strip_suffix(").xy"))
        {
            if let Some((_node_id, node)) = nodes_by_id.iter().find(|(_, n)| {
                n.node_type == "Vector2Input" && graph_field_name(&n.id) == inner
            }) {
                let w = cpu_num_f32(scene, &nodes_by_id, node, "x", 0.0)?;
                let h = cpu_num_f32(scene, &nodes_by_id, node, "y", 0.0)?;
                to_positive_target_size(layer_id, w, h)?
            } else {
                bail!(
                    "Upsample.targetSize must be a CPU-constant vec2f(w,h) for now, got {}",
                    target_size_expr.expr
                );
            }
        } else if let Some(inner) =
            s.strip_prefix("vec2f(").and_then(|x| x.strip_suffix(')'))
        {
            let parts: Vec<&str> = inner.split(',').collect();
            if parts.len() == 2 {
                let w = parts[0].parse::<f32>().map_err(|_| {
                    anyhow!(
                        "Upsample.targetSize must be vec2f(w,h), got {}",
                        target_size_expr.expr
                    )
                })?;
                let h = parts[1].parse::<f32>().map_err(|_| {
                    anyhow!(
                        "Upsample.targetSize must be vec2f(w,h), got {}",
                        target_size_expr.expr
                    )
                })?;
                to_positive_target_size(layer_id, w, h)?
            } else {
                bail!(
                    "Upsample.targetSize must be vec2f(w,h), got {}",
                    target_size_expr.expr
                );
            }
        } else {
            bail!(
                "Upsample.targetSize must be a CPU-constant vec2f(w,h) for now, got {}",
                target_size_expr.expr
            );
        }
    };

    let is_sampled_output = bs.sampled_pass_ids.contains(layer_id);
    let needs_intermediate = is_sampled_output || (out_w != tgt_w_u || out_h != tgt_h_u);
    let writes_scene_output_target = !is_sampled_output;

    let upsample_out_tex: ResourceName = if needs_intermediate {
        let tex: ResourceName = format!("sys.upsample.{layer_id}.out").into();
        bs.textures.push(TextureDecl {
            name: tex.clone(),
            size: [out_w, out_h],
            format: if is_sampled_output { bs.sampled_pass_format } else { bs.target_format },
            sample_count: 1,
            needs_sampling: false,
        });
        tex
    } else {
        bs.target_texture_name.clone()
    };

    let geo: ResourceName = format!("sys.upsample.{layer_id}.geo").into();
    bs.push_fullscreen_geometry(geo.clone(), out_w as f32, out_h as f32);

    let params_name: ResourceName = format!("params.sys.upsample.{layer_id}").into();
    let out_w_f = out_w as f32;
    let out_h_f = out_h as f32;
    let params_val = make_params(
        [out_w_f, out_h_f],
        [out_w_f, out_h_f],
        [out_w_f * 0.5, out_h_f * 0.5],
        resolve_effective_camera_for_pass_node(scene, &nodes_by_id, layer_node, [out_w_f, out_h_f])?,
        [0.0, 0.0, 0.0, 0.0],
    );

    let address_mode = parse_str(&layer_node.params, "address_mode")
        .unwrap_or("clamp-to-edge")
        .trim()
        .to_ascii_lowercase();
    let filter = parse_str(&layer_node.params, "filter")
        .unwrap_or("linear")
        .trim()
        .to_ascii_lowercase();

    let address = match address_mode.as_str() {
        "clamp-to-edge" => "clamp",
        "repeat" => "repeat",
        "mirror-repeat" => "mirror",
        other => bail!("Upsample: invalid address_mode: {other}"),
    };
    let nearest = match filter.as_str() {
        "nearest" => true,
        "linear" => false,
        other => bail!("Upsample: invalid filter: {other}"),
    };
    let sampler_kind = match (nearest, address) {
        (true, "mirror") => SamplerKind::NearestMirror,
        (true, "repeat") => SamplerKind::NearestRepeat,
        (true, _) => SamplerKind::NearestClamp,
        (false, "mirror") => SamplerKind::LinearMirror,
        (false, "repeat") => SamplerKind::LinearRepeat,
        (false, _) => SamplerKind::LinearClamp,
    };

    let bundle = build_upsample_bilinear_bundle();

    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: pass_name.as_str().to_string(),
        name: pass_name.clone(),
        geometry_buffer: geo,
        instance_buffer: None,
        normals_buffer: None,
        target_texture: upsample_out_tex.clone(),
        resolve_target: None,
        params_buffer: params_name,
        baked_data_parse_buffer: None,
        params: params_val,
        graph_binding: None,
        graph_values: None,
        shader_wgsl: bundle.module,
        texture_bindings: vec![PassTextureBinding {
            texture: src_tex.clone(),
            image_node_id: None,
        }],
        sampler_kinds: vec![sampler_kind],
        blend_state: if upsample_out_tex == *bs.target_texture_name {
            pass_blend_state
        } else {
            BlendState::REPLACE
        },
        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
        sample_count: 1,
    });
    bs.composite_passes.push(pass_name);

    // If output size differs from target and not sampled, add a fit pass.
    if !is_sampled_output && upsample_out_tex != *bs.target_texture_name {
        let fit_pass_name: ResourceName =
            format!("sys.upsample.{layer_id}.fit.pass").into();
        let fit_geo: ResourceName = format!("sys.upsample.{layer_id}.fit.geo").into();
        bs.push_fullscreen_geometry(fit_geo.clone(), tgt_w, tgt_h);

        let fit_params_name: ResourceName =
            format!("params.sys.upsample.{layer_id}.fit").into();
        let fit_params = make_params(
            [tgt_w, tgt_h],
            [tgt_w, tgt_h],
            [tgt_w * 0.5, tgt_h * 0.5],
            resolve_effective_camera_for_pass_node(scene, &nodes_by_id, layer_node, [tgt_w, tgt_h])?,
            [0.0, 0.0, 0.0, 0.0],
        );
        let fit_bundle = build_upsample_bilinear_bundle();

        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: fit_pass_name.as_str().to_string(),
            name: fit_pass_name.clone(),
            geometry_buffer: fit_geo,
            instance_buffer: None,
            normals_buffer: None,
            target_texture: bs.target_texture_name.clone(),
            resolve_target: None,
            params_buffer: fit_params_name,
            baked_data_parse_buffer: None,
            params: fit_params,
            graph_binding: None,
            graph_values: None,
            shader_wgsl: fit_bundle.module,
            texture_bindings: vec![PassTextureBinding {
                texture: upsample_out_tex.clone(),
                image_node_id: None,
            }],
            sampler_kinds: vec![SamplerKind::LinearClamp],
            blend_state: pass_blend_state,
            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
            sample_count: 1,
        });
        bs.composite_passes.push(fit_pass_name);
    }

    let upsample_output_tex = upsample_out_tex.clone();
    if is_sampled_output {
        bs.pass_output_registry.register(PassOutputSpec {
            node_id: layer_id.to_string(),
            texture_name: upsample_output_tex.clone(),
            resolution: [out_w, out_h],
            format: bs.sampled_pass_format,
        });
    }

    // Composition consumer blits.
    let composition_consumers = sc
        .composition_consumers_by_source
        .get(layer_id)
        .cloned()
        .unwrap_or_default();

    if !composition_consumers.is_empty() {
        let compose_blend_state = pass_blend_state;
        for composition_id in composition_consumers {
            let Some(comp_ctx) = sc.composition_contexts.get(&composition_id) else {
                continue;
            };
            if upsample_output_tex == comp_ctx.target_texture_name {
                continue;
            }
            if writes_scene_output_target
                && comp_ctx.target_texture_name == *bs.target_texture_name
            {
                continue;
            }

            let comp_w = comp_ctx.target_size_px[0];
            let comp_h = comp_ctx.target_size_px[1];

            let compose_geo: ResourceName =
                format!("sys.upsample.{layer_id}.to.{composition_id}.compose.geo").into();
            bs.push_fullscreen_geometry(compose_geo.clone(), comp_w, comp_h);

            let compose_pass_name: ResourceName =
                format!("sys.upsample.{layer_id}.to.{composition_id}.compose.pass").into();
            let compose_params_name: ResourceName =
                format!("params.sys.upsample.{layer_id}.to.{composition_id}.compose").into();
            let compose_params = make_params(
                [comp_w, comp_h],
                [comp_w, comp_h],
                [comp_w * 0.5, comp_h * 0.5],
                legacy_projection_camera_matrix([comp_w, comp_h]),
                [0.0, 0.0, 0.0, 0.0],
            );

            bs.render_pass_specs.push(RenderPassSpec {
                pass_id: compose_pass_name.as_str().to_string(),
                name: compose_pass_name.clone(),
                geometry_buffer: compose_geo,
                instance_buffer: None,
                normals_buffer: None,
                target_texture: comp_ctx.target_texture_name.clone(),
                resolve_target: None,
                params_buffer: compose_params_name,
                baked_data_parse_buffer: None,
                params: compose_params,
                graph_binding: None,
                graph_values: None,
                shader_wgsl: build_fullscreen_textured_bundle(
                    "return textureSample(src_tex, src_samp, in.uv);".to_string(),
                )
                .module,
                texture_bindings: vec![PassTextureBinding {
                    texture: upsample_output_tex.clone(),
                    image_node_id: None,
                }],
                sampler_kinds: vec![SamplerKind::LinearClamp],
                blend_state: compose_blend_state,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                sample_count: 1,
            });

            bs.composite_passes.push(compose_pass_name);
        }
    }

    Ok(())
}
