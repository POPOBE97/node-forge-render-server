//! Shared context threaded through pass assemblers.
//!
//! `AssembleContext` bundles all mutable builder state (texture declarations,
//! geometry buffers, render-pass specs, composite ordering, etc.) together with
//! immutable references to the prepared scene, resolved draw contexts, and GPU
//! device/adapter.  Every pass assembler receives `&mut AssembleContext` so that
//! it can register resources without touching any other assembler's internals.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Result, anyhow};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color, TextureFormat},
};

use crate::{
    dsl::{SceneDSL, Node},
    renderer::{
        camera::legacy_projection_camera_matrix,
        geometry_resolver::types::ResolvedCompositionContext,
        node_compiler::geometry_nodes::rect2d_geometry_vertices,
        scene_prep::PreparedScene,
        types::{BakedDataParseMeta, PassOutputRegistry, PassOutputSpec},
        utils::as_bytes_slice,
        wgsl::build_fullscreen_textured_bundle,
    },
};

use super::pass_spec::{
    DepthResolvePass, ImagePrepass, PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl,
    make_params,
};
use super::resource_naming::resolve_chain_camera_for_first_pass;

/// Immutable references into the prepared scene and resolved draw contexts.
///
/// Kept separate so we don't accidentally make the prepared-scene data mutable.
pub(crate) struct SceneRef<'a> {
    pub prepared: &'a PreparedScene,
    pub composition_contexts: &'a HashMap<String, ResolvedCompositionContext>,
    pub composition_consumers_by_source: &'a HashMap<String, Vec<String>>,
    pub draw_coord_size_by_pass: &'a HashMap<String, [f32; 2]>,
}

impl<'a> SceneRef<'a> {
    /// Convenience: the underlying `SceneDSL`.
    #[inline]
    pub fn scene(&self) -> &SceneDSL {
        &self.prepared.scene
    }

    /// Convenience: nodes indexed by id.
    #[inline]
    pub fn nodes_by_id(&self) -> &HashMap<String, Node> {
        &self.prepared.nodes_by_id
    }

    /// Convenience: deterministic resource names indexed by node id.
    #[inline]
    pub fn ids(&self) -> &HashMap<String, ResourceName> {
        &self.prepared.ids
    }
}

/// Mutable builder state accumulated while assembling render passes.
///
/// Each pass assembler pushes into the vectors / maps here.  After all passes
/// have been assembled the owning function drains these collections to build the
/// final `ShaderSpace`.
pub(crate) struct AssembleContext<'a> {
    // ---- GPU handles (immutable, shared by reference) ----
    pub device: &'a Arc<wgpu::Device>,
    pub adapter: Option<&'a wgpu::Adapter>,
    pub asset_store: Option<&'a crate::asset_store::AssetStore>,

    // ---- target texture info ----
    pub target_texture_name: ResourceName,
    pub target_format: TextureFormat,
    pub sampled_pass_format: TextureFormat,
    pub tgt_size: [f32; 2],
    pub tgt_size_u: [u32; 2],

    // ---- mutable builder state ----
    pub geometry_buffers: Vec<(ResourceName, Arc<[u8]>)>,
    pub instance_buffers: Vec<(ResourceName, Arc<[u8]>)>,
    pub textures: Vec<TextureDecl>,
    pub render_pass_specs: Vec<RenderPassSpec>,
    pub composite_passes: Vec<ResourceName>,
    pub depth_resolve_passes: Vec<DepthResolvePass>,
    pub pass_cull_mode_by_name: HashMap<ResourceName, Option<wgpu::Face>>,
    pub pass_depth_attachment_by_name: HashMap<ResourceName, ResourceName>,

    // ---- baked data parse bookkeeping ----
    pub baked_data_parse_meta_by_pass: HashMap<String, Arc<BakedDataParseMeta>>,
    pub baked_data_parse_bytes_by_pass: HashMap<String, Arc<[u8]>>,
    pub baked_data_parse_buffer_to_pass_id: HashMap<ResourceName, String>,

    // ---- pass output registry (chain resolution) ----
    pub pass_output_registry: PassOutputRegistry,

    // ---- sampled pass tracking ----
    pub sampled_pass_ids: HashSet<String>,

    // ---- source-type pass tracking for intermediate-output decisions ----
    pub downsample_source_pass_ids: HashSet<String>,
    pub upsample_source_pass_ids: HashSet<String>,
    pub gaussian_source_pass_ids: HashSet<String>,
    pub bloom_source_pass_ids: HashSet<String>,
    pub gradient_source_pass_ids: HashSet<String>,
}

impl<'a> AssembleContext<'a> {
    /// Create a fullscreen geometry buffer (two-triangle quad covering `w × h` pixels).
    pub fn make_fullscreen_geometry(&self, w: f32, h: f32) -> Arc<[u8]> {
        let verts = rect2d_geometry_vertices(w, h);
        Arc::from(as_bytes_slice(&verts).to_vec())
    }

    /// Push a new geometry buffer and return its name.
    pub fn push_geometry(&mut self, name: ResourceName, bytes: Arc<[u8]>) {
        self.geometry_buffers.push((name, bytes));
    }

    /// Convenience: push a fullscreen geometry buffer and return its name.
    pub fn push_fullscreen_geometry(&mut self, name: ResourceName, w: f32, h: f32) {
        let bytes = self.make_fullscreen_geometry(w, h);
        self.geometry_buffers.push((name, bytes));
    }

    /// Register a pass output for downstream chain resolution.
    pub fn register_pass_output(&mut self, spec: PassOutputSpec) {
        self.pass_output_registry.register(spec);
    }

    /// Register a pass output on a specific port for downstream chain resolution.
    pub fn register_pass_output_for_port(&mut self, spec: PassOutputSpec, port: &str) {
        self.pass_output_registry.register_for_port(spec, port);
    }

    /// Build composition-consumer blit passes that copy `source_texture` into
    /// each downstream Composition target.
    ///
    /// This deduplicates the pattern that formerly appeared 7 times across every
    /// pass-type arm.
    ///
    /// # Arguments
    ///
    /// * `scene_ref` — Immutable scene references.
    /// * `layer_id` — The current pass node id whose output is being composed.
    /// * `source_texture` — The texture to sample from.
    /// * `prefix` — A label segment for resource naming (e.g., `"pass"`, `"blur"`,
    ///   `"downsample"`, `"upsample"`, `"gradient_blur"`, `"bloom"`, `"comp"`).
    /// * `blend_state` — Blend state for the compose pass.
    /// * `sampler_kind` — Sampler to use for the blit (typically `LinearClamp`).
    /// * `geo_size` — Size of the geometry quad for the blit. When `None`, each
    ///   composition target's own size is used (simple fullscreen blit).
    /// * `center` — Center of the geometry in the target. When `None`, target center is used.
    /// * `camera_override` — Optional camera matrix override. When provided, this
    ///   exact camera is used for all compose passes. When `None`, the default
    ///   orthographic projection is used.
    /// * `skip_self_target` — When `true`, skip compose passes where
    ///   `source_texture == comp_ctx.target_texture_name`.
    /// * `extra_skip` — Optional additional predicate to skip specific consumers.
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
