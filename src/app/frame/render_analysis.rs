use rust_wgpu_fiber::eframe::{egui_wgpu, wgpu};

use crate::{
    app::{
        texture_bridge,
        types::{AnalysisSourceDomain, App, DiffMetricMode, DiffStats, RefImageMode, TestMode},
    },
    renderer, ui,
};

use super::{
    advance::AdvancePhase,
    ingest::IngestPhase,
    request_keys::{
        AnalysisSourceKey, ClippingRequestKey, DiffRequestKey, DiffStatsRequestKey,
        HistogramRequestKey, ParadeRequestKey, QualifierRequestKey, VectorscopeRequestKey,
    },
};

pub(super) fn run(
    app: &mut App,
    render_state: &egui_wgpu::RenderState,
    renderer_guard: &mut egui_wgpu::Renderer,
    ingest: &IngestPhase,
    advance: &AdvancePhase,
) {
    if advance.should_redraw_scene {
        let t = app.runtime.time_value_secs;
        for pass in &mut app.core.passes {
            let mut params = pass.base_params;
            params.time = t;
            let _ = renderer::update_pass_params(&app.core.shader_space, pass, &params);
        }

        if advance.time_driven_scene || advance.animation_values_changed {
            if app.canvas.reference.ref_image.is_some() {
                app.canvas.invalidation.mark_diff_dirty();
            }
            app.canvas.invalidation.mark_analysis_dirty();
            app.canvas.invalidation.mark_clipping_dirty();
            app.canvas.invalidation.mark_qualifier_dirty();
            app.canvas.invalidation.mark_pixel_overlay_dirty();
        }

        app.core.shader_space.render();
        app.runtime.scene_redraw_pending = false;
        app.runtime
            .render_texture_fps_tracker
            .record_scene_redraw(ingest.frame_time);
    }

    texture_bridge::ensure_output_texture_registered(app, render_state, renderer_guard);

    let matrix_active =
        app.shell.test_mode == TestMode::Matrix && !app.shell.matrix_state.cells.is_empty();
    if matrix_active {
        run_matrix_analysis(app, render_state, renderer_guard);
        if ingest.did_rebuild_shader_space {
            let _ = render_state
                .device
                .poll(rust_wgpu_fiber::eframe::wgpu::PollType::Poll);
        }
        return;
    }

    let output_texture_name = app.core.output_texture_name.as_str();
    let (display_texture_name, display_texture) =
        if let Some(preview_name) = app.canvas.display.preview_texture_name.as_ref() {
            if let Some(texture) = app.core.shader_space.textures.get(preview_name.as_str()) {
                (preview_name.as_str(), Some(texture))
            } else {
                (
                    output_texture_name,
                    app.core.shader_space.textures.get(output_texture_name),
                )
            }
        } else {
            (
                output_texture_name,
                app.core.shader_space.textures.get(output_texture_name),
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
    let compare_source_key = display_source.as_ref().map(AnalysisSourceKey::from_source);

    let mut computed_diff_stats: Option<DiffStats> = None;
    let pending_shortwire_diff_capture = app.shell.pending_shortwire_diff_capture.clone();
    let mut computed_shortwire_diff_result = None;
    let mut completed_shortwire_diff_capture = pending_shortwire_diff_capture.is_some()
        && (app.canvas.reference.ref_image.is_none() || compare_source_key.is_none());
    let mut did_update_diff_output = false;

    if let Some(reference) = app.canvas.reference.ref_image.as_ref()
        && let Some(source) = display_source.as_ref()
        && let Some(source_key) = compare_source_key
    {
        let reference_mode = reference.mode;
        let capture_shortwire_diff = pending_shortwire_diff_capture.is_some();
        let effective_reference_mode = if capture_shortwire_diff {
            RefImageMode::Diff
        } else {
            reference_mode
        };
        let effective_metric_mode = if capture_shortwire_diff {
            DiffMetricMode::AE
        } else {
            app.canvas.analysis.diff_metric_mode
        };
        let reference_offset = [
            reference.offset.x.round() as i32,
            reference.offset.y.round() as i32,
        ];
        let diff_output_format =
            ui::diff_renderer::select_diff_output_format(source.format, reference.texture_format);
        let needs_recreate = app
            .canvas
            .analysis
            .diff_renderer
            .as_ref()
            .map(|renderer| {
                renderer.output_size() != source.size
                    || renderer.output_format() != diff_output_format
            })
            .unwrap_or(true);
        if needs_recreate {
            app.canvas.analysis.diff_renderer = Some(ui::diff_renderer::DiffRenderer::new(
                &render_state.device,
                source.size,
                diff_output_format,
            ));
        }

        if let Some(diff_renderer) = app.canvas.analysis.diff_renderer.as_mut() {
            let request_key = DiffRequestKey::new(
                source_key,
                reference.size,
                reference_offset,
                effective_reference_mode,
                reference.opacity.to_bits(),
                effective_metric_mode,
                app.canvas.display.hdr_preview_clamp_enabled,
            );
            let stats_key = DiffStatsRequestKey::new(request_key);
            let collect_stats = matches!(effective_reference_mode, RefImageMode::Diff);
            let should_update_diff = app.canvas.invalidation.diff_dirty()
                || advance.should_redraw_scene
                || needs_recreate
                || app.canvas.analysis.diff_texture_id.is_none()
                || app.canvas.analysis.last_diff_request_key != Some(request_key)
                || (collect_stats
                    && app.canvas.analysis.last_diff_stats_request_key != Some(stats_key));

            if should_update_diff {
                let diff_stats = diff_renderer.update(
                    &render_state.device,
                    app.core.shader_space.queue.as_ref(),
                    source.view,
                    source.size,
                    &reference.wgpu_texture_view,
                    reference.size,
                    reference_offset,
                    effective_reference_mode,
                    reference.opacity,
                    effective_metric_mode,
                    app.canvas.display.hdr_preview_clamp_enabled,
                    collect_stats,
                );
                did_update_diff_output = true;
                app.canvas.analysis.last_diff_request_key = Some(request_key);
                if collect_stats {
                    app.canvas.analysis.last_diff_stats_request_key = Some(stats_key);
                    if matches!(reference_mode, RefImageMode::Diff) {
                        computed_diff_stats = diff_stats;
                    }
                    if let Some(capture) = pending_shortwire_diff_capture.clone() {
                        completed_shortwire_diff_capture = true;
                        computed_shortwire_diff_result = diff_stats.and_then(|stats| {
                            ui::pass_debug_window::ShortwireDiffResult::from_stats(
                                stats,
                                source.size,
                                reference.size,
                                reference_offset,
                            )
                            .map(|diff_result| (capture, diff_result))
                        });
                    }
                } else {
                    app.canvas.analysis.last_diff_stats_request_key = None;
                }
            }

            if did_update_diff_output {
                let mut sampler =
                    texture_bridge::diff_sampler_descriptor(app.canvas.display.texture_filter);
                sampler.label = Some("sys.diff.sampler");

                if let Some(id) = app.canvas.analysis.diff_texture_id {
                    renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                        &render_state.device,
                        diff_renderer.output_view(),
                        sampler,
                        id,
                    );
                } else {
                    app.canvas.analysis.diff_texture_id =
                        Some(renderer_guard.register_native_texture_with_sampler_options(
                            &render_state.device,
                            diff_renderer.output_view(),
                            sampler,
                        ));
                }
                app.canvas.invalidation.clear_diff();
            }
        }
    }

    let mut analysis_source = display_source;
    if matches!(
        app.canvas
            .reference
            .ref_image
            .as_ref()
            .map(|reference| reference.mode),
        Some(RefImageMode::Diff)
    ) && let Some(diff_renderer) = app.canvas.analysis.diff_renderer.as_ref()
    {
        analysis_source = Some(AnalysisSourceDomain {
            texture_name: "sys.diff.analysis",
            view: diff_renderer.analysis_output_view(),
            size: diff_renderer.analysis_output_size(),
            format: diff_renderer.output_format(),
        });
        app.canvas.analysis.analysis_source_is_diff = true;
    } else {
        app.canvas.analysis.analysis_source_is_diff = false;
    }

    let analysis_source_key = analysis_source.as_ref().map(|source| {
        let base_key = AnalysisSourceKey::from_source(source);
        if app.canvas.analysis.analysis_source_is_diff {
            base_key.with_diff_request(app.canvas.analysis.last_diff_request_key)
        } else {
            base_key
        }
    });
    app.canvas.analysis.analysis_source_key = analysis_source_key;

    if computed_diff_stats.is_some() {
        app.canvas.analysis.diff_stats = computed_diff_stats;
    } else if !matches!(
        app.canvas
            .reference
            .ref_image
            .as_ref()
            .map(|reference| reference.mode),
        Some(RefImageMode::Diff)
    ) {
        app.canvas.analysis.diff_stats = None;
        app.canvas.analysis.last_diff_stats_request_key = None;
    }

    if let Some((capture, diff_result)) = computed_shortwire_diff_result {
        let artifacts = ui::pass_debug_window::record_shortwire_diff_result(
            &mut app.shell.pass_debug_windows,
            &capture,
            diff_result,
        );
        for (item, content_text) in artifacts {
            app.shell
                .debug_artifacts
                .upsert(item.clone(), Some(content_text.clone()));
            crate::ws::broadcast_debug_artifact_upsert(&app.core.ws_hub, item, Some(content_text));
        }
        app.shell.pending_shortwire_diff_capture = None;
    } else if completed_shortwire_diff_capture {
        app.shell.pending_shortwire_diff_capture = None;
    }

    if app.canvas.analysis.histogram_renderer.is_none() {
        app.canvas.analysis.histogram_renderer =
            Some(ui::histogram::HistogramRenderer::new(&render_state.device));
    }
    if app.canvas.analysis.parade_renderer.is_none() {
        app.canvas.analysis.parade_renderer =
            Some(ui::parade::ParadeRenderer::new(&render_state.device));
    }
    if app.canvas.analysis.vectorscope_renderer.is_none() {
        app.canvas.analysis.vectorscope_renderer = Some(ui::vectorscope::VectorscopeRenderer::new(
            &render_state.device,
        ));
    }

    let mut did_update_active_analysis = false;
    let mut did_update_clipping = false;
    let mut did_update_qualifier = false;

    if let Some(source) = analysis_source.as_ref()
        && let Some(source_key) = analysis_source_key
    {
        match app.canvas.analysis.analysis_tab {
            crate::app::AnalysisTab::Histogram => {
                let request_key = HistogramRequestKey::new(source_key);
                let should_update = app.canvas.invalidation.analysis_dirty()
                    || app.canvas.analysis.histogram_texture_id.is_none()
                    || app.canvas.analysis.last_histogram_request_key != Some(request_key);
                if should_update
                    && let Some(histogram_renderer) =
                        app.canvas.analysis.histogram_renderer.as_ref()
                {
                    histogram_renderer.update(
                        &render_state.device,
                        app.core.shader_space.queue.as_ref(),
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

                    if let Some(id) = app.canvas.analysis.histogram_texture_id {
                        renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                            &render_state.device,
                            histogram_renderer.output_view(),
                            sampler,
                            id,
                        );
                    } else {
                        app.canvas.analysis.histogram_texture_id =
                            Some(renderer_guard.register_native_texture_with_sampler_options(
                                &render_state.device,
                                histogram_renderer.output_view(),
                                sampler,
                            ));
                    }
                    app.canvas.analysis.last_histogram_request_key = Some(request_key);
                    did_update_active_analysis = true;
                }
            }
            crate::app::AnalysisTab::Parade => {
                let request_key = ParadeRequestKey::new(source_key);
                let should_update = app.canvas.invalidation.analysis_dirty()
                    || app.canvas.analysis.parade_texture_id.is_none()
                    || app.canvas.analysis.last_parade_request_key != Some(request_key);
                if should_update
                    && let Some(parade_renderer) = app.canvas.analysis.parade_renderer.as_ref()
                {
                    parade_renderer.update(
                        &render_state.device,
                        app.core.shader_space.queue.as_ref(),
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
                    if let Some(id) = app.canvas.analysis.parade_texture_id {
                        renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                            &render_state.device,
                            parade_renderer.parade_output_view(),
                            parade_sampler,
                            id,
                        );
                    } else {
                        app.canvas.analysis.parade_texture_id =
                            Some(renderer_guard.register_native_texture_with_sampler_options(
                                &render_state.device,
                                parade_renderer.parade_output_view(),
                                parade_sampler,
                            ));
                    }
                    app.canvas.analysis.last_parade_request_key = Some(request_key);
                    did_update_active_analysis = true;
                }
            }
            crate::app::AnalysisTab::Vectorscope => {
                let request_key = VectorscopeRequestKey::new(source_key);
                let should_update = app.canvas.invalidation.analysis_dirty()
                    || app.canvas.analysis.vectorscope_texture_id.is_none()
                    || app.canvas.analysis.last_vectorscope_request_key != Some(request_key);
                if should_update
                    && let Some(vectorscope_renderer) =
                        app.canvas.analysis.vectorscope_renderer.as_ref()
                {
                    vectorscope_renderer.update(
                        &render_state.device,
                        app.core.shader_space.queue.as_ref(),
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

                    if let Some(id) = app.canvas.analysis.vectorscope_texture_id {
                        renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                            &render_state.device,
                            vectorscope_renderer.output_view(),
                            sampler,
                            id,
                        );
                    } else {
                        app.canvas.analysis.vectorscope_texture_id =
                            Some(renderer_guard.register_native_texture_with_sampler_options(
                                &render_state.device,
                                vectorscope_renderer.output_view(),
                                sampler,
                            ));
                    }
                    app.canvas.analysis.last_vectorscope_request_key = Some(request_key);
                    did_update_active_analysis = true;
                }
            }
        }

        if app.canvas.analysis.clip_enabled {
            let request_key =
                ClippingRequestKey::new(source_key, app.canvas.analysis.clipping_settings, true);
            if app.canvas.analysis.clipping_renderer.is_none() {
                app.canvas.analysis.clipping_renderer = Some(
                    ui::clipping_map::ClippingMapRenderer::new(&render_state.device, source.size),
                );
            }

            let should_update_clipping = app.canvas.invalidation.analysis_dirty()
                || app.canvas.invalidation.clipping_dirty()
                || app.canvas.analysis.clipping_texture_id.is_none()
                || app.canvas.analysis.last_clipping_request_key != Some(request_key);
            if should_update_clipping
                && let Some(clipping_renderer) = app.canvas.analysis.clipping_renderer.as_mut()
            {
                clipping_renderer.update(
                    &render_state.device,
                    app.core.shader_space.queue.as_ref(),
                    source.view,
                    source.size,
                    app.canvas.analysis.clipping_settings.shadow_threshold,
                    app.canvas.analysis.clipping_settings.highlight_threshold,
                );

                let mut sampler =
                    texture_bridge::canvas_sampler_descriptor(wgpu::FilterMode::Nearest);
                sampler.label = Some("sys.scope.clipping.sampler");

                if let Some(id) = app.canvas.analysis.clipping_texture_id {
                    renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                        &render_state.device,
                        clipping_renderer.output_view(),
                        sampler,
                        id,
                    );
                } else {
                    app.canvas.analysis.clipping_texture_id =
                        Some(renderer_guard.register_native_texture_with_sampler_options(
                            &render_state.device,
                            clipping_renderer.output_view(),
                            sampler,
                        ));
                }
                app.canvas.analysis.last_clipping_request_key = Some(request_key);
                did_update_clipping = true;
            }
        }

        if app.canvas.analysis.qualifier_enabled {
            let request_key =
                QualifierRequestKey::new(source_key, app.canvas.analysis.qualifier_settings, true);
            if app.canvas.analysis.qualifier_renderer.is_none() {
                app.canvas.analysis.qualifier_renderer = Some(
                    ui::qualifier_map::QualifierMapRenderer::new(&render_state.device, source.size),
                );
            }

            let should_update_qualifier = app.canvas.invalidation.analysis_dirty()
                || app.canvas.invalidation.qualifier_dirty()
                || app.canvas.analysis.qualifier_texture_id.is_none()
                || app.canvas.analysis.last_qualifier_request_key != Some(request_key);
            if should_update_qualifier
                && let Some(qualifier_renderer) = app.canvas.analysis.qualifier_renderer.as_mut()
            {
                let s = app.canvas.analysis.qualifier_settings;
                qualifier_renderer.update(
                    &render_state.device,
                    app.core.shader_space.queue.as_ref(),
                    source.view,
                    source.size,
                    s.r_min,
                    s.r_max,
                    s.g_min,
                    s.g_max,
                    s.b_min,
                    s.b_max,
                );

                let mut sampler =
                    texture_bridge::canvas_sampler_descriptor(wgpu::FilterMode::Nearest);
                sampler.label = Some("sys.scope.qualifier.sampler");

                if let Some(id) = app.canvas.analysis.qualifier_texture_id {
                    renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                        &render_state.device,
                        qualifier_renderer.output_view(),
                        sampler,
                        id,
                    );
                } else {
                    app.canvas.analysis.qualifier_texture_id =
                        Some(renderer_guard.register_native_texture_with_sampler_options(
                            &render_state.device,
                            qualifier_renderer.output_view(),
                            sampler,
                        ));
                }
                app.canvas.analysis.last_qualifier_request_key = Some(request_key);
                did_update_qualifier = true;
            }
        }
    }

    if did_update_active_analysis {
        app.canvas.invalidation.clear_analysis();
    }
    if did_update_clipping {
        app.canvas.invalidation.clear_clipping();
    }
    if did_update_qualifier {
        app.canvas.invalidation.clear_qualifier();
    }

    if ingest.did_rebuild_shader_space {
        let _ = render_state
            .device
            .poll(rust_wgpu_fiber::eframe::wgpu::PollType::Poll);
    }
}

fn run_matrix_analysis(
    app: &mut App,
    render_state: &egui_wgpu::RenderState,
    renderer_guard: &mut egui_wgpu::Renderer,
) {
    let reference_mode = app.canvas.reference.ref_image.as_ref().map(|r| r.mode);
    let clip_enabled = app.canvas.analysis.clip_enabled;
    let clipping_settings = app.canvas.analysis.clipping_settings;
    let qualifier_enabled = app.canvas.analysis.qualifier_enabled;
    let qualifier_settings = app.canvas.analysis.qualifier_settings;
    let metric_mode = app.canvas.analysis.diff_metric_mode;
    let hdr_clamp = app.canvas.display.hdr_preview_clamp_enabled;

    let diff_dirty = app.canvas.invalidation.diff_dirty();
    let clipping_dirty = app.canvas.invalidation.clipping_dirty();
    let qualifier_dirty = app.canvas.invalidation.qualifier_dirty();
    let analysis_dirty = app.canvas.invalidation.analysis_dirty();

    // Single-image overlay state isn't drawn in matrix mode; clear so the
    // caller's existing flow (debug_sidebar reads diff_stats) gets only the
    // hovered-cell value below, and stale single-image renderers don't leak
    // into a future single-image session.
    app.canvas.analysis.diff_texture_id = None;
    app.canvas.analysis.clipping_texture_id = None;
    app.canvas.analysis.qualifier_texture_id = None;
    app.canvas.analysis.last_diff_request_key = None;
    app.canvas.analysis.last_diff_stats_request_key = None;
    app.canvas.analysis.last_clipping_request_key = None;
    app.canvas.analysis.last_qualifier_request_key = None;

    let cell_count = app.shell.matrix_state.cells.len();
    for idx in 0..cell_count {
        update_matrix_cell_analysis(
            app,
            render_state,
            renderer_guard,
            idx,
            reference_mode,
            clip_enabled,
            clipping_settings,
            qualifier_enabled,
            qualifier_settings,
            metric_mode,
            hdr_clamp,
            diff_dirty,
            clipping_dirty,
            qualifier_dirty,
            analysis_dirty,
        );
    }

    // Mirror the hovered (or sticky / fallback first) cell's diff stats into
    // the global slot so the existing sidebar pipeline displays them.
    let hovered_stats = app
        .shell
        .matrix_state
        .stats_cell()
        .and_then(|cell| cell.diff_stats);
    let in_diff_mode = matches!(reference_mode, Some(RefImageMode::Diff));
    app.canvas.analysis.diff_stats = if in_diff_mode { hovered_stats } else { None };

    if reference_mode.is_none() || !in_diff_mode {
        // No diff source — sweep cell stats so a later toggle starts clean.
        for cell in &mut app.shell.matrix_state.cells {
            cell.diff_stats = None;
        }
    }

    app.canvas.invalidation.clear_diff();
    app.canvas.invalidation.clear_analysis();
    app.canvas.invalidation.clear_clipping();
    app.canvas.invalidation.clear_qualifier();
}

#[allow(clippy::too_many_arguments)]
fn update_matrix_cell_analysis(
    app: &mut App,
    render_state: &egui_wgpu::RenderState,
    renderer_guard: &mut egui_wgpu::Renderer,
    cell_idx: usize,
    reference_mode: Option<RefImageMode>,
    clip_enabled: bool,
    clipping_settings: crate::app::ClippingSettings,
    qualifier_enabled: bool,
    qualifier_settings: crate::app::QualifierSettings,
    metric_mode: crate::app::DiffMetricMode,
    hdr_clamp: bool,
    diff_dirty: bool,
    clipping_dirty: bool,
    qualifier_dirty: bool,
    analysis_dirty: bool,
) {
    // Resolve the cell's own output texture into an AnalysisSourceDomain.
    let (cell_coord, cell_view, cell_size, cell_format) = {
        let cell = &app.shell.matrix_state.cells[cell_idx];
        let texture = cell
            .shader_space
            .textures
            .get(cell.output_texture_name.as_str());
        match texture.and_then(|t| {
            t.wgpu_texture_view
                .as_ref()
                .map(|view| (view, t.wgpu_texture_desc.size, t.wgpu_texture_desc.format))
        }) {
            Some((view, size, format)) => {
                (cell.coord, view.clone(), [size.width, size.height], format)
            }
            None => return,
        }
    };
    let cell_source_key =
        AnalysisSourceKey::from_hashable(&(cell_coord, cell_size, format!("{cell_format:?}")));

    // Diff
    let mut diff_done = false;
    if let Some(reference) = app.canvas.reference.ref_image.as_ref()
        && let Some(reference_mode) = reference_mode
    {
        let reference_offset = [
            reference.offset.x.round() as i32,
            reference.offset.y.round() as i32,
        ];
        let reference_size = reference.size;
        let reference_opacity = reference.opacity;
        let reference_view = &reference.wgpu_texture_view;
        let diff_output_format =
            ui::diff_renderer::select_diff_output_format(cell_format, reference.texture_format);
        let request_key = DiffRequestKey::new(
            cell_source_key,
            reference_size,
            reference_offset,
            reference_mode,
            reference_opacity.to_bits(),
            metric_mode,
            hdr_clamp,
        );
        let stats_key = DiffStatsRequestKey::new(request_key);
        let collect_stats = matches!(reference_mode, RefImageMode::Diff);

        let cell = &mut app.shell.matrix_state.cells[cell_idx];
        let needs_recreate = cell
            .diff_renderer
            .as_ref()
            .map(|r| r.output_size() != cell_size || r.output_format() != diff_output_format)
            .unwrap_or(true);
        if needs_recreate {
            cell.diff_renderer = Some(ui::diff_renderer::DiffRenderer::new(
                &render_state.device,
                cell_size,
                diff_output_format,
            ));
        }

        let should_update = diff_dirty
            || needs_recreate
            || cell.diff_texture_id.is_none()
            || cell.last_diff_request_key != Some(request_key)
            || (collect_stats && cell.last_diff_stats_request_key != Some(stats_key));

        if should_update && let Some(diff_renderer) = cell.diff_renderer.as_mut() {
            let stats = diff_renderer.update(
                &render_state.device,
                app.core.shader_space.queue.as_ref(),
                &cell_view,
                cell_size,
                reference_view,
                reference_size,
                reference_offset,
                reference_mode,
                reference_opacity,
                metric_mode,
                hdr_clamp,
                collect_stats,
            );
            cell.last_diff_request_key = Some(request_key);
            if collect_stats {
                cell.last_diff_stats_request_key = Some(stats_key);
                cell.diff_stats = stats;
            } else {
                cell.last_diff_stats_request_key = None;
            }

            let mut sampler =
                texture_bridge::diff_sampler_descriptor(app.canvas.display.texture_filter);
            sampler.label = Some("sys.diff.sampler.matrix");
            if let Some(id) = cell.diff_texture_id {
                renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                    &render_state.device,
                    diff_renderer.output_view(),
                    sampler,
                    id,
                );
            } else {
                cell.diff_texture_id =
                    Some(renderer_guard.register_native_texture_with_sampler_options(
                        &render_state.device,
                        diff_renderer.output_view(),
                        sampler,
                    ));
            }
            diff_done = true;
        }
    } else {
        // Reference removed: drop any per-cell diff overlay so it doesn't leak.
        let cell = &mut app.shell.matrix_state.cells[cell_idx];
        if let Some(id) = cell.diff_texture_id.take() {
            renderer_guard.free_texture(&id);
        }
        cell.last_diff_request_key = None;
        cell.last_diff_stats_request_key = None;
        cell.diff_stats = None;
    }
    let _ = diff_done;

    // Clip
    if clip_enabled {
        // Clip source: if we're in Diff mode, use the diff renderer's analysis
        // output (mirrors the single-image behavior in run()).
        let clip_in_diff = matches!(reference_mode, Some(RefImageMode::Diff));
        let cell = &mut app.shell.matrix_state.cells[cell_idx];
        let (clip_view, clip_size, clip_source_label) =
            if clip_in_diff && let Some(diff_renderer) = cell.diff_renderer.as_ref() {
                (
                    diff_renderer.analysis_output_view().clone(),
                    diff_renderer.analysis_output_size(),
                    "matrix.diff.analysis",
                )
            } else {
                (cell_view.clone(), cell_size, "matrix.cell.output")
            };
        let clip_source_key =
            AnalysisSourceKey::from_hashable(&(cell_coord, clip_size, clip_source_label));
        let request_key = ClippingRequestKey::new(clip_source_key, clipping_settings, true);

        let needs_new_renderer = cell
            .clipping_renderer
            .as_ref()
            .map(|_| false)
            .unwrap_or(true);
        if needs_new_renderer {
            cell.clipping_renderer = Some(ui::clipping_map::ClippingMapRenderer::new(
                &render_state.device,
                clip_size,
            ));
        }

        let should_update = analysis_dirty
            || clipping_dirty
            || cell.clipping_texture_id.is_none()
            || cell.last_clipping_request_key != Some(request_key);

        if should_update && let Some(clipping_renderer) = cell.clipping_renderer.as_mut() {
            clipping_renderer.update(
                &render_state.device,
                app.core.shader_space.queue.as_ref(),
                &clip_view,
                clip_size,
                clipping_settings.shadow_threshold,
                clipping_settings.highlight_threshold,
            );

            let mut sampler = texture_bridge::canvas_sampler_descriptor(wgpu::FilterMode::Nearest);
            sampler.label = Some("sys.scope.clipping.sampler.matrix");
            if let Some(id) = cell.clipping_texture_id {
                renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                    &render_state.device,
                    clipping_renderer.output_view(),
                    sampler,
                    id,
                );
            } else {
                cell.clipping_texture_id =
                    Some(renderer_guard.register_native_texture_with_sampler_options(
                        &render_state.device,
                        clipping_renderer.output_view(),
                        sampler,
                    ));
            }
            cell.last_clipping_request_key = Some(request_key);
        }
    } else {
        let cell = &mut app.shell.matrix_state.cells[cell_idx];
        if let Some(id) = cell.clipping_texture_id.take() {
            renderer_guard.free_texture(&id);
        }
        cell.last_clipping_request_key = None;
    }

    // Qualifier
    if qualifier_enabled {
        let q_in_diff = matches!(reference_mode, Some(RefImageMode::Diff));
        let cell = &mut app.shell.matrix_state.cells[cell_idx];
        let (q_view, q_size, q_source_label) =
            if q_in_diff && let Some(diff_renderer) = cell.diff_renderer.as_ref() {
                (
                    diff_renderer.analysis_output_view().clone(),
                    diff_renderer.analysis_output_size(),
                    "matrix.diff.analysis.qualifier",
                )
            } else {
                (cell_view.clone(), cell_size, "matrix.cell.output.qualifier")
            };
        let q_source_key = AnalysisSourceKey::from_hashable(&(cell_coord, q_size, q_source_label));
        let request_key = QualifierRequestKey::new(q_source_key, qualifier_settings, true);

        if cell.qualifier_renderer.is_none() {
            cell.qualifier_renderer = Some(ui::qualifier_map::QualifierMapRenderer::new(
                &render_state.device,
                q_size,
            ));
        }

        let should_update = analysis_dirty
            || qualifier_dirty
            || cell.qualifier_texture_id.is_none()
            || cell.last_qualifier_request_key != Some(request_key);

        if should_update && let Some(qualifier_renderer) = cell.qualifier_renderer.as_mut() {
            qualifier_renderer.update(
                &render_state.device,
                app.core.shader_space.queue.as_ref(),
                &q_view,
                q_size,
                qualifier_settings.r_min,
                qualifier_settings.r_max,
                qualifier_settings.g_min,
                qualifier_settings.g_max,
                qualifier_settings.b_min,
                qualifier_settings.b_max,
            );

            let mut sampler = texture_bridge::canvas_sampler_descriptor(wgpu::FilterMode::Nearest);
            sampler.label = Some("sys.scope.qualifier.sampler.matrix");
            if let Some(id) = cell.qualifier_texture_id {
                renderer_guard.update_egui_texture_from_wgpu_texture_with_sampler_options(
                    &render_state.device,
                    qualifier_renderer.output_view(),
                    sampler,
                    id,
                );
            } else {
                cell.qualifier_texture_id =
                    Some(renderer_guard.register_native_texture_with_sampler_options(
                        &render_state.device,
                        qualifier_renderer.output_view(),
                        sampler,
                    ));
            }
            cell.last_qualifier_request_key = Some(request_key);
        }
    } else {
        let cell = &mut app.shell.matrix_state.cells[cell_idx];
        if let Some(id) = cell.qualifier_texture_id.take() {
            renderer_guard.free_texture(&id);
        }
        cell.last_qualifier_request_key = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AnalysisSourceKey, ClippingRequestKey, DiffRequestKey, HistogramRequestKey,
        ParadeRequestKey, RefImageMode, VectorscopeRequestKey,
    };
    use crate::app::{ClippingSettings, DiffMetricMode};

    #[test]
    fn request_keys_change_with_source_domain() {
        let source_a = AnalysisSourceKey::from_hashable(&(
            "output",
            [128_u32, 128_u32],
            rust_wgpu_fiber::eframe::wgpu::TextureFormat::Rgba8Unorm,
        ));
        let source_b = AnalysisSourceKey::from_hashable(&(
            "output",
            [128_u32, 128_u32],
            rust_wgpu_fiber::eframe::wgpu::TextureFormat::Rgba16Float,
        ));
        assert_ne!(
            HistogramRequestKey::new(source_a),
            HistogramRequestKey::new(source_b)
        );
        assert_ne!(
            ParadeRequestKey::new(source_a),
            ParadeRequestKey::new(source_b)
        );
        assert_ne!(
            VectorscopeRequestKey::new(source_a),
            VectorscopeRequestKey::new(source_b)
        );
    }

    #[test]
    fn diff_request_key_changes_with_offset_and_metric() {
        let source_key = AnalysisSourceKey::from_hashable(&("output", [320_u32, 180_u32]));
        let key_1 = DiffRequestKey::new(
            source_key,
            [64, 64],
            [0, 0],
            RefImageMode::Diff,
            0.5f32.to_bits(),
            DiffMetricMode::AE,
            false,
        );
        let key_2 = DiffRequestKey::new(
            source_key,
            [64, 64],
            [1, 0],
            RefImageMode::Diff,
            0.5f32.to_bits(),
            DiffMetricMode::AE,
            false,
        );
        let key_3 = DiffRequestKey::new(
            source_key,
            [64, 64],
            [0, 0],
            RefImageMode::Diff,
            0.5f32.to_bits(),
            DiffMetricMode::SE,
            false,
        );
        let key_4 = DiffRequestKey::new(
            source_key,
            [64, 64],
            [0, 0],
            RefImageMode::Diff,
            0.5f32.to_bits(),
            DiffMetricMode::AE,
            true,
        );
        let key_5 = DiffRequestKey::new(
            source_key,
            [64, 64],
            [0, 0],
            RefImageMode::Overlay,
            0.5f32.to_bits(),
            DiffMetricMode::AE,
            false,
        );
        let key_6 = DiffRequestKey::new(
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
    fn clipping_request_key_changes_with_toggle_and_thresholds() {
        let source_key = AnalysisSourceKey::from_hashable(&("output", [1920_u32, 1080_u32]));
        let base_settings = ClippingSettings::default();
        let base = ClippingRequestKey::new(source_key, base_settings, true);
        let toggled = ClippingRequestKey::new(source_key, base_settings, false);
        let changed_threshold = ClippingRequestKey::new(
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
