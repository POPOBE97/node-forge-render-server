//! Upsample pass assembler.
//!
//! Handles the `"Upsample"` node type. Upsamples source pass output into a
//! `targetSize`-sized texture using bilinear filtering. Optionally adds a fit
//! pass to scale to the Composite target size.

use anyhow::{Context, Result, bail};
use rust_wgpu_fiber::{
    ResourceName,
    wgpu::{self, BlendState, Color},
};

use crate::{
    dsl::{Node, parse_str},
    renderer::{
        camera::{legacy_projection_camera_matrix, resolve_effective_camera_for_pass_node},
        pass_source::processing_input_connection,
        wgsl::{build_fullscreen_textured_bundle, build_upsample_bilinear_bundle},
    },
};

use super::super::pass_spec::{
    PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl, make_params,
};
use super::args::{BuilderState, SceneContext};

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

    let (src_conn, _) = processing_input_connection(scene, layer_id)?;
    let src = super::super::resource_naming::resolve_sampled_source(
        scene,
        nodes_by_id,
        sc.ids(),
        bs.pass_output_registry,
        &src_conn.from,
        sc.asset_store,
    )?;
    let src_binding = src.binding.clone();

    let [out_w, out_h] = crate::dsl::processing_target_size(scene, &nodes_by_id, layer_node)?;

    let materializes_texture = bs.materialized_texture_output_ids.contains(layer_id);
    let needs_intermediate = materializes_texture || (out_w != tgt_w_u || out_h != tgt_h_u);
    let writes_scene_output_target = !materializes_texture;

    let upsample_out_tex: ResourceName = if needs_intermediate {
        let tex: ResourceName = format!("sys.upsample.{layer_id}.out").into();
        bs.textures.push(TextureDecl {
            name: tex.clone(),
            size: [out_w, out_h],
            format: if materializes_texture {
                bs.sampled_pass_format
            } else {
                bs.target_format
            },
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
        resolve_effective_camera_for_pass_node(
            scene,
            &nodes_by_id,
            layer_node,
            [out_w_f, out_h_f],
        )?,
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
    let node_sampler_kind = match (nearest, address) {
        (true, "mirror") => SamplerKind::NearestMirror,
        (true, "repeat") => SamplerKind::NearestRepeat,
        (true, _) => SamplerKind::NearestClamp,
        (false, "mirror") => SamplerKind::LinearMirror,
        (false, "repeat") => SamplerKind::LinearRepeat,
        (false, _) => SamplerKind::LinearClamp,
    };
    let sampler_kind = if src
        .surface_ref
        .as_ref()
        .is_some_and(|value| value.sampler_node_id.is_some())
        || src.binding.image_node_id.is_some()
    {
        src.sampler
    } else {
        node_sampler_kind
    };

    let bundle = build_upsample_bilinear_bundle();

    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: pass_name.as_str().to_string(),
        name: pass_name.clone(),
        geometry_buffer: geo,
        instance_buffer: None,
        normals_buffer: None,
        vertex_layout: Default::default(),
        target_texture: upsample_out_tex.clone(),
        resolve_target: None,
        params_buffer: params_name,
        baked_data_parse_buffer: None,
        params: params_val,
        graph_binding: None,
        graph_values: None,
        shader_wgsl: bundle.module,
        texture_bindings: vec![src_binding],
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
    if !materializes_texture && upsample_out_tex != *bs.target_texture_name {
        let fit_pass_name: ResourceName = format!("sys.upsample.{layer_id}.fit.pass").into();
        let fit_geo: ResourceName = format!("sys.upsample.{layer_id}.fit.geo").into();
        bs.push_fullscreen_geometry(fit_geo.clone(), tgt_w, tgt_h);

        let fit_params_name: ResourceName = format!("params.sys.upsample.{layer_id}.fit").into();
        let fit_params = make_params(
            [tgt_w, tgt_h],
            [tgt_w, tgt_h],
            [tgt_w * 0.5, tgt_h * 0.5],
            resolve_effective_camera_for_pass_node(
                scene,
                &nodes_by_id,
                layer_node,
                [tgt_w, tgt_h],
            )?,
            [0.0, 0.0, 0.0, 0.0],
        );
        let fit_bundle = build_upsample_bilinear_bundle();

        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: fit_pass_name.as_str().to_string(),
            name: fit_pass_name.clone(),
            geometry_buffer: fit_geo,
            instance_buffer: None,
            normals_buffer: None,
            vertex_layout: Default::default(),
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
    bs.pass_output_registry.register_ports(
        layer_id,
        &["texture"],
        if materializes_texture {
            upsample_output_tex.clone()
        } else {
            bs.target_texture_name.clone()
        },
        if materializes_texture {
            [out_w, out_h]
        } else {
            [tgt_w_u, tgt_h_u]
        },
        if materializes_texture {
            bs.sampled_pass_format
        } else {
            bs.target_format
        },
    );

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
            if writes_scene_output_target && comp_ctx.target_texture_name == *bs.target_texture_name
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
                vertex_layout: Default::default(),
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
