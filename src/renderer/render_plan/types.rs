use std::{collections::HashMap, path::PathBuf, sync::Arc};

use image::DynamicImage;
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color, TextureFormat},
};

use crate::renderer::{
    ShaderSpacePresentationMode,
    scene_prep::{PreparedScene, ScenePrepReport},
    types::{GraphBinding, Params, PassBindings, PassOutputRegistry},
};

#[derive(Clone, Debug, Default)]
pub(crate) struct PlanningGpuCaps {
    pub features: wgpu::Features,
    pub limits: wgpu::Limits,
}

#[derive(Clone, Debug)]
pub(crate) struct PlanBuildOptions {
    pub gpu_caps: PlanningGpuCaps,
    pub presentation_mode: ShaderSpacePresentationMode,
    pub debug_dump_wgsl_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassTextureBinding {
    /// ResourceName of the texture to bind.
    pub texture: ResourceName,
    /// If this binding refers to an ImageTexture node id, keep it here so the loader knows
    /// it must provide CPU image bytes.
    pub image_node_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct TextureDecl {
    pub name: ResourceName,
    pub size: [u32; 2],
    pub format: TextureFormat,
    pub sample_count: u32,
    /// When true, include `TEXTURE_BINDING` even for multi-sampled textures
    /// (e.g. depth attachments read by a depth-resolve pass via `textureLoad`).
    pub needs_sampling: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ImageTextureSpec {
    pub name: ResourceName,
    pub image: Arc<DynamicImage>,
    pub usage: wgpu::TextureUsages,
    pub srgb: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct RenderPassSpec {
    pub pass_id: String,
    pub name: ResourceName,
    pub geometry_buffer: ResourceName,
    pub instance_buffer: Option<ResourceName>,
    pub normals_buffer: Option<ResourceName>,
    pub target_texture: ResourceName,
    pub resolve_target: Option<ResourceName>,
    pub params_buffer: ResourceName,
    pub baked_data_parse_buffer: Option<ResourceName>,
    pub params: Params,
    pub graph_binding: Option<GraphBinding>,
    pub graph_values: Option<Vec<u8>>,
    pub shader_wgsl: String,
    pub texture_bindings: Vec<PassTextureBinding>,
    pub sampler_kinds: Vec<SamplerKind>,
    pub blend_state: BlendState,
    pub color_load_op: wgpu::LoadOp<Color>,
    pub sample_count: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct DepthResolvePass {
    pub pass_name: ResourceName,
    pub geometry_buffer: ResourceName,
    pub params_buffer: ResourceName,
    pub params: Params,
    pub depth_texture: ResourceName,
    pub dst_texture: ResourceName,
    pub shader_wgsl: String,
    pub is_multisampled: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ImagePrepass {
    pub pass_name: ResourceName,
    pub geometry_buffer: ResourceName,
    pub params_buffer: ResourceName,
    pub params: Params,
    pub src_texture: ResourceName,
    pub dst_texture: ResourceName,
    pub shader_wgsl: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SamplerKind {
    NearestClamp,
    NearestMirror,
    NearestRepeat,
    LinearMirror,
    LinearRepeat,
    LinearClamp,
}

#[derive(Clone, Debug)]
pub(crate) struct TextureCapabilityRequirement {
    pub name: ResourceName,
    pub format: TextureFormat,
    pub usage: wgpu::TextureUsages,
    pub sample_count: u32,
    pub sampled_by_passes: Vec<String>,
    pub blend_target_passes: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ResourcePlans {
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
    pub pass_output_registry: PassOutputRegistry,
    pub pass_bindings: Vec<PassBindings>,
    pub baked_data_parse_bytes_by_pass: HashMap<String, Arc<[u8]>>,
    pub baked_data_parse_buffer_to_pass_id: HashMap<ResourceName, String>,
}

#[derive(Clone, Debug)]
pub(crate) struct RenderPlan {
    pub prepared: PreparedScene,
    pub scene_report: ScenePrepReport,
    pub resolution: [u32; 2],
    pub scene_output_texture: ResourceName,
    pub present_output_texture: ResourceName,
    pub export_output_texture: ResourceName,
    pub export_encode_pass_name: Option<ResourceName>,
    pub resources: ResourcePlans,
    pub debug_dump_wgsl_dir: Option<PathBuf>,
}
