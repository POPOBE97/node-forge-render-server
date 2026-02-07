use rust_wgpu_fiber::eframe::{
    egui::{self, Color32, Rect, pos2},
    egui_wgpu, wgpu,
};

use crate::ui::animation_manager::{AnimationSpec, Easing};

use super::{
    layout_math::{
        clamp_uv_rect_into_unit, clamp_zoom, cover_uv_rect, lerp, lerp_pos2, lerp_rect, lerp_vec2,
    },
    texture_bridge,
    types::{App, CANVAS_RADIUS, SIDEBAR_ANIM_SECS, UiWindowMode},
    window_mode::WindowModeFrame,
};

const ANIM_KEY_PAN_ZOOM_FACTOR: &str = "ui.canvas.pan_zoom.factor";

pub fn show_canvas_panel(
    app: &mut App,
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    frame: WindowModeFrame,
    now: f64,
) -> bool {
    let mut requested_toggle_canvas_only = false;

    if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::F)) {
        requested_toggle_canvas_only = true;
    }

    let avail_rect = ui.available_rect_before_wrap();
    let image_size = egui::vec2(app.resolution[0] as f32, app.resolution[1] as f32);

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
            avail_w - CANVAS_RADIUS * 2.0,
            (avail_w - CANVAS_RADIUS * 2.0) / aspect,
        )
    };
    let framed_canvas_rect = Rect::from_center_size(avail_rect.center(), egui::vec2(w, h));
    let animated_canvas_rect = lerp_rect(full_rect, framed_canvas_rect, frame.sidebar_factor);
    let paint_frame = frame.sidebar_factor > 0.001;

    let prev_center = app
        .canvas_center_prev
        .unwrap_or(animated_canvas_rect.center());
    let new_center = animated_canvas_rect.center();
    app.pan += prev_center - new_center;

    let fit_zoom = (animated_canvas_rect.width() / image_size.x)
        .min(animated_canvas_rect.height() / image_size.y)
        .max(0.01);

    let framed_fit_zoom = (animated_canvas_rect.width() / image_size.x)
        .min(animated_canvas_rect.height() / image_size.y)
        .max(0.01);

    if !app.zoom_initialized {
        app.zoom = fit_zoom;
        app.zoom_initialized = true;
        app.min_zoom = Some(fit_zoom);
        app.pan_zoom_target_zoom = fit_zoom;
    }
    let min_zoom = app.min_zoom.unwrap_or(fit_zoom);

    if frame.prev_mode != frame.mode {
        let (start_zoom, start_pan, target_zoom, target_pan) = match frame.mode {
            UiWindowMode::Sidebar => (app.zoom, app.pan, framed_fit_zoom, egui::Vec2::ZERO),
            UiWindowMode::CanvasOnly => (
                fit_zoom,
                egui::Vec2::ZERO,
                if app.pan_zoom_target_zoom > 0.0 {
                    app.pan_zoom_target_zoom
                } else {
                    app.zoom
                },
                app.pan_zoom_target_pan,
            ),
        };
        app.pan_zoom_start_zoom = start_zoom;
        app.pan_zoom_start_pan = start_pan;
        app.pan_zoom_target_zoom = target_zoom;
        app.pan_zoom_target_pan = target_pan;
        app.animations.start(
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

    if let Some((factor, done)) = app.animations.sample_f32(ANIM_KEY_PAN_ZOOM_FACTOR, now) {
        app.zoom = lerp(app.pan_zoom_start_zoom, app.pan_zoom_target_zoom, factor);
        app.pan =
            app.pan_zoom_start_pan + (app.pan_zoom_target_pan - app.pan_zoom_start_pan) * factor;
        app.pan_start = None;
        if done {
            app.zoom = app.pan_zoom_target_zoom;
            app.pan = app.pan_zoom_target_pan;
        }
    }

    let pan_zoom_animating = app.animations.is_active(ANIM_KEY_PAN_ZOOM_FACTOR);
    let pan_zoom_enabled = matches!(frame.mode, UiWindowMode::CanvasOnly) && !pan_zoom_animating;
    let effective_min_zoom = if pan_zoom_animating {
        0.01
    } else if frame.mode == UiWindowMode::CanvasOnly {
        min_zoom
    } else {
        fit_zoom
    };

    if pan_zoom_enabled {
        app.pan_zoom_target_zoom = app.zoom;
        app.pan_zoom_target_pan = app.pan;
    }

    let zoom = clamp_zoom(app.zoom, effective_min_zoom);
    app.zoom = zoom;
    let draw_size = image_size * zoom;
    let base_min = animated_canvas_rect.center() - draw_size * 0.5;
    let mut image_rect = Rect::from_min_size(base_min + app.pan, draw_size);

    let response = ui.allocate_rect(avail_rect, egui::Sense::click_and_drag());

    if pan_zoom_enabled && ctx.input(|i| i.key_pressed(egui::Key::R)) {
        app.zoom = fit_zoom;
        app.pan = egui::Vec2::ZERO;
        app.pan_start = None;
        let draw_size = image_size * app.zoom;
        let base_min = animated_canvas_rect.center() - draw_size * 0.5;
        image_rect = Rect::from_min_size(base_min, draw_size);
    }

    if ctx.input(|i| i.key_pressed(egui::Key::P)) {
        app.texture_filter = match app.texture_filter {
            wgpu::FilterMode::Nearest => wgpu::FilterMode::Linear,
            wgpu::FilterMode::Linear => wgpu::FilterMode::Nearest,
        };
        let texture_name = app.output_texture_name.clone();
        texture_bridge::sync_output_texture(
            app,
            render_state,
            renderer,
            &texture_name,
            app.texture_filter,
        );
    }

    if pan_zoom_enabled {
        if response.drag_started_by(egui::PointerButton::Middle) {
            if let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) {
                app.pan_start = Some(pointer_pos);
            }
        }
        if response.dragged_by(egui::PointerButton::Middle) {
            if let (Some(start), Some(pointer_pos)) =
                (app.pan_start, ctx.input(|i| i.pointer.hover_pos()))
            {
                app.pan += pointer_pos - start;
                app.pan_start = Some(pointer_pos);
                image_rect = Rect::from_min_size(base_min + app.pan, draw_size);
            }
        } else if !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle)) {
            app.pan_start = None;
        }
    }

    let zoom_delta = ctx.input(|i| i.zoom_delta());
    let scroll_delta = ctx.input(|i| i.smooth_scroll_delta);
    let scroll_zoom = if zoom_delta != 1.0 {
        zoom_delta
    } else {
        let base = 1.0025f32;
        let exponent = scroll_delta.y.clamp(-1200.0, 1200.0);
        base.powf(exponent)
    };
    if pan_zoom_enabled && scroll_zoom != 1.0 {
        if let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) {
            let prev_zoom = app.zoom;
            let next_zoom = clamp_zoom(prev_zoom * scroll_zoom, effective_min_zoom);
            if next_zoom != prev_zoom {
                let prev_size = image_size * prev_zoom;
                let prev_min = animated_canvas_rect.center() - prev_size * 0.5 + app.pan;
                let local = (pointer_pos - prev_min) / prev_size;
                app.zoom = next_zoom;
                let next_size = image_size * next_zoom;
                let next_min = pointer_pos - local * next_size;
                let desired_pan = next_min - (animated_canvas_rect.center() - next_size * 0.5);
                app.pan = desired_pan;
                image_rect = Rect::from_min_size(
                    animated_canvas_rect.center() - next_size * 0.5 + app.pan,
                    next_size,
                );
            }
        }
    }

    let rounding = if paint_frame {
        let alpha = (frame.sidebar_factor * 255.0).round() as u8;
        let fill = egui::Color32::from_rgba_unmultiplied(18, 18, 18, alpha);
        let stroke_color = egui::Color32::from_rgba_unmultiplied(48, 48, 48, alpha);
        let radius = (CANVAS_RADIUS * frame.sidebar_factor)
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

    let image_rect_size = image_rect.size();
    let uv0_min = (animated_canvas_rect.min - image_rect.min) / image_rect_size;
    let uv0_max = (animated_canvas_rect.max - image_rect.min) / image_rect_size;
    let uv0 = Rect::from_min_max(pos2(uv0_min.x, uv0_min.y), pos2(uv0_max.x, uv0_max.y));

    let mut uv1 = cover_uv_rect(animated_canvas_rect.size(), image_size);
    uv1 = Rect::from_center_size(uv0.center(), uv1.size());
    uv1 = clamp_uv_rect_into_unit(uv1);

    let uv_center = lerp_pos2(uv0.center(), uv1.center(), frame.sidebar_factor);
    let uv_size = lerp_vec2(uv0.size(), uv1.size(), frame.sidebar_factor);
    let computed_uv = Rect::from_center_size(uv_center, uv_size);
    ui.painter().add(
        egui::epaint::RectShape::filled(animated_canvas_rect, rounding, Color32::WHITE)
            .with_texture(app.color_attachment.unwrap(), computed_uv),
    );

    if response.clicked_by(egui::PointerButton::Primary) {
        if let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) {
            if animated_canvas_rect.contains(pointer_pos)
                && (if matches!(frame.mode, UiWindowMode::Sidebar) {
                    animated_canvas_rect.contains(pointer_pos)
                } else {
                    image_rect.contains(pointer_pos)
                })
            {
                let local = (pointer_pos - animated_canvas_rect.min) / animated_canvas_rect.size();
                let uv_x = computed_uv.min.x + local.x * computed_uv.width();
                let uv_y = computed_uv.min.y + local.y * computed_uv.height();
                let x = (uv_x * app.resolution[0] as f32).floor() as u32;
                let y = (uv_y * app.resolution[1] as f32).floor() as u32;
                if x < app.resolution[0] && y < app.resolution[1] {
                    if let Ok(image) = app
                        .shader_space
                        .read_texture_rgba8(app.output_texture_name.as_str())
                    {
                        let idx = ((y * app.resolution[0] + x) * 4) as usize;
                        if idx + 3 < image.bytes.len() {
                            app.last_sampled = Some(super::types::SampledPixel {
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

    app.canvas_center_prev = Some(animated_canvas_rect.center());

    requested_toggle_canvas_only
}
