use rust_wgpu_fiber::eframe::wgpu;
use rust_wgpu_fiber::eframe::{egui, egui_wgpu};

use crate::app::{
    canvas::{
        actions::{CanvasAction, CanvasFrameResult},
        ops, pixel_overlay, reference,
    },
    layout_math::clamp_zoom,
    texture_bridge,
    types::{App, RefImageAlphaMode, RefImageMode, SampledPixel},
};

pub fn apply_action(
    app: &mut App,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    action: CanvasAction,
) -> anyhow::Result<CanvasFrameResult> {
    match action {
        CanvasAction::SetPreviewTexture(name) => {
            app.canvas.display.preview_texture_name = Some(name);
            app.canvas.viewport.pending_view_reset = true;
            pixel_overlay::clear_cache(app);
            app.canvas.invalidation.preview_source_changed();
        }
        CanvasAction::ClearPreviewTexture => {
            app.canvas.display.preview_texture_name = None;
            app.shell.file_tree_state.selected_id = None;
            if let Some(id) = app.canvas.display.preview_color_attachment.take() {
                app.canvas.display.deferred_texture_frees.push(id);
            }
            pixel_overlay::clear_cache(app);
            app.canvas.invalidation.preview_source_changed();
        }
        CanvasAction::ToggleHdrClamp => {
            app.canvas.display.hdr_preview_clamp_enabled =
                !app.canvas.display.hdr_preview_clamp_enabled;
            app.canvas.invalidation.mark_pixel_overlay_dirty();
        }
        CanvasAction::TogglePause => {
            app.runtime.time_updates_enabled = !app.runtime.time_updates_enabled;
            let has_reference_diff = matches!(
                app.canvas.reference.ref_image.as_ref().map(|r| r.mode),
                Some(RefImageMode::Diff)
            );
            app.canvas
                .invalidation
                .time_pause_toggled(app.runtime.scene_uses_time, has_reference_diff);
        }
        CanvasAction::ResetView => {
            app.canvas.viewport.pending_view_reset = true;
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
        } => {
            let prev_zoom = app.canvas.viewport.zoom;
            let next_zoom = clamp_zoom(prev_zoom * zoom_delta, effective_min_zoom);
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
    use super::super::actions::CanvasAction;
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
}
