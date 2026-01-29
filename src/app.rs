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

    pub zoom: f32,
    pub zoom_initialized: bool,
    pub pan: egui::Vec2,
    pub pan_start: Option<egui::Pos2>,
    pub last_sampled: Option<SampledPixel>,
    pub texture_filter: wgpu::FilterMode,

    /// When true, automatically resize the OS window to match the scene DSL
    /// `screen_resolution` on updates. False by default: UI mode is the source
    /// of truth for window sizing.
    pub follow_scene_resolution_for_window: bool,
}

const CANVAS_RADIUS: f32 = 16.0;
const OUTER_MARGIN: f32 = 4.0;
const SIDEBAR_WIDTH: f32 = 340.0;
const SIDEBAR_MIN_WIDTH: f32 = 260.0;
const SIDEBAR_MAX_WIDTH: f32 = 460.0;
const SIDEBAR_ANIM_SECS: f64 = 0.25;

#[derive(Clone, Copy, Debug)]
struct UiAnim {
    start_time: f64,
    from: f32,
    to: f32,
}

/// UI layout mode: Sidebar visible vs canvas-only (takes full window space).
/// Note: "CanvasOnly" is NOT OS fullscreen - the window size stays the same.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum UiWindowMode {
    #[default]
    Sidebar,
    /// Canvas takes full window space (sidebar hidden). NOT OS fullscreen.
    CanvasOnly,
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

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_pos2(a: egui::Pos2, b: egui::Pos2, t: f32) -> egui::Pos2 {
    egui::pos2(lerp(a.x, b.x, t), lerp(a.y, b.y, t))
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::from_min_max(lerp_pos2(a.min, b.min, t), lerp_pos2(a.max, b.max, t))
}

fn ease_out_cubic(t: f32) -> f32 {
    let t = clamp01(t);
    1.0 - (1.0 - t).powi(3)
}

#[derive(Clone, Copy, Debug)]
enum TailwindButtonVariant {
    Destructive,
    Connected,
    Idle,
}

#[derive(Clone, Copy, Debug)]
struct TailwindButtonVisuals {
    bg: Color32,
    hover_bg: Color32,
    text: Color32,
}

fn tailwind_button_visuals(variant: TailwindButtonVariant) -> TailwindButtonVisuals {
    match variant {
        TailwindButtonVariant::Destructive => TailwindButtonVisuals {
            bg: Color32::from_rgb(0xEF, 0x44, 0x44),
            hover_bg: Color32::from_rgba_unmultiplied(0xEF, 0x44, 0x44, 230),
            text: Color32::WHITE,
        },
        TailwindButtonVariant::Connected => TailwindButtonVisuals {
            bg: Color32::from_rgba_unmultiplied(0x59, 0x8C, 0x5C, 77),
            hover_bg: Color32::from_rgba_unmultiplied(0x59, 0x8C, 0x5C, 77),
            text: Color32::from_rgb(0x63, 0xC7, 0x63),
        },
        TailwindButtonVariant::Idle => TailwindButtonVisuals {
            bg: Color32::TRANSPARENT,
            hover_bg: Color32::from_rgb(0x40, 0x40, 0x40),
            text: Color32::from_rgb(0xE6, 0xE6, 0xE6),
        },
    }
}

fn tailwind_button(
    ui: &mut egui::Ui,
    label: &str,
    title: &str,
    variant: TailwindButtonVariant,
    enabled: bool,
) -> egui::Response {
    let visuals = tailwind_button_visuals(variant);
    let bg = if enabled {
        visuals.bg
    } else {
        visuals.bg.gamma_multiply(0.6)
    };
    let hover_bg = if enabled {
        visuals.hover_bg
    } else {
        visuals.hover_bg.gamma_multiply(0.6)
    };
    let text = if enabled {
        visuals.text
    } else {
        visuals.text.gamma_multiply(0.6)
    };

    let font_id = egui::FontId::proportional(12.0);
    let label = egui::RichText::new(label).font(font_id).color(text);
    let button = egui::Button::new(label)
        .frame(true)
        .corner_radius(egui::CornerRadius::same(0));

    ui.scope(|ui| {
        let mut style = ui.style().as_ref().clone();
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.visuals.widgets.inactive.bg_fill = bg;
        style.visuals.widgets.inactive.weak_bg_fill = bg;
        style.visuals.widgets.hovered.bg_fill = hover_bg;
        style.visuals.widgets.active.bg_fill = hover_bg;
        style.visuals.widgets.inactive.fg_stroke.color = text;
        style.visuals.widgets.hovered.fg_stroke.color = text;
        style.visuals.widgets.active.fg_stroke.color = text;
        style.visuals.widgets.noninteractive.bg_fill = bg;
        style.visuals.widgets.noninteractive.fg_stroke.color = text;
        ui.set_style(style);

        ui.add_enabled(enabled, button).on_hover_text(title)
    })
    .inner
}

fn canvas_sampler_descriptor(filter: wgpu::FilterMode) -> wgpu::SamplerDescriptor<'static> {
    wgpu::SamplerDescriptor {
        label: Some("canvas_texture_sampler"),
        address_mode_u: wgpu::AddressMode::ClampToBorder,
        address_mode_v: wgpu::AddressMode::ClampToBorder,
        address_mode_w: wgpu::AddressMode::ClampToBorder,
        border_color: Some(wgpu::SamplerBorderColor::TransparentBlack),
        mag_filter: filter,
        min_filter: filter,
        ..Default::default()
    }
}

fn apply_scene_resolution_to_window_state(
    current_window_resolution: [u32; 2],
    scene_screen_resolution: Option<[u32; 2]>,
    follow_scene_resolution_for_window: bool,
) -> ([u32; 2], Option<[f32; 2]>) {
    let Some([w, h]) = scene_screen_resolution else {
        return (current_window_resolution, None);
    };

    if [w, h] == current_window_resolution {
        return (current_window_resolution, None);
    }

    let next_window_resolution = [w, h];
    let maybe_resize = if follow_scene_resolution_for_window {
        Some([w as f32, h as f32])
    } else {
        None
    };

    (next_window_resolution, maybe_resize)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        ctx.set_visuals(egui::Visuals::dark());

        // UI window mode is stored in egui memory so we don't have to plumb
        // new fields through App construction.
        let window_mode_id = egui::Id::new("ui_window_mode");
        let window_mode_prev_id = egui::Id::new("ui_window_mode_prev");
        let canvas_center_prev_id = egui::Id::new("ui_canvas_center_prev");
        let startup_sidebar_sized_id = egui::Id::new("ui_startup_sidebar_sized");
        let min_zoom_id = egui::Id::new("ui_min_zoom");
        let sidebar_factor_id = egui::Id::new("ui_sidebar_factor");
        let sidebar_anim_id = egui::Id::new("ui_sidebar_anim");

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
                    let (next_window_resolution, maybe_resize) =
                        apply_scene_resolution_to_window_state(
                            self.window_resolution,
                            crate::dsl::screen_resolution(&scene),
                            self.follow_scene_resolution_for_window,
                        );
                    self.window_resolution = next_window_resolution;

                    // Only force the OS window size to follow the scene when explicitly enabled
                    // (e.g., for dedicated render-only workflows). In normal UI mode, the window
                    // size should be controlled by the UI / user.
                    if let Some([w, h]) = maybe_resize {
                        let size = egui::vec2(w, h);
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(size));
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
                                renderer_guard
                                    .update_egui_texture_from_wgpu_texture_with_sampler_options(
                                        &render_state.device,
                                        texture.wgpu_texture_view.as_ref().unwrap(),
                                        canvas_sampler_descriptor(self.texture_filter),
                                        id,
                                    );
                            } else {
                                self.color_attachment = Some(
                                    renderer_guard.register_native_texture_with_sampler_options(
                                        &render_state.device,
                                        texture.wgpu_texture_view.as_ref().unwrap(),
                                        canvas_sampler_descriptor(self.texture_filter),
                                    ),
                                );
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
                                    renderer_guard
                                        .update_egui_texture_from_wgpu_texture_with_sampler_options(
                                            &render_state.device,
                                            texture.wgpu_texture_view.as_ref().unwrap(),
                                            canvas_sampler_descriptor(self.texture_filter),
                                            id,
                                        );
                                } else {
                                    self.color_attachment = Some(
                                        renderer_guard
                                            .register_native_texture_with_sampler_options(
                                                &render_state.device,
                                                texture.wgpu_texture_view.as_ref().unwrap(),
                                                canvas_sampler_descriptor(self.texture_filter),
                                            ),
                                    );
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
                                    renderer_guard
                                        .update_egui_texture_from_wgpu_texture_with_sampler_options(
                                            &render_state.device,
                                            texture.wgpu_texture_view.as_ref().unwrap(),
                                            canvas_sampler_descriptor(wgpu::FilterMode::Linear),
                                            id,
                                        );
                                } else {
                                    self.color_attachment = Some(
                                        renderer_guard
                                            .register_native_texture_with_sampler_options(
                                                &render_state.device,
                                                texture.wgpu_texture_view.as_ref().unwrap(),
                                                canvas_sampler_descriptor(wgpu::FilterMode::Linear),
                                            ),
                                    );
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
                            renderer_guard
                                .update_egui_texture_from_wgpu_texture_with_sampler_options(
                                    &render_state.device,
                                    texture.wgpu_texture_view.as_ref().unwrap(),
                                    canvas_sampler_descriptor(self.texture_filter),
                                    id,
                                );
                        } else {
                            self.color_attachment =
                                Some(renderer_guard.register_native_texture_with_sampler_options(
                                    &render_state.device,
                                    texture.wgpu_texture_view.as_ref().unwrap(),
                                    canvas_sampler_descriptor(self.texture_filter),
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
            self.color_attachment =
                Some(renderer_guard.register_native_texture_with_sampler_options(
                    &render_state.device,
                    texture.wgpu_texture_view.as_ref().unwrap(),
                    canvas_sampler_descriptor(wgpu::FilterMode::Linear),
                ));
        }

        // wgpu resource destruction is deferred; after hot-rebuilding lots of GPU resources
        // (pipelines/textures/bind groups), explicitly poll once to help the backend
        // process deferred drops. This mitigates slow steady growth during repeated rebuilds.
        if did_rebuild_shader_space {
            let _ = render_state.device.poll(wgpu::PollType::Poll);
        }

        const CARD_RADIUS: f32 = 12.0;

        let now = ctx.input(|i| i.time);

        let toggle_canvas_only = || {
            let mode = ctx
                .memory(|mem| mem.data.get_temp::<UiWindowMode>(window_mode_id))
                .unwrap_or_default();

            let next = match mode {
                UiWindowMode::Sidebar => UiWindowMode::CanvasOnly,
                UiWindowMode::CanvasOnly => UiWindowMode::Sidebar,
            };

            ctx.memory_mut(|mem| {
                mem.data.insert_temp(window_mode_id, next);
            });
        };

        let window_mode = ctx
            .memory(|mem| mem.data.get_temp::<UiWindowMode>(window_mode_id))
            .unwrap_or_default();

        // Animated transition state (stored in egui memory).
        let prev_mode = ctx
            .memory(|mem| mem.data.get_temp::<UiWindowMode>(window_mode_prev_id))
            .unwrap_or(window_mode);
        let target_sidebar_factor = match window_mode {
            UiWindowMode::Sidebar => 1.0,
            UiWindowMode::CanvasOnly => 0.0,
        };

        let mut ui_sidebar_factor = ctx
            .memory(|mem| mem.data.get_temp::<f32>(sidebar_factor_id))
            .unwrap_or(target_sidebar_factor);
        let mut ui_sidebar_anim = ctx
            .memory(|mem| mem.data.get_temp::<Option<UiAnim>>(sidebar_anim_id))
            .unwrap_or(None);
        let was_animating_before_update = ui_sidebar_anim.is_some();

        if prev_mode != window_mode {
            ui_sidebar_anim = Some(UiAnim {
                start_time: now,
                from: ui_sidebar_factor,
                to: target_sidebar_factor,
            });
        }

        if let Some(anim) = ui_sidebar_anim {
            let raw_t = ((now - anim.start_time) / SIDEBAR_ANIM_SECS).clamp(0.0, 1.0) as f32;
            let eased = ease_out_cubic(raw_t);
            ui_sidebar_factor = clamp01(lerp(anim.from, anim.to, eased));
            if raw_t >= 1.0 {
                ui_sidebar_factor = clamp01(anim.to);
                ui_sidebar_anim = None;
            }
        } else {
            ui_sidebar_factor = target_sidebar_factor;
        }

        ctx.memory_mut(|mem| {
            mem.data.insert_temp(sidebar_factor_id, ui_sidebar_factor);
            mem.data
                .insert_temp::<Option<UiAnim>>(sidebar_anim_id, ui_sidebar_anim);
        });

        // One-shot startup sizing for sidebar mode: expand the OS window width
        // to accommodate the render view + sidebar. This must not run during a
        // per-frame animated transition to avoid override/jitter.
        let did_startup_sidebar_size = ctx
            .memory(|mem| mem.data.get_temp::<bool>(startup_sidebar_sized_id))
            .unwrap_or(false);
        if window_mode == UiWindowMode::Sidebar && !did_startup_sidebar_size {
            let target_width =
                self.window_resolution[0] as f32 + SIDEBAR_WIDTH + 2.0 * OUTER_MARGIN;
            let target_height = self.window_resolution[1] as f32;
            let mut target = egui::vec2(target_width, target_height);

            if let Some(monitor_size) = ctx.input(|i| i.viewport().monitor_size) {
                target.x = target.x.min(monitor_size.x);
                target.y = target.y.min(monitor_size.y);
            }

            // Keep the sidebar usable while allowing some minimum render area.
            let mut min_size = egui::vec2(SIDEBAR_WIDTH + 240.0, 240.0);
            if let Some(monitor_size) = ctx.input(|i| i.viewport().monitor_size) {
                min_size.x = min_size.x.min(monitor_size.x);
                min_size.y = min_size.y.min(monitor_size.y);
            }
            min_size.x = min_size.x.min(target.x);
            min_size.y = min_size.y.min(target.y);

            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(target));
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(min_size));

            ctx.memory_mut(|mem| {
                mem.data.insert_temp(startup_sidebar_sized_id, true);
            });
        }

        // In Fullscreen mode, hide the sidebar entirely (not just width=0)
        // to allow the canvas to take the full window.
        let animation_just_finished_opening =
            was_animating_before_update && ui_sidebar_anim.is_none() && ui_sidebar_factor >= 1.0;
        if ui_sidebar_factor > 0.0 {
            let sidebar = if ui_sidebar_factor < 1.0 || animation_just_finished_opening {
                egui::SidePanel::right("debug_sidebar")
                    .exact_width(SIDEBAR_WIDTH * ui_sidebar_factor)
                    .resizable(false)
            } else {
                egui::SidePanel::right("debug_sidebar")
                    .default_width(SIDEBAR_WIDTH)
                    .width_range(SIDEBAR_MIN_WIDTH..=SIDEBAR_MAX_WIDTH)
                    .resizable(true)
            };
            sidebar.show(ctx, |ui| {
                let content_rect = ui.available_rect_before_wrap();
                ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
                    ui.set_clip_rect(content_rect);
                    if ui_sidebar_factor > 0.01 {
                        ui.horizontal(|ui| {
                            ui.heading("Debug");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if tailwind_button(
                                        ui,
                                        "Canvas Only",
                                        "Toggle canvas-only layout",
                                        TailwindButtonVariant::Idle,
                                        true,
                                    )
                                    .clicked()
                                    {
                                        toggle_canvas_only();
                                    }
                                },
                            );
                        });
                        ui.separator();

                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for idx in 0..3 {
                                let card_width = ui.available_size_before_wrap().x;
                                egui::Frame::default()
                                    .fill(egui::Color32::from_gray(24))
                                    .inner_margin(egui::Margin::same(12))
                                    .corner_radius(egui::CornerRadius::same(CARD_RADIUS as u8))
                                    .show(ui, |ui| {
                                        ui.set_max_width(card_width);
                                        ui.label(egui::RichText::new(format!(
                                            "Placeholder card {}",
                                            idx + 1
                                        )));
                                        ui.add_space(6.0);
                                        ui.label("TODO: debug content");
                                    });

                                if idx != 2 {
                                    ui.add_space(10.0);
                                }
                            }
                        });
                    }
                });
            });
        }

        let f = egui::Frame::default()
            .fill(egui::Color32::BLACK)
            .inner_margin(egui::Margin::same(0));
        egui::CentralPanel::default().frame(f).show(ctx, |ui| {
            // Hotkey: toggle canvas-only (ignore when typing into egui widgets).
            if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::F)) {
                toggle_canvas_only();
            }

            let avail_rect = ui.available_rect_before_wrap();
            let image_size = egui::vec2(self.resolution[0] as f32, self.resolution[1] as f32);

            // In sidebar mode, render into a centered framed canvas (not edge-to-edge).
            // For animation, we compute both targets and interpolate.
            let full_rect = avail_rect;
            let aspect = (image_size.x / image_size.y).max(0.0001);
            let avail_w = avail_rect.width();
            let avail_h = avail_rect.height();
            let (w, h) = if avail_w / avail_h > aspect {
                (
                    (avail_h - CANVAS_RADIUS * 2.0) * aspect,
                    avail_h - CANVAS_RADIUS * 2.0,
                )
            } else {
                (
                    (avail_w - CANVAS_RADIUS * 2.0),
                    (avail_w - CANVAS_RADIUS * 2.0) / aspect,
                )
            };
            let framed_canvas_rect = Rect::from_center_size(avail_rect.center(), egui::vec2(w, h));
            let framed_view_rect = framed_canvas_rect; //.shrink(OUTER_MARGIN);

            let animated_view_rect = lerp_rect(full_rect, framed_view_rect, ui_sidebar_factor);
            let animated_canvas_rect = lerp_rect(full_rect, framed_canvas_rect, ui_sidebar_factor);

            let view_rect = animated_view_rect;
            let paint_frame = ui_sidebar_factor > 0.001;

            // Preserve pan during the animation by compensating for per-frame center drift.
            let prev_center = ctx
                .memory(|mem| mem.data.get_temp::<egui::Pos2>(canvas_center_prev_id))
                .unwrap_or(animated_view_rect.center());
            let new_center = animated_view_rect.center();
            self.pan += prev_center - new_center;

            // Fit-zoom is used for initial zoom + manual reset only. For min clamping,
            // use a stable min-zoom captured once to avoid zoom jumps across mode changes.
            let fit_zoom = (view_rect.width() / image_size.x)
                .min(view_rect.height() / image_size.y)
                .max(0.01);
            if !self.zoom_initialized {
                self.zoom = fit_zoom;
                self.zoom_initialized = true;
                ctx.memory_mut(|mem| {
                    mem.data.insert_temp(min_zoom_id, fit_zoom);
                });
            }
            let min_zoom = ctx
                .memory(|mem| mem.data.get_temp::<f32>(min_zoom_id))
                .unwrap_or_else(|| {
                    ctx.memory_mut(|mem| {
                        mem.data.insert_temp(min_zoom_id, fit_zoom);
                    });
                    fit_zoom
                });

            let zoom = clamp_zoom(self.zoom, min_zoom);
            self.zoom = zoom;
            let draw_size = image_size * zoom;
            let base_min = view_rect.center() - draw_size * 0.5;
            let mut image_rect = Rect::from_min_size(base_min + self.pan, draw_size);

            let response = ui.allocate_rect(avail_rect, egui::Sense::click_and_drag());

            if ctx.input(|i| i.key_pressed(egui::Key::R)) {
                self.zoom = fit_zoom;
                self.pan = egui::Vec2::ZERO;
                self.pan_start = None;
                let draw_size = image_size * self.zoom;
                let base_min = view_rect.center() - draw_size * 0.5;
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
                        renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                            &render_state.device,
                            texture.wgpu_texture_view.as_ref().unwrap(),
                            canvas_sampler_descriptor(self.texture_filter),
                            id,
                        );
                    } else {
                        self.color_attachment =
                            Some(renderer_guard.register_native_texture_with_sampler_options(
                                &render_state.device,
                                texture.wgpu_texture_view.as_ref().unwrap(),
                                canvas_sampler_descriptor(self.texture_filter),
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
                    let next_zoom = clamp_zoom(prev_zoom * scroll_zoom, min_zoom);
                    if next_zoom != prev_zoom {
                        let prev_size = image_size * prev_zoom;
                        let prev_min = view_rect.center() - prev_size * 0.5 + self.pan;
                        let local = (pointer_pos - prev_min) / prev_size;
                        self.zoom = next_zoom;
                        let next_size = image_size * next_zoom;
                        let next_min = pointer_pos - local * next_size;
                        let desired_pan = next_min - (view_rect.center() - next_size * 0.5);
                        self.pan = desired_pan;
                        image_rect = Rect::from_min_size(
                            view_rect.center() - next_size * 0.5 + self.pan,
                            next_size,
                        );
                    }
                }
            }

            let rounding = if paint_frame {
                let alpha = (ui_sidebar_factor * 255.0).round() as u8;
                let fill = egui::Color32::from_rgba_unmultiplied(18, 18, 18, alpha);
                let stroke_color = egui::Color32::from_rgba_unmultiplied(48, 48, 48, alpha);
                let radius = (CANVAS_RADIUS * ui_sidebar_factor)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let rounding = egui::CornerRadius::same(radius);
                ui.painter()
                    .rect_filled(animated_canvas_rect, rounding, fill);
                ui.painter().rect_stroke(
                    animated_canvas_rect,
                    rounding,
                    egui::Stroke::new(1.0, stroke_color),
                    egui::StrokeKind::Outside,
                );
                rounding
            } else {
                egui::CornerRadius::ZERO
            };

            let image_size = image_rect.size();
            let uv_min = (animated_canvas_rect.min - image_rect.min) / image_size;
            let uv_max = (animated_canvas_rect.max - image_rect.min) / image_size;
            let computed_uv =
                Rect::from_min_max(pos2(uv_min.x, uv_min.y), pos2(uv_max.x, uv_max.y));
            ui.painter().add(
                egui::epaint::RectShape::filled(animated_canvas_rect, rounding, Color32::WHITE)
                    .with_texture(self.color_attachment.unwrap(), computed_uv),
            );

            if response.clicked_by(egui::PointerButton::Primary) {
                if let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) {
                    if animated_view_rect.contains(pointer_pos) && image_rect.contains(pointer_pos)
                    {
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

            // Remember per-frame for next frame's mode-switch compensation.
            ctx.memory_mut(|mem| {
                mem.data.insert_temp(window_mode_prev_id, window_mode);
                mem.data
                    .insert_temp(canvas_center_prev_id, animated_view_rect.center());
            });
        });

        let title = if let Some(sampled) = self.last_sampled {
            format!(
                "Node Forge Render Server - x={} y={} rgba=({}, {}, {}, {})",
                sampled.x,
                sampled.y,
                sampled.rgba[0],
                sampled.rgba[1],
                sampled.rgba[2],
                sampled.rgba[3]
            )
        } else {
            "Node Forge Render Server".to_string()
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        // Continuous rendering mode: keep repainting even without input events.
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_scene_resolution_updates_window_state_without_forcing_resize_by_default() {
        let (next, resize) =
            apply_scene_resolution_to_window_state([1024, 1024], Some([800, 600]), false);
        assert_eq!(next, [800, 600]);
        assert_eq!(resize, None);
    }

    #[test]
    fn apply_scene_resolution_can_request_resize_when_enabled() {
        let (next, resize) =
            apply_scene_resolution_to_window_state([1024, 1024], Some([800, 600]), true);
        assert_eq!(next, [800, 600]);
        assert_eq!(resize, Some([800.0, 600.0]));
    }

    #[test]
    fn apply_scene_resolution_is_noop_when_same_or_missing() {
        let (next, resize) =
            apply_scene_resolution_to_window_state([800, 600], Some([800, 600]), true);
        assert_eq!(next, [800, 600]);
        assert_eq!(resize, None);

        let (next, resize) = apply_scene_resolution_to_window_state([800, 600], None, true);
        assert_eq!(next, [800, 600]);
        assert_eq!(resize, None);
    }
}
