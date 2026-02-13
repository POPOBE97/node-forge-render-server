mod canvas_controller;
mod layout_math;
mod scene_runtime;
mod texture_bridge;
mod types;
mod window_mode;

pub use types::{
    AnalysisTab, App, AppInit, ClippingSettings, DiffMetricMode, DiffStats, RefImageMode,
    SampledPixel,
};

use rust_wgpu_fiber::eframe::{self, egui, wgpu};

use crate::{renderer, ui};

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let frame_time = ctx.input(|i| i.time);

        let mut visuals = egui::Visuals::dark();
        visuals.override_text_color =
            Some(egui::Color32::from_rgba_unmultiplied(255, 255, 255, 204));
        ctx.set_visuals(visuals);

        let render_state = frame.wgpu_render_state().unwrap();
        let mut renderer_guard = frame.wgpu_render_state().unwrap().renderer.as_ref().write();

        let mut did_rebuild_shader_space = false;
        if let Some(update) = scene_runtime::drain_latest_scene_update(self) {
            let apply_result = scene_runtime::apply_scene_update(self, ctx, render_state, update);
            self.diff_dirty = true;
            self.analysis_dirty = true;
            self.clipping_dirty = true;
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

        canvas_controller::sync_reference_image_from_scene(
            self,
            ctx,
            render_state,
            &mut renderer_guard,
        );

        let raw_t = self.start.elapsed().as_secs_f32();
        let delta_t = (raw_t - self.time_last_raw_secs).max(0.0);
        self.time_last_raw_secs = raw_t;
        if self.time_updates_enabled {
            self.time_value_secs += delta_t;
        }
        let t = self.time_value_secs;
        for pass in &mut self.passes {
            let mut p = pass.base_params;
            p.time = t;
            let _ = renderer::update_pass_params(&self.shader_space, pass, &p);
        }

        if self.scene_uses_time && self.time_updates_enabled {
            if matches!(
                self.ref_image.as_ref().map(|r| r.mode),
                Some(RefImageMode::Diff)
            ) {
                self.diff_dirty = true;
            }
            self.analysis_dirty = true;
            self.clipping_dirty = true;
        }

        self.shader_space.render();

        texture_bridge::ensure_output_texture_registered(self, render_state, &mut renderer_guard);

        let display_texture_name = self
            .preview_texture_name
            .as_ref()
            .unwrap_or(&self.output_texture_name);

        let display_texture = self
            .shader_space
            .textures
            .get(display_texture_name.as_str())
            .or_else(|| {
                self.shader_space
                    .textures
                    .get(self.output_texture_name.as_str())
            });

        let mut computed_diff_stats = None;
        let mut did_update_diff_output = false;

        if let Some(reference) = self.ref_image.as_ref()
            && let Some(texture) = display_texture
            && let Some(source_view) = texture.wgpu_texture_view.as_ref()
        {
            let reference_mode = reference.mode;
            let reference_offset = [
                reference.offset.x.round() as i32,
                reference.offset.y.round() as i32,
            ];
            let ref_size = reference.size;
            let needs_recreate = self
                .diff_renderer
                .as_ref()
                .map(|r| r.output_size() != ref_size)
                .unwrap_or(true);
            if needs_recreate {
                self.diff_renderer = Some(ui::diff_renderer::DiffRenderer::new(
                    &render_state.device,
                    ref_size,
                ));
            }

            if let Some(diff_renderer) = self.diff_renderer.as_mut() {
                let source_size = [
                    texture.wgpu_texture_desc.size.width,
                    texture.wgpu_texture_desc.size.height,
                ];
                let should_update_diff = matches!(reference_mode, RefImageMode::Diff)
                    && (self.diff_dirty || needs_recreate || self.diff_texture_id.is_none());

                if should_update_diff {
                    let diff_stats = diff_renderer.update(
                        &render_state.device,
                        self.shader_space.queue.as_ref(),
                        source_view,
                        source_size,
                        &reference.wgpu_texture_view,
                        reference.size,
                        reference_offset,
                        self.diff_metric_mode,
                        matches!(reference_mode, RefImageMode::Diff),
                    );
                    did_update_diff_output = true;

                    computed_diff_stats = diff_stats;
                }

                if did_update_diff_output {
                    let sampler = wgpu::SamplerDescriptor {
                        label: Some("sys.diff.sampler"),
                        address_mode_u: wgpu::AddressMode::ClampToEdge,
                        address_mode_v: wgpu::AddressMode::ClampToEdge,
                        address_mode_w: wgpu::AddressMode::ClampToEdge,
                        mag_filter: wgpu::FilterMode::Nearest,
                        min_filter: wgpu::FilterMode::Nearest,
                        ..Default::default()
                    };

                    if let Some(id) = self.diff_texture_id {
                        renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                            &render_state.device,
                            diff_renderer.output_view(),
                            sampler,
                            id,
                        );
                    } else {
                        self.diff_texture_id =
                            Some(renderer_guard.register_native_texture_with_sampler_options(
                                &render_state.device,
                                diff_renderer.output_view(),
                                sampler,
                            ));
                    }
                    self.diff_dirty = false;
                    self.analysis_dirty = true;
                    self.clipping_dirty = true;
                }
            }
        }

        if computed_diff_stats.is_some() {
            self.diff_stats = computed_diff_stats;
        } else if !matches!(
            self.ref_image.as_ref().map(|r| r.mode),
            Some(RefImageMode::Diff)
        ) {
            self.diff_stats = None;
        }

        if self.histogram_renderer.is_none() {
            self.histogram_renderer =
                Some(ui::histogram::HistogramRenderer::new(&render_state.device));
        }
        if self.parade_renderer.is_none() {
            self.parade_renderer = Some(ui::parade::ParadeRenderer::new(&render_state.device));
        }
        if self.vectorscope_renderer.is_none() {
            self.vectorscope_renderer = Some(ui::vectorscope::VectorscopeRenderer::new(
                &render_state.device,
            ));
        }

        let mut analysis_source = None;
        if let Some(texture) = display_texture
            && let Some(source_view) = texture.wgpu_texture_view.as_ref()
        {
            let analysis_view = source_view;
            let analysis_size = [
                texture.wgpu_texture_desc.size.width,
                texture.wgpu_texture_desc.size.height,
            ];
            // Infographics/clipping always sample from the rendered texture region
            // (never the reference-sized diff texture).
            self.analysis_source_is_diff = false;
            analysis_source = Some((analysis_view, analysis_size));
        } else {
            self.analysis_source_is_diff = false;
        }

        let mut did_update_active_analysis = false;
        let mut did_update_clipping = false;

        if let Some((analysis_view, analysis_size)) = analysis_source {
            match self.analysis_tab {
                AnalysisTab::Histogram => {
                    let should_update = self.analysis_dirty
                        || self.histogram_texture_id.is_none()
                        || did_update_diff_output;
                    if should_update
                        && let Some(histogram_renderer) = self.histogram_renderer.as_ref()
                    {
                        histogram_renderer.update(
                            &render_state.device,
                            self.shader_space.queue.as_ref(),
                            analysis_view,
                            analysis_size,
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
                            renderer_guard
                                .update_egui_texture_from_wgpu_texture_with_sampler_options(
                                    &render_state.device,
                                    histogram_renderer.output_view(),
                                    sampler,
                                    id,
                                );
                        } else {
                            self.histogram_texture_id =
                                Some(renderer_guard.register_native_texture_with_sampler_options(
                                    &render_state.device,
                                    histogram_renderer.output_view(),
                                    sampler,
                                ));
                        }
                        did_update_active_analysis = true;
                    }
                }
                AnalysisTab::Parade => {
                    let should_update = self.analysis_dirty
                        || self.parade_texture_id.is_none()
                        || did_update_diff_output;
                    if should_update && let Some(parade_renderer) = self.parade_renderer.as_ref() {
                        parade_renderer.update(
                            &render_state.device,
                            self.shader_space.queue.as_ref(),
                            analysis_view,
                            analysis_size,
                        );

                        let parade_sampler = wgpu::SamplerDescriptor {
                            label: Some("sys.scope.parade.sampler"),
                            address_mode_u: wgpu::AddressMode::ClampToEdge,
                            address_mode_v: wgpu::AddressMode::ClampToEdge,
                            address_mode_w: wgpu::AddressMode::ClampToEdge,
                            mag_filter: wgpu::FilterMode::Nearest,
                            min_filter: wgpu::FilterMode::Nearest,
                            ..Default::default()
                        };
                        if let Some(id) = self.parade_texture_id {
                            renderer_guard
                                .update_egui_texture_from_wgpu_texture_with_sampler_options(
                                    &render_state.device,
                                    parade_renderer.parade_output_view(),
                                    parade_sampler,
                                    id,
                                );
                        } else {
                            self.parade_texture_id =
                                Some(renderer_guard.register_native_texture_with_sampler_options(
                                    &render_state.device,
                                    parade_renderer.parade_output_view(),
                                    parade_sampler,
                                ));
                        }

                        did_update_active_analysis = true;
                    }
                }
                AnalysisTab::Vectorscope => {
                    let should_update = self.analysis_dirty
                        || self.vectorscope_texture_id.is_none()
                        || did_update_diff_output;
                    if should_update
                        && let Some(vectorscope_renderer) = self.vectorscope_renderer.as_ref()
                    {
                        vectorscope_renderer.update(
                            &render_state.device,
                            self.shader_space.queue.as_ref(),
                            analysis_view,
                            analysis_size,
                        );

                        let sampler = wgpu::SamplerDescriptor {
                            label: Some("sys.scope.vectorscope.sampler"),
                            address_mode_u: wgpu::AddressMode::ClampToEdge,
                            address_mode_v: wgpu::AddressMode::ClampToEdge,
                            address_mode_w: wgpu::AddressMode::ClampToEdge,
                            mag_filter: wgpu::FilterMode::Nearest,
                            min_filter: wgpu::FilterMode::Nearest,
                            ..Default::default()
                        };

                        if let Some(id) = self.vectorscope_texture_id {
                            renderer_guard
                                .update_egui_texture_from_wgpu_texture_with_sampler_options(
                                    &render_state.device,
                                    vectorscope_renderer.output_view(),
                                    sampler,
                                    id,
                                );
                        } else {
                            self.vectorscope_texture_id =
                                Some(renderer_guard.register_native_texture_with_sampler_options(
                                    &render_state.device,
                                    vectorscope_renderer.output_view(),
                                    sampler,
                                ));
                        }
                        did_update_active_analysis = true;
                    }
                }
                AnalysisTab::Clipping => {}
            }

            let clipping_required = matches!(self.analysis_tab, AnalysisTab::Clipping);
            if clipping_required {
                if self.clipping_renderer.is_none() {
                    self.clipping_renderer = Some(ui::clipping_map::ClippingMapRenderer::new(
                        &render_state.device,
                        analysis_size,
                    ));
                }

                let should_update_clipping = self.analysis_dirty
                    || self.clipping_dirty
                    || self.clipping_texture_id.is_none()
                    || did_update_diff_output;
                if should_update_clipping
                    && let Some(clipping_renderer) = self.clipping_renderer.as_mut()
                {
                    clipping_renderer.update(
                        &render_state.device,
                        self.shader_space.queue.as_ref(),
                        analysis_view,
                        analysis_size,
                        self.clipping_settings.shadow_threshold,
                        self.clipping_settings.highlight_threshold,
                    );

                    let sampler = wgpu::SamplerDescriptor {
                        label: Some("sys.scope.clipping.sampler"),
                        address_mode_u: wgpu::AddressMode::ClampToEdge,
                        address_mode_v: wgpu::AddressMode::ClampToEdge,
                        address_mode_w: wgpu::AddressMode::ClampToEdge,
                        mag_filter: wgpu::FilterMode::Nearest,
                        min_filter: wgpu::FilterMode::Nearest,
                        ..Default::default()
                    };

                    if let Some(id) = self.clipping_texture_id {
                        renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                            &render_state.device,
                            clipping_renderer.output_view(),
                            sampler,
                            id,
                        );
                    } else {
                        self.clipping_texture_id =
                            Some(renderer_guard.register_native_texture_with_sampler_options(
                                &render_state.device,
                                clipping_renderer.output_view(),
                                sampler,
                            ));
                    }

                    did_update_clipping = true;
                    if matches!(self.analysis_tab, AnalysisTab::Clipping) {
                        did_update_active_analysis = true;
                    }
                }
            }
        }

        if did_update_active_analysis {
            self.analysis_dirty = false;
        }
        if did_update_clipping {
            self.clipping_dirty = false;
        }

        if did_rebuild_shader_space {
            let _ = render_state
                .device
                .poll(rust_wgpu_fiber::eframe::wgpu::PollType::Poll);
        }

        let now = frame_time;
        let frame_state = window_mode::update_window_mode_frame(self, now);
        window_mode::maybe_apply_startup_sidebar_sizing(self, ctx);

        let mut request_toggle_from_sidebar = false;
        let sidebar_full_w = ui::debug_sidebar::sidebar_width(ctx);
        let sidebar_w = sidebar_full_w * frame_state.sidebar_factor;
        let reference_sidebar_state =
            self.ref_image
                .as_ref()
                .map(|reference| ui::debug_sidebar::ReferenceSidebarState {
                    mode: reference.mode,
                    opacity: reference.opacity,
                    diff_metric_mode: self.diff_metric_mode,
                    diff_stats: self.diff_stats,
                });
        let analysis_sidebar_state = ui::debug_sidebar::AnalysisSidebarState {
            tab: self.analysis_tab,
            clipping: self.clipping_settings,
            source_is_diff: self.analysis_source_is_diff,
        };

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
                        self.parade_texture_id,
                        self.vectorscope_texture_id,
                        analysis_sidebar_state,
                        reference_sidebar_state.as_ref(),
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
                self.diff_dirty = true;
                self.analysis_dirty = true;
                self.clipping_dirty = true;
            }
            Some(ui::debug_sidebar::SidebarAction::ClearPreview) => {
                // Only clear the name; the canvas controller will stop using the
                // attachment this frame and we free it next frame to avoid
                // use-after-free when the texture was already submitted for
                // painting earlier in this frame.
                self.preview_texture_name = None;
                self.diff_dirty = true;
                self.analysis_dirty = true;
                self.clipping_dirty = true;
            }
            Some(ui::debug_sidebar::SidebarAction::SetReferenceOpacity(opacity)) => {
                if let Some(reference) = self.ref_image.as_mut() {
                    reference.opacity = opacity.clamp(0.0, 1.0);
                }
            }
            Some(ui::debug_sidebar::SidebarAction::ToggleReferenceMode) => {
                if let Some(reference) = self.ref_image.as_mut() {
                    reference.mode = match reference.mode {
                        RefImageMode::Overlay => RefImageMode::Diff,
                        RefImageMode::Diff => RefImageMode::Overlay,
                    };
                    self.diff_dirty = true;
                    self.analysis_dirty = true;
                    self.clipping_dirty = true;
                }
            }
            Some(ui::debug_sidebar::SidebarAction::ClearReference) => {
                canvas_controller::clear_reference(self, &mut renderer_guard);
            }
            Some(ui::debug_sidebar::SidebarAction::SetDiffMetricMode(mode)) => {
                if self.diff_metric_mode != mode {
                    self.diff_metric_mode = mode;
                    self.diff_dirty = true;
                    self.analysis_dirty = true;
                    self.clipping_dirty = true;
                }
            }
            Some(ui::debug_sidebar::SidebarAction::SetAnalysisTab(tab)) => {
                let next_tab = match tab {
                    AnalysisTab::Clipping => {
                        if matches!(self.analysis_tab, AnalysisTab::Clipping) {
                            match self.last_info_graphics_tab {
                                AnalysisTab::Histogram
                                | AnalysisTab::Parade
                                | AnalysisTab::Vectorscope => self.last_info_graphics_tab,
                                AnalysisTab::Clipping => AnalysisTab::Histogram,
                            }
                        } else {
                            self.last_info_graphics_tab = self.analysis_tab;
                            AnalysisTab::Clipping
                        }
                    }
                    AnalysisTab::Histogram | AnalysisTab::Parade | AnalysisTab::Vectorscope => {
                        self.last_info_graphics_tab = tab;
                        tab
                    }
                };

                if self.analysis_tab != next_tab {
                    self.analysis_tab = next_tab;
                    self.analysis_dirty = true;
                    self.clipping_dirty = true;
                }
            }
            Some(ui::debug_sidebar::SidebarAction::SetClippingShadowThreshold(threshold)) => {
                let threshold = threshold.clamp(0.0, 1.0);
                if (self.clipping_settings.shadow_threshold - threshold).abs() > f32::EPSILON {
                    self.clipping_settings.shadow_threshold = threshold;
                    self.clipping_dirty = true;
                    if matches!(self.analysis_tab, AnalysisTab::Clipping) {
                        self.analysis_dirty = true;
                    }
                }
            }
            Some(ui::debug_sidebar::SidebarAction::SetClippingHighlightThreshold(threshold)) => {
                let threshold = threshold.clamp(0.0, 1.0);
                if (self.clipping_settings.highlight_threshold - threshold).abs() > f32::EPSILON {
                    self.clipping_settings.highlight_threshold = threshold;
                    self.clipping_dirty = true;
                    if matches!(self.analysis_tab, AnalysisTab::Clipping) {
                        self.analysis_dirty = true;
                    }
                }
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
