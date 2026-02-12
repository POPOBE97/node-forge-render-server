mod canvas_controller;
mod layout_math;
mod scene_runtime;
mod texture_bridge;
mod types;
mod window_mode;

pub use types::{App, AppInit, SampledPixel};

use rust_wgpu_fiber::eframe::{self, egui, wgpu};

use crate::{renderer, ui};

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let mut visuals = egui::Visuals::dark();
        visuals.override_text_color =
            Some(egui::Color32::from_rgba_unmultiplied(255, 255, 255, 204));
        ctx.set_visuals(visuals);

        let render_state = frame.wgpu_render_state().unwrap();
        let mut renderer_guard = frame.wgpu_render_state().unwrap().renderer.as_ref().write();

        let mut did_rebuild_shader_space = false;
        if let Some(update) = scene_runtime::drain_latest_scene_update(self) {
            let apply_result = scene_runtime::apply_scene_update(self, ctx, render_state, update);
            if apply_result.did_rebuild_shader_space {
                let filter = apply_result
                    .texture_filter_override
                    .unwrap_or(self.texture_filter);
                let texture_name = self.output_texture_name.clone();
                texture_bridge::sync_output_texture(
                    self,
                    render_state,
                    &mut renderer_guard,
                    &texture_name,
                    filter,
                );
                did_rebuild_shader_space = true;
            }
            if apply_result.reset_viewport {
                self.pending_view_reset = true;
            }
        }

        let t = self.start.elapsed().as_secs_f32();
        for pass in &mut self.passes {
            let mut p = pass.base_params;
            p.time = t;
            let _ = renderer::update_pass_params(&self.shader_space, pass, &p);
        }

        self.shader_space.render();

        texture_bridge::ensure_output_texture_registered(self, render_state, &mut renderer_guard);

        if self.histogram_renderer.is_none() {
            self.histogram_renderer = Some(ui::histogram::HistogramRenderer::new(&render_state.device));
        }

        let display_texture_name = self
            .preview_texture_name
            .as_ref()
            .unwrap_or(&self.output_texture_name);

        let display_texture = self
            .shader_space
            .textures
            .get(display_texture_name.as_str())
            .or_else(|| self.shader_space.textures.get(self.output_texture_name.as_str()));

        if let Some(texture) = display_texture
            && let Some(source_view) = texture.wgpu_texture_view.as_ref()
            && let Some(histogram_renderer) = self.histogram_renderer.as_ref()
        {
            let source_size = [
                texture.wgpu_texture_desc.size.width,
                texture.wgpu_texture_desc.size.height,
            ];
            histogram_renderer.update(
                &render_state.device,
                self.shader_space.queue.as_ref(),
                source_view,
                source_size,
            );

            let sampler = wgpu::SamplerDescriptor {
                label: Some("sys.histogram.sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            };

            if let Some(id) = self.histogram_texture_id {
                renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                    &render_state.device,
                    histogram_renderer.output_view(),
                    sampler,
                    id,
                );
            } else {
                self.histogram_texture_id = Some(
                    renderer_guard.register_native_texture_with_sampler_options(
                        &render_state.device,
                        histogram_renderer.output_view(),
                        sampler,
                    ),
                );
            }
        }

        if did_rebuild_shader_space {
            let _ = render_state
                .device
                .poll(rust_wgpu_fiber::eframe::wgpu::PollType::Poll);
        }

        let now = ctx.input(|i| i.time);
        let frame_state = window_mode::update_window_mode_frame(self, now);
        window_mode::maybe_apply_startup_sidebar_sizing(self, ctx);

        let mut request_toggle_from_sidebar = false;
        let sidebar_full_w = ui::debug_sidebar::sidebar_width(ctx);
        let sidebar_w = sidebar_full_w * frame_state.sidebar_factor;

        // Rebuild resource snapshot when needed (pipeline changed or first frame).
        if self.resource_snapshot_generation != self.pipeline_rebuild_count {
            let snap =
                ui::resource_tree::ResourceSnapshot::capture(&self.shader_space, &self.passes);
            self.resource_tree_nodes = snap.to_tree();
            self.resource_snapshot = Some(snap);
            self.resource_snapshot_generation = self.pipeline_rebuild_count;
        }

        let mut sidebar_action: Option<ui::debug_sidebar::SidebarAction> = None;
        if sidebar_w > 0.0 {
            egui::SidePanel::left("debug_sidebar")
                .exact_width(sidebar_w)
                .resizable(false)
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let clip_rect = ui.available_rect_before_wrap();
                    let x_offset = -sidebar_full_w * (1.0 - frame_state.sidebar_factor);
                    let sidebar_rect = egui::Rect::from_min_size(
                        clip_rect.min + egui::vec2(x_offset, 0.0),
                        egui::vec2(sidebar_full_w, clip_rect.height()),
                    );

                    sidebar_action = ui::debug_sidebar::show_in_rect(
                        ctx,
                        ui,
                        frame_state.sidebar_factor,
                        frame_state.animation_just_finished_opening,
                        clip_rect,
                        sidebar_rect,
                        |ui| {
                            ui::button::tailwind_button(
                                ui,
                                "Canvas Only",
                                "Toggle canvas-only layout",
                                ui::button::TailwindButtonVariant::Idle,
                                ui::button::TailwindButtonGroupPosition::Single,
                                true,
                            )
                            .clicked()
                        },
                        || {
                            request_toggle_from_sidebar = true;
                        },
                        self.histogram_texture_id,
                        &self.resource_tree_nodes,
                        &mut self.file_tree_state,
                    );
                });
        }

        // Handle sidebar actions.
        match sidebar_action {
            Some(ui::debug_sidebar::SidebarAction::PreviewTexture(name)) => {
                self.preview_texture_name =
                    Some(rust_wgpu_fiber::ResourceName::from(name.as_str()));
                self.pending_view_reset = true;
            }
            Some(ui::debug_sidebar::SidebarAction::ClearPreview) => {
                // Only clear the name; the canvas controller will stop using the
                // attachment this frame and we free it next frame to avoid
                // use-after-free when the texture was already submitted for
                // painting earlier in this frame.
                self.preview_texture_name = None;
            }
            None => {}
        }

        let panel_frame = egui::Frame::default()
            .fill(egui::Color32::BLACK)
            .inner_margin(egui::Margin::same(0));

        let mut request_toggle_from_canvas = false;
        egui::CentralPanel::default()
            .frame(panel_frame)
            .show(ctx, |ui| {
                request_toggle_from_canvas = canvas_controller::show_canvas_panel(
                    self,
                    ctx,
                    ui,
                    render_state,
                    &mut renderer_guard,
                    frame_state,
                    now,
                );
            });

        if request_toggle_from_sidebar || request_toggle_from_canvas {
            window_mode::toggle_canvas_only(self, now);
        }

        // Keep previous mode as the mode used for this frame's layout pass.
        // If a toggle happened during this frame, next frame should see
        // prev != current and start the transition animation.
        self.prev_window_mode = frame_state.mode;

        // Force dark title bar.
        ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));

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
        ctx.request_repaint();
    }
}
