//! Gradient blur pass assembler.
//!
//! Handles the `"GradientBlur"` node type. Pads the source, builds a MIP chain,
//! then composites with a per-pixel blur radius (mask-driven) by sampling across
//! MIP levels.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color},
};

use crate::{
    dsl::{incoming_connection, Node},
    renderer::{
        camera::pass_node_uses_custom_camera,
        graph_uniforms::{choose_graph_binding_kind, pack_graph_values},
        types::{GraphBinding, PassOutputSpec},
        wgsl::{build_fullscreen_textured_bundle, clamp_min_1},
        wgsl_gradient_blur::*,
    },
};

use super::args::{BuilderState, SceneContext, make_fullscreen_geometry};
use super::super::image_utils::image_node_dimensions;
use super::super::pass_spec::{
    PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl, make_params,
};
use super::super::resource_naming::{
    resolve_chain_camera_for_first_pass, resolve_pass_texture_bindings,
};
use super::super::sampler::{sampler_kind_for_pass_texture, sampler_kind_from_node_params};

/// Assemble a `"GradientBlur"` layer.
pub(crate) fn assemble_gradient_blur(
    sc: &SceneContext<'_>,
    bs: &mut BuilderState<'_>,
    layer_id: &str,
    layer_node: &Node,
) -> Result<()> {
    let prepared = sc.prepared;
    let nodes_by_id = sc.nodes_by_id();
    let ids = sc.ids();
    let asset_store = sc.asset_store;
    let device = sc.device;

    let target_texture_name = bs.target_texture_name.clone();
    let target_format = bs.target_format;
    let sampled_pass_format = bs.sampled_pass_format;
    let tgt_w = bs.tgt_size[0];
    let tgt_h = bs.tgt_size[1];
    let tgt_w_u = bs.tgt_size_u[0];
    let tgt_h_u = bs.tgt_size_u[1];

    // ---------- resolve source dimensions ----------
    let mut gb_src_resolution: [u32; 2] = [tgt_w_u, tgt_h_u];
    let mut gb_output_center: Option<[f32; 2]> = None;

    if let Some(src_conn) = incoming_connection(&prepared.scene, layer_id, "source") {
        if let Some(src_node) = nodes_by_id.get(&src_conn.from.node_id) {
            if src_node.node_type == "RenderPass" {
                if let Some(geo_conn) = incoming_connection(
                    &prepared.scene,
                    &src_conn.from.node_id,
                    "geometry",
                ) {
                    if let Ok((
                        _,
                        src_geo_w,
                        src_geo_h,
                        src_geo_x,
                        src_geo_y,
                        _,
                        _,
                        _,
                        _,
                        _,
                        _,
                        _,
                        _,
                        _,
                        _,
                    )) =
                        crate::renderer::render_plan::resolve_geometry_for_render_pass(
                            &prepared.scene,
                            nodes_by_id,
                            ids,
                            &geo_conn.from.node_id,
                            [tgt_w, tgt_h],
                            None,
                            asset_store,
                        )
                    {
                        gb_src_resolution = [
                            src_geo_w.max(1.0).round() as u32,
                            src_geo_h.max(1.0).round() as u32,
                        ];
                        gb_output_center = Some([src_geo_x, src_geo_y]);
                    }
                }
            }
        }

        // (A) Upstream pass output.
        if let Some(src_spec) = bs
            .pass_output_registry
            .get_for_port(&src_conn.from.node_id, &src_conn.from.port_id)
        {
            gb_src_resolution = src_spec.resolution;
        }
        // (B) Direct ImageTexture.
        if let Some(src_node) = nodes_by_id.get(&src_conn.from.node_id) {
            if src_node.node_type == "ImageTexture" {
                if let Some(dims) = image_node_dimensions(src_node, asset_store) {
                    gb_src_resolution = dims;
                }
            }
        }
    }

    let [padded_w, padded_h] =
        gradient_blur_padded_size(gb_src_resolution[0], gb_src_resolution[1]);
    let src_w = gb_src_resolution[0] as f32;
    let src_h = gb_src_resolution[1] as f32;
    let pad_w = padded_w as f32;
    let pad_h = padded_h as f32;
    let pad_offset_x = (pad_w - src_w) * 0.5;
    let pad_offset_y = (pad_h - src_h) * 0.5;

    let is_sampled_output = bs.sampled_pass_ids.contains(layer_id);
    let mut gradient_chain_first_camera_consumed = false;

    // ---------- source pass ----------
    let mut initial_source_texture: Option<ResourceName> = None;
    let mut initial_source_image_node_id: Option<String> = None;

    if let Some(src_conn) = incoming_connection(&prepared.scene, layer_id, "source") {
        // (A) upstream pass output bypass
        if let Some(spec) = bs
            .pass_output_registry
            .get_for_port(&src_conn.from.node_id, &src_conn.from.port_id)
        {
            if spec.format == sampled_pass_format {
                initial_source_texture = Some(spec.texture_name.clone());
            }
        }
        // (B) direct ImageTexture bypass
        if initial_source_texture.is_none() {
            if let Some(src_node) = nodes_by_id.get(&src_conn.from.node_id) {
                if src_node.node_type == "ImageTexture"
                    && src_conn.from.port_id == "color"
                    && incoming_connection(
                        &prepared.scene,
                        &src_conn.from.node_id,
                        "uv",
                    )
                    .is_none()
                {
                    if let Some(tex) = ids.get(&src_conn.from.node_id).cloned() {
                        initial_source_texture = Some(tex);
                        initial_source_image_node_id =
                            Some(src_conn.from.node_id.clone());
                    }
                }
            }
        }
    }

    // Keep camera semantics stable across bypass/elision.
    let force_source_pass_for_custom_camera = pass_node_uses_custom_camera(
        &prepared.scene,
        nodes_by_id,
        layer_node,
        [src_w, src_h],
    )?;
    if force_source_pass_for_custom_camera {
        initial_source_texture = None;
        initial_source_image_node_id = None;
    }

    let source_texture: ResourceName = if let Some(existing_tex) =
        initial_source_texture
    {
        existing_tex
    } else {
        // Create intermediate source texture.
        let src_tex: ResourceName = format!("sys.gb.{layer_id}.src").into();
        bs.textures.push(TextureDecl {
            name: src_tex.clone(),
            size: gb_src_resolution,
            format: sampled_pass_format,
            sample_count: 1,
            needs_sampling: false,
        });

        let geo_src: ResourceName = format!("sys.gb.{layer_id}.src.geo").into();
        bs.geometry_buffers
            .push((geo_src.clone(), make_fullscreen_geometry(src_w, src_h)));

        let params_src: ResourceName = format!("params.sys.gb.{layer_id}.src").into();
        let params_src_val = make_params(
            [src_w, src_h],
            [src_w, src_h],
            [src_w * 0.5, src_h * 0.5],
            resolve_chain_camera_for_first_pass(
                &mut gradient_chain_first_camera_consumed,
                &prepared.scene,
                nodes_by_id,
                layer_node,
                [src_w, src_h],
            )?,
            [0.0, 0.0, 0.0, 0.0],
        );

        let mut src_bundle = build_gradient_blur_source_wgsl_bundle(
            &prepared.scene,
            nodes_by_id,
            layer_id,
        )?;
        let mut src_graph_binding: Option<GraphBinding> = None;
        let mut src_graph_values: Option<Vec<u8>> = None;
        if let Some(schema) = src_bundle.graph_schema.clone() {
            let limits = device.limits();
            let kind = choose_graph_binding_kind(
                schema.size_bytes,
                limits.max_uniform_buffer_binding_size as u64,
                limits.max_storage_buffer_binding_size as u64,
            )?;
            if src_bundle.graph_binding_kind != Some(kind) {
                src_bundle = build_gradient_blur_source_wgsl_bundle_with_graph_binding(
                    &prepared.scene,
                    nodes_by_id,
                    layer_id,
                    Some(kind),
                )?;
            }
            let schema = src_bundle
                .graph_schema
                .clone()
                .ok_or_else(|| anyhow!("missing gb source graph schema"))?;
            let values = pack_graph_values(&prepared.scene, &schema)?;
            src_graph_values = Some(values);
            src_graph_binding = Some(GraphBinding {
                buffer_name: format!("params.sys.gb.{layer_id}.src.graph").into(),
                kind,
                schema,
            });
        }

        let mut src_texture_bindings: Vec<PassTextureBinding> = Vec::new();
        let mut src_sampler_kinds: Vec<SamplerKind> = Vec::new();

        for id in src_bundle.image_textures.iter() {
            let Some(tex) = ids.get(id).cloned() else {
                continue;
            };
            src_texture_bindings.push(PassTextureBinding {
                texture: tex,
                image_node_id: Some(id.clone()),
            });
            let kind = nodes_by_id
                .get(id)
                .map(|n| sampler_kind_from_node_params(&n.params))
                .unwrap_or(SamplerKind::LinearClamp);
            src_sampler_kinds.push(kind);
        }

        let src_pass_bindings = resolve_pass_texture_bindings(
            &bs.pass_output_registry,
            &src_bundle.pass_textures,
        )?;
        for (upstream_pass_id, binding) in
            src_bundle.pass_textures.iter().zip(src_pass_bindings)
        {
            src_texture_bindings.push(binding);
            src_sampler_kinds.push(sampler_kind_for_pass_texture(
                &prepared.scene,
                upstream_pass_id,
            ));
        }

        let src_pass_name: ResourceName = format!("sys.gb.{layer_id}.src.pass").into();
        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: src_pass_name.as_str().to_string(),
            name: src_pass_name.clone(),
            geometry_buffer: geo_src,
            instance_buffer: None,
            normals_buffer: None,
            target_texture: src_tex.clone(),
            resolve_target: None,
            params_buffer: params_src.clone(),
            baked_data_parse_buffer: None,
            params: params_src_val,
            graph_binding: src_graph_binding,
            graph_values: src_graph_values,
            shader_wgsl: src_bundle.module,
            texture_bindings: src_texture_bindings,
            sampler_kinds: src_sampler_kinds,
            blend_state: BlendState::REPLACE,
            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
            sample_count: 1,
        });
        bs.composite_passes.push(src_pass_name);
        src_tex
    };

    // ---------- pad pass ----------
    let pad_tex: ResourceName = format!("sys.gb.{layer_id}.pad").into();
    bs.textures.push(TextureDecl {
        name: pad_tex.clone(),
        size: [padded_w, padded_h],
        format: sampled_pass_format,
        sample_count: 1,
        needs_sampling: false,
    });

    let pad_geo: ResourceName = format!("sys.gb.{layer_id}.pad.geo").into();
    bs.geometry_buffers.push((pad_geo.clone(), make_fullscreen_geometry(pad_w, pad_h)));

    let params_pad: ResourceName = format!("params.sys.gb.{layer_id}.pad").into();
    let params_pad_val = make_params(
        [pad_w, pad_h],
        [pad_w, pad_h],
        [pad_w * 0.5, pad_h * 0.5],
        resolve_chain_camera_for_first_pass(
            &mut gradient_chain_first_camera_consumed,
            &prepared.scene,
            nodes_by_id,
            layer_node,
            [pad_w, pad_h],
        )?,
        [0.0, 0.0, 0.0, 0.0],
    );

    let pad_bundle = build_gradient_blur_pad_wgsl_bundle(src_w, src_h, pad_w, pad_h);

    let pad_pass_name: ResourceName = format!("sys.gb.{layer_id}.pad.pass").into();
    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: pad_pass_name.as_str().to_string(),
        name: pad_pass_name.clone(),
        geometry_buffer: pad_geo,
        instance_buffer: None,
        normals_buffer: None,
        target_texture: pad_tex.clone(),
        resolve_target: None,
        params_buffer: params_pad,
        baked_data_parse_buffer: None,
        params: params_pad_val,
        graph_binding: None,
        graph_values: None,
        shader_wgsl: pad_bundle.module,
        texture_bindings: vec![PassTextureBinding {
            texture: source_texture.clone(),
            image_node_id: initial_source_image_node_id.clone(),
        }],
        sampler_kinds: vec![SamplerKind::LinearMirror],
        blend_state: BlendState::REPLACE,
        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
        sample_count: 1,
    });
    bs.composite_passes.push(pad_pass_name);

    // ---------- mip chain ----------
    let mip_pass_ids: Vec<String> = (0..GB_MIP_LEVELS)
        .map(|i| {
            if i == 0 {
                format!("sys.gb.{layer_id}.pad")
            } else {
                format!("sys.gb.{layer_id}.mip{i}")
            }
        })
        .collect();

    let mut prev_mip_tex = pad_tex.clone();
    let mut cur_mip_w = padded_w;
    let mut cur_mip_h = padded_h;

    for i in 1..GB_MIP_LEVELS {
        cur_mip_w = clamp_min_1(cur_mip_w / 2);
        cur_mip_h = clamp_min_1(cur_mip_h / 2);
        let mip_tex: ResourceName = format!("sys.gb.{layer_id}.mip{i}").into();
        bs.textures.push(TextureDecl {
            name: mip_tex.clone(),
            size: [cur_mip_w, cur_mip_h],
            format: sampled_pass_format,
            sample_count: 1,
            needs_sampling: false,
        });

        let mip_geo: ResourceName = format!("sys.gb.{layer_id}.mip{i}.geo").into();
        bs.geometry_buffers.push((
            mip_geo.clone(),
            make_fullscreen_geometry(cur_mip_w as f32, cur_mip_h as f32),
        ));

        let params_mip: ResourceName =
            format!("params.sys.gb.{layer_id}.mip{i}").into();
        let cur_mip_w_f = cur_mip_w as f32;
        let cur_mip_h_f = cur_mip_h as f32;
        let params_mip_val = make_params(
            [cur_mip_w_f, cur_mip_h_f],
            [cur_mip_w_f, cur_mip_h_f],
            [cur_mip_w_f * 0.5, cur_mip_h_f * 0.5],
            resolve_chain_camera_for_first_pass(
                &mut gradient_chain_first_camera_consumed,
                &prepared.scene,
                nodes_by_id,
                layer_node,
                [cur_mip_w_f, cur_mip_h_f],
            )?,
            [0.0, 0.0, 0.0, 0.0],
        );

        let ds_bundle = crate::renderer::wgsl::build_downsample_pass_wgsl_bundle(
            &gradient_blur_cross_kernel(),
        )?;

        let mip_pass_name: ResourceName =
            format!("sys.gb.{layer_id}.mip{i}.pass").into();
        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: mip_pass_name.as_str().to_string(),
            name: mip_pass_name.clone(),
            geometry_buffer: mip_geo,
            instance_buffer: None,
            normals_buffer: None,
            target_texture: mip_tex.clone(),
            resolve_target: None,
            params_buffer: params_mip,
            baked_data_parse_buffer: None,
            params: params_mip_val,
            graph_binding: None,
            graph_values: None,
            shader_wgsl: ds_bundle.module,
            texture_bindings: vec![PassTextureBinding {
                texture: prev_mip_tex.clone(),
                image_node_id: None,
            }],
            sampler_kinds: vec![SamplerKind::LinearMirror],
            blend_state: BlendState::REPLACE,
            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
            sample_count: 1,
        });
        bs.composite_passes.push(mip_pass_name);
        prev_mip_tex = mip_tex;
    }

    // ---------- register mip pass outputs ----------
    for (i, mip_id) in mip_pass_ids.iter().enumerate() {
        let mip_w = clamp_min_1(padded_w >> i);
        let mip_h = clamp_min_1(padded_h >> i);
        let tex_name: ResourceName = mip_id.clone().into();
        bs.pass_output_registry.register(PassOutputSpec {
            node_id: mip_id.clone(),
            texture_name: tex_name,
            resolution: [mip_w, mip_h],
            format: sampled_pass_format,
        });
    }

    // ---------- composite/final pass ----------
    let output_tex: ResourceName = if is_sampled_output {
        let out: ResourceName = format!("sys.gb.{layer_id}.out").into();
        bs.textures.push(TextureDecl {
            name: out.clone(),
            size: gb_src_resolution,
            format: sampled_pass_format,
            sample_count: 1,
            needs_sampling: false,
        });
        out
    } else {
        target_texture_name.clone()
    };

    let final_geo: ResourceName = format!("sys.gb.{layer_id}.final.geo").into();
    bs.geometry_buffers.push((final_geo.clone(), make_fullscreen_geometry(src_w, src_h)));

    let params_final: ResourceName = format!("params.sys.gb.{layer_id}.final").into();
    let final_target_size = if output_tex == target_texture_name {
        [tgt_w, tgt_h]
    } else {
        [src_w, src_h]
    };
    let final_center = if output_tex == target_texture_name {
        gb_output_center.unwrap_or([src_w * 0.5, src_h * 0.5])
    } else {
        [src_w * 0.5, src_h * 0.5]
    };
    let params_final_val = make_params(
        final_target_size,
        [src_w, src_h],
        final_center,
        resolve_chain_camera_for_first_pass(
            &mut gradient_chain_first_camera_consumed,
            &prepared.scene,
            nodes_by_id,
            layer_node,
            final_target_size,
        )?,
        [0.0, 0.0, 0.0, 0.0],
    );

    let mut composite_bundle = build_gradient_blur_composite_wgsl_bundle(
        &prepared.scene,
        nodes_by_id,
        layer_id,
        &mip_pass_ids,
        [pad_w, pad_h],
        [pad_offset_x, pad_offset_y],
    )?;

    let mut final_graph_binding: Option<GraphBinding> = None;
    let mut final_graph_values: Option<Vec<u8>> = None;
    if let Some(schema) = composite_bundle.graph_schema.clone() {
        let limits = device.limits();
        let kind = choose_graph_binding_kind(
            schema.size_bytes,
            limits.max_uniform_buffer_binding_size as u64,
            limits.max_storage_buffer_binding_size as u64,
        )?;
        if composite_bundle.graph_binding_kind != Some(kind) {
            composite_bundle =
                build_gradient_blur_composite_wgsl_bundle_with_graph_binding(
                    &prepared.scene,
                    nodes_by_id,
                    layer_id,
                    &mip_pass_ids,
                    [pad_w, pad_h],
                    [pad_offset_x, pad_offset_y],
                    Some(kind),
                )?;
        }
        let schema = composite_bundle
            .graph_schema
            .clone()
            .ok_or_else(|| anyhow!("missing gb composite graph schema"))?;
        let values = pack_graph_values(&prepared.scene, &schema)?;
        final_graph_values = Some(values);
        final_graph_binding = Some(GraphBinding {
            buffer_name: format!("params.sys.gb.{layer_id}.final.graph").into(),
            kind,
            schema,
        });
    }

    // Build texture bindings for the composite pass.
    let mut final_texture_bindings: Vec<PassTextureBinding> = Vec::new();
    let mut final_sampler_kinds: Vec<SamplerKind> = Vec::new();

    // Image textures from mask expression.
    for id in composite_bundle.image_textures.iter() {
        let Some(tex) = ids.get(id).cloned() else {
            continue;
        };
        final_texture_bindings.push(PassTextureBinding {
            texture: tex,
            image_node_id: Some(id.clone()),
        });
        let kind = nodes_by_id
            .get(id)
            .map(|n| sampler_kind_from_node_params(&n.params))
            .unwrap_or(SamplerKind::LinearClamp);
        final_sampler_kinds.push(kind);
    }

    // Pass textures (mip textures + any from mask expression).
    let final_pass_bindings = resolve_pass_texture_bindings(
        &bs.pass_output_registry,
        &composite_bundle.pass_textures,
    )?;
    for (upstream_pass_id, binding) in composite_bundle
        .pass_textures
        .iter()
        .zip(final_pass_bindings)
    {
        final_texture_bindings.push(binding);
        if upstream_pass_id.contains("sys.gb.") {
            final_sampler_kinds.push(SamplerKind::LinearClamp);
        } else {
            final_sampler_kinds.push(sampler_kind_for_pass_texture(
                &prepared.scene,
                upstream_pass_id,
            ));
        }
    }

    let pass_blend_state =
        crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
            .with_context(|| {
                format!(
                    "invalid blend params for {}",
                    crate::dsl::node_display_label_with_id(layer_node)
                )
            })?;
    let final_blend_state: BlendState = if output_tex == target_texture_name {
        pass_blend_state
    } else {
        BlendState::REPLACE
    };

    let final_pass_name: ResourceName = format!("sys.gb.{layer_id}.final.pass").into();
    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: final_pass_name.as_str().to_string(),
        name: final_pass_name.clone(),
        geometry_buffer: final_geo,
        instance_buffer: None,
        normals_buffer: None,
        target_texture: output_tex.clone(),
        resolve_target: None,
        params_buffer: params_final,
        baked_data_parse_buffer: None,
        params: params_final_val,
        graph_binding: final_graph_binding,
        graph_values: final_graph_values,
        shader_wgsl: composite_bundle.module,
        texture_bindings: final_texture_bindings,
        sampler_kinds: final_sampler_kinds,
        blend_state: final_blend_state,
        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
        sample_count: 1,
    });
    bs.composite_passes.push(final_pass_name);

    // Register GradientBlur output for downstream chaining.
    let gradient_output_tex = output_tex.clone();
    bs.pass_output_registry.register(PassOutputSpec {
        node_id: layer_id.to_string(),
        texture_name: gradient_output_tex.clone(),
        resolution: gb_src_resolution,
        format: if is_sampled_output {
            sampled_pass_format
        } else {
            target_format
        },
    });

    let composition_consumers = sc
        .composition_consumers_by_source
        .get(layer_id)
        .cloned()
        .unwrap_or_default();
    for composition_id in composition_consumers {
        let Some(comp_ctx) = sc.composition_contexts.get(&composition_id) else {
            continue;
        };
        if gradient_output_tex == comp_ctx.target_texture_name {
            continue;
        }

        let comp_w = comp_ctx.target_size_px[0];
        let comp_h = comp_ctx.target_size_px[1];
        let compose_geo: ResourceName =
            format!("sys.gb.{layer_id}.to.{composition_id}.compose.geo").into();
        bs.geometry_buffers
            .push((compose_geo.clone(), make_fullscreen_geometry(src_w, src_h)));
        let compose_pass_name: ResourceName =
            format!("sys.gb.{layer_id}.to.{composition_id}.compose.pass").into();
        let compose_params_name: ResourceName =
            format!("params.sys.gb.{layer_id}.to.{composition_id}.compose").into();
        let compose_params = make_params(
            [comp_w, comp_h],
            [src_w, src_h],
            gb_output_center.unwrap_or([comp_w * 0.5, comp_h * 0.5]),
            resolve_chain_camera_for_first_pass(
                &mut gradient_chain_first_camera_consumed,
                &prepared.scene,
                nodes_by_id,
                layer_node,
                [comp_w, comp_h],
            )?,
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
                texture: gradient_output_tex.clone(),
                image_node_id: None,
            }],
            sampler_kinds: vec![SamplerKind::LinearClamp],
            blend_state: pass_blend_state,
            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
            sample_count: 1,
        });
        bs.composite_passes.push(compose_pass_name);
    }

    Ok(())
}
