use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::{Result, bail};
use rust_wgpu_fiber::shader_space::ShaderSpace;
use rust_wgpu_fiber::{ResourceName, eframe::wgpu};

use crate::{
    asset_store::AssetStore,
    dsl::SceneDSL,
    renderer::{
        pass_debug::PassDebugSource,
        render_plan::{
            planner::RenderPlanner,
            types::{PlanBuildOptions, PlanningGpuCaps},
        },
        types::PassBindings,
    },
};

use super::{error_space, finalizer::ShaderSpaceFinalizer};

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
    pub pass_shader_overrides: HashMap<String, String>,
    pub strict_pass_shader_overrides: bool,
}

pub struct ShaderSpaceBuildResult {
    pub shader_space: ShaderSpace,
    pub resolution: [u32; 2],
    pub scene_output_texture: ResourceName,
    /// Texture for on-screen display (registered with egui).
    pub present_output_texture: ResourceName,
    /// Texture for clipboard copy and headless PNG export.
    /// Contains sRGB-encoded bytes suitable for `read_texture_rgba8`.
    pub export_output_texture: ResourceName,
    /// Pass name for the on-demand SDR sRGB encode (UiHdrNative only).
    /// When `Some`, the pass is registered but excluded from per-frame
    /// composition — it must be executed via `render_pass_by_name` before
    /// reading `export_output_texture`.
    pub export_encode_pass_name: Option<ResourceName>,
    pub pass_bindings: Vec<PassBindings>,
    pub pipeline_signature: [u8; 32],
    pub pass_debug_sources: HashMap<String, PassDebugSource>,
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
        let plan_options = PlanBuildOptions {
            gpu_caps: PlanningGpuCaps {
                features: self.device.features(),
                limits: self.device.limits().clone(),
            },
            presentation_mode: self.options.presentation_mode,
            debug_dump_wgsl_dir: self.options.debug_dump_wgsl_dir.clone(),
        };
        let mut plan = RenderPlanner::new(plan_options).plan(
            scene,
            self.asset_store.as_ref(),
            self.adapter.as_ref(),
        )?;
        apply_pass_shader_overrides(
            &mut plan,
            &self.options.pass_shader_overrides,
            self.options.strict_pass_shader_overrides,
        )?;
        let finalized =
            ShaderSpaceFinalizer::finalize(&plan, self.device, self.queue, self.adapter.as_ref())?;

        Ok(ShaderSpaceBuildResult {
            shader_space: finalized.shader_space,
            resolution: plan.resolution,
            scene_output_texture: plan.scene_output_texture,
            present_output_texture: plan.present_output_texture,
            export_output_texture: plan.export_output_texture,
            export_encode_pass_name: plan.export_encode_pass_name,
            pass_bindings: finalized.pass_bindings,
            pipeline_signature: finalized.pipeline_signature,
            pass_debug_sources: plan.pass_debug_sources,
        })
    }

    pub fn build_error(self, resolution: [u32; 2]) -> Result<ShaderSpaceBuildResult> {
        let (shader_space, resolution, scene_output_texture, pass_bindings, pipeline_signature) =
            error_space::build_error_shader_space(self.device, self.queue, resolution)?;

        Ok(ShaderSpaceBuildResult {
            shader_space,
            resolution,
            present_output_texture: scene_output_texture.clone(),
            export_output_texture: scene_output_texture.clone(),
            export_encode_pass_name: None,
            scene_output_texture,
            pass_bindings,
            pipeline_signature,
            pass_debug_sources: HashMap::new(),
        })
    }
}

fn apply_pass_shader_overrides(
    plan: &mut crate::renderer::render_plan::types::RenderPlan,
    overrides: &HashMap<String, String>,
    strict: bool,
) -> Result<()> {
    if overrides.is_empty() {
        return Ok(());
    }

    let mut applied = std::collections::HashSet::<String>::new();
    for spec in &mut plan.resources.render_pass_specs {
        let pass_name = spec.name.as_str();
        if let Some(source) = overrides.get(pass_name) {
            spec.shader_wgsl = source.clone();
            plan.pass_debug_sources.insert(
                pass_name.to_string(),
                PassDebugSource::from_wgsl(pass_name, source.clone()),
            );
            applied.insert(pass_name.to_string());
        }
    }

    for spec in &mut plan.resources.image_prepasses {
        let pass_name = spec.pass_name.as_str();
        if let Some(source) = overrides.get(pass_name) {
            spec.shader_wgsl = source.clone();
            plan.pass_debug_sources.insert(
                pass_name.to_string(),
                PassDebugSource::from_wgsl(pass_name, source.clone()),
            );
            applied.insert(pass_name.to_string());
        }
    }

    for spec in &mut plan.resources.depth_resolve_passes {
        let pass_name = spec.pass_name.as_str();
        if let Some(source) = overrides.get(pass_name) {
            spec.shader_wgsl = source.clone();
            plan.pass_debug_sources.insert(
                pass_name.to_string(),
                PassDebugSource::from_wgsl(pass_name, source.clone()),
            );
            applied.insert(pass_name.to_string());
        }
    }

    if strict && applied.len() != overrides.len() {
        let mut missing = overrides
            .keys()
            .filter(|pass_name| !applied.contains(pass_name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        missing.sort();
        bail!(
            "shader override did not match any current render pass: {}",
            missing.join(", ")
        );
    }

    Ok(())
}
