use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, bail, Result};
use rust_wgpu_fiber::{
    eframe::wgpu::{
        self, include_wgsl, vertex_attr_array, BlendState, Color, ShaderStages, TextureFormat,
        TextureUsages,
    },
    pool::{
        buffer_pool::BufferSpec,
        texture_pool::TextureSpec as FiberTextureSpec,
    },
    shader_space::{ShaderSpace, ShaderSpaceResult},
    ResourceName,
};

use crate::{
    dsl::{
        find_node, incoming_connection, parse_f32, parse_texture_format, parse_u32, Node, SceneDSL,
    },
    graph::{topo_sort, upstream_reachable},
    vm,
};

#[derive(Clone, Debug)]
pub struct PassBindings {
    pub globals_buffer: ResourceName,
    pub program_buffer: ResourceName,
    pub consts_buffer: ResourceName,
    pub base_globals: vm::Globals,
}

pub fn update_pass_vm(
    shader_space: &ShaderSpace,
    pass: &PassBindings,
    globals: Option<&vm::Globals>,
    program_words: Option<&[u32]>,
    consts: Option<&[[f32; 4]]>,
) -> ShaderSpaceResult<()> {
    if let Some(g) = globals {
        shader_space.write_buffer(pass.globals_buffer.as_str(), 0, vm::as_bytes(g))?;
    }

    if let Some(words) = program_words {
        shader_space.write_buffer_slice(pass.program_buffer.as_str(), 0, words)?;
    }

    if let Some(c) = consts {
        shader_space.write_buffer_slice(pass.consts_buffer.as_str(), 0, c)?;
    }

    Ok(())
}

const PLANE_GEOMETRY: [[f32; 3]; 6] = [
    [-1.0, -1.0, 0.0],
    [1.0, -1.0, 0.0],
    [1.0, 1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
];

fn material_kind_from_renderpass(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    pass_id: &str,
) -> Result<u32> {
    let Some(conn) = incoming_connection(scene, pass_id, "material") else {
        return Ok(0);
    };
    let src = find_node(nodes_by_id, &conn.from.node_id)?;
    match src.node_type.as_str() {
        "Attribute" => Ok(1),
        other => bail!("unsupported material node type: {other}"),
    }
}

#[derive(Clone)]
struct TextureDecl {
    name: ResourceName,
    size: [u32; 2],
    format: TextureFormat,
}

#[derive(Clone)]
struct RenderPassSpec {
    name: ResourceName,
    geometry_buffer: ResourceName,
    target_texture: ResourceName,
    globals_buffer: ResourceName,
    program_buffer: ResourceName,
    consts_buffer: ResourceName,
    globals: vm::Globals,
    program_words: Vec<u32>,
    consts: Vec<[f32; 4]>,
}

pub fn build_shader_space_from_scene(
    scene: &SceneDSL,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
) -> Result<(ShaderSpace, [u32; 2], ResourceName, Vec<PassBindings>)> {
    let nodes_by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|n| (n.id.clone(), n))
        .collect();

    let mut ids: HashMap<String, ResourceName> = HashMap::new();
    for n in &scene.nodes {
        ids.insert(n.id.clone(), n.id.clone().into());
    }

    let order = topo_sort(scene)?;

    let output_node_id: String = scene
        .outputs
        .as_ref()
        .and_then(|m| m.get("composite").cloned())
        .or_else(|| {
            scene
                .nodes
                .iter()
                .find(|n| n.node_type == "CompositeOutput")
                .map(|n| n.id.clone())
        })
        .ok_or_else(|| anyhow!("no outputs.composite and no CompositeOutput node"))?;

    let render_pass_id: String = incoming_connection(scene, &output_node_id, "image")
        .map(|c| c.from.node_id.clone())
        .ok_or_else(|| anyhow!("CompositeOutput.image has no incoming connection"))?;

    let reachable = upstream_reachable(scene, &output_node_id);
    let render_passes_in_order: Vec<String> = order
        .iter()
        .filter(|id| reachable.contains(*id))
        .filter(|id| {
            nodes_by_id
                .get(*id)
                .is_some_and(|n| n.node_type == "RenderPass")
        })
        .cloned()
        .collect();
    if render_passes_in_order.is_empty() {
        bail!("no RenderPass reachable from CompositeOutput");
    }

    let last_pass_id: String = render_passes_in_order
        .last()
        .cloned()
        .unwrap_or_else(|| render_pass_id.clone());
    let output_texture_node_id: String = incoming_connection(scene, &last_pass_id, "target")
        .map(|c| c.from.node_id.clone())
        .ok_or_else(|| anyhow!("RenderPass.target has no incoming connection"))?;

    let output_texture_name: ResourceName = ids
        .get(&output_texture_node_id)
        .cloned()
        .ok_or_else(|| anyhow!("missing name for node: {}", output_texture_node_id))?;

    let output_texture_node = find_node(&nodes_by_id, &output_texture_node_id)?;
    if output_texture_node.node_type != "RenderTexture" {
        bail!(
            "RenderPass.target must come from RenderTexture, got {}",
            output_texture_node.node_type
        );
    }

    let width = parse_u32(&output_texture_node.params, "width").unwrap_or(1024);
    let height = parse_u32(&output_texture_node.params, "height").unwrap_or(1024);
    let resolution = [width, height];

    let mut geometry_buffers: Vec<ResourceName> = Vec::new();
    let mut textures: Vec<TextureDecl> = Vec::new();
    let mut render_pass_specs: Vec<RenderPassSpec> = Vec::new();
    let mut composite_passes: Vec<ResourceName> = Vec::new();

    for id in &order {
        if !reachable.contains(id) {
            continue;
        }
        let node = match nodes_by_id.get(id) {
            Some(n) => n,
            None => continue,
        };
        let name = ids
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("missing name for node: {id}"))?;

        match node.node_type.as_str() {
            "Rect2DGeometry" => {
                geometry_buffers.push(name);
            }
            "RenderTexture" => {
                let w = parse_u32(&node.params, "width").unwrap_or(width);
                let h = parse_u32(&node.params, "height").unwrap_or(height);
                let format = parse_texture_format(&node.params)?;
                textures.push(TextureDecl {
                    name,
                    size: [w, h],
                    format,
                });
            }
            _ => {}
        }
    }

    for pass_id in &render_passes_in_order {
        let pass_name = ids
            .get(pass_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing name for node: {pass_id}"))?;

        let geometry_node_id = incoming_connection(scene, pass_id, "geometry")
            .map(|c| c.from.node_id.clone())
            .ok_or_else(|| anyhow!("RenderPass.geometry missing for {pass_id}"))?;
        let target_texture_id = incoming_connection(scene, pass_id, "target")
            .map(|c| c.from.node_id.clone())
            .ok_or_else(|| anyhow!("RenderPass.target missing for {pass_id}"))?;

        let geometry_node = find_node(&nodes_by_id, &geometry_node_id)?;
        if geometry_node.node_type != "Rect2DGeometry" {
            bail!(
                "RenderPass.geometry must come from Rect2DGeometry, got {}",
                geometry_node.node_type
            );
        }
        let target_node = find_node(&nodes_by_id, &target_texture_id)?;
        if target_node.node_type != "RenderTexture" {
            bail!(
                "RenderPass.target must come from RenderTexture, got {}",
                target_node.node_type
            );
        }

        let geometry_buffer = ids
            .get(&geometry_node_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing name for node: {}", geometry_node_id))?;
        let target_texture = ids
            .get(&target_texture_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing name for node: {}", target_texture_id))?;

        let geo_w = parse_f32(&geometry_node.params, "width").unwrap_or(100.0);
        let geo_h = parse_f32(&geometry_node.params, "height").unwrap_or(geo_w);

        let tgt_w = parse_f32(&target_node.params, "width")
            .unwrap_or(width as f32)
            .max(1.0);
        let tgt_h = parse_f32(&target_node.params, "height")
            .unwrap_or(height as f32)
            .max(1.0);

        let scale_x = (geo_w / tgt_w).clamp(0.0, 10.0);
        let scale_y = (geo_h / tgt_h).clamp(0.0, 10.0);

        let material_kind = material_kind_from_renderpass(scene, &nodes_by_id, pass_id)?;

        let globals_name: ResourceName = format!("globals_{pass_id}").into();
        let program_name: ResourceName = format!("program_{pass_id}").into();
        let consts_name: ResourceName = format!("consts_{pass_id}").into();

        let consts: Vec<[f32; 4]> = vec![[0.9, 0.2, 0.2, 1.0]];
        let program_words = if material_kind == 1 {
            vm::program_uv_debug()
        } else {
            vm::program_constant_animated()
        };

        let globals = vm::Globals {
            scale: [scale_x, scale_y],
            time: 0.0,
            _pad0: 0.0,
            prog_len: program_words.len() as u32,
            const_len: consts.len() as u32,
            _pad1: [0, 0],
        };

        render_pass_specs.push(RenderPassSpec {
            name: pass_name.clone(),
            geometry_buffer,
            target_texture,
            globals_buffer: globals_name,
            program_buffer: program_name,
            consts_buffer: consts_name,
            globals,
            program_words,
            consts,
        });
        composite_passes.push(pass_name);
    }

    let mut shader_space = ShaderSpace::new(device, queue);

    let pass_program_sizes: Vec<(ResourceName, usize)> = render_pass_specs
        .iter()
        .map(|s| {
            (
                s.program_buffer.clone(),
                (s.program_words.len() * core::mem::size_of::<u32>()).max(8),
            )
        })
        .collect();
    let pass_const_sizes: Vec<(ResourceName, usize)> = render_pass_specs
        .iter()
        .map(|s| {
            (
                s.consts_buffer.clone(),
                (s.consts.len() * core::mem::size_of::<[f32; 4]>()).max(16),
            )
        })
        .collect();

    let pass_bindings: Vec<PassBindings> = render_pass_specs
        .iter()
        .map(|s| PassBindings {
            globals_buffer: s.globals_buffer.clone(),
            program_buffer: s.program_buffer.clone(),
            consts_buffer: s.consts_buffer.clone(),
            base_globals: s.globals,
        })
        .collect();

    // ---------------- data-driven declarations ----------------
    // 1) Buffers
    let plane_bytes: Arc<[u8]> = Arc::from(vm::as_bytes_slice(&PLANE_GEOMETRY).to_vec());
    let mut buffer_specs: Vec<BufferSpec> = Vec::new();

    for name in &geometry_buffers {
        buffer_specs.push(BufferSpec::Init {
            name: name.clone(),
            contents: plane_bytes.clone(),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
    }

    for pass in &pass_bindings {
        buffer_specs.push(BufferSpec::Sized {
            name: pass.globals_buffer.clone(),
            size: core::mem::size_of::<vm::Globals>(),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
    }
    for (name, bytes) in &pass_program_sizes {
        buffer_specs.push(BufferSpec::Sized {
            name: name.clone(),
            size: *bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
    }
    for (name, bytes) in &pass_const_sizes {
        buffer_specs.push(BufferSpec::Sized {
            name: name.clone(),
            size: *bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
    }

    shader_space.declare_buffers(buffer_specs);

    // 2) Textures
    let texture_specs: Vec<FiberTextureSpec> = textures
        .iter()
        .map(|t| FiberTextureSpec::Texture {
            name: t.name.clone(),
            resolution: t.size,
            format: t.format,
            usage: TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_SRC,
        })
        .collect();
    shader_space.declare_textures(texture_specs);

    for spec in &render_pass_specs {
        let geometry_buffer = spec.geometry_buffer.clone();
        let target_texture = spec.target_texture.clone();
        let globals_buffer = spec.globals_buffer.clone();
        let program_buffer = spec.program_buffer.clone();
        let consts_buffer = spec.consts_buffer.clone();
        let shader_desc = include_wgsl!("./shaders/bytecode_vm.wgsl");
        shader_space.render_pass(spec.name.clone(), move |builder| {
            builder
                .shader(shader_desc)
                .bind_storage_buffer(0, 0, globals_buffer, ShaderStages::VERTEX_FRAGMENT, true)
                .bind_storage_buffer(0, 1, program_buffer, ShaderStages::FRAGMENT, true)
                .bind_storage_buffer(0, 2, consts_buffer, ShaderStages::FRAGMENT, true)
                .bind_attribute_buffer(
                    0,
                    geometry_buffer,
                    wgpu::VertexStepMode::Vertex,
                    vertex_attr_array![0 => Float32x3].to_vec(),
                )
                .bind_color_attachment(target_texture)
                .blending(BlendState::REPLACE)
                .load_op(wgpu::LoadOp::Clear(Color::TRANSPARENT))
        });
    }

    shader_space.composite(move |composer| {
        let mut c = composer;
        for pass in &composite_passes {
            c = c.pass(pass.clone());
        }
        c
    });

    shader_space.prepare();

    for spec in &render_pass_specs {
        shader_space.write_buffer(spec.globals_buffer.as_str(), 0, vm::as_bytes(&spec.globals))?;
        shader_space.write_buffer_slice(spec.program_buffer.as_str(), 0, &spec.program_words)?;
        shader_space.write_buffer_slice(spec.consts_buffer.as_str(), 0, &spec.consts)?;
    }

    Ok((shader_space, resolution, output_texture_name, pass_bindings))
}
