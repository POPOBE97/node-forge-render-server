//! Bottom timeline panel for state-machine animation review.
//!
//! Frames are positioned on a real logical-time axis. The view supports
//! horizontal scrolling, command/control-wheel zoom, a transient hover cursor,
//! and one draggable persistent anchor.

use std::collections::{BTreeSet, HashSet};

use rust_wgpu_fiber::eframe::egui;

use crate::animation::{TimelineBuffer, TimelineFrame, TimelineFrameId};
use crate::state_machine::OverrideKey;

use super::{
    animation_curves::{
        self, CurveSeries, CurveSignal, MAX_PIXELS_PER_SECOND, MIN_PIXELS_PER_SECOND,
        StateFrameRef, TimeAxisViewState, XAxisGesture,
    },
    button::{self, ButtonGroupPosition, ButtonOptions, ButtonSize, ButtonVariant},
    design_tokens::{self, TextRole},
};

const HEADER_ROW_H: f32 = 28.0;
const VALUE_ROW_H: f32 = 24.0;
const LABEL_COL_W: f32 = 220.0;
pub(crate) const CONTENT_EDGE_PAD: f32 = 8.0;
const DIAMOND_HALF: f32 = 3.5;
const ANCHOR_HIT_RADIUS: f32 = 6.0;
const MIN_TICK_SPACING_PX: f32 = 64.0;
const CURVE_MIN_ROWS: usize = 6;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TimelineDisplayMode {
    #[default]
    Keyframes,
    Curve,
}

#[derive(Debug, Default)]
pub(crate) struct TimelinePanelState {
    mode: TimelineDisplayMode,
    filter: String,
    hidden_channels: HashSet<String>,
    time_view: TimeAxisViewState,
    source_recording_id: Option<u64>,
}

impl TimelinePanelState {
    pub(crate) fn sync_recording(&mut self, buffer: &TimelineBuffer) {
        if self.source_recording_id == Some(buffer.recording_id()) {
            return;
        }
        *self = Self {
            source_recording_id: Some(buffer.recording_id()),
            ..Self::default()
        };
    }

    fn channel_visible(&self, key: &str) -> bool {
        !self.hidden_channels.contains(key)
    }

    fn toggle_channel(&mut self, key: &str) {
        if !self.hidden_channels.remove(key) {
            self.hidden_channels.insert(key.to_string());
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimelineInteraction {
    pub hovered_frame_id: Option<TimelineFrameId>,
    pub set_anchor_frame_id: Option<TimelineFrameId>,
    pub delete_anchor: bool,
}

pub(crate) fn show_timeline(
    ui: &mut egui::Ui,
    buffer: &TimelineBuffer,
    anchor_frame_id: Option<TimelineFrameId>,
    state: &mut TimelinePanelState,
) -> TimelineInteraction {
    let mut interaction = TimelineInteraction::default();
    state.sync_recording(buffer);

    if buffer.is_empty() {
        ui.label(design_tokens::rich_text(
            "Press Play or select a State to record",
            TextRole::InactiveItemTitle,
        ));
        return interaction;
    }

    let available_w = ui.available_width();
    let label_w = LABEL_COL_W.min((available_w - 1.0).max(0.0));
    let grid_viewport_w = (available_w - label_w).max(1.0);
    let (header_rect, header_response) = ui.allocate_exact_size(
        egui::vec2(available_w, HEADER_ROW_H),
        egui::Sense::click_and_drag(),
    );
    let label_header_rect =
        egui::Rect::from_min_size(header_rect.min, egui::vec2(label_w, HEADER_ROW_H));
    show_header_controls(ui, label_header_rect, state);

    let physical_series = (state.mode == TimelineDisplayMode::Curve)
        .then(|| build_physical_series(buffer))
        .unwrap_or_default();
    let normalized_filter = state.filter.trim().to_lowercase();
    let display_keys =
        filtered_channel_keys(buffer, &physical_series, state.mode, &normalized_filter);
    let row_count = match state.mode {
        TimelineDisplayMode::Keyframes => display_keys.len().max(1),
        TimelineDisplayMode::Curve => display_keys.len().max(CURVE_MIN_ROWS),
    };
    let body_h = row_count as f32 * VALUE_ROW_H;
    let (body_rect, body_response) = ui.allocate_exact_size(
        egui::vec2(available_w, body_h),
        egui::Sense::click_and_drag(),
    );
    let label_body_rect = egui::Rect::from_min_size(body_rect.min, egui::vec2(label_w, body_h));
    let grid_header_rect = egui::Rect::from_min_size(
        egui::pos2(header_rect.min.x + label_w, header_rect.min.y),
        egui::vec2(grid_viewport_w, HEADER_ROW_H),
    );
    let grid_body_rect = egui::Rect::from_min_size(
        egui::pos2(body_rect.min.x + label_w, body_rect.min.y),
        egui::vec2(grid_viewport_w, body_h),
    );
    let grid_clip_rect = grid_header_rect.union(grid_body_rect);
    let response = header_response.union(body_response);

    let (start_time, end_time) = buffer.time_range().unwrap_or((0.0, 0.0));
    let pointer_in_grid = ui
        .ctx()
        .input(|input| input.pointer.hover_pos())
        .filter(|pointer| grid_clip_rect.contains(*pointer));
    update_time_axis_view(
        ui,
        pointer_in_grid,
        grid_header_rect.min.x,
        grid_viewport_w,
        start_time,
        end_time,
        &mut state.time_view,
    );
    let pixels_per_second = state.time_view.pixels_per_second;
    let scroll_x = state.time_view.scroll_x;

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

    show_channel_buttons(ui, label_body_rect, &display_keys, state);
    let grid_painter = ui.painter_at(grid_clip_rect);
    paint_time_grid(
        &grid_painter,
        grid_clip_rect,
        grid_origin_x,
        start_time,
        end_time,
        pixels_per_second,
    );
    match state.mode {
        TimelineDisplayMode::Keyframes => paint_value_rows(
            &grid_painter,
            buffer,
            &display_keys,
            &state.hidden_channels,
            grid_clip_rect,
            grid_origin_x,
            start_time,
            pixels_per_second,
        ),
        TimelineDisplayMode::Curve => paint_curve_view(
            &grid_painter,
            buffer,
            &physical_series,
            &display_keys,
            &state.hidden_channels,
            grid_body_rect,
            grid_origin_x,
            start_time,
            pixels_per_second,
        ),
    }

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

    interaction
}

fn show_header_controls(
    ui: &mut egui::Ui,
    header_rect: egui::Rect,
    state: &mut TimelinePanelState,
) {
    ui.painter().rect_filled(
        header_rect,
        egui::CornerRadius::ZERO,
        crate::color::lab(7.78201, -0.000_014_901_2, 0.0),
    );
    let inner = header_rect.shrink2(egui::vec2(4.0, 3.0));
    let mut header_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("timeline-header-controls")
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    header_ui.spacing_mut().item_spacing.x = 2.0;
    let keyframes_response = button::button_with_width(
        &mut header_ui,
        ButtonOptions {
            label: "K",
            tooltip: Some("Show keyframes"),
            variant: if state.mode == TimelineDisplayMode::Keyframes {
                ButtonVariant::Default
            } else {
                ButtonVariant::Secondary
            },
            size: ButtonSize::ExtraSmall,
            enabled: true,
            icon: None,
            icon_kind: None,
            visual_override: None,
            group_position: ButtonGroupPosition::First,
        },
        26.0,
    );
    let curve_response = button::button_with_width(
        &mut header_ui,
        ButtonOptions {
            label: "C",
            tooltip: Some("Show normalized physical curves"),
            variant: if state.mode == TimelineDisplayMode::Curve {
                ButtonVariant::Default
            } else {
                ButtonVariant::Secondary
            },
            size: ButtonSize::ExtraSmall,
            enabled: true,
            icon: None,
            icon_kind: None,
            visual_override: None,
            group_position: ButtonGroupPosition::Last,
        },
        26.0,
    );
    if keyframes_response.clicked() {
        state.mode = TimelineDisplayMode::Keyframes;
    }
    if curve_response.clicked() {
        state.mode = TimelineDisplayMode::Curve;
    }

    let filter_width = header_ui.available_width().max(1.0);
    header_ui.add_sized(
        egui::vec2(filter_width, 22.0),
        egui::TextEdit::singleline(&mut state.filter).hint_text("Filter"),
    );
}

fn show_channel_buttons(
    ui: &mut egui::Ui,
    label_rect: egui::Rect,
    display_keys: &[String],
    state: &mut TimelinePanelState,
) {
    ui.painter().rect_filled(
        label_rect,
        egui::CornerRadius::ZERO,
        crate::color::lab(7.78201, -0.000_014_901_2, 0.0),
    );
    for (row_index, key) in display_keys.iter().enumerate() {
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(
                label_rect.left() + 4.0,
                label_rect.top() + row_index as f32 * VALUE_ROW_H + 1.0,
            ),
            egui::vec2((label_rect.width() - 8.0).max(1.0), VALUE_ROW_H - 2.0),
        );
        let mut row_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("timeline-channel", key))
                .max_rect(row_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let visible = state.channel_visible(key);
        let response = button::button_with_width(
            &mut row_ui,
            ButtonOptions {
                label: key,
                tooltip: Some(key),
                variant: if visible {
                    ButtonVariant::Default
                } else {
                    ButtonVariant::Ghost
                },
                size: ButtonSize::ExtraSmall,
                enabled: true,
                icon: None,
                icon_kind: None,
                visual_override: None,
                group_position: ButtonGroupPosition::Single,
            },
            row_rect.width(),
        );
        if response.clicked() {
            state.toggle_channel(key);
        }
    }
}

fn build_physical_series(buffer: &TimelineBuffer) -> Vec<CurveSeries> {
    animation_curves::build_curve_series(
        buffer.frames().iter().map(|frame| {
            (
                frame.presentation_time_secs,
                frame.motion_channels.as_slice(),
            )
        }),
        CurveSignal::Physical,
    )
}

fn filtered_channel_keys(
    buffer: &TimelineBuffer,
    physical_series: &[CurveSeries],
    mode: TimelineDisplayMode,
    normalized_filter: &str,
) -> Vec<String> {
    let normalized_filter = normalized_filter.to_lowercase();
    let keys = match mode {
        TimelineDisplayMode::Keyframes => buffer.tracked_keys.iter().cloned().collect(),
        TimelineDisplayMode::Curve => physical_series
            .iter()
            .map(|series| series.channel_key.clone())
            .collect::<BTreeSet<_>>(),
    };
    keys.into_iter()
        .filter(|key| {
            normalized_filter.is_empty() || key.to_lowercase().contains(&normalized_filter)
        })
        .collect()
}

pub(crate) fn natural_height(buffer: &TimelineBuffer, state: &TimelinePanelState) -> f32 {
    let normalized_filter = state.filter.trim().to_lowercase();
    let row_count = match state.mode {
        TimelineDisplayMode::Keyframes => buffer
            .tracked_keys
            .iter()
            .filter(|key| {
                normalized_filter.is_empty() || key.to_lowercase().contains(&normalized_filter)
            })
            .count()
            .max(1),
        TimelineDisplayMode::Curve => buffer
            .frames()
            .iter()
            .flat_map(|frame| frame.motion_channels.iter())
            .filter(|channel| {
                normalized_filter.is_empty()
                    || channel.key.to_lowercase().contains(&normalized_filter)
            })
            .map(|channel| channel.key.as_str())
            .collect::<BTreeSet<_>>()
            .len()
            .max(CURVE_MIN_ROWS),
    };
    HEADER_ROW_H + row_count as f32 * VALUE_ROW_H + 16.0
}

fn update_time_axis_view(
    ui: &egui::Ui,
    pointer_in_grid: Option<egui::Pos2>,
    grid_min_x: f32,
    grid_viewport_w: f32,
    start_time: f64,
    end_time: f64,
    view: &mut TimeAxisViewState,
) {
    view.pixels_per_second = view
        .pixels_per_second
        .clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND);
    let initial_content_w = timeline_content_width(
        start_time,
        end_time,
        view.pixels_per_second,
        grid_viewport_w,
    );
    let initial_max_scroll = (initial_content_w - grid_viewport_w).max(0.0);
    if !view.initialized {
        view.scroll_x = initial_max_scroll;
        view.initialized = true;
    }
    view.scroll_x = view.scroll_x.clamp(0.0, initial_max_scroll);

    let mut user_changed_view = false;
    if let Some(pointer) = pointer_in_grid {
        let (scroll_delta, zoom_factor, shift_pressed) = ui.ctx().input(|input| {
            (
                input.smooth_scroll_delta,
                input.zoom_delta(),
                input.modifiers.shift,
            )
        });
        match animation_curves::x_axis_gesture(scroll_delta, zoom_factor, shift_pressed) {
            XAxisGesture::ZoomFactor(factor) => {
                let old_scale = view.pixels_per_second;
                view.pixels_per_second =
                    (old_scale * factor).clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND);
                let pointer_x = pointer.x - grid_min_x;
                view.scroll_x = zoom_scroll_around_pointer(
                    view.scroll_x,
                    pointer_x,
                    old_scale,
                    view.pixels_per_second,
                );
                user_changed_view = (view.pixels_per_second - old_scale).abs() > f32::EPSILON;
            }
            XAxisGesture::Pan(delta) => {
                view.scroll_x -= delta;
                user_changed_view = true;
            }
            XAxisGesture::None => {}
        }
    }

    let content_w = timeline_content_width(
        start_time,
        end_time,
        view.pixels_per_second,
        grid_viewport_w,
    );
    let max_scroll = (content_w - grid_viewport_w).max(0.0);
    (view.scroll_x, view.auto_follow) = animation_curves::resolve_auto_follow_scroll(
        view.scroll_x,
        max_scroll,
        view.auto_follow,
        user_changed_view,
    );
}

pub(crate) fn timeline_content_width(
    start_time: f64,
    end_time: f64,
    pixels_per_second: f32,
    viewport_width: f32,
) -> f32 {
    let duration = (end_time - start_time).max(0.0) as f32;
    (duration * pixels_per_second + CONTENT_EDGE_PAD * 2.0).max(viewport_width)
}

pub(crate) fn zoom_scroll_around_pointer(
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

pub(crate) fn pointer_time(
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

pub(crate) fn tick_step(pixels_per_second: f32) -> f64 {
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
    hidden_channels: &HashSet<String>,
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
        if hidden_channels.contains(&tracked_keys[row_index]) {
            continue;
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

#[allow(clippy::too_many_arguments)]
fn paint_curve_view(
    painter: &egui::Painter,
    buffer: &TimelineBuffer,
    series: &[CurveSeries],
    display_keys: &[String],
    hidden_channels: &HashSet<String>,
    plot_rect: egui::Rect,
    grid_origin_x: f32,
    start_time: f64,
    pixels_per_second: f32,
) {
    for step in 0..=4 {
        let normalized = step as f32 / 4.0;
        let y = egui::lerp(plot_rect.bottom()..=plot_rect.top(), normalized);
        painter.line_segment(
            [
                egui::pos2(plot_rect.left(), y),
                egui::pos2(plot_rect.right(), y),
            ],
            egui::Stroke::new(
                0.5_f32,
                design_tokens::white(if step == 0 || step == 4 { 24 } else { 14 }),
            ),
        );
        if step % 2 == 0 {
            painter.text(
                egui::pos2(plot_rect.left() + 4.0, y),
                egui::Align2::LEFT_CENTER,
                format!("{normalized:.1}"),
                egui::FontId::monospace(8.0),
                design_tokens::white(50),
            );
        }
    }

    let display_keys = display_keys
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let markers =
        animation_curves::build_state_markers(buffer.frames().iter().map(|frame| StateFrameRef {
            time_secs: frame.presentation_time_secs,
            current_state_id: &frame.current_state_id,
            active_transition_id: frame.active_transition_id.as_deref(),
            transition_source_name: frame.transition_source_name.as_deref(),
            transition_target_name: frame.transition_target_name.as_deref(),
        }));
    animation_curves::paint_state_markers(
        painter,
        plot_rect,
        grid_origin_x,
        CONTENT_EDGE_PAD,
        start_time,
        pixels_per_second,
        &markers,
        true,
    );

    let mut visible_count = 0;
    for curve in series {
        if !display_keys.contains(curve.channel_key.as_str())
            || hidden_channels.contains(&curve.channel_key)
        {
            continue;
        }
        visible_count += 1;
        animation_curves::paint_series(
            painter,
            plot_rect,
            grid_origin_x,
            CONTENT_EDGE_PAD,
            start_time,
            pixels_per_second,
            curve,
        );
    }
    if visible_count == 0 {
        painter.text(
            plot_rect.center(),
            egui::Align2::CENTER_CENTER,
            "No visible curves",
            egui::FontId::proportional(11.0),
            design_tokens::white(45),
        );
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
    use std::collections::{BTreeMap, HashMap};

    use super::*;
    use crate::{animation::TimelineFrameRecord, state_machine::MotionChannelDebug};

    fn record(channels: Vec<MotionChannelDebug>) -> TimelineFrameRecord {
        TimelineFrameRecord {
            scene_time_secs: 0.0,
            current_state_id: "state".into(),
            active_transition_id: None,
            motion_channels: channels,
            transition_source_name: None,
            transition_target_name: None,
            state_local_times: BTreeMap::new(),
            diagnostics: Vec::new(),
            active_overrides: HashMap::new(),
        }
    }

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

    #[test]
    fn panel_defaults_to_keyframes_with_channels_visible() {
        let state = TimelinePanelState::default();
        assert_eq!(state.mode, TimelineDisplayMode::Keyframes);
        assert!(state.channel_visible("new:channel"));
    }

    #[test]
    fn filter_matches_keyframes_case_insensitively_without_changing_visibility() {
        let buffer = TimelineBuffer::new(10.0, vec!["Alpha:value".into(), "beta:value".into()]);
        let mut state = TimelinePanelState::default();
        state.toggle_channel("Alpha:value");

        let keys = filtered_channel_keys(&buffer, &[], TimelineDisplayMode::Keyframes, "ALPHA");
        assert_eq!(keys, vec!["Alpha:value"]);
        assert!(!state.channel_visible("Alpha:value"));

        let all_keys = filtered_channel_keys(&buffer, &[], TimelineDisplayMode::Keyframes, "");
        assert_eq!(all_keys.len(), 2);
        assert!(!state.channel_visible("Alpha:value"));
    }

    #[test]
    fn vector_components_share_one_curve_channel_title() {
        let mut buffer = TimelineBuffer::new(10.0, Vec::new());
        buffer.push(record(vec![MotionChannelDebug {
            key: "node:position".into(),
            value: vec![1.0, 2.0, 3.0],
            ..Default::default()
        }]));
        let series = build_physical_series(&buffer);
        assert_eq!(series.len(), 3);
        assert!(
            series
                .iter()
                .all(|curve| curve.channel_key == "node:position")
        );
        assert_eq!(
            filtered_channel_keys(&buffer, &series, TimelineDisplayMode::Curve, ""),
            vec!["node:position"]
        );
    }

    #[test]
    fn mode_switch_preserves_shared_time_view() {
        let mut state = TimelinePanelState::default();
        state.time_view.pixels_per_second = 900.0;
        state.time_view.scroll_x = 123.0;
        state.time_view.auto_follow = false;
        state.mode = TimelineDisplayMode::Curve;

        assert_eq!(state.time_view.pixels_per_second, 900.0);
        assert_eq!(state.time_view.scroll_x, 123.0);
        assert!(!state.time_view.auto_follow);
    }

    #[test]
    fn new_recording_resets_filter_visibility_mode_and_time_view() {
        let first = TimelineBuffer::new(10.0, Vec::new());
        let second = TimelineBuffer::new(10.0, Vec::new());
        let mut state = TimelinePanelState::default();
        state.sync_recording(&first);
        state.mode = TimelineDisplayMode::Curve;
        state.filter = "position".into();
        state.toggle_channel("node:position");
        state.time_view.scroll_x = 100.0;
        state.time_view.initialized = true;

        state.sync_recording(&second);
        assert_eq!(state.mode, TimelineDisplayMode::Keyframes);
        assert!(state.filter.is_empty());
        assert!(state.hidden_channels.is_empty());
        assert_eq!(state.time_view.scroll_x, 0.0);
        assert!(!state.time_view.initialized);
        assert!(state.time_view.auto_follow);
    }
}
