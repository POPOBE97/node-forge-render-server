use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use rust_wgpu_fiber::shader_space::ShaderSpace;
use rust_wgpu_fiber::{ResourceName, eframe::wgpu};

use crate::{dsl::SceneDSL, renderer::types::PassBindings};

use super::{assembler, error_space};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderSpacePresentationMode {
    SceneLinear,
    UiSdrDisplayEncode,
}

impl Default for ShaderSpacePresentationMode {
    fn default() -> Self {
        Self::SceneLinear
    }
}

#[derive(Clone, Debug, Default)]
pub struct ShaderSpaceBuildOptions {
    pub presentation_mode: ShaderSpacePresentationMode,
    pub debug_dump_wgsl_dir: Option<PathBuf>,
}

pub struct ShaderSpaceBuildResult {
    pub shader_space: ShaderSpace,
    pub resolution: [u32; 2],
    pub scene_output_texture: ResourceName,
    pub present_output_texture: ResourceName,
    pub pass_bindings: Vec<PassBindings>,
    pub pipeline_signature: [u8; 32],
}

pub struct ShaderSpaceBuilder {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    options: ShaderSpaceBuildOptions,
}

impl ShaderSpaceBuilder {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Self {
            device,
            queue,
            options: ShaderSpaceBuildOptions::default(),
        }
    }

    pub fn with_options(mut self, options: ShaderSpaceBuildOptions) -> Self {
        self.options = options;
        self
    }

    pub fn build(self, scene: &SceneDSL) -> Result<ShaderSpaceBuildResult> {
        let enable_display_encode =
            self.options.presentation_mode == ShaderSpacePresentationMode::UiSdrDisplayEncode;
        let (shader_space, resolution, scene_output_texture, pass_bindings, pipeline_signature) =
            assembler::build_shader_space_from_scene_internal(
                scene,
                self.device,
                self.queue,
                enable_display_encode,
                self.options.debug_dump_wgsl_dir.clone(),
            )?;

        let present_output_texture = if enable_display_encode {
            let maybe_display: ResourceName =
                format!("{}.present.sdr.srgb", scene_output_texture.as_str()).into();
            if shader_space.textures.get(maybe_display.as_str()).is_some() {
                maybe_display
            } else {
                scene_output_texture.clone()
            }
        } else {
            scene_output_texture.clone()
        };

        Ok(ShaderSpaceBuildResult {
            shader_space,
            resolution,
            scene_output_texture,
            present_output_texture,
            pass_bindings,
            pipeline_signature,
        })
    }

    pub fn build_error(self, resolution: [u32; 2]) -> Result<ShaderSpaceBuildResult> {
        let (shader_space, resolution, scene_output_texture, pass_bindings, pipeline_signature) =
            error_space::build_error_shader_space(self.device, self.queue, resolution)?;

        Ok(ShaderSpaceBuildResult {
            shader_space,
            resolution,
            present_output_texture: scene_output_texture.clone(),
            scene_output_texture,
            pass_bindings,
            pipeline_signature,
        })
    }
}
