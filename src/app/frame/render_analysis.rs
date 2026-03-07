use rust_wgpu_fiber::eframe::{egui_wgpu, wgpu};

use crate::{
    app::{
        texture_bridge,
        types::{AnalysisSourceDomain, App, DiffStats, RefImageMode},
    },
    renderer, ui,
};

use super::{
    advance::AdvancePhase,
    ingest::IngestPhase,
    request_keys::{
        AnalysisSourceKey, ClippingRequestKey, DiffRequestKey, DiffStatsRequestKey,
        HistogramRequestKey, ParadeRequestKey, VectorscopeRequestKey,
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
            app.canvas.invalidation.mark_pixel_overlay_dirty();
        }

        app.core.shader_space.render();
        app.runtime.scene_redraw_pending = false;
        app.runtime
            .render_texture_fps_tracker
            .record_scene_redraw(ingest.frame_time);
    }

    texture_bridge::ensure_output_texture_registered(app, render_state, renderer_guard);

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
    let mut did_update_diff_output = false;

    if let Some(reference) = app.canvas.reference.ref_image.as_ref()
        && let Some(source) = display_source.as_ref()
        && let Some(source_key) = compare_source_key
    {
        let reference_mode = reference.mode;
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
                reference_mode,
                reference.opacity.to_bits(),
                app.canvas.analysis.diff_metric_mode,
                app.canvas.display.hdr_preview_clamp_enabled,
            );
            let stats_key = DiffStatsRequestKey::new(request_key);
            let collect_stats = matches!(reference_mode, RefImageMode::Diff);
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
                    reference_mode,
                    reference.opacity,
                    app.canvas.analysis.diff_metric_mode,
                    app.canvas.display.hdr_preview_clamp_enabled,
                    collect_stats,
                );
                did_update_diff_output = true;
                app.canvas.analysis.last_diff_request_key = Some(request_key);
                if collect_stats {
                    app.canvas.analysis.last_diff_stats_request_key = Some(stats_key);
                    computed_diff_stats = diff_stats;
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
    }

    if did_update_active_analysis {
        app.canvas.invalidation.clear_analysis();
    }
    if did_update_clipping {
        app.canvas.invalidation.clear_clipping();
    }

    if ingest.did_rebuild_shader_space {
        let _ = render_state
            .device
            .poll(rust_wgpu_fiber::eframe::wgpu::PollType::Poll);
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
