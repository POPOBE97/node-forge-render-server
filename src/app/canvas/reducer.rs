use rust_wgpu_fiber::eframe::{egui, egui_wgpu};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu,
    shader_space::{PASS_CAPTURE_OUTPUT_TEXTURE_NAME, PassCaptureMode},
};

use crate::app::{
    canvas::{
        actions::{CanvasAction, CanvasFrameResult},
        ops, pixel_overlay, reference,
        state::{CanvasViewportState, DrawCallCaptureState, PhysicalZoomRequest},
    },
    display_metrics,
    layout_math::clamp_zoom,
    matrix_render, texture_bridge,
    types::{App, QualifierChannel, RefImageAlphaMode, RefImageMode, SampledPixel},
};

fn set_viewport_display_ppi(viewport: &mut CanvasViewportState, display_ppi: Option<f32>) {
    viewport.display_ppi = display_ppi.map(display_metrics::clamp_display_ppi);
}

fn sync_zoom_to_display_ppi(
    viewport: &mut CanvasViewportState,
    current_display_ppi: Option<f32>,
    pixels_per_point: f32,
    zoom: f32,
    effective_min_zoom: f32,
) -> f32 {
    let Some(current_display_ppi) = current_display_ppi else {
        return zoom;
    };
    let Some(ppi) =
        display_metrics::display_ppi_from_zoom(current_display_ppi, zoom, pixels_per_point)
    else {
        return zoom;
    };

    let ppi = display_metrics::clamp_display_ppi(ppi);
    set_viewport_display_ppi(viewport, Some(ppi));

    display_metrics::simulation_zoom(current_display_ppi, ppi, pixels_per_point)
        .map(|synced_zoom| clamp_zoom(synced_zoom, effective_min_zoom))
        .unwrap_or(zoom)
}

pub fn apply_action(
    app: &mut App,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    action: CanvasAction,
) -> anyhow::Result<CanvasFrameResult> {
    match action {
        CanvasAction::SetPreviewTexture(name) => {
            app.canvas.display.pass_capture = None;
            app.canvas.display.preview_texture_name = Some(name);
            app.canvas.viewport.pending_view_reset = true;
            pixel_overlay::clear_cache(app);
            app.canvas.invalidation.preview_source_changed();
        }
        CanvasAction::SetPassCapture(pass_name) => {
            let mode = app
                .canvas
                .display
                .pass_capture
                .as_ref()
                .filter(|capture| capture.pass_name == pass_name)
                .map(|capture| capture.mode)
                .unwrap_or(PassCaptureMode::Solo);
            app.canvas.design.active = None;
            app.canvas.display.pass_capture = Some(DrawCallCaptureState { pass_name, mode });
            app.canvas.display.preview_texture_name =
                Some(ResourceName::from(PASS_CAPTURE_OUTPUT_TEXTURE_NAME));
            app.canvas.viewport.pending_view_reset = true;
            app.runtime.scene_redraw_pending = true;
            pixel_overlay::clear_cache(app);
            app.canvas.invalidation.preview_source_changed();
        }
        CanvasAction::SetPassCaptureMode(mode) => {
            if let Some(capture) = app.canvas.display.pass_capture.as_mut()
                && capture.mode != mode
            {
                capture.mode = mode;
                app.runtime.scene_redraw_pending = true;
                pixel_overlay::clear_cache(app);
                app.canvas.invalidation.preview_source_changed();
            }
        }
        CanvasAction::ClearPreviewTexture => {
            app.canvas.display.pass_capture = None;
            app.canvas.display.preview_texture_name = None;
            app.shell.file_tree_state.selected_id = None;
            if let Some(id) = app.canvas.display.preview_color_attachment.take() {
                app.canvas.display.deferred_texture_frees.push(id);
            }
            pixel_overlay::clear_cache(app);
            app.canvas.invalidation.preview_source_changed();
        }
        CanvasAction::EnterPassDesign(target) => {
            if app.canvas.display.pass_capture.take().is_some()
                && app
                    .canvas
                    .display
                    .preview_texture_name
                    .as_ref()
                    .is_some_and(|name| name.as_str() == PASS_CAPTURE_OUTPUT_TEXTURE_NAME)
            {
                app.canvas.display.preview_texture_name = None;
            }
            let previous_preview_texture = app
                .canvas
                .design
                .active
                .as_ref()
                .map(|session| session.previous_preview_texture.clone())
                .unwrap_or_else(|| app.canvas.display.preview_texture_name.clone());
            let target_texture = target.target_texture.clone();
            app.canvas.design.active = crate::app::canvas::design::enter_session(
                app.canvas.design.active.take(),
                target,
                previous_preview_texture,
            );
            if let Some(target_texture) = target_texture {
                app.canvas.display.preview_texture_name =
                    Some(ResourceName::from(target_texture.as_str()));
                pixel_overlay::clear_cache(app);
                app.canvas.invalidation.preview_source_changed();
            }
        }
        CanvasAction::ExitPassDesign => {
            if let Some(session) = app.canvas.design.active.take()
                && session.owns_preview_texture
            {
                app.canvas.display.preview_texture_name = session.previous_preview_texture;
                if app.canvas.display.preview_texture_name.is_none()
                    && let Some(id) = app.canvas.display.preview_color_attachment.take()
                {
                    app.canvas.display.deferred_texture_frees.push(id);
                }
                pixel_overlay::clear_cache(app);
                app.canvas.invalidation.preview_source_changed();
            }
        }
        CanvasAction::ToggleHdrClamp => {
            app.canvas.display.hdr_preview_clamp_enabled =
                !app.canvas.display.hdr_preview_clamp_enabled;
            app.canvas.invalidation.mark_pixel_overlay_dirty();
            matrix_render::sync_matrix_hdr_clamp(
                &mut app.shell.matrix_state,
                render_state,
                renderer,
                app.canvas.display.hdr_preview_clamp_enabled,
                app.canvas.display.texture_filter,
            );
        }
        CanvasAction::ToggleWireframe => {
            let requested_enabled = !app.canvas.display.wireframe_enabled;
            let applied = app
                .core
                .shader_space
                .set_wireframe_enabled(requested_enabled);
            app.canvas.display.wireframe_enabled = requested_enabled && applied;
            app.runtime.scene_redraw_pending = true;

            if requested_enabled && !applied {
                eprintln!(
                    "[wireframe] wgpu device does not support POLYGON_MODE_LINE; keeping fill mode"
                );
            }
        }
        CanvasAction::TogglePause => {
            app.runtime.time_updates_enabled = !app.runtime.time_updates_enabled;
            // Pause / resume the timeline presentation clock so that
            // wall-clock time spent paused doesn't create a gap in
            // presentation_time_secs (which drives the rolling-window trim).
            if let Some(ref mut buf) = app.runtime.timeline_buffer {
                if app.runtime.time_updates_enabled {
                    buf.resume();
                } else {
                    buf.pause();
                }
            }
            let has_reference_diff = matches!(
                app.canvas.reference.ref_image.as_ref().map(|r| r.mode),
                Some(RefImageMode::Diff)
            );
            app.canvas
                .invalidation
                .time_pause_toggled(app.runtime.scene_uses_time, has_reference_diff);
        }
        CanvasAction::ResetView {
            current_display_ppi,
        } => {
            app.canvas.viewport.pending_view_reset = true;
            set_viewport_display_ppi(&mut app.canvas.viewport, current_display_ppi);
        }
        CanvasAction::CenterAt1x {
            pixels_per_point,
            current_display_ppi,
        } => {
            set_viewport_display_ppi(&mut app.canvas.viewport, current_display_ppi);
            if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
                app.canvas.viewport.pending_center_physical_zoom = Some(PhysicalZoomRequest {
                    zoom: 1.0 / pixels_per_point,
                    pixels_per_point,
                });
            }
        }
        CanvasAction::SetDisplayPpi {
            ppi,
            current_display_ppi,
            pixels_per_point,
        } => {
            let ppi = display_metrics::clamp_display_ppi(ppi);
            set_viewport_display_ppi(&mut app.canvas.viewport, Some(ppi));

            if let Some(current_display_ppi) = current_display_ppi {
                if let Some(zoom) =
                    display_metrics::simulation_zoom(current_display_ppi, ppi, pixels_per_point)
                {
                    app.canvas.viewport.pending_center_physical_zoom = Some(PhysicalZoomRequest {
                        zoom,
                        pixels_per_point,
                    });
                }
            }
        }
        CanvasAction::ToggleSampling => {
            app.canvas.display.texture_filter = match app.canvas.display.texture_filter {
                wgpu::FilterMode::Nearest => wgpu::FilterMode::Linear,
                wgpu::FilterMode::Linear => wgpu::FilterMode::Nearest,
            };
            if let Some(preview_name) = app.canvas.display.preview_texture_name.clone() {
                texture_bridge::sync_preview_texture(
                    app,
                    render_state,
                    renderer,
                    &preview_name,
                    app.canvas.display.texture_filter,
                );
            }
            let texture_name = app.core.output_texture_name.clone();
            texture_bridge::sync_output_texture(
                app,
                render_state,
                renderer,
                &texture_name,
                app.canvas.display.texture_filter,
            );
            if let Some(diff_renderer) = app.canvas.analysis.diff_renderer.as_ref()
                && let Some(diff_texture_id) = app.canvas.analysis.diff_texture_id
            {
                let mut sampler =
                    texture_bridge::diff_sampler_descriptor(app.canvas.display.texture_filter);
                sampler.label = Some("sys.diff.sampler");
                renderer.update_egui_texture_from_wgpu_texture_with_sampler_options(
                    &render_state.device,
                    diff_renderer.output_view(),
                    sampler,
                    diff_texture_id,
                );
            }
            matrix_render::sync_matrix_filter(
                &mut app.shell.matrix_state,
                render_state,
                renderer,
                app.canvas.display.texture_filter,
            );
        }
        CanvasAction::ToggleReferenceAlpha => {
            app.canvas.reference.alpha_mode = match app.canvas.reference.alpha_mode {
                RefImageAlphaMode::Premultiplied => RefImageAlphaMode::Straight,
                RefImageAlphaMode::Straight => RefImageAlphaMode::Premultiplied,
            };

            let mut changed = false;
            if let Some(reference_image) = app.canvas.reference.ref_image.as_mut() {
                match reference::set_reference_alpha_mode(
                    app.core.shader_space.queue.as_ref(),
                    reference_image,
                    app.canvas.reference.alpha_mode,
                ) {
                    Ok(did_change) => changed = did_change,
                    Err(e) => eprintln!("[reference-image] failed to switch alpha mode: {e:#}"),
                }
            }
            if changed {
                app.canvas.invalidation.reference_mode_changed();
            }
        }
        CanvasAction::ToggleClipping => {
            app.canvas.analysis.clip_enabled = !app.canvas.analysis.clip_enabled;
            app.canvas.invalidation.clipping_controls_changed();
        }
        CanvasAction::SetClipEnabled(enabled) => {
            if app.canvas.analysis.clip_enabled != enabled {
                app.canvas.analysis.clip_enabled = enabled;
                app.canvas.invalidation.clipping_controls_changed();
            }
        }
        CanvasAction::ResetReferenceOffset => {
            if let Some(reference_image) = app.canvas.reference.ref_image.as_mut()
                && reference_image.offset != egui::Vec2::ZERO
            {
                reference_image.offset = egui::Vec2::ZERO;
                app.canvas
                    .invalidation
                    .reference_pixels_changed(reference_image.mode);
            }
        }
        CanvasAction::SetReferenceOpacity(opacity) => {
            if let Some(reference_image) = app.canvas.reference.ref_image.as_mut() {
                let opacity = opacity.clamp(0.0, 1.0);
                if (reference_image.opacity - opacity).abs() > f32::EPSILON {
                    reference_image.opacity = opacity;
                    app.canvas
                        .invalidation
                        .reference_pixels_changed(reference_image.mode);
                }
            }
        }
        CanvasAction::ToggleReferenceMode => {
            if let Some(reference_image) = app.canvas.reference.ref_image.as_mut() {
                reference_image.mode = match reference_image.mode {
                    RefImageMode::Overlay => RefImageMode::Diff,
                    RefImageMode::Diff => RefImageMode::Overlay,
                };
                app.canvas.invalidation.reference_mode_changed();
            }
        }
        CanvasAction::SetDiffMetricMode(mode) => {
            if app.canvas.analysis.diff_metric_mode != mode {
                app.canvas.analysis.diff_metric_mode = mode;
                app.canvas.invalidation.mark_diff_dirty();
                app.canvas.invalidation.mark_pixel_overlay_dirty();
            }
        }
        CanvasAction::SetAnalysisTab(tab) => {
            if app.canvas.analysis.analysis_tab != tab {
                app.canvas.analysis.analysis_tab = tab;
                app.canvas.invalidation.analysis_tab_changed();
            }
        }
        CanvasAction::SetClippingShadowThreshold(threshold) => {
            let threshold = threshold.clamp(0.0, 1.0);
            if (app.canvas.analysis.clipping_settings.shadow_threshold - threshold).abs()
                > f32::EPSILON
            {
                app.canvas.analysis.clipping_settings.shadow_threshold = threshold;
                app.canvas.invalidation.clipping_controls_changed();
            }
        }
        CanvasAction::SetClippingHighlightThreshold(threshold) => {
            let threshold = threshold.clamp(0.0, 1.0);
            if (app.canvas.analysis.clipping_settings.highlight_threshold - threshold).abs()
                > f32::EPSILON
            {
                app.canvas.analysis.clipping_settings.highlight_threshold = threshold;
                app.canvas.invalidation.clipping_controls_changed();
            }
        }
        CanvasAction::ToggleQualifier => {
            app.canvas.analysis.qualifier_enabled = !app.canvas.analysis.qualifier_enabled;
            app.canvas.invalidation.qualifier_controls_changed();
        }
        CanvasAction::SetQualifierEnabled(enabled) => {
            if app.canvas.analysis.qualifier_enabled != enabled {
                app.canvas.analysis.qualifier_enabled = enabled;
                app.canvas.invalidation.qualifier_controls_changed();
            }
        }
        CanvasAction::SetQualifierRange { channel, min, max } => {
            let lo = min.clamp(0.0, 1.0);
            let hi = max.clamp(0.0, 1.0);
            let (lo, hi) = if lo > hi { (hi, lo) } else { (lo, hi) };
            let s = &mut app.canvas.analysis.qualifier_settings;
            let (cur_min, cur_max) = match channel {
                QualifierChannel::R => (&mut s.r_min, &mut s.r_max),
                QualifierChannel::G => (&mut s.g_min, &mut s.g_max),
                QualifierChannel::B => (&mut s.b_min, &mut s.b_max),
            };
            if (*cur_min - lo).abs() > f32::EPSILON || (*cur_max - hi).abs() > f32::EPSILON {
                *cur_min = lo;
                *cur_max = hi;
                app.canvas.invalidation.qualifier_controls_changed();
            }
        }
        CanvasAction::BeginPanDrag(pointer_pos) => {
            app.canvas.viewport.pan_start = Some(pointer_pos);
        }
        CanvasAction::UpdatePanDrag(pointer_pos) => {
            if let Some(start) = app.canvas.viewport.pan_start {
                app.canvas.viewport.pan += pointer_pos - start;
                app.canvas.viewport.pan_start = Some(pointer_pos);
            }
        }
        CanvasAction::EndPanDrag => {
            app.canvas.viewport.pan_start = None;
        }
        CanvasAction::BeginReferenceDrag(pointer_pos) => {
            if let Some(reference_image) = app.canvas.reference.ref_image.as_mut() {
                reference_image.drag_start = Some(pointer_pos);
                reference_image.drag_start_offset = reference_image.offset;
            }
        }
        CanvasAction::UpdateReferenceDrag(pointer_pos) => {
            if let Some(reference_image) = app.canvas.reference.ref_image.as_mut()
                && let Some(start) = reference_image.drag_start
            {
                let delta = (pointer_pos - start) / app.canvas.viewport.zoom.max(0.000_1);
                let next_offset = egui::vec2(
                    (reference_image.drag_start_offset.x + delta.x).round(),
                    (reference_image.drag_start_offset.y + delta.y).round(),
                );
                if reference_image.offset != next_offset {
                    reference_image.offset = next_offset;
                    app.canvas
                        .invalidation
                        .reference_pixels_changed(reference_image.mode);
                }
            }
        }
        CanvasAction::EndReferenceDrag => {
            if let Some(reference_image) = app.canvas.reference.ref_image.as_mut() {
                reference_image.drag_start = None;
            }
        }
        CanvasAction::ApplyScrollPan(delta) => {
            app.canvas.viewport.pan += delta;
        }
        CanvasAction::ApplyZoomAroundPointer {
            pointer_pos,
            zoom_delta,
            canvas_rect,
            image_size,
            effective_min_zoom,
            current_display_ppi,
            pixels_per_point,
        } => {
            let prev_zoom = app.canvas.viewport.zoom;
            let mut next_zoom = clamp_zoom(prev_zoom * zoom_delta, effective_min_zoom);
            next_zoom = sync_zoom_to_display_ppi(
                &mut app.canvas.viewport,
                current_display_ppi,
                pixels_per_point,
                next_zoom,
                effective_min_zoom,
            );
            if next_zoom != prev_zoom {
                let prev_size = image_size * prev_zoom;
                let prev_min = canvas_rect.center() - prev_size * 0.5 + app.canvas.viewport.pan;
                let local = (pointer_pos - prev_min) / prev_size;
                app.canvas.viewport.zoom = next_zoom;
                let next_size = image_size * next_zoom;
                let next_min = pointer_pos - local * next_size;
                let desired_pan = next_min - (canvas_rect.center() - next_size * 0.5);
                app.canvas.viewport.pan = desired_pan;
            }
        }
        CanvasAction::SamplePixel { x, y, rgba } => {
            app.canvas.viewport.last_sampled = Some(SampledPixel { x, y, rgba });
        }
        CanvasAction::PollClipboardOp { now } => {
            ops::poll(&mut app.canvas.async_ops, now);
        }
    }

    Ok(CanvasFrameResult::default())
}

#[cfg(test)]
mod tests {
    use super::{super::actions::CanvasAction, set_viewport_display_ppi, sync_zoom_to_display_ppi};
    use crate::app::canvas::state::CanvasViewportState;
    use crate::app::types::{AnalysisTab, ClippingSettings, DiffMetricMode, UiWindowMode};

    #[test]
    fn invalidation_sets_analysis_and_clipping_on_analysis_tab_change() {
        let mut invalidation = crate::app::canvas::state::CanvasInvalidation::default();
        invalidation.clear_analysis();
        invalidation.clear_clipping();
        invalidation.analysis_tab_changed();
        assert!(invalidation.analysis_dirty());
        assert!(invalidation.clipping_dirty());
    }

    #[test]
    fn canvas_action_variants_cover_sidebar_actions() {
        let _ = CanvasAction::SetAnalysisTab(AnalysisTab::Histogram);
        let _ = CanvasAction::SetClippingShadowThreshold(0.1);
        let _ = CanvasAction::SetClippingHighlightThreshold(0.9);
        let _ = CanvasAction::SetDiffMetricMode(DiffMetricMode::AE);
        let _ = CanvasAction::SetClipEnabled(true);
        let _ = UiWindowMode::Sidebar;
        let _ = ClippingSettings::default();
    }

    #[test]
    fn reset_view_syncs_ppi_back_to_current_display() {
        let mut viewport = CanvasViewportState {
            display_ppi: Some(440.0),
            ..CanvasViewportState::default()
        };

        set_viewport_display_ppi(&mut viewport, Some(264.0));

        assert_eq!(viewport.display_ppi, Some(264.0));
        assert_eq!(viewport.effective_display_ppi(), 264.0);
    }

    #[test]
    fn wheel_zoom_syncs_ppi_from_zoom() {
        let mut viewport = CanvasViewportState {
            display_ppi: Some(220.0),
            ..CanvasViewportState::default()
        };

        let zoom = sync_zoom_to_display_ppi(&mut viewport, Some(220.0), 2.0, 0.25, 0.01);

        assert!((zoom - 0.25).abs() < 1e-6);
        assert_eq!(viewport.display_ppi, Some(440.0));
    }

    #[test]
    fn wheel_zoom_clamps_ppi_and_returns_matching_zoom() {
        let mut viewport = CanvasViewportState {
            display_ppi: Some(220.0),
            ..CanvasViewportState::default()
        };

        let zoom = sync_zoom_to_display_ppi(&mut viewport, Some(220.0), 2.0, 0.01, 0.01);

        assert!((zoom - 0.11).abs() < 1e-6);
        assert_eq!(viewport.display_ppi, Some(1000.0));
    }
}
