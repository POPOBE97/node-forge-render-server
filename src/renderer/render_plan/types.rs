use std::{collections::HashMap, path::PathBuf, sync::Arc};

use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color, TextureFormat},
};

use crate::renderer::types::{Params, PassBindings};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentationMode {
    SceneLinear,
    UiSdrDisplayEncode,
}

impl Default for PresentationMode {
    fn default() -> Self {
        Self::SceneLinear
    }
}

#[derive(Clone, Debug, Default)]
pub struct PlanBuildOptions {
    pub presentation_mode: PresentationMode,
    pub debug_dump_wgsl_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplerKind {
    NearestClamp,
    LinearMirror,
    LinearRepeat,
    LinearClamp,
}

#[derive(Clone, Debug)]
pub struct PassTextureBinding {
    /// ResourceName of the texture to bind.
    pub texture: ResourceName,
    /// If this binding refers to an ImageTexture node id, keep it here so the loader knows
    /// it must provide CPU image bytes.
    pub image_node_id: Option<String>,
}

#[derive(Clone)]
pub struct TextureDecl {
    pub name: ResourceName,
    pub size: [u32; 2],
    pub format: TextureFormat,
}

#[derive(Clone)]
pub struct RenderPassSpec {
    pub name: ResourceName,
    pub geometry_buffer: ResourceName,
    pub instance_buffer: Option<ResourceName>,
    pub target_texture: ResourceName,
    pub params_buffer: ResourceName,
    pub baked_data_parse_buffer: Option<ResourceName>,
    pub params: Params,
    pub shader_wgsl: String,
    pub texture_bindings: Vec<PassTextureBinding>,
    pub sampler_kind: SamplerKind,
    pub blend_state: BlendState,
    pub color_load_op: wgpu::LoadOp<Color>,
}

#[derive(Clone)]
pub struct ImagePrepassSpec {
    pub pass_name: ResourceName,
    pub geometry_buffer: ResourceName,
    pub params_buffer: ResourceName,
    pub params: Params,
    pub src_texture: ResourceName,
    pub dst_texture: ResourceName,
    pub shader_wgsl: String,
}

#[derive(Clone, Default)]
pub struct ResourcePlans {
    pub geometry_buffers: Vec<(ResourceName, Arc<[u8]>)>,
    pub instance_buffers: Vec<(ResourceName, Arc<[u8]>)>,
    pub textures: Vec<TextureDecl>,
    pub render_pass_specs: Vec<RenderPassSpec>,
    pub image_prepasses: Vec<ImagePrepassSpec>,
    pub composite_passes: Vec<ResourceName>,
    pub pass_bindings: Vec<PassBindings>,
    pub baked_data_parse_bytes_by_pass: HashMap<String, Arc<[u8]>>,
    pub baked_data_parse_buffer_to_pass_id: HashMap<ResourceName, String>,
}

#[derive(Clone)]
pub struct RenderPlan {
    pub resolution: [u32; 2],
    pub scene_output_texture: ResourceName,
    pub present_output_texture: ResourceName,
    pub resources: ResourcePlans,
    pub debug_dump_wgsl_dir: Option<PathBuf>,
}
