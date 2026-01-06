use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use crossbeam_channel::Receiver;
use rust_wgpu_fiber::{
    eframe::{
        self,
        egui::{self, pos2, Color32, Rect, TextureId},
        wgpu,
    },
    shader_space::ShaderSpace,
    ResourceName,
};

use crate::{protocol, renderer, ws};

pub struct App {
    pub shader_space: ShaderSpace,
    pub resolution: [u32; 2],
    pub window_resolution: [u32; 2],
    pub output_texture_name: ResourceName,
    pub color_attachment: Option<TextureId>,
    pub start: Instant,
    pub passes: Vec<renderer::PassBindings>,

    pub scene_rx: Receiver<ws::SceneUpdate>,
    pub ws_hub: ws::WsHub,
    pub last_good: Arc<Mutex<Option<crate::dsl::SceneDSL>>>,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let render_state = frame.wgpu_render_state().unwrap();

        // Apply latest scene update (non-blocking; drop older updates).
        let mut latest: Option<ws::SceneUpdate> = None;
        while let Ok(update) = self.scene_rx.try_recv() {
            latest = Some(update);
        }

        if let Some(update) = latest {
            match update {
                ws::SceneUpdate::Parsed { scene, request_id } => {
                    if let Some([w, h]) = crate::dsl::screen_resolution(&scene) {
                        if [w, h] != self.window_resolution {
                            self.window_resolution = [w, h];
                            let size = egui::vec2(w as f32, h as f32);
                            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(size));
                        }
                    }

                    match renderer::build_shader_space_from_scene(
                        &scene,
                        Arc::new(render_state.device.clone()),
                        Arc::new(render_state.queue.clone()),
                    ) {
                        Ok((shader_space, resolution, output_texture_name, passes)) => {
                            self.shader_space = shader_space;
                            self.resolution = resolution;
                            self.output_texture_name = output_texture_name;
                            self.passes = passes;
                            self.color_attachment = None;

                            if let Ok(mut g) = self.last_good.lock() {
                                *g = Some(scene);
                            }
                        }
                        Err(e) => {
                            let msg = protocol::WSMessage {
                                msg_type: "error".to_string(),
                                timestamp: protocol::now_millis(),
                                request_id,
                                payload: Some(protocol::ErrorPayload {
                                    code: "VALIDATION_ERROR".to_string(),
                                    message: format!("{e:#}"),
                                }),
                            };
                            if let Ok(text) = serde_json::to_string(&msg) {
                                self.ws_hub.broadcast(text);
                            }

                            if let Ok((shader_space, resolution, output_texture_name, passes)) =
                                renderer::build_error_shader_space(
                                    Arc::new(render_state.device.clone()),
                                    Arc::new(render_state.queue.clone()),
                                    self.resolution,
                                )
                            {
                                self.shader_space = shader_space;
                                self.resolution = resolution;
                                self.output_texture_name = output_texture_name;
                                self.passes = passes;
                                self.color_attachment = None;
                            }
                        }
                    }
                }
                ws::SceneUpdate::ParseError { .. } => {
                    if let Ok((shader_space, resolution, output_texture_name, passes)) =
                        renderer::build_error_shader_space(
                            Arc::new(render_state.device.clone()),
                            Arc::new(render_state.queue.clone()),
                            self.resolution,
                        )
                    {
                        self.shader_space = shader_space;
                        self.resolution = resolution;
                        self.output_texture_name = output_texture_name;
                        self.passes = passes;
                        self.color_attachment = None;
                    }
                }
            }
        }

        let t = self.start.elapsed().as_secs_f32();
        for pass in &mut self.passes {
            let mut p = pass.base_params;
            p.time = t;
            let _ = renderer::update_pass_params(&self.shader_space, pass, &p);
        }

        self.shader_space.render();

        if self.color_attachment.is_none() {
            let mut renderer = render_state.renderer.as_ref().write();
            let texture = self
                .shader_space
                .textures
                .get(self.output_texture_name.as_str())
                .unwrap_or_else(|| {
                    panic!("output texture not found: {}", self.output_texture_name)
                });
            self.color_attachment = Some(renderer.register_native_texture(
                &render_state.device,
                texture.wgpu_texture_view.as_ref().unwrap(),
                wgpu::FilterMode::Linear,
            ));
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.centered_and_justified(|ui| {
                ui.painter().image(
                    self.color_attachment.unwrap(),
                    Rect::from_min_max(
                        pos2(0.0, 0.0),
                        pos2(self.resolution[0] as f32, self.resolution[1] as f32),
                    ),
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            });
        });

        // Continuous rendering mode: keep repainting even without input events.
        ctx.request_repaint();
    }
}
