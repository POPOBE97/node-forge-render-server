use std::collections::HashMap;

use rust_wgpu_fiber::ResourceName;

pub const DRAW_PASS_NODE_TYPES: &[&str] = &[
    "RenderPass",
    "GuassianBlurPass",
    "Downsample",
    "Upsample",
    "GradientBlur",
];

pub fn is_draw_pass_node_type(node_type: &str) -> bool {
    DRAW_PASS_NODE_TYPES.contains(&node_type)
}

pub fn is_composition_route_node_type(node_type: &str) -> bool {
    node_type == "Composite"
}

pub fn is_pass_like_node_type(node_type: &str) -> bool {
    is_draw_pass_node_type(node_type) || is_composition_route_node_type(node_type)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeRole {
    DrawPass,
    CompositionRoute,
    Other,
}

#[derive(Clone, Debug)]
pub struct CoordDomain {
    pub composition_node_id: String,
    pub render_texture_node_id: String,
    pub texture_name: ResourceName,
    pub size_px: [f32; 2],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedGeometrySource {
    DirectGeometry(String),
    FullscreenFallback,
}

#[derive(Clone, Debug)]
pub struct ResolvedGeometry {
    pub size_px: [f32; 2],
    pub center_px: [f32; 2],
    pub source: ResolvedGeometrySource,
}

#[derive(Clone, Debug)]
pub struct ResolvedDrawContext {
    pub pass_node_id: String,
    pub downstream_node_id: String,
    pub downstream_port_id: String,
    pub coord_domain: CoordDomain,
    pub geometry: ResolvedGeometry,
}

#[derive(Clone, Debug)]
pub struct ResolvedCompositionContext {
    pub composition_node_id: String,
    pub target_texture_node_id: String,
    pub target_texture_name: ResourceName,
    pub target_size_px: [f32; 2],
    pub layer_node_ids: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ResolvedSceneContexts {
    pub node_roles: HashMap<String, NodeRole>,
    pub draw_contexts: Vec<ResolvedDrawContext>,
    pub composition_contexts: HashMap<String, ResolvedCompositionContext>,
    pub composition_consumers_by_source: HashMap<String, Vec<String>>,
}
