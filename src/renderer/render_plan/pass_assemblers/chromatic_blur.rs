//! Private color stages for the integrated ChromaticBlur filter.
use anyhow::{Result, anyhow};
use rust_wgpu_fiber::{
    ResourceName,
    wgpu::{self, BlendState, Color},
};

use super::super::{
    pass_spec::{PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl, make_params},
    resource_naming::{resolve_chain_camera_for_first_pass, resolve_pass_texture_bindings},
};
use super::args::{BuilderState, SceneContext, make_fullscreen_geometry};
use crate::{
    dsl::Node,
    renderer::{
        graph_uniforms::{choose_graph_binding_kind, pack_graph_values},
        shader_space::sampler::{sampler_kind_for_pass_texture, sampler_kind_from_node_params},
        types::GraphBinding,
        wgsl_chromatic_blur::{ChromaticStage, build_chromatic_stage_bundle},
    },
};

pub(crate) fn assemble_color_stages(
    sc: &SceneContext<'_>,
    bs: &mut BuilderState<'_>,
    node: &Node,
    source: ResourceName,
    image_node_id: Option<String>,
    source_sampler: SamplerKind,
    size: [u32; 2],
    camera_consumed: &mut bool,
) -> Result<ResourceName> {
    let layer_id = &node.id;
    let mut source_id = format!("sys.cb.{layer_id}.input");
    bs.pass_output_registry.register_ports(
        &source_id,
        &["pass"],
        source.clone(),
        size,
        wgpu::TextureFormat::Rgba16Float,
    );
    let mut result = source;
    for stage in [ChromaticStage::Radial, ChromaticStage::Rgb] {
        let name = format!("sys.cb.{layer_id}.{}", stage.name());
        let output: ResourceName = name.clone().into();
        let mut bundle = build_chromatic_stage_bundle(
            sc.scene(),
            sc.nodes_by_id(),
            layer_id,
            &source_id,
            stage,
            None,
        )?;
        let mut graph_binding = None;
        let mut graph_values = None;
        if let Some(schema) = &bundle.graph_schema {
            let limits = sc.device.limits();
            let kind = choose_graph_binding_kind(
                schema.size_bytes,
                limits.max_uniform_buffer_binding_size as u64,
                limits.max_storage_buffer_binding_size as u64,
            )?;
            if bundle.graph_binding_kind != Some(kind) {
                bundle = build_chromatic_stage_bundle(
                    sc.scene(),
                    sc.nodes_by_id(),
                    layer_id,
                    &source_id,
                    stage,
                    Some(kind),
                )?;
            }
            let schema = bundle
                .graph_schema
                .clone()
                .ok_or_else(|| anyhow!("missing chromatic graph schema"))?;
            graph_values = Some(pack_graph_values(sc.scene(), &schema)?);
            graph_binding = Some(GraphBinding {
                buffer_name: format!("params.{name}.graph").into(),
                kind,
                schema,
            });
        }
        let mut texture_bindings = Vec::new();
        let mut sampler_kinds = Vec::new();
        for texture_ref in &bundle.image_textures {
            let texture = sc
                .ids()
                .get(&texture_ref.image_node_id)
                .ok_or_else(|| anyhow!("missing chromatic image {}", texture_ref.image_node_id))?;
            texture_bindings.push(PassTextureBinding {
                texture: texture.clone(),
                image_node_id: Some(texture_ref.image_node_id.clone()),
            });
            let sampler_id = texture_ref
                .sampler_node_id
                .as_deref()
                .unwrap_or(&texture_ref.image_node_id);
            sampler_kinds.push(
                sc.nodes_by_id()
                    .get(sampler_id)
                    .map(|node| sampler_kind_from_node_params(&node.params))
                    .unwrap_or(SamplerKind::LinearClamp),
            );
        }
        let pass_bindings =
            resolve_pass_texture_bindings(bs.pass_output_registry, &bundle.pass_textures)?;
        for (texture_ref, mut binding) in bundle.pass_textures.iter().zip(pass_bindings) {
            let sampler = if texture_ref.source.node_id == source_id {
                if matches!(stage, ChromaticStage::Radial) {
                    binding.image_node_id = image_node_id.clone();
                    source_sampler
                } else {
                    SamplerKind::LinearMirror
                }
            } else {
                sampler_kind_for_pass_texture(sc.scene(), texture_ref)
            };
            texture_bindings.push(binding);
            sampler_kinds.push(sampler);
        }
        bs.textures.push(TextureDecl {
            name: output.clone(),
            size,
            format: wgpu::TextureFormat::Rgba16Float,
            sample_count: 1,
            needs_sampling: false,
        });
        let geometry: ResourceName = format!("{name}.geo").into();
        let [w, h] = size.map(|v| v as f32);
        bs.geometry_buffers
            .push((geometry.clone(), make_fullscreen_geometry(w, h)));
        let params = make_params(
            [w, h],
            [w, h],
            [w * 0.5, h * 0.5],
            resolve_chain_camera_for_first_pass(
                camera_consumed,
                sc.scene(),
                sc.nodes_by_id(),
                node,
                [w, h],
            )?,
            [0.0; 4],
        );
        let pass_name: ResourceName = format!("{name}.pass").into();
        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: pass_name.to_string(),
            name: pass_name.clone(),
            geometry_buffer: geometry,
            instance_buffer: None,
            normals_buffer: None,
            vertex_layout: Default::default(),
            target_texture: output.clone(),
            resolve_target: None,
            params_buffer: format!("params.{name}").into(),
            baked_data_parse_buffer: None,
            params,
            graph_binding,
            graph_values,
            shader_wgsl: bundle.module,
            texture_bindings,
            sampler_kinds,
            blend_state: BlendState::REPLACE,
            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
            sample_count: 1,
        });
        bs.composite_passes.push(pass_name);
        bs.pass_output_registry.register_ports(
            &name,
            &["pass"],
            output.clone(),
            size,
            wgpu::TextureFormat::Rgba16Float,
        );
        source_id = name;
        result = output;
    }
    Ok(result)
}
