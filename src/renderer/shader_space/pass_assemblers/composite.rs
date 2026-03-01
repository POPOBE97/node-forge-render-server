//! Composite pass assembler.
//!
//! Handles the `"Composite"` node type: registers the composition target in the
//! pass-output registry and synthesises implicit Composite-to-Composite blits for
//! downstream consumers.

use anyhow::{Context, Result, bail};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, Color},
};

use crate::{
    dsl::{find_node, parse_texture_format},
    renderer::{
        camera::legacy_projection_camera_matrix,
        types::PassOutputSpec,
        wgsl::build_fullscreen_textured_bundle,
    },
};

use super::args::{BuilderState, SceneContext, make_fullscreen_geometry};
use super::super::pass_spec::{
    PassTextureBinding, RenderPassSpec, SamplerKind,
    make_params,
};

/// Assemble a `"Composite"` layer.
///
/// Registers the composition target in `pass_output_registry`, then builds
/// implicit Composite-to-downstream-Composite fullscreen blits.
pub(crate) fn assemble_composite(
    sc: &SceneContext<'_>,
    bs: &mut BuilderState<'_>,
    layer_id: &str,
    layer_node: &crate::dsl::Node,
) -> Result<()> {
    let nodes_by_id = sc.nodes_by_id();

    let Some(comp_ctx) = sc.composition_contexts.get(layer_id) else {
        bail!("missing resolved Composition context for '{layer_id}'");
    };

    let comp_target_node = find_node(nodes_by_id, &comp_ctx.target_texture_node_id)?;
    let comp_target_format = parse_texture_format(&comp_target_node.params)?;

    let pass_blend_state =
        crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
            .with_context(|| {
                format!(
                    "invalid blend params for {}",
                    crate::dsl::node_display_label_with_id(layer_node)
                )
            })?;

    bs.pass_output_registry.register(PassOutputSpec {
        node_id: layer_id.to_string(),
        texture_name: comp_ctx.target_texture_name.clone(),
        resolution: [
            comp_ctx.target_size_px[0].max(1.0).round() as u32,
            comp_ctx.target_size_px[1].max(1.0).round() as u32,
        ],
        format: comp_target_format,
    });

    // Implicit Composition -> Composition fullscreen blit.
    let consumers = sc.composition_consumers_by_source
        .get(layer_id)
        .cloned()
        .unwrap_or_default();

    if !consumers.is_empty() {
        let compose_blend_state = pass_blend_state;
        for downstream_comp_id in consumers {
            let Some(dst_ctx) = sc.composition_contexts.get(&downstream_comp_id) else {
                continue;
            };
            // Skip self-referencing composition node.
            if dst_ctx.composition_node_id == comp_ctx.composition_node_id {
                continue;
            }
            // Skip when both compositions share the same target texture.
            if dst_ctx.target_texture_name == comp_ctx.target_texture_name {
                continue;
            }

            let dst_w = dst_ctx.target_size_px[0];
            let dst_h = dst_ctx.target_size_px[1];

            let geo: ResourceName =
                format!("sys.comp.{layer_id}.to.{downstream_comp_id}.compose.geo").into();
            bs.geometry_buffers.push((geo.clone(), make_fullscreen_geometry(dst_w, dst_h)));

            let pass_name: ResourceName =
                format!("sys.comp.{layer_id}.to.{downstream_comp_id}.compose.pass").into();
            let params_name: ResourceName =
                format!("params.sys.comp.{layer_id}.to.{downstream_comp_id}.compose").into();
            let params = make_params(
                [dst_w, dst_h],
                [dst_w, dst_h],
                [dst_w * 0.5, dst_h * 0.5],
                legacy_projection_camera_matrix([dst_w, dst_h]),
                [0.0, 0.0, 0.0, 0.0],
            );

            bs.render_pass_specs.push(RenderPassSpec {
                pass_id: pass_name.as_str().to_string(),
                name: pass_name.clone(),
                geometry_buffer: geo,
                instance_buffer: None,
                normals_buffer: None,
                target_texture: dst_ctx.target_texture_name.clone(),
                resolve_target: None,
                params_buffer: params_name,
                baked_data_parse_buffer: None,
                params,
                graph_binding: None,
                graph_values: None,
                shader_wgsl: build_fullscreen_textured_bundle(
                    "return textureSample(src_tex, src_samp, in.uv);".to_string(),
                )
                .module,
                texture_bindings: vec![PassTextureBinding {
                    texture: comp_ctx.target_texture_name.clone(),
                    image_node_id: None,
                }],
                sampler_kinds: vec![SamplerKind::LinearClamp],
                blend_state: compose_blend_state,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                sample_count: 1,
            });

            bs.composite_passes.push(pass_name);
        }
    }

    Ok(())
}
