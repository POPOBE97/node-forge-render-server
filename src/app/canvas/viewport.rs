use rust_wgpu_fiber::eframe::egui::{self, Rect};

use crate::{
    app::{
        display_metrics::{self, CurrentDisplayMetrics},
        layout_math::{clamp_zoom, lerp},
        types::{App, SIDEBAR_ANIM_SECS},
        window_mode::WindowModeFrame,
    },
    ui::animation_manager::{AnimationSpec, Easing},
};

pub const ANIM_KEY_PAN_ZOOM_FACTOR: &str = "ui.canvas.pan_zoom.factor";
const EXPLICIT_MIN_ZOOM: f32 = 0.001;

#[derive(Clone, Copy, Debug)]
pub struct ViewportFrame {
    pub effective_min_zoom: f32,
    pub image_rect: Rect,
    pub pan_zoom_enabled: bool,
}

pub fn is_pan_zoom_animating(app: &App) -> bool {
    app.shell.animations.is_active(ANIM_KEY_PAN_ZOOM_FACTOR)
}

pub fn prepare_viewport(
    app: &mut App,
    frame: WindowModeFrame,
    now: f64,
    canvas_rect: Rect,
    image_size: egui::Vec2,
    current_display_metrics: CurrentDisplayMetrics,
) -> ViewportFrame {
    let prev_center = app
        .canvas
        .viewport
        .canvas_center_prev
        .unwrap_or(canvas_rect.center());
    let new_center = canvas_rect.center();
    app.canvas.viewport.pan += prev_center - new_center;

    let fit_zoom = (canvas_rect.width() / image_size.x)
        .min(canvas_rect.height() / image_size.y)
        .max(0.01);

    if !app.canvas.viewport.zoom_initialized {
        app.canvas.viewport.zoom = fit_zoom;
        app.canvas.viewport.zoom_initialized = true;
        app.canvas.viewport.min_zoom = Some(fit_zoom);
        app.canvas.viewport.pan_zoom_target_zoom = fit_zoom;
    }

    if frame.prev_mode != frame.mode {
        let target_zoom = if app.canvas.viewport.pan_zoom_target_zoom > 0.0 {
            app.canvas.viewport.pan_zoom_target_zoom
        } else {
            app.canvas.viewport.zoom
        };
        let target_pan = app.canvas.viewport.pan_zoom_target_pan;
        app.canvas.viewport.pan_zoom_start_zoom = app.canvas.viewport.zoom;
        app.canvas.viewport.pan_zoom_start_pan = app.canvas.viewport.pan;
        app.canvas.viewport.pan_zoom_target_zoom = target_zoom;
        app.canvas.viewport.pan_zoom_target_pan = target_pan;
        app.shell.animations.start(
            ANIM_KEY_PAN_ZOOM_FACTOR,
            AnimationSpec {
                from: 0.0f32,
                to: 1.0f32,
                duration_secs: SIDEBAR_ANIM_SECS,
                easing: Easing::EaseOutCubic,
            },
            now,
        );
    }

    if let Some((factor, done)) = app
        .shell
        .animations
        .sample_f32(ANIM_KEY_PAN_ZOOM_FACTOR, now)
    {
        app.canvas.viewport.zoom = lerp(
            app.canvas.viewport.pan_zoom_start_zoom,
            app.canvas.viewport.pan_zoom_target_zoom,
            factor,
        );
        app.canvas.viewport.pan = app.canvas.viewport.pan_zoom_start_pan
            + (app.canvas.viewport.pan_zoom_target_pan - app.canvas.viewport.pan_zoom_start_pan)
                * factor;
        app.canvas.viewport.pan_start = None;
        if done {
            app.canvas.viewport.zoom = app.canvas.viewport.pan_zoom_target_zoom;
            app.canvas.viewport.pan = app.canvas.viewport.pan_zoom_target_pan;
        }
    }

    let pan_zoom_animating = is_pan_zoom_animating(app);
    let pan_zoom_enabled = !pan_zoom_animating;
    let mut effective_min_zoom = compute_effective_min_zoom(app, fit_zoom, pan_zoom_animating);

    if pan_zoom_enabled {
        app.canvas.viewport.pan_zoom_target_zoom = app.canvas.viewport.zoom;
        app.canvas.viewport.pan_zoom_target_pan = app.canvas.viewport.pan;
    }

    let zoom = clamp_zoom(app.canvas.viewport.zoom, effective_min_zoom);
    app.canvas.viewport.zoom = zoom;

    if app.canvas.viewport.pending_view_reset {
        app.canvas.viewport.zoom = fit_zoom;
        app.canvas.viewport.pan = egui::Vec2::ZERO;
        app.canvas.viewport.pan_start = None;
        app.canvas.viewport.explicit_min_zoom = Some(EXPLICIT_MIN_ZOOM);
        app.canvas.viewport.min_zoom = Some(fit_zoom);
        app.canvas.viewport.pan_zoom_target_zoom = fit_zoom;
        app.canvas.viewport.pan_zoom_target_pan = egui::Vec2::ZERO;
        app.canvas.viewport.pending_view_reset = false;
    }

    if let Some(request) = app.canvas.viewport.pending_center_physical_zoom.take() {
        let target_zoom = request.zoom.max(EXPLICIT_MIN_ZOOM);
        let pixels_per_point = request.pixels_per_point.max(0.000_1);
        app.canvas.viewport.explicit_min_zoom = Some(EXPLICIT_MIN_ZOOM);
        app.canvas.viewport.zoom = target_zoom;
        app.canvas.viewport.pan_start = None;
        app.canvas.viewport.pan_zoom_target_zoom = target_zoom;

        // Snap image origin to the physical pixel grid, eliminating sub-pixel
        // offsets that make linear and nearest sampling disagree.
        let draw_size = image_size * target_zoom;
        let base_min = canvas_rect.center() - draw_size * 0.5;
        let snapped_x = (base_min.x * pixels_per_point).round() / pixels_per_point;
        let snapped_y = (base_min.y * pixels_per_point).round() / pixels_per_point;
        let pan = egui::Vec2::new(snapped_x - base_min.x, snapped_y - base_min.y);
        app.canvas.viewport.pan = pan;
        app.canvas.viewport.pan_zoom_target_pan = pan;
    }

    effective_min_zoom = compute_effective_min_zoom(app, fit_zoom, pan_zoom_animating);
    let zoom = clamp_zoom(app.canvas.viewport.zoom, effective_min_zoom);
    app.canvas.viewport.zoom = zoom;
    sync_display_ppi_from_zoom(app, current_display_metrics);

    let draw_size = image_size * app.canvas.viewport.zoom;
    let base_min = canvas_rect.center() - draw_size * 0.5;
    let image_rect = Rect::from_min_size(base_min + app.canvas.viewport.pan, draw_size);

    ViewportFrame {
        effective_min_zoom,
        image_rect,
        pan_zoom_enabled,
    }
}

pub fn image_rect(app: &App, canvas_rect: Rect, image_size: egui::Vec2) -> Rect {
    let draw_size = image_size * app.canvas.viewport.zoom;
    let base_min = canvas_rect.center() - draw_size * 0.5;
    Rect::from_min_size(base_min + app.canvas.viewport.pan, draw_size)
}

fn compute_effective_min_zoom(app: &App, fit_zoom: f32, pan_zoom_animating: bool) -> f32 {
    if pan_zoom_animating {
        0.01
    } else {
        app.canvas
            .viewport
            .explicit_min_zoom
            .unwrap_or_else(|| app.canvas.viewport.min_zoom.unwrap_or(fit_zoom))
    }
}

fn sync_display_ppi_from_zoom(app: &mut App, current_display_metrics: CurrentDisplayMetrics) {
    let Some(current_display_ppi) = current_display_metrics.display_ppi else {
        return;
    };
    let Some(ppi) = display_metrics::display_ppi_from_zoom(
        current_display_ppi,
        app.canvas.viewport.zoom,
        current_display_metrics.pixels_per_point,
    ) else {
        return;
    };

    app.canvas.viewport.display_ppi = Some(display_metrics::clamp_display_ppi(ppi));
}
