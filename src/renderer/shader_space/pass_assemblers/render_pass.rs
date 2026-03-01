//! RenderPass assembler.
//!
//! Handles the `"RenderPass"` node type — the most complex pass arm. Compiles
//! material WGSL, resolves geometry (including instancing / dynamic rects),
//! manages MSAA, baked-data-parse buffers, depth attachment + resolve,
//! graph bindings, and synthesises compose passes for downstream consumers.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, anyhow};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color, TextureFormat},
};

use crate::{
    dsl::{find_node, incoming_connection, Node},
    renderer::{
        camera::{legacy_projection_camera_matrix, resolve_effective_camera_for_pass_node},
        geometry_resolver::is_draw_pass_node_type,
        graph_uniforms::{choose_graph_binding_kind, pack_graph_values},
        node_compiler::geometry_nodes::{rect2d_geometry_vertices, rect2d_unit_geometry_vertices},
        scene_prep::bake_data_parse_nodes,
        types::{
            BakedDataParseMeta, BakedValue, GraphBinding, GraphBindingKind,
            MaterialCompileContext, PassOutputSpec,
        },
        utils::{as_bytes_slice, cpu_num_u32_floor},
        wgsl::{
            build_dynamic_rect_compose_bundle, build_fullscreen_textured_bundle,
            build_pass_wgsl_bundle_with_graph_binding,
        },
    },
};

use super::args::{BuilderState, SceneContext, make_fullscreen_geometry};
use super::super::pass_spec::{
    DepthResolvePass, PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl,
    IDENTITY_MAT4, build_depth_resolve_wgsl, make_params,
};
use super::super::resource_naming::{
    parse_render_pass_cull_mode, parse_render_pass_depth_test,
    readable_pass_name_for_node, sampled_render_pass_output_size,
    select_effective_msaa_sample_count,
};
use super::super::sampler::{sampler_kind_for_pass_texture, sampler_kind_from_node_params};

/// Assemble a `"RenderPass"` layer.
pub(crate) fn assemble_render_pass(
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
    let adapter = sc.adapter;

    let target_texture_name = bs.target_texture_name.clone();
    let target_format = bs.target_format;
    let sampled_pass_format = bs.sampled_pass_format;
    let tgt_w = bs.tgt_size[0];
    let tgt_h = bs.tgt_size[1];
    let tgt_w_u = bs.tgt_size_u[0];
    let tgt_h_u = bs.tgt_size_u[1];

    let pass_name = readable_pass_name_for_node(layer_node);

    let requested_msaa = cpu_num_u32_floor(
        &prepared.scene,
        nodes_by_id,
        layer_node,
        "msaaSampleCount",
        1,
    )?;
    let msaa_sample_count = select_effective_msaa_sample_count(
        layer_id,
        requested_msaa,
        sampled_pass_format,
        device.features(),
        adapter,
    )?;
    let is_sampled_output = bs.sampled_pass_ids.contains(layer_id);
    let composition_consumers = sc
        .composition_consumers_by_source
        .get(layer_id)
        .cloned()
        .unwrap_or_default();
    let has_composition_consumer = !composition_consumers.is_empty();
    let has_processing_consumer = prepared.scene.connections.iter().any(|conn| {
        conn.from.node_id == *layer_id
            && conn.from.port_id == "pass"
            && nodes_by_id
                .get(&conn.to.node_id)
                .is_some_and(|n| is_draw_pass_node_type(&n.node_type))
    });
    let has_extend_blur_consumer = prepared.scene.connections.iter().any(|conn| {
        if conn.from.node_id != *layer_id || conn.from.port_id != "pass" {
            return false;
        }
        let Some(dst_node) = nodes_by_id.get(&conn.to.node_id) else {
            return false;
        };
        if dst_node.node_type != "GuassianBlurPass" {
            return false;
        }
        dst_node
            .params
            .get("extend")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    });
    let is_downsample_source = bs.downsample_source_pass_ids.contains(layer_id);
    let is_upsample_source = bs.upsample_source_pass_ids.contains(layer_id);
    let is_blur_source = bs.gaussian_source_pass_ids.contains(layer_id)
        || bs.bloom_source_pass_ids.contains(layer_id)
        || bs.gradient_source_pass_ids.contains(layer_id);
    let pass_coord_size = sc
        .draw_coord_size_by_pass
        .get(layer_id)
        .copied()
        .unwrap_or([tgt_w, tgt_h]);
    let pass_coord_w_u = pass_coord_size[0].max(1.0).round() as u32;
    let pass_coord_h_u = pass_coord_size[1].max(1.0).round() as u32;

    let blend_state =
        crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
            .with_context(|| {
                format!(
                    "invalid blend params for {}",
                    crate::dsl::node_display_label_with_id(layer_node)
                )
            })?;
    let cull_mode = parse_render_pass_cull_mode(&layer_node.params).with_context(|| {
        format!(
            "invalid culling params for {}",
            crate::dsl::node_display_label_with_id(layer_node)
        )
    })?;
    let depth_test_enabled =
        parse_render_pass_depth_test(&layer_node.params).with_context(|| {
            format!(
                "invalid depth params for {}",
                crate::dsl::node_display_label_with_id(layer_node)
            )
        })?;

    let render_geo_node_id = incoming_connection(&prepared.scene, layer_id, "geometry")
        .map(|c| c.from.node_id.clone())
        .ok_or_else(|| anyhow!("RenderPass.geometry missing for {layer_id}"))?;
    let render_geo_is_rect =
        find_node(nodes_by_id, &render_geo_node_id)?.node_type == "Rect2DGeometry";

    let (
        geometry_buffer,
        geo_w,
        geo_h,
        geo_x,
        geo_y,
        instance_count,
        _base_m,
        _instance_mats,
        _translate_expr,
        _vertex_inline_stmts,
        _vertex_wgsl_decls,
        _vertex_graph_input_kinds,
        _vertex_uses_instance_index,
        _rect_dyn,
        normals_bytes,
    ) = crate::renderer::render_plan::resolve_geometry_for_render_pass(
        &prepared.scene,
        nodes_by_id,
        ids,
        &render_geo_node_id,
        pass_coord_size,
        Some(&MaterialCompileContext {
            baked_data_parse: Some(std::sync::Arc::new(
                prepared.baked_data_parse.clone(),
            )),
            baked_data_parse_meta: None,
            ..Default::default()
        }),
        asset_store,
    )?;

    // Determine the single-sample output target for this pass.
    let (pass_target_w_u, pass_target_h_u, pass_output_texture): (
        u32,
        u32,
        ResourceName,
    ) = if is_sampled_output {
        let out_tex: ResourceName = format!("sys.pass.{layer_id}.out").into();
        let [w_u, h_u] = sampled_render_pass_output_size(
            has_processing_consumer,
            is_downsample_source || is_upsample_source || is_blur_source,
            [pass_coord_w_u, pass_coord_h_u],
            [geo_w, geo_h],
        );
        bs.textures.push(TextureDecl {
            name: out_tex.clone(),
            size: [w_u, h_u],
            format: sampled_pass_format,
            sample_count: 1,
            needs_sampling: false,
        });
        (w_u, h_u, out_tex)
    } else {
        (tgt_w_u, tgt_h_u, target_texture_name.clone())
    };
    let pass_output_format = if is_sampled_output {
        sampled_pass_format
    } else {
        target_format
    };
    let (pass_render_target_texture, pass_resolve_target) = if msaa_sample_count > 1 {
        let msaa_tex: ResourceName = format!(
            "sys.msaa.{}.{}.color",
            pass_output_texture.as_str(),
            msaa_sample_count
        )
        .into();
        bs.textures.push(TextureDecl {
            name: msaa_tex.clone(),
            size: [pass_target_w_u, pass_target_h_u],
            format: pass_output_format,
            sample_count: msaa_sample_count,
            needs_sampling: false,
        });
        (msaa_tex, Some(pass_output_texture.clone()))
    } else {
        (pass_output_texture.clone(), None)
    };
    let depth_stencil_attachment = if depth_test_enabled {
        let depth_tex: ResourceName = format!("sys.pass.{layer_id}.depth").into();
        bs.textures.push(TextureDecl {
            name: depth_tex.clone(),
            size: [pass_target_w_u, pass_target_h_u],
            format: TextureFormat::Depth32Float,
            sample_count: msaa_sample_count,
            needs_sampling: true,
        });
        Some(depth_tex)
    } else {
        None
    };
    let pass_target_w = pass_target_w_u as f32;
    let pass_target_h = pass_target_h_u as f32;

    let mut baked = prepared.baked_data_parse.clone();
    baked.extend(bake_data_parse_nodes(
        nodes_by_id,
        layer_id,
        instance_count,
    )?);

    let mut slot_by_output: HashMap<(String, String, String), u32> = HashMap::new();
    let mut keys: Vec<(String, String, String)> = baked
        .keys()
        .filter(|(pass_id, _, _)| pass_id == layer_id)
        .cloned()
        .collect();
    keys.sort();

    for (i, k) in keys.iter().enumerate() {
        slot_by_output.insert(k.clone(), i as u32);
    }

    let meta = Arc::new(BakedDataParseMeta {
        pass_id: layer_id.to_string(),
        outputs_per_instance: keys.len() as u32,
        slot_by_output,
    });

    let mut packed: Vec<f32> = Vec::new();
    let instances = instance_count.min(1024) as usize;
    packed.resize(instances * meta.outputs_per_instance as usize * 4, 0.0);

    for (slot, (pass_id, node_id, port_id)) in keys.iter().enumerate() {
        let vs = baked
            .get(&(pass_id.clone(), node_id.clone(), port_id.clone()))
            .cloned()
            .unwrap_or_default();
        for i in 0..instances {
            let v = vs.get(i).cloned().unwrap_or(BakedValue::F32(0.0));
            let base = (i * meta.outputs_per_instance as usize + slot) * 4;
            match v {
                BakedValue::F32(x) => {
                    packed[base] = x;
                }
                BakedValue::I32(x) => {
                    packed[base] = x as f32;
                }
                BakedValue::U32(x) => {
                    packed[base] = x as f32;
                }
                BakedValue::Bool(x) => {
                    packed[base] = if x { 1.0 } else { 0.0 };
                }
                BakedValue::Vec2([x, y]) => {
                    packed[base] = x;
                    packed[base + 1] = y;
                }
                BakedValue::Vec3([x, y, z]) => {
                    packed[base] = x;
                    packed[base + 1] = y;
                    packed[base + 2] = z;
                }
                BakedValue::Vec4([x, y, z, w]) => {
                    packed[base] = x;
                    packed[base + 1] = y;
                    packed[base + 2] = z;
                    packed[base + 3] = w;
                }
            }
        }
    }

    let bytes: Arc<[u8]> = Arc::from(as_bytes_slice(&packed).to_vec());
    bs.baked_data_parse_meta_by_pass
        .insert(layer_id.to_string(), meta);
    bs.baked_data_parse_bytes_by_pass
        .insert(layer_id.to_string(), bytes.clone());

    let (
        _geometry_buffer_2,
        _geo_w_2,
        _geo_h_2,
        _geo_x_2,
        _geo_y_2,
        _instance_count_2,
        base_m_2,
        instance_mats_2,
        translate_expr,
        vertex_inline_stmts,
        vertex_wgsl_decls,
        vertex_graph_input_kinds,
        vertex_uses_instance_index,
        rect_dyn_2,
        _normals_bytes_2,
    ) = crate::renderer::render_plan::resolve_geometry_for_render_pass(
        &prepared.scene,
        nodes_by_id,
        ids,
        &render_geo_node_id,
        pass_coord_size,
        Some(&MaterialCompileContext {
            baked_data_parse: Some(std::sync::Arc::new(baked.clone())),
            baked_data_parse_meta: bs
                .baked_data_parse_meta_by_pass
                .get(layer_id)
                .cloned(),
            ..Default::default()
        }),
        asset_store,
    )?;

    // For intermediate pass outputs that will be blitted into a final Composition target,
    // render the main pass in local texture space (fullscreen in its own output), then
    // apply scene placement at compose time.
    let use_fullscreen_for_downsample_source =
        is_downsample_source && rect_dyn_2.is_some();
    let use_fullscreen_for_upsample_source = is_upsample_source && rect_dyn_2.is_some();
    let use_fullscreen_for_extend_blur_source =
        is_sampled_output && has_extend_blur_consumer;
    let use_fullscreen_for_local_blit =
        is_sampled_output && !has_processing_consumer && has_composition_consumer;
    let use_fullscreen_main_pass = use_fullscreen_for_downsample_source
        || use_fullscreen_for_upsample_source
        || use_fullscreen_for_extend_blur_source
        || use_fullscreen_for_local_blit;
    let pass_camera = resolve_effective_camera_for_pass_node(
        &prepared.scene,
        nodes_by_id,
        layer_node,
        [pass_target_w, pass_target_h],
    )?;

    let (main_pass_geometry_buffer, main_pass_params, main_pass_rect_dyn) =
        if use_fullscreen_main_pass {
            let fs_geo: ResourceName =
                format!("sys.pass.{layer_id}.fullscreen.geo").into();
            bs.geometry_buffers.push((
                fs_geo.clone(),
                make_fullscreen_geometry(pass_target_w, pass_target_h),
            ));
            (
                fs_geo,
                make_params(
                    [pass_target_w, pass_target_h],
                    [pass_target_w, pass_target_h],
                    [pass_target_w * 0.5, pass_target_h * 0.5],
                    pass_camera,
                    [0.9, 0.2, 0.2, 1.0],
                ),
                rect_dyn_2.clone(),
            )
        } else {
            let resolved_geometry_buffer: ResourceName = if render_geo_is_rect {
                let geo_name: ResourceName =
                    format!("sys.pass.{layer_id}.resolved.geo").into();
                let geo_bytes: Arc<[u8]> = if rect_dyn_2.is_some() {
                    Arc::from(as_bytes_slice(&rect2d_unit_geometry_vertices()).to_vec())
                } else {
                    Arc::from(
                        as_bytes_slice(&rect2d_geometry_vertices(
                            geo_w.max(1.0),
                            geo_h.max(1.0),
                        ))
                        .to_vec(),
                    )
                };
                bs.geometry_buffers.push((geo_name.clone(), geo_bytes));
                geo_name
            } else {
                geometry_buffer.clone()
            };
            (
                resolved_geometry_buffer,
                make_params(
                    [pass_target_w, pass_target_h],
                    [geo_w.max(1.0), geo_h.max(1.0)],
                    [geo_x, geo_y],
                    pass_camera,
                    [0.9, 0.2, 0.2, 1.0],
                ),
                rect_dyn_2.clone(),
            )
        };

    let params_name: ResourceName = format!("params.{layer_id}").into();
    let params = main_pass_params;

    let has_non_identity_base_m = base_m_2 != IDENTITY_MAT4;
    let has_instance_mats = instance_mats_2.as_ref().is_some_and(|m| !m.is_empty());
    let is_instanced =
        instance_count > 1 || has_non_identity_base_m || has_instance_mats;

    let baked_buf_name: ResourceName =
        format!("sys.pass.{layer_id}.baked_data_parse").into();

    let baked_arc = std::sync::Arc::new(baked);
    let translate_expr_wgsl = translate_expr.map(|e| e.expr);
    let vertex_inline_stmts_for_bundle = vertex_inline_stmts.clone();
    let vertex_wgsl_decls_for_bundle = vertex_wgsl_decls.clone();
    let vertex_graph_input_kinds_for_bundle = vertex_graph_input_kinds.clone();

    let has_normals = normals_bytes.is_some();
    let fullscreen_vertex_positioning =
        use_fullscreen_main_pass && rect_dyn_2.is_some();

    let mut bundle = build_pass_wgsl_bundle_with_graph_binding(
        &prepared.scene,
        nodes_by_id,
        Some(baked_arc.clone()),
        bs.baked_data_parse_meta_by_pass.get(layer_id).cloned(),
        layer_id,
        is_instanced,
        translate_expr_wgsl.clone(),
        vertex_inline_stmts_for_bundle.clone(),
        vertex_wgsl_decls_for_bundle.clone(),
        vertex_uses_instance_index,
        main_pass_rect_dyn.clone(),
        vertex_graph_input_kinds_for_bundle.clone(),
        None,
        fullscreen_vertex_positioning,
        has_normals,
    )?;

    let mut graph_binding: Option<GraphBinding> = None;
    let mut graph_values: Option<Vec<u8>> = None;
    if let Some(schema) = bundle.graph_schema.clone() {
        let limits = device.limits();
        let kind = choose_graph_binding_kind(
            schema.size_bytes,
            limits.max_uniform_buffer_binding_size as u64,
            limits.max_storage_buffer_binding_size as u64,
        )?;

        if bundle.graph_binding_kind != Some(kind) {
            bundle = build_pass_wgsl_bundle_with_graph_binding(
                &prepared.scene,
                nodes_by_id,
                Some(baked_arc.clone()),
                bs.baked_data_parse_meta_by_pass.get(layer_id).cloned(),
                layer_id,
                is_instanced,
                translate_expr_wgsl.clone(),
                vertex_inline_stmts_for_bundle.clone(),
                vertex_wgsl_decls_for_bundle.clone(),
                vertex_uses_instance_index,
                main_pass_rect_dyn.clone(),
                vertex_graph_input_kinds_for_bundle.clone(),
                Some(kind),
                fullscreen_vertex_positioning,
                has_normals,
            )?;
        }

        let schema = bundle.graph_schema.clone().ok_or_else(|| {
            anyhow!("missing graph schema after graph binding selection")
        })?;
        let graph_buffer_name: ResourceName = format!("params.{layer_id}.graph").into();
        let values = pack_graph_values(&prepared.scene, &schema)?;
        graph_values = Some(values);
        graph_binding = Some(GraphBinding {
            buffer_name: graph_buffer_name,
            kind,
            schema,
        });
    }

    let shader_wgsl = bundle.module;

    let mut texture_bindings: Vec<PassTextureBinding> = Vec::new();
    let mut sampler_kinds: Vec<SamplerKind> = Vec::new();

    // ImageTexture bindings first.
    for id in bundle.image_textures.iter() {
        let Some(tex) = ids.get(id).cloned() else {
            continue;
        };
        texture_bindings.push(PassTextureBinding {
            texture: tex,
            image_node_id: Some(id.clone()),
        });
        let kind = nodes_by_id
            .get(id)
            .map(|n| sampler_kind_from_node_params(&n.params))
            .unwrap_or(SamplerKind::LinearClamp);
        sampler_kinds.push(kind);
    }

    // PassTexture bindings next.
    let pass_bindings = crate::renderer::render_plan::resolve_pass_texture_bindings(
        &bs.pass_output_registry,
        &bundle.pass_textures,
    )?;
    for (upstream_pass_id, binding) in bundle.pass_textures.iter().zip(pass_bindings) {
        texture_bindings.push(binding);
        sampler_kinds.push(sampler_kind_for_pass_texture(
            &prepared.scene,
            upstream_pass_id,
        ));
    }

    let instance_buffer = if is_instanced {
        let b: ResourceName = format!("sys.pass.{layer_id}.instances").into();

        let mats: Vec<[f32; 16]> = if let Some(mats) = instance_mats_2 {
            mats
        } else {
            let mut mats: Vec<[f32; 16]> = Vec::with_capacity(instance_count as usize);
            for _ in 0..instance_count {
                mats.push(base_m_2);
            }
            mats
        };

        let bytes: Arc<[u8]> = Arc::from(as_bytes_slice(&mats).to_vec());

        debug_assert_eq!(bytes.len(), (instance_count as usize) * 16 * 4);

        bs.instance_buffers.push((b.clone(), bytes));

        Some(b)
    } else {
        None
    };

    let baked_data_parse_buffer: Option<ResourceName> = if keys.is_empty() {
        None
    } else {
        bs.baked_data_parse_buffer_to_pass_id
            .insert(baked_buf_name.clone(), layer_id.to_string());
        Some(baked_buf_name.clone())
    };

    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: pass_name.as_str().to_string(),
        name: pass_name.clone(),
        geometry_buffer: main_pass_geometry_buffer.clone(),
        instance_buffer,
        normals_buffer: normals_bytes.as_ref().map(|_| {
            let nb_name: ResourceName =
                format!("{}.normals", main_pass_geometry_buffer).into();
            nb_name
        }),
        target_texture: pass_render_target_texture.clone(),
        resolve_target: pass_resolve_target,
        params_buffer: params_name,
        baked_data_parse_buffer,
        params,
        graph_binding: graph_binding.clone(),
        graph_values: graph_values.clone(),
        shader_wgsl,
        texture_bindings,
        sampler_kinds,
        blend_state,
        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
        sample_count: msaa_sample_count,
    });
    bs.pass_cull_mode_by_name.insert(pass_name.clone(), cull_mode);
    if let Some(depth_attachment) = depth_stencil_attachment.clone() {
        bs.pass_depth_attachment_by_name
            .insert(pass_name.clone(), depth_attachment);
    }
    bs.composite_passes.push(pass_name);

    // Build depth-resolve pass BEFORE compose passes so that it
    // executes first and the resolved texture is ready to sample.
    let resolved_depth_tex: Option<ResourceName> = if depth_test_enabled {
        let depth_tex = depth_stencil_attachment.clone().unwrap();
        let is_multisampled_depth = msaa_sample_count > 1;

        let resolved: ResourceName =
            format!("sys.pass.{layer_id}.depth.resolved").into();
        bs.textures.push(TextureDecl {
            name: resolved.clone(),
            size: [pass_target_w_u, pass_target_h_u],
            format: sampled_pass_format,
            sample_count: 1,
            needs_sampling: false,
        });

        let depth_resolve_geo: ResourceName =
            format!("sys.pass.{layer_id}.depth.resolve.geo").into();
        let depth_resolve_params_name: ResourceName =
            format!("params.sys.pass.{layer_id}.depth.resolve").into();
        let depth_resolve_camera = legacy_projection_camera_matrix([
            pass_target_w_u as f32,
            pass_target_h_u as f32,
        ]);
        let depth_resolve_params = make_params(
            [pass_target_w_u as f32, pass_target_h_u as f32],
            [pass_target_w_u as f32, pass_target_h_u as f32],
            [pass_target_w_u as f32 * 0.5, pass_target_h_u as f32 * 0.5],
            depth_resolve_camera,
            [0.0, 0.0, 0.0, 0.0],
        );

        let depth_resolve_pass_name: ResourceName =
            format!("sys.pass.{layer_id}.depth.resolve.pass").into();
        let depth_resolve_wgsl = build_depth_resolve_wgsl(is_multisampled_depth);

        bs.depth_resolve_passes.push(DepthResolvePass {
            pass_name: depth_resolve_pass_name.clone(),
            geometry_buffer: depth_resolve_geo,
            params_buffer: depth_resolve_params_name,
            params: depth_resolve_params,
            depth_texture: depth_tex,
            dst_texture: resolved.clone(),
            shader_wgsl: depth_resolve_wgsl,
            is_multisampled: is_multisampled_depth,
        });

        bs.composite_passes.push(depth_resolve_pass_name);

        bs.pass_output_registry.register_for_port(
            PassOutputSpec {
                node_id: layer_id.to_string(),
                texture_name: resolved.clone(),
                resolution: [pass_target_w_u, pass_target_h_u],
                format: sampled_pass_format,
            },
            "depth",
        );

        Some(resolved)
    } else {
        None
    };

    // Unconditionally create compose passes for depth-port consumers.
    if let Some(ref resolved) = resolved_depth_tex {
        for composition_id in &composition_consumers {
            let is_depth_connection = prepared
                .scene
                .connections
                .iter()
                .any(|c| {
                    c.from.node_id == *layer_id
                        && c.from.port_id == "depth"
                        && c.to.node_id == *composition_id
                });
            if !is_depth_connection {
                continue;
            }
            let Some(comp_ctx) = sc.composition_contexts.get(composition_id) else {
                continue;
            };
            let comp_tgt_w = comp_ctx.target_size_px[0];
            let comp_tgt_h = comp_ctx.target_size_px[1];

            let compose_pass_name: ResourceName =
                format!("sys.pass.{layer_id}.depth.to.{composition_id}.compose.pass")
                    .into();
            let compose_params_name: ResourceName =
                format!("params.sys.pass.{layer_id}.depth.to.{composition_id}.compose")
                    .into();
            let compose_camera =
                legacy_projection_camera_matrix([comp_tgt_w, comp_tgt_h]);
            let compose_geo: ResourceName =
                format!("sys.pass.{layer_id}.depth.to.{composition_id}.compose.geo")
                    .into();
            bs.geometry_buffers.push((
                compose_geo.clone(),
                make_fullscreen_geometry(comp_tgt_w, comp_tgt_h),
            ));
            let compose_bundle = build_fullscreen_textured_bundle(
                "return textureSample(src_tex, src_samp, in.uv);".to_string(),
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
                params: make_params(
                    [comp_tgt_w, comp_tgt_h],
                    [comp_tgt_w, comp_tgt_h],
                    [comp_tgt_w * 0.5, comp_tgt_h * 0.5],
                    compose_camera,
                    [0.0, 0.0, 0.0, 0.0],
                ),
                graph_binding: None,
                graph_values: None,
                shader_wgsl: compose_bundle.module,
                texture_bindings: vec![PassTextureBinding {
                    texture: resolved.clone(),
                    image_node_id: None,
                }],
                sampler_kinds: vec![SamplerKind::NearestClamp],
                blend_state,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                sample_count: 1,
            });
            bs.composite_passes.push(compose_pass_name);
        }
    }

    // If a pass is sampled and consumed by Composition nodes, synthesize compose passes.
    if is_sampled_output && has_composition_consumer {
        for composition_id in &composition_consumers {
            let Some(comp_ctx) = sc.composition_contexts.get(composition_id) else {
                continue;
            };

            // Skip depth-port connections — already handled above.
            let is_depth_connection = prepared
                .scene
                .connections
                .iter()
                .any(|c| {
                    c.from.node_id == *layer_id
                        && c.from.port_id == "depth"
                        && c.to.node_id == *composition_id
                });
            if is_depth_connection {
                continue;
            }
            let comp_tgt_w = comp_ctx.target_size_px[0];
            let comp_tgt_h = comp_ctx.target_size_px[1];
            let comp_tgt_w_u = comp_tgt_w.max(1.0).round() as u32;
            let comp_tgt_h_u = comp_tgt_h.max(1.0).round() as u32;

            let compose_pass_name: ResourceName =
                format!("sys.pass.{layer_id}.to.{composition_id}.compose.pass").into();
            let compose_params_name: ResourceName =
                format!("params.sys.pass.{layer_id}.to.{composition_id}.compose")
                    .into();

            let (
                compose_geometry_buffer,
                compose_params_val,
                compose_bundle,
                compose_graph_binding,
                compose_graph_values,
            ) = if pass_target_w_u == comp_tgt_w_u && pass_target_h_u == comp_tgt_h_u {
                let compose_camera = legacy_projection_camera_matrix(
                    [comp_tgt_w, comp_tgt_h],
                );
                let compose_geo: ResourceName =
                    format!("sys.pass.{layer_id}.to.{composition_id}.compose.geo")
                        .into();
                bs.geometry_buffers.push((
                    compose_geo.clone(),
                    make_fullscreen_geometry(comp_tgt_w, comp_tgt_h),
                ));
                let fragment_body =
                    "return textureSample(src_tex, src_samp, in.uv);".to_string();
                (
                    compose_geo,
                    make_params(
                        [comp_tgt_w, comp_tgt_h],
                        [comp_tgt_w, comp_tgt_h],
                        [comp_tgt_w * 0.5, comp_tgt_h * 0.5],
                        compose_camera,
                        [0.0, 0.0, 0.0, 0.0],
                    ),
                    build_fullscreen_textured_bundle(fragment_body),
                    None,
                    None,
                )
            } else if use_fullscreen_main_pass && rect_dyn_2.is_some() {
                let compose_camera = legacy_projection_camera_matrix(
                    [comp_tgt_w, comp_tgt_h],
                );
                let compose_geo: ResourceName =
                    format!("sys.pass.{layer_id}.to.{composition_id}.compose.geo")
                        .into();
                bs.geometry_buffers.push((
                    compose_geo.clone(),
                    Arc::from(
                        as_bytes_slice(&rect2d_unit_geometry_vertices()).to_vec(),
                    ),
                ));

                let rect_dyn = rect_dyn_2.as_ref().expect(
                    "fullscreen-main-pass dynamic compose implies rect_dyn_2.is_some()",
                );
                let position_expr = rect_dyn
                    .position_expr
                    .as_ref()
                    .map(|e| e.expr.as_str())
                    .unwrap_or("params.center");
                let size_expr = rect_dyn
                    .size_expr
                    .as_ref()
                    .map(|e| e.expr.as_str())
                    .unwrap_or("params.geo_size");

                let graph_inputs_wgsl = if let Some(gb) = graph_binding.as_ref() {
                    let mut decl = String::new();
                    decl.push_str("\nstruct GraphInputs {\n");
                    for field in &gb.schema.fields {
                        decl.push_str(&format!(
                            "    // Node: {}\n    {}: {},\n",
                            field.node_id,
                            field.field_name,
                            field.kind.wgsl_slot_type()
                        ));
                    }
                    decl.push_str("};\n\n");
                    decl.push_str("@group(0) @binding(2)\n");
                    match gb.kind {
                        GraphBindingKind::Uniform => {
                            decl.push_str("var<uniform> graph_inputs: GraphInputs;\n")
                        }
                        GraphBindingKind::StorageRead => decl.push_str(
                            "var<storage, read> graph_inputs: GraphInputs;\n",
                        ),
                    }
                    decl
                } else {
                    String::new()
                };

                let bundle = build_dynamic_rect_compose_bundle(
                    &graph_inputs_wgsl,
                    position_expr,
                    size_expr,
                );

                (
                    compose_geo,
                    make_params(
                        [comp_tgt_w, comp_tgt_h],
                        [pass_target_w, pass_target_h],
                        [geo_x, geo_y],
                        compose_camera,
                        [0.0, 0.0, 0.0, 0.0],
                    ),
                    bundle,
                    graph_binding.clone(),
                    graph_values.clone(),
                )
            } else if use_fullscreen_for_local_blit {
                let compose_camera = legacy_projection_camera_matrix(
                    [comp_tgt_w, comp_tgt_h],
                );
                let compose_geo: ResourceName =
                    format!("sys.pass.{layer_id}.to.{composition_id}.compose.geo")
                        .into();
                bs.geometry_buffers.push((
                    compose_geo.clone(),
                    Arc::from(
                        as_bytes_slice(&rect2d_geometry_vertices(
                            geo_w.max(1.0),
                            geo_h.max(1.0),
                        ))
                        .to_vec(),
                    ),
                ));
                let fragment_body =
                    "return textureSample(src_tex, src_samp, in.uv);".to_string();
                (
                    compose_geo,
                    make_params(
                        [comp_tgt_w, comp_tgt_h],
                        [geo_w.max(1.0), geo_h.max(1.0)],
                        [geo_x, geo_y],
                        compose_camera,
                        [0.0, 0.0, 0.0, 0.0],
                    ),
                    build_fullscreen_textured_bundle(fragment_body),
                    None,
                    None,
                )
            } else {
                let compose_camera = legacy_projection_camera_matrix(
                    [comp_tgt_w, comp_tgt_h],
                );
                let fragment_body =
                    "return textureSample(src_tex, src_samp, in.uv);".to_string();
                (
                    main_pass_geometry_buffer.clone(),
                    make_params(
                        [comp_tgt_w, comp_tgt_h],
                        [geo_w.max(1.0), geo_h.max(1.0)],
                        [geo_x, geo_y],
                        compose_camera,
                        [0.0, 0.0, 0.0, 0.0],
                    ),
                    build_fullscreen_textured_bundle(fragment_body),
                    None,
                    None,
                )
            };

            bs.render_pass_specs.push(RenderPassSpec {
                pass_id: compose_pass_name.as_str().to_string(),
                name: compose_pass_name.clone(),
                geometry_buffer: compose_geometry_buffer,
                instance_buffer: None,
                normals_buffer: None,
                target_texture: comp_ctx.target_texture_name.clone(),
                resolve_target: None,
                params_buffer: compose_params_name,
                baked_data_parse_buffer: None,
                params: compose_params_val,
                graph_binding: compose_graph_binding,
                graph_values: compose_graph_values,
                shader_wgsl: compose_bundle.module,
                texture_bindings: vec![PassTextureBinding {
                    texture: pass_output_texture.clone(),
                    image_node_id: None,
                }],
                sampler_kinds: vec![SamplerKind::NearestClamp],
                blend_state,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                sample_count: 1,
            });
            bs.composite_passes.push(compose_pass_name);
        }
    }

    // Register output so downstream PassTexture nodes can resolve it.
    bs.pass_output_registry.register(PassOutputSpec {
        node_id: layer_id.to_string(),
        texture_name: pass_output_texture,
        resolution: [pass_target_w_u, pass_target_h_u],
        format: if is_sampled_output {
            sampled_pass_format
        } else {
            target_format
        },
    });

    Ok(())
}
