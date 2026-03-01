//! Gaussian blur pass assembler.
//!
//! Handles the `"GuassianBlurPass"` node type. Takes a source pass texture,
//! optionally downsamples, applies horizontal + vertical separated Gaussian blur,
//! and optionally upsamples back to the target resolution.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
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
        utils::{cpu_num_f32_min_0, cpu_num_u32_min_1},
        wgsl::{
            build_blur_image_wgsl_bundle, build_blur_image_wgsl_bundle_with_graph_binding,
            build_downsample_bundle, build_fullscreen_textured_bundle,
            build_horizontal_blur_bundle_with_tap_count, build_upsample_bilinear_bundle,
            build_vertical_blur_bundle_with_tap_count, clamp_min_1, gaussian_kernel_8,
            gaussian_mip_level_and_sigma_p,
        },
    },
};

use super::args::{BuilderState, SceneContext, make_fullscreen_geometry};
use super::super::image_utils::image_node_dimensions;
use super::super::pass_spec::{
    PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl, make_params,
};
use super::super::resource_naming::{
    blur_downsample_steps_for_factor, gaussian_blur_extend_upsample_geo_size,
    infer_uniform_resolution_from_pass_deps, resolve_chain_camera_for_first_pass,
    should_skip_blur_downsample_pass, should_skip_blur_upsample_pass,
};
use super::super::sampler::{sampler_kind_for_pass_texture, sampler_kind_from_node_params};

/// Assemble a `"GuassianBlurPass"` layer.
pub(crate) fn assemble_gaussian_blur(
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

    // Determine the base resolution for this blur pass.
    let mut base_resolution: [u32; 2] = [tgt_w_u, tgt_h_u];
    let mut blur_output_center: Option<[f32; 2]> = None;

    let radius_px =
        cpu_num_f32_min_0(&prepared.scene, nodes_by_id, layer_node, "radius", 0.0)?;
    let extend_enabled = layer_node
        .params
        .get("extend")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let extend_pad_px: u32 = if extend_enabled {
        radius_px.ceil().max(0.0) as u32
    } else {
        0
    };
    let can_direct_bypass = extend_pad_px == 0;
    let mut blur_chain_first_camera_consumed = false;

    // Optimization: skip the intermediate `sys.blur.<id>.src` pass when we can
    // directly consume an existing texture resource as the blur source.
    let mut initial_blur_source_texture: Option<ResourceName> = None;
    let mut initial_blur_source_image_node_id: Option<String> = None;
    let mut initial_blur_source_sampler_kind: Option<SamplerKind> = None;
    if let Some(src_conn) = incoming_connection(&prepared.scene, layer_id, "pass") {
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
                        base_resolution = [
                            src_geo_w.max(1.0).round() as u32,
                            src_geo_h.max(1.0).round() as u32,
                        ];
                        blur_output_center = Some([src_geo_x, src_geo_y]);
                    }
                }
            }
        }

        // (A) Upstream pass output bypass.
        if let Some(src_spec) = bs
            .pass_output_registry
            .get_for_port(&src_conn.from.node_id, &src_conn.from.port_id)
        {
            base_resolution = src_spec.resolution;
            if can_direct_bypass && src_spec.format == sampled_pass_format {
                initial_blur_source_texture = Some(src_spec.texture_name.clone());
                initial_blur_source_sampler_kind = Some(sampler_kind_for_pass_texture(
                    &prepared.scene,
                    &src_conn.from.node_id,
                ));
            }
        } else {
            // Non-pass source path (e.g. MathClosure with samplePass):
            // infer blur source resolution from its transitive pass dependencies.
            let src_bundle =
                build_blur_image_wgsl_bundle(&prepared.scene, nodes_by_id, layer_id)?;
            if let Some(inferred_resolution) = infer_uniform_resolution_from_pass_deps(
                layer_id,
                &src_bundle.pass_textures,
                &bs.pass_output_registry,
            )? {
                base_resolution = inferred_resolution;
            }
        }

        // (B) ImageTexture direct bypass (only when UV is default).
        if initial_blur_source_texture.is_none() {
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
                    if let Some(dims) = image_node_dimensions(src_node, asset_store) {
                        base_resolution = dims;
                    }
                    if let Some(tex) = ids.get(&src_conn.from.node_id).cloned() {
                        initial_blur_source_texture = Some(tex);
                        initial_blur_source_image_node_id =
                            Some(src_conn.from.node_id.clone());
                        initial_blur_source_sampler_kind =
                            Some(sampler_kind_from_node_params(&src_node.params));
                    }
                }
            }
        }
    }
    let src_content_resolution = base_resolution;
    let src_resolution = if extend_pad_px > 0 {
        [
            src_content_resolution[0].saturating_add(extend_pad_px.saturating_mul(2)),
            src_content_resolution[1].saturating_add(extend_pad_px.saturating_mul(2)),
        ]
    } else {
        src_content_resolution
    };
    let src_w = src_resolution[0] as f32;
    let src_h = src_resolution[1] as f32;
    let src_content_w = src_content_resolution[0] as f32;
    let src_content_h = src_content_resolution[1] as f32;

    // Keep camera semantics stable across bypass/elision.
    let force_source_pass_for_custom_camera = pass_node_uses_custom_camera(
        &prepared.scene,
        nodes_by_id,
        layer_node,
        [src_w, src_h],
    )?;
    if force_source_pass_for_custom_camera {
        initial_blur_source_texture = None;
        initial_blur_source_image_node_id = None;
        initial_blur_source_sampler_kind = None;
    }

    let initial_blur_source_texture: ResourceName = if let Some(existing_tex) =
        initial_blur_source_texture
    {
        existing_tex
    } else {
        // Create source texture for the pass input.
        let src_tex: ResourceName = format!("sys.blur.{layer_id}.src").into();
        bs.textures.push(TextureDecl {
            name: src_tex.clone(),
            size: src_resolution,
            format: sampled_pass_format,
            sample_count: 1,
            needs_sampling: false,
        });

        // Build source pass geometry.
        let src_geo_w = if extend_pad_px > 0 { src_content_w } else { src_w };
        let src_geo_h = if extend_pad_px > 0 { src_content_h } else { src_h };

        let geo_src: ResourceName = format!("sys.blur.{layer_id}.src.geo").into();
        bs.geometry_buffers.push((
            geo_src.clone(),
            make_fullscreen_geometry(src_geo_w, src_geo_h),
        ));

        let params_src: ResourceName = format!("params.sys.blur.{layer_id}.src").into();
        let params_src_val = make_params(
            [src_w, src_h],
            [src_geo_w, src_geo_h],
            [src_w * 0.5, src_h * 0.5],
            resolve_chain_camera_for_first_pass(
                &mut blur_chain_first_camera_consumed,
                &prepared.scene,
                nodes_by_id,
                layer_node,
                [src_w, src_h],
            )?,
            [0.0, 0.0, 0.0, 0.0],
        );

        // Build WGSL for sampling the `pass` input source.
        let mut src_bundle =
            build_blur_image_wgsl_bundle(&prepared.scene, nodes_by_id, layer_id)?;
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
                src_bundle = build_blur_image_wgsl_bundle_with_graph_binding(
                    &prepared.scene,
                    nodes_by_id,
                    layer_id,
                    Some(kind),
                )?;
            }
            let schema = src_bundle
                .graph_schema
                .clone()
                .ok_or_else(|| anyhow!("missing blur source graph schema"))?;
            let values = pack_graph_values(&prepared.scene, &schema)?;
            src_graph_values = Some(values);
            src_graph_binding = Some(GraphBinding {
                buffer_name: format!("params.sys.blur.{layer_id}.src.graph").into(),
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

        let src_pass_bindings =
            crate::renderer::render_plan::resolve_pass_texture_bindings(
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

        let src_pass_name: ResourceName =
            format!("sys.blur.{layer_id}.src.pass").into();
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

    // Resolution: use source resolution (possibly extended), but allow override via params.
    let blur_w = cpu_num_u32_min_1(
        &prepared.scene,
        nodes_by_id,
        layer_node,
        "width",
        src_resolution[0],
    )?;
    let blur_h = cpu_num_u32_min_1(
        &prepared.scene,
        nodes_by_id,
        layer_node,
        "height",
        src_resolution[1],
    )?;

    // sigma from radius
    let sigma = radius_px / 3.525_494;
    let (mip_level, sigma_p) = gaussian_mip_level_and_sigma_p(sigma);
    let downsample_factor: u32 = 1 << mip_level;
    let (kernel, offset, num) = gaussian_kernel_8(sigma_p.max(1e-6));
    let tap_count = num.clamp(1, 8);
    let is_sampled_output = bs.sampled_pass_ids.contains(layer_id);
    let skip_factor1_downsample = should_skip_blur_downsample_pass(downsample_factor);
    let skip_factor1_upsample = should_skip_blur_upsample_pass(
        downsample_factor,
        extend_enabled,
        is_sampled_output,
    );
    let emit_upsample_pass = !skip_factor1_upsample;

    let downsample_steps: Vec<u32> = if skip_factor1_downsample {
        Vec::new()
    } else {
        blur_downsample_steps_for_factor(downsample_factor)?
    };

    // Allocate textures for each downsample step.
    let mut step_textures: Vec<(u32, ResourceName, u32, u32, ResourceName)> = Vec::new();
    let mut cur_w: u32 = blur_w;
    let mut cur_h: u32 = blur_h;
    for step in &downsample_steps {
        let shift = match *step {
            1 => 0,
            2 => 1,
            4 => 2,
            8 => 3,
            other => bail!("GuassianBlurPass: unsupported downsample factor {other}"),
        };
        let next_w = clamp_min_1(cur_w >> shift);
        let next_h = clamp_min_1(cur_h >> shift);
        let tex: ResourceName = format!("sys.blur.{layer_id}.ds.{step}").into();
        bs.textures.push(TextureDecl {
            name: tex.clone(),
            size: [next_w, next_h],
            format: sampled_pass_format,
            sample_count: 1,
            needs_sampling: false,
        });
        let geo: ResourceName = format!("sys.blur.{layer_id}.ds.{step}.geo").into();
        bs.geometry_buffers.push((
            geo.clone(),
            make_fullscreen_geometry(next_w as f32, next_h as f32),
        ));
        step_textures.push((*step, tex, next_w, next_h, geo));
        cur_w = next_w;
        cur_h = next_h;
    }

    let ds_w = cur_w;
    let ds_h = cur_h;

    let h_tex: ResourceName = format!("sys.blur.{layer_id}.h").into();
    let v_tex: ResourceName = format!("sys.blur.{layer_id}.v").into();

    bs.textures.push(TextureDecl {
        name: h_tex.clone(),
        size: [ds_w, ds_h],
        format: sampled_pass_format,
        sample_count: 1,
        needs_sampling: false,
    });
    bs.textures.push(TextureDecl {
        name: v_tex.clone(),
        size: [ds_w, ds_h],
        format: sampled_pass_format,
        sample_count: 1,
        needs_sampling: false,
    });

    // Output texture: sampled downstream → intermediate; else → composite target.
    let output_tex: ResourceName = if is_sampled_output {
        if emit_upsample_pass {
            let out_tex: ResourceName = format!("sys.blur.{layer_id}.out").into();
            bs.textures.push(TextureDecl {
                name: out_tex.clone(),
                size: [blur_w, blur_h],
                format: sampled_pass_format,
                sample_count: 1,
                needs_sampling: false,
            });
            out_tex
        } else {
            // Factor=1 sampled blur can directly publish v_tex.
            v_tex.clone()
        }
    } else {
        target_texture_name.clone()
    };
    let upsample_geo_size: Option<[f32; 2]> = if emit_upsample_pass {
        Some(if extend_pad_px > 0 && output_tex == target_texture_name {
            gaussian_blur_extend_upsample_geo_size(
                src_content_resolution,
                [blur_w, blur_h],
            )
        } else {
            [blur_w as f32, blur_h as f32]
        })
    } else {
        None
    };

    let pass_blend_state =
        crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
            .with_context(|| {
                format!(
                    "invalid blend params for {}",
                    crate::dsl::node_display_label_with_id(layer_node)
                )
            })?;
    let blur_output_blend_state: BlendState = if output_tex == target_texture_name {
        pass_blend_state
    } else {
        BlendState::REPLACE
    };

    // Fullscreen geometry buffers for blur + upsample.
    let geo_ds: ResourceName = format!("sys.blur.{layer_id}.ds.geo").into();
    bs.geometry_buffers.push((
        geo_ds.clone(),
        make_fullscreen_geometry(ds_w as f32, ds_h as f32),
    ));
    let geo_out: Option<ResourceName> =
        if let Some(upsample_geo_size) = upsample_geo_size {
            let geo_out: ResourceName = format!("sys.blur.{layer_id}.out.geo").into();
            bs.geometry_buffers.push((
                geo_out.clone(),
                make_fullscreen_geometry(upsample_geo_size[0], upsample_geo_size[1]),
            ));
            Some(geo_out)
        } else {
            None
        };

    // Downsample chain
    let mut prev_tex: Option<ResourceName> = None;
    for (step, tex, step_w, step_h, step_geo) in &step_textures {
        let params_name: ResourceName =
            format!("params.sys.blur.{layer_id}.ds.{step}").into();
        let bundle = build_downsample_bundle(*step)?;

        let sampler_kind = if prev_tex.is_none() {
            initial_blur_source_sampler_kind.unwrap_or(SamplerKind::LinearMirror)
        } else {
            SamplerKind::LinearMirror
        };

        let step_w_f = *step_w as f32;
        let step_h_f = *step_h as f32;
        let params_val = make_params(
            [step_w_f, step_h_f],
            [step_w_f, step_h_f],
            [step_w_f * 0.5, step_h_f * 0.5],
            resolve_chain_camera_for_first_pass(
                &mut blur_chain_first_camera_consumed,
                &prepared.scene,
                nodes_by_id,
                layer_node,
                [step_w_f, step_h_f],
            )?,
            [0.0, 0.0, 0.0, 0.0],
        );

        let (src_tex, src_image_node_id) = match &prev_tex {
            None => (
                initial_blur_source_texture.clone(),
                initial_blur_source_image_node_id.clone(),
            ),
            Some(t) => (t.clone(), None),
        };

        let baked_buf: ResourceName =
            format!("sys.pass.{layer_id}.baked_data_parse").into();
        bs.baked_data_parse_buffer_to_pass_id
            .entry(baked_buf.clone())
            .or_insert_with(|| layer_id.to_string());

        let pass_name: ResourceName =
            format!("sys.blur.{layer_id}.ds.{step}.pass").into();
        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: pass_name.as_str().to_string(),
            name: pass_name.clone(),
            geometry_buffer: step_geo.clone(),
            instance_buffer: None,
            normals_buffer: None,
            target_texture: tex.clone(),
            resolve_target: None,
            params_buffer: params_name,
            baked_data_parse_buffer: Some(baked_buf),
            params: params_val,
            graph_binding: None,
            graph_values: None,
            shader_wgsl: bundle.module,
            texture_bindings: vec![PassTextureBinding {
                texture: src_tex,
                image_node_id: src_image_node_id,
            }],
            sampler_kinds: vec![sampler_kind],
            blend_state: BlendState::REPLACE,
            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
            sample_count: 1,
        });
        bs.composite_passes.push(pass_name);
        prev_tex = Some(tex.clone());
    }

    let (ds_src_tex, ds_src_image_node_id): (ResourceName, Option<String>) =
        if let Some(prev_tex) = prev_tex {
            (prev_tex, None)
        } else {
            (
                initial_blur_source_texture.clone(),
                initial_blur_source_image_node_id.clone(),
            )
        };

    // 2) Horizontal blur: ds_src_tex -> h_tex
    let params_h: ResourceName =
        format!("params.sys.blur.{layer_id}.h.ds{downsample_factor}").into();
    let bundle_h =
        build_horizontal_blur_bundle_with_tap_count(kernel, offset, tap_count);
    let ds_w_f = ds_w as f32;
    let ds_h_f = ds_h as f32;
    let params_h_val = make_params(
        [ds_w_f, ds_h_f],
        [ds_w_f, ds_h_f],
        [ds_w_f * 0.5, ds_h_f * 0.5],
        resolve_chain_camera_for_first_pass(
            &mut blur_chain_first_camera_consumed,
            &prepared.scene,
            nodes_by_id,
            layer_node,
            [ds_w_f, ds_h_f],
        )?,
        [0.0, 0.0, 0.0, 0.0],
    );

    let pass_name_h: ResourceName =
        format!("sys.blur.{layer_id}.h.ds{downsample_factor}.pass").into();
    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: pass_name_h.as_str().to_string(),
        name: pass_name_h.clone(),
        geometry_buffer: geo_ds.clone(),
        instance_buffer: None,
        normals_buffer: None,
        target_texture: h_tex.clone(),
        resolve_target: None,
        params_buffer: params_h.clone(),
        baked_data_parse_buffer: None,
        params: params_h_val,
        graph_binding: None,
        graph_values: None,
        shader_wgsl: bundle_h.module,
        texture_bindings: vec![PassTextureBinding {
            texture: ds_src_tex.clone(),
            image_node_id: ds_src_image_node_id,
        }],
        sampler_kinds: vec![SamplerKind::LinearMirror],
        blend_state: BlendState::REPLACE,
        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
        sample_count: 1,
    });
    bs.composite_passes.push(pass_name_h);

    // 3) Vertical blur: h_tex -> v_tex
    let params_v: ResourceName =
        format!("params.sys.blur.{layer_id}.v.ds{downsample_factor}").into();
    let bundle_v = build_vertical_blur_bundle_with_tap_count(kernel, offset, tap_count);
    let pass_name_v: ResourceName =
        format!("sys.blur.{layer_id}.v.ds{downsample_factor}.pass").into();
    let params_v_val = make_params(
        [ds_w_f, ds_h_f],
        [ds_w_f, ds_h_f],
        [ds_w_f * 0.5, ds_h_f * 0.5],
        resolve_chain_camera_for_first_pass(
            &mut blur_chain_first_camera_consumed,
            &prepared.scene,
            nodes_by_id,
            layer_node,
            [ds_w_f, ds_h_f],
        )?,
        [0.0, 0.0, 0.0, 0.0],
    );
    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: pass_name_v.as_str().to_string(),
        name: pass_name_v.clone(),
        geometry_buffer: geo_ds.clone(),
        instance_buffer: None,
        normals_buffer: None,
        target_texture: v_tex.clone(),
        resolve_target: None,
        params_buffer: params_v.clone(),
        baked_data_parse_buffer: None,
        params: params_v_val,
        graph_binding: None,
        graph_values: None,
        shader_wgsl: bundle_v.module,
        texture_bindings: vec![PassTextureBinding {
            texture: h_tex.clone(),
            image_node_id: None,
        }],
        sampler_kinds: vec![SamplerKind::LinearMirror],
        blend_state: BlendState::REPLACE,
        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
        sample_count: 1,
    });

    bs.composite_passes.push(pass_name_v);

    // 4) Upsample bilinear back to output: v_tex -> output_tex
    if emit_upsample_pass {
        let upsample_geo_size = upsample_geo_size
            .ok_or_else(|| anyhow!("GuassianBlurPass: missing upsample geo size"))?;
        let geo_out = geo_out
            .clone()
            .ok_or_else(|| anyhow!("GuassianBlurPass: missing upsample geometry"))?;
        let params_u: ResourceName = format!(
            "params.sys.blur.{layer_id}.upsample_bilinear.ds{downsample_factor}"
        )
        .into();
        let bundle_u = build_upsample_bilinear_bundle();
        let upsample_target_size: [f32; 2] = if output_tex == target_texture_name {
            [tgt_w, tgt_h]
        } else {
            [blur_w as f32, blur_h as f32]
        };
        let upsample_center: [f32; 2] = if output_tex == target_texture_name {
            blur_output_center
                .unwrap_or([upsample_geo_size[0] * 0.5, upsample_geo_size[1] * 0.5])
        } else {
            [upsample_geo_size[0] * 0.5, upsample_geo_size[1] * 0.5]
        };
        let params_u_val = make_params(
            upsample_target_size,
            upsample_geo_size,
            upsample_center,
            resolve_chain_camera_for_first_pass(
                &mut blur_chain_first_camera_consumed,
                &prepared.scene,
                nodes_by_id,
                layer_node,
                upsample_target_size,
            )?,
            [0.0, 0.0, 0.0, 0.0],
        );
        let pass_name_u: ResourceName =
            format!("sys.blur.{layer_id}.upsample_bilinear.ds{downsample_factor}.pass")
                .into();
        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: pass_name_u.as_str().to_string(),
            name: pass_name_u.clone(),
            geometry_buffer: geo_out,
            instance_buffer: None,
            normals_buffer: None,
            target_texture: output_tex.clone(),
            resolve_target: None,
            params_buffer: params_u.clone(),
            baked_data_parse_buffer: None,
            params: params_u_val,
            graph_binding: None,
            graph_values: None,
            shader_wgsl: bundle_u.module,
            texture_bindings: vec![PassTextureBinding {
                texture: v_tex.clone(),
                image_node_id: None,
            }],
            sampler_kinds: vec![SamplerKind::LinearMirror],
            blend_state: blur_output_blend_state,
            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
            sample_count: 1,
        });

        bs.composite_passes.push(pass_name_u);
    }

    // Register this GuassianBlurPass output for potential downstream chaining.
    let blur_output_tex = output_tex.clone();
    bs.pass_output_registry.register(PassOutputSpec {
        node_id: layer_id.to_string(),
        texture_name: blur_output_tex.clone(),
        resolution: [blur_w, blur_h],
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
        if blur_output_tex == comp_ctx.target_texture_name {
            continue;
        }

        let comp_w = comp_ctx.target_size_px[0];
        let comp_h = comp_ctx.target_size_px[1];
        let compose_geo: ResourceName =
            format!("sys.blur.{layer_id}.to.{composition_id}.compose.geo").into();
        bs.geometry_buffers.push((
            compose_geo.clone(),
            make_fullscreen_geometry(blur_w as f32, blur_h as f32),
        ));
        let compose_pass_name: ResourceName =
            format!("sys.blur.{layer_id}.to.{composition_id}.compose.pass").into();
        let compose_params_name: ResourceName =
            format!("params.sys.blur.{layer_id}.to.{composition_id}.compose").into();
        let compose_params = make_params(
            [comp_w, comp_h],
            [blur_w as f32, blur_h as f32],
            blur_output_center.unwrap_or([comp_w * 0.5, comp_h * 0.5]),
            resolve_chain_camera_for_first_pass(
                &mut blur_chain_first_camera_consumed,
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
                texture: blur_output_tex.clone(),
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
