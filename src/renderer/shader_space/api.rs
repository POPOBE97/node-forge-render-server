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
            applied.insert(pass_name.to_string());
        }
    }

    for spec in &mut plan.resources.image_prepasses {
        let pass_name = spec.pass_name.as_str();
        if let Some(source) = overrides.get(pass_name) {
            spec.shader_wgsl = source.clone();
            applied.insert(pass_name.to_string());
        }
    }

    for spec in &mut plan.resources.depth_resolve_passes {
        let pass_name = spec.pass_name.as_str();
        if let Some(source) = overrides.get(pass_name) {
            spec.shader_wgsl = source.clone();
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

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use anyhow::Result;

    use super::apply_pass_shader_overrides;
    use crate::{
        asset_store, dsl,
        renderer::{
            ShaderSpacePresentationMode,
            render_plan::{
                planner::RenderPlanner,
                types::{PlanBuildOptions, PlanningGpuCaps},
            },
        },
    };

    fn load_case(case_name: &str) -> Result<(dsl::SceneDSL, Option<asset_store::AssetStore>)> {
        let archive = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("render")
            .join("editor-examples")
            .join(case_name)
            .join("scene.nforge");
        let (scene, asset_store) = asset_store::load_from_nforge(&archive)?;
        let assets = if scene.assets.is_empty() {
            None
        } else {
            Some(asset_store)
        };
        Ok((scene, assets))
    }

    #[test]
    fn shader_overrides_do_not_replace_pass_debug_sources() -> Result<()> {
        let (scene, assets) = load_case("graph-rectangle")?;
        let mut plan = RenderPlanner::new(PlanBuildOptions {
            gpu_caps: PlanningGpuCaps::default(),
            presentation_mode: ShaderSpacePresentationMode::UiSdrDisplayEncode,
            debug_dump_wgsl_dir: None,
        })
        .plan(&scene, assets.as_ref(), None)?;

        let pass_name = "node_2.pass";
        let canonical_debug_source = plan
            .pass_debug_sources
            .get(pass_name)
            .expect("canonical debug source")
            .module_source
            .clone();
        let override_source = format!(
            "{canonical_debug_source}\nfn shortwire_debug_root() -> f32 {{ return 1.0; }}\n"
        );

        apply_pass_shader_overrides(
            &mut plan,
            &HashMap::from([(pass_name.to_string(), override_source.clone())]),
            true,
        )?;

        let render_spec = plan
            .resources
            .render_pass_specs
            .iter()
            .find(|spec| spec.name.as_str() == pass_name)
            .expect("render pass spec");
        assert_eq!(render_spec.shader_wgsl, override_source);

        let debug_source = plan
            .pass_debug_sources
            .get(pass_name)
            .expect("debug source after override");
        assert_eq!(debug_source.module_source, canonical_debug_source);
        assert!(!debug_source.module_source.contains("shortwire_debug_root"));

        Ok(())
    }
}
