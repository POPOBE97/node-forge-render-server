use std::{borrow::Cow, collections::HashSet, sync::Arc};

use anyhow::{Context, Result, anyhow};
use rust_wgpu_fiber::{
    ResourceName,
    composition::CompositionBuilder,
    eframe::wgpu::{self, ShaderStages, TextureUsages, vertex_attr_array},
    pool::{buffer_pool::BufferSpec, texture_pool::TextureSpec as FiberTextureSpec},
    shader_space::ShaderSpace,
};

use crate::{
    dsl::SceneDSL,
    renderer::{
        render_plan::{
            pass_assemblers::dynamic_gaussian_blur::{
                GaussianBlurBundlePlan, GaussianBlurBundleRuntime, apply_runtime_values_to_route,
                gaussian_buffer_name,
            },
            pass_spec::{SamplerKind, VertexLayoutKind},
        },
        types::{GraphBindingKind, Params, PassBindings},
        utils::as_bytes,
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GaussianBlurUpdate {
    pub updated: usize,
    pub rebuilt: usize,
}

/// Update Gaussian scheduling for the current frame.
///
/// `extend=false` only writes the active route's H/V uniforms and flips prepared pass enable bits.
/// Allocation-sensitive `extend=true` changes keep the blur-bundle-only rebuild fallback.
pub(crate) fn update_dynamic_gaussian_blur_bundles(
    scene: &SceneDSL,
    shader_space: &mut ShaderSpace,
    pass_bindings: &mut Vec<PassBindings>,
    bundles: &mut std::collections::HashMap<String, GaussianBlurBundleRuntime>,
    pass_shader_overrides: &std::collections::HashMap<String, String>,
) -> Result<GaussianBlurUpdate> {
    let mut result = GaussianBlurUpdate::default();
    for runtime in bundles.values_mut() {
        let values = runtime.template.evaluate(scene)?;
        if !runtime.current.extend_enabled && !values.extend_enabled {
            if values.radius_px.to_bits() == runtime.current.radius_px.to_bits() {
                continue;
            }
            let previous_factor = runtime.current.active_factor;
            apply_runtime_values_to_route(&mut runtime.current, values);
            write_active_route_values(shader_space, &runtime.current)?;
            if runtime.current.active_factor != previous_factor {
                set_route_enabled(shader_space, &runtime.current)?;
            }
            result.updated += 1;
            continue;
        }

        if values.radius_px.to_bits() == runtime.current.radius_px.to_bits()
            && values.extend_enabled == runtime.current.extend_enabled
        {
            continue;
        }
        let mut next = runtime.template.build(scene)?;
        for spec in &mut next.render_pass_specs {
            if let Some(source) = pass_shader_overrides.get(spec.name.as_str()) {
                spec.shader_wgsl = source.clone();
            }
        }
        preserve_external_target_load_ops(&runtime.current, &mut next);
        replace_gaussian_blur_bundle(shader_space, pass_bindings, &runtime.current, &next)
            .with_context(|| format!("failed to replace Gaussian blur '{}'", next.layer_id))?;
        runtime.current = next;
        result.updated += 1;
        result.rebuilt += 1;
    }
    Ok(result)
}

fn preserve_external_target_load_ops(
    current: &GaussianBlurBundlePlan,
    next: &mut GaussianBlurBundlePlan,
) {
    let current_owned_textures = current
        .textures
        .iter()
        .map(|texture| texture.name.clone())
        .chain(current.texture_views.iter().map(|view| view.name.clone()))
        .collect::<HashSet<_>>();
    for next_spec in &mut next.render_pass_specs {
        if current_owned_textures.contains(&next_spec.target_texture) {
            continue;
        }
        if let Some(current_spec) = current
            .render_pass_specs
            .iter()
            .rev()
            .find(|spec| spec.target_texture == next_spec.target_texture)
        {
            next_spec.color_load_op = current_spec.color_load_op;
        }
    }
}

fn replace_gaussian_blur_bundle(
    shader_space: &mut ShaderSpace,
    pass_bindings: &mut Vec<PassBindings>,
    current: &GaussianBlurBundlePlan,
    next: &GaussianBlurBundlePlan,
) -> Result<()> {
    let current_pass_names = current.pass_names();
    let next_pass_names = next.pass_names();
    let current_buffer_names = bundle_buffer_names(current);
    let next_buffer_names = bundle_buffer_names(next);
    let current_texture_names = current
        .textures
        .iter()
        .map(|texture| texture.name.clone())
        .chain(current.texture_views.iter().map(|view| view.name.clone()))
        .collect::<HashSet<_>>();
    let next_texture_names = next
        .textures
        .iter()
        .map(|texture| texture.name.clone())
        .chain(next.texture_views.iter().map(|view| view.name.clone()))
        .collect::<HashSet<_>>();
    let texture_bindings_to_refresh = current_texture_names
        .union(&next_texture_names)
        .cloned()
        .collect::<HashSet<_>>();

    let next_composition = splice_bundle_composition(
        shader_space,
        &current.composite_passes,
        &next.composite_passes,
    )?;

    for pass_name in &current_pass_names {
        shader_space.passes.inner.remove(pass_name.as_str());
    }
    if let Ok(mut buffers) = shader_space.buffers.lock() {
        for name in current_buffer_names.difference(&next_buffer_names) {
            buffers.remove(name.as_str());
        }
    }
    for name in current_texture_names.difference(&next_texture_names) {
        shader_space.textures.remove(name.as_str());
    }

    declare_bundle_resources(shader_space, next);
    declare_bundle_passes(shader_space, next);
    shader_space.invalidate_bind_groups_using(&texture_bindings_to_refresh);
    shader_space.composite(move |composer| compose_in_strict_order(composer, &next_composition));
    shader_space.prepare();
    set_route_enabled(shader_space, next)?;
    write_bundle_values(shader_space, next)?;

    let old_pass_ids = current
        .render_pass_specs
        .iter()
        .map(|spec| spec.pass_id.as_str())
        .collect::<HashSet<_>>();
    let insertion_index = pass_bindings
        .iter()
        .position(|binding| old_pass_ids.contains(binding.pass_id.as_str()))
        .unwrap_or(pass_bindings.len());
    pass_bindings.retain(|binding| !old_pass_ids.contains(binding.pass_id.as_str()));
    let next_bindings = next
        .render_pass_specs
        .iter()
        .map(pass_binding_from_spec)
        .collect::<Vec<_>>();
    pass_bindings.splice(insertion_index..insertion_index, next_bindings);

    debug_assert!(
        next_pass_names
            .iter()
            .all(|name| shader_space.passes.inner.contains_key(name.as_str()))
    );
    Ok(())
}

pub(crate) fn set_route_enabled(
    shader_space: &mut ShaderSpace,
    bundle: &GaussianBlurBundlePlan,
) -> Result<()> {
    for (factor, passes) in &bundle.route_passes {
        let enabled = *factor == bundle.active_factor;
        for pass in passes {
            shader_space.set_pass_enabled(pass.as_str(), enabled)?;
        }
    }
    Ok(())
}

fn write_active_route_values(
    shader_space: &ShaderSpace,
    bundle: &GaussianBlurBundlePlan,
) -> Result<()> {
    let Some(uniforms) = bundle.route_uniforms.get(&bundle.active_factor) else {
        return Err(anyhow!(
            "Gaussian blur '{}' has no prebuilt factor {} route",
            bundle.layer_id,
            bundle.active_factor
        ));
    };
    for (buffer_name, value) in uniforms {
        shader_space.write_buffer(buffer_name.as_str(), 0, as_bytes(value))?;
    }
    Ok(())
}

fn bundle_buffer_names(bundle: &GaussianBlurBundlePlan) -> HashSet<ResourceName> {
    let mut names = bundle
        .geometry_buffers
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<HashSet<_>>();
    for spec in &bundle.render_pass_specs {
        names.insert(spec.params_buffer.clone());
        let gaussian_name = gaussian_buffer_name(&spec.params_buffer);
        if bundle
            .route_uniforms
            .values()
            .any(|uniforms| uniforms.iter().any(|(name, _)| *name == gaussian_name))
        {
            names.insert(gaussian_name);
        }
        if let Some(binding) = &spec.graph_binding {
            names.insert(binding.buffer_name.clone());
        }
        if let Some(name) = &spec.baked_data_parse_buffer {
            names.insert(name.clone());
        }
    }
    names
}

fn splice_bundle_composition(
    shader_space: &ShaderSpace,
    current_bundle_passes: &[ResourceName],
    next_bundle_passes: &[ResourceName],
) -> Result<Vec<ResourceName>> {
    let mut ordered = shader_space
        .composition
        .flatten()
        .into_iter()
        .map(|node| node.pass_name.clone())
        .collect::<Vec<_>>();
    ordered.reverse();
    splice_bundle_order(ordered, current_bundle_passes, next_bundle_passes)
}

fn splice_bundle_order(
    mut ordered: Vec<ResourceName>,
    current_bundle_passes: &[ResourceName],
    next_bundle_passes: &[ResourceName],
) -> Result<Vec<ResourceName>> {
    let current_names = current_bundle_passes
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let insertion_index = ordered
        .iter()
        .position(|name| current_names.contains(name))
        .ok_or_else(|| anyhow!("Gaussian blur bundle is absent from the active composition"))?;
    ordered.retain(|name| !current_names.contains(name));
    ordered.splice(
        insertion_index..insertion_index,
        next_bundle_passes.iter().cloned(),
    );
    Ok(ordered)
}

fn declare_bundle_resources(shader_space: &mut ShaderSpace, bundle: &GaussianBlurBundlePlan) {
    let mut buffer_specs = bundle
        .geometry_buffers
        .iter()
        .map(|(name, bytes)| BufferSpec::Init {
            name: name.clone(),
            contents: bytes.clone(),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        })
        .collect::<Vec<_>>();
    for spec in &bundle.render_pass_specs {
        buffer_specs.push(BufferSpec::Sized {
            name: spec.params_buffer.clone(),
            size: core::mem::size_of::<Params>(),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        if let Some(binding) = &spec.graph_binding {
            let usage = match binding.kind {
                GraphBindingKind::Uniform => {
                    wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST
                }
                GraphBindingKind::StorageRead => {
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST
                }
            };
            buffer_specs.push(BufferSpec::Sized {
                name: binding.buffer_name.clone(),
                size: binding.schema.size_bytes as usize,
                usage,
            });
        }
        if let Some(name) = &spec.baked_data_parse_buffer {
            let contents = bundle
                .baked_data_parse_buffer_to_pass_id
                .get(name)
                .and_then(|pass_id| bundle.baked_data_parse_bytes_by_pass.get(pass_id))
                .cloned()
                .unwrap_or_else(|| Arc::from(vec![0_u8; 16]));
            buffer_specs.push(BufferSpec::Init {
                name: name.clone(),
                contents,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        }
    }
    for uniforms in bundle.route_uniforms.values() {
        for (name, _) in uniforms {
            buffer_specs.push(BufferSpec::Sized {
                name: name.clone(),
                size: core::mem::size_of::<crate::renderer::types::GaussianUniform>(),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        }
    }
    shader_space.declare_buffers(buffer_specs);
    let mut texture_specs = bundle
        .textures
        .iter()
        .map(|texture| {
            let usage = if texture.sample_count > 1 {
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
            };
            if let Some(mip_level_count) =
                bundle.texture_mip_level_counts.get(&texture.name).copied()
            {
                FiberTextureSpec::MipmappedTexture {
                    name: texture.name.clone(),
                    resolution: texture.size,
                    format: texture.format,
                    usage,
                    mip_level_count,
                }
            } else {
                FiberTextureSpec::Texture {
                    name: texture.name.clone(),
                    resolution: texture.size,
                    format: texture.format,
                    usage,
                    sample_count: texture.sample_count,
                }
            }
        })
        .collect::<Vec<_>>();
    texture_specs.extend(
        bundle
            .texture_views
            .iter()
            .map(|view| FiberTextureSpec::TextureView {
                name: view.name.clone(),
                texture: view.texture.clone(),
                base_mip_level: view.base_mip_level,
            }),
    );
    shader_space.declare_textures(texture_specs);
}

fn declare_bundle_passes(shader_space: &mut ShaderSpace, bundle: &GaussianBlurBundlePlan) {
    for spec in &bundle.render_pass_specs {
        let spec = spec.clone();
        let shader_desc = wgpu::ShaderModuleDescriptor {
            label: Some("node-forge-dynamic-gaussian-blur"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(spec.shader_wgsl.clone())),
        };
        shader_space.render_pass(spec.name.clone(), move |builder| {
            let mut pass_builder = builder.shader(shader_desc).bind_uniform_buffer(
                0,
                0,
                spec.params_buffer.clone(),
                ShaderStages::VERTEX_FRAGMENT,
            );
            let gaussian_name = gaussian_buffer_name(&spec.params_buffer);
            if bundle
                .route_uniforms
                .values()
                .any(|uniforms| uniforms.iter().any(|(name, _)| *name == gaussian_name))
            {
                pass_builder =
                    pass_builder.bind_uniform_buffer(0, 4, gaussian_name, ShaderStages::FRAGMENT);
            }
            if let Some(buffer) = &spec.baked_data_parse_buffer {
                pass_builder = pass_builder.bind_storage_buffer(
                    0,
                    1,
                    buffer.clone(),
                    ShaderStages::VERTEX_FRAGMENT,
                    true,
                );
            }
            if let Some(binding) = &spec.graph_binding {
                pass_builder = match binding.kind {
                    GraphBindingKind::Uniform => pass_builder.bind_uniform_buffer(
                        0,
                        2,
                        binding.buffer_name.clone(),
                        ShaderStages::VERTEX_FRAGMENT,
                    ),
                    GraphBindingKind::StorageRead => pass_builder.bind_storage_buffer(
                        0,
                        2,
                        binding.buffer_name.clone(),
                        ShaderStages::VERTEX_FRAGMENT,
                        true,
                    ),
                };
            }
            let vertex_attributes = match spec.vertex_layout {
                VertexLayoutKind::PositionUv => {
                    vertex_attr_array![0 => Float32x3, 1 => Float32x2].to_vec()
                }
                VertexLayoutKind::PositionUvColor => {
                    vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4].to_vec()
                }
            };
            pass_builder = pass_builder.bind_attribute_buffer(
                0,
                spec.geometry_buffer.clone(),
                wgpu::VertexStepMode::Vertex,
                vertex_attributes,
            );
            for (index, binding) in spec.texture_bindings.iter().enumerate() {
                let sampler_name: ResourceName = match spec
                    .sampler_kinds
                    .get(index)
                    .copied()
                    .unwrap_or(SamplerKind::LinearClamp)
                {
                    SamplerKind::NearestClamp => "sampler_nearest",
                    SamplerKind::NearestMirror => "sampler_nearest_mirror",
                    SamplerKind::NearestRepeat => "sampler_nearest_repeat",
                    SamplerKind::LinearMirror => "sampler_linear_mirror",
                    SamplerKind::LinearRepeat => "sampler_linear_repeat",
                    SamplerKind::LinearClamp => "sampler_linear_clamp",
                }
                .into();
                let texture_binding = index as u32 * 2;
                pass_builder = pass_builder
                    .bind_texture(
                        1,
                        texture_binding,
                        binding.texture.clone(),
                        ShaderStages::FRAGMENT,
                    )
                    .bind_sampler(1, texture_binding + 1, sampler_name, ShaderStages::FRAGMENT);
            }
            pass_builder
                .bind_color_attachment(spec.target_texture.clone())
                .sample_count(spec.sample_count)
                .blending(spec.blend_state)
                .load_op(spec.color_load_op)
        });
    }
}

fn write_bundle_values(shader_space: &ShaderSpace, bundle: &GaussianBlurBundlePlan) -> Result<()> {
    for spec in &bundle.render_pass_specs {
        shader_space.write_buffer(spec.params_buffer.as_str(), 0, as_bytes(&spec.params))?;
        if let (Some(binding), Some(values)) = (&spec.graph_binding, &spec.graph_values) {
            shader_space.write_buffer(binding.buffer_name.as_str(), 0, values)?;
        }
    }
    for uniforms in bundle.route_uniforms.values() {
        for (name, value) in uniforms {
            shader_space.write_buffer(name.as_str(), 0, as_bytes(value))?;
        }
    }
    Ok(())
}

fn pass_binding_from_spec(
    spec: &crate::renderer::render_plan::types::RenderPassSpec,
) -> PassBindings {
    PassBindings {
        pass_id: spec.pass_id.clone(),
        params_buffer: spec.params_buffer.clone(),
        base_params: spec.params,
        graph_binding: spec.graph_binding.clone(),
        last_graph_hash: spec
            .graph_values
            .as_ref()
            .map(|values| crate::renderer::graph_uniforms::hash_bytes(values.as_slice())),
        shader_parameter_binding: None,
        last_shader_parameter_hash: None,
        extension: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig, pass::Pipeline};

    #[test]
    fn composition_splice_replaces_only_bundle_passes() {
        let order = splice_bundle_order(
            vec![
                "before".into(),
                "blur.h".into(),
                "blur.v".into(),
                "after".into(),
            ],
            &["blur.h".into(), "blur.v".into()],
            &["blur.ds".into(), "blur.h2".into(), "blur.v2".into()],
        )
        .unwrap();
        assert_eq!(
            order,
            vec![
                ResourceName::from("before"),
                ResourceName::from("blur.ds"),
                ResourceName::from("blur.h2"),
                ResourceName::from("blur.v2"),
                ResourceName::from("after"),
            ]
        );
    }

    #[test]
    fn replacing_blur_bundle_preserves_unrelated_pipeline() {
        let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/render/editor-examples/blur-guassian-20/scene.nforge");
        let (mut scene, assets) = crate::asset_store::load_from_nforge(&archive).unwrap();
        scene.nodes.push(
            serde_json::from_value(serde_json::json!({
                "id": "dynamic_blur_radius",
                "type": "FloatInput",
                "params": { "value": 20.0 }
            }))
            .unwrap(),
        );
        scene.connections.push(
            serde_json::from_value(serde_json::json!({
                "id": "dynamic_blur_radius_to_node_2",
                "from": { "nodeId": "dynamic_blur_radius", "portId": "value" },
                "to": { "nodeId": "node_2", "portId": "radius" }
            }))
            .unwrap(),
        );
        let headless = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("No adapter available for dynamic blur bundle test: {error:?}");
                return;
            }
        };
        if headless.adapter.get_info().backend == wgpu::Backend::Noop {
            eprintln!("Native GPU unavailable; skipping dynamic blur bundle integration test");
            return;
        }

        let mut result = crate::renderer::ShaderSpaceBuilder::new(
            headless.device.clone(),
            headless.queue.clone(),
        )
        .with_adapter(headless.adapter.clone())
        .with_asset_store(assets)
        .build(&scene)
        .unwrap();
        let untouched_name = result
            .shader_space
            .passes
            .inner
            .keys()
            .find(|name| !name.as_str().starts_with("sys.blur.node_2."))
            .cloned()
            .expect("unrelated pass");
        let untouched_pipeline_before = pipeline_debug(
            result
                .shader_space
                .passes
                .inner
                .get(untouched_name.as_str())
                .unwrap(),
        );
        let output_before = result.gaussian_blur_bundles["node_2"]
            .current
            .output_spec
            .texture_name
            .clone();
        let mut animated_scene = result.prepared_scene.take().unwrap();
        animated_scene
            .nodes
            .iter_mut()
            .find(|node| node.id == "dynamic_blur_radius")
            .unwrap()
            .params
            .insert("value".to_string(), serde_json::json!(100.0));

        let prepare_generation_before = result.shader_space.prepare_generation;
        let updated = update_dynamic_gaussian_blur_bundles(
            &animated_scene,
            &mut result.shader_space,
            &mut result.pass_bindings,
            &mut result.gaussian_blur_bundles,
            &Default::default(),
        )
        .unwrap();
        assert_eq!(updated.updated, 1);
        assert_eq!(
            updated.rebuilt, 0,
            "extend=false must not rebuild GPU state"
        );
        assert_eq!(
            result.shader_space.prepare_generation, prepare_generation_before,
            "extend=false animation must not call ShaderSpace::prepare"
        );
        let active_factor = result.gaussian_blur_bundles["node_2"].current.active_factor;
        for (factor, passes) in &result.gaussian_blur_bundles["node_2"].current.route_passes {
            for pass_name in passes {
                assert_eq!(
                    result.shader_space.passes.inner[pass_name.as_str()].enabled,
                    *factor == active_factor,
                    "route enable state must switch in the same update"
                );
            }
        }
        assert_eq!(
            result.gaussian_blur_bundles["node_2"]
                .current
                .output_spec
                .texture_name,
            output_before
        );
        assert_eq!(
            pipeline_debug(
                result
                    .shader_space
                    .passes
                    .inner
                    .get(untouched_name.as_str())
                    .unwrap(),
            ),
            untouched_pipeline_before
        );
        result.shader_space.render();
    }

    fn pipeline_debug(pass: &rust_wgpu_fiber::pass::Pass) -> String {
        match &pass.pipeline {
            Pipeline::Render(Some(pipeline)) => format!("{pipeline:?}"),
            Pipeline::Compute(Some(pipeline)) => format!("{pipeline:?}"),
            Pipeline::Render(None) => "render:none".to_string(),
            Pipeline::Compute(None) => "compute:none".to_string(),
        }
    }
}
