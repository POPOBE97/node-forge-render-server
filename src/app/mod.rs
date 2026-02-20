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

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use rust_wgpu_fiber::eframe::{self, egui, wgpu};

use crate::{app::types::AnalysisSourceDomain, renderer, ui};

fn hash_key<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn analysis_source_request_key(source: &AnalysisSourceDomain<'_>) -> u64 {
    hash_key(&(source.texture_name, source.size, source.format))
}

fn diff_request_key(
    source_key: u64,
    reference_size: [u32; 2],
    reference_offset: [i32; 2],
    metric_mode: DiffMetricMode,
    clamp_output: bool,
) -> u64 {
    hash_key(&(
        source_key,
        reference_size,
        reference_offset,
        metric_mode,
        clamp_output,
    ))
}

fn diff_stats_request_key(diff_key: u64) -> u64 {
    hash_key(&(diff_key, "stats"))
}

fn histogram_request_key(source_key: u64) -> u64 {
    hash_key(&(source_key, "histogram"))
}

fn parade_request_key(source_key: u64) -> u64 {
    hash_key(&(source_key, "parade"))
}

fn vectorscope_request_key(source_key: u64) -> u64 {
    hash_key(&(source_key, "vectorscope"))
}

fn clipping_request_key(source_key: u64, settings: ClippingSettings, enabled: bool) -> u64 {
    hash_key(&(
        source_key,
        enabled,
        settings.shadow_threshold.to_bits(),
        settings.highlight_threshold.to_bits(),
    ))
}

fn apply_analysis_tab_change(
    analysis_tab: &mut AnalysisTab,
    analysis_dirty: &mut bool,
    clipping_dirty: &mut bool,
    next_tab: AnalysisTab,
) {
    if *analysis_tab != next_tab {
        *analysis_tab = next_tab;
        *analysis_dirty = true;
        *clipping_dirty = true;
    }
}

fn apply_clip_enabled_change(clip_enabled: &mut bool, clipping_dirty: &mut bool, enabled: bool) {
    if *clip_enabled != enabled {
        *clip_enabled = enabled;
        *clipping_dirty = true;
    }
}

fn apply_clipping_shadow_threshold_change(
    clipping_settings: &mut ClippingSettings,
    clipping_dirty: &mut bool,
    threshold: f32,
) {
    let threshold = threshold.clamp(0.0, 1.0);
    if (clipping_settings.shadow_threshold - threshold).abs() > f32::EPSILON {
        clipping_settings.shadow_threshold = threshold;
        *clipping_dirty = true;
    }
}

fn apply_clipping_highlight_threshold_change(
    clipping_settings: &mut ClippingSettings,
    clipping_dirty: &mut bool,
    threshold: f32,
) {
    let threshold = threshold.clamp(0.0, 1.0);
    if (clipping_settings.highlight_threshold - threshold).abs() > f32::EPSILON {
        clipping_settings.highlight_threshold = threshold;
        *clipping_dirty = true;
    }
}

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
            self.pixel_overlay_dirty = true;
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
            self.pixel_overlay_dirty = true;
        }

        self.shader_space.render();
        self.render_texture_fps_tracker
            .record_render_update(frame_time);

        texture_bridge::ensure_output_texture_registered(self, render_state, &mut renderer_guard);

        let output_texture_name = self.output_texture_name.as_str();
        let (display_texture_name, display_texture) =
            if let Some(preview_name) = self.preview_texture_name.as_ref() {
                if let Some(texture) = self.shader_space.textures.get(preview_name.as_str()) {
                    (preview_name.as_str(), Some(texture))
                } else {
                    (
                        output_texture_name,
                        self.shader_space.textures.get(output_texture_name),
                    )
                }
            } else {
                (
                    output_texture_name,
                    self.shader_space.textures.get(output_texture_name),
                )
            };

        let render_source = display_texture.and_then(|texture| {
            texture
                .wgpu_texture_view
                .as_ref()
                .map(|view| AnalysisSourceDomain {
                    texture_name: display_texture_name,
                    view,
                    size: [
                        texture.wgpu_texture_desc.size.width,
                        texture.wgpu_texture_desc.size.height,
                    ],
                    format: texture.wgpu_texture_desc.format,
                })
        });
        let render_source_key = render_source.as_ref().map(analysis_source_request_key);

        let mut computed_diff_stats = None;
        let mut did_update_diff_output = false;

        if let Some(reference) = self.ref_image.as_ref()
            && let Some(source) = render_source.as_ref()
            && let Some(source_key) = render_source_key
        {
            let reference_mode = reference.mode;
            let reference_offset = [
                reference.offset.x.round() as i32,
                reference.offset.y.round() as i32,
            ];
            let ref_size = reference.size;
            let diff_output_format = ui::diff_renderer::select_diff_output_format(
                source.format,
                reference.texture_format,
            );
            let needs_recreate = self
                .diff_renderer
                .as_ref()
                .map(|r| r.output_size() != ref_size || r.output_format() != diff_output_format)
                .unwrap_or(true);
            if needs_recreate {
                self.diff_renderer = Some(ui::diff_renderer::DiffRenderer::new(
                    &render_state.device,
                    ref_size,
                    diff_output_format,
                ));
            }

            if let Some(diff_renderer) = self.diff_renderer.as_mut() {
                let request_key = diff_request_key(
                    source_key,
                    reference.size,
                    reference_offset,
                    self.diff_metric_mode,
                    self.hdr_preview_clamp_enabled,
                );
                let stats_key = diff_stats_request_key(request_key);
                let should_update_diff = matches!(reference_mode, RefImageMode::Diff)
                    && (self.diff_dirty
                        || needs_recreate
                        || self.diff_texture_id.is_none()
                        || self.last_diff_request_key != Some(request_key)
                        || self.last_diff_stats_request_key != Some(stats_key));

                if should_update_diff {
                    let diff_stats = diff_renderer.update(
                        &render_state.device,
                        self.shader_space.queue.as_ref(),
                        source.view,
                        source.size,
                        &reference.wgpu_texture_view,
                        reference.size,
                        reference_offset,
                        self.diff_metric_mode,
                        self.hdr_preview_clamp_enabled,
                        true,
                    );
                    did_update_diff_output = true;
                    self.last_diff_request_key = Some(request_key);
                    self.last_diff_stats_request_key = Some(stats_key);
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
                }
            }
        }

        let mut analysis_source = render_source;
        if matches!(
            self.ref_image.as_ref().map(|r| r.mode),
            Some(RefImageMode::Diff)
        ) && let Some(diff_renderer) = self.diff_renderer.as_ref()
        {
            analysis_source = Some(AnalysisSourceDomain {
                texture_name: "sys.diff.analysis",
                view: diff_renderer.analysis_output_view(),
                size: diff_renderer.analysis_output_size(),
                format: diff_renderer.output_format(),
            });
            self.analysis_source_is_diff = true;
        } else {
            self.analysis_source_is_diff = false;
        }
        let analysis_source_key = analysis_source.as_ref().map(|source| {
            let base_key = analysis_source_request_key(source);
            if self.analysis_source_is_diff {
                hash_key(&(base_key, self.last_diff_request_key))
            } else {
                base_key
            }
        });
        self.analysis_source_key = analysis_source_key;

        if computed_diff_stats.is_some() {
            self.diff_stats = computed_diff_stats;
        } else if !matches!(
            self.ref_image.as_ref().map(|r| r.mode),
            Some(RefImageMode::Diff)
        ) {
            self.diff_stats = None;
            self.last_diff_stats_request_key = None;
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

        let mut did_update_active_analysis = false;
        let mut did_update_clipping = false;

        if let Some(source) = analysis_source.as_ref()
            && let Some(source_key) = analysis_source_key
        {
            match self.analysis_tab {
                AnalysisTab::Histogram => {
                    let request_key = histogram_request_key(source_key);
                    let should_update = self.analysis_dirty
                        || self.histogram_texture_id.is_none()
                        || self.last_histogram_request_key != Some(request_key);
                    if should_update
                        && let Some(histogram_renderer) = self.histogram_renderer.as_ref()
                    {
                        histogram_renderer.update(
                            &render_state.device,
                            self.shader_space.queue.as_ref(),
                            source.view,
                            source.size,
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
                        self.last_histogram_request_key = Some(request_key);
                        did_update_active_analysis = true;
                    }
                }
                AnalysisTab::Parade => {
                    let request_key = parade_request_key(source_key);
                    let should_update = self.analysis_dirty
                        || self.parade_texture_id.is_none()
                        || self.last_parade_request_key != Some(request_key);
                    if should_update && let Some(parade_renderer) = self.parade_renderer.as_ref() {
                        parade_renderer.update(
                            &render_state.device,
                            self.shader_space.queue.as_ref(),
                            source.view,
                            source.size,
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
                        self.last_parade_request_key = Some(request_key);
                        did_update_active_analysis = true;
                    }
                }
                AnalysisTab::Vectorscope => {
                    let request_key = vectorscope_request_key(source_key);
                    let should_update = self.analysis_dirty
                        || self.vectorscope_texture_id.is_none()
                        || self.last_vectorscope_request_key != Some(request_key);
                    if should_update
                        && let Some(vectorscope_renderer) = self.vectorscope_renderer.as_ref()
                    {
                        vectorscope_renderer.update(
                            &render_state.device,
                            self.shader_space.queue.as_ref(),
                            source.view,
                            source.size,
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
                        self.last_vectorscope_request_key = Some(request_key);
                        did_update_active_analysis = true;
                    }
                }
            }

            if self.clip_enabled {
                let request_key = clipping_request_key(source_key, self.clipping_settings, true);
                if self.clipping_renderer.is_none() {
                    self.clipping_renderer = Some(ui::clipping_map::ClippingMapRenderer::new(
                        &render_state.device,
                        source.size,
                    ));
                }

                let should_update_clipping = self.analysis_dirty
                    || self.clipping_dirty
                    || self.clipping_texture_id.is_none()
                    || self.last_clipping_request_key != Some(request_key);
                if should_update_clipping
                    && let Some(clipping_renderer) = self.clipping_renderer.as_mut()
                {
                    clipping_renderer.update(
                        &render_state.device,
                        self.shader_space.queue.as_ref(),
                        source.view,
                        source.size,
                        self.clipping_settings.shadow_threshold,
                        self.clipping_settings.highlight_threshold,
                    );

                    let mut sampler =
                        texture_bridge::canvas_sampler_descriptor(wgpu::FilterMode::Nearest);
                    sampler.label = Some("sys.scope.clipping.sampler");

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
                    self.last_clipping_request_key = Some(request_key);
                    did_update_clipping = true;
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

        let sidebar_full_w = ui::debug_sidebar::sidebar_width(ctx);
        let sidebar_w = sidebar_full_w * frame_state.sidebar_factor;
        let reference_sidebar_state =
            self.ref_image
                .as_ref()
                .map(|reference| ui::debug_sidebar::ReferenceSidebarState {
                    name: reference.name.clone(),
                    mode: reference.mode,
                    opacity: reference.opacity,
                    diff_metric_mode: self.diff_metric_mode,
                    diff_stats: self.diff_stats,
                });
        let analysis_sidebar_state = ui::debug_sidebar::AnalysisSidebarState {
            tab: self.analysis_tab,
            clipping: self.clipping_settings,
            clip_enabled: self.clip_enabled,
        };

        // Rebuild resource snapshot when needed (pipeline changed or first frame).
        if self.resource_snapshot_generation != self.pipeline_rebuild_count {
            let snap = ui::resource_tree::ResourceSnapshot::capture(
                &self.shader_space,
                &self.passes,
                Some(self.output_texture_name.as_str()),
            );
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
                self.pixel_overlay_dirty = true;
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
                self.pixel_overlay_dirty = true;
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
                }
            }
            Some(ui::debug_sidebar::SidebarAction::PickReferenceImage) => {
                if let Err(e) = canvas_controller::pick_reference_image_from_dialog(
                    self,
                    ctx,
                    render_state,
                    &mut renderer_guard,
                ) {
                    eprintln!(
                        "[reference-image] failed to load manually-picked reference image: {e:#}"
                    );
                }
            }
            Some(ui::debug_sidebar::SidebarAction::RemoveReferenceImage) => {
                if self.ref_image.is_some() {
                    canvas_controller::clear_reference(self, &mut renderer_guard);
                }
            }
            Some(ui::debug_sidebar::SidebarAction::SetDiffMetricMode(mode)) => {
                if self.diff_metric_mode != mode {
                    self.diff_metric_mode = mode;
                    self.diff_dirty = true;
                }
            }
            Some(ui::debug_sidebar::SidebarAction::SetAnalysisTab(tab)) => {
                apply_analysis_tab_change(
                    &mut self.analysis_tab,
                    &mut self.analysis_dirty,
                    &mut self.clipping_dirty,
                    tab,
                );
            }
            Some(ui::debug_sidebar::SidebarAction::SetClipEnabled(enabled)) => {
                apply_clip_enabled_change(
                    &mut self.clip_enabled,
                    &mut self.clipping_dirty,
                    enabled,
                );
            }
            Some(ui::debug_sidebar::SidebarAction::SetClippingShadowThreshold(threshold)) => {
                apply_clipping_shadow_threshold_change(
                    &mut self.clipping_settings,
                    &mut self.clipping_dirty,
                    threshold,
                );
            }
            Some(ui::debug_sidebar::SidebarAction::SetClippingHighlightThreshold(threshold)) => {
                apply_clipping_highlight_threshold_change(
                    &mut self.clipping_settings,
                    &mut self.clipping_dirty,
                    threshold,
                );
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

        if request_toggle_from_canvas {
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
                "Node Forge Render Server - x={} y={} rgba=({:.3}, {:.3}, {:.3}, {:.3})",
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

#[cfg(test)]
mod tests {
    use super::{
        AnalysisTab, ClippingSettings, DiffMetricMode, apply_analysis_tab_change,
        apply_clip_enabled_change, apply_clipping_highlight_threshold_change,
        apply_clipping_shadow_threshold_change, clipping_request_key, diff_request_key, hash_key,
        histogram_request_key, parade_request_key, vectorscope_request_key,
    };

    #[test]
    fn switching_infographics_tab_does_not_disable_clip() {
        let mut tab = AnalysisTab::Histogram;
        let clip_enabled = true;
        let mut analysis_dirty = false;
        let mut clipping_dirty = false;

        apply_analysis_tab_change(
            &mut tab,
            &mut analysis_dirty,
            &mut clipping_dirty,
            AnalysisTab::Vectorscope,
        );

        assert_eq!(tab, AnalysisTab::Vectorscope);
        assert!(analysis_dirty);
        assert!(clipping_dirty);
        assert!(clip_enabled);
    }

    #[test]
    fn toggling_clip_does_not_change_infographics_tab() {
        let tab = AnalysisTab::Parade;
        let mut clip_enabled = false;
        let mut clipping_dirty = false;

        apply_clip_enabled_change(&mut clip_enabled, &mut clipping_dirty, true);

        assert!(clip_enabled);
        assert!(clipping_dirty);
        assert_eq!(tab, AnalysisTab::Parade);
    }

    #[test]
    fn changing_clip_threshold_marks_clipping_dirty() {
        let mut clipping = ClippingSettings::default();
        let mut clipping_dirty = false;

        apply_clipping_shadow_threshold_change(&mut clipping, &mut clipping_dirty, 0.05);
        assert!(clipping_dirty);

        clipping_dirty = false;
        apply_clipping_highlight_threshold_change(&mut clipping, &mut clipping_dirty, 0.95);
        assert!(clipping_dirty);
    }

    #[test]
    fn request_keys_change_with_source_domain() {
        let source_a = hash_key(&(
            "output",
            [128_u32, 128_u32],
            super::wgpu::TextureFormat::Rgba8Unorm,
        ));
        let source_b = hash_key(&(
            "output",
            [128_u32, 128_u32],
            super::wgpu::TextureFormat::Rgba16Float,
        ));
        assert_ne!(
            histogram_request_key(source_a),
            histogram_request_key(source_b)
        );
        assert_ne!(parade_request_key(source_a), parade_request_key(source_b));
        assert_ne!(
            vectorscope_request_key(source_a),
            vectorscope_request_key(source_b)
        );
    }

    #[test]
    fn diff_request_key_changes_with_offset_and_metric() {
        let source_key = hash_key(&("output", [320_u32, 180_u32]));
        let key_1 = diff_request_key(source_key, [64, 64], [0, 0], DiffMetricMode::AE, false);
        let key_2 = diff_request_key(source_key, [64, 64], [1, 0], DiffMetricMode::AE, false);
        let key_3 = diff_request_key(source_key, [64, 64], [0, 0], DiffMetricMode::SE, false);
        let key_4 = diff_request_key(source_key, [64, 64], [0, 0], DiffMetricMode::AE, true);
        assert_ne!(key_1, key_2);
        assert_ne!(key_1, key_3);
        assert_ne!(key_1, key_4);
    }

    #[test]
    fn clipping_request_key_changes_with_toggle_and_thresholds() {
        let source_key = hash_key(&("output", [1920_u32, 1080_u32]));
        let base_settings = ClippingSettings::default();
        let base = clipping_request_key(source_key, base_settings, true);
        let toggled = clipping_request_key(source_key, base_settings, false);
        let changed_threshold = clipping_request_key(
            source_key,
            ClippingSettings {
                shadow_threshold: 0.05,
                ..base_settings
            },
            true,
        );
        assert_ne!(base, toggled);
        assert_ne!(base, changed_threshold);
    }
}
