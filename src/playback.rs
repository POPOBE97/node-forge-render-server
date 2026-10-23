//! Platform-independent playback core shared by the native app and WebRunner shell.

use std::sync::Arc;

use anyhow::{Context, Result};
use rust_wgpu_fiber::{ResourceName, wgpu, shader_space::ShaderSpace};

use crate::{
    animation::AnimationSession,
    asset_store::AssetStore,
    dsl::SceneDSL,
    protocol::InteractionEventPayload,
    renderer::{
        ShaderSpaceBuildOptions, ShaderSpaceBuildResult, ShaderSpaceBuilder,
        ShaderSpacePresentationMode, graph_uniforms, update_pass_params,
    },
    state_machine,
};

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackFrame {
    pub scene_time_seconds: f64,
    pub current_state_id: Option<String>,
    pub ended: bool,
    pub diagnostics: Vec<String>,
}

/// CPU-only planning diagnostics, not an allocation limit or GPU-time estimate.
#[derive(Debug, Clone)]
pub struct PlaybackResourceEstimate {
    pub texture_bytes: u64,
    pub pass_pixels: u64,
    pub pass_count: usize,
    pub largest_texture_dimension: u32,
}

pub struct PlaybackRuntime {
    authored_scene: SceneDSL,
    uniform_scene: SceneDSL,
    animation: Option<AnimationSession>,
    render: ShaderSpaceBuildResult,
    playing: bool,
    static_time_seconds: f64,
    dirty: bool,
}

impl PlaybackRuntime {
    /// Plan and decode resources without creating a GPU device or submitting work.
    /// Uses conservative storage accounting; pipelines, driver allocations and
    /// future dynamic graph changes are not bounded by this estimate.
    pub fn estimate_resources(scene: &SceneDSL, assets: &AssetStore) -> Result<PlaybackResourceEstimate> {
        use crate::renderer::render_plan::{planner::RenderPlanner, types::{PlanBuildOptions, PlanningGpuCaps}};
        let plan = RenderPlanner::new(PlanBuildOptions {
            gpu_caps: PlanningGpuCaps::default(),
            presentation_mode: ShaderSpacePresentationMode::UiSdrDisplayEncode,
            debug_dump_wgsl_dir: None,
        }).plan(scene, Some(assets), None)?;
        let resources = &plan.resources;
        let mut bytes = 0u64;
        let mut largest = 0u32;
        for texture in &resources.textures {
            largest = largest.max(texture.size[0]).max(texture.size[1]);
            let block = texture.format.block_copy_size(None).context("Cannot estimate texture format")? as u64;
            let (bw, bh) = texture.format.block_dimensions();
            let levels = resources.texture_mip_level_counts.get(&texture.name).copied().unwrap_or(1);
            for level in 0..levels {
                let w = (texture.size[0] >> level).max(1).div_ceil(bw) as u64;
                let h = (texture.size[1] >> level).max(1).div_ceil(bh) as u64;
                bytes = bytes.saturating_add(w.saturating_mul(h).saturating_mul(block).saturating_mul(texture.sample_count.max(1) as u64));
            }
        }
        for image in &resources.image_textures {
            let w = image.image.width(); let h = image.image.height();
            largest = largest.max(w).max(h);
            // RGBA32F plus mip/transfer headroom, regardless of source encoding.
            bytes = bytes.saturating_add(u64::from(w).saturating_mul(u64::from(h)).saturating_mul(32));
        }
        let mut pixels = 0u64;
        for pass in &resources.render_pass_specs {
            let target = resources.textures.iter().find(|texture| texture.name == pass.target_texture)
                .context("Cannot estimate render target")?;
            pixels = pixels.saturating_add(u64::from(target.size[0]).saturating_mul(u64::from(target.size[1]))
                .saturating_mul(pass.sample_count.max(1) as u64));
        }
        Ok(PlaybackResourceEstimate { texture_bytes: bytes, pass_pixels: pixels,
            pass_count: resources.render_pass_specs.len() + resources.image_prepasses.len() + resources.depth_resolve_passes.len(),
            largest_texture_dimension: largest })
    }

    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        adapter: wgpu::Adapter,
        scene: SceneDSL,
        assets: AssetStore,
    ) -> Result<Self> {
        Self::new_with_presentation(device, queue, adapter, scene, assets, ShaderSpacePresentationMode::UiSdrDisplayEncode)
    }

    pub fn new_with_presentation(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        adapter: wgpu::Adapter,
        scene: SceneDSL,
        assets: AssetStore,
        presentation_mode: ShaderSpacePresentationMode,
    ) -> Result<Self> {
        let animation = AnimationSession::from_scene(&scene)?;
        let render = ShaderSpaceBuilder::new(device, queue)
            .with_adapter(adapter)
            .with_asset_store(assets)
            .with_options(ShaderSpaceBuildOptions {
                presentation_mode,
                ..Default::default()
            })
            .build(&scene)
            .context("failed to build playback render graph")?;
        let uniform_scene = render
            .prepared_scene
            .clone()
            .unwrap_or_else(|| scene.clone());
        Ok(Self {
            authored_scene: scene,
            uniform_scene,
            animation,
            render,
            playing: false,
            static_time_seconds: 0.0,
            dirty: true,
        })
    }

    pub fn play(&mut self) {
        self.playing = true;
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn resolution(&self) -> [u32; 2] {
        self.render.resolution
    }

    pub fn current_state_id(&self) -> Option<&str> {
        self.animation
            .as_ref()
            .map(|animation| animation.runtime().current_state_id())
    }

    pub fn present_texture_name(&self) -> &ResourceName {
        &self.render.present_output_texture
    }

    pub fn present_texture_view(&self) -> Option<&wgpu::TextureView> {
        self.render
            .shader_space
            .textures
            .get(self.render.present_output_texture.as_str())?
            .wgpu_texture_view
            .as_ref()
    }

    pub fn shader_space(&self) -> &ShaderSpace {
        &self.render.shader_space
    }

    pub fn diagnostic_snapshot(&self) -> serde_json::Value {
        let uniforms: std::collections::BTreeMap<_, _> = self.uniform_scene.nodes.iter()
            .map(|node| (node.id.as_str(), &node.params)).collect();
        serde_json::json!({"stateId":self.current_state_id(),"sceneTime":self.animation.as_ref().map_or(self.static_time_seconds, AnimationSession::scene_time),"uniforms":uniforms})
    }

    pub fn scene_output_texture_name(&self) -> &ResourceName {
        &self.render.scene_output_texture
    }

    pub fn working_color_space(&self) -> crate::color::WorkingColorSpace {
        self.authored_scene.working_color_space
    }

    /// Capture the scene before display encoding, retaining HDR and linearizing sRGB storage.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture_linear_rgba(&self) -> Result<(u32, u32, Vec<f32>)> {
        let space = self.shader_space();
        let name = self.scene_output_texture_name().as_str();
        let texture = space.textures.get(name).context("Scene output texture missing")?;
        Ok(match texture.wgpu_texture_desc.format {
            wgpu::TextureFormat::Rgba16Float => {
                let image = space.read_texture_rgba16f(name)?;
                (image.width, image.height, image.channels)
            }
            wgpu::TextureFormat::Rgba32Float => {
                let image = space.read_texture_rgba32f(name)?;
                (image.width, image.height, image.channels)
            }
            format @ (wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb) => {
                let image = space.read_texture_rgba8(name)?;
                let channels = image.bytes.iter().enumerate().map(|(i, v)| {
                    let value = *v as f32 / 255.0;
                    if format.is_srgb() && i % 4 != 3 { crate::color::srgb_to_linear_channel(value) } else { value }
                }).collect();
                (image.width, image.height, channels)
            }
            format => anyhow::bail!("Unsupported scene capture format {format:?}"),
        })
    }

    /// Apply a uniform-only edit without recreating Motion cursors or losing velocities.
    /// Structural edits return false and are built transactionally by the caller.
    /// The caller must verify that Function and material sources are unchanged.
    pub fn update_uniform_scene(&mut self, scene: &SceneDSL) -> Result<bool> {
        if serde_json::to_value(&scene.state_machine)? != serde_json::to_value(&self.authored_scene.state_machine)? {
            return Ok(false);
        }
        // Match RenderPlanner's execution scene, including synthesized texture/pass bridges.
        // Preparing the authored graph directly rejects valid ImageTexture pass inputs.
        let mut execution_scene = scene.clone();
        crate::renderer::render_plan::coercion::plan_consumer_scoped_coercions(&mut execution_scene)?;
        let prepared = crate::renderer::prepare_scene(&execution_scene)?;
        let signature = graph_uniforms::compute_pipeline_signature_for_pass_bindings(&prepared.scene, &self.render.pass_bindings);
        // A rejected material transaction can increment the registry generation while
        // restoring exactly the original sources. Compare both scenes in that generation.
        let baseline = self.render.prepared_scene.as_ref().unwrap_or(&self.authored_scene);
        if signature != graph_uniforms::compute_pipeline_signature_for_pass_bindings(baseline, &self.render.pass_bindings) { return Ok(false); }
        if let Some(animation) = &mut self.animation {
            let updates = state_machine::collect_scene_current_values(scene).into_iter().collect::<Vec<_>>();
            animation.update_base_values(&updates);
        }
        self.authored_scene = scene.clone();
        self.render.prepared_scene = Some(prepared.scene.clone());
        self.uniform_scene = prepared.scene;
        self.dirty = true;
        Ok(true)
    }

    pub fn dispatch(&mut self, event: InteractionEventPayload) {
        if let Some(animation) = &mut self.animation {
            if let Some(position) = event.data.as_ref().and_then(|data| data.position.as_ref()) {
                animation.update_mouse_position(state_machine::MousePosition {
                    x: f64::from(position.frag_pixel_x.unwrap_or(position.canvas_x)),
                    y: f64::from(position.frag_pixel_y.unwrap_or(position.canvas_y)),
                });
            }
            animation.fire_event(&event);
            self.dirty = true;
        }
    }

    /// Force playback to a selectable authored State and render its initial frame.
    pub fn force_state(&mut self, state_id: &str) -> Result<PlaybackFrame> {
        self.animation
            .as_mut()
            .context("scene has no State Machine")?
            .force_state(state_id)?;
        self.dirty = true;
        self.advance(0.0)
    }

    pub fn reset(&mut self) -> Result<PlaybackFrame> {
        self.playing = false;
        self.static_time_seconds = 0.0;
        self.uniform_scene = self
            .render
            .prepared_scene
            .clone()
            .unwrap_or_else(|| self.authored_scene.clone());
        if let Some(animation) = &mut self.animation {
            let restores = animation.reset();
            state_machine::apply_overrides(&mut self.uniform_scene, &restores);
        }
        self.dirty = true;
        self.advance(0.0)
    }

    /// Advance every motion driver exactly once with the complete supplied `dt`.
    pub fn advance(&mut self, delta_seconds: f64) -> Result<PlaybackFrame> {
        self.advance_internal(delta_seconds, None)
    }

    /// Advance once and defer GPU submission to the owning multi-scene host.
    pub fn advance_deferred(&mut self, delta_seconds: f64, commands: &mut Vec<wgpu::CommandBuffer>) -> Result<PlaybackFrame> {
        self.advance_internal(delta_seconds, Some(commands))
    }

    fn advance_internal(&mut self, delta_seconds: f64, commands: Option<&mut Vec<wgpu::CommandBuffer>>) -> Result<PlaybackFrame> {
        let dt = accepted_delta(delta_seconds, self.playing);
        self.static_time_seconds += dt;
        let mut ended = false;
        let mut diagnostics = Vec::new();
        let mut scene_time_seconds = self.static_time_seconds;
        if let Some(animation) = &mut self.animation {
            let step = animation.step(dt);
            scene_time_seconds = step.scene_time_secs;
            ended = step.finished;
            diagnostics = step.diagnostics;
            if step.needs_redraw || dt == 0.0 || self.dirty {
                state_machine::apply_overrides(&mut self.uniform_scene, &step.active_overrides);
                self.dirty = true;
            }
        }
        if self.dirty || dt > 0.0 {
            if self.dirty && !self.render.gaussian_blur_bundles.is_empty() {
                crate::renderer::shader_space::dynamic_gaussian_blur::update_dynamic_gaussian_blur_bundles(
                    &self.uniform_scene,
                    &mut self.render.shader_space,
                    &mut self.render.pass_bindings,
                    &mut self.render.gaussian_blur_bundles,
                    &std::collections::HashMap::new(),
                )?;
            }
            graph_uniforms::apply_graph_uniform_updates(
                &mut self.render.pass_bindings,
                &mut self.render.shader_space,
                &self.uniform_scene,
            )?;
            for pass in &self.render.pass_bindings {
                let mut params = pass.base_params;
                params.time = scene_time_seconds as f32;
                update_pass_params(&self.render.shader_space, pass, &params)?;
            }
            if let Some(commands) = commands {
                self.render.shader_space.render_deferred(commands);
            } else {
                self.render.shader_space.render();
            }
            self.dirty = false;
        }
        if ended {
            self.playing = false;
        }
        Ok(PlaybackFrame {
            scene_time_seconds,
            current_state_id: self.current_state_id().map(str::to_owned),
            ended,
            diagnostics,
        })
    }
}

fn accepted_delta(delta_seconds: f64, playing: bool) -> f64 {
    if playing && delta_seconds.is_finite() && delta_seconds > 0.0 {
        delta_seconds
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::accepted_delta;

    #[test]
    fn playback_uses_complete_frame_delta_without_clamping() {
        assert_eq!(accepted_delta(0.75, true), 0.75);
        assert_eq!(accepted_delta(0.75, false), 0.0);
        assert_eq!(accepted_delta(f64::NAN, true), 0.0);
        assert_eq!(accepted_delta(-1.0, true), 0.0);
    }
}
