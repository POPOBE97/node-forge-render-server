//! Bottom timeline panel for state-machine animation playback.
//!
//! Renders a tile-grid timeline where each cell is exactly `CELL_W` pixels
//! wide.  The grid grows as frames are recorded and scrolls horizontally
//! when it overflows the container.  Each tracked value gets its own row
//! with a label pinned to the left.  A diamond ◆ keyframe marker is drawn
//! whenever a cell's value differs from the previous cell.

use rust_wgpu_fiber::eframe::egui;

use crate::animation::TimelineBuffer;
use crate::state_machine::OverrideKey;

use super::design_tokens::{self, TextRole};

// ── Constants ────────────────────────────────────────────────────────────

const CELL_W: f32 = 10.0;
const HEADER_ROW_H: f32 = 16.0;
const VALUE_ROW_H: f32 = 20.0;
const LABEL_COL_W: f32 = 120.0;
const DIAMOND_HALF: f32 = 3.5;

/// Result of a single frame of timeline widget interaction.
#[derive(Debug, Clone, Default)]
pub struct TimelineInteraction {
    /// Index of the frame the cursor is hovering over, if any.
    pub hovered_frame_index: Option<usize>,
}

/// Draw the timeline panel.  Returns interaction state.
pub fn show_timeline(ui: &mut egui::Ui, buffer: &TimelineBuffer) -> TimelineInteraction {
    let mut interaction = TimelineInteraction::default();

    if buffer.is_empty() {
        ui.label(design_tokens::rich_text(
            "Press Play to record",
            TextRole::InactiveItemTitle,
        ));
        return interaction;
    }

    let frame_count = buffer.len();
    let tracked_keys = &buffer.tracked_keys;
    let value_row_count = tracked_keys.len();

    // Total grid width = one cell per recorded frame.
    let grid_w = frame_count as f32 * CELL_W;
    // Total height = header + value rows.
    let grid_h = HEADER_ROW_H + value_row_count.max(1) as f32 * VALUE_ROW_H;

    let available_w = ui.available_width();
    let has_labels = !tracked_keys.is_empty();
    let label_w = if has_labels { LABEL_COL_W } else { 0.0 };

    // ── Layout: [label column | scrollable grid] ─────────────────────────
    // We use a horizontal layout so the label column stays pinned.
    let grid_viewport_w = (available_w - label_w).max(0.0);

    // Allocate the full height so egui knows our size.
    let (total_rect, _) =
        ui.allocate_exact_size(egui::vec2(available_w, grid_h), egui::Sense::hover());

    let label_rect = egui::Rect::from_min_size(total_rect.min, egui::vec2(label_w, grid_h));
    let grid_clip_rect = egui::Rect::from_min_size(
        egui::pos2(total_rect.min.x + label_w, total_rect.min.y),
        egui::vec2(grid_viewport_w, grid_h),
    );

    // ── Scroll state (stored in egui temp memory) ────────────────────────
    let scroll_id = ui.id().with("tl_scroll");
    let max_scroll = (grid_w - grid_viewport_w).max(0.0);
    let mut scroll_x: f32 = ui
        .ctx()
        .data_mut(|d| d.get_temp(scroll_id).unwrap_or(max_scroll));

    // Auto-follow: pin to the right edge unless the user has scrolled
    // away.  Re-engage when they scroll back near the end.
    let was_pinned = scroll_x >= max_scroll - CELL_W;

    // Handle mouse wheel / trackpad inside the grid viewport.
    // Map vertical scroll to horizontal (standard UX for horizontal-only
    // panels) and also accept native horizontal scroll (trackpad swipe).
    let mut user_scrolled = false;
    if ui.rect_contains_pointer(grid_clip_rect) {
        let delta = ui.ctx().input(|i| i.smooth_scroll_delta);
        // Prefer horizontal if present, otherwise use vertical.
        let dx = if delta.x.abs() > 0.5 {
            delta.x
        } else {
            delta.y
        };
        if dx.abs() > 0.5 {
            scroll_x = (scroll_x - dx).clamp(0.0, max_scroll);
            user_scrolled = true;
        }
    }

    // Only auto-follow if the user didn't just scroll and was already
    // pinned to the right edge.
    if was_pinned && !user_scrolled {
        scroll_x = max_scroll;
    }
    scroll_x = scroll_x.clamp(0.0, max_scroll);
    ui.ctx().data_mut(|d| d.insert_temp(scroll_id, scroll_x));

    // The grid origin in screen space (may be negative / off-screen left).
    let grid_origin = egui::pos2(grid_clip_rect.min.x - scroll_x, grid_clip_rect.min.y);

    // ── Hover detection ──────────────────────────────────────────────────
    let pointer_in_grid = ui.ctx().input(|i| i.pointer.hover_pos()).and_then(|p| {
        if grid_clip_rect.contains(p) {
            Some(p)
        } else {
            None
        }
    });

    let hovered_cell: Option<usize> = pointer_in_grid.and_then(|p| {
        let local_x = p.x - grid_origin.x;
        if local_x < 0.0 {
            return None;
        }
        let col = (local_x / CELL_W) as usize;
        if col < frame_count { Some(col) } else { None }
    });
    interaction.hovered_frame_index = hovered_cell;

    // ── Paint: clipped grid area ─────────────────────────────────────────
    let grid_painter = ui.painter_at(grid_clip_rect);
    let frames = buffer.frames();

    // Visible cell range (avoid drawing thousands of off-screen cells).
    let vis_first = (scroll_x / CELL_W).floor() as usize;
    let vis_last = ((scroll_x + grid_viewport_w) / CELL_W).ceil() as usize;
    let vis_last = vis_last.min(frame_count);

    // ── Header row (scene-time anchored second marks) ──────────────────
    //
    // Labels are anchored to absolute scene_time (whole seconds) so they
    // stay rock-steady even when the rolling buffer trims old frames.
    {
        let header_y = grid_origin.y;
        // Subtle separator line below the header.
        grid_painter.line_segment(
            [
                egui::pos2(
                    grid_origin.x + vis_first as f32 * CELL_W,
                    header_y + HEADER_ROW_H,
                ),
                egui::pos2(
                    grid_origin.x + vis_last as f32 * CELL_W,
                    header_y + HEADER_ROW_H,
                ),
            ],
            egui::Stroke::new(0.5, design_tokens::white(20)),
        );

        // Walk visible cells and place a tick at each whole-second boundary.
        // We scan from vis_first to vis_last, checking where
        // floor(scene_time) changes between adjacent frames.
        let first_scene_t = frames
            .get(vis_first)
            .map(|f| f.scene_time_secs)
            .unwrap_or(0.0);
        let mut next_whole_sec = first_scene_t.ceil(); // first whole second >= first visible frame

        for col in vis_first..vis_last {
            let t = frames[col].scene_time_secs;
            if t >= next_whole_sec {
                let secs = next_whole_sec as u64;
                let x = grid_origin.x + col as f32 * CELL_W;
                // Tick mark.
                grid_painter.line_segment(
                    [
                        egui::pos2(x, header_y + HEADER_ROW_H - 4.0),
                        egui::pos2(x, header_y + HEADER_ROW_H),
                    ],
                    egui::Stroke::new(0.5, design_tokens::white(40)),
                );
                let label = format!("{secs}s");
                grid_painter.text(
                    egui::pos2(x + 2.0, header_y + 1.0),
                    egui::Align2::LEFT_TOP,
                    &label,
                    egui::FontId::proportional(8.0),
                    design_tokens::white(50),
                );
                next_whole_sec += 1.0;
            }
        }
    }

    // ── Value rows ───────────────────────────────────────────────────────
    let parsed_keys: Vec<Option<OverrideKey>> =
        tracked_keys.iter().map(|k| OverrideKey::parse(k)).collect();

    for (row_idx, parsed) in parsed_keys.iter().enumerate() {
        let row_y = grid_origin.y + HEADER_ROW_H + row_idx as f32 * VALUE_ROW_H;

        // Row background (alternating subtle shade).
        let bg = if row_idx % 2 == 0 {
            design_tokens::white(10)
        } else {
            egui::Color32::TRANSPARENT
        };
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(grid_origin.x + vis_first as f32 * CELL_W, row_y),
            egui::vec2((vis_last - vis_first) as f32 * CELL_W, VALUE_ROW_H),
        );
        grid_painter.rect_filled(row_rect, egui::CornerRadius::ZERO, bg);

        // Vertical grid lines.
        for col in vis_first..=vis_last {
            let x = grid_origin.x + col as f32 * CELL_W;
            grid_painter.line_segment(
                [egui::pos2(x, row_y), egui::pos2(x, row_y + VALUE_ROW_H)],
                egui::Stroke::new(0.5, design_tokens::white(10)),
            );
        }

        let Some(key) = parsed else { continue };

        // Draw keyframe diamonds where value changes.
        for col in vis_first..vis_last {
            let cur = frames[col].active_overrides.get(key);
            let prev = if col > 0 {
                frames[col - 1].active_overrides.get(key)
            } else {
                None
            };
            let is_keyframe = match (cur, prev) {
                (Some(c), Some(p)) => !json_values_equal(c, p),
                (Some(_), None) => true, // value appeared
                (None, Some(_)) => true, // value disappeared
                (None, None) => false,
            };
            if is_keyframe {
                let cx = grid_origin.x + col as f32 * CELL_W + CELL_W * 0.5;
                let cy = row_y + VALUE_ROW_H * 0.5;
                draw_diamond(
                    &grid_painter,
                    egui::pos2(cx, cy),
                    DIAMOND_HALF,
                    egui::Color32::from_rgb(255, 200, 60),
                );
            }
        }
    }

    // ── Hover highlight column ───────────────────────────────────────────
    if let Some(col) = hovered_cell {
        let x = grid_origin.x + col as f32 * CELL_W;
        let value_area_y = grid_origin.y + HEADER_ROW_H;
        let value_area_h = grid_h - HEADER_ROW_H;
        let highlight_rect = egui::Rect::from_min_size(
            egui::pos2(x, value_area_y),
            egui::vec2(CELL_W, value_area_h),
        );
        grid_painter.rect_filled(
            highlight_rect,
            egui::CornerRadius::ZERO,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 30),
        );
        // Vertical cursor line.
        let cx = x + CELL_W * 0.5;
        grid_painter.line_segment(
            [
                egui::pos2(cx, value_area_y),
                egui::pos2(cx, value_area_y + value_area_h),
            ],
            egui::Stroke::new(1.0, egui::Color32::WHITE),
        );
    }

    // ── Label column (pinned, not scrolled) ──────────────────────────────
    if has_labels {
        let label_painter = ui.painter_at(label_rect);
        // Background to occlude grid content that scrolls behind.
        label_painter.rect_filled(
            label_rect,
            egui::CornerRadius::ZERO,
            crate::color::lab(7.78201, -0.000_014_901_2, 0.0),
        );

        for (row_idx, key) in tracked_keys.iter().enumerate() {
            let row_y = label_rect.min.y + HEADER_ROW_H + row_idx as f32 * VALUE_ROW_H;
            label_painter.text(
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

    // ── Hover tooltip ────────────────────────────────────────────────────
    if let Some(idx) = interaction.hovered_frame_index
        && let Some(frame) = buffer.frame_at(idx)
    {
        if let Some(pointer) = ui.ctx().input(|i| i.pointer.hover_pos()) {
            let label_font = design_tokens::font_id(
                design_tokens::FONT_SIZE_9,
                design_tokens::FontWeight::Normal,
            );
            let value_font = design_tokens::font_id(
                design_tokens::FONT_SIZE_9,
                design_tokens::FontWeight::Medium,
            );
            let label_color = design_tokens::white(50);
            let value_color = design_tokens::white(90);
            let section_font =
                design_tokens::font_id(design_tokens::FONT_SIZE_9, design_tokens::FontWeight::Bold);
            let section_color = design_tokens::white(40);

            egui::Area::new(ui.id().with("timeline_tooltip"))
                .fixed_pos(egui::pos2(pointer.x + 12.0, total_rect.min.y))
                .pivot(egui::Align2::LEFT_BOTTOM)
                .order(egui::Order::Tooltip)
                .show(ui.ctx(), |ui| {
                    egui::Frame::NONE
                        .fill(crate::color::lab(10.0, 0.0, 0.0))
                        .stroke(egui::Stroke::new(1.0, design_tokens::white(10)))
                        .corner_radius(design_tokens::BORDER_RADIUS_SMALL)
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.set_min_width(160.0);
                            let grid_id = ui.id().with("tt_grid");

                            // ── Frame / Timing ───────────────────────
                            ui.label(
                                egui::RichText::new("TIMING")
                                    .font(section_font.clone())
                                    .color(section_color),
                            );
                            ui.add_space(2.0);
                            egui::Grid::new(grid_id.with("timing"))
                                .num_columns(2)
                                .min_col_width(70.0)
                                .spacing(egui::vec2(12.0, 1.0))
                                .show(ui, |ui| {
                                    tooltip_row(
                                        ui,
                                        "Frame",
                                        &format!("#{idx}"),
                                        &label_font,
                                        &value_font,
                                        label_color,
                                        value_color,
                                    );
                                    tooltip_row(
                                        ui,
                                        "Scene",
                                        &format!("{:.3}s", frame.scene_time_secs),
                                        &label_font,
                                        &value_font,
                                        label_color,
                                        value_color,
                                    );
                                    tooltip_row(
                                        ui,
                                        "Wall",
                                        &format!("{:.3}s", frame.presentation_time_secs),
                                        &label_font,
                                        &value_font,
                                        label_color,
                                        value_color,
                                    );
                                });

                            // ── State ────────────────────────────────
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("STATE")
                                    .font(section_font.clone())
                                    .color(section_color),
                            );
                            ui.add_space(2.0);
                            egui::Grid::new(grid_id.with("state"))
                                .num_columns(2)
                                .min_col_width(70.0)
                                .spacing(egui::vec2(12.0, 1.0))
                                .show(ui, |ui| {
                                    tooltip_row(
                                        ui,
                                        "Active",
                                        &frame.current_state_id,
                                        &label_font,
                                        &value_font,
                                        label_color,
                                        value_color,
                                    );
                                    if let Some(ref tid) = frame.active_transition_id {
                                        tooltip_row(
                                            ui,
                                            "Transition",
                                            tid,
                                            &label_font,
                                            &value_font,
                                            label_color,
                                            value_color,
                                        );
                                        if let Some(blend) = frame.transition_blend {
                                            tooltip_row(
                                                ui,
                                                "Blend",
                                                &format!("{blend:.2}"),
                                                &label_font,
                                                &value_font,
                                                label_color,
                                                value_color,
                                            );
                                        }
                                    }
                                });

                            // ── Overrides ────────────────────────────
                            if !frame.active_overrides.is_empty() {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new("VALUES")
                                        .font(section_font.clone())
                                        .color(section_color),
                                );
                                ui.add_space(2.0);
                                egui::Grid::new(grid_id.with("values"))
                                    .num_columns(2)
                                    .min_col_width(70.0)
                                    .spacing(egui::vec2(12.0, 1.0))
                                    .show(ui, |ui| {
                                        let mut sorted: Vec<_> =
                                            frame.active_overrides.iter().collect();
                                        sorted.sort_by(|a, b| {
                                            (&a.0.node_id, &a.0.param_name)
                                                .cmp(&(&b.0.node_id, &b.0.param_name))
                                        });
                                        for (k, v) in sorted {
                                            let key_label =
                                                format!("{}.{}", k.node_id, k.param_name);
                                            let val_str =
                                                super::state_machine_panel::format_json_value_2dp(
                                                    v,
                                                );
                                            tooltip_row(
                                                ui,
                                                &key_label,
                                                &val_str,
                                                &label_font,
                                                &value_font,
                                                label_color,
                                                value_color,
                                            );
                                        }
                                    });
                            }

                            // ── Diagnostics ──────────────────────────
                            if !frame.diagnostics.is_empty() {
                                ui.add_space(4.0);
                                for diag in &frame.diagnostics {
                                    ui.label(
                                        egui::RichText::new(format!("⚠ {diag}"))
                                            .font(label_font.clone())
                                            .color(egui::Color32::from_rgb(255, 200, 80)),
                                    );
                                }
                            }
                        });
                });
        }
    }

    interaction
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Single row inside a tooltip grid: dim label left, bright value right.
fn tooltip_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    label_font: &egui::FontId,
    value_font: &egui::FontId,
    label_color: egui::Color32,
    value_color: egui::Color32,
) {
    ui.label(
        egui::RichText::new(label)
            .font(label_font.clone())
            .color(label_color),
    );
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(
            egui::RichText::new(value)
                .font(value_font.clone())
                .color(value_color),
        );
    });
    ui.end_row();
}

/// Draw a diamond (rotated square) centred at `center`.
fn draw_diamond(painter: &egui::Painter, center: egui::Pos2, half: f32, color: egui::Color32) {
    let points = vec![
        egui::pos2(center.x, center.y - half),
        egui::pos2(center.x + half, center.y),
        egui::pos2(center.x, center.y + half),
        egui::pos2(center.x - half, center.y),
    ];
    painter.add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

/// Compare two JSON values for approximate equality (numeric tolerance).
fn json_values_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a, b) {
        (serde_json::Value::Number(na), serde_json::Value::Number(nb)) => {
            let fa = na.as_f64().unwrap_or(0.0);
            let fb = nb.as_f64().unwrap_or(0.0);
            (fa - fb).abs() < 1e-6
        }
        (serde_json::Value::Array(aa), serde_json::Value::Array(ab)) => {
            aa.len() == ab.len()
                && aa
                    .iter()
                    .zip(ab.iter())
                    .all(|(x, y)| json_values_equal(x, y))
        }
        (serde_json::Value::Bool(ba), serde_json::Value::Bool(bb)) => ba == bb,
        _ => a == b,
    }
}
