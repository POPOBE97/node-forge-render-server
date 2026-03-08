use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Result;
use rust_wgpu_fiber::{
    ResourceName,
    composition::CompositionBuilder,
    eframe::wgpu::{self, BlendState, Color, ShaderStages, TextureUsages, vertex_attr_array},
    pool::{
        buffer_pool::BufferSpec, sampler_pool::SamplerSpec,
        texture_pool::TextureSpec as FiberTextureSpec,
    },
    shader_space::ShaderSpace,
};

use crate::renderer::{
    graph_uniforms::compute_pipeline_signature_for_pass_bindings,
    node_compiler::geometry_nodes::rect2d_geometry_vertices,
    types::{GraphBindingKind, Params, PassBindings},
    utils::{as_bytes, as_bytes_slice},
};

use super::{
    pass_spec::SamplerKind,
    texture_caps::{
        collect_texture_capability_requirements, validate_texture_capability_requirements,
    },
};
use crate::renderer::render_plan::types::RenderPlan;

pub(crate) struct FinalizedShaderSpace {
    pub shader_space: ShaderSpace,
    pub pass_bindings: Vec<PassBindings>,
    pub pipeline_signature: [u8; 32],
}

pub(crate) struct ShaderSpaceFinalizer;

impl ShaderSpaceFinalizer {
    pub(crate) fn finalize(
        plan: &RenderPlan,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        adapter: Option<&wgpu::Adapter>,
    ) -> Result<FinalizedShaderSpace> {
        let resources = &plan.resources;
        let mut shader_space = ShaderSpace::new(device, queue);

        let pass_bindings: Vec<PassBindings> = resources
            .render_pass_specs
            .iter()
            .map(|spec| PassBindings {
                pass_id: spec.pass_id.clone(),
                params_buffer: spec.params_buffer.clone(),
                base_params: spec.params,
                graph_binding: spec.graph_binding.clone(),
                last_graph_hash: spec
                    .graph_values
                    .as_ref()
                    .map(|values| crate::renderer::graph_uniforms::hash_bytes(values.as_slice())),
            })
            .collect();
        let pipeline_signature =
            compute_pipeline_signature_for_pass_bindings(&plan.prepared.scene, &pass_bindings);

        let mut buffer_specs: Vec<BufferSpec> = Vec::new();
        for (name, bytes) in &resources.geometry_buffers {
            buffer_specs.push(BufferSpec::Init {
                name: name.clone(),
                contents: bytes.clone(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
        }
        for (name, bytes) in &resources.instance_buffers {
            buffer_specs.push(BufferSpec::Init {
                name: name.clone(),
                contents: bytes.clone(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
        }
        for pass in &pass_bindings {
            buffer_specs.push(BufferSpec::Sized {
                name: pass.params_buffer.clone(),
                size: core::mem::size_of::<Params>(),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            if let Some(graph_binding) = pass.graph_binding.as_ref() {
                let usage = match graph_binding.kind {
                    GraphBindingKind::Uniform => {
                        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST
                    }
                    GraphBindingKind::StorageRead => {
                        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST
                    }
                };
                buffer_specs.push(BufferSpec::Sized {
                    name: graph_binding.buffer_name.clone(),
                    size: graph_binding.schema.size_bytes as usize,
                    usage,
                });
            }
        }
        for spec in &resources.render_pass_specs {
            let Some(name) = spec.baked_data_parse_buffer.clone() else {
                continue;
            };
            let contents = resources
                .baked_data_parse_buffer_to_pass_id
                .get(&name)
                .and_then(|pass_id| resources.baked_data_parse_bytes_by_pass.get(pass_id))
                .cloned()
                .unwrap_or_else(|| Arc::from(vec![0u8; 16]));
            buffer_specs.push(BufferSpec::Init {
                name,
                contents,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        }
        for spec in &resources.image_prepasses {
            buffer_specs.push(BufferSpec::Sized {
                name: spec.params_buffer.clone(),
                size: core::mem::size_of::<Params>(),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        }
        for spec in &resources.depth_resolve_passes {
            buffer_specs.push(BufferSpec::Init {
                name: spec.geometry_buffer.clone(),
                contents: make_fullscreen_geometry(
                    spec.params.target_size[0],
                    spec.params.target_size[1],
                ),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            buffer_specs.push(BufferSpec::Sized {
                name: spec.params_buffer.clone(),
                size: core::mem::size_of::<Params>(),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        }
        shader_space.declare_buffers(buffer_specs);

        let mut texture_specs: Vec<FiberTextureSpec> = resources
            .textures
            .iter()
            .map(|texture| FiberTextureSpec::Texture {
                name: texture.name.clone(),
                resolution: texture.size,
                format: texture.format,
                usage: if texture.sample_count > 1 {
                    let base = TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC;
                    if texture.needs_sampling {
                        base | TextureUsages::TEXTURE_BINDING
                    } else {
                        base
                    }
                } else {
                    TextureUsages::RENDER_ATTACHMENT
                        | TextureUsages::TEXTURE_BINDING
                        | TextureUsages::COPY_SRC
                },
                sample_count: texture.sample_count,
            })
            .collect();
        texture_specs.extend(resources.image_textures.iter().map(|texture| {
            FiberTextureSpec::Image {
                name: texture.name.clone(),
                image: texture.image.clone(),
                usage: texture.usage,
                srgb: texture.srgb,
            }
        }));

        let texture_capability_requirements = collect_texture_capability_requirements(
            &texture_specs,
            &resources.render_pass_specs,
            &resources.prepass_texture_samples,
        )?;
        validate_texture_capability_requirements(
            &texture_capability_requirements,
            shader_space.device.features(),
            adapter,
        )?;
        shader_space.declare_textures(texture_specs);

        let nearest_sampler: ResourceName = "sampler_nearest".into();
        let nearest_mirror_sampler: ResourceName = "sampler_nearest_mirror".into();
        let nearest_repeat_sampler: ResourceName = "sampler_nearest_repeat".into();
        let linear_mirror_sampler: ResourceName = "sampler_linear_mirror".into();
        let linear_repeat_sampler: ResourceName = "sampler_linear_repeat".into();
        let linear_clamp_sampler: ResourceName = "sampler_linear_clamp".into();
        shader_space.declare_samplers(vec![
            SamplerSpec {
                name: nearest_sampler.clone(),
                desc: wgpu::SamplerDescriptor {
                    mag_filter: wgpu::FilterMode::Nearest,
                    min_filter: wgpu::FilterMode::Nearest,
                    mipmap_filter: wgpu::FilterMode::Nearest,
                    address_mode_u: wgpu::AddressMode::ClampToEdge,
                    address_mode_v: wgpu::AddressMode::ClampToEdge,
                    address_mode_w: wgpu::AddressMode::ClampToEdge,
                    ..Default::default()
                },
            },
            SamplerSpec {
                name: nearest_mirror_sampler.clone(),
                desc: wgpu::SamplerDescriptor {
                    mag_filter: wgpu::FilterMode::Nearest,
                    min_filter: wgpu::FilterMode::Nearest,
                    mipmap_filter: wgpu::FilterMode::Nearest,
                    address_mode_u: wgpu::AddressMode::MirrorRepeat,
                    address_mode_v: wgpu::AddressMode::MirrorRepeat,
                    address_mode_w: wgpu::AddressMode::MirrorRepeat,
                    ..Default::default()
                },
            },
            SamplerSpec {
                name: nearest_repeat_sampler.clone(),
                desc: wgpu::SamplerDescriptor {
                    mag_filter: wgpu::FilterMode::Nearest,
                    min_filter: wgpu::FilterMode::Nearest,
                    mipmap_filter: wgpu::FilterMode::Nearest,
                    address_mode_u: wgpu::AddressMode::Repeat,
                    address_mode_v: wgpu::AddressMode::Repeat,
                    address_mode_w: wgpu::AddressMode::Repeat,
                    ..Default::default()
                },
            },
            SamplerSpec {
                name: linear_mirror_sampler.clone(),
                desc: wgpu::SamplerDescriptor {
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    mipmap_filter: wgpu::FilterMode::Nearest,
                    address_mode_u: wgpu::AddressMode::MirrorRepeat,
                    address_mode_v: wgpu::AddressMode::MirrorRepeat,
                    address_mode_w: wgpu::AddressMode::MirrorRepeat,
                    ..Default::default()
                },
            },
            SamplerSpec {
                name: linear_repeat_sampler.clone(),
                desc: wgpu::SamplerDescriptor {
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    mipmap_filter: wgpu::FilterMode::Nearest,
                    address_mode_u: wgpu::AddressMode::Repeat,
                    address_mode_v: wgpu::AddressMode::Repeat,
                    address_mode_w: wgpu::AddressMode::Repeat,
                    ..Default::default()
                },
            },
            SamplerSpec {
                name: linear_clamp_sampler.clone(),
                desc: wgpu::SamplerDescriptor {
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    mipmap_filter: wgpu::FilterMode::Nearest,
                    address_mode_u: wgpu::AddressMode::ClampToEdge,
                    address_mode_v: wgpu::AddressMode::ClampToEdge,
                    address_mode_w: wgpu::AddressMode::ClampToEdge,
                    ..Default::default()
                },
            },
        ]);

        for spec in &resources.render_pass_specs {
            let geometry_buffer = spec.geometry_buffer.clone();
            let target_texture = spec.target_texture.clone();
            let resolve_target = spec.resolve_target.clone();
            let sample_count = spec.sample_count;
            let params_buffer = spec.params_buffer.clone();
            let shader_wgsl = spec.shader_wgsl.clone();
            let blend_state = spec.blend_state;
            let color_load_op = spec.color_load_op;
            let cull_mode = resources
                .pass_cull_mode_by_name
                .get(&spec.name)
                .copied()
                .unwrap_or(None);
            let depth_stencil_attachment = resources
                .pass_depth_attachment_by_name
                .get(&spec.name)
                .cloned();
            let graph_binding = spec.graph_binding.clone();

            let texture_names: Vec<ResourceName> = spec
                .texture_bindings
                .iter()
                .map(|binding| binding.texture.clone())
                .collect();
            let sampler_names: Vec<ResourceName> = spec
                .sampler_kinds
                .iter()
                .map(|kind| match kind {
                    SamplerKind::NearestClamp => nearest_sampler.clone(),
                    SamplerKind::NearestMirror => nearest_mirror_sampler.clone(),
                    SamplerKind::NearestRepeat => nearest_repeat_sampler.clone(),
                    SamplerKind::LinearMirror => linear_mirror_sampler.clone(),
                    SamplerKind::LinearRepeat => linear_repeat_sampler.clone(),
                    SamplerKind::LinearClamp => linear_clamp_sampler.clone(),
                })
                .collect();
            let fallback_sampler = linear_clamp_sampler.clone();

            if let Some(dir) = &plan.debug_dump_wgsl_dir {
                let debug_dump_path =
                    dir.join(format!("node-forge-pass.{}.wgsl", spec.name.as_str()));
                if let Some(parent) = debug_dump_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&debug_dump_path, &shader_wgsl);
            }

            let shader_desc = wgpu::ShaderModuleDescriptor {
                label: Some("node-forge-pass"),
                source: wgpu::ShaderSource::Wgsl(Cow::Owned(shader_wgsl)),
            };
            shader_space.render_pass(spec.name.clone(), move |builder| {
                let mut pass_builder = builder.shader(shader_desc).bind_uniform_buffer(
                    0,
                    0,
                    params_buffer,
                    ShaderStages::VERTEX_FRAGMENT,
                );

                if let Some(baked_data_parse_buffer) = spec.baked_data_parse_buffer.clone() {
                    pass_builder = pass_builder.bind_storage_buffer(
                        0,
                        1,
                        baked_data_parse_buffer.as_str(),
                        ShaderStages::VERTEX_FRAGMENT,
                        true,
                    );
                }

                if let Some(graph_binding) = graph_binding.clone() {
                    pass_builder = match graph_binding.kind {
                        GraphBindingKind::Uniform => pass_builder.bind_uniform_buffer(
                            0,
                            2,
                            graph_binding.buffer_name.clone(),
                            ShaderStages::VERTEX_FRAGMENT,
                        ),
                        GraphBindingKind::StorageRead => pass_builder.bind_storage_buffer(
                            0,
                            2,
                            graph_binding.buffer_name.clone(),
                            ShaderStages::VERTEX_FRAGMENT,
                            true,
                        ),
                    };
                }

                pass_builder = pass_builder.bind_attribute_buffer(
                    0,
                    geometry_buffer,
                    wgpu::VertexStepMode::Vertex,
                    vertex_attr_array![0 => Float32x3, 1 => Float32x2].to_vec(),
                );

                if let Some(instance_buffer) = spec.instance_buffer.clone() {
                    pass_builder = pass_builder.bind_attribute_buffer(
                        1,
                        instance_buffer,
                        wgpu::VertexStepMode::Instance,
                        vertex_attr_array![
                            2 => Float32x4,
                            3 => Float32x4,
                            4 => Float32x4,
                            5 => Float32x4
                        ]
                        .to_vec(),
                    );
                }

                if let Some(normals_buffer) = spec.normals_buffer.clone() {
                    let normals_slot = if spec.instance_buffer.is_some() { 2 } else { 1 };
                    pass_builder = pass_builder.bind_attribute_buffer(
                        normals_slot,
                        normals_buffer,
                        wgpu::VertexStepMode::Vertex,
                        vertex_attr_array![6 => Float32x3].to_vec(),
                    );
                }

                debug_assert_eq!(texture_names.len(), sampler_names.len());
                for (index, texture_name) in texture_names.iter().enumerate() {
                    let tex_binding = (index as u32) * 2;
                    let sampler_binding = tex_binding + 1;
                    pass_builder = pass_builder
                        .bind_texture(1, tex_binding, texture_name.clone(), ShaderStages::FRAGMENT)
                        .bind_sampler(
                            1,
                            sampler_binding,
                            sampler_names
                                .get(index)
                                .cloned()
                                .unwrap_or_else(|| fallback_sampler.clone()),
                            ShaderStages::FRAGMENT,
                        );
                }

                pass_builder = pass_builder
                    .bind_color_attachment(target_texture)
                    .sample_count(sample_count);
                if let Some(depth_texture) = depth_stencil_attachment.clone() {
                    pass_builder = pass_builder.bind_depth_stencil_attachment(depth_texture);
                }
                if let Some(resolve_target) = resolve_target.clone() {
                    pass_builder = pass_builder.resolve_target(resolve_target);
                }
                pass_builder
                    .cull_mode(cull_mode)
                    .blending(blend_state)
                    .load_op(color_load_op)
            });
        }

        for spec in &resources.image_prepasses {
            let shader_desc = wgpu::ShaderModuleDescriptor {
                label: Some("node-forge-imgpm"),
                source: wgpu::ShaderSource::Wgsl(Cow::Owned(spec.shader_wgsl.clone())),
            };
            let nearest_sampler_for_pass = nearest_sampler.clone();
            shader_space.render_pass(spec.pass_name.clone(), move |builder| {
                builder
                    .shader(shader_desc)
                    .bind_uniform_buffer(
                        0,
                        0,
                        spec.params_buffer.clone(),
                        ShaderStages::VERTEX_FRAGMENT,
                    )
                    .bind_attribute_buffer(
                        0,
                        spec.geometry_buffer.clone(),
                        wgpu::VertexStepMode::Vertex,
                        vertex_attr_array![0 => Float32x3, 1 => Float32x2].to_vec(),
                    )
                    .bind_texture(1, 0, spec.src_texture.clone(), ShaderStages::FRAGMENT)
                    .bind_sampler(1, 1, nearest_sampler_for_pass, ShaderStages::FRAGMENT)
                    .bind_color_attachment(spec.dst_texture.clone())
                    .blending(BlendState::REPLACE)
                    .load_op(wgpu::LoadOp::Clear(Color::TRANSPARENT))
            });
        }

        for spec in &resources.depth_resolve_passes {
            let shader_desc = wgpu::ShaderModuleDescriptor {
                label: Some("node-forge-depth-resolve"),
                source: wgpu::ShaderSource::Wgsl(Cow::Owned(spec.shader_wgsl.clone())),
            };
            shader_space.render_pass(spec.pass_name.clone(), move |builder| {
                builder
                    .shader(shader_desc)
                    .bind_uniform_buffer(
                        0,
                        0,
                        spec.params_buffer.clone(),
                        ShaderStages::VERTEX_FRAGMENT,
                    )
                    .bind_attribute_buffer(
                        0,
                        spec.geometry_buffer.clone(),
                        wgpu::VertexStepMode::Vertex,
                        vertex_attr_array![0 => Float32x3, 1 => Float32x2].to_vec(),
                    )
                    .bind_depth_texture(
                        1,
                        0,
                        spec.depth_texture.clone(),
                        ShaderStages::FRAGMENT,
                        spec.is_multisampled,
                    )
                    .bind_color_attachment(spec.dst_texture.clone())
                    .blending(BlendState::REPLACE)
                    .load_op(wgpu::LoadOp::Clear(Color::TRANSPARENT))
            });
        }

        let mut composite_passes = resources.composite_passes.clone();
        if !resources.image_prepasses.is_empty() {
            let mut ordered: Vec<ResourceName> = resources
                .image_prepasses
                .iter()
                .map(|spec| spec.pass_name.clone())
                .collect();
            ordered.append(&mut composite_passes);
            composite_passes = ordered;
        }
        shader_space
            .composite(move |composer| compose_in_strict_order(composer, &composite_passes));
        shader_space.prepare();

        for spec in &resources.render_pass_specs {
            shader_space.write_buffer(spec.params_buffer.as_str(), 0, as_bytes(&spec.params))?;
            if let (Some(graph_binding), Some(values)) = (&spec.graph_binding, &spec.graph_values) {
                shader_space.write_buffer(graph_binding.buffer_name.as_str(), 0, values)?;
            }
        }
        for spec in &resources.image_prepasses {
            shader_space.write_buffer(spec.params_buffer.as_str(), 0, as_bytes(&spec.params))?;
        }
        for spec in &resources.depth_resolve_passes {
            shader_space.write_buffer(spec.params_buffer.as_str(), 0, as_bytes(&spec.params))?;
        }

        Ok(FinalizedShaderSpace {
            shader_space,
            pass_bindings,
            pipeline_signature,
        })
    }
}

fn compose_in_strict_order(
    composer: CompositionBuilder,
    ordered_passes: &[ResourceName],
) -> CompositionBuilder {
    match ordered_passes {
        [] => composer,
        [only] => composer.pass(only.clone()),
        _ => {
            let (deps, last) = ordered_passes.split_at(ordered_passes.len() - 1);
            let last = last[0].clone();
            composer.pass_with_deps(last, move |composer| {
                compose_in_strict_order(composer, deps)
            })
        }
    }
}

fn make_fullscreen_geometry(w: f32, h: f32) -> Arc<[u8]> {
    let verts = rect2d_geometry_vertices(w, h);
    Arc::from(as_bytes_slice(&verts).to_vec())
}
