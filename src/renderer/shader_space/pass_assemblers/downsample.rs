//! Downsample pass assembler.
//!
//! Handles the `"Downsample"` node type. Downsamples source pass output into a
//! `targetSize`-sized texture using a convolution kernel. Optionally synthesises
//! an upsample pass to scale back to the Composite target size.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color},
};

use crate::{
    dsl::{find_node, incoming_connection, parse_str, Node},
    renderer::{
        camera::{legacy_projection_camera_matrix, resolve_effective_camera_for_pass_node},
        graph_uniforms::graph_field_name,
        types::{MaterialCompileContext, PassOutputSpec, TypedExpr, ValueType},
        utils::{coerce_to_type, cpu_num_u32_min_1},
        wgsl::{build_downsample_pass_wgsl_bundle, build_fullscreen_textured_bundle, build_upsample_bilinear_bundle},
    },
};

use super::args::{BuilderState, SceneContext};
use super::super::pass_spec::{
    PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl,
    make_params,
};

/// Assemble a `"Downsample"` layer.
pub(crate) fn assemble_downsample(
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

    let pass_name: ResourceName = format!("sys.downsample.{layer_id}.pass").into();
    let pass_blend_state =
        crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
            .with_context(|| {
                format!(
                    "invalid blend params for {}",
                    crate::dsl::node_display_label_with_id(layer_node)
                )
            })?;

    // Resolve inputs.
    let src_conn = incoming_connection(scene, layer_id, "source")
        .ok_or_else(|| anyhow!("Downsample.source missing for {layer_id}"))?;
    let src_pass_id = src_conn.from.node_id.clone();
    let src_tex = bs
        .pass_output_registry
        .get_texture(&src_pass_id)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "Downsample.source references upstream pass {src_pass_id}, but its output texture is not registered yet"
            )
        })?;

    let kernel_conn = incoming_connection(scene, layer_id, "kernel")
        .ok_or_else(|| anyhow!("Downsample.kernel missing for {layer_id}"))?;
    let kernel_node = find_node(&nodes_by_id, &kernel_conn.from.node_id)?;
    let kernel_src = kernel_node
        .params
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let kernel = crate::renderer::render_plan::parse_kernel_source_js_like(kernel_src)?;

    fn parse_json_number_f32(v: &serde_json::Value) -> Option<f32> {
        v.as_f64()
            .map(|x| x as f32)
            .or_else(|| v.as_i64().map(|x| x as f32))
            .or_else(|| v.as_u64().map(|x| x as f32))
    }

    // Resolve targetSize.
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
            (
                arr.get(0).and_then(parse_json_number_f32).unwrap_or(0.0),
                arr.get(1).and_then(parse_json_number_f32).unwrap_or(0.0),
            )
        } else if let Some(obj) = v.as_object() {
            (
                obj.get("x").and_then(parse_json_number_f32).unwrap_or(0.0),
                obj.get("y").and_then(parse_json_number_f32).unwrap_or(0.0),
            )
        } else {
            bail!(
                "Downsample.targetSize must be an object {{x,y}} or array [x,y] in params for {layer_id}"
            );
        };
        TypedExpr::new(format!("vec2f({x}, {y})"), ValueType::Vec2)
    } else {
        bail!("missing input '{layer_id}.targetSize' (no connection and no param)");
    };

    // Require CPU-known size for texture allocation.
    let (out_w, out_h) = {
        let s = target_size_expr.expr.replace([' ', '\n', '\t', '\r'], "");
        if let Some(inner) = s
            .strip_prefix("(graph_inputs.")
            .and_then(|x| x.strip_suffix(").xy"))
        {
            if let Some((_node_id, node)) = nodes_by_id.iter().find(|(_, n)| {
                n.node_type == "Vector2Input" && graph_field_name(&n.id) == inner
            }) {
                let w = cpu_num_u32_min_1(scene, &nodes_by_id, node, "x", 1)?;
                let h = cpu_num_u32_min_1(scene, &nodes_by_id, node, "y", 1)?;
                (w, h)
            } else {
                bail!(
                    "Downsample.targetSize must be a CPU-constant vec2f(w,h) for now, got {}",
                    target_size_expr.expr
                );
            }
        } else if let Some(inner) =
            s.strip_prefix("vec2f(").and_then(|x| x.strip_suffix(')'))
        {
            let parts: Vec<&str> = inner.split(',').collect();
            if parts.len() == 2 {
                let w = parts[0].parse::<f32>().unwrap_or(0.0).max(1.0).floor() as u32;
                let h = parts[1].parse::<f32>().unwrap_or(0.0).max(1.0).floor() as u32;
                (w, h)
            } else {
                bail!(
                    "Downsample.targetSize must be vec2f(w,h), got {}",
                    target_size_expr.expr
                );
            }
        } else {
            bail!(
                "Downsample.targetSize must be a CPU-constant vec2f(w,h) for now, got {}",
                target_size_expr.expr
            );
        }
    };

    let is_sampled_output = bs.sampled_pass_ids.contains(layer_id);
    let needs_upsample = !is_sampled_output && (out_w != tgt_w_u || out_h != tgt_h_u);
    let writes_scene_output_target = !is_sampled_output;
    let needs_intermediate = is_sampled_output || needs_upsample;

    let downsample_out_tex: ResourceName = if needs_intermediate {
        let tex: ResourceName = format!("sys.downsample.{layer_id}.out").into();
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

    // Fullscreen geometry for Downsample output size.
    let geo: ResourceName = format!("sys.downsample.{layer_id}.geo").into();
    bs.push_fullscreen_geometry(geo.clone(), out_w as f32, out_h as f32);

    let params_name: ResourceName = format!("params.sys.downsample.{layer_id}").into();
    let out_w_f = out_w as f32;
    let out_h_f = out_h as f32;
    let params_val = make_params(
        [out_w_f, out_h_f],
        [out_w_f, out_h_f],
        [out_w_f * 0.5, out_h_f * 0.5],
        resolve_effective_camera_for_pass_node(scene, &nodes_by_id, layer_node, [out_w_f, out_h_f])?,
        [0.0, 0.0, 0.0, 0.0],
    );

    let sampling = parse_str(&layer_node.params, "sampling").unwrap_or("Mirror");
    let sampler_kind = match sampling {
        "Mirror" => SamplerKind::LinearMirror,
        "Repeat" => SamplerKind::LinearRepeat,
        "Clamp" => SamplerKind::LinearClamp,
        "ClampToBorder" => SamplerKind::LinearClamp,
        other => bail!("Downsample.sampling unsupported: {other}"),
    };

    let bundle = build_downsample_pass_wgsl_bundle(&kernel)?;

    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: pass_name.as_str().to_string(),
        name: pass_name.clone(),
        geometry_buffer: geo.clone(),
        instance_buffer: None,
        normals_buffer: None,
        target_texture: downsample_out_tex.clone(),
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
        blend_state: if downsample_out_tex == *bs.target_texture_name {
            pass_blend_state
        } else {
            BlendState::REPLACE
        },
        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
        sample_count: 1,
    });
    bs.composite_passes.push(pass_name);

    // If Downsample is the final layer and targetSize != Composite target,
    // add an upsample bilinear pass to scale to Composite target size.
    if needs_upsample {
        let upsample_pass_name: ResourceName =
            format!("sys.downsample.{layer_id}.upsample.pass").into();
        let upsample_geo: ResourceName =
            format!("sys.downsample.{layer_id}.upsample.geo").into();
        bs.push_fullscreen_geometry(upsample_geo.clone(), tgt_w, tgt_h);

        let upsample_params_name: ResourceName =
            format!("params.sys.downsample.{layer_id}.upsample").into();
        let upsample_params_val = make_params(
            [tgt_w, tgt_h],
            [tgt_w, tgt_h],
            [tgt_w * 0.5, tgt_h * 0.5],
            resolve_effective_camera_for_pass_node(scene, &nodes_by_id, layer_node, [tgt_w, tgt_h])?,
            [0.0, 0.0, 0.0, 0.0],
        );

        let upsample_bundle = build_upsample_bilinear_bundle();

        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: upsample_pass_name.as_str().to_string(),
            name: upsample_pass_name.clone(),
            geometry_buffer: upsample_geo,
            instance_buffer: None,
            normals_buffer: None,
            target_texture: bs.target_texture_name.clone(),
            resolve_target: None,
            params_buffer: upsample_params_name,
            baked_data_parse_buffer: None,
            params: upsample_params_val,
            graph_binding: None,
            graph_values: None,
            shader_wgsl: upsample_bundle.module,
            texture_bindings: vec![PassTextureBinding {
                texture: downsample_out_tex.clone(),
                image_node_id: None,
            }],
            sampler_kinds: vec![SamplerKind::LinearClamp],
            blend_state: pass_blend_state,
            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
            sample_count: 1,
        });
        bs.composite_passes.push(upsample_pass_name);
    }

    // Register Downsample output for chaining.
    let downsample_output_tex = downsample_out_tex.clone();
    if is_sampled_output {
        bs.pass_output_registry.register(PassOutputSpec {
            node_id: layer_id.to_string(),
            texture_name: downsample_output_tex.clone(),
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
            if downsample_output_tex == comp_ctx.target_texture_name {
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
                format!("sys.downsample.{layer_id}.to.{composition_id}.compose.geo").into();
            bs.push_fullscreen_geometry(compose_geo.clone(), comp_w, comp_h);

            let compose_pass_name: ResourceName =
                format!("sys.downsample.{layer_id}.to.{composition_id}.compose.pass").into();
            let compose_params_name: ResourceName =
                format!("params.sys.downsample.{layer_id}.to.{composition_id}.compose").into();
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
                    texture: downsample_output_tex.clone(),
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
