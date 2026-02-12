use rust_wgpu_fiber::eframe::{
    egui::{self, Color32, Rect, pos2},
    egui_wgpu, wgpu,
};

use crate::ui::{
    animation_manager::{AnimationSpec, Easing},
    viewport_indicators::{ViewportIndicator, draw_viewport_indicators},
};

use super::{
    layout_math::{
        clamp_zoom, lerp,
    },
    texture_bridge,
    types::{App, SIDEBAR_ANIM_SECS, UiWindowMode},
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

    // ESC clears texture preview.
    if app.preview_texture_name.is_some()
        && ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        app.preview_texture_name = None;
        if let Some(id) = app.preview_color_attachment.take() {
            renderer.free_texture(&id);
        }
    }

    // Sync preview texture if active.
    let using_preview = if let Some(preview_name) = app.preview_texture_name.clone() {
        // Check the texture still exists.
        if app.shader_space.textures.contains_key(preview_name.as_str()) {
            texture_bridge::sync_preview_texture(
                app,
                render_state,
                renderer,
                &preview_name,
                app.texture_filter,
            );
            true
        } else {
            // Texture gone — clear preview.
            app.preview_texture_name = None;
            if let Some(id) = app.preview_color_attachment.take() {
                renderer.free_texture(&id);
            }
            false
        }
    } else {
        // Preview was cleared — free the attachment if it's still registered.
        if let Some(id) = app.preview_color_attachment.take() {
            renderer.free_texture(&id);
        }
        false
    };

    let avail_rect = ui.available_rect_before_wrap();

    // Use preview texture resolution when previewing.
    let effective_resolution = if using_preview {
        if let Some(ref pn) = app.preview_texture_name {
            if let Some(info) = app.shader_space.texture_info(pn.as_str()) {
                [info.size.width, info.size.height]
            } else {
                app.resolution
            }
        } else {
            app.resolution
        }
    } else {
        app.resolution
    };
    let image_size = egui::vec2(effective_resolution[0] as f32, effective_resolution[1] as f32);

    let animated_canvas_rect = avail_rect;

    let prev_center = app
        .canvas_center_prev
        .unwrap_or(animated_canvas_rect.center());
    let new_center = animated_canvas_rect.center();
    app.pan += prev_center - new_center;

    let fit_zoom = (animated_canvas_rect.width() / image_size.x)
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
        // Animate pan/zoom smoothly during mode transition.
        // Preserve the user's current pan/zoom in both directions —
        // no longer reset to fit-zoom when entering Sidebar mode.
        let (start_zoom, start_pan, target_zoom, target_pan) = match frame.mode {
            UiWindowMode::Sidebar => (
                app.zoom,
                app.pan,
                if app.pan_zoom_target_zoom > 0.0 {
                    app.pan_zoom_target_zoom
                } else {
                    app.zoom
                },
                app.pan_zoom_target_pan,
            ),
            UiWindowMode::CanvasOnly => (
                app.zoom,
                app.pan,
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
    let pan_zoom_enabled = !pan_zoom_animating;
    let effective_min_zoom = if pan_zoom_animating {
        0.01
    } else {
        min_zoom
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
        app.pan_zoom_target_zoom = fit_zoom;
        app.pan_zoom_target_pan = egui::Vec2::ZERO;
        let draw_size = image_size * app.zoom;
        let base_min = animated_canvas_rect.center() - draw_size * 0.5;
        image_rect = Rect::from_min_size(base_min, draw_size);
    }

    if app.pending_view_reset {
        app.zoom = fit_zoom;
        app.pan = egui::Vec2::ZERO;
        app.pan_start = None;
        app.pan_zoom_target_zoom = fit_zoom;
        app.pan_zoom_target_pan = egui::Vec2::ZERO;
        let draw_size = image_size * app.zoom;
        let base_min = animated_canvas_rect.center() - draw_size * 0.5;
        image_rect = Rect::from_min_size(base_min, draw_size);
        app.pending_view_reset = false;
    }

    if ctx.input(|i| i.key_pressed(egui::Key::P)) {
        app.texture_filter = match app.texture_filter {
            wgpu::FilterMode::Nearest => wgpu::FilterMode::Linear,
            wgpu::FilterMode::Linear => wgpu::FilterMode::Nearest,
        };
        if let Some(ref preview_name) = app.preview_texture_name.clone() {
            texture_bridge::sync_preview_texture(
                app,
                render_state,
                renderer,
                preview_name,
                app.texture_filter,
            );
        }
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
        // Pan with middle mouse button drag.
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

        // Pan with primary button drag (for trackpad users).
        if response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) {
                app.pan_start = Some(pointer_pos);
            }
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            if let (Some(start), Some(pointer_pos)) =
                (app.pan_start, ctx.input(|i| i.pointer.hover_pos()))
            {
                app.pan += pointer_pos - start;
                app.pan_start = Some(pointer_pos);
                image_rect = Rect::from_min_size(base_min + app.pan, draw_size);
            }
        } else if !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Primary))
            && !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle))
        {
            app.pan_start = None;
        }
    }

    // Only process scroll/zoom when pointer is over the canvas, so sidebar
    // scroll events don't leak into the canvas.
    let canvas_hovered = response.hovered();
    let zoom_delta = if canvas_hovered { ctx.input(|i| i.zoom_delta()) } else { 1.0 };
    let scroll_delta = if canvas_hovered { ctx.input(|i| i.smooth_scroll_delta) } else { egui::Vec2::ZERO };

    // Pan with two-finger scroll (trackpad) when not pinch-zooming.
    if pan_zoom_enabled && zoom_delta == 1.0 && (scroll_delta.x != 0.0 || scroll_delta.y != 0.0) {
        app.pan += scroll_delta;
        image_rect = Rect::from_min_size(base_min + app.pan, draw_size);
    }

    let scroll_zoom = if zoom_delta != 1.0 {
        zoom_delta
    } else {
        1.0
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

    let rounding = egui::CornerRadius::ZERO;

    // Draw checkerboard background for transparency (GPU-tiled 2×2 texture).
    {
        let checker_tex = {
            let cache_id = egui::Id::new("ui.canvas.checkerboard_texture");
            if let Some(tex) =
                ctx.memory(|mem| mem.data.get_temp::<egui::TextureHandle>(cache_id))
            {
                tex
            } else {
                let c0 = Color32::from_gray(28);
                let c1 = Color32::from_gray(38);
                let pixels = vec![c0, c1, c1, c0];
                let img = egui::ColorImage {
                    size: [2, 2],
                    pixels,
                    source_size: egui::Vec2::new(2.0, 2.0),
                };
                let tex = ctx.load_texture(
                    "ui.canvas.checkerboard",
                    img,
                    egui::TextureOptions::NEAREST_REPEAT,
                );
                ctx.memory_mut(|mem| mem.data.insert_temp(cache_id, tex.clone()));
                tex
            }
        };
        let cell = 16.0_f32;
        let uv_w = animated_canvas_rect.width() / cell;
        let uv_h = animated_canvas_rect.height() / cell;
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(uv_w, uv_h));
        ui.painter().add(
            egui::epaint::RectShape::filled(animated_canvas_rect, rounding, Color32::WHITE)
                .with_texture(checker_tex.id(), uv),
        );
    }

    let image_rect_size = image_rect.size();
    let uv0_min = (animated_canvas_rect.min - image_rect.min) / image_rect_size;
    let uv0_max = (animated_canvas_rect.max - image_rect.min) / image_rect_size;
    let computed_uv = Rect::from_min_max(pos2(uv0_min.x, uv0_min.y), pos2(uv0_max.x, uv0_max.y));

    let display_attachment = if using_preview {
        app.preview_color_attachment.or(app.color_attachment)
    } else {
        app.color_attachment
    };

    if let Some(tex_id) = display_attachment {
        ui.painter().add(
            egui::epaint::RectShape::filled(animated_canvas_rect, rounding, Color32::WHITE)
                .with_texture(tex_id, computed_uv),
        );
    }

    let sampling_indicator = match app.texture_filter {
        wgpu::FilterMode::Nearest => ViewportIndicator {
            icon: "N",
            tooltip: "Viewport sampling: Nearest (press P to toggle Linear)",
        },
        wgpu::FilterMode::Linear => ViewportIndicator {
            icon: "L",
            tooltip: "Viewport sampling: Linear (press P to toggle Nearest)",
        },
    };
    draw_viewport_indicators(ui, animated_canvas_rect, &[sampling_indicator]);

    // Draw preview overlay badge.
    if let Some(ref preview_name) = app.preview_texture_name {
        if using_preview {
            let badge_text = if let Some(info) = app.shader_space.texture_info(preview_name.as_str()) {
                format!(
                    "Preview: {}  {}×{} {:?}",
                    preview_name.as_str(),
                    info.size.width,
                    info.size.height,
                    info.format,
                )
            } else {
                format!("Preview: {}", preview_name.as_str())
            };
            let badge_font = egui::FontId::new(
                11.0,
                crate::ui::typography::mi_sans_family_for_weight(500.0),
            );
            let badge_galley = ui.painter().layout_no_wrap(
                badge_text,
                badge_font,
                Color32::from_gray(220),
            );
            let badge_size = badge_galley.size() + egui::vec2(16.0, 8.0);
            let badge_rect = Rect::from_min_size(
                pos2(
                    animated_canvas_rect.min.x + 8.0,
                    animated_canvas_rect.min.y + 8.0,
                ),
                badge_size,
            );
            ui.painter().rect(
                badge_rect,
                egui::CornerRadius::same(6),
                Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                egui::Stroke::new(1.0, Color32::from_gray(32)),
                egui::StrokeKind::Outside,
            );
            ui.painter().galley(
                pos2(badge_rect.min.x + 8.0, badge_rect.min.y + 4.0),
                badge_galley,
                Color32::PLACEHOLDER,
            );

            // Close button "×" at right of badge.
            let close_rect = Rect::from_min_size(
                pos2(badge_rect.max.x + 4.0, badge_rect.min.y),
                egui::vec2(badge_size.y, badge_size.y),
            );
            let close_resp = ui.allocate_rect(close_rect, egui::Sense::click());
            let close_color = if close_resp.hovered() {
                Color32::from_gray(255)
            } else {
                Color32::from_gray(160)
            };
            ui.painter().text(
                close_rect.center(),
                egui::Align2::CENTER_CENTER,
                "×",
                egui::FontId::new(14.0, egui::FontFamily::Proportional),
                close_color,
            );
            if close_resp.clicked() {
                // Will be handled next frame.
                // (We can't easily free the texture here because we're borrowing app mutably
                //  via renderer — set a flag and the App::update() will handle cleanup.)
            }
        }
    }

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
