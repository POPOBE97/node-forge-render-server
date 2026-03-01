//! Shared argument bundle for pass assembler functions.
//!
//! `PassAssemblerArgs` bundles the mutable builder state and immutable context
//! that every pass-type assembler needs.  Each extracted assembler function
//! receives `&mut PassAssemblerArgs<'_, '_>` instead of 10+ individual parameters.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, TextureFormat},
};

use crate::{
    dsl::{Node, SceneDSL},
    renderer::{
        geometry_resolver::types::ResolvedCompositionContext,
        node_compiler::geometry_nodes::rect2d_geometry_vertices,
        scene_prep::PreparedScene,
        types::{BakedDataParseMeta, PassOutputRegistry},
        utils::as_bytes_slice,
    },
};

use super::super::pass_spec::{DepthResolvePass, RenderPassSpec, TextureDecl};

/// Immutable context shared by all pass assemblers.
pub(crate) struct SceneContext<'a> {
    pub prepared: &'a PreparedScene,
    pub composition_contexts: &'a HashMap<String, ResolvedCompositionContext>,
    pub composition_consumers_by_source: &'a HashMap<String, Vec<String>>,
    pub draw_coord_size_by_pass: &'a HashMap<String, [f32; 2]>,
    pub asset_store: Option<&'a crate::asset_store::AssetStore>,
    /// The wgpu device (shared Arc).
    pub device: &'a Arc<wgpu::Device>,
    pub adapter: Option<&'a wgpu::Adapter>,
}

impl<'a> SceneContext<'a> {
    #[inline]
    pub fn scene(&self) -> &SceneDSL { &self.prepared.scene }

    #[inline]
    pub fn nodes_by_id(&self) -> &HashMap<String, Node> { &self.prepared.nodes_by_id }

    #[inline]
    pub fn ids(&self) -> &HashMap<String, ResourceName> { &self.prepared.ids }
}

/// Mutable builder state accumulated during pass assembly.
///
/// Fields are **references** into the assembler's local variables so that
/// a `BuilderState` can be constructed cheaply per-arm without moving
/// ownership away from the main loop.
pub(crate) struct BuilderState<'b> {
    // ---- target info (read-only) ----
    pub target_texture_name: &'b ResourceName,
    pub target_format: TextureFormat,
    pub sampled_pass_format: TextureFormat,
    pub tgt_size: [f32; 2],
    pub tgt_size_u: [u32; 2],

    // ---- resource collectors (mutable) ----
    pub geometry_buffers: &'b mut Vec<(ResourceName, Arc<[u8]>)>,
    pub instance_buffers: &'b mut Vec<(ResourceName, Arc<[u8]>)>,
    pub textures: &'b mut Vec<TextureDecl>,
    pub render_pass_specs: &'b mut Vec<RenderPassSpec>,
    pub composite_passes: &'b mut Vec<ResourceName>,
    pub depth_resolve_passes: &'b mut Vec<DepthResolvePass>,

    // ---- pass metadata ----
    pub pass_cull_mode_by_name: &'b mut HashMap<ResourceName, Option<wgpu::Face>>,
    pub pass_depth_attachment_by_name: &'b mut HashMap<ResourceName, ResourceName>,
    pub pass_output_registry: &'b mut PassOutputRegistry,
    pub sampled_pass_ids: &'b HashSet<String>,

    // ---- baked data parse bookkeeping ----
    pub baked_data_parse_meta_by_pass: &'b mut HashMap<String, Arc<BakedDataParseMeta>>,
    pub baked_data_parse_bytes_by_pass: &'b mut HashMap<String, Arc<[u8]>>,
    pub baked_data_parse_buffer_to_pass_id: &'b mut HashMap<ResourceName, String>,

    // ---- source-type pass tracking ----
    pub downsample_source_pass_ids: &'b mut HashSet<String>,
    pub upsample_source_pass_ids: &'b mut HashSet<String>,
    pub gaussian_source_pass_ids: &'b mut HashSet<String>,
    pub bloom_source_pass_ids: &'b mut HashSet<String>,
    pub gradient_source_pass_ids: &'b mut HashSet<String>,
}

/// Create a fullscreen geometry buffer (two-triangle quad covering `w × h` pixels).
pub(crate) fn make_fullscreen_geometry(w: f32, h: f32) -> Arc<[u8]> {
    let verts = rect2d_geometry_vertices(w, h);
    Arc::from(as_bytes_slice(&verts).to_vec())
}

impl<'b> BuilderState<'b> {
    /// Push a fullscreen geometry buffer.
    pub fn push_fullscreen_geometry(&mut self, name: ResourceName, w: f32, h: f32) {
        let bytes = make_fullscreen_geometry(w, h);
        self.geometry_buffers.push((name, bytes));
    }
}
