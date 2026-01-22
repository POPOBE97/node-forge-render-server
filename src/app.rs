use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use crossbeam_channel::Receiver;
use rust_wgpu_fiber::{
    ResourceName,
    eframe::{
        self,
        egui::{self, Color32, Rect, TextureId, pos2},
        wgpu,
    },
    shader_space::ShaderSpace,
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
        let mut renderer_guard = frame.wgpu_render_state().unwrap().renderer.as_ref().write();
        let mut did_rebuild_shader_space = false;

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

                    let build_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        renderer::build_shader_space_from_scene_for_ui(
                            &scene,
                            Arc::new(render_state.device.clone()),
                            Arc::new(render_state.queue.clone()),
                        )
                    }));

                    match build_result {
                        Ok(Ok((shader_space, resolution, output_texture_name, passes))) => {
                            self.shader_space = shader_space;
                            self.resolution = resolution;
                            // In UI mode, prefer the derived display texture when present.
                            // It contains sRGB-encoded bytes in a linear UNORM texture so it looks
                            // correct when presented via egui-wgpu's non-sRGB swapchain.
                            let mut display_name = output_texture_name.clone();
                            let maybe_display: ResourceName =
                                format!("{}.present.sdr.srgb", output_texture_name.as_str()).into();
                            if self
                                .shader_space
                                .textures
                                .get(maybe_display.as_str())
                                .is_some()
                            {
                                display_name = maybe_display;
                            }
                            self.output_texture_name = display_name;
                            self.passes = passes;

                            // Keep a stable egui TextureId across scene refreshes.
                            // If the underlying wgpu TextureView changes (new ShaderSpace),
                            // update the existing TextureId instead of allocating a new one.
                            let texture = self
                                .shader_space
                                .textures
                                .get(self.output_texture_name.as_str())
                                .unwrap_or_else(|| {
                                    panic!(
                                        "output texture not found: {}",
                                        self.output_texture_name
                                    )
                                });
                            if let Some(id) = self.color_attachment {
                                renderer_guard.update_egui_texture_from_wgpu_texture(
                                    &render_state.device,
                                    texture.wgpu_texture_view.as_ref().unwrap(),
                                    wgpu::FilterMode::Linear,
                                    id,
                                );
                            } else {
                                self.color_attachment = Some(renderer_guard.register_native_texture(
                                    &render_state.device,
                                    texture.wgpu_texture_view.as_ref().unwrap(),
                                    wgpu::FilterMode::Linear,
                                ));
                            }

                            did_rebuild_shader_space = true;

                            if let Ok(mut g) = self.last_good.lock() {
                                *g = Some(scene);
                            }
                        }
                        Ok(Err(e)) => {
                            let message = format!("{e:#}");
                            eprintln!("[error-plane] scene build failed: {message}");

                            let msg = protocol::WSMessage {
                                msg_type: "error".to_string(),
                                timestamp: protocol::now_millis(),
                                request_id,
                                payload: Some(protocol::ErrorPayload {
                                    code: "VALIDATION_ERROR".to_string(),
                                    message,
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

                                let texture = self
                                    .shader_space
                                    .textures
                                    .get(self.output_texture_name.as_str())
                                    .unwrap_or_else(|| {
                                        panic!(
                                            "output texture not found: {}",
                                            self.output_texture_name
                                        )
                                    });
                                if let Some(id) = self.color_attachment {
                                    renderer_guard.update_egui_texture_from_wgpu_texture(
                                        &render_state.device,
                                        texture.wgpu_texture_view.as_ref().unwrap(),
                                        wgpu::FilterMode::Linear,
                                        id,
                                    );
                                } else {
                                    self.color_attachment = Some(renderer_guard.register_native_texture(
                                        &render_state.device,
                                        texture.wgpu_texture_view.as_ref().unwrap(),
                                        wgpu::FilterMode::Linear,
                                    ));
                                }

                                did_rebuild_shader_space = true;
                            }
                        }
                        Err(panic_payload) => {
                            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                                (*s).to_string()
                            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "(non-string panic payload)".to_string()
                            };

                            let message =
                                format!("scene build panicked; showing error plane: {panic_msg}");
                            eprintln!("{message}");

                            let msg = protocol::WSMessage {
                                msg_type: "error".to_string(),
                                timestamp: protocol::now_millis(),
                                request_id,
                                payload: Some(protocol::ErrorPayload {
                                    code: "PANIC".to_string(),
                                    message,
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

                                let texture = self
                                    .shader_space
                                    .textures
                                    .get(self.output_texture_name.as_str())
                                    .unwrap_or_else(|| {
                                        panic!(
                                            "output texture not found: {}",
                                            self.output_texture_name
                                        )
                                    });
                                if let Some(id) = self.color_attachment {
                                    renderer_guard.update_egui_texture_from_wgpu_texture(
                                        &render_state.device,
                                        texture.wgpu_texture_view.as_ref().unwrap(),
                                        wgpu::FilterMode::Linear,
                                        id,
                                    );
                                } else {
                                    self.color_attachment = Some(renderer_guard.register_native_texture(
                                        &render_state.device,
                                        texture.wgpu_texture_view.as_ref().unwrap(),
                                        wgpu::FilterMode::Linear,
                                    ));
                                }

                                did_rebuild_shader_space = true;
                            }
                        }
                    }
                }
                ws::SceneUpdate::ParseError {
                    message,
                    request_id,
                } => {
                    eprintln!("[error-plane] scene parse error: {message}");

                    let msg = protocol::WSMessage {
                        msg_type: "error".to_string(),
                        timestamp: protocol::now_millis(),
                        request_id,
                        payload: Some(protocol::ErrorPayload {
                            code: "PARSE_ERROR".to_string(),
                            message,
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

                        let texture = self
                            .shader_space
                            .textures
                            .get(self.output_texture_name.as_str())
                            .unwrap_or_else(|| {
                                panic!("output texture not found: {}", self.output_texture_name)
                            });
                        if let Some(id) = self.color_attachment {
                            renderer_guard.update_egui_texture_from_wgpu_texture(
                                &render_state.device,
                                texture.wgpu_texture_view.as_ref().unwrap(),
                                wgpu::FilterMode::Linear,
                                id,
                            );
                        } else {
                            self.color_attachment = Some(renderer_guard.register_native_texture(
                                &render_state.device,
                                texture.wgpu_texture_view.as_ref().unwrap(),
                                wgpu::FilterMode::Linear,
                            ));
                        }

                        did_rebuild_shader_space = true;
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
            let texture = self
                .shader_space
                .textures
                .get(self.output_texture_name.as_str())
                .unwrap_or_else(|| {
                    panic!("output texture not found: {}", self.output_texture_name)
                });
            self.color_attachment = Some(renderer_guard.register_native_texture(
                &render_state.device,
                texture.wgpu_texture_view.as_ref().unwrap(),
                wgpu::FilterMode::Linear,
            ));
        }

        // wgpu resource destruction is deferred; after hot-rebuilding lots of GPU resources
        // (pipelines/textures/bind groups), explicitly poll once to help the backend
        // process deferred drops. This mitigates slow steady growth during repeated rebuilds.
        if did_rebuild_shader_space {
            let _ = render_state.device.poll(wgpu::PollType::Poll);
        }

        let f = egui::Frame::default().fill(egui::Color32::BLACK);
        egui::CentralPanel::default().frame(f).show(ctx, |ui| {
            let avail_rect = ui.available_rect_before_wrap();
            ui.centered_and_justified(|ui| {
                ui.painter().image(
                    self.color_attachment.unwrap(),
                    Rect::from_min_max(
                        pos2(0.0, 0.0),
                        pos2(avail_rect.width() as f32, avail_rect.height() as f32),
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
