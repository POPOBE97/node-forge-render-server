use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Result, anyhow};
use rust_wgpu_fiber::{ResourceName, eframe::wgpu};

use crate::{
    asset_store::AssetStore,
    dsl::{Node, SceneDSL, incoming_connection, parse_f32, parse_u32},
    renderer::{
        geometry_resolver::types::ResolvedCompositionContext,
        scene_prep::PreparedScene,
        types::{
            BakedDataParseMeta, GaussianUniform, PassExtension, PassOutputRegistry, PassOutputSpec,
        },
        utils::cpu_num_f32_min_0,
        wgsl::{gaussian_kernel_8, gaussian_mip_level_and_sigma_p},
    },
};

use super::{
    args::{BuilderState, SceneContext},
    gaussian_blur::assemble_gaussian_blur_with_radius,
};
use crate::renderer::render_plan::types::{
    PlanningDevice, RenderPassSpec, ShaderParameterBufferPlan, TextureDecl, TextureViewDecl,
};

pub(crate) const GAUSSIAN_ROUTE_FACTORS: [u32; 5] = [1, 2, 4, 8, 16];

/// Immutable planning context for one Gaussian blur node.
///
/// `extend=false` builds all five allocation-invariant routes once. `extend=true` keeps the
/// existing blur-bundle-only rebuild fallback because its texture dimensions depend on radius.
#[derive(Clone, Debug)]
pub struct GaussianBlurBundleTemplate {
    pub layer_id: String,
    prepared: PreparedScene,
    composition_contexts: HashMap<String, ResolvedCompositionContext>,
    composition_consumers_by_source: HashMap<String, Vec<String>>,
    draw_coord_size_by_pass: HashMap<String, [f32; 2]>,
    asset_store: Option<AssetStore>,
    planning_device: PlanningDevice,
    target_texture_name: ResourceName,
    target_format: wgpu::TextureFormat,
    sampled_pass_format: wgpu::TextureFormat,
    target_size: [f32; 2],
    target_size_u: [u32; 2],
    pass_output_registry_seed: PassOutputRegistry,
    sampled_pass_ids: HashSet<String>,
    baked_data_parse_bytes_by_pass_seed: HashMap<String, Arc<[u8]>>,
}

#[derive(Clone, Debug)]
pub struct GaussianBlurBundlePlan {
    pub(crate) layer_id: String,
    pub(crate) radius_px: f32,
    pub(crate) extend_enabled: bool,
    pub(crate) active_factor: u32,
    pub(crate) route_passes: HashMap<u32, Vec<ResourceName>>,
    /// Dedicated H/V Gaussian uniform buffers for each prebuilt route.
    pub(crate) route_uniforms: HashMap<u32, Vec<(ResourceName, GaussianUniform)>>,
    pub(crate) geometry_buffers: Vec<(ResourceName, Arc<[u8]>)>,
    pub(crate) textures: Vec<TextureDecl>,
    pub(crate) texture_mip_level_counts: HashMap<ResourceName, u32>,
    pub(crate) texture_views: Vec<TextureViewDecl>,
    pub(crate) render_pass_specs: Vec<RenderPassSpec>,
    pub(crate) composite_passes: Vec<ResourceName>,
    pub(crate) baked_data_parse_bytes_by_pass: HashMap<String, Arc<[u8]>>,
    pub(crate) baked_data_parse_buffer_to_pass_id: HashMap<ResourceName, String>,
    pub(crate) output_spec: PassOutputSpec,
}

#[derive(Clone, Debug)]
pub struct GaussianBlurBundleRuntime {
    pub(crate) template: GaussianBlurBundleTemplate,
    pub(crate) current: GaussianBlurBundlePlan,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GaussianRuntimeValues {
    pub radius_px: f32,
    pub extend_enabled: bool,
    pub factor: u32,
    pub kernel: [f32; 8],
    pub offset: [f32; 8],
    pub tap_count: u32,
}

impl GaussianBlurBundleTemplate {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        layer_id: String,
        prepared: PreparedScene,
        composition_contexts: HashMap<String, ResolvedCompositionContext>,
        composition_consumers_by_source: HashMap<String, Vec<String>>,
        draw_coord_size_by_pass: HashMap<String, [f32; 2]>,
        asset_store: Option<AssetStore>,
        planning_device: PlanningDevice,
        target_texture_name: ResourceName,
        target_format: wgpu::TextureFormat,
        sampled_pass_format: wgpu::TextureFormat,
        target_size: [f32; 2],
        target_size_u: [u32; 2],
        pass_output_registry_seed: PassOutputRegistry,
        sampled_pass_ids: HashSet<String>,
        baked_data_parse_bytes_by_pass_seed: HashMap<String, Arc<[u8]>>,
    ) -> Self {
        Self {
            layer_id,
            prepared,
            composition_contexts,
            composition_consumers_by_source,
            draw_coord_size_by_pass,
            asset_store,
            planning_device,
            target_texture_name,
            target_format,
            sampled_pass_format,
            target_size,
            target_size_u,
            pass_output_registry_seed,
            sampled_pass_ids,
            baked_data_parse_bytes_by_pass_seed,
        }
    }

    pub(crate) fn evaluate(&self, scene: &SceneDSL) -> Result<GaussianRuntimeValues> {
        let layer_node = scene
            .nodes
            .iter()
            .find(|node| node.id == self.layer_id)
            .ok_or_else(|| anyhow!("missing dynamic Gaussian blur node '{}'", self.layer_id))?;
        let radius_px = if incoming_connection(scene, &self.layer_id, "radius").is_none() {
            // State/Mutation animation writes the authored radius directly into this param. Keep
            // that 60 Hz path allocation-free; only graph-connected CPU inputs need an id map.
            parse_f32(&layer_node.params, "radius")
                .or_else(|| parse_u32(&layer_node.params, "radius").map(|value| value as f32))
                .unwrap_or(0.0)
                .max(0.0)
        } else {
            let nodes_by_id = scene
                .nodes
                .iter()
                .cloned()
                .map(|node| (node.id.clone(), node))
                .collect::<HashMap<String, Node>>();
            cpu_num_f32_min_0(scene, &nodes_by_id, layer_node, "radius", 0.0)?
        };
        let extend_enabled = layer_node
            .params
            .get("extend")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        Ok(runtime_values(radius_px, extend_enabled))
    }

    pub(crate) fn build(&self, scene: &SceneDSL) -> Result<GaussianBlurBundlePlan> {
        let values = self.evaluate(scene)?;
        if values.extend_enabled {
            self.build_single(scene, None, values)
        } else {
            self.build_prebuilt_routes(scene, values)
        }
    }

    fn build_prebuilt_routes(
        &self,
        scene: &SceneDSL,
        values: GaussianRuntimeValues,
    ) -> Result<GaussianBlurBundlePlan> {
        let stable_output: ResourceName = format!("sys.blur.{}.out", self.layer_id).into();
        let mut routes = Vec::with_capacity(GAUSSIAN_ROUTE_FACTORS.len());
        for factor in GAUSSIAN_ROUTE_FACTORS {
            let representative_radius = representative_radius_for_factor(factor);
            debug_assert_eq!(runtime_values(representative_radius, false).factor, factor);
            let route_values = runtime_values(representative_radius, false);
            let route = self.build_single(scene, Some(representative_radius), route_values)?;
            routes.push(rename_route(route, factor, &stable_output));
        }

        let first = routes
            .first()
            .ok_or_else(|| anyhow!("Gaussian route set is empty"))?;
        let output_spec = first.output_spec.clone();
        let mut aggregate = GaussianBlurBundlePlan {
            layer_id: self.layer_id.clone(),
            radius_px: values.radius_px,
            extend_enabled: false,
            active_factor: values.factor,
            route_passes: HashMap::new(),
            route_uniforms: HashMap::new(),
            geometry_buffers: Vec::new(),
            textures: Vec::new(),
            texture_mip_level_counts: HashMap::new(),
            texture_views: Vec::new(),
            render_pass_specs: Vec::new(),
            composite_passes: Vec::new(),
            baked_data_parse_bytes_by_pass: HashMap::new(),
            baked_data_parse_buffer_to_pass_id: HashMap::new(),
            output_spec,
        };

        let mut seen_geometry = HashSet::new();
        let mut seen_textures = HashSet::new();
        for mut route in routes {
            for (name, bytes) in route.geometry_buffers.drain(..) {
                if seen_geometry.insert(name.clone()) {
                    aggregate.geometry_buffers.push((name, bytes));
                }
            }
            for texture in route.textures.drain(..) {
                if seen_textures.insert(texture.name.clone()) {
                    aggregate.textures.push(texture);
                }
            }
            aggregate.route_passes.extend(route.route_passes.drain());
            aggregate
                .route_uniforms
                .extend(route.route_uniforms.drain());
            aggregate
                .texture_mip_level_counts
                .extend(route.texture_mip_level_counts.drain());
            aggregate.texture_views.append(&mut route.texture_views);
            aggregate
                .render_pass_specs
                .append(&mut route.render_pass_specs);
            aggregate
                .composite_passes
                .append(&mut route.composite_passes);
            aggregate
                .baked_data_parse_bytes_by_pass
                .extend(route.baked_data_parse_bytes_by_pass);
            aggregate
                .baked_data_parse_buffer_to_pass_id
                .extend(route.baked_data_parse_buffer_to_pass_id);
        }

        compact_prebuilt_route_textures(&mut aggregate)?;
        apply_runtime_values_to_route(&mut aggregate, values);
        Ok(aggregate)
    }

    fn build_single(
        &self,
        scene: &SceneDSL,
        radius_override: Option<f32>,
        values: GaussianRuntimeValues,
    ) -> Result<GaussianBlurBundlePlan> {
        let mut prepared = self.prepared.clone();
        prepared.scene = scene.clone();
        prepared.nodes_by_id = scene
            .nodes
            .iter()
            .cloned()
            .map(|node| (node.id.clone(), node))
            .collect::<HashMap<String, Node>>();

        let layer_node = prepared
            .nodes_by_id
            .get(&self.layer_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing dynamic Gaussian blur node '{}'", self.layer_id))?;

        let mut geometry_buffers = Vec::new();
        let mut instance_buffers = Vec::new();
        let mut textures = Vec::new();
        let mut texture_mip_level_counts = HashMap::new();
        let mut texture_views = Vec::new();
        let mut render_pass_specs = Vec::new();
        let mut composite_passes = Vec::new();
        let mut depth_resolve_passes = Vec::new();
        let mut pass_cull_mode_by_name = HashMap::new();
        let mut authored_color_load_ops_by_pass = HashMap::new();
        let mut pass_depth_attachment_by_name = HashMap::new();
        let mut pass_output_registry = self.pass_output_registry_seed.clone();
        let mut baked_data_parse_meta_by_pass: HashMap<String, Arc<BakedDataParseMeta>> =
            HashMap::new();
        let mut baked_data_parse_bytes_by_pass = self.baked_data_parse_bytes_by_pass_seed.clone();
        let mut baked_data_parse_buffer_to_pass_id = HashMap::new();
        let mut downsample_source_pass_ids = HashSet::new();
        let mut upsample_source_pass_ids = HashSet::new();
        let mut gaussian_source_pass_ids = HashSet::new();
        let mut bloom_source_pass_ids = HashSet::new();
        let mut gradient_source_pass_ids = HashSet::new();
        let mut pass_extensions: HashMap<String, PassExtension> = HashMap::new();
        let mut shader_parameter_buffers_by_pass: HashMap<String, ShaderParameterBufferPlan> =
            HashMap::new();

        let scene_context = SceneContext {
            prepared: &prepared,
            composition_contexts: &self.composition_contexts,
            composition_consumers_by_source: &self.composition_consumers_by_source,
            draw_coord_size_by_pass: &self.draw_coord_size_by_pass,
            asset_store: self.asset_store.as_ref(),
            device: &self.planning_device,
            adapter: None,
        };
        let mut builder_state = BuilderState {
            target_texture_name: &self.target_texture_name,
            target_format: self.target_format,
            sampled_pass_format: self.sampled_pass_format,
            tgt_size: self.target_size,
            tgt_size_u: self.target_size_u,
            geometry_buffers: &mut geometry_buffers,
            instance_buffers: &mut instance_buffers,
            textures: &mut textures,
            texture_mip_level_counts: &mut texture_mip_level_counts,
            texture_views: &mut texture_views,
            render_pass_specs: &mut render_pass_specs,
            composite_passes: &mut composite_passes,
            depth_resolve_passes: &mut depth_resolve_passes,
            pass_cull_mode_by_name: &mut pass_cull_mode_by_name,
            authored_color_load_ops_by_pass: &mut authored_color_load_ops_by_pass,
            pass_depth_attachment_by_name: &mut pass_depth_attachment_by_name,
            pass_output_registry: &mut pass_output_registry,
            sampled_pass_ids: &self.sampled_pass_ids,
            baked_data_parse_meta_by_pass: &mut baked_data_parse_meta_by_pass,
            baked_data_parse_bytes_by_pass: &mut baked_data_parse_bytes_by_pass,
            baked_data_parse_buffer_to_pass_id: &mut baked_data_parse_buffer_to_pass_id,
            downsample_source_pass_ids: &mut downsample_source_pass_ids,
            upsample_source_pass_ids: &mut upsample_source_pass_ids,
            gaussian_source_pass_ids: &mut gaussian_source_pass_ids,
            bloom_source_pass_ids: &mut bloom_source_pass_ids,
            gradient_source_pass_ids: &mut gradient_source_pass_ids,
            pass_extensions: &mut pass_extensions,
            shader_parameter_buffers_by_pass: &mut shader_parameter_buffers_by_pass,
        };
        assemble_gaussian_blur_with_radius(
            &scene_context,
            &mut builder_state,
            &self.layer_id,
            &layer_node,
            radius_override,
        )?;

        let output_spec = pass_output_registry
            .get_for_port(&self.layer_id, "pass")
            .cloned()
            .ok_or_else(|| anyhow!("dynamic Gaussian blur '{}' has no output", self.layer_id))?;
        let referenced_baked_pass_ids = baked_data_parse_buffer_to_pass_id
            .values()
            .cloned()
            .collect::<HashSet<_>>();
        baked_data_parse_bytes_by_pass
            .retain(|pass_id, _| referenced_baked_pass_ids.contains(pass_id));

        let route_pass_names = render_pass_specs
            .iter()
            .map(|spec| spec.name.clone())
            .collect::<Vec<_>>();
        let route_uniforms = blur_uniforms(&render_pass_specs, values);
        Ok(GaussianBlurBundlePlan {
            layer_id: self.layer_id.clone(),
            radius_px: values.radius_px,
            extend_enabled: values.extend_enabled,
            active_factor: values.factor,
            route_passes: HashMap::from([(values.factor, route_pass_names)]),
            route_uniforms: HashMap::from([(values.factor, route_uniforms)]),
            geometry_buffers,
            textures,
            texture_mip_level_counts,
            texture_views,
            render_pass_specs,
            composite_passes,
            baked_data_parse_bytes_by_pass,
            baked_data_parse_buffer_to_pass_id,
            output_spec,
        })
    }
}

impl GaussianBlurBundlePlan {
    pub(crate) fn append_to(self, builder_state: &mut BuilderState<'_>) {
        builder_state.geometry_buffers.extend(self.geometry_buffers);
        builder_state.textures.extend(self.textures);
        builder_state
            .texture_mip_level_counts
            .extend(self.texture_mip_level_counts);
        builder_state.texture_views.extend(self.texture_views);
        builder_state
            .render_pass_specs
            .extend(self.render_pass_specs);
        builder_state.composite_passes.extend(self.composite_passes);
        builder_state
            .baked_data_parse_bytes_by_pass
            .extend(self.baked_data_parse_bytes_by_pass);
        builder_state
            .baked_data_parse_buffer_to_pass_id
            .extend(self.baked_data_parse_buffer_to_pass_id);
        builder_state
            .pass_output_registry
            .register(self.output_spec);
    }

    pub(crate) fn pass_names(&self) -> HashSet<ResourceName> {
        self.render_pass_specs
            .iter()
            .map(|spec| spec.name.clone())
            .collect()
    }
}

/// Keep external attachment load semantics identical across mutually-exclusive routes.
///
/// Global first-write normalization sees every prebuilt route, although only one will execute.
/// Without this repair, later routes can inherit `Load` merely because route 1 appeared first.
pub(crate) fn synchronize_route_external_load_ops(
    plan: &GaussianBlurBundlePlan,
    render_pass_specs: &mut [RenderPassSpec],
) {
    let owned = plan
        .textures
        .iter()
        .map(|texture| texture.name.clone())
        .chain(plan.texture_views.iter().map(|view| view.name.clone()))
        .collect::<HashSet<_>>();
    let reference_passes = plan
        .route_passes
        .get(&GAUSSIAN_ROUTE_FACTORS[0])
        .into_iter()
        .flatten()
        .cloned()
        .collect::<HashSet<_>>();
    let reference_by_target = render_pass_specs
        .iter()
        .filter(|spec| {
            reference_passes.contains(&spec.name) && !owned.contains(&spec.target_texture)
        })
        .map(|spec| (spec.target_texture.clone(), spec.color_load_op))
        .collect::<HashMap<_, _>>();
    let bundle_passes = plan.pass_names();
    for spec in render_pass_specs {
        if !bundle_passes.contains(&spec.name) || owned.contains(&spec.target_texture) {
            continue;
        }
        if let Some(load_op) = reference_by_target.get(&spec.target_texture) {
            spec.color_load_op = *load_op;
        }
    }
}

pub(crate) fn runtime_values(radius_px: f32, extend_enabled: bool) -> GaussianRuntimeValues {
    let radius_px = radius_px.max(0.0);
    let sigma = radius_px / 3.525_494;
    let (mip_level, sigma_p) = gaussian_mip_level_and_sigma_p(sigma);
    let factor = 1_u32 << mip_level;
    let (kernel, offset, count) = gaussian_kernel_8(sigma_p.max(1e-6));
    GaussianRuntimeValues {
        radius_px,
        extend_enabled,
        factor,
        kernel,
        offset,
        tap_count: count.clamp(1, 8),
    }
}

pub(crate) fn apply_runtime_values_to_route(
    plan: &mut GaussianBlurBundlePlan,
    values: GaussianRuntimeValues,
) {
    plan.radius_px = values.radius_px;
    plan.active_factor = values.factor;
    if let Some(uniforms) = plan.route_uniforms.get_mut(&values.factor) {
        for (_, uniform) in uniforms {
            *uniform = GaussianUniform::new(values.kernel, values.offset, values.tap_count);
        }
    }
}

fn representative_radius_for_factor(factor: u32) -> f32 {
    match factor {
        1 => 0.0,
        2 => 20.0,
        4 => 50.0,
        8 => 100.0,
        16 => 200.0,
        _ => unreachable!("unsupported Gaussian route factor {factor}"),
    }
}

fn blur_uniforms(
    specs: &[RenderPassSpec],
    values: GaussianRuntimeValues,
) -> Vec<(ResourceName, GaussianUniform)> {
    specs
        .iter()
        .filter(|spec| {
            let name = spec.name.as_str();
            name.contains(".h.ds") || name.contains(".v.ds")
        })
        .map(|spec| {
            (
                gaussian_buffer_name(&spec.params_buffer),
                GaussianUniform::new(values.kernel, values.offset, values.tap_count),
            )
        })
        .collect()
}

pub(crate) fn gaussian_buffer_name(params_buffer: &ResourceName) -> ResourceName {
    format!("{}.gaussian", params_buffer.as_str()).into()
}

fn compact_prebuilt_route_textures(plan: &mut GaussianBlurBundlePlan) -> Result<()> {
    let h1: ResourceName = format!("sys.blur.{}.h.route1", plan.layer_id).into();
    let Some(base_decl) = plan
        .textures
        .iter()
        .find(|texture| texture.name == h1)
        .cloned()
    else {
        return Ok(());
    };
    // A level-4 view requires at least one 16-pixel dimension. Keep the already-correct separate
    // allocation layout for unusually tiny surfaces where wgpu cannot expose five mip levels.
    if base_decl.size[0].max(base_decl.size[1]) < 16 {
        return Ok(());
    }

    let owned_output = plan
        .textures
        .iter()
        .any(|texture| texture.name == plan.output_spec.texture_name);
    let chain_a: ResourceName = format!("sys.blur.{}.ping.a", plan.layer_id).into();
    let chain_b: ResourceName = format!("sys.blur.{}.ping.b", plan.layer_id).into();
    let a_views = (0..5)
        .map(|level| -> ResourceName {
            format!("sys.blur.{}.ping.a.mip{level}", plan.layer_id).into()
        })
        .collect::<Vec<_>>();
    let b_views = (0..5)
        .map(|level| -> ResourceName {
            if level == 0 && owned_output {
                plan.output_spec.texture_name.clone()
            } else {
                format!("sys.blur.{}.ping.b.mip{level}", plan.layer_id).into()
            }
        })
        .collect::<Vec<_>>();

    let mut aliases = prebuilt_route_texture_aliases(&plan.layer_id, &a_views, &b_views);
    let common_src: ResourceName = format!("sys.blur.{}.src", plan.layer_id).into();
    for factor in GAUSSIAN_ROUTE_FACTORS {
        let src: ResourceName = format!("sys.blur.{}.src.route{factor}", plan.layer_id).into();
        if plan.textures.iter().any(|texture| texture.name == src) {
            aliases.insert(src, common_src.clone());
        }
    }

    for spec in &mut plan.render_pass_specs {
        if let Some(name) = aliases.get(&spec.target_texture) {
            spec.target_texture = name.clone();
        }
        for binding in &mut spec.texture_bindings {
            if let Some(name) = aliases.get(&binding.texture) {
                binding.texture = name.clone();
            }
        }
    }

    let source_decl = plan
        .textures
        .iter()
        .find(|texture| aliases.get(&texture.name) == Some(&common_src))
        .cloned()
        .map(|mut texture| {
            texture.name = common_src;
            texture
        });
    plan.textures.clear();
    if let Some(source_decl) = source_decl {
        plan.textures.push(source_decl);
    }
    plan.textures.push(TextureDecl {
        name: chain_a.clone(),
        size: base_decl.size,
        format: base_decl.format,
        sample_count: 1,
        needs_sampling: true,
    });
    plan.textures.push(TextureDecl {
        name: chain_b.clone(),
        size: base_decl.size,
        format: base_decl.format,
        sample_count: 1,
        needs_sampling: true,
    });
    plan.texture_mip_level_counts.insert(chain_a.clone(), 5);
    plan.texture_mip_level_counts.insert(chain_b.clone(), 5);
    plan.texture_views = a_views
        .into_iter()
        .enumerate()
        .map(|(level, name)| TextureViewDecl {
            name,
            texture: chain_a.clone(),
            base_mip_level: level as u32,
        })
        .chain(
            b_views
                .into_iter()
                .enumerate()
                .map(|(level, name)| TextureViewDecl {
                    name,
                    texture: chain_b.clone(),
                    base_mip_level: level as u32,
                }),
        )
        .collect();
    Ok(())
}

fn prebuilt_route_texture_aliases(
    layer_id: &str,
    a_views: &[ResourceName],
    b_views: &[ResourceName],
) -> HashMap<ResourceName, ResourceName> {
    debug_assert_eq!(a_views.len(), GAUSSIAN_ROUTE_FACTORS.len());
    debug_assert_eq!(b_views.len(), GAUSSIAN_ROUTE_FACTORS.len());
    let mut aliases = HashMap::new();
    for factor in GAUSSIAN_ROUTE_FACTORS {
        let level = factor.trailing_zeros() as usize;
        let h: ResourceName = format!("sys.blur.{layer_id}.h.route{factor}").into();
        let v: ResourceName = format!("sys.blur.{layer_id}.v.route{factor}").into();
        if factor == 1 {
            aliases.insert(h, a_views[0].clone());
            aliases.insert(v, b_views[0].clone());
        } else if factor == 16 {
            // Flip the 16x route phase so its final upsample reads A.mip4 while writing B.mip0.
            // This avoids binding two subresources of the same backing texture for read/write in
            // one render pass, which is rejected by stricter WebGPU backends.
            aliases.insert(h, b_views[4].clone());
            aliases.insert(v, a_views[4].clone());
            aliases.insert(
                format!("sys.blur.{layer_id}.ds.8.route16").into(),
                b_views[3].clone(),
            );
            aliases.insert(
                format!("sys.blur.{layer_id}.ds.2.route16").into(),
                a_views[4].clone(),
            );
        } else {
            aliases.insert(h, b_views[level].clone());
            aliases.insert(v, a_views[level].clone());
            aliases.insert(
                format!("sys.blur.{layer_id}.ds.{factor}.route{factor}").into(),
                a_views[level].clone(),
            );
        }
    }
    aliases
}

fn route_name(name: &ResourceName, factor: u32) -> ResourceName {
    format!("{}.route{factor}", name.as_str()).into()
}

fn rename_route(
    mut plan: GaussianBlurBundlePlan,
    factor: u32,
    stable_output: &ResourceName,
) -> GaussianBlurBundlePlan {
    let old_output = plan.output_spec.texture_name.clone();
    let output_is_owned = plan
        .textures
        .iter()
        .any(|texture| texture.name == old_output);
    let mut names = HashMap::<ResourceName, ResourceName>::new();
    for (name, _) in &plan.geometry_buffers {
        names.insert(name.clone(), route_name(name, factor));
    }
    for texture in &plan.textures {
        let renamed = if output_is_owned && texture.name == old_output {
            stable_output.clone()
        } else {
            route_name(&texture.name, factor)
        };
        names.insert(texture.name.clone(), renamed);
    }
    for spec in &plan.render_pass_specs {
        names.insert(spec.name.clone(), route_name(&spec.name, factor));
        names.insert(
            spec.params_buffer.clone(),
            route_name(&spec.params_buffer, factor),
        );
        if let Some(binding) = &spec.graph_binding {
            names.insert(
                binding.buffer_name.clone(),
                route_name(&binding.buffer_name, factor),
            );
        }
        if let Some(name) = &spec.baked_data_parse_buffer {
            names.insert(name.clone(), route_name(name, factor));
        }
    }

    let rename = |name: &ResourceName| names.get(name).cloned().unwrap_or_else(|| name.clone());
    for (name, _) in &mut plan.geometry_buffers {
        *name = rename(name);
    }
    for texture in &mut plan.textures {
        texture.name = rename(&texture.name);
    }
    for spec in &mut plan.render_pass_specs {
        spec.name = rename(&spec.name);
        spec.pass_id = spec.name.as_str().to_string();
        spec.geometry_buffer = rename(&spec.geometry_buffer);
        spec.instance_buffer = spec.instance_buffer.as_ref().map(rename);
        spec.normals_buffer = spec.normals_buffer.as_ref().map(rename);
        spec.target_texture = rename(&spec.target_texture);
        spec.resolve_target = spec.resolve_target.as_ref().map(rename);
        spec.params_buffer = rename(&spec.params_buffer);
        spec.baked_data_parse_buffer = spec.baked_data_parse_buffer.as_ref().map(rename);
        if let Some(binding) = &mut spec.graph_binding {
            binding.buffer_name = rename(&binding.buffer_name);
        }
        for binding in &mut spec.texture_bindings {
            binding.texture = rename(&binding.texture);
        }
    }
    for pass in &mut plan.composite_passes {
        *pass = rename(pass);
    }
    plan.baked_data_parse_buffer_to_pass_id = plan
        .baked_data_parse_buffer_to_pass_id
        .into_iter()
        .map(|(name, pass_id)| (rename(&name), pass_id))
        .collect();
    plan.output_spec.texture_name = if output_is_owned {
        stable_output.clone()
    } else {
        old_output
    };
    plan.route_passes = HashMap::from([(
        factor,
        plan.render_pass_specs
            .iter()
            .map(|spec| spec.name.clone())
            .collect(),
    )]);
    plan.route_uniforms = HashMap::from([(
        factor,
        blur_uniforms(
            &plan.render_pass_specs,
            runtime_values(plan.radius_px, plan.extend_enabled),
        ),
    )]);
    plan.active_factor = factor;
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representative_radii_select_all_prebuilt_routes() {
        for factor in GAUSSIAN_ROUTE_FACTORS {
            assert_eq!(
                runtime_values(representative_radius_for_factor(factor), false).factor,
                factor
            );
        }
    }

    #[test]
    fn runtime_coefficients_keep_original_gaussian_math() {
        let values = runtime_values(100.0, false);
        let sigma = 100.0 / 3.525_494;
        let (level, sigma_p) = gaussian_mip_level_and_sigma_p(sigma);
        let (kernel, offset, count) = gaussian_kernel_8(sigma_p.max(1e-6));
        assert_eq!(values.factor, 1 << level);
        assert_eq!(values.kernel, kernel);
        assert_eq!(values.offset, offset);
        assert_eq!(values.tap_count, count.clamp(1, 8));
    }

    #[test]
    fn prebuilt_routes_ping_pong_between_two_mip_chains() {
        let a = (0..5)
            .map(|level| ResourceName::from(format!("a{level}")))
            .collect::<Vec<_>>();
        let b = (0..5)
            .map(|level| ResourceName::from(format!("b{level}")))
            .collect::<Vec<_>>();
        let aliases = prebuilt_route_texture_aliases("blur", &a, &b);
        let mapped = |name: &str| aliases.get(name).unwrap().as_str();

        assert_eq!(mapped("sys.blur.blur.h.route1"), "a0");
        assert_eq!(mapped("sys.blur.blur.v.route1"), "b0");
        for (factor, level) in [(2, 1), (4, 2), (8, 3)] {
            assert_eq!(
                mapped(&format!("sys.blur.blur.ds.{factor}.route{factor}")),
                format!("a{level}")
            );
            assert_eq!(
                mapped(&format!("sys.blur.blur.h.route{factor}")),
                format!("b{level}")
            );
            assert_eq!(
                mapped(&format!("sys.blur.blur.v.route{factor}")),
                format!("a{level}")
            );
        }
        assert_eq!(mapped("sys.blur.blur.ds.8.route16"), "b3");
        assert_eq!(mapped("sys.blur.blur.ds.2.route16"), "a4");
        assert_eq!(mapped("sys.blur.blur.h.route16"), "b4");
        assert_eq!(mapped("sys.blur.blur.v.route16"), "a4");
    }
}
