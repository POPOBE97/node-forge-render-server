//! Platform-independent playback core shared by the native app and WebRunner shell.

use std::sync::Arc;

use anyhow::{Context, Result};
use rust_wgpu_fiber::{ResourceName, eframe::wgpu, shader_space::ShaderSpace};

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
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        adapter: wgpu::Adapter,
        scene: SceneDSL,
        assets: AssetStore,
    ) -> Result<Self> {
        let animation = AnimationSession::from_scene(&scene)?;
        let render = ShaderSpaceBuilder::new(device, queue)
            .with_adapter(adapter)
            .with_asset_store(assets)
            .with_options(ShaderSpaceBuildOptions {
                presentation_mode: ShaderSpacePresentationMode::UiSdrDisplayEncode,
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
            self.render.shader_space.render();
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
