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

    pub zoom: f32,
    pub zoom_initialized: bool,
    pub pan: egui::Vec2,
    pub pan_start: Option<egui::Pos2>,
    pub last_sampled: Option<SampledPixel>,
    pub texture_filter: wgpu::FilterMode,
}

#[derive(Clone, Copy, Debug)]
pub struct SampledPixel {
    pub x: u32,
    pub y: u32,
    pub rgba: [u8; 4],
}

fn clamp_zoom(value: f32, min_zoom: f32) -> f32 {
    value.clamp(min_zoom, 100.0)
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

                    let build_result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
                                    panic!("output texture not found: {}", self.output_texture_name)
                                });
                            if let Some(id) = self.color_attachment {
                                renderer_guard.update_egui_texture_from_wgpu_texture(
                                    &render_state.device,
                                    texture.wgpu_texture_view.as_ref().unwrap(),
                                    self.texture_filter,
                                    id,
                                );
                            } else {
                                self.color_attachment =
                                    Some(renderer_guard.register_native_texture(
                                        &render_state.device,
                                        texture.wgpu_texture_view.as_ref().unwrap(),
                                        self.texture_filter,
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
                                        self.texture_filter,
                                        id,
                                    );
                                } else {
                                    self.color_attachment =
                                        Some(renderer_guard.register_native_texture(
                                            &render_state.device,
                                            texture.wgpu_texture_view.as_ref().unwrap(),
                                            self.texture_filter,
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
                                    self.color_attachment =
                                        Some(renderer_guard.register_native_texture(
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
                                self.texture_filter,
                                id,
                            );
                        } else {
                            self.color_attachment = Some(renderer_guard.register_native_texture(
                                &render_state.device,
                                texture.wgpu_texture_view.as_ref().unwrap(),
                                self.texture_filter,
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
            let image_size = egui::vec2(self.resolution[0] as f32, self.resolution[1] as f32);
            let fit_zoom = (avail_rect.width() / image_size.x)
                .min(avail_rect.height() / image_size.y)
                .max(0.01);
            if !self.zoom_initialized {
                self.zoom = fit_zoom;
                self.zoom_initialized = true;
            }
            let zoom = clamp_zoom(self.zoom, fit_zoom);
            self.zoom = zoom;
            let draw_size = image_size * zoom;
            let base_min = avail_rect.center() - draw_size * 0.5;
            let mut image_rect = Rect::from_min_size(base_min + self.pan, draw_size);
            let response = ui.allocate_rect(avail_rect, egui::Sense::click_and_drag());

            if ctx.input(|i| i.key_pressed(egui::Key::R)) {
                self.zoom = fit_zoom;
                self.pan = egui::Vec2::ZERO;
                self.pan_start = None;
                let draw_size = image_size * self.zoom;
                let base_min = avail_rect.center() - draw_size * 0.5;
                image_rect = Rect::from_min_size(base_min, draw_size);
            }

            if ctx.input(|i| i.key_pressed(egui::Key::P)) {
                self.texture_filter = match self.texture_filter {
                    wgpu::FilterMode::Nearest => wgpu::FilterMode::Linear,
                    wgpu::FilterMode::Linear => wgpu::FilterMode::Nearest,
                };
                if let Some(texture) = self
                    .shader_space
                    .textures
                    .get(self.output_texture_name.as_str())
                {
                    if let Some(id) = self.color_attachment {
                        renderer_guard.update_egui_texture_from_wgpu_texture(
                            &render_state.device,
                            texture.wgpu_texture_view.as_ref().unwrap(),
                            self.texture_filter,
                            id,
                        );
                    } else {
                        self.color_attachment = Some(renderer_guard.register_native_texture(
                            &render_state.device,
                            texture.wgpu_texture_view.as_ref().unwrap(),
                            self.texture_filter,
                        ));
                    }
                }
            }

            if response.drag_started_by(egui::PointerButton::Middle) {
                if let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) {
                    self.pan_start = Some(pointer_pos);
                }
            }
            if response.dragged_by(egui::PointerButton::Middle) {
                if let (Some(start), Some(pointer_pos)) =
                    (self.pan_start, ctx.input(|i| i.pointer.hover_pos()))
                {
                    self.pan += pointer_pos - start;
                    self.pan_start = Some(pointer_pos);
                    image_rect = Rect::from_min_size(base_min + self.pan, draw_size);
                }
            } else if !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle)) {
                self.pan_start = None;
            }

            let zoom_delta = ctx.input(|i| i.zoom_delta());
            let scroll_delta = ctx.input(|i| i.smooth_scroll_delta);
            let scroll_zoom = if zoom_delta != 1.0 {
                zoom_delta
            } else {
                let base = 1.0025f32;
                let exponent = scroll_delta.y.max(-1200.0).min(1200.0);
                base.powf(exponent)
            };
            if scroll_zoom != 1.0 {
                if let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) {
                    let prev_zoom = self.zoom;
                    let next_zoom = clamp_zoom(prev_zoom * scroll_zoom, fit_zoom);
                    if next_zoom != prev_zoom {
                        let prev_size = image_size * prev_zoom;
                        let prev_min = avail_rect.center() - prev_size * 0.5 + self.pan;
                        let local = (pointer_pos - prev_min) / prev_size;
                        self.zoom = next_zoom;
                        let next_size = image_size * next_zoom;
                        let next_min = pointer_pos - local * next_size;
                        let desired_pan = next_min - (avail_rect.center() - next_size * 0.5);
                        self.pan = desired_pan;
                        image_rect = Rect::from_min_size(
                            avail_rect.center() - next_size * 0.5 + self.pan,
                            next_size,
                        );
                    }
                }
            }

            let uv_rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            ui.painter().image(
                self.color_attachment.unwrap(),
                image_rect,
                uv_rect,
                Color32::WHITE,
            );

            if response.clicked_by(egui::PointerButton::Primary) {
                if let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) {
                    if image_rect.contains(pointer_pos) {
                        let local = (pointer_pos - image_rect.min) / image_rect.size();
                        let x = (local.x * self.resolution[0] as f32).floor() as u32;
                        let y = (local.y * self.resolution[1] as f32).floor() as u32;
                        if x < self.resolution[0] && y < self.resolution[1] {
                            if let Ok(image) = self
                                .shader_space
                                .read_texture_rgba8(self.output_texture_name.as_str())
                            {
                                let idx = ((y * self.resolution[0] + x) * 4) as usize;
                                if idx + 3 < image.bytes.len() {
                                    self.last_sampled = Some(SampledPixel {
                                        x,
                                        y,
                                        rgba: [
                                            image.bytes[idx],
                                            image.bytes[idx + 1],
                                            image.bytes[idx + 2],
                                            image.bytes[idx + 3],
                                        ],
                                    });
                                }
                            }
                        }
                    }
                }
            }

            if let Some(sampled) = self.last_sampled {
                let label = format!(
                    "x={} y={} rgba=({}, {}, {}, {})",
                    sampled.x,
                    sampled.y,
                    sampled.rgba[0],
                    sampled.rgba[1],
                    sampled.rgba[2],
                    sampled.rgba[3]
                );
                ui.painter().text(
                    avail_rect.left_top() + egui::vec2(8.0, 8.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::TextStyle::Monospace.resolve(ui.style()),
                    Color32::WHITE,
                );
            }
        });

        // Continuous rendering mode: keep repainting even without input events.
        ctx.request_repaint();
    }
}
