//! Shared planning context threaded through pass planners.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color, TextureFormat},
};

use crate::{
    dsl::{Node, SceneDSL},
    renderer::{
        camera::legacy_projection_camera_matrix,
        geometry_resolver::types::ResolvedCompositionContext,
        node_compiler::geometry_nodes::rect2d_geometry_vertices,
        render_plan::types::{
            DepthResolvePass, ImagePrepass, ImageTextureSpec, PlanningGpuCaps, RenderPassSpec,
            SamplerKind, TextureDecl,
        },
        scene_prep::PreparedScene,
        types::{BakedDataParseMeta, PassOutputRegistry, PassOutputSpec},
        utils::as_bytes_slice,
        wgsl::build_fullscreen_textured_bundle,
    },
};

use super::pass_spec::{PassTextureBinding, make_params};

#[derive(Clone, Debug, Default)]
pub(crate) struct PlanningDevice {
    features: wgpu::Features,
    limits: wgpu::Limits,
}

impl PlanningDevice {
    pub fn new(features: wgpu::Features, limits: wgpu::Limits) -> Self {
        Self { features, limits }
    }

    pub fn features(&self) -> wgpu::Features {
        self.features
    }

    pub fn limits(&self) -> &wgpu::Limits {
        &self.limits
    }
}

pub(crate) struct SceneRef<'a> {
    pub prepared: &'a PreparedScene,
    pub composition_contexts: &'a HashMap<String, ResolvedCompositionContext>,
    pub composition_consumers_by_source: &'a HashMap<String, Vec<String>>,
    pub draw_coord_size_by_pass: &'a HashMap<String, [f32; 2]>,
}

impl<'a> SceneRef<'a> {
    #[inline]
    pub fn scene(&self) -> &SceneDSL {
        &self.prepared.scene
    }

    #[inline]
    pub fn nodes_by_id(&self) -> &HashMap<String, Node> {
        &self.prepared.nodes_by_id
    }

    #[inline]
    pub fn ids(&self) -> &HashMap<String, ResourceName> {
        &self.prepared.ids
    }
}

pub(crate) struct AssembleContext<'a> {
    pub gpu_caps: PlanningGpuCaps,
    pub device: PlanningDevice,
    pub adapter: Option<&'a wgpu::Adapter>,
    pub asset_store: Option<&'a crate::asset_store::AssetStore>,

    pub target_texture_name: ResourceName,
    pub target_format: TextureFormat,
    pub sampled_pass_format: TextureFormat,
    pub tgt_size: [f32; 2],
    pub tgt_size_u: [u32; 2],

    pub geometry_buffers: Vec<(ResourceName, Arc<[u8]>)>,
    pub instance_buffers: Vec<(ResourceName, Arc<[u8]>)>,
    pub textures: Vec<TextureDecl>,
    pub image_textures: Vec<ImageTextureSpec>,
    pub render_pass_specs: Vec<RenderPassSpec>,
    pub composite_passes: Vec<ResourceName>,
    pub depth_resolve_passes: Vec<DepthResolvePass>,
    pub image_prepasses: Vec<ImagePrepass>,
    pub prepass_texture_samples: Vec<(String, ResourceName)>,
    pub pass_cull_mode_by_name: HashMap<ResourceName, Option<wgpu::Face>>,
    pub pass_depth_attachment_by_name: HashMap<ResourceName, ResourceName>,

    pub baked_data_parse_meta_by_pass: HashMap<String, Arc<BakedDataParseMeta>>,
    pub baked_data_parse_bytes_by_pass: HashMap<String, Arc<[u8]>>,
    pub baked_data_parse_buffer_to_pass_id: HashMap<ResourceName, String>,

    pub pass_output_registry: PassOutputRegistry,
    pub sampled_pass_ids: HashSet<String>,
    pub downsample_source_pass_ids: HashSet<String>,
    pub upsample_source_pass_ids: HashSet<String>,
    pub gaussian_source_pass_ids: HashSet<String>,
    pub bloom_source_pass_ids: HashSet<String>,
    pub gradient_source_pass_ids: HashSet<String>,
}

impl<'a> AssembleContext<'a> {
    pub fn make_fullscreen_geometry(&self, w: f32, h: f32) -> Arc<[u8]> {
        let verts = rect2d_geometry_vertices(w, h);
        Arc::from(as_bytes_slice(&verts).to_vec())
    }

    pub fn push_geometry(&mut self, name: ResourceName, bytes: Arc<[u8]>) {
        self.geometry_buffers.push((name, bytes));
    }

    pub fn push_fullscreen_geometry(&mut self, name: ResourceName, w: f32, h: f32) {
        let bytes = self.make_fullscreen_geometry(w, h);
        self.geometry_buffers.push((name, bytes));
    }

    pub fn register_pass_output(&mut self, spec: PassOutputSpec) {
        self.pass_output_registry.register(spec);
    }

    pub fn register_pass_output_for_port(&mut self, spec: PassOutputSpec, port: &str) {
        self.pass_output_registry.register_for_port(spec, port);
    }

    pub fn build_composition_consumer_passes(
        &mut self,
        scene_ref: &SceneRef<'_>,
        layer_id: &str,
        source_texture: &ResourceName,
        prefix: &str,
        blend_state: BlendState,
        sampler_kind: SamplerKind,
        geo_size: Option<[f32; 2]>,
        center: Option<[f32; 2]>,
        camera_override: Option<[f32; 16]>,
        skip_self_target: bool,
        extra_skip: Option<&dyn Fn(&str, &ResolvedCompositionContext) -> bool>,
    ) {
        let composition_consumers = scene_ref
            .composition_consumers_by_source
            .get(layer_id)
            .cloned()
            .unwrap_or_default();

        for composition_id in &composition_consumers {
            let Some(comp_ctx) = scene_ref.composition_contexts.get(composition_id) else {
                continue;
            };
            if skip_self_target && *source_texture == comp_ctx.target_texture_name {
                continue;
            }
            if let Some(skip_fn) = &extra_skip {
                if skip_fn(composition_id, comp_ctx) {
                    continue;
                }
            }

            let comp_w = comp_ctx.target_size_px[0];
            let comp_h = comp_ctx.target_size_px[1];

            let blit_w = geo_size.map_or(comp_w, |s| s[0]);
            let blit_h = geo_size.map_or(comp_h, |s| s[1]);
            let blit_cx = center.map_or(comp_w * 0.5, |c| c[0]);
            let blit_cy = center.map_or(comp_h * 0.5, |c| c[1]);

            let camera = camera_override
                .unwrap_or_else(|| legacy_projection_camera_matrix([comp_w, comp_h]));

            let compose_geo: ResourceName =
                format!("sys.{prefix}.{layer_id}.to.{composition_id}.compose.geo").into();
            self.push_fullscreen_geometry(compose_geo.clone(), blit_w, blit_h);

            let compose_pass_name: ResourceName =
                format!("sys.{prefix}.{layer_id}.to.{composition_id}.compose.pass").into();
            let compose_params_name: ResourceName =
                format!("params.sys.{prefix}.{layer_id}.to.{composition_id}.compose").into();

            let compose_params = make_params(
                [comp_w, comp_h],
                [blit_w, blit_h],
                [blit_cx, blit_cy],
                camera,
                [0.0, 0.0, 0.0, 0.0],
            );

            self.render_pass_specs.push(RenderPassSpec {
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
                    texture: source_texture.clone(),
                    image_node_id: None,
                }],
                sampler_kinds: vec![sampler_kind],
                blend_state,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                sample_count: 1,
            });

            self.composite_passes.push(compose_pass_name);
        }
    }
}
