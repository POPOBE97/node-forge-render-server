use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use rust_wgpu_fiber::eframe::egui;

use crate::{
    animation::{TimelineBuffer, TimelineFrameId},
    state_machine::MotionChannelDebug,
    ui::{
        animation_curves::{
            self, CurveSeries, CurveSignal, MAX_PIXELS_PER_SECOND, MIN_PIXELS_PER_SECOND,
            StateFrameRef, StateMarker, TimeAxisViewState as PlotViewState, XAxisGesture,
            resolve_auto_follow_scroll, x_axis_gesture,
        },
        button::{
            self, ButtonGroupPosition, ButtonOptions, ButtonSize, ButtonVariant,
            GroupButtonBehavior, GroupButtonOptions,
        },
        design_tokens, timeline_panel,
    },
};

const WINDOW_TITLE: &str = "Animation Curves";
const WINDOW_DEFAULT_SIZE: egui::Vec2 = egui::vec2(960.0, 540.0);
const WINDOW_MIN_SIZE: egui::Vec2 = egui::vec2(480.0, 320.0);
const SERIES_PANEL_WIDTH: f32 = 280.0;
const TOOLBAR_HEIGHT: f32 = 44.0;
const PLOT_LEFT_GUTTER: f32 = 44.0;
const PLOT_RIGHT_GUTTER: f32 = 8.0;
const PLOT_TOP_GUTTER: f32 = 26.0;
const PLOT_BOTTOM_GUTTER: f32 = 24.0;
const PLOT_SPLIT_GAP: f32 = 4.0;

#[derive(Debug, Clone)]
struct CurveFrameSnapshot {
    frame_id: TimelineFrameId,
    presentation_time_secs: f64,
    current_state_id: String,
    active_transition_id: Option<String>,
    transition_source_name: Option<String>,
    transition_target_name: Option<String>,
    motion_channels: Vec<MotionChannelDebug>,
}

#[derive(Debug, Clone, Default)]
struct CurveHistorySnapshot {
    source_recording_id: Option<u64>,
    frames: Vec<CurveFrameSnapshot>,
}

impl CurveHistorySnapshot {
    fn from_buffer(buffer: Option<&TimelineBuffer>) -> Self {
        let source_recording_id = buffer.map(TimelineBuffer::recording_id);
        let frames = buffer
            .into_iter()
            .flat_map(TimelineBuffer::frames)
            .map(|frame| CurveFrameSnapshot {
                frame_id: frame.id,
                presentation_time_secs: frame.presentation_time_secs,
                current_state_id: frame.current_state_id.clone(),
                active_transition_id: frame.active_transition_id.clone(),
                transition_source_name: frame.transition_source_name.clone(),
                transition_target_name: frame.transition_target_name.clone(),
                motion_channels: frame.motion_channels.clone(),
            })
            .collect();
        Self {
            source_recording_id,
            frames,
        }
    }

    fn time_range(&self) -> Option<(f64, f64)> {
        Some((
            self.frames.first()?.presentation_time_secs,
            self.frames.last()?.presentation_time_secs,
        ))
    }

    fn sync_from_buffer(&mut self, buffer: Option<&TimelineBuffer>) {
        let Some(buffer) = buffer else {
            *self = Self::default();
            return;
        };
        if self.source_recording_id != Some(buffer.recording_id()) {
            *self = Self::from_buffer(Some(buffer));
            return;
        }
        let Some(first_frame_id) = buffer.frames().front().map(|frame| frame.id) else {
            self.frames.clear();
            return;
        };

        self.frames.retain(|frame| frame.frame_id >= first_frame_id);
        let latest_cached_id = self.frames.last().map(|frame| frame.frame_id);
        self.frames.extend(
            buffer
                .frames()
                .iter()
                .filter(|frame| latest_cached_id.is_none_or(|id| frame.id > id))
                .map(|frame| CurveFrameSnapshot {
                    frame_id: frame.id,
                    presentation_time_secs: frame.presentation_time_secs,
                    current_state_id: frame.current_state_id.clone(),
                    active_transition_id: frame.active_transition_id.clone(),
                    transition_source_name: frame.transition_source_name.clone(),
                    transition_target_name: frame.transition_target_name.clone(),
                    motion_channels: frame.motion_channels.clone(),
                }),
        );
    }
}

fn build_state_markers(history: &CurveHistorySnapshot) -> Vec<StateMarker> {
    animation_curves::build_state_markers(history.frames.iter().map(|frame| StateFrameRef {
        time_secs: frame.presentation_time_secs,
        current_state_id: &frame.current_state_id,
        active_transition_id: frame.active_transition_id.as_deref(),
        transition_source_name: frame.transition_source_name.as_deref(),
        transition_target_name: frame.transition_target_name.as_deref(),
    }))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CurveStats {
    min_value: f64,
    max_value: f64,
    latest_value: f64,
}

impl From<&CurveSeries> for CurveStats {
    fn from(series: &CurveSeries) -> Self {
        Self {
            min_value: series.min_value,
            max_value: series.max_value,
            latest_value: series.latest_value,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct CurveLegendEntry {
    id: String,
    label: String,
    color: egui::Color32,
    physical: Option<CurveStats>,
    mutation_target: Option<CurveStats>,
}

fn build_curve_series(history: &CurveHistorySnapshot, signal: CurveSignal) -> Vec<CurveSeries> {
    animation_curves::build_curve_series(
        history.frames.iter().map(|frame| {
            (
                frame.presentation_time_secs,
                frame.motion_channels.as_slice(),
            )
        }),
        signal,
    )
}

fn build_curve_legend(
    physical_series: &[CurveSeries],
    mutation_target_series: &[CurveSeries],
) -> Vec<CurveLegendEntry> {
    let mut entries = BTreeMap::<String, CurveLegendEntry>::new();
    for series in physical_series {
        entries.insert(
            series.id.clone(),
            CurveLegendEntry {
                id: series.id.clone(),
                label: series.label.clone(),
                color: series.color,
                physical: Some(CurveStats::from(series)),
                mutation_target: None,
            },
        );
    }
    for series in mutation_target_series {
        let entry = entries
            .entry(series.id.clone())
            .or_insert_with(|| CurveLegendEntry {
                id: series.id.clone(),
                label: series.label.clone(),
                color: series.color,
                physical: None,
                mutation_target: None,
            });
        entry.mutation_target = Some(CurveStats::from(series));
    }
    entries.into_values().collect()
}

fn series_matches_filter(series: &CurveLegendEntry, normalized_filter: &str) -> bool {
    normalized_filter.is_empty() || series.label.to_lowercase().contains(normalized_filter)
}

fn set_filtered_series_visibility(
    series: &[CurveLegendEntry],
    normalized_filter: &str,
    hidden_series: &mut HashSet<String>,
    visible: bool,
) {
    for curve in series
        .iter()
        .filter(|curve| series_matches_filter(curve, normalized_filter))
    {
        if visible {
            hidden_series.remove(&curve.id);
        } else {
            hidden_series.insert(curve.id.clone());
        }
    }
}

#[derive(Debug, Default)]
struct AnimationCurveWindowDocument {
    filter: String,
    hidden_series: HashSet<String>,
    plot_view: PlotViewState,
}

pub struct AnimationCurveWindowState {
    viewport_id: egui::ViewportId,
    document: Arc<Mutex<AnimationCurveWindowDocument>>,
    history: Arc<Mutex<CurveHistorySnapshot>>,
    close_requested: Arc<AtomicBool>,
    viewport_initialized: bool,
    focus_requested: bool,
}

impl AnimationCurveWindowState {
    fn new() -> Self {
        Self {
            viewport_id: egui::ViewportId::from_hash_of("animation-curves"),
            document: Arc::new(Mutex::new(AnimationCurveWindowDocument::default())),
            history: Arc::new(Mutex::new(CurveHistorySnapshot::default())),
            close_requested: Arc::new(AtomicBool::new(false)),
            viewport_initialized: false,
            focus_requested: true,
        }
    }
}

pub fn is_animation_curve_window_open(window: &Option<AnimationCurveWindowState>) -> bool {
    window
        .as_ref()
        .is_some_and(|state| !state.close_requested.load(Ordering::Relaxed))
}

pub fn open_animation_curve_window(window: &mut Option<AnimationCurveWindowState>) {
    if let Some(state) = window.as_mut() {
        state.close_requested.store(false, Ordering::Relaxed);
        state.focus_requested = true;
    } else {
        *window = Some(AnimationCurveWindowState::new());
    }
}

fn discard_closed_window(window: &mut Option<AnimationCurveWindowState>) -> bool {
    let should_discard = window
        .as_ref()
        .is_some_and(|state| state.close_requested.load(Ordering::Relaxed));
    if should_discard {
        *window = None;
    }
    should_discard
}

pub fn show_animation_curve_window(
    ctx: &egui::Context,
    window: &mut Option<AnimationCurveWindowState>,
    timeline_buffer: Option<&TimelineBuffer>,
    animation_session_active: bool,
) {
    if discard_closed_window(window) {
        return;
    }
    let Some(state) = window.as_mut() else {
        return;
    };

    if let Ok(mut history) = state.history.lock() {
        history.sync_from_buffer(timeline_buffer);
    }
    let history = Arc::clone(&state.history);
    let document = Arc::clone(&state.document);
    let close_requested = Arc::clone(&state.close_requested);
    let viewport_builder = animation_curve_viewport_builder(!state.viewport_initialized);
    state.viewport_initialized = true;

    ctx.show_viewport_deferred(
        state.viewport_id,
        viewport_builder,
        move |ui, class| match class {
            egui::ViewportClass::EmbeddedWindow => {
                let mut open = true;
                egui::Window::new(WINDOW_TITLE)
                    .id(egui::Id::new("animation-curves-embedded"))
                    .open(&mut open)
                    .title_bar(false)
                    .default_size(WINDOW_DEFAULT_SIZE)
                    .show(ui.ctx(), |window_ui| {
                        render_animation_curve_window(window_ui, &document, &history);
                    });
                if !open {
                    close_requested.store(true, Ordering::Relaxed);
                }
                if animation_session_active {
                    ui.ctx().request_repaint();
                }
            }
            _ => {
                if ui.ctx().input(|input| input.viewport().close_requested()) {
                    close_requested.store(true, Ordering::Relaxed);
                    return;
                }
                render_animation_curve_window(ui, &document, &history);
                if animation_session_active {
                    ui.ctx().request_repaint();
                }
            }
        },
    );

    if animation_session_active {
        ctx.request_repaint_of(state.viewport_id);
    }

    if state.focus_requested {
        ctx.send_viewport_cmd_to(state.viewport_id, egui::ViewportCommand::Focus);
        state.focus_requested = false;
    }
}

fn animation_curve_viewport_builder(include_initial_size: bool) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title(WINDOW_TITLE)
        .with_min_inner_size(WINDOW_MIN_SIZE);
    if include_initial_size {
        builder.with_inner_size(WINDOW_DEFAULT_SIZE)
    } else {
        builder
    }
}

fn render_animation_curve_window(
    ui: &mut egui::Ui,
    document: &Arc<Mutex<AnimationCurveWindowDocument>>,
    history: &Arc<Mutex<CurveHistorySnapshot>>,
) {
    let Ok(mut document) = document.lock() else {
        ui.label("Animation curve state is unavailable");
        return;
    };

    egui::Panel::top("animation-curve-toolbar")
        .exact_size(TOOLBAR_HEIGHT)
        .frame(
            egui::Frame::NONE
                .fill(egui::Color32::from_gray(20))
                .inner_margin(egui::Margin::symmetric(10, 8)),
        )
        .show_inside(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(design_tokens::rich_text(
                    "P / Q",
                    design_tokens::TextRole::ActiveItemTitle,
                ));
                ui.label(design_tokens::rich_text(
                    "shared curve filter",
                    design_tokens::TextRole::AttributeTitle,
                ));
                ui.separator();
                ui.label("Search");
                ui.add(
                    egui::TextEdit::singleline(&mut document.filter)
                        .hint_text("channel or component")
                        .desired_width(220.0),
                );
            });
        });

    let (physical_series, mutation_target_series, state_markers, time_range, history_is_empty) =
        match history.lock() {
            Ok(history) => (
                build_curve_series(&history, CurveSignal::Physical),
                build_curve_series(&history, CurveSignal::MutationTarget),
                build_state_markers(&history),
                history.time_range(),
                history.frames.is_empty(),
            ),
            Err(_) => (Vec::new(), Vec::new(), Vec::new(), None, true),
        };
    let series = build_curve_legend(&physical_series, &mutation_target_series);
    let normalized_filter = document.filter.trim().to_lowercase();

    egui::Panel::left("animation-curve-series")
        .resizable(true)
        .default_size(SERIES_PANEL_WIDTH)
        .min_size(180.0)
        .max_size(420.0)
        .frame(
            egui::Frame::NONE
                .fill(egui::Color32::from_gray(16))
                .inner_margin(egui::Margin::same(8)),
        )
        .show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(design_tokens::rich_text(
                    format!("Curves ({})", series.len()),
                    design_tokens::TextRole::ActiveItemTitle,
                ));
                let visibility_buttons = button::group_button(
                    ui,
                    GroupButtonOptions {
                        primary: ButtonOptions {
                            label: "All",
                            tooltip: Some("Show all filtered curves in P and Q"),
                            variant: ButtonVariant::Secondary,
                            size: ButtonSize::ExtraSmall,
                            enabled: true,
                            icon: None,
                            icon_kind: None,
                            visual_override: None,
                            group_position: ButtonGroupPosition::Single,
                        },
                        secondary: Some(ButtonOptions {
                            label: "None",
                            tooltip: Some("Hide all filtered curves in P and Q"),
                            variant: ButtonVariant::Secondary,
                            size: ButtonSize::ExtraSmall,
                            enabled: true,
                            icon: None,
                            icon_kind: None,
                            visual_override: None,
                            group_position: ButtonGroupPosition::Single,
                        }),
                        behavior: GroupButtonBehavior::default(),
                    },
                );
                if visibility_buttons.primary.clicked() {
                    set_filtered_series_visibility(
                        &series,
                        &normalized_filter,
                        &mut document.hidden_series,
                        true,
                    );
                }
                if visibility_buttons
                    .secondary
                    .is_some_and(|response| response.clicked())
                {
                    set_filtered_series_visibility(
                        &series,
                        &normalized_filter,
                        &mut document.hidden_series,
                        false,
                    );
                }
            });
            ui.separator();

            if series.is_empty() {
                ui.label(design_tokens::rich_text(
                    "No sampled motion channels",
                    design_tokens::TextRole::InactiveItemTitle,
                ));
                return;
            }

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for curve in series
                        .iter()
                        .filter(|curve| series_matches_filter(curve, &normalized_filter))
                    {
                        let mut visible = !document.hidden_series.contains(&curve.id);
                        if ui
                            .checkbox(
                                &mut visible,
                                egui::RichText::new(&curve.label).color(curve.color),
                            )
                            .changed()
                        {
                            if visible {
                                document.hidden_series.remove(&curve.id);
                            } else {
                                document.hidden_series.insert(curve.id.clone());
                            }
                        }
                        if let Some(stats) = curve.physical {
                            show_curve_stats(ui, "P", stats);
                        }
                        if let Some(stats) = curve.mutation_target {
                            show_curve_stats(ui, "Q", stats);
                        }
                        ui.add_space(6.0);
                    }
                });
        });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(egui::Color32::BLACK)
                .inner_margin(egui::Margin::same(0)),
        )
        .show_inside(ui, |ui| {
            if history_is_empty {
                ui.centered_and_justified(|ui| {
                    ui.label(design_tokens::rich_text(
                        "Press Play or select a State to record",
                        design_tokens::TextRole::InactiveItemTitle,
                    ));
                });
                return;
            }

            let hidden_series = document.hidden_series.clone();
            let available = ui.available_size();
            let plot_height = ((available.y - PLOT_SPLIT_GAP) * 0.5).max(1.0);
            let previous_item_spacing_y = ui.spacing().item_spacing.y;
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.allocate_ui_with_layout(
                egui::vec2(available.x, plot_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    paint_curve_plot(
                        ui,
                        "P — Physical value",
                        time_range,
                        &physical_series,
                        &state_markers,
                        true,
                        &hidden_series,
                        &mut document.plot_view,
                    );
                },
            );
            ui.add_space(PLOT_SPLIT_GAP);
            ui.allocate_ui_with_layout(
                egui::vec2(available.x, plot_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    paint_curve_plot(
                        ui,
                        "Q — Mutation target",
                        time_range,
                        &mutation_target_series,
                        &state_markers,
                        false,
                        &hidden_series,
                        &mut document.plot_view,
                    );
                },
            );
            ui.spacing_mut().item_spacing.y = previous_item_spacing_y;
        });
}

fn show_curve_stats(ui: &mut egui::Ui, signal: &str, stats: CurveStats) {
    ui.label(
        egui::RichText::new(format!(
            "{signal}  latest {}   range {} … {}",
            format_raw_value(stats.latest_value),
            format_raw_value(stats.min_value),
            format_raw_value(stats.max_value),
        ))
        .small()
        .color(design_tokens::white(45)),
    );
}

fn format_raw_value(value: f64) -> String {
    let abs = value.abs();
    if abs != 0.0 && !(0.001..1_000.0).contains(&abs) {
        format!("{value:.3e}")
    } else {
        format!("{value:.4}")
    }
}

fn paint_curve_plot(
    ui: &mut egui::Ui,
    plot_label: &str,
    time_range: Option<(f64, f64)>,
    series: &[CurveSeries],
    state_markers: &[StateMarker],
    show_state_marker_labels: bool,
    hidden_series: &HashSet<String>,
    view: &mut PlotViewState,
) {
    let available = ui.available_size();
    let (outer_rect, response) = ui.allocate_exact_size(
        egui::vec2(available.x.max(1.0), available.y.max(1.0)),
        egui::Sense::hover(),
    );
    let plot_rect = egui::Rect::from_min_max(
        egui::pos2(
            outer_rect.min.x + PLOT_LEFT_GUTTER,
            outer_rect.min.y + PLOT_TOP_GUTTER,
        ),
        egui::pos2(
            (outer_rect.max.x - PLOT_RIGHT_GUTTER).max(outer_rect.min.x + PLOT_LEFT_GUTTER + 1.0),
            (outer_rect.max.y - PLOT_BOTTOM_GUTTER).max(outer_rect.min.y + PLOT_TOP_GUTTER + 1.0),
        ),
    );
    let Some((start_time, end_time)) = time_range else {
        return;
    };

    view.pixels_per_second = view
        .pixels_per_second
        .clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND);
    let initial_content_width = timeline_panel::timeline_content_width(
        start_time,
        end_time,
        view.pixels_per_second,
        plot_rect.width(),
    );
    let initial_max_scroll = (initial_content_width - plot_rect.width()).max(0.0);
    if !view.initialized {
        view.scroll_x = initial_max_scroll;
        view.initialized = true;
    }
    view.scroll_x = view.scroll_x.clamp(0.0, initial_max_scroll);

    let pointer_in_plot = ui
        .ctx()
        .input(|input| input.pointer.hover_pos())
        .filter(|pointer| outer_rect.contains(*pointer));
    let mut user_changed_view = false;
    if let Some(pointer) = pointer_in_plot {
        let (scroll_delta, zoom_factor, shift_pressed) = ui.ctx().input(|input| {
            (
                input.smooth_scroll_delta,
                input.zoom_delta(),
                input.modifiers.shift,
            )
        });
        match x_axis_gesture(scroll_delta, zoom_factor, shift_pressed) {
            XAxisGesture::ZoomFactor(factor) => {
                let old_scale = view.pixels_per_second;
                view.pixels_per_second =
                    (old_scale * factor).clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND);
                let pointer_x =
                    pointer.x.clamp(plot_rect.left(), plot_rect.right()) - plot_rect.min.x;
                view.scroll_x = timeline_panel::zoom_scroll_around_pointer(
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

    let content_width = timeline_panel::timeline_content_width(
        start_time,
        end_time,
        view.pixels_per_second,
        plot_rect.width(),
    );
    let max_scroll = (content_width - plot_rect.width()).max(0.0);
    (view.scroll_x, view.auto_follow) = resolve_auto_follow_scroll(
        view.scroll_x,
        max_scroll,
        view.auto_follow,
        user_changed_view,
    );

    let outer_painter = ui.painter_at(outer_rect);
    outer_painter.rect_filled(outer_rect, egui::CornerRadius::ZERO, egui::Color32::BLACK);
    outer_painter.text(
        egui::pos2(outer_rect.left() + PLOT_LEFT_GUTTER, outer_rect.top() + 5.0),
        egui::Align2::LEFT_TOP,
        plot_label,
        egui::FontId::proportional(11.0),
        design_tokens::white(75),
    );
    outer_painter.rect_filled(
        plot_rect,
        egui::CornerRadius::ZERO,
        egui::Color32::from_gray(9),
    );
    outer_painter.rect_stroke(
        plot_rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0_f32, design_tokens::white(18)),
        egui::StrokeKind::Inside,
    );

    paint_normalized_grid(&outer_painter, plot_rect);
    let grid_origin_x = plot_rect.min.x - view.scroll_x;
    paint_time_grid(
        &outer_painter,
        plot_rect,
        grid_origin_x,
        start_time,
        end_time,
        view.pixels_per_second,
    );

    let curve_painter = ui.painter_at(plot_rect);
    animation_curves::paint_state_markers(
        &curve_painter,
        plot_rect,
        grid_origin_x,
        timeline_panel::CONTENT_EDGE_PAD,
        start_time,
        view.pixels_per_second,
        state_markers,
        show_state_marker_labels,
    );
    let mut visible_count = 0;
    for curve in series {
        if hidden_series.contains(&curve.id) {
            continue;
        }
        visible_count += 1;
        animation_curves::paint_series(
            &curve_painter,
            plot_rect,
            grid_origin_x,
            timeline_panel::CONTENT_EDGE_PAD,
            start_time,
            view.pixels_per_second,
            curve,
        );
    }

    if visible_count == 0 {
        outer_painter.text(
            plot_rect.center(),
            egui::Align2::CENTER_CENTER,
            "No visible curves",
            egui::FontId::proportional(12.0),
            design_tokens::white(45),
        );
    }

    if response.hovered() {
        response.on_hover_text("⌘ + wheel: zoom X · ⇧ + wheel: pan X");
    }
}

fn paint_normalized_grid(painter: &egui::Painter, plot_rect: egui::Rect) {
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
        painter.text(
            egui::pos2(plot_rect.left() - 6.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{normalized:.2}"),
            egui::FontId::monospace(9.0),
            design_tokens::white(55),
        );
    }
}

fn paint_time_grid(
    painter: &egui::Painter,
    plot_rect: egui::Rect,
    grid_origin_x: f32,
    start_time: f64,
    end_time: f64,
    pixels_per_second: f32,
) {
    if (end_time - start_time).abs() < f64::EPSILON {
        let x = grid_origin_x + timeline_panel::CONTENT_EDGE_PAD;
        painter.text(
            egui::pos2(x, plot_rect.bottom() + 5.0),
            egui::Align2::CENTER_TOP,
            format_time_tick(start_time),
            egui::FontId::monospace(9.0),
            design_tokens::white(55),
        );
        return;
    }

    let visible_start = timeline_panel::pointer_time(
        plot_rect.left(),
        grid_origin_x,
        start_time,
        end_time,
        pixels_per_second,
    );
    let visible_end = timeline_panel::pointer_time(
        plot_rect.right(),
        grid_origin_x,
        start_time,
        end_time,
        pixels_per_second,
    );
    let step = timeline_panel::tick_step(pixels_per_second);
    let mut tick = (visible_start / step).ceil() * step;
    while tick <= visible_end + step * 0.001 {
        let x = grid_origin_x
            + timeline_panel::CONTENT_EDGE_PAD
            + ((tick - start_time) as f32 * pixels_per_second);
        painter.line_segment(
            [
                egui::pos2(x, plot_rect.top()),
                egui::pos2(x, plot_rect.bottom()),
            ],
            egui::Stroke::new(0.5_f32, design_tokens::white(14)),
        );
        painter.text(
            egui::pos2(x, plot_rect.bottom() + 5.0),
            egui::Align2::CENTER_TOP,
            format_time_tick(tick),
            egui::FontId::monospace(9.0),
            design_tokens::white(55),
        );
        tick += step;
    }
}

fn format_time_tick(time_secs: f64) -> String {
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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use super::*;
    use crate::animation::TimelineFrameRecord;
    use crate::ui::animation_curves::{
        AUTO_FOLLOW_THRESHOLD_PX, normalize_value, stable_series_color,
    };

    fn channel(key: &str, values: [f64; 5]) -> MotionChannelDebug {
        MotionChannelDebug {
            key: key.into(),
            value: vec![values[0]],
            velocity: vec![values[1]],
            state_value: vec![values[2]],
            target_value: vec![values[3]],
            transition_error: vec![values[4]],
            ..Default::default()
        }
    }

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

    fn snapshot(
        frame_id: u64,
        time_secs: f64,
        state_id: &str,
        transition: Option<(&str, &str, &str)>,
        motion_channels: Vec<MotionChannelDebug>,
    ) -> CurveFrameSnapshot {
        CurveFrameSnapshot {
            frame_id: TimelineFrameId(frame_id),
            presentation_time_secs: time_secs,
            current_state_id: state_id.into(),
            active_transition_id: transition.map(|(id, _, _)| id.into()),
            transition_source_name: transition.map(|(_, source, _)| source.into()),
            transition_target_name: transition.map(|(_, _, target)| target.into()),
            motion_channels,
        }
    }

    fn history_with_values(values: &[f64]) -> CurveHistorySnapshot {
        let mut buffer = TimelineBuffer::new(10.0, Vec::new());
        for value in values {
            buffer.advance_time(0.1);
            buffer.push(record(vec![channel(
                "node:value",
                [
                    *value,
                    *value + 10.0,
                    *value + 20.0,
                    *value + 30.0,
                    *value + 40.0,
                ],
            )]));
        }
        CurveHistorySnapshot::from_buffer(Some(&buffer))
    }

    #[test]
    fn signal_mapping_reads_p_v_s_q_and_e() {
        let history = history_with_values(&[1.0]);
        let expected = [
            (CurveSignal::Physical, 1.0),
            (CurveSignal::Velocity, 11.0),
            (CurveSignal::State, 21.0),
            (CurveSignal::MutationTarget, 31.0),
            (CurveSignal::TransitionError, 41.0),
        ];
        for (signal, raw_value) in expected {
            let series = build_curve_series(&history, signal);
            assert_eq!(series.len(), 1);
            assert_eq!(series[0].samples[0].raw_value, raw_value);
        }
    }

    #[test]
    fn vector_components_expand_to_stable_series_ids() {
        let mut vector = channel("node:position", [0.0; 5]);
        vector.value = vec![1.0, 2.0, 3.0];
        let history = CurveHistorySnapshot {
            source_recording_id: None,
            frames: vec![snapshot(0, 0.0, "state", None, vec![vector])],
        };
        let ids = build_curve_series(&history, CurveSignal::Physical)
            .into_iter()
            .map(|series| series.id)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                "node:position[0]".to_string(),
                "node:position[1]".to_string(),
                "node:position[2]".to_string(),
            ]
        );
    }

    #[test]
    fn normalizes_each_series_independently_and_centers_constants() {
        let history = history_with_values(&[-2.0, 0.0, 2.0]);
        let series = build_curve_series(&history, CurveSignal::Physical);
        assert_eq!(series[0].min_value, -2.0);
        assert_eq!(series[0].max_value, 2.0);
        assert_eq!(
            series[0]
                .samples
                .iter()
                .map(|sample| sample.normalized_value)
                .collect::<Vec<_>>(),
            vec![0.0, 0.5, 1.0]
        );
        assert_eq!(normalize_value(4.0, 4.0, 4.0), 0.5);
    }

    #[test]
    fn non_finite_values_are_skipped_and_leave_frame_gaps() {
        let history = history_with_values(&[1.0, f64::NAN, 3.0]);
        let series = build_curve_series(&history, CurveSignal::Physical);
        assert_eq!(series[0].samples.len(), 2);
        assert_eq!(series[0].samples[0].frame_index, 0);
        assert_eq!(series[0].samples[1].frame_index, 2);
    }

    #[test]
    fn rolling_buffer_eviction_changes_normalization_domain() {
        let mut buffer = TimelineBuffer::new(0.15, Vec::new());
        for value in [0.0, 10.0, 20.0] {
            buffer.advance_time(0.1);
            buffer.push(record(vec![channel("node:value", [value; 5])]));
        }
        let history = CurveHistorySnapshot::from_buffer(Some(&buffer));
        let series = build_curve_series(&history, CurveSignal::Physical);
        assert_eq!(series[0].min_value, 10.0);
        assert_eq!(series[0].max_value, 20.0);
        assert_eq!(series[0].samples[0].normalized_value, 0.0);
        assert_eq!(series[0].samples[1].normalized_value, 1.0);
    }

    #[test]
    fn history_snapshot_syncs_incrementally_and_resets_with_recording() {
        let mut buffer = TimelineBuffer::new(0.15, Vec::new());
        buffer.advance_time(0.1);
        buffer.push(record(vec![channel("node:value", [1.0; 5])]));
        let mut history = CurveHistorySnapshot::from_buffer(Some(&buffer));
        assert_eq!(history.frames.len(), 1);

        buffer.advance_time(0.1);
        buffer.push(record(vec![channel("node:value", [2.0; 5])]));
        buffer.advance_time(0.1);
        buffer.push(record(vec![channel("node:value", [3.0; 5])]));
        history.sync_from_buffer(Some(&buffer));
        assert_eq!(history.frames.len(), 2);
        assert_eq!(history.frames[0].presentation_time_secs, 0.2);

        buffer.clear();
        history.sync_from_buffer(Some(&buffer));
        assert!(history.frames.is_empty());

        let mut replacement = TimelineBuffer::new(10.0, Vec::new());
        replacement.push(record(vec![channel("replacement:value", [4.0; 5])]));
        history.sync_from_buffer(Some(&replacement));
        assert_eq!(history.frames.len(), 1);
        assert_eq!(
            history.frames[0].motion_channels[0].key,
            "replacement:value"
        );
    }

    #[test]
    fn stable_color_only_depends_on_series_identity() {
        assert_eq!(
            stable_series_color("node:value[0]"),
            stable_series_color("node:value[0]")
        );
    }

    #[test]
    fn gesture_uses_egui_zoom_factor_and_requires_shift_for_pan() {
        assert_eq!(
            x_axis_gesture(egui::vec2(0.0, 12.0), 1.0, false),
            XAxisGesture::None
        );
        assert_eq!(
            x_axis_gesture(egui::Vec2::ZERO, 1.1, false),
            XAxisGesture::ZoomFactor(1.1)
        );
        assert_eq!(
            x_axis_gesture(egui::vec2(3.0, 12.0), 1.0, true),
            XAxisGesture::Pan(12.0)
        );
        assert_eq!(
            x_axis_gesture(egui::vec2(20.0, 3.0), 1.0, true),
            XAxisGesture::Pan(20.0)
        );
        assert_eq!(
            x_axis_gesture(egui::vec2(20.0, 3.0), 0.9, true),
            XAxisGesture::ZoomFactor(0.9)
        );
    }

    #[test]
    fn egui_converts_command_wheel_to_zoom_delta() {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            modifiers: egui::Modifiers::COMMAND,
            events: vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 20.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::COMMAND,
            }],
            ..Default::default()
        });
        let (scroll_delta, zoom_factor) =
            ctx.input(|input| (input.smooth_scroll_delta, input.zoom_delta()));
        let _ = ctx.end_pass();

        assert_eq!(scroll_delta, egui::Vec2::ZERO);
        assert!(zoom_factor > 1.0);
        assert_eq!(
            x_axis_gesture(scroll_delta, zoom_factor, false),
            XAxisGesture::ZoomFactor(zoom_factor)
        );
    }

    #[test]
    fn auto_follow_stops_outside_threshold_and_resumes_inside_it() {
        assert_eq!(
            resolve_auto_follow_scroll(40.0, 100.0, true, true),
            (40.0, false)
        );
        assert_eq!(
            resolve_auto_follow_scroll(100.0 - AUTO_FOLLOW_THRESHOLD_PX, 100.0, false, true,),
            (100.0, true)
        );
        assert_eq!(
            resolve_auto_follow_scroll(10.0, 140.0, true, false),
            (140.0, true)
        );
    }

    #[test]
    fn state_markers_label_transition_starts_and_instant_state_changes() {
        let history = CurveHistorySnapshot {
            source_recording_id: None,
            frames: vec![
                snapshot(0, 0.0, "idle", None, Vec::new()),
                snapshot(
                    1,
                    0.1,
                    "idle",
                    Some(("idle-to-run", "Idle", "Run")),
                    Vec::new(),
                ),
                snapshot(
                    2,
                    0.2,
                    "run",
                    Some(("idle-to-run", "Idle", "Run")),
                    Vec::new(),
                ),
                snapshot(3, 0.3, "run", None, Vec::new()),
                snapshot(4, 0.4, "jump", None, Vec::new()),
            ],
        };

        assert_eq!(
            build_state_markers(&history),
            vec![
                StateMarker {
                    time_secs: 0.1,
                    label: "Idle → Run".into(),
                },
                StateMarker {
                    time_secs: 0.4,
                    label: "jump".into(),
                },
            ]
        );
    }

    #[test]
    fn open_focus_close_and_reopen_reset_document_state() {
        let mut window = None;
        open_animation_curve_window(&mut window);
        let first_document = Arc::clone(&window.as_ref().unwrap().document);
        window.as_mut().unwrap().focus_requested = false;

        open_animation_curve_window(&mut window);
        assert!(window.as_ref().unwrap().focus_requested);
        assert!(Arc::ptr_eq(
            &first_document,
            &window.as_ref().unwrap().document
        ));

        window
            .as_ref()
            .unwrap()
            .close_requested
            .store(true, Ordering::Relaxed);
        assert!(discard_closed_window(&mut window));
        open_animation_curve_window(&mut window);
        assert!(!Arc::ptr_eq(
            &first_document,
            &window.as_ref().unwrap().document
        ));
    }

    #[test]
    fn hidden_set_defaults_new_series_to_visible() {
        let mut document = AnimationCurveWindowDocument::default();
        assert!(!document.hidden_series.contains("new:channel[0]"));
        document.hidden_series.insert("old:channel[0]".into());
        assert!(!document.hidden_series.contains("new:channel[0]"));
    }

    #[test]
    fn bulk_visibility_only_changes_filtered_series() {
        let history = CurveHistorySnapshot {
            source_recording_id: None,
            frames: vec![snapshot(
                0,
                0.0,
                "state",
                None,
                vec![
                    channel("alpha:value", [1.0; 5]),
                    channel("beta:value", [2.0; 5]),
                ],
            )],
        };
        let physical_series = build_curve_series(&history, CurveSignal::Physical);
        let mutation_target_series = build_curve_series(&history, CurveSignal::MutationTarget);
        let series = build_curve_legend(&physical_series, &mutation_target_series);
        let mut hidden = HashSet::new();

        set_filtered_series_visibility(&series, "alpha", &mut hidden, false);
        assert!(hidden.contains("alpha:value[0]"));
        assert!(!hidden.contains("beta:value[0]"));

        set_filtered_series_visibility(&series, "alpha", &mut hidden, true);
        assert!(hidden.is_empty());
    }

    #[test]
    fn shared_legend_combines_physical_and_mutation_target_stats() {
        let history = history_with_values(&[1.0, 3.0]);
        let physical_series = build_curve_series(&history, CurveSignal::Physical);
        let mutation_target_series = build_curve_series(&history, CurveSignal::MutationTarget);
        let legend = build_curve_legend(&physical_series, &mutation_target_series);

        assert_eq!(legend.len(), 1);
        assert_eq!(legend[0].id, "node:value[0]");
        assert_eq!(legend[0].physical.unwrap().latest_value, 3.0);
        assert_eq!(legend[0].mutation_target.unwrap().latest_value, 33.0);
    }

    #[test]
    fn shared_legend_includes_components_present_only_in_q() {
        let mut vector = channel("node:position", [0.0; 5]);
        vector.value = vec![1.0];
        vector.target_value = vec![2.0, 3.0];
        let history = CurveHistorySnapshot {
            source_recording_id: None,
            frames: vec![snapshot(0, 0.0, "state", None, vec![vector])],
        };
        let physical_series = build_curve_series(&history, CurveSignal::Physical);
        let mutation_target_series = build_curve_series(&history, CurveSignal::MutationTarget);
        let legend = build_curve_legend(&physical_series, &mutation_target_series);

        assert_eq!(legend.len(), 2);
        assert!(legend[1].physical.is_none());
        assert_eq!(legend[1].id, "node:position[1]");
        assert_eq!(legend[1].mutation_target.unwrap().latest_value, 3.0);
    }
}
