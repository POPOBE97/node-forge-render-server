//! Bottom timeline panel for state-machine animation review.
//!
//! Frames are positioned on a real logical-time axis. The view supports
//! horizontal scrolling, command/control-wheel zoom, a transient hover cursor,
//! and one draggable persistent anchor.

use rust_wgpu_fiber::eframe::egui;

use crate::animation::{TimelineBuffer, TimelineFrame, TimelineFrameId};
use crate::state_machine::OverrideKey;

use super::design_tokens::{self, TextRole};

const DEFAULT_PIXELS_PER_SECOND: f32 = 600.0;
const MIN_PIXELS_PER_SECOND: f32 = 60.0;
const MAX_PIXELS_PER_SECOND: f32 = 2_400.0;
const ZOOM_SENSITIVITY: f32 = 0.01;
const HEADER_ROW_H: f32 = 18.0;
const VALUE_ROW_H: f32 = 20.0;
const LABEL_COL_W: f32 = 120.0;
const CONTENT_EDGE_PAD: f32 = 8.0;
const DIAMOND_HALF: f32 = 3.5;
const ANCHOR_HIT_RADIUS: f32 = 6.0;
const MIN_TICK_SPACING_PX: f32 = 64.0;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimelineInteraction {
    pub hovered_frame_id: Option<TimelineFrameId>,
    pub set_anchor_frame_id: Option<TimelineFrameId>,
    pub delete_anchor: bool,
}

pub fn show_timeline(
    ui: &mut egui::Ui,
    buffer: &TimelineBuffer,
    anchor_frame_id: Option<TimelineFrameId>,
) -> TimelineInteraction {
    let mut interaction = TimelineInteraction::default();

    if buffer.is_empty() {
        ui.label(design_tokens::rich_text(
            "Press Play or select a State to record",
            TextRole::InactiveItemTitle,
        ));
        return interaction;
    }

    let tracked_keys = &buffer.tracked_keys;
    let value_row_count = tracked_keys.len();
    let grid_h = HEADER_ROW_H + value_row_count.max(1) as f32 * VALUE_ROW_H;
    let available_w = ui.available_width();
    let has_labels = !tracked_keys.is_empty();
    let label_w = if has_labels { LABEL_COL_W } else { 0.0 };
    let grid_viewport_w = (available_w - label_w).max(1.0);

    let (total_rect, response) = ui.allocate_exact_size(
        egui::vec2(available_w, grid_h),
        egui::Sense::click_and_drag(),
    );
    let label_rect = egui::Rect::from_min_size(total_rect.min, egui::vec2(label_w, grid_h));
    let grid_clip_rect = egui::Rect::from_min_size(
        egui::pos2(total_rect.min.x + label_w, total_rect.min.y),
        egui::vec2(grid_viewport_w, grid_h),
    );

    let (start_time, end_time) = buffer.time_range().unwrap_or((0.0, 0.0));
    let scale_id = ui.id().with("timeline_pixels_per_second");
    let scroll_id = ui.id().with("timeline_scroll_x");
    let mut pixels_per_second = ui
        .ctx()
        .data_mut(|data| data.get_temp(scale_id).unwrap_or(DEFAULT_PIXELS_PER_SECOND));
    pixels_per_second = pixels_per_second.clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND);

    let initial_content_w =
        timeline_content_width(start_time, end_time, pixels_per_second, grid_viewport_w);
    let initial_max_scroll = (initial_content_w - grid_viewport_w).max(0.0);
    let mut scroll_x = ui
        .ctx()
        .data_mut(|data| data.get_temp(scroll_id).unwrap_or(initial_max_scroll));
    scroll_x = scroll_x.clamp(0.0, initial_max_scroll);
    let was_pinned = scroll_x >= initial_max_scroll - ANCHOR_HIT_RADIUS;

    let pointer_in_grid = ui
        .ctx()
        .input(|input| input.pointer.hover_pos())
        .filter(|pointer| grid_clip_rect.contains(*pointer));
    let mut user_changed_view = false;
    if let Some(pointer) = pointer_in_grid {
        let (scroll_delta, command_pressed, shift_pressed) = ui.ctx().input(|input| {
            (
                input.smooth_scroll_delta,
                input.modifiers.command,
                input.modifiers.shift,
            )
        });
        if command_pressed && scroll_delta.y.abs() > 0.5 {
            let old_scale = pixels_per_second;
            pixels_per_second = (old_scale * (scroll_delta.y * ZOOM_SENSITIVITY).exp())
                .clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND);
            let pointer_x = pointer.x - grid_clip_rect.min.x;
            scroll_x =
                zoom_scroll_around_pointer(scroll_x, pointer_x, old_scale, pixels_per_second);
            user_changed_view = (pixels_per_second - old_scale).abs() > f32::EPSILON;
        } else if !command_pressed {
            let horizontal_delta = if scroll_delta.x.abs() > 0.5 {
                scroll_delta.x
            } else if shift_pressed {
                scroll_delta.y
            } else {
                0.0
            };
            if horizontal_delta.abs() > 0.5 {
                scroll_x -= horizontal_delta;
                user_changed_view = true;
            }
        }
    }

    let content_w =
        timeline_content_width(start_time, end_time, pixels_per_second, grid_viewport_w);
    let max_scroll = (content_w - grid_viewport_w).max(0.0);
    if was_pinned && !user_changed_view {
        scroll_x = max_scroll;
    }
    scroll_x = scroll_x.clamp(0.0, max_scroll);
    ui.ctx().data_mut(|data| {
        data.insert_temp(scale_id, pixels_per_second);
        data.insert_temp(scroll_id, scroll_x);
    });

    let grid_origin_x = grid_clip_rect.min.x - scroll_x;
    let hovered_frame_id = pointer_in_grid.and_then(|pointer| {
        let time = pointer_time(
            pointer.x,
            grid_origin_x,
            start_time,
            end_time,
            pixels_per_second,
        );
        buffer.nearest_frame_id(time)
    });
    interaction.hovered_frame_id = hovered_frame_id;

    let pointer_pressed_or_dragged =
        response.clicked() || response.drag_started() || response.dragged();
    if pointer_pressed_or_dragged && pointer_in_grid.is_some() {
        response.request_focus();
        interaction.set_anchor_frame_id = hovered_frame_id;
    }
    if response.has_focus()
        && anchor_frame_id.is_some()
        && ui.ctx().input(|input| {
            input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace)
        })
    {
        interaction.delete_anchor = true;
    }

    if let (Some(pointer), Some(anchor_id)) = (pointer_in_grid, anchor_frame_id)
        && let Some(anchor) = buffer.frame_by_id(anchor_id)
    {
        let anchor_x = frame_x(anchor, grid_origin_x, start_time, pixels_per_second);
        if (pointer.x - anchor_x).abs() <= ANCHOR_HIT_RADIUS {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        } else {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    } else if pointer_in_grid.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }

    let grid_painter = ui.painter_at(grid_clip_rect);
    paint_time_grid(
        &grid_painter,
        grid_clip_rect,
        grid_origin_x,
        start_time,
        end_time,
        pixels_per_second,
    );
    paint_value_rows(
        &grid_painter,
        buffer,
        tracked_keys,
        grid_clip_rect,
        grid_origin_x,
        start_time,
        pixels_per_second,
    );

    if let Some(hover_id) = hovered_frame_id
        && let Some(frame) = buffer.frame_by_id(hover_id)
    {
        paint_cursor(
            &grid_painter,
            frame_x(frame, grid_origin_x, start_time, pixels_per_second),
            grid_clip_rect,
            egui::Color32::WHITE,
            false,
        );
    }
    if let Some(anchor_id) = anchor_frame_id
        && let Some(frame) = buffer.frame_by_id(anchor_id)
    {
        paint_cursor(
            &grid_painter,
            frame_x(frame, grid_origin_x, start_time, pixels_per_second),
            grid_clip_rect,
            egui::Color32::from_rgb(235, 67, 67),
            true,
        );
    }

    if has_labels {
        paint_labels(ui, label_rect, tracked_keys);
    }

    interaction
}

fn timeline_content_width(
    start_time: f64,
    end_time: f64,
    pixels_per_second: f32,
    viewport_width: f32,
) -> f32 {
    let duration = (end_time - start_time).max(0.0) as f32;
    (duration * pixels_per_second + CONTENT_EDGE_PAD * 2.0).max(viewport_width)
}

fn zoom_scroll_around_pointer(
    scroll_x: f32,
    pointer_x: f32,
    old_scale: f32,
    new_scale: f32,
) -> f32 {
    if old_scale <= 0.0 || !old_scale.is_finite() || !new_scale.is_finite() {
        return scroll_x;
    }
    let focus_time_offset = (scroll_x + pointer_x - CONTENT_EDGE_PAD) / old_scale;
    CONTENT_EDGE_PAD + focus_time_offset * new_scale - pointer_x
}

fn pointer_time(
    pointer_x: f32,
    grid_origin_x: f32,
    start_time: f64,
    end_time: f64,
    pixels_per_second: f32,
) -> f64 {
    let local_x = pointer_x - grid_origin_x - CONTENT_EDGE_PAD;
    (start_time + f64::from(local_x / pixels_per_second)).clamp(start_time, end_time)
}

fn frame_x(
    frame: &TimelineFrame,
    grid_origin_x: f32,
    start_time: f64,
    pixels_per_second: f32,
) -> f32 {
    grid_origin_x
        + CONTENT_EDGE_PAD
        + ((frame.presentation_time_secs - start_time) as f32 * pixels_per_second)
}

fn tick_step(pixels_per_second: f32) -> f64 {
    let minimum_seconds = f64::from(MIN_TICK_SPACING_PX / pixels_per_second.max(1.0));
    let exponent = minimum_seconds.log10().floor();
    let base = 10_f64.powf(exponent);
    for multiplier in [1.0, 2.0, 5.0, 10.0] {
        let candidate = base * multiplier;
        if candidate >= minimum_seconds * (1.0 - 1e-6) {
            return candidate;
        }
    }
    base * 10.0
}

fn paint_time_grid(
    painter: &egui::Painter,
    clip_rect: egui::Rect,
    grid_origin_x: f32,
    start_time: f64,
    end_time: f64,
    pixels_per_second: f32,
) {
    painter.line_segment(
        [
            egui::pos2(clip_rect.min.x, clip_rect.min.y + HEADER_ROW_H),
            egui::pos2(clip_rect.max.x, clip_rect.min.y + HEADER_ROW_H),
        ],
        egui::Stroke::new(0.5_f32, design_tokens::white(20)),
    );

    if (end_time - start_time).abs() < f64::EPSILON {
        let x = grid_origin_x + CONTENT_EDGE_PAD;
        painter.text(
            egui::pos2(x + 3.0, clip_rect.min.y + 2.0),
            egui::Align2::LEFT_TOP,
            format_tick(start_time),
            egui::FontId::proportional(8.0),
            design_tokens::white(55),
        );
        return;
    }

    let visible_start = pointer_time(
        clip_rect.min.x,
        grid_origin_x,
        start_time,
        end_time,
        pixels_per_second,
    );
    let visible_end = pointer_time(
        clip_rect.max.x,
        grid_origin_x,
        start_time,
        end_time,
        pixels_per_second,
    );
    let step = tick_step(pixels_per_second);
    let mut tick = (visible_start / step).ceil() * step;
    while tick <= visible_end + step * 0.001 {
        let x = grid_origin_x + CONTENT_EDGE_PAD + ((tick - start_time) as f32 * pixels_per_second);
        painter.line_segment(
            [
                egui::pos2(x, clip_rect.min.y + HEADER_ROW_H - 4.0),
                egui::pos2(x, clip_rect.max.y),
            ],
            egui::Stroke::new(0.5_f32, design_tokens::white(18)),
        );
        painter.text(
            egui::pos2(x + 3.0, clip_rect.min.y + 2.0),
            egui::Align2::LEFT_TOP,
            format_tick(tick),
            egui::FontId::proportional(8.0),
            design_tokens::white(55),
        );
        tick += step;
    }
}

fn format_tick(time_secs: f64) -> String {
    if time_secs.abs() >= 1.0 {
        if (time_secs - time_secs.round()).abs() < 0.001 {
            format!("{time_secs:.0}s")
        } else {
            format!("{time_secs:.1}s")
        }
    } else {
        format!("{time_secs:.2}s")
    }
}

fn paint_value_rows(
    painter: &egui::Painter,
    buffer: &TimelineBuffer,
    tracked_keys: &[String],
    clip_rect: egui::Rect,
    grid_origin_x: f32,
    start_time: f64,
    pixels_per_second: f32,
) {
    let parsed_keys: Vec<Option<OverrideKey>> = tracked_keys
        .iter()
        .map(|key| OverrideKey::parse(key))
        .collect();
    for (row_index, parsed_key) in parsed_keys.iter().enumerate() {
        let row_y = clip_rect.min.y + HEADER_ROW_H + row_index as f32 * VALUE_ROW_H;
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(clip_rect.min.x, row_y),
            egui::vec2(clip_rect.width(), VALUE_ROW_H),
        );
        if row_index % 2 == 0 {
            painter.rect_filled(row_rect, egui::CornerRadius::ZERO, design_tokens::white(10));
        }
        let Some(key) = parsed_key else { continue };
        for (frame_index, frame) in buffer.frames().iter().enumerate() {
            let current = frame.active_overrides.get(key);
            let previous = frame_index
                .checked_sub(1)
                .and_then(|index| buffer.frames().get(index))
                .and_then(|previous_frame| previous_frame.active_overrides.get(key));
            let is_keyframe = match (current, previous) {
                (Some(current), Some(previous)) => !json_values_equal(current, previous),
                (Some(_), None) | (None, Some(_)) => true,
                (None, None) => false,
            };
            if !is_keyframe {
                continue;
            }
            let x = frame_x(frame, grid_origin_x, start_time, pixels_per_second);
            if x >= clip_rect.min.x - DIAMOND_HALF && x <= clip_rect.max.x + DIAMOND_HALF {
                draw_diamond(
                    painter,
                    egui::pos2(x, row_y + VALUE_ROW_H * 0.5),
                    DIAMOND_HALF,
                    egui::Color32::from_rgb(255, 200, 60),
                );
            }
        }
    }
}

fn paint_cursor(
    painter: &egui::Painter,
    x: f32,
    clip_rect: egui::Rect,
    color: egui::Color32,
    anchor: bool,
) {
    painter.line_segment(
        [
            egui::pos2(x, clip_rect.min.y),
            egui::pos2(x, clip_rect.max.y),
        ],
        egui::Stroke::new(if anchor { 1.5_f32 } else { 1.0_f32 }, color),
    );
    if anchor {
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(x - 5.0, clip_rect.min.y + 2.0),
                egui::pos2(x + 5.0, clip_rect.min.y + 2.0),
                egui::pos2(x, clip_rect.min.y + 9.0),
            ],
            color,
            egui::Stroke::NONE,
        ));
    }
}

fn paint_labels(ui: &egui::Ui, label_rect: egui::Rect, tracked_keys: &[String]) {
    let painter = ui.painter_at(label_rect);
    painter.rect_filled(
        label_rect,
        egui::CornerRadius::ZERO,
        crate::color::lab(7.78201, -0.000_014_901_2, 0.0),
    );
    painter.text(
        egui::pos2(label_rect.min.x + 4.0, label_rect.min.y + 2.0),
        egui::Align2::LEFT_TOP,
        "Time",
        design_tokens::font_id(
            design_tokens::FONT_SIZE_9,
            design_tokens::FontWeight::Medium,
        ),
        design_tokens::white(50),
    );
    for (row_index, key) in tracked_keys.iter().enumerate() {
        let row_y = label_rect.min.y + HEADER_ROW_H + row_index as f32 * VALUE_ROW_H;
        painter.text(
            egui::pos2(label_rect.min.x + 4.0, row_y + 3.0),
            egui::Align2::LEFT_TOP,
            key,
            design_tokens::font_id(
                design_tokens::FONT_SIZE_11,
                design_tokens::FontWeight::Normal,
            ),
            design_tokens::white(60),
        );
    }
}

fn draw_diamond(painter: &egui::Painter, center: egui::Pos2, half: f32, color: egui::Color32) {
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(center.x, center.y - half),
            egui::pos2(center.x + half, center.y),
            egui::pos2(center.x, center.y + half),
            egui::pos2(center.x - half, center.y),
        ],
        color,
        egui::Stroke::NONE,
    ));
}

fn json_values_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a, b) {
        (serde_json::Value::Number(left), serde_json::Value::Number(right)) => {
            let left = left.as_f64().unwrap_or(0.0);
            let right = right.as_f64().unwrap_or(0.0);
            (left - right).abs() < 1e-6
        }
        (serde_json::Value::Array(left), serde_json::Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| json_values_equal(left, right))
        }
        (serde_json::Value::Bool(left), serde_json::Value::Bool(right)) => left == right,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_step_uses_one_two_five_progression() {
        assert_eq!(tick_step(640.0), 0.1);
        assert_eq!(tick_step(320.0), 0.2);
        assert_eq!(tick_step(100.0), 1.0);
        assert_eq!(tick_step(60.0), 2.0);
    }

    #[test]
    fn zoom_keeps_time_under_pointer_stationary() {
        let old_scroll = 200.0;
        let pointer_x = 150.0;
        let old_scale = 600.0;
        let new_scale = 1_200.0;
        let old_time = (old_scroll + pointer_x - CONTENT_EDGE_PAD) / old_scale;
        let new_scroll = zoom_scroll_around_pointer(old_scroll, pointer_x, old_scale, new_scale);
        let new_time = (new_scroll + pointer_x - CONTENT_EDGE_PAD) / new_scale;
        assert!((old_time - new_time).abs() < 1e-6);
    }

    #[test]
    fn content_width_preserves_real_time_span() {
        assert_eq!(timeline_content_width(2.0, 3.0, 600.0, 400.0), 616.0);
        assert_eq!(timeline_content_width(2.0, 2.0, 600.0, 400.0), 400.0);
    }
}
