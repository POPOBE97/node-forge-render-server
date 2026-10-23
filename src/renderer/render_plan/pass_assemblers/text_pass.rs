//! Text is shaped on the CPU, then drawn as native-density glyph quads.
use anyhow::{Context, Result, ensure};
use rust_wgpu_fiber::{ResourceName, wgpu};

use super::super::{
    pass_spec::{
        PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl, VertexLayoutKind, make_params,
    },
    types::ImageTextureSpec,
};
use super::{
    args::{BuilderState, SceneContext},
    mesh_gradient::parse_color_value,
};
use crate::{
    dsl::{Node, incoming_connection},
    renderer::{
        camera::{legacy_projection_camera_matrix, resolve_effective_camera_for_pass_node},
        text::prepare_text,
        utils::cpu_num_f32,
    },
};

pub(crate) fn build_text_wgsl() -> String {
    include_str!("../../node_compiler/templates/text_pass.wgsl").into()
}

pub(crate) fn assemble_text_pass(
    sc: &SceneContext<'_>,
    bs: &mut BuilderState<'_>,
    id: &str,
    node: &Node,
) -> Result<()> {
    let inputs = crate::renderer::text_inputs::TextInputs {
        scene: sc.scene(), nodes: sc.nodes_by_id(), asset_store: sc.asset_store,
    };
    let (mut request, fonts) = crate::renderer::text_inputs::resolve(&inputs, node)?;
    let number = |port, default| cpu_num_f32(sc.scene(), sc.nodes_by_id(), node, port, default);
    let local_output = sc
        .scene()
        .connections
        .iter()
        .any(|edge| edge.from.node_id == id && edge.from.port_id == "texture");
    let max_atlas = sc.device.limits().max_texture_dimension_2d;
    let measured = prepare_text(&request, &fonts, max_atlas)
        .with_context(|| format!("TextPass {id} layout"))?;
    request.measured_frame = Some(measured.frame);
    let target = if local_output {
        [measured.size[0].ceil(), measured.size[1].ceil()]
    } else {
        bs.tgt_size
    };
    ensure!(
        target.iter().all(|n| *n >= 1.0 && *n <= max_atlas as f32),
        "TextPass target exceeds device limits"
    );
    let center = if local_output {
        [target[0] * 0.5, target[1] * 0.5]
    } else {
        crate::renderer::text_inputs::vec2(&inputs, node, "position")?.unwrap_or([target[0] * 0.5, target[1] * 0.5])
    };
    ensure!(
        center.iter().all(|n| n.is_finite()),
        "TextPass {id} position must be finite"
    );
    let camera = if local_output {
        legacy_projection_camera_matrix(target)
    } else {
        resolve_effective_camera_for_pass_node(sc.scene(), sc.nodes_by_id(), node, target)?
    };
    // Affine cameras have a constant physical pixel density over the text frame.
    ensure!(
        camera[3].abs() < 1e-6 && camera[7].abs() < 1e-6 && camera[15].abs() > 1e-6,
        "TextPass supports affine cameras; perspective text is not supported"
    );
    let w = camera[15];
    let sx = [
        (camera[0] / w) * target[0] * 0.5,
        (camera[1] / w) * target[1] * 0.5,
    ];
    let sy = [
        (camera[4] / w) * target[0] * 0.5,
        (camera[5] / w) * target[1] * 0.5,
    ];
    request.density = sx[0].hypot(sx[1]).max(sy[0].hypot(sy[1]));
    if sx[1].abs() < 1e-6 && sy[0].abs() < 1e-6 && sx[0] > 0.0 && sy[1] > 0.0 {
        let origin = [
            center[0] - measured.size[0] * 0.5 + measured.frame.origin[0],
            center[1] + measured.size[1] * 0.5 - measured.frame.origin[1],
        ];
        request.phase = [
            ((camera[0] * origin[0] + camera[12]) / w + 1.0) * target[0] * 0.5,
            (1.0 - (camera[5] * origin[1] + camera[13]) / w) * target[1] * 0.5,
        ]
        .map(|n| {
            if (n - n.round()).abs() < 1e-4 {
                0.0
            } else {
                n.rem_euclid(1.0)
            }
        });
    }
    let mesh = prepare_text(&request, &fonts, max_atlas)
        .with_context(|| format!("TextPass {id} rasterization"))?;
    let color = if let Some(edge) = incoming_connection(sc.scene(), id, "color") {
        let source = sc
            .nodes_by_id()
            .get(&edge.from.node_id)
            .context("missing text color source")?;
        ensure!(
            source.node_type == "ColorInput" && edge.from.port_id == "color",
            "TextPass.color requires a CPU ColorInput"
        );
        source
            .params
            .get("value")
            .and_then(parse_color_value)
            .context("invalid ColorInput value")?
    } else {
        node.params
            .get("color")
            .map(|v| parse_color_value(v).context("invalid TextPass color"))
            .transpose()?
            .unwrap_or([1.0; 4])
    };
    let mut color = color;
    let alpha = number("alpha", 1.0)?;
    ensure!(
        color.iter().all(|v| v.is_finite()) && alpha.is_finite(),
        "TextPass color and alpha must be finite"
    );
    color[3] = color[3].clamp(0.0, 1.0) * alpha.clamp(0.0, 1.0);
    let geometry: ResourceName = format!("sys.text.{id}.geo").into();
    let atlas: ResourceName = format!("sys.text.{id}.atlas").into();
    bs.geometry_buffers
        .push((geometry.clone(), mesh.vertices.clone()));
    bs.image_textures.push(ImageTextureSpec {
        name: atlas.clone(),
        image: mesh.atlas.clone(),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        srgb: false,
    });
    let materialized = bs.materialized_texture_output_ids.contains(id);
    let output: ResourceName = if materialized {
        format!("sys.text.{id}.out").into()
    } else {
        bs.target_texture_name.clone()
    };
    let format = if materialized {
        wgpu::TextureFormat::Rgba16Float
    } else {
        bs.target_format
    };
    let target_u = target.map(|n| n.round() as u32);
    if materialized {
        bs.textures.push(TextureDecl {
            name: output.clone(),
            size: target_u,
            format,
            sample_count: 1,
            needs_sampling: true,
        });
    }
    let blend = crate::renderer::render_plan::parse_render_pass_blend_state(&node.params)?;
    let pass: ResourceName = format!("sys.text.{id}.pass").into();
    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: pass.as_str().into(),
        name: pass.clone(),
        geometry_buffer: geometry,
        instance_buffer: None,
        normals_buffer: None,
        vertex_layout: VertexLayoutKind::PositionUv,
        target_texture: output.clone(),
        resolve_target: None,
        params_buffer: format!("params.sys.text.{id}").into(),
        baked_data_parse_buffer: None,
        params: make_params(target, mesh.size, center, camera, color),
        graph_binding: None,
        graph_values: None,
        shader_wgsl: build_text_wgsl(),
        texture_bindings: vec![PassTextureBinding {
            texture: atlas,
            image_node_id: None,
        }],
        sampler_kinds: vec![SamplerKind::LinearClamp],
        blend_state: blend,
        color_load_op: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
        sample_count: 1,
    });
    bs.composite_passes.push(pass);
    bs.pass_output_registry.register_ports(
        id,
        &["pass", "texture"],
        output.clone(),
        target_u,
        format,
    );
    // A sampled pass can also be a direct composition layer. Preserve both routes.
    for consumer in sc
        .composition_consumers_by_source
        .get(id)
        .into_iter()
        .flatten()
    {
        let Some(context) = sc.composition_contexts.get(consumer) else {
            continue;
        };
        if output == context.target_texture_name {
            continue;
        }
        let size = context.target_size_px;
        let geometry: ResourceName = format!("sys.text.{id}.to.{consumer}.geo").into();
        bs.push_fullscreen_geometry(geometry.clone(), size[0], size[1]);
        let pass: ResourceName = format!("sys.text.{id}.to.{consumer}.pass").into();
        bs.render_pass_specs.push(RenderPassSpec {
            pass_id: pass.as_str().into(),
            name: pass.clone(),
            geometry_buffer: geometry,
            instance_buffer: None,
            normals_buffer: None,
            vertex_layout: VertexLayoutKind::PositionUv,
            target_texture: context.target_texture_name.clone(),
            resolve_target: None,
            params_buffer: format!("params.sys.text.{id}.to.{consumer}").into(),
            baked_data_parse_buffer: None,
            params: make_params(
                size,
                size,
                size.map(|n| n * 0.5),
                legacy_projection_camera_matrix(size),
                [0.0; 4],
            ),
            graph_binding: None,
            graph_values: None,
            shader_wgsl: crate::renderer::wgsl::build_fullscreen_textured_bundle(
                "return textureSample(src_tex, src_samp, in.uv);".into(),
            )
            .module,
            texture_bindings: vec![PassTextureBinding {
                texture: output.clone(),
                image_node_id: None,
            }],
            sampler_kinds: vec![SamplerKind::LinearClamp],
            blend_state: blend,
            color_load_op: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            sample_count: 1,
        });
        bs.composite_passes.push(pass);
    }
    Ok(())
}
