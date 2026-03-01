//! Bloom pass assembler.
//!
//! Handles the `"BloomNode"` node type. Applies bloom effect by extracting bright
//! areas, downsampling through a MIP chain, applying Gaussian blur at each level,
//! and additively combining back up to the original resolution.

use anyhow::{Context, Result, anyhow};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color},
};

use crate::{
    dsl::{incoming_connection, Node},
    renderer::{
        camera::resolve_effective_camera_for_pass_node,
        types::{Kernel2D, PassOutputSpec},
        utils::cpu_num_f32,
        wgsl::{
            build_downsample_pass_wgsl_bundle, build_fullscreen_textured_bundle,
            build_horizontal_blur_bundle_with_tap_count, build_upsample_bilinear_bundle,
            build_vertical_blur_bundle_with_tap_count, clamp_min_1,
            gaussian_kernel_8, gaussian_mip_level_and_sigma_p,
        },
        wgsl_bloom::{build_bloom_additive_combine_bundle, build_bloom_extract_bundle},
    },
};

use super::args::{BuilderState, SceneContext, make_fullscreen_geometry};
use super::super::pass_spec::{
    PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl,
    make_params,
};
use super::super::resource_naming::{bloom_downsample_level_count, parse_tint_from_node_or_default};
use super::super::sampler::sampler_kind_for_pass_texture;
use super::super::resource_naming::resolve_chain_camera_for_first_pass;

/// Assemble a `"BloomNode"` layer.
pub(crate) fn assemble_bloom(
    sc: &SceneContext<'_>,
    bs: &mut BuilderState<'_>,
    layer_id: &str,
    layer_node: &Node,
) -> Result<()> {
    let scene = sc.scene();
    let nodes_by_id = sc.nodes_by_id();
    let tgt_w = bs.tgt_size[0];
    let tgt_h = bs.tgt_size[1];

    let src_conn = incoming_connection(scene, layer_id, "pass")
        .ok_or_else(|| anyhow!("BloomNode.pass missing for {layer_id}"))?;
    let src_spec = bs
        .pass_output_registry
        .get_for_port(&src_conn.from.node_id, &src_conn.from.port_id)
        .ok_or_else(|| {
            anyhow!(
                "BloomNode.pass references upstream pass {}, but its output is not registered yet",
                src_conn.from.node_id
            )
        })?;

    let base_resolution = src_spec.resolution;
    let base_w = base_resolution[0].max(1) as f32;
    let base_h = base_resolution[1].max(1) as f32;

    let threshold =
        cpu_num_f32(scene, &nodes_by_id, layer_node, "threshold", 0.5)?.clamp(0.0, 1.0);
    let smoothness =
        cpu_num_f32(scene, &nodes_by_id, layer_node, "smoothness", 0.5)?.clamp(0.0, 1.0);
    let strength =
        cpu_num_f32(scene, &nodes_by_id, layer_node, "strength", 1.0)?.clamp(0.0, 1.0);
    let saturation =
        cpu_num_f32(scene, &nodes_by_id, layer_node, "saturation", 1.0)?.clamp(0.0, 1.0);
    let size =
        cpu_num_f32(scene, &nodes_by_id, layer_node, "size", 0.5)?.clamp(0.0, 1.0);
    let smooth_width_px = (1.0 - smoothness) * 40.0;
    let radius_px = size * 6.0;
    let tint = parse_tint_from_node_or_default(scene, &nodes_by_id, layer_node)?;

    let sigma = radius_px / 3.525_494;
    let (_mip_level, sigma_p) = gaussian_mip_level_and_sigma_p(sigma);
    let (kernel, offset, num) = gaussian_kernel_8(sigma_p.max(1e-6));
    let tap_count = num.clamp(1, 8);

    let is_sampled_output = bs.sampled_pass_ids.contains(layer_id);
    let pass_blend_state =
        crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
            .with_context(|| {
                format!(
                    "invalid blend params for {}",
                    crate::dsl::node_display_label_with_id(layer_node)
                )
            })?;

    let output_tex: ResourceName = if is_sampled_output {
        let out: ResourceName = format!("sys.bloom.{layer_id}.out").into();
        bs.textures.push(TextureDecl {
            name: out.clone(),
            size: base_resolution,
            format: bs.sampled_pass_format,
            sample_count: 1,
            needs_sampling: false,
        });
        out
    } else {
        bs.target_texture_name.clone()
    };

    let output_blend = if output_tex == *bs.target_texture_name {
        pass_blend_state
    } else {
        BlendState::REPLACE
    };

    let mip_levels = bloom_downsample_level_count(base_resolution);

    // ---- MIP0 (extract) ----
    let mip0_tex: ResourceName = format!("sys.bloom.{layer_id}.mip0").into();
    bs.textures.push(TextureDecl {
        name: mip0_tex.clone(),
        size: base_resolution,
        format: bs.sampled_pass_format,
        sample_count: 1,
        needs_sampling: false,
    });
    let mip0_geo: ResourceName = format!("sys.bloom.{layer_id}.mip0.geo").into();
    bs.geometry_buffers
        .push((mip0_geo.clone(), make_fullscreen_geometry(base_w, base_h)));
    let mip0_params: ResourceName = format!("params.sys.bloom.{layer_id}.mip0").into();
    let mut bloom_chain_first_camera_consumed = false;
    let mip0_params_val = make_params(
        [base_w, base_h],
        [base_w, base_h],
        [base_w * 0.5, base_h * 0.5],
        resolve_chain_camera_for_first_pass(
            &mut bloom_chain_first_camera_consumed,
            scene,
            &nodes_by_id,
            layer_node,
            [base_w, base_h],
        )?,
        [0.0, 0.0, 0.0, 0.0],
    );

    let extract_bundle =
        build_bloom_extract_bundle(threshold, smooth_width_px, strength, saturation, tint);
    let extract_pass_name: ResourceName =
        format!("sys.bloom.{layer_id}.extract.pass").into();
    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: extract_pass_name.as_str().to_string(),
        name: extract_pass_name.clone(),
        geometry_buffer: mip0_geo,
        instance_buffer: None,
        normals_buffer: None,
        target_texture: mip0_tex.clone(),
        resolve_target: None,
        params_buffer: mip0_params,
        baked_data_parse_buffer: None,
        params: mip0_params_val,
        graph_binding: None,
        graph_values: None,
        shader_wgsl: extract_bundle.module,
        texture_bindings: vec![PassTextureBinding {
            texture: src_spec.texture_name.clone(),
            image_node_id: None,
        }],
        sampler_kinds: vec![sampler_kind_for_pass_texture(scene, &src_conn.from.node_id)],
        blend_state: BlendState::REPLACE,
        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
        sample_count: 1,
    });
    bs.composite_passes.push(extract_pass_name);

    // ---- MIP downsample chain ----
    let mut mip_textures: Vec<ResourceName> = vec![mip0_tex.clone()];
    let mut mip_sizes: Vec<[u32; 2]> = vec![base_resolution];

    let mut prev_tex = mip0_tex.clone();
    let mut cur_size = base_resolution;
    for level in 1..=mip_levels {
        cur_size = [clamp_min_1(cur_size[0] / 2), clamp_min_1(cur_size[1] / 2)];
        let mip_tex: ResourceName = format!("sys.bloom.{layer_id}.mip{level}").into();
        bs.textures.push(TextureDecl {
            name: mip_tex.clone(),
            size: cur_size,
            format: bs.sampled_pass_format,
            sample_count: 1,
            needs_sampling: false,
        });
        let mip_geo: ResourceName = format!("sys.bloom.{layer_id}.mip{level}.geo").into();
        let mip_w = cur_size[0] as f32;
        let mip_h = cur_size[1] as f32;
        bs.geometry_buffers
            .push((mip_geo.clone(), make_fullscreen_geometry(mip_w, mip_h)));
        let mip_params: ResourceName =
            format!("params.sys.bloom.{layer_id}.mip{level}").into();
        let mip_params_val = make_params(
            [mip_w, mip_h],
            [mip_w, mip_h],
            [mip_w * 0.5, mip_h * 0.5],
            resolve_effective_camera_for_pass_node(scene, &nodes_by_id, layer_node, [mip_w, mip_h])?,
            [0.0, 0.0, 0.0, 0.0],
        );

        let bloom_crossbox_kernel = Kernel2D {
            width: 3,
            height: 3,
            values: vec![0.0, 0.25, 0.0, 0.25, 0.0, 0.25, 0.0, 0.25, 0.0],
        };
        let ds_bundle = build_downsample_pass_wgsl_bundle(&bloom_crossbox_kernel)?;
        let ds_pass_name: ResourceName =
            format!("sys.bloom.{layer_id}.mip{level}.down.pass").into();
        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: ds_pass_name.as_str().to_string(),
            name: ds_pass_name.clone(),
            geometry_buffer: mip_geo,
            instance_buffer: None,
            normals_buffer: None,
            target_texture: mip_tex.clone(),
            resolve_target: None,
            params_buffer: mip_params,
            baked_data_parse_buffer: None,
            params: mip_params_val,
            graph_binding: None,
            graph_values: None,
            shader_wgsl: ds_bundle.module,
            texture_bindings: vec![PassTextureBinding {
                texture: prev_tex.clone(),
                image_node_id: None,
            }],
            sampler_kinds: vec![SamplerKind::LinearClamp],
            blend_state: BlendState::REPLACE,
            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
            sample_count: 1,
        });
        bs.composite_passes.push(ds_pass_name);
        prev_tex = mip_tex.clone();
        mip_textures.push(mip_tex);
        mip_sizes.push(cur_size);
    }

    // ---- Blur + upsample (or blur-only for mip_levels == 0) ----
    let bloom_output_tex: ResourceName = if mip_levels == 0 {
        let h_tex: ResourceName = format!("sys.bloom.{layer_id}.mip0.h").into();
        let v_tex: ResourceName = format!("sys.bloom.{layer_id}.mip0.v").into();
        bs.textures.push(TextureDecl {
            name: h_tex.clone(),
            size: base_resolution,
            format: bs.sampled_pass_format,
            sample_count: 1,
            needs_sampling: false,
        });
        bs.textures.push(TextureDecl {
            name: v_tex.clone(),
            size: base_resolution,
            format: bs.sampled_pass_format,
            sample_count: 1,
            needs_sampling: false,
        });
        let geo: ResourceName = format!("sys.bloom.{layer_id}.mip0.blur.geo").into();
        bs.geometry_buffers
            .push((geo.clone(), make_fullscreen_geometry(base_w, base_h)));

        let params_h: ResourceName = format!("params.sys.bloom.{layer_id}.mip0.h").into();
        let params_v: ResourceName = format!("params.sys.bloom.{layer_id}.mip0.v").into();
        let params_blur = make_params(
            [base_w, base_h],
            [base_w, base_h],
            [base_w * 0.5, base_h * 0.5],
            resolve_effective_camera_for_pass_node(
                scene,
                &nodes_by_id,
                layer_node,
                [base_w, base_h],
            )?,
            [0.0, 0.0, 0.0, 0.0],
        );

        let h_bundle =
            build_horizontal_blur_bundle_with_tap_count(kernel, offset, tap_count);
        let v_bundle =
            build_vertical_blur_bundle_with_tap_count(kernel, offset, tap_count);
        let h_pass_name: ResourceName =
            format!("sys.bloom.{layer_id}.mip0.h.pass").into();
        let v_pass_name: ResourceName =
            format!("sys.bloom.{layer_id}.mip0.v.pass").into();
        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: h_pass_name.as_str().to_string(),
            name: h_pass_name.clone(),
            geometry_buffer: geo.clone(),
            instance_buffer: None,
            normals_buffer: None,
            target_texture: h_tex.clone(),
            resolve_target: None,
            params_buffer: params_h,
            baked_data_parse_buffer: None,
            params: params_blur,
            graph_binding: None,
            graph_values: None,
            shader_wgsl: h_bundle.module,
            texture_bindings: vec![PassTextureBinding {
                texture: mip0_tex.clone(),
                image_node_id: None,
            }],
            sampler_kinds: vec![SamplerKind::LinearMirror],
            blend_state: BlendState::REPLACE,
            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
            sample_count: 1,
        });
        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: v_pass_name.as_str().to_string(),
            name: v_pass_name.clone(),
            geometry_buffer: geo,
            instance_buffer: None,
            normals_buffer: None,
            target_texture: v_tex.clone(),
            resolve_target: None,
            params_buffer: params_v,
            baked_data_parse_buffer: None,
            params: params_blur,
            graph_binding: None,
            graph_values: None,
            shader_wgsl: v_bundle.module,
            texture_bindings: vec![PassTextureBinding {
                texture: h_tex,
                image_node_id: None,
            }],
            sampler_kinds: vec![SamplerKind::LinearMirror],
            blend_state: BlendState::REPLACE,
            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
            sample_count: 1,
        });
        bs.composite_passes.push(h_pass_name);
        bs.composite_passes.push(v_pass_name);
        v_tex
    } else {
        let add_bundle = build_bloom_additive_combine_bundle();
        let mut current_tex = mip_textures[mip_levels as usize].clone();

        for level in (1..=mip_levels).rev() {
            let src_size = mip_sizes[level as usize];
            let dst_size = mip_sizes[(level - 1) as usize];

            let h_tex: ResourceName =
                format!("sys.bloom.{layer_id}.lvl{level}.h").into();
            let v_tex: ResourceName =
                format!("sys.bloom.{layer_id}.lvl{level}.v").into();
            bs.textures.push(TextureDecl {
                name: h_tex.clone(),
                size: src_size,
                format: bs.sampled_pass_format,
                sample_count: 1,
                needs_sampling: false,
            });
            bs.textures.push(TextureDecl {
                name: v_tex.clone(),
                size: src_size,
                format: bs.sampled_pass_format,
                sample_count: 1,
                needs_sampling: false,
            });

            let src_w = src_size[0] as f32;
            let src_h = src_size[1] as f32;
            let geo_blur: ResourceName =
                format!("sys.bloom.{layer_id}.lvl{level}.blur.geo").into();
            bs.geometry_buffers
                .push((geo_blur.clone(), make_fullscreen_geometry(src_w, src_h)));

            let params_blur_name: ResourceName =
                format!("params.sys.bloom.{layer_id}.lvl{level}.blur").into();
            let params_blur = make_params(
                [src_w, src_h],
                [src_w, src_h],
                [src_w * 0.5, src_h * 0.5],
                resolve_effective_camera_for_pass_node(
                    scene,
                    &nodes_by_id,
                    layer_node,
                    [src_w, src_h],
                )?,
                [0.0, 0.0, 0.0, 0.0],
            );

            let h_bundle =
                build_horizontal_blur_bundle_with_tap_count(kernel, offset, tap_count);
            let v_bundle =
                build_vertical_blur_bundle_with_tap_count(kernel, offset, tap_count);
            let h_pass_name: ResourceName =
                format!("sys.bloom.{layer_id}.lvl{level}.h.pass").into();
            let v_pass_name: ResourceName =
                format!("sys.bloom.{layer_id}.lvl{level}.v.pass").into();
            bs.render_pass_specs.push(RenderPassSpec {
                pass_id: h_pass_name.as_str().to_string(),
                name: h_pass_name.clone(),
                geometry_buffer: geo_blur.clone(),
                instance_buffer: None,
                normals_buffer: None,
                target_texture: h_tex.clone(),
                resolve_target: None,
                params_buffer: params_blur_name.clone(),
                baked_data_parse_buffer: None,
                params: params_blur,
                graph_binding: None,
                graph_values: None,
                shader_wgsl: h_bundle.module,
                texture_bindings: vec![PassTextureBinding {
                    texture: current_tex.clone(),
                    image_node_id: None,
                }],
                sampler_kinds: vec![SamplerKind::LinearMirror],
                blend_state: BlendState::REPLACE,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                sample_count: 1,
            });
            bs.render_pass_specs.push(RenderPassSpec {
                pass_id: v_pass_name.as_str().to_string(),
                name: v_pass_name.clone(),
                geometry_buffer: geo_blur,
                instance_buffer: None,
                normals_buffer: None,
                target_texture: v_tex.clone(),
                resolve_target: None,
                params_buffer: params_blur_name,
                baked_data_parse_buffer: None,
                params: params_blur,
                graph_binding: None,
                graph_values: None,
                shader_wgsl: v_bundle.module,
                texture_bindings: vec![PassTextureBinding {
                    texture: h_tex,
                    image_node_id: None,
                }],
                sampler_kinds: vec![SamplerKind::LinearMirror],
                blend_state: BlendState::REPLACE,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                sample_count: 1,
            });
            bs.composite_passes.push(h_pass_name);
            bs.composite_passes.push(v_pass_name);

            let dst_w = dst_size[0] as f32;
            let dst_h = dst_size[1] as f32;
            let up_tex: ResourceName =
                format!("sys.bloom.{layer_id}.lvl{level}.up").into();
            bs.textures.push(TextureDecl {
                name: up_tex.clone(),
                size: dst_size,
                format: bs.sampled_pass_format,
                sample_count: 1,
                needs_sampling: false,
            });
            let up_geo: ResourceName =
                format!("sys.bloom.{layer_id}.lvl{level}.up.geo").into();
            bs.geometry_buffers
                .push((up_geo.clone(), make_fullscreen_geometry(dst_w, dst_h)));
            let up_params_name: ResourceName =
                format!("params.sys.bloom.{layer_id}.lvl{level}.up").into();
            let up_params = make_params(
                [dst_w, dst_h],
                [dst_w, dst_h],
                [dst_w * 0.5, dst_h * 0.5],
                resolve_effective_camera_for_pass_node(
                    scene,
                    &nodes_by_id,
                    layer_node,
                    [dst_w, dst_h],
                )?,
                [0.0, 0.0, 0.0, 0.0],
            );
            let up_bundle = build_upsample_bilinear_bundle();
            let up_pass_name: ResourceName =
                format!("sys.bloom.{layer_id}.lvl{level}.up.pass").into();
            bs.render_pass_specs.push(RenderPassSpec {
                pass_id: up_pass_name.as_str().to_string(),
                name: up_pass_name.clone(),
                geometry_buffer: up_geo,
                instance_buffer: None,
                normals_buffer: None,
                target_texture: up_tex.clone(),
                resolve_target: None,
                params_buffer: up_params_name,
                baked_data_parse_buffer: None,
                params: up_params,
                graph_binding: None,
                graph_values: None,
                shader_wgsl: up_bundle.module,
                texture_bindings: vec![PassTextureBinding {
                    texture: v_tex,
                    image_node_id: None,
                }],
                sampler_kinds: vec![SamplerKind::LinearClamp],
                blend_state: BlendState::REPLACE,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                sample_count: 1,
            });
            bs.composite_passes.push(up_pass_name);

            if level == 1 {
                current_tex = up_tex;
                continue;
            }

            let add_tex: ResourceName =
                format!("sys.bloom.{layer_id}.lvl{level}.add").into();
            bs.textures.push(TextureDecl {
                name: add_tex.clone(),
                size: dst_size,
                format: bs.sampled_pass_format,
                sample_count: 1,
                needs_sampling: false,
            });
            let add_geo: ResourceName =
                format!("sys.bloom.{layer_id}.lvl{level}.add.geo").into();
            bs.geometry_buffers
                .push((add_geo.clone(), make_fullscreen_geometry(dst_w, dst_h)));
            let add_params_name: ResourceName =
                format!("params.sys.bloom.{layer_id}.lvl{level}.add").into();
            let add_params = make_params(
                [dst_w, dst_h],
                [dst_w, dst_h],
                [dst_w * 0.5, dst_h * 0.5],
                resolve_effective_camera_for_pass_node(
                    scene,
                    &nodes_by_id,
                    layer_node,
                    [dst_w, dst_h],
                )?,
                [0.0, 0.0, 0.0, 0.0],
            );
            let add_pass_name: ResourceName =
                format!("sys.bloom.{layer_id}.lvl{level}.add.pass").into();
            bs.render_pass_specs.push(RenderPassSpec {
                pass_id: add_pass_name.as_str().to_string(),
                name: add_pass_name.clone(),
                geometry_buffer: add_geo,
                instance_buffer: None,
                normals_buffer: None,
                target_texture: add_tex.clone(),
                resolve_target: None,
                params_buffer: add_params_name,
                baked_data_parse_buffer: None,
                params: add_params,
                graph_binding: None,
                graph_values: None,
                shader_wgsl: add_bundle.module.clone(),
                texture_bindings: vec![
                    PassTextureBinding {
                        texture: up_tex,
                        image_node_id: None,
                    },
                    PassTextureBinding {
                        texture: mip_textures[(level - 1) as usize].clone(),
                        image_node_id: None,
                    },
                ],
                sampler_kinds: vec![SamplerKind::LinearClamp, SamplerKind::LinearClamp],
                blend_state: BlendState::REPLACE,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                sample_count: 1,
            });
            bs.composite_passes.push(add_pass_name);
            current_tex = add_tex;
        }
        current_tex
    };

    // ---- Final copy to output ----
    if bloom_output_tex != output_tex {
        let out_w = base_resolution[0] as f32;
        let out_h = base_resolution[1] as f32;
        let geo_out: ResourceName = format!("sys.bloom.{layer_id}.out.geo").into();
        bs.geometry_buffers
            .push((geo_out.clone(), make_fullscreen_geometry(out_w, out_h)));
        let params_out: ResourceName = format!("params.sys.bloom.{layer_id}.out").into();
        let params_out_val = make_params(
            if output_tex == *bs.target_texture_name {
                [tgt_w, tgt_h]
            } else {
                [out_w, out_h]
            },
            [out_w, out_h],
            [out_w * 0.5, out_h * 0.5],
            resolve_effective_camera_for_pass_node(
                scene,
                &nodes_by_id,
                layer_node,
                if output_tex == *bs.target_texture_name {
                    [tgt_w, tgt_h]
                } else {
                    [out_w, out_h]
                },
            )?,
            [0.0, 0.0, 0.0, 0.0],
        );
        let copy_pass_name: ResourceName =
            format!("sys.bloom.{layer_id}.out.pass").into();
        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: copy_pass_name.as_str().to_string(),
            name: copy_pass_name.clone(),
            geometry_buffer: geo_out,
            instance_buffer: None,
            normals_buffer: None,
            target_texture: output_tex.clone(),
            resolve_target: None,
            params_buffer: params_out,
            baked_data_parse_buffer: None,
            params: params_out_val,
            graph_binding: None,
            graph_values: None,
            shader_wgsl: build_fullscreen_textured_bundle(
                "return textureSample(src_tex, src_samp, in.uv);".to_string(),
            )
            .module,
            texture_bindings: vec![PassTextureBinding {
                texture: bloom_output_tex.clone(),
                image_node_id: None,
            }],
            sampler_kinds: vec![SamplerKind::LinearClamp],
            blend_state: output_blend,
            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
            sample_count: 1,
        });
        bs.composite_passes.push(copy_pass_name);
    }

    bs.pass_output_registry.register(PassOutputSpec {
        node_id: layer_id.to_string(),
        texture_name: output_tex.clone(),
        resolution: base_resolution,
        format: if is_sampled_output {
            bs.sampled_pass_format
        } else {
            bs.target_format
        },
    });

    Ok(())
}
