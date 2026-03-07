mod canvas;
mod interaction_report;
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

use crate::{app::types::AnalysisSourceDomain, protocol, renderer, ui};

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
    reference_mode: RefImageMode,
    reference_opacity_bits: u32,
    metric_mode: DiffMetricMode,
    clamp_output: bool,
) -> u64 {
    hash_key(&(
        source_key,
        reference_size,
        reference_offset,
        reference_mode,
        reference_opacity_bits,
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

fn should_request_immediate_repaint(
    time_driven_scene: bool,
    sidebar_animating: bool,
    pan_zoom_animating: bool,
    operation_indicator_visible: bool,
    capture_redraw_active: bool,
) -> bool {
    time_driven_scene
        || sidebar_animating
        || pan_zoom_animating
        || operation_indicator_visible
        || capture_redraw_active
}

fn broadcast_state_interaction_event(
    app: &mut App,
    event_type: &str,
    state_id: &str,
    transition_id: Option<&str>,
) {
    app.canvas.interactions.interaction_event_seq = app
        .canvas
        .interactions
        .interaction_event_seq
        .saturating_add(1);
    let payload = protocol::InteractionEventPayload {
        event_type: event_type.to_string(),
        seq: app.canvas.interactions.interaction_event_seq,
        data: Some(protocol::InteractionEventData {
            state: Some(protocol::InteractionStateData {
                state_id: state_id.to_string(),
                transition_id: transition_id.map(str::to_string),
            }),
            ..protocol::InteractionEventData::default()
        }),
    };
    let message = protocol::WSMessage {
        msg_type: "interaction_event".to_string(),
        timestamp: protocol::now_millis(),
        request_id: None,
        payload: Some(payload),
    };
    if let Ok(text) = serde_json::to_string(&message) {
        app.ws_hub.broadcast(text);
    }
}

fn sync_animation_state_interaction_events(
    app: &mut App,
    current_state_id: Option<&str>,
    transition_id: Option<&str>,
) {
    let previous_state_id = app
        .canvas
        .interactions
        .last_synced_animation_state_id
        .clone();
    if previous_state_id.as_deref() == current_state_id {
        return;
    }

    if let Some(prev) = previous_state_id.as_deref() {
        broadcast_state_interaction_event(app, "stateleave", prev, transition_id);
    }
    if let Some(curr) = current_state_id {
        broadcast_state_interaction_event(app, "stateenter", curr, transition_id);
    }

    app.canvas.interactions.last_synced_animation_state_id = current_state_id.map(str::to_string);
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

        let mut latest_capture_state = None;
        if let Some(capture_state_rx) = self.capture_state_rx.as_ref() {
            while let Ok(capture_active) = capture_state_rx.try_recv() {
                latest_capture_state = Some(capture_active);
            }
        }
        if let Some(capture_active) = latest_capture_state {
            if self.capture_redraw_active != capture_active {
                if capture_active {
                    eprintln!("[capture] enabling continuous redraw for active capture session");
                } else {
                    eprintln!("[capture] disabling continuous redraw after capture session");
                }
            }
            self.capture_redraw_active = capture_active;
            if capture_active {
                self.scene_redraw_pending = true;
            }
        }

        let mut did_rebuild_shader_space = false;
        if let Some(update) = scene_runtime::drain_latest_scene_update(self) {
            let apply_result = scene_runtime::apply_scene_update(self, ctx, render_state, update);
            self.scene_redraw_pending = true;
            self.canvas.invalidation.preview_source_changed();
            if apply_result.did_rebuild_shader_space {
                let filter = apply_result
                    .texture_filter_override
                    .unwrap_or(self.canvas.display.texture_filter);
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
                self.canvas.viewport.pending_view_reset = true;
            }
        }

        canvas::sync_reference_from_scene(self, ctx, render_state);

        let raw_t = self.start.elapsed().as_secs_f32();
        let delta_t = (raw_t - self.time_last_raw_secs).max(0.0);
        self.time_last_raw_secs = raw_t;

        // ── Early interaction-event collection ──────────────────────────
        // Collect canvas interaction events and queue them into
        // `pending_events` BEFORE `session.step()` runs, so that
        // interaction-triggered state-machine transitions are processed
        // in the same frame the input arrives.
        let early_interaction_payloads = canvas::collect_interaction_events(self, ctx);

        // ── Animation session tick (fixed-step, deterministic) ─────────
        let effective_dt = if self.time_updates_enabled && self.animation_playing {
            delta_t
        } else {
            0.0
        };
        // Step the session first, collecting the result (drops borrow on
        // self.animation_session so we can mutate other App fields freely).
        let anim_step = if self.animation_playing {
            self.animation_session
                .as_mut()
                .map(|session| session.step(effective_dt as f64))
        } else {
            None
        };

        let mut animation_values_changed = false;
        let mut animation_current_state_id: Option<String> = None;
        let mut animation_active_transition_id: Option<String> = None;
        if let Some(step) = anim_step {
            animation_current_state_id = Some(step.current_state_id.clone());
            animation_active_transition_id = step.active_transition_id.clone();
            if step.needs_redraw {
                animation_values_changed = true;
                // Phase 1: apply value overrides to the cached uniform scene.
                if let Some(ref mut uniform_scene) = self.uniform_scene {
                    crate::state_machine::apply_overrides(uniform_scene, &step.active_overrides);
                }
                // Phase 2: push updated uniforms to GPU buffers (split
                // borrows: passes + shader_space + uniform_scene).
                if let Some(ref uniform_scene) = self.uniform_scene {
                    let _ = scene_runtime::apply_graph_uniform_updates_parts(
                        &mut self.passes,
                        &mut self.shader_space,
                        uniform_scene,
                    );
                }
                self.scene_redraw_pending = true;
            }
            // Unified clock: fixed-step scene time drives params.time.
            self.time_value_secs = step.scene_time_secs as f32;

            // Cache dynamic data from the step for the state machine panel.
            self.canvas.interactions.cached_state_local_times =
                step.state_local_times.into_iter().collect();
            self.canvas.interactions.cached_transition_blend = step.transition_blend;
            self.canvas.interactions.cached_override_values = step
                .active_overrides
                .iter()
                .map(|(k, v)| {
                    let key = format!("{}:{}", k.node_id, k.param_name);
                    let val = ui::state_machine_panel::format_json_value_2dp(v);
                    (key, val)
                })
                .collect();
            self.canvas
                .interactions
                .cached_override_values
                .sort_by(|a, b| a.0.cmp(&b.0));
        } else {
            // No animation session — advance time with wall-clock as before.
            if self.time_updates_enabled {
                self.time_value_secs += delta_t;
            }
        }

        sync_animation_state_interaction_events(
            self,
            animation_current_state_id.as_deref(),
            animation_active_transition_id.as_deref(),
        );

        let time_driven_scene = self.scene_uses_time && self.time_updates_enabled;
        let should_redraw_scene =
            self.scene_redraw_pending || time_driven_scene || self.capture_redraw_active;

        if should_redraw_scene {
            let t = self.time_value_secs;
            for pass in &mut self.passes {
                let mut p = pass.base_params;
                p.time = t;
                let _ = renderer::update_pass_params(&self.shader_space, pass, &p);
            }

            if time_driven_scene || animation_values_changed {
                if self.canvas.reference.ref_image.is_some() {
                    self.canvas.invalidation.mark_diff_dirty();
                }
                self.canvas.invalidation.mark_analysis_dirty();
                self.canvas.invalidation.mark_clipping_dirty();
                self.canvas.invalidation.mark_pixel_overlay_dirty();
            }

            self.shader_space.render();
            self.scene_redraw_pending = false;
            self.render_texture_fps_tracker
                .record_scene_redraw(frame_time);
        }

        texture_bridge::ensure_output_texture_registered(self, render_state, &mut renderer_guard);

        let output_texture_name = self.output_texture_name.as_str();
        let (display_texture_name, display_texture) =
            if let Some(preview_name) = self.canvas.display.preview_texture_name.as_ref() {
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

        let display_source = display_texture.and_then(|texture| {
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
        let compare_source_key = display_source.as_ref().map(analysis_source_request_key);

        let mut computed_diff_stats = None;
        let mut did_update_diff_output = false;

        if let Some(reference) = self.canvas.reference.ref_image.as_ref()
            && let Some(source) = display_source.as_ref()
            && let Some(source_key) = compare_source_key
        {
            let reference_mode = reference.mode;
            let reference_offset = [
                reference.offset.x.round() as i32,
                reference.offset.y.round() as i32,
            ];
            let diff_output_format = ui::diff_renderer::select_diff_output_format(
                source.format,
                reference.texture_format,
            );
            let needs_recreate = self
                .canvas
                .analysis
                .diff_renderer
                .as_ref()
                .map(|r| r.output_size() != source.size || r.output_format() != diff_output_format)
                .unwrap_or(true);
            if needs_recreate {
                self.canvas.analysis.diff_renderer = Some(ui::diff_renderer::DiffRenderer::new(
                    &render_state.device,
                    source.size,
                    diff_output_format,
                ));
            }

            if let Some(diff_renderer) = self.canvas.analysis.diff_renderer.as_mut() {
                let request_key = diff_request_key(
                    source_key,
                    reference.size,
                    reference_offset,
                    reference_mode,
                    reference.opacity.to_bits(),
                    self.canvas.analysis.diff_metric_mode,
                    self.canvas.display.hdr_preview_clamp_enabled,
                );
                let stats_key = diff_stats_request_key(request_key);
                let collect_stats = matches!(reference_mode, RefImageMode::Diff);
                let should_update_diff = self.canvas.invalidation.diff_dirty()
                    || should_redraw_scene
                    || needs_recreate
                    || self.canvas.analysis.diff_texture_id.is_none()
                    || self.canvas.analysis.last_diff_request_key != Some(request_key)
                    || (collect_stats
                        && self.canvas.analysis.last_diff_stats_request_key != Some(stats_key));

                if should_update_diff {
                    let diff_stats = diff_renderer.update(
                        &render_state.device,
                        self.shader_space.queue.as_ref(),
                        source.view,
                        source.size,
                        &reference.wgpu_texture_view,
                        reference.size,
                        reference_offset,
                        reference_mode,
                        reference.opacity,
                        self.canvas.analysis.diff_metric_mode,
                        self.canvas.display.hdr_preview_clamp_enabled,
                        collect_stats,
                    );
                    did_update_diff_output = true;
                    self.canvas.analysis.last_diff_request_key = Some(request_key);
                    if collect_stats {
                        self.canvas.analysis.last_diff_stats_request_key = Some(stats_key);
                        computed_diff_stats = diff_stats;
                    } else {
                        self.canvas.analysis.last_diff_stats_request_key = None;
                    }
                }

                if did_update_diff_output {
                    let mut sampler =
                        texture_bridge::diff_sampler_descriptor(self.canvas.display.texture_filter);
                    sampler.label = Some("sys.diff.sampler");

                    if let Some(id) = self.canvas.analysis.diff_texture_id {
                        renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                            &render_state.device,
                            diff_renderer.output_view(),
                            sampler,
                            id,
                        );
                    } else {
                        self.canvas.analysis.diff_texture_id =
                            Some(renderer_guard.register_native_texture_with_sampler_options(
                                &render_state.device,
                                diff_renderer.output_view(),
                                sampler,
                            ));
                    }
                    self.canvas.invalidation.clear_diff();
                }
            }
        }

        let mut analysis_source = display_source;
        if matches!(
            self.canvas.reference.ref_image.as_ref().map(|r| r.mode),
            Some(RefImageMode::Diff)
        ) && let Some(diff_renderer) = self.canvas.analysis.diff_renderer.as_ref()
        {
            analysis_source = Some(AnalysisSourceDomain {
                texture_name: "sys.diff.analysis",
                view: diff_renderer.analysis_output_view(),
                size: diff_renderer.analysis_output_size(),
                format: diff_renderer.output_format(),
            });
            self.canvas.analysis.analysis_source_is_diff = true;
        } else {
            self.canvas.analysis.analysis_source_is_diff = false;
        }
        let analysis_source_key = analysis_source.as_ref().map(|source| {
            let base_key = analysis_source_request_key(source);
            if self.canvas.analysis.analysis_source_is_diff {
                hash_key(&(base_key, self.canvas.analysis.last_diff_request_key))
            } else {
                base_key
            }
        });
        self.canvas.analysis.analysis_source_key = analysis_source_key;

        if computed_diff_stats.is_some() {
            self.canvas.analysis.diff_stats = computed_diff_stats;
        } else if !matches!(
            self.canvas.reference.ref_image.as_ref().map(|r| r.mode),
            Some(RefImageMode::Diff)
        ) {
            self.canvas.analysis.diff_stats = None;
            self.canvas.analysis.last_diff_stats_request_key = None;
        }

        if self.canvas.analysis.histogram_renderer.is_none() {
            self.canvas.analysis.histogram_renderer =
                Some(ui::histogram::HistogramRenderer::new(&render_state.device));
        }
        if self.canvas.analysis.parade_renderer.is_none() {
            self.canvas.analysis.parade_renderer =
                Some(ui::parade::ParadeRenderer::new(&render_state.device));
        }
        if self.canvas.analysis.vectorscope_renderer.is_none() {
            self.canvas.analysis.vectorscope_renderer = Some(
                ui::vectorscope::VectorscopeRenderer::new(&render_state.device),
            );
        }

        let mut did_update_active_analysis = false;
        let mut did_update_clipping = false;

        if let Some(source) = analysis_source.as_ref()
            && let Some(source_key) = analysis_source_key
        {
            match self.canvas.analysis.analysis_tab {
                AnalysisTab::Histogram => {
                    let request_key = histogram_request_key(source_key);
                    let should_update = self.canvas.invalidation.analysis_dirty()
                        || self.canvas.analysis.histogram_texture_id.is_none()
                        || self.canvas.analysis.last_histogram_request_key != Some(request_key);
                    if should_update
                        && let Some(histogram_renderer) =
                            self.canvas.analysis.histogram_renderer.as_ref()
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

                        if let Some(id) = self.canvas.analysis.histogram_texture_id {
                            renderer_guard
                                .update_egui_texture_from_wgpu_texture_with_sampler_options(
                                    &render_state.device,
                                    histogram_renderer.output_view(),
                                    sampler,
                                    id,
                                );
                        } else {
                            self.canvas.analysis.histogram_texture_id =
                                Some(renderer_guard.register_native_texture_with_sampler_options(
                                    &render_state.device,
                                    histogram_renderer.output_view(),
                                    sampler,
                                ));
                        }
                        self.canvas.analysis.last_histogram_request_key = Some(request_key);
                        did_update_active_analysis = true;
                    }
                }
                AnalysisTab::Parade => {
                    let request_key = parade_request_key(source_key);
                    let should_update = self.canvas.invalidation.analysis_dirty()
                        || self.canvas.analysis.parade_texture_id.is_none()
                        || self.canvas.analysis.last_parade_request_key != Some(request_key);
                    if should_update
                        && let Some(parade_renderer) = self.canvas.analysis.parade_renderer.as_ref()
                    {
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
                        if let Some(id) = self.canvas.analysis.parade_texture_id {
                            renderer_guard
                                .update_egui_texture_from_wgpu_texture_with_sampler_options(
                                    &render_state.device,
                                    parade_renderer.parade_output_view(),
                                    parade_sampler,
                                    id,
                                );
                        } else {
                            self.canvas.analysis.parade_texture_id =
                                Some(renderer_guard.register_native_texture_with_sampler_options(
                                    &render_state.device,
                                    parade_renderer.parade_output_view(),
                                    parade_sampler,
                                ));
                        }
                        self.canvas.analysis.last_parade_request_key = Some(request_key);
                        did_update_active_analysis = true;
                    }
                }
                AnalysisTab::Vectorscope => {
                    let request_key = vectorscope_request_key(source_key);
                    let should_update = self.canvas.invalidation.analysis_dirty()
                        || self.canvas.analysis.vectorscope_texture_id.is_none()
                        || self.canvas.analysis.last_vectorscope_request_key != Some(request_key);
                    if should_update
                        && let Some(vectorscope_renderer) =
                            self.canvas.analysis.vectorscope_renderer.as_ref()
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

                        if let Some(id) = self.canvas.analysis.vectorscope_texture_id {
                            renderer_guard
                                .update_egui_texture_from_wgpu_texture_with_sampler_options(
                                    &render_state.device,
                                    vectorscope_renderer.output_view(),
                                    sampler,
                                    id,
                                );
                        } else {
                            self.canvas.analysis.vectorscope_texture_id =
                                Some(renderer_guard.register_native_texture_with_sampler_options(
                                    &render_state.device,
                                    vectorscope_renderer.output_view(),
                                    sampler,
                                ));
                        }
                        self.canvas.analysis.last_vectorscope_request_key = Some(request_key);
                        did_update_active_analysis = true;
                    }
                }
            }

            if self.canvas.analysis.clip_enabled {
                let request_key =
                    clipping_request_key(source_key, self.canvas.analysis.clipping_settings, true);
                if self.canvas.analysis.clipping_renderer.is_none() {
                    self.canvas.analysis.clipping_renderer =
                        Some(ui::clipping_map::ClippingMapRenderer::new(
                            &render_state.device,
                            source.size,
                        ));
                }

                let should_update_clipping = self.canvas.invalidation.analysis_dirty()
                    || self.canvas.invalidation.clipping_dirty()
                    || self.canvas.analysis.clipping_texture_id.is_none()
                    || self.canvas.analysis.last_clipping_request_key != Some(request_key);
                if should_update_clipping
                    && let Some(clipping_renderer) = self.canvas.analysis.clipping_renderer.as_mut()
                {
                    clipping_renderer.update(
                        &render_state.device,
                        self.shader_space.queue.as_ref(),
                        source.view,
                        source.size,
                        self.canvas.analysis.clipping_settings.shadow_threshold,
                        self.canvas.analysis.clipping_settings.highlight_threshold,
                    );

                    let mut sampler =
                        texture_bridge::canvas_sampler_descriptor(wgpu::FilterMode::Nearest);
                    sampler.label = Some("sys.scope.clipping.sampler");

                    if let Some(id) = self.canvas.analysis.clipping_texture_id {
                        renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                            &render_state.device,
                            clipping_renderer.output_view(),
                            sampler,
                            id,
                        );
                    } else {
                        self.canvas.analysis.clipping_texture_id =
                            Some(renderer_guard.register_native_texture_with_sampler_options(
                                &render_state.device,
                                clipping_renderer.output_view(),
                                sampler,
                            ));
                    }
                    self.canvas.analysis.last_clipping_request_key = Some(request_key);
                    did_update_clipping = true;
                }
            }
        }

        if did_update_active_analysis {
            self.canvas.invalidation.clear_analysis();
        }
        if did_update_clipping {
            self.canvas.invalidation.clear_clipping();
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
        let reference_sidebar_state = self.canvas.reference.ref_image.as_ref().map(|reference| {
            ui::debug_sidebar::ReferenceSidebarState {
                name: reference.name.clone(),
                mode: reference.mode,
                opacity: reference.opacity,
                diff_metric_mode: self.canvas.analysis.diff_metric_mode,
                diff_stats: self.canvas.analysis.diff_stats,
            }
        });
        let analysis_sidebar_state = ui::debug_sidebar::AnalysisSidebarState {
            tab: self.canvas.analysis.analysis_tab,
            clipping: self.canvas.analysis.clipping_settings,
            clip_enabled: self.canvas.analysis.clip_enabled,
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

                    let sm_snapshot = self.animation_session.as_ref().map(|session| {
                        let mut snap = ui::state_machine_panel::snapshot_from_session(session);
                        snap.state_local_times =
                            self.canvas.interactions.cached_state_local_times.clone();
                        snap.transition_blend = self.canvas.interactions.cached_transition_blend;
                        snap.override_values =
                            self.canvas.interactions.cached_override_values.clone();
                        snap
                    });

                    sidebar_action = ui::debug_sidebar::show_in_rect(
                        ctx,
                        ui,
                        frame_state.sidebar_factor,
                        frame_state.animation_just_finished_opening,
                        clip_rect,
                        sidebar_rect,
                        self.canvas.analysis.histogram_texture_id,
                        self.canvas.analysis.parade_texture_id,
                        self.canvas.analysis.vectorscope_texture_id,
                        analysis_sidebar_state,
                        reference_sidebar_state.as_ref(),
                        &self.resource_tree_nodes,
                        &mut self.file_tree_state,
                        sm_snapshot.as_ref(),
                    );
                });
        }

        // Handle sidebar actions.
        match sidebar_action {
            Some(ui::debug_sidebar::SidebarAction::PreviewTexture(name)) => {
                let _ = canvas::reducer::apply_action(
                    self,
                    render_state,
                    &mut renderer_guard,
                    canvas::actions::CanvasAction::SetPreviewTexture(
                        rust_wgpu_fiber::ResourceName::from(name.as_str()),
                    ),
                );
            }
            Some(ui::debug_sidebar::SidebarAction::ClearPreview) => {
                let _ = canvas::reducer::apply_action(
                    self,
                    render_state,
                    &mut renderer_guard,
                    canvas::actions::CanvasAction::ClearPreviewTexture,
                );
            }
            Some(ui::debug_sidebar::SidebarAction::SetReferenceOpacity(opacity)) => {
                let _ = canvas::reducer::apply_action(
                    self,
                    render_state,
                    &mut renderer_guard,
                    canvas::actions::CanvasAction::SetReferenceOpacity(opacity),
                );
            }
            Some(ui::debug_sidebar::SidebarAction::ToggleReferenceMode) => {
                let _ = canvas::reducer::apply_action(
                    self,
                    render_state,
                    &mut renderer_guard,
                    canvas::actions::CanvasAction::ToggleReferenceMode,
                );
            }
            Some(ui::debug_sidebar::SidebarAction::PickReferenceImage) => {
                if let Err(e) = canvas::pick_reference_image_from_dialog(self, ctx, render_state) {
                    eprintln!(
                        "[reference-image] failed to load manually-picked reference image: {e:#}"
                    );
                }
            }
            Some(ui::debug_sidebar::SidebarAction::RemoveReferenceImage) => {
                if self.canvas.reference.ref_image.is_some() {
                    canvas::clear_reference(self);
                }
            }
            Some(ui::debug_sidebar::SidebarAction::SetDiffMetricMode(mode)) => {
                let _ = canvas::reducer::apply_action(
                    self,
                    render_state,
                    &mut renderer_guard,
                    canvas::actions::CanvasAction::SetDiffMetricMode(mode),
                );
            }
            Some(ui::debug_sidebar::SidebarAction::SetAnalysisTab(tab)) => {
                let _ = canvas::reducer::apply_action(
                    self,
                    render_state,
                    &mut renderer_guard,
                    canvas::actions::CanvasAction::SetAnalysisTab(tab),
                );
            }
            Some(ui::debug_sidebar::SidebarAction::SetClipEnabled(enabled)) => {
                let _ = canvas::reducer::apply_action(
                    self,
                    render_state,
                    &mut renderer_guard,
                    canvas::actions::CanvasAction::SetClipEnabled(enabled),
                );
            }
            Some(ui::debug_sidebar::SidebarAction::SetClippingShadowThreshold(threshold)) => {
                let _ = canvas::reducer::apply_action(
                    self,
                    render_state,
                    &mut renderer_guard,
                    canvas::actions::CanvasAction::SetClippingShadowThreshold(threshold),
                );
            }
            Some(ui::debug_sidebar::SidebarAction::SetClippingHighlightThreshold(threshold)) => {
                let _ = canvas::reducer::apply_action(
                    self,
                    render_state,
                    &mut renderer_guard,
                    canvas::actions::CanvasAction::SetClippingHighlightThreshold(threshold),
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
                request_toggle_from_canvas = canvas::show(
                    self,
                    ctx,
                    ui,
                    render_state,
                    &mut renderer_guard,
                    frame_state,
                    now,
                    early_interaction_payloads,
                )
                .request_toggle_canvas_only;
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

        let title = if let Some(sampled) = self.canvas.viewport.last_sampled {
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

        let sidebar_animating = self
            .animations
            .is_active(window_mode::ANIM_KEY_SIDEBAR_FACTOR);
        let pan_zoom_animating = canvas::is_pan_zoom_animating(self);
        let operation_indicator_visible = canvas::ops::is_visible(&self.canvas.async_ops);

        let time_driven_scene_for_schedule = self.scene_uses_time && self.time_updates_enabled;
        let animation_session_active = self.animation_playing
            && self
                .animation_session
                .as_ref()
                .is_some_and(|s| s.is_active());

        if should_request_immediate_repaint(
            time_driven_scene_for_schedule || animation_session_active,
            sidebar_animating,
            pan_zoom_animating,
            operation_indicator_visible,
            self.capture_redraw_active,
        ) {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClippingSettings, DiffMetricMode, RefImageMode, clipping_request_key, diff_request_key,
        hash_key,
        histogram_request_key, parade_request_key, should_request_immediate_repaint,
        vectorscope_request_key,
    };

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
        let key_1 = diff_request_key(
            source_key,
            [64, 64],
            [0, 0],
            RefImageMode::Diff,
            0.5f32.to_bits(),
            DiffMetricMode::AE,
            false,
        );
        let key_2 = diff_request_key(
            source_key,
            [64, 64],
            [1, 0],
            RefImageMode::Diff,
            0.5f32.to_bits(),
            DiffMetricMode::AE,
            false,
        );
        let key_3 = diff_request_key(
            source_key,
            [64, 64],
            [0, 0],
            RefImageMode::Diff,
            0.5f32.to_bits(),
            DiffMetricMode::SE,
            false,
        );
        let key_4 = diff_request_key(
            source_key,
            [64, 64],
            [0, 0],
            RefImageMode::Diff,
            0.5f32.to_bits(),
            DiffMetricMode::AE,
            true,
        );
        let key_5 = diff_request_key(
            source_key,
            [64, 64],
            [0, 0],
            RefImageMode::Overlay,
            0.5f32.to_bits(),
            DiffMetricMode::AE,
            false,
        );
        let key_6 = diff_request_key(
            source_key,
            [64, 64],
            [0, 0],
            RefImageMode::Diff,
            0.25f32.to_bits(),
            DiffMetricMode::AE,
            false,
        );
        assert_ne!(key_1, key_2);
        assert_ne!(key_1, key_3);
        assert_ne!(key_1, key_4);
        assert_ne!(key_1, key_5);
        assert_ne!(key_1, key_6);
    }

    #[test]
    fn diff_request_key_changes_with_source_domain() {
        let source_a = hash_key(&(
            "output",
            [320_u32, 180_u32],
            super::wgpu::TextureFormat::Rgba8Unorm,
        ));
        let source_b = hash_key(&(
            "output",
            [640_u32, 360_u32],
            super::wgpu::TextureFormat::Rgba16Float,
        ));
        let key_a = diff_request_key(
            source_a,
            [64, 64],
            [0, 0],
            RefImageMode::Overlay,
            0.5f32.to_bits(),
            DiffMetricMode::AE,
            false,
        );
        let key_b = diff_request_key(
            source_b,
            [64, 64],
            [0, 0],
            RefImageMode::Overlay,
            0.5f32.to_bits(),
            DiffMetricMode::AE,
            false,
        );
        assert_ne!(key_a, key_b);
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

    #[test]
    fn repaint_policy_requests_immediate_for_time_driven_scene() {
        assert!(should_request_immediate_repaint(
            true, false, false, false, false
        ));
    }

    #[test]
    fn repaint_policy_requests_immediate_for_any_active_animation() {
        assert!(should_request_immediate_repaint(
            false, true, false, false, false
        ));
        assert!(should_request_immediate_repaint(
            false, false, true, false, false
        ));
    }

    #[test]
    fn repaint_policy_requests_immediate_when_operation_indicator_visible() {
        assert!(should_request_immediate_repaint(
            false, false, false, true, false
        ));
    }

    #[test]
    fn repaint_policy_requests_immediate_when_capture_redraw_active() {
        assert!(should_request_immediate_repaint(
            false, false, false, false, true
        ));
    }

    #[test]
    fn repaint_policy_skips_immediate_when_capture_inactive_and_other_triggers_inactive() {
        assert!(!should_request_immediate_repaint(
            false, false, false, false, false
        ));
    }
}
