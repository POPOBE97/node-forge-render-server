use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use anyhow::{Result, anyhow, bail};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color, TextureFormat, TextureUsages},
};

use crate::{
    asset_store::AssetStore,
    dsl::{SceneDSL, find_node, incoming_connection, parse_texture_format},
    renderer::{
        ShaderSpacePresentationMode,
        camera::legacy_projection_camera_matrix,
        geometry_resolver::{is_pass_like_node_type, resolve_scene_draw_contexts},
        graph_uniforms::{compute_pipeline_signature_for_pass_bindings, hash_bytes},
        node_compiler::geometry_nodes::{rect2d_geometry_vertices, rect2d_unit_geometry_vertices},
        scene_prep::{PreparedScene, ScenePrepReport, prepare_scene_with_report},
        shader_space::{
            image_utils::{ensure_rgba8, load_image_from_data_url_checked, load_image_from_path},
            sampler::build_image_premultiply_wgsl,
        },
        types::{MaterialCompileContext, PassBindings, PassOutputRegistry},
        utils::{as_bytes_slice, cpu_num_u32_min_1},
    },
};

use super::{
    compute_pass_render_order, forward_root_dependencies_from_roots,
    load_gltf_geometry_pixel_space,
    pass_assemblers::args::{BuilderState, SceneContext, make_fullscreen_geometry},
    pass_handlers::PassPlannerRegistry,
    pass_spec::{PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl, make_params},
    resolve_geometry_for_render_pass,
    resource_naming::{
        UI_PRESENT_HDR_GAMMA_SUFFIX, UI_PRESENT_SDR_SRGB_SUFFIX, build_hdr_gamma_encode_wgsl,
        build_srgb_display_encode_wgsl,
    },
    sampled_pass_node_ids_from_roots,
    types::{
        ImagePrepass, ImageTextureSpec, PlanBuildOptions, PlanningDevice, RenderPlan, ResourcePlans,
    },
};

pub(crate) struct RenderPlanner {
    options: PlanBuildOptions,
}

impl RenderPlanner {
    pub(crate) fn new(options: PlanBuildOptions) -> Self {
        Self { options }
    }

    pub(crate) fn plan(
        &self,
        scene: &SceneDSL,
        asset_store: Option<&AssetStore>,
        adapter: Option<&wgpu::Adapter>,
    ) -> Result<RenderPlan> {
        let (prepared, scene_report) = prepare_scene_with_report(scene)?;
        self.plan_prepared(prepared, scene_report, asset_store, adapter)
    }

    fn plan_prepared(
        &self,
        prepared: PreparedScene,
        scene_report: ScenePrepReport,
        asset_store: Option<&AssetStore>,
        adapter: Option<&wgpu::Adapter>,
    ) -> Result<RenderPlan> {
        let resolution = prepared.resolution;
        let nodes_by_id = &prepared.nodes_by_id;
        let ids = &prepared.ids;
        let output_texture_node_id = &prepared.output_texture_node_id;
        let output_texture_name = prepared.output_texture_name.clone();
        let composite_layers_in_order = &prepared.composite_layers_in_draw_order;
        let order = &prepared.topo_order;
        let presentation_mode = self.options.presentation_mode;
        let enable_display_encode = matches!(
            presentation_mode,
            ShaderSpacePresentationMode::UiSdrDisplayEncode
                | ShaderSpacePresentationMode::UiHdrNative
        );

        let resolved_contexts = resolve_scene_draw_contexts(
            &prepared.scene,
            nodes_by_id,
            ids,
            resolution,
            asset_store,
        )?;
        let composition_contexts = resolved_contexts.composition_contexts.clone();
        let composition_consumers_by_source = resolved_contexts.composition_consumers_by_source;
        let mut draw_coord_size_by_pass: HashMap<String, [f32; 2]> = HashMap::new();
        for draw_ctx in &resolved_contexts.draw_contexts {
            draw_coord_size_by_pass
                .insert(draw_ctx.pass_node_id.clone(), draw_ctx.coord_domain.size_px);
        }

        let target_texture_id = output_texture_node_id.clone();
        let target_node = find_node(nodes_by_id, &target_texture_id)?;
        if target_node.node_type != "RenderTexture" {
            bail!(
                "Composite.target must come from RenderTexture, got {}",
                target_node.node_type
            );
        }

        let tgt_w_u = cpu_num_u32_min_1(
            &prepared.scene,
            nodes_by_id,
            target_node,
            "width",
            resolution[0],
        )?;
        let tgt_h_u = cpu_num_u32_min_1(
            &prepared.scene,
            nodes_by_id,
            target_node,
            "height",
            resolution[1],
        )?;
        let tgt_w = tgt_w_u as f32;
        let tgt_h = tgt_h_u as f32;
        let target_texture_name = ids
            .get(&target_texture_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing name for node: {}", target_texture_id))?;
        let target_format = parse_texture_format(&target_node.params)?;
        let sampled_pass_format = match target_format {
            TextureFormat::Rgba8UnormSrgb => TextureFormat::Rgba8Unorm,
            TextureFormat::Bgra8UnormSrgb => TextureFormat::Bgra8Unorm,
            other => other,
        };

        let planning_device = PlanningDevice::new(
            self.options.gpu_caps.features,
            self.options.gpu_caps.limits.clone(),
        );

        let mut sampled_pass_ids = sampled_pass_node_ids_from_roots(
            &prepared.scene,
            nodes_by_id,
            composite_layers_in_order,
        )?;
        let (
            mut downsample_source_pass_ids,
            mut upsample_source_pass_ids,
            mut gaussian_source_pass_ids,
            mut bloom_source_pass_ids,
            mut gradient_source_pass_ids,
        ) = collect_processing_source_pass_ids(&prepared);

        let mut geometry_buffers: Vec<(ResourceName, Arc<[u8]>)> = Vec::new();
        let mut instance_buffers: Vec<(ResourceName, Arc<[u8]>)> = Vec::new();
        let mut textures: Vec<TextureDecl> = Vec::new();
        let mut image_textures: Vec<ImageTextureSpec> = Vec::new();
        let mut render_pass_specs: Vec<RenderPassSpec> = Vec::new();
        let mut composite_passes: Vec<ResourceName> = Vec::new();
        let mut depth_resolve_passes = Vec::new();
        let mut image_prepasses: Vec<ImagePrepass> = Vec::new();
        let mut prepass_texture_samples: Vec<(String, ResourceName)> = Vec::new();
        let mut pass_cull_mode_by_name: HashMap<ResourceName, Option<wgpu::Face>> = HashMap::new();
        let mut pass_depth_attachment_by_name: HashMap<ResourceName, ResourceName> = HashMap::new();
        let mut baked_data_parse_meta_by_pass = HashMap::new();
        let mut baked_data_parse_bytes_by_pass = HashMap::new();
        let mut baked_data_parse_buffer_to_pass_id = HashMap::new();
        let mut pass_output_registry: PassOutputRegistry = Default::default();

        for id in order {
            let Some(node) = nodes_by_id.get(id) else {
                continue;
            };
            let name = ids
                .get(id)
                .cloned()
                .ok_or_else(|| anyhow!("missing name for node: {id}"))?;

            match node.node_type.as_str() {
                "Rect2DGeometry" => {
                    let (
                        _geo_buf,
                        geo_w,
                        geo_h,
                        _geo_x,
                        _geo_y,
                        _instances,
                        _base_m,
                        _instance_mats,
                        _translate_expr,
                        _vtx_inline_stmts,
                        _vtx_wgsl_decls,
                        _vtx_graph_input_kinds,
                        _uses_instance_index,
                        rect_dyn,
                        _normals_bytes,
                    ) = resolve_geometry_for_render_pass(
                        &prepared.scene,
                        nodes_by_id,
                        ids,
                        &node.id,
                        [tgt_w, tgt_h],
                        None,
                        asset_store,
                    )?;
                    let verts = if rect_dyn.is_some() {
                        rect2d_unit_geometry_vertices()
                    } else {
                        rect2d_geometry_vertices(geo_w, geo_h)
                    };
                    let bytes: Arc<[u8]> = Arc::from(as_bytes_slice(&verts).to_vec());
                    geometry_buffers.push((name, bytes));
                }
                "GLTFGeometry" => {
                    let store =
                        asset_store.ok_or_else(|| anyhow!("GLTFGeometry: no asset store"))?;
                    let loaded = load_gltf_geometry_pixel_space(
                        &prepared.scene,
                        &node.id,
                        node,
                        [tgt_w, tgt_h],
                        store,
                    )?;
                    let vert_bytes: Arc<[u8]> =
                        Arc::from(bytemuck::cast_slice::<[f32; 5], u8>(&loaded.vertices).to_vec());
                    geometry_buffers.push((name.clone(), vert_bytes));
                    if let Some(normals_bytes) = loaded.normals_bytes {
                        let normals_name: ResourceName = format!("{name}.normals").into();
                        geometry_buffers.push((normals_name, normals_bytes));
                    }
                }
                "RenderTexture" => {
                    let w = cpu_num_u32_min_1(
                        &prepared.scene,
                        nodes_by_id,
                        node,
                        "width",
                        resolution[0],
                    )?;
                    let h = cpu_num_u32_min_1(
                        &prepared.scene,
                        nodes_by_id,
                        node,
                        "height",
                        resolution[1],
                    )?;
                    let format = parse_texture_format(&node.params)?;
                    textures.push(TextureDecl {
                        name,
                        size: [w, h],
                        format,
                        sample_count: 1,
                        needs_sampling: false,
                    });
                }
                _ => {}
            }
        }

        let is_hdr_native = presentation_mode == ShaderSpacePresentationMode::UiHdrNative;
        let hdr_gamma_texture = if enable_display_encode
            && is_hdr_native
            && target_format == TextureFormat::Rgba16Float
        {
            let name: ResourceName = format!(
                "{}{}",
                target_texture_name.as_str(),
                UI_PRESENT_HDR_GAMMA_SUFFIX
            )
            .into();
            textures.push(TextureDecl {
                name: name.clone(),
                size: [tgt_w_u, tgt_h_u],
                format: TextureFormat::Rgba16Float,
                sample_count: 1,
                needs_sampling: false,
            });
            Some(name)
        } else {
            None
        };

        let sdr_srgb_texture = if enable_display_encode {
            let needs_sdr = if is_hdr_native {
                matches!(
                    target_format,
                    TextureFormat::Rgba8Unorm
                        | TextureFormat::Bgra8Unorm
                        | TextureFormat::Rgba16Float
                )
            } else {
                matches!(
                    target_format,
                    TextureFormat::Rgba8UnormSrgb
                        | TextureFormat::Bgra8UnormSrgb
                        | TextureFormat::Rgba8Unorm
                        | TextureFormat::Bgra8Unorm
                )
            };
            if needs_sdr {
                let name: ResourceName = format!(
                    "{}{}",
                    target_texture_name.as_str(),
                    UI_PRESENT_SDR_SRGB_SUFFIX
                )
                .into();
                textures.push(TextureDecl {
                    name: name.clone(),
                    size: [tgt_w_u, tgt_h_u],
                    format: TextureFormat::Rgba8Unorm,
                    sample_count: 1,
                    needs_sampling: false,
                });
                Some(name)
            } else {
                None
            }
        } else {
            None
        };

        let pass_nodes_in_order =
            compute_pass_render_order(&prepared.scene, nodes_by_id, composite_layers_in_order)?;
        let warmup_root_ids = forward_root_dependencies_from_roots(
            &prepared.scene,
            nodes_by_id,
            composite_layers_in_order,
        )?;
        sampled_pass_ids.extend(warmup_root_ids.iter().cloned());

        let warmup_items: Vec<(String, bool)> = composite_layers_in_order
            .iter()
            .filter(|id| warmup_root_ids.contains(*id))
            .cloned()
            .map(|id| (id, true))
            .collect();
        let normal_items: Vec<(String, bool)> = pass_nodes_in_order
            .iter()
            .cloned()
            .map(|id| (id, false))
            .collect();
        let mut execution_items: Vec<(String, bool)> =
            Vec::with_capacity(warmup_items.len() + normal_items.len());
        execution_items.extend(warmup_items);
        execution_items.extend(normal_items);

        let registry = PassPlannerRegistry::default();
        let mut deferred_target_compose_passes_by_layer: HashMap<String, Vec<ResourceName>> =
            HashMap::new();

        for (layer_id, is_warmup_pass) in &execution_items {
            if !*is_warmup_pass && warmup_root_ids.contains(layer_id) {
                if let Some(deferred_passes) = deferred_target_compose_passes_by_layer.get(layer_id)
                {
                    composite_passes.extend(deferred_passes.iter().cloned());
                    continue;
                }
            }

            let branch_spec_start = render_pass_specs.len();
            let branch_composite_start = composite_passes.len();
            let layer_node = find_node(nodes_by_id, layer_id)?;
            let scene_ctx = SceneContext {
                prepared: &prepared,
                composition_contexts: &composition_contexts,
                composition_consumers_by_source: &composition_consumers_by_source,
                draw_coord_size_by_pass: &draw_coord_size_by_pass,
                asset_store,
                device: &planning_device,
                adapter,
            };
            let mut builder_state = BuilderState {
                target_texture_name: &target_texture_name,
                target_format,
                sampled_pass_format,
                tgt_size: [tgt_w, tgt_h],
                tgt_size_u: [tgt_w_u, tgt_h_u],
                geometry_buffers: &mut geometry_buffers,
                instance_buffers: &mut instance_buffers,
                textures: &mut textures,
                render_pass_specs: &mut render_pass_specs,
                composite_passes: &mut composite_passes,
                depth_resolve_passes: &mut depth_resolve_passes,
                pass_cull_mode_by_name: &mut pass_cull_mode_by_name,
                pass_depth_attachment_by_name: &mut pass_depth_attachment_by_name,
                pass_output_registry: &mut pass_output_registry,
                sampled_pass_ids: &sampled_pass_ids,
                baked_data_parse_meta_by_pass: &mut baked_data_parse_meta_by_pass,
                baked_data_parse_bytes_by_pass: &mut baked_data_parse_bytes_by_pass,
                baked_data_parse_buffer_to_pass_id: &mut baked_data_parse_buffer_to_pass_id,
                downsample_source_pass_ids: &mut downsample_source_pass_ids,
                upsample_source_pass_ids: &mut upsample_source_pass_ids,
                gaussian_source_pass_ids: &mut gaussian_source_pass_ids,
                bloom_source_pass_ids: &mut bloom_source_pass_ids,
                gradient_source_pass_ids: &mut gradient_source_pass_ids,
            };
            registry.plan_layer(&scene_ctx, &mut builder_state, layer_id, layer_node)?;
            drop(builder_state);

            if *is_warmup_pass {
                let target_writer_names: HashSet<ResourceName> = render_pass_specs
                    [branch_spec_start..]
                    .iter()
                    .filter(|spec| spec.target_texture == target_texture_name)
                    .map(|spec| spec.name.clone())
                    .collect();
                if !target_writer_names.is_empty() {
                    let prefix: Vec<ResourceName> =
                        composite_passes[..branch_composite_start].to_vec();
                    let mut deferred: Vec<ResourceName> = Vec::new();
                    let mut suffix: Vec<ResourceName> = Vec::new();
                    for name in composite_passes[branch_composite_start..].iter().cloned() {
                        if target_writer_names.contains(&name) {
                            deferred.push(name);
                        } else {
                            suffix.push(name);
                        }
                    }
                    if !deferred.is_empty() {
                        deferred_target_compose_passes_by_layer
                            .entry(layer_id.clone())
                            .or_default()
                            .extend(deferred);
                    }
                    composite_passes = prefix.into_iter().chain(suffix.into_iter()).collect();
                }
            }
        }

        let mut sdr_encode_pass_name: Option<ResourceName> = None;
        let encode_passes: Vec<(&ResourceName, &str, String)> = [
            sdr_srgb_texture.as_ref().map(|tex| {
                (
                    tex,
                    UI_PRESENT_SDR_SRGB_SUFFIX,
                    build_srgb_display_encode_wgsl("src_tex", "src_samp"),
                )
            }),
            hdr_gamma_texture.as_ref().map(|tex| {
                (
                    tex,
                    UI_PRESENT_HDR_GAMMA_SUFFIX,
                    build_hdr_gamma_encode_wgsl("src_tex", "src_samp"),
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
        for (encode_tex, suffix, shader_wgsl) in encode_passes {
            let pass_name: ResourceName =
                format!("{}{}.pass", target_texture_name.as_str(), suffix).into();
            let geo: ResourceName =
                format!("{}{}.geo", target_texture_name.as_str(), suffix).into();
            geometry_buffers.push((geo.clone(), make_fullscreen_geometry(tgt_w, tgt_h)));

            let params_name: ResourceName =
                format!("params.{}{}", target_texture_name.as_str(), suffix).into();
            let params = make_params(
                [tgt_w, tgt_h],
                [tgt_w, tgt_h],
                [tgt_w * 0.5, tgt_h * 0.5],
                legacy_projection_camera_matrix([tgt_w, tgt_h]),
                [0.0, 0.0, 0.0, 0.0],
            );

            render_pass_specs.push(RenderPassSpec {
                pass_id: pass_name.as_str().to_string(),
                name: pass_name.clone(),
                geometry_buffer: geo,
                instance_buffer: None,
                normals_buffer: None,
                target_texture: encode_tex.clone(),
                resolve_target: None,
                params_buffer: params_name,
                baked_data_parse_buffer: None,
                params,
                graph_binding: None,
                graph_values: None,
                shader_wgsl,
                texture_bindings: vec![PassTextureBinding {
                    texture: target_texture_name.clone(),
                    image_node_id: None,
                }],
                sampler_kinds: vec![SamplerKind::NearestClamp],
                blend_state: BlendState::REPLACE,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                sample_count: 1,
            });

            if is_hdr_native {
                if suffix == UI_PRESENT_SDR_SRGB_SUFFIX {
                    sdr_encode_pass_name = Some(pass_name);
                }
            } else {
                composite_passes.push(pass_name);
            }
        }

        normalize_first_write_load_ops(&composite_passes, &mut render_pass_specs);
        plan_image_textures(
            &prepared,
            asset_store,
            &render_pass_specs,
            &mut image_textures,
            &mut textures,
            &mut geometry_buffers,
            &mut image_prepasses,
            &mut prepass_texture_samples,
        )?;

        let pass_bindings: Vec<PassBindings> = render_pass_specs
            .iter()
            .map(|spec| PassBindings {
                pass_id: spec.pass_id.clone(),
                params_buffer: spec.params_buffer.clone(),
                base_params: spec.params,
                graph_binding: spec.graph_binding.clone(),
                last_graph_hash: spec.graph_values.as_ref().map(|v| hash_bytes(v.as_slice())),
            })
            .collect();
        let _pipeline_signature =
            compute_pipeline_signature_for_pass_bindings(&prepared.scene, &pass_bindings);

        let present_output_texture = match presentation_mode {
            ShaderSpacePresentationMode::UiHdrNative => output_texture_name.clone(),
            ShaderSpacePresentationMode::UiSdrDisplayEncode => sdr_srgb_texture
                .clone()
                .unwrap_or_else(|| output_texture_name.clone()),
            ShaderSpacePresentationMode::SceneLinear => output_texture_name.clone(),
        };
        let export_output_texture = if enable_display_encode {
            sdr_srgb_texture
                .clone()
                .unwrap_or_else(|| output_texture_name.clone())
        } else {
            output_texture_name.clone()
        };

        Ok(RenderPlan {
            prepared,
            scene_report,
            resolution,
            scene_output_texture: output_texture_name,
            present_output_texture,
            export_output_texture,
            export_encode_pass_name: sdr_encode_pass_name,
            resources: ResourcePlans {
                geometry_buffers,
                instance_buffers,
                textures,
                image_textures,
                render_pass_specs,
                composite_passes,
                depth_resolve_passes,
                image_prepasses,
                prepass_texture_samples,
                pass_cull_mode_by_name,
                pass_depth_attachment_by_name,
                pass_output_registry,
                pass_bindings,
                baked_data_parse_bytes_by_pass,
                baked_data_parse_buffer_to_pass_id,
            },
            debug_dump_wgsl_dir: self.options.debug_dump_wgsl_dir.clone(),
        })
    }
}

fn collect_processing_source_pass_ids(
    prepared: &PreparedScene,
) -> (
    HashSet<String>,
    HashSet<String>,
    HashSet<String>,
    HashSet<String>,
    HashSet<String>,
) {
    let mut downsample_source_pass_ids: HashSet<String> = HashSet::new();
    let mut upsample_source_pass_ids: HashSet<String> = HashSet::new();
    let mut gaussian_source_pass_ids: HashSet<String> = HashSet::new();
    let mut bloom_source_pass_ids: HashSet<String> = HashSet::new();
    let mut gradient_source_pass_ids: HashSet<String> = HashSet::new();

    for (node_id, node) in &prepared.nodes_by_id {
        if node.node_type == "Downsample" {
            if let Some(conn) = incoming_connection(&prepared.scene, node_id, "source") {
                downsample_source_pass_ids.insert(conn.from.node_id.clone());
            }
            continue;
        }
        if node.node_type == "Upsample" {
            if let Some(conn) = incoming_connection(&prepared.scene, node_id, "source") {
                upsample_source_pass_ids.insert(conn.from.node_id.clone());
            }
            continue;
        }
        if node.node_type == "GuassianBlurPass" {
            if let Some(conn) = incoming_connection(&prepared.scene, node_id, "pass") {
                gaussian_source_pass_ids.insert(conn.from.node_id.clone());
            }
            continue;
        }
        if node.node_type == "BloomNode" {
            if let Some(conn) = incoming_connection(&prepared.scene, node_id, "pass") {
                let src_is_pass_like = prepared
                    .nodes_by_id
                    .get(&conn.from.node_id)
                    .is_some_and(|n| is_pass_like_node_type(&n.node_type));
                if src_is_pass_like {
                    bloom_source_pass_ids.insert(conn.from.node_id.clone());
                }
            }
            continue;
        }
        if node.node_type == "GradientBlur" {
            if let Some(conn) = incoming_connection(&prepared.scene, node_id, "source") {
                let src_is_pass_like = prepared
                    .nodes_by_id
                    .get(&conn.from.node_id)
                    .is_some_and(|n| is_pass_like_node_type(&n.node_type));
                if src_is_pass_like {
                    gradient_source_pass_ids.insert(conn.from.node_id.clone());
                }
            }
        }
    }

    (
        downsample_source_pass_ids,
        upsample_source_pass_ids,
        gaussian_source_pass_ids,
        bloom_source_pass_ids,
        gradient_source_pass_ids,
    )
}

fn normalize_first_write_load_ops(
    composite_passes: &[ResourceName],
    render_pass_specs: &mut [RenderPassSpec],
) {
    let pass_order: HashMap<ResourceName, usize> = composite_passes
        .iter()
        .enumerate()
        .map(|(idx, name)| (name.clone(), idx))
        .collect();

    let mut spec_indices_by_exec_order: Vec<usize> = (0..render_pass_specs.len()).collect();
    spec_indices_by_exec_order.sort_by_key(|idx| {
        let name = &render_pass_specs[*idx].name;
        pass_order.get(name).copied().unwrap_or(usize::MAX)
    });

    let mut seen_targets: HashSet<ResourceName> = HashSet::new();
    for idx in spec_indices_by_exec_order {
        let spec = &mut render_pass_specs[idx];
        if seen_targets.insert(spec.target_texture.clone()) {
            spec.color_load_op = wgpu::LoadOp::Clear(Color::TRANSPARENT);
        } else {
            spec.color_load_op = wgpu::LoadOp::Load;
        }
    }
}

fn plan_image_textures(
    prepared: &PreparedScene,
    asset_store: Option<&AssetStore>,
    render_pass_specs: &[RenderPassSpec],
    image_textures: &mut Vec<ImageTextureSpec>,
    textures: &mut Vec<TextureDecl>,
    geometry_buffers: &mut Vec<(ResourceName, Arc<[u8]>)>,
    image_prepasses: &mut Vec<ImagePrepass>,
    prepass_texture_samples: &mut Vec<(String, ResourceName)>,
) -> Result<()> {
    let rel_base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut seen_image_nodes: HashSet<String> = HashSet::new();

    let specs_snapshot = render_pass_specs.to_vec();
    for pass in &specs_snapshot {
        for binding in &pass.texture_bindings {
            let Some(node_id) = binding.image_node_id.as_ref() else {
                continue;
            };
            if !seen_image_nodes.insert(node_id.clone()) {
                continue;
            }

            let node = find_node(&prepared.nodes_by_id, node_id)?;
            if node.node_type != "ImageTexture" && node.node_type != "Matcap" {
                bail!(
                    "expected ImageTexture node for {node_id}, got {}",
                    node.node_type
                );
            }

            let asset_id = node
                .params
                .get("assetId")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty());
            let encoder_space = node
                .params
                .get("encoderSpace")
                .and_then(|v| v.as_str())
                .unwrap_or("srgb")
                .trim()
                .to_ascii_lowercase();
            let is_srgb = match encoder_space.as_str() {
                "srgb" => true,
                "linear" => false,
                other => bail!("unsupported ImageTexture.encoderSpace: {other}"),
            };

            let image = if let Some(asset_id) = asset_id {
                if let Some(store) = asset_store {
                    match store.load_image(asset_id)? {
                        Some(image) => ensure_rgba8(Arc::new(image)),
                        None => bail!(
                            "ImageTexture node '{node_id}': asset '{asset_id}' not found in asset store"
                        ),
                    }
                } else {
                    bail!(
                        "ImageTexture node '{node_id}': has assetId '{asset_id}' but no asset store provided"
                    )
                }
            } else {
                let data_url = node
                    .params
                    .get("dataUrl")
                    .and_then(|v| v.as_str())
                    .or_else(|| node.params.get("data_url").and_then(|v| v.as_str()));
                match data_url {
                    Some(data_url) if !data_url.trim().is_empty() => {
                        load_image_from_data_url_checked(data_url, node_id)?
                    }
                    _ => {
                        let path = node.params.get("path").and_then(|v| v.as_str());
                        ensure_rgba8(load_image_from_path(&rel_base, path, node_id)?)
                    }
                }
            };

            let alpha_mode = node
                .params
                .get("alphaMode")
                .and_then(|v| v.as_str())
                .unwrap_or("straight")
                .trim()
                .to_ascii_lowercase();
            let needs_premultiply = match alpha_mode.as_str() {
                "straight" => true,
                "premultiplied" => false,
                other => bail!("unsupported ImageTexture.alphaMode: {other}"),
            };

            let img_w = image.width();
            let img_h = image.height();
            let name = prepared
                .ids
                .get(node_id)
                .cloned()
                .ok_or_else(|| anyhow!("missing name for node: {node_id}"))?;

            if needs_premultiply {
                let src_name: ResourceName = format!("sys.image.{node_id}.src").into();
                image_textures.push(ImageTextureSpec {
                    name: src_name.clone(),
                    image,
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                    srgb: is_srgb,
                });

                textures.push(TextureDecl {
                    name: name.clone(),
                    size: [img_w, img_h],
                    format: if is_srgb {
                        TextureFormat::Rgba16Float
                    } else {
                        TextureFormat::Rgba8Unorm
                    },
                    sample_count: 1,
                    needs_sampling: false,
                });

                let w = img_w as f32;
                let h = img_h as f32;
                let geo: ResourceName = format!("sys.image.{node_id}.premultiply.geo").into();
                geometry_buffers.push((geo.clone(), make_fullscreen_geometry(w, h)));

                let params_buffer: ResourceName =
                    format!("params.sys.image.{node_id}.premultiply").into();
                let params = make_params(
                    [w, h],
                    [w, h],
                    [w * 0.5, h * 0.5],
                    legacy_projection_camera_matrix([w, h]),
                    [0.0, 0.0, 0.0, 0.0],
                );
                let tex_var = MaterialCompileContext::tex_var_name(src_name.as_str());
                let samp_var = MaterialCompileContext::sampler_var_name(src_name.as_str());
                let shader_wgsl = build_image_premultiply_wgsl(&tex_var, &samp_var);
                let pass_name: ResourceName =
                    format!("sys.image.{node_id}.premultiply.pass").into();

                image_prepasses.push(ImagePrepass {
                    pass_name: pass_name.clone(),
                    geometry_buffer: geo,
                    params_buffer,
                    params,
                    src_texture: src_name.clone(),
                    dst_texture: name,
                    shader_wgsl,
                });
                prepass_texture_samples.push((pass_name.as_str().to_string(), src_name));
            } else {
                image_textures.push(ImageTextureSpec {
                    name,
                    image,
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                    srgb: is_srgb,
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use crate::{
        asset_store::{self, AssetStore},
        dsl,
        renderer::render_plan::types::PlanningGpuCaps,
    };

    #[derive(Debug, PartialEq, Eq)]
    struct PlanSummary {
        resolution: [u32; 2],
        scene_output_texture: String,
        present_output_texture: String,
        export_output_texture: String,
        export_encode_pass_name: Option<String>,
        textures: Vec<String>,
        image_textures: Vec<String>,
        image_prepasses: Vec<String>,
        pass_order: Vec<String>,
        load_ops: Vec<String>,
        graph_bound_passes: Vec<String>,
        pass_outputs: Vec<String>,
    }

    fn planner_for_mode(presentation_mode: ShaderSpacePresentationMode) -> RenderPlanner {
        RenderPlanner::new(PlanBuildOptions {
            gpu_caps: PlanningGpuCaps::default(),
            presentation_mode,
            debug_dump_wgsl_dir: None,
        })
    }

    fn load_case(case_name: &str) -> Result<(SceneDSL, Option<AssetStore>)> {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("cases")
            .join(case_name);
        let scene_path = base.join("scene.json");
        let scene = dsl::load_scene_from_path(&scene_path)?;
        let assets = if scene.assets.is_empty() {
            None
        } else {
            Some(asset_store::load_from_scene_dir(&scene, Path::new(&base))?)
        };
        Ok((scene, assets))
    }

    fn summarize(plan: &RenderPlan) -> PlanSummary {
        let mut textures: Vec<String> = plan
            .resources
            .textures
            .iter()
            .map(|texture| {
                format!(
                    "{}:{}:{}x{}:samples={}",
                    texture.name.as_str(),
                    format!("{:?}", texture.format),
                    texture.size[0],
                    texture.size[1],
                    texture.sample_count
                )
            })
            .collect();
        textures.sort();

        let mut image_textures: Vec<String> = plan
            .resources
            .image_textures
            .iter()
            .map(|texture| texture.name.as_str().to_string())
            .collect();
        image_textures.sort();

        let image_prepasses: Vec<String> = plan
            .resources
            .image_prepasses
            .iter()
            .map(|prepass| prepass.pass_name.as_str().to_string())
            .collect();

        let load_ops: Vec<String> = plan
            .resources
            .render_pass_specs
            .iter()
            .map(|spec| {
                let load = match spec.color_load_op {
                    wgpu::LoadOp::Clear(_) => "Clear",
                    wgpu::LoadOp::Load => "Load",
                };
                format!(
                    "{}->{}/{}",
                    spec.name.as_str(),
                    spec.target_texture.as_str(),
                    load
                )
            })
            .collect();

        let mut graph_bound_passes: Vec<String> = plan
            .resources
            .render_pass_specs
            .iter()
            .filter(|spec| spec.graph_binding.is_some())
            .map(|spec| spec.name.as_str().to_string())
            .collect();
        graph_bound_passes.sort();

        let mut pass_outputs: Vec<String> = Vec::new();
        for node_id in &plan.prepared.topo_order {
            if let Some(spec) = plan.resources.pass_output_registry.get(node_id) {
                pass_outputs.push(format!(
                    "{}:pass->{}:{}x{}:{:?}",
                    node_id,
                    spec.texture_name.as_str(),
                    spec.resolution[0],
                    spec.resolution[1],
                    spec.format
                ));
            }
            if let Some(spec) = plan
                .resources
                .pass_output_registry
                .get_for_port(node_id, "depth")
            {
                pass_outputs.push(format!(
                    "{}:depth->{}:{}x{}:{:?}",
                    node_id,
                    spec.texture_name.as_str(),
                    spec.resolution[0],
                    spec.resolution[1],
                    spec.format
                ));
            }
        }

        PlanSummary {
            resolution: plan.resolution,
            scene_output_texture: plan.scene_output_texture.as_str().to_string(),
            present_output_texture: plan.present_output_texture.as_str().to_string(),
            export_output_texture: plan.export_output_texture.as_str().to_string(),
            export_encode_pass_name: plan
                .export_encode_pass_name
                .as_ref()
                .map(|name| name.as_str().to_string()),
            textures,
            image_textures,
            image_prepasses,
            pass_order: plan
                .resources
                .composite_passes
                .iter()
                .map(|name| name.as_str().to_string())
                .collect(),
            load_ops,
            graph_bound_passes,
            pass_outputs,
        }
    }

    #[test]
    fn graph_rectangle_plan_summary_is_stable() -> Result<()> {
        let (scene, assets) = load_case("graph-rectangle")?;
        let plan = planner_for_mode(ShaderSpacePresentationMode::UiSdrDisplayEncode).plan(
            &scene,
            assets.as_ref(),
            None,
        )?;
        let summary = format!("{:#?}", summarize(&plan));
        let expected = r#"PlanSummary {
    resolution: [
        1080,
        2400,
    ],
    scene_output_texture: "node_5",
    present_output_texture: "node_5.present.sdr.srgb",
    export_output_texture: "node_5.present.sdr.srgb",
    export_encode_pass_name: None,
    textures: [
        "node_5.present.sdr.srgb:Rgba8Unorm:1080x2400:samples=1",
        "node_5:Rgba8Unorm:1080x2400:samples=1",
    ],
    image_textures: [],
    image_prepasses: [],
    pass_order: [
        "node_2.pass",
        "node_5.present.sdr.srgb.pass",
    ],
    load_ops: [
        "node_2.pass->node_5/Clear",
        "node_5.present.sdr.srgb.pass->node_5.present.sdr.srgb/Clear",
    ],
    graph_bound_passes: [
        "node_2.pass",
    ],
    pass_outputs: [
        "node_2:pass->node_5:1080x2400:Rgba8Unorm",
    ],
}"#;
        assert_eq!(summary, expected);
        Ok(())
    }

    #[test]
    fn blur_chain_plan_summary_is_stable() -> Result<()> {
        let (scene, assets) = load_case("graph-blur-pass")?;
        let plan = planner_for_mode(ShaderSpacePresentationMode::UiSdrDisplayEncode).plan(
            &scene,
            assets.as_ref(),
            None,
        )?;
        let summary = format!("{:#?}", summarize(&plan));
        let expected = r#"PlanSummary {
    resolution: [
        1080,
        2400,
    ],
    scene_output_texture: "node_5",
    present_output_texture: "node_5.present.sdr.srgb",
    export_output_texture: "node_5.present.sdr.srgb",
    export_encode_pass_name: None,
    textures: [
        "node_15:Rgba16Float:1080x2400:samples=1",
        "node_5.present.sdr.srgb:Rgba8Unorm:1080x2400:samples=1",
        "node_5:Rgba8UnormSrgb:1080x2400:samples=1",
        "sys.blur.GuassianBlurPass_17.h:Rgba8Unorm:1080x2400:samples=1",
        "sys.blur.GuassianBlurPass_17.src:Rgba8Unorm:1080x2400:samples=1",
        "sys.blur.GuassianBlurPass_17.v:Rgba8Unorm:1080x2400:samples=1",
        "sys.blur.GuassianBlurPass_18.h:Rgba8Unorm:1080x2400:samples=1",
        "sys.blur.GuassianBlurPass_18.src:Rgba8Unorm:1080x2400:samples=1",
        "sys.blur.GuassianBlurPass_18.v:Rgba8Unorm:1080x2400:samples=1",
        "sys.pass.node_11.out:Rgba8Unorm:1080x2400:samples=1",
    ],
    image_textures: [
        "sys.image.node_15.src",
    ],
    image_prepasses: [
        "sys.image.node_15.premultiply.pass",
    ],
    pass_order: [
        "node_11.pass",
        "sys.blur.GuassianBlurPass_17.src.pass",
        "sys.blur.GuassianBlurPass_17.h.ds1.pass",
        "sys.blur.GuassianBlurPass_17.v.ds1.pass",
        "sys.blur.GuassianBlurPass_18.src.pass",
        "sys.blur.GuassianBlurPass_18.h.ds1.pass",
        "sys.blur.GuassianBlurPass_18.v.ds1.pass",
        "sys.blur.GuassianBlurPass_18.upsample_bilinear.ds1.pass",
        "node_2.pass",
        "node_5.present.sdr.srgb.pass",
    ],
    load_ops: [
        "node_11.pass->sys.pass.node_11.out/Clear",
        "sys.blur.GuassianBlurPass_17.src.pass->sys.blur.GuassianBlurPass_17.src/Clear",
        "sys.blur.GuassianBlurPass_17.h.ds1.pass->sys.blur.GuassianBlurPass_17.h/Clear",
        "sys.blur.GuassianBlurPass_17.v.ds1.pass->sys.blur.GuassianBlurPass_17.v/Clear",
        "sys.blur.GuassianBlurPass_18.src.pass->sys.blur.GuassianBlurPass_18.src/Clear",
        "sys.blur.GuassianBlurPass_18.h.ds1.pass->sys.blur.GuassianBlurPass_18.h/Clear",
        "sys.blur.GuassianBlurPass_18.v.ds1.pass->sys.blur.GuassianBlurPass_18.v/Clear",
        "sys.blur.GuassianBlurPass_18.upsample_bilinear.ds1.pass->node_5/Clear",
        "node_2.pass->node_5/Load",
        "node_5.present.sdr.srgb.pass->node_5.present.sdr.srgb/Clear",
    ],
    graph_bound_passes: [
        "node_2.pass",
    ],
    pass_outputs: [
        "node_11:pass->sys.pass.node_11.out:1080x2400:Rgba8Unorm",
        "GuassianBlurPass_17:pass->sys.blur.GuassianBlurPass_17.v:1080x2400:Rgba8Unorm",
        "node_2:pass->node_5:1080x2400:Rgba8UnormSrgb",
        "GuassianBlurPass_18:pass->node_5:1080x2400:Rgba8UnormSrgb",
    ],
}"#;
        assert_eq!(summary, expected);
        Ok(())
    }

    #[test]
    fn hdr_bloom_plan_summary_is_stable() -> Result<()> {
        let (scene, assets) = load_case("hdr-bloom-nodes")?;
        let plan = planner_for_mode(ShaderSpacePresentationMode::UiHdrNative).plan(
            &scene,
            assets.as_ref(),
            None,
        )?;
        let summary = format!("{:#?}", summarize(&plan));
        let expected = r#"PlanSummary {
    resolution: [
        1080,
        2400,
    ],
    scene_output_texture: "RenderTexture_6",
    present_output_texture: "RenderTexture_6",
    export_output_texture: "RenderTexture_6.present.sdr.srgb",
    export_encode_pass_name: Some(
        "RenderTexture_6.present.sdr.srgb.pass",
    ),
    textures: [
        "RenderTexture_6.present.hdr.gamma:Rgba16Float:1080x2400:samples=1",
        "RenderTexture_6.present.sdr.srgb:Rgba8Unorm:1080x2400:samples=1",
        "RenderTexture_6:Rgba16Float:1080x2400:samples=1",
        "sys.blur.GuassianBlurPass_23.h:Rgba16Float:16x37:samples=1",
        "sys.blur.GuassianBlurPass_23.src:Rgba16Float:16x37:samples=1",
        "sys.blur.GuassianBlurPass_23.v:Rgba16Float:16x37:samples=1",
        "sys.blur.GuassianBlurPass_27.h:Rgba16Float:33x75:samples=1",
        "sys.blur.GuassianBlurPass_27.src:Rgba16Float:33x75:samples=1",
        "sys.blur.GuassianBlurPass_27.v:Rgba16Float:33x75:samples=1",
        "sys.blur.GuassianBlurPass_31.h:Rgba16Float:67x150:samples=1",
        "sys.blur.GuassianBlurPass_31.src:Rgba16Float:67x150:samples=1",
        "sys.blur.GuassianBlurPass_31.v:Rgba16Float:67x150:samples=1",
        "sys.blur.GuassianBlurPass_34.h:Rgba16Float:135x300:samples=1",
        "sys.blur.GuassianBlurPass_34.src:Rgba16Float:135x300:samples=1",
        "sys.blur.GuassianBlurPass_34.v:Rgba16Float:135x300:samples=1",
        "sys.blur.GuassianBlurPass_37.h:Rgba16Float:270x600:samples=1",
        "sys.blur.GuassianBlurPass_37.src:Rgba16Float:270x600:samples=1",
        "sys.blur.GuassianBlurPass_37.v:Rgba16Float:270x600:samples=1",
        "sys.blur.GuassianBlurPass_40.h:Rgba16Float:540x1200:samples=1",
        "sys.blur.GuassianBlurPass_40.src:Rgba16Float:540x1200:samples=1",
        "sys.blur.GuassianBlurPass_40.v:Rgba16Float:540x1200:samples=1",
        "sys.downsample.Downsample_10.out:Rgba16Float:540x1200:samples=1",
        "sys.downsample.Downsample_12.out:Rgba16Float:270x600:samples=1",
        "sys.downsample.Downsample_14.out:Rgba16Float:135x300:samples=1",
        "sys.downsample.Downsample_16.out:Rgba16Float:67x150:samples=1",
        "sys.downsample.Downsample_18.out:Rgba16Float:33x75:samples=1",
        "sys.downsample.Downsample_20.out:Rgba16Float:16x37:samples=1",
        "sys.msaa.sys.pass.RenderPass_4.out.4.color:Rgba16Float:1080x2400:samples=4",
        "sys.pass.RenderPass_4.out:Rgba16Float:1080x2400:samples=1",
        "sys.upsample.Upsample_24.out:Rgba16Float:33x75:samples=1",
        "sys.upsample.Upsample_28.out:Rgba16Float:67x150:samples=1",
        "sys.upsample.Upsample_32.out:Rgba16Float:135x300:samples=1",
        "sys.upsample.Upsample_35.out:Rgba16Float:270x600:samples=1",
        "sys.upsample.Upsample_38.out:Rgba16Float:540x1200:samples=1",
        "sys.upsample.Upsample_41.out:Rgba16Float:1080x2400:samples=1",
    ],
    image_textures: [],
    image_prepasses: [],
    pass_order: [
        "mip0.rpass4.pass",
        "sys.downsample.Downsample_10.pass",
        "sys.downsample.Downsample_12.pass",
        "sys.downsample.Downsample_14.pass",
        "sys.downsample.Downsample_16.pass",
        "sys.downsample.Downsample_18.pass",
        "sys.downsample.Downsample_20.pass",
        "sys.blur.GuassianBlurPass_23.src.pass",
        "sys.blur.GuassianBlurPass_23.h.ds1.pass",
        "sys.blur.GuassianBlurPass_23.v.ds1.pass",
        "sys.upsample.Upsample_24.pass",
        "sys.blur.GuassianBlurPass_27.src.pass",
        "sys.blur.GuassianBlurPass_27.h.ds1.pass",
        "sys.blur.GuassianBlurPass_27.v.ds1.pass",
        "sys.upsample.Upsample_28.pass",
        "sys.blur.GuassianBlurPass_31.src.pass",
        "sys.blur.GuassianBlurPass_31.h.ds1.pass",
        "sys.blur.GuassianBlurPass_31.v.ds1.pass",
        "sys.upsample.Upsample_32.pass",
        "sys.blur.GuassianBlurPass_34.src.pass",
        "sys.blur.GuassianBlurPass_34.h.ds1.pass",
        "sys.blur.GuassianBlurPass_34.v.ds1.pass",
        "sys.upsample.Upsample_35.pass",
        "sys.blur.GuassianBlurPass_37.src.pass",
        "sys.blur.GuassianBlurPass_37.h.ds1.pass",
        "sys.blur.GuassianBlurPass_37.v.ds1.pass",
        "sys.upsample.Upsample_38.pass",
        "sys.blur.GuassianBlurPass_40.src.pass",
        "sys.blur.GuassianBlurPass_40.h.ds1.pass",
        "sys.blur.GuassianBlurPass_40.v.ds1.pass",
        "sys.upsample.Upsample_41.pass",
        "sys.auto.fullscreen.pass.edge_75.pass",
    ],
    load_ops: [
        "mip0.rpass4.pass->sys.msaa.sys.pass.RenderPass_4.out.4.color/Clear",
        "sys.downsample.Downsample_10.pass->sys.downsample.Downsample_10.out/Clear",
        "sys.downsample.Downsample_12.pass->sys.downsample.Downsample_12.out/Clear",
        "sys.downsample.Downsample_14.pass->sys.downsample.Downsample_14.out/Clear",
        "sys.downsample.Downsample_16.pass->sys.downsample.Downsample_16.out/Clear",
        "sys.downsample.Downsample_18.pass->sys.downsample.Downsample_18.out/Clear",
        "sys.downsample.Downsample_20.pass->sys.downsample.Downsample_20.out/Clear",
        "sys.blur.GuassianBlurPass_23.src.pass->sys.blur.GuassianBlurPass_23.src/Clear",
        "sys.blur.GuassianBlurPass_23.h.ds1.pass->sys.blur.GuassianBlurPass_23.h/Clear",
        "sys.blur.GuassianBlurPass_23.v.ds1.pass->sys.blur.GuassianBlurPass_23.v/Clear",
        "sys.upsample.Upsample_24.pass->sys.upsample.Upsample_24.out/Clear",
        "sys.blur.GuassianBlurPass_27.src.pass->sys.blur.GuassianBlurPass_27.src/Clear",
        "sys.blur.GuassianBlurPass_27.h.ds1.pass->sys.blur.GuassianBlurPass_27.h/Clear",
        "sys.blur.GuassianBlurPass_27.v.ds1.pass->sys.blur.GuassianBlurPass_27.v/Clear",
        "sys.upsample.Upsample_28.pass->sys.upsample.Upsample_28.out/Clear",
        "sys.blur.GuassianBlurPass_31.src.pass->sys.blur.GuassianBlurPass_31.src/Clear",
        "sys.blur.GuassianBlurPass_31.h.ds1.pass->sys.blur.GuassianBlurPass_31.h/Clear",
        "sys.blur.GuassianBlurPass_31.v.ds1.pass->sys.blur.GuassianBlurPass_31.v/Clear",
        "sys.upsample.Upsample_32.pass->sys.upsample.Upsample_32.out/Clear",
        "sys.blur.GuassianBlurPass_34.src.pass->sys.blur.GuassianBlurPass_34.src/Clear",
        "sys.blur.GuassianBlurPass_34.h.ds1.pass->sys.blur.GuassianBlurPass_34.h/Clear",
        "sys.blur.GuassianBlurPass_34.v.ds1.pass->sys.blur.GuassianBlurPass_34.v/Clear",
        "sys.upsample.Upsample_35.pass->sys.upsample.Upsample_35.out/Clear",
        "sys.blur.GuassianBlurPass_37.src.pass->sys.blur.GuassianBlurPass_37.src/Clear",
        "sys.blur.GuassianBlurPass_37.h.ds1.pass->sys.blur.GuassianBlurPass_37.h/Clear",
        "sys.blur.GuassianBlurPass_37.v.ds1.pass->sys.blur.GuassianBlurPass_37.v/Clear",
        "sys.upsample.Upsample_38.pass->sys.upsample.Upsample_38.out/Clear",
        "sys.blur.GuassianBlurPass_40.src.pass->sys.blur.GuassianBlurPass_40.src/Clear",
        "sys.blur.GuassianBlurPass_40.h.ds1.pass->sys.blur.GuassianBlurPass_40.h/Clear",
        "sys.blur.GuassianBlurPass_40.v.ds1.pass->sys.blur.GuassianBlurPass_40.v/Clear",
        "sys.upsample.Upsample_41.pass->sys.upsample.Upsample_41.out/Clear",
        "sys.auto.fullscreen.pass.edge_75.pass->RenderTexture_6/Clear",
        "RenderTexture_6.present.sdr.srgb.pass->RenderTexture_6.present.sdr.srgb/Clear",
        "RenderTexture_6.present.hdr.gamma.pass->RenderTexture_6.present.hdr.gamma/Clear",
    ],
    graph_bound_passes: [
        "mip0.rpass4.pass",
    ],
    pass_outputs: [
        "RenderPass_4:pass->sys.pass.RenderPass_4.out:1080x2400:Rgba16Float",
        "Downsample_10:pass->sys.downsample.Downsample_10.out:540x1200:Rgba16Float",
        "Downsample_12:pass->sys.downsample.Downsample_12.out:270x600:Rgba16Float",
        "Downsample_14:pass->sys.downsample.Downsample_14.out:135x300:Rgba16Float",
        "Downsample_16:pass->sys.downsample.Downsample_16.out:67x150:Rgba16Float",
        "Downsample_18:pass->sys.downsample.Downsample_18.out:33x75:Rgba16Float",
        "Downsample_20:pass->sys.downsample.Downsample_20.out:16x37:Rgba16Float",
        "GuassianBlurPass_23:pass->sys.blur.GuassianBlurPass_23.v:16x37:Rgba16Float",
        "Upsample_24:pass->sys.upsample.Upsample_24.out:33x75:Rgba16Float",
        "GuassianBlurPass_27:pass->sys.blur.GuassianBlurPass_27.v:33x75:Rgba16Float",
        "Upsample_28:pass->sys.upsample.Upsample_28.out:67x150:Rgba16Float",
        "GuassianBlurPass_31:pass->sys.blur.GuassianBlurPass_31.v:67x150:Rgba16Float",
        "Upsample_32:pass->sys.upsample.Upsample_32.out:135x300:Rgba16Float",
        "GuassianBlurPass_34:pass->sys.blur.GuassianBlurPass_34.v:135x300:Rgba16Float",
        "Upsample_35:pass->sys.upsample.Upsample_35.out:270x600:Rgba16Float",
        "GuassianBlurPass_37:pass->sys.blur.GuassianBlurPass_37.v:270x600:Rgba16Float",
        "Upsample_38:pass->sys.upsample.Upsample_38.out:540x1200:Rgba16Float",
        "GuassianBlurPass_40:pass->sys.blur.GuassianBlurPass_40.v:540x1200:Rgba16Float",
        "Upsample_41:pass->sys.upsample.Upsample_41.out:1080x2400:Rgba16Float",
        "sys.auto.fullscreen.pass.edge_75:pass->RenderTexture_6:1080x2400:Rgba16Float",
    ],
}"#;
        assert_eq!(summary, expected);
        Ok(())
    }

    #[test]
    fn colorspace_image_plan_summary_is_stable() -> Result<()> {
        let (scene, assets) = load_case("colorspace-image")?;
        let plan = planner_for_mode(ShaderSpacePresentationMode::UiSdrDisplayEncode).plan(
            &scene,
            assets.as_ref(),
            None,
        )?;
        let summary = format!("{:#?}", summarize(&plan));
        let expected = r#"PlanSummary {
    resolution: [
        1080,
        2400,
    ],
    scene_output_texture: "RenderTexture_7",
    present_output_texture: "RenderTexture_7.present.sdr.srgb",
    export_output_texture: "RenderTexture_7.present.sdr.srgb",
    export_encode_pass_name: None,
    textures: [
        "ImageTexture_9:Rgba16Float:1080x2400:samples=1",
        "RenderTexture_7.present.sdr.srgb:Rgba8Unorm:1080x2400:samples=1",
        "RenderTexture_7:Rgba8UnormSrgb:1080x2400:samples=1",
    ],
    image_textures: [
        "sys.image.ImageTexture_9.src",
    ],
    image_prepasses: [
        "sys.image.ImageTexture_9.premultiply.pass",
    ],
    pass_order: [
        "sys.auto.fullscreen.pass.edge_7.pass",
        "RenderTexture_7.present.sdr.srgb.pass",
    ],
    load_ops: [
        "sys.auto.fullscreen.pass.edge_7.pass->RenderTexture_7/Clear",
        "RenderTexture_7.present.sdr.srgb.pass->RenderTexture_7.present.sdr.srgb/Clear",
    ],
    graph_bound_passes: [],
    pass_outputs: [
        "sys.auto.fullscreen.pass.edge_7:pass->RenderTexture_7:1080x2400:Rgba8UnormSrgb",
    ],
}"#;
        assert_eq!(summary, expected);
        assert_eq!(plan.resources.image_prepasses.len(), 1);
        Ok(())
    }

    #[test]
    fn presentation_routing_modes_are_planned_without_gpu() -> Result<()> {
        let (scene, assets) = load_case("graph-rectangle")?;

        let linear = planner_for_mode(ShaderSpacePresentationMode::SceneLinear).plan(
            &scene,
            assets.as_ref(),
            None,
        )?;
        assert_eq!(linear.present_output_texture.as_str(), "node_5");
        assert_eq!(linear.export_output_texture.as_str(), "node_5");
        assert_eq!(linear.export_encode_pass_name, None);

        let sdr = planner_for_mode(ShaderSpacePresentationMode::UiSdrDisplayEncode).plan(
            &scene,
            assets.as_ref(),
            None,
        )?;
        assert_eq!(
            sdr.present_output_texture.as_str(),
            "node_5.present.sdr.srgb"
        );
        assert_eq!(
            sdr.export_output_texture.as_str(),
            "node_5.present.sdr.srgb"
        );
        assert_eq!(sdr.export_encode_pass_name, None);

        let hdr = planner_for_mode(ShaderSpacePresentationMode::UiHdrNative).plan(
            &scene,
            assets.as_ref(),
            None,
        )?;
        assert_eq!(hdr.present_output_texture.as_str(), "node_5");
        assert_eq!(
            hdr.export_output_texture.as_str(),
            "node_5.present.sdr.srgb"
        );
        assert_eq!(
            hdr.export_encode_pass_name
                .as_ref()
                .map(|name| name.as_str()),
            Some("node_5.present.sdr.srgb.pass")
        );

        Ok(())
    }
}
