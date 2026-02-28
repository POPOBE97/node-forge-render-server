use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use rust_wgpu_fiber::shader_space::ShaderSpace;
use rust_wgpu_fiber::{ResourceName, eframe::wgpu};

use crate::{asset_store::AssetStore, dsl::SceneDSL, renderer::types::PassBindings};

use super::{assembler, error_space};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderSpacePresentationMode {
    SceneLinear,
    UiSdrDisplayEncode,
    /// HDR-native UI mode: the wgpu surface is `Rgba16Float` (macOS EDR).
    /// No display-encode pass is created; the scene output texture is
    /// registered directly with egui.  Values > 1.0 are preserved and
    /// shown as EDR brightness on HDR-capable displays.
    UiHdrNative,
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
    adapter: Option<wgpu::Adapter>,
    options: ShaderSpaceBuildOptions,
    asset_store: Option<AssetStore>,
}

impl ShaderSpaceBuilder {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Self {
            device,
            queue,
            adapter: None,
            options: ShaderSpaceBuildOptions::default(),
            asset_store: None,
        }
    }

    pub fn with_adapter(mut self, adapter: wgpu::Adapter) -> Self {
        self.adapter = Some(adapter);
        self
    }

    pub fn with_options(mut self, options: ShaderSpaceBuildOptions) -> Self {
        self.options = options;
        self
    }

    pub fn with_asset_store(mut self, store: AssetStore) -> Self {
        self.asset_store = Some(store);
        self
    }

    pub fn build(self, scene: &SceneDSL) -> Result<ShaderSpaceBuildResult> {
        let presentation_mode = self.options.presentation_mode;
        // Enable the display-encode pass for any mode that needs sRGB-encoded
        // output.  UiHdrNative needs it so clipboard copy / file export get
        // correct gamma, and UiSdrDisplayEncode needs it for the legacy path.
        // SceneLinear (tests, raw pipeline inspection) deliberately skips it.
        let enable_display_encode = matches!(
            presentation_mode,
            ShaderSpacePresentationMode::UiSdrDisplayEncode
                | ShaderSpacePresentationMode::UiHdrNative
        );
        let (shader_space, resolution, scene_output_texture, pass_bindings, pipeline_signature) =
            assembler::build_shader_space_from_scene_internal(
                scene,
                self.device,
                self.queue,
                self.adapter.as_ref(),
                enable_display_encode,
                self.options.debug_dump_wgsl_dir.clone(),
                self.asset_store.as_ref(),
                presentation_mode,
            )?;

        let present_output_texture = if enable_display_encode {
            // Try HDR gamma-encoded presentation texture first, then SDR sRGB.
            let hdr_name: ResourceName =
                format!("{}.present.hdr.gamma", scene_output_texture.as_str()).into();
            let sdr_name: ResourceName =
                format!("{}.present.sdr.srgb", scene_output_texture.as_str()).into();
            if shader_space.textures.get(hdr_name.as_str()).is_some() {
                hdr_name
            } else if shader_space.textures.get(sdr_name.as_str()).is_some() {
                sdr_name
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
