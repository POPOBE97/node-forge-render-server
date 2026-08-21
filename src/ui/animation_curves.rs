//! Shared animation-curve sampling, normalization, painting, and X-axis state.

use std::collections::BTreeMap;

use rust_wgpu_fiber::eframe::egui;

use crate::state_machine::MotionChannelDebug;

pub(crate) const DEFAULT_PIXELS_PER_SECOND: f32 = 600.0;
pub(crate) const MIN_PIXELS_PER_SECOND: f32 = 60.0;
pub(crate) const MAX_PIXELS_PER_SECOND: f32 = 2_400.0;
pub(crate) const AUTO_FOLLOW_THRESHOLD_PX: f32 = 32.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum CurveSignal {
    #[default]
    Physical,
    Velocity,
    State,
    MutationTarget,
    TransitionError,
}

impl CurveSignal {
    fn values(self, channel: &MotionChannelDebug) -> &[f64] {
        match self {
            Self::Physical => &channel.value,
            Self::Velocity => &channel.velocity,
            Self::State => &channel.state_value,
            Self::MutationTarget => &channel.target_value,
            Self::TransitionError => &channel.transition_error,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CurveSample {
    pub frame_index: usize,
    pub time_secs: f64,
    pub raw_value: f64,
    pub normalized_value: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CurveSeries {
    pub id: String,
    pub channel_key: String,
    pub label: String,
    pub color: egui::Color32,
    pub min_value: f64,
    pub max_value: f64,
    pub latest_value: f64,
    pub samples: Vec<CurveSample>,
}

#[derive(Debug, Default)]
struct CurveSeriesBuilder {
    channel_key: String,
    samples: Vec<(usize, f64, f64)>,
}

pub(crate) fn series_id(channel_key: &str, component_index: usize) -> String {
    format!("{channel_key}[{component_index}]")
}

pub(crate) fn build_curve_series<'a>(
    frames: impl IntoIterator<Item = (f64, &'a [MotionChannelDebug])>,
    signal: CurveSignal,
) -> Vec<CurveSeries> {
    let mut builders: BTreeMap<String, CurveSeriesBuilder> = BTreeMap::new();

    for (frame_index, (time_secs, channels)) in frames.into_iter().enumerate() {
        for channel in channels {
            for (component_index, value) in signal.values(channel).iter().copied().enumerate() {
                let id = series_id(&channel.key, component_index);
                let builder = builders.entry(id).or_insert_with(|| CurveSeriesBuilder {
                    channel_key: channel.key.clone(),
                    samples: Vec::new(),
                });
                if value.is_finite() {
                    builder.samples.push((frame_index, time_secs, value));
                }
            }
        }
    }

    builders
        .into_iter()
        .filter_map(|(id, builder)| {
            let min_value = builder
                .samples
                .iter()
                .map(|(_, _, value)| *value)
                .reduce(f64::min)?;
            let max_value = builder
                .samples
                .iter()
                .map(|(_, _, value)| *value)
                .reduce(f64::max)?;
            let latest_value = builder.samples.last()?.2;
            let samples = builder
                .samples
                .into_iter()
                .map(|(frame_index, time_secs, raw_value)| CurveSample {
                    frame_index,
                    time_secs,
                    raw_value,
                    normalized_value: normalize_value(raw_value, min_value, max_value),
                })
                .collect();
            Some(CurveSeries {
                channel_key: builder.channel_key,
                label: id.clone(),
                color: stable_series_color(&id),
                id,
                min_value,
                max_value,
                latest_value,
                samples,
            })
        })
        .collect()
}

pub(crate) fn normalize_value(value: f64, min_value: f64, max_value: f64) -> f32 {
    let span = max_value - min_value;
    if !value.is_finite() || !span.is_finite() {
        return f32::NAN;
    }
    if span.abs() <= f64::EPSILON {
        0.5
    } else {
        ((value - min_value) / span).clamp(0.0, 1.0) as f32
    }
}

pub(crate) fn stable_series_color(id: &str) -> egui::Color32 {
    const COLORS: [[u8; 3]; 12] = [
        [78, 161, 255],
        [255, 111, 97],
        [88, 214, 141],
        [255, 205, 86],
        [177, 134, 255],
        [72, 209, 204],
        [255, 146, 76],
        [238, 105, 165],
        [123, 201, 111],
        [100, 181, 246],
        [210, 168, 255],
        [244, 214, 92],
    ];
    let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    let [r, g, b] = COLORS[hash as usize % COLORS.len()];
    egui::Color32::from_rgb(r, g, b)
}

#[derive(Clone, Copy)]
pub(crate) struct StateFrameRef<'a> {
    pub time_secs: f64,
    pub current_state_id: &'a str,
    pub active_transition_id: Option<&'a str>,
    pub transition_source_name: Option<&'a str>,
    pub transition_target_name: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StateMarker {
    pub time_secs: f64,
    pub label: String,
}

pub(crate) fn build_state_markers<'a>(
    frames: impl IntoIterator<Item = StateFrameRef<'a>>,
) -> Vec<StateMarker> {
    let mut markers = Vec::new();
    let mut previous = None;
    for current in frames {
        let Some(previous_frame) = previous else {
            previous = Some(current);
            continue;
        };
        let transition_started = current.active_transition_id.is_some()
            && current.active_transition_id != previous_frame.active_transition_id;
        if transition_started {
            let label = match (
                current.transition_source_name,
                current.transition_target_name,
            ) {
                (Some(source), Some(target)) => format!("{source} → {target}"),
                (_, Some(target)) => target.to_string(),
                _ => current.current_state_id.to_string(),
            };
            markers.push(StateMarker {
                time_secs: current.time_secs,
                label,
            });
        } else if current.current_state_id != previous_frame.current_state_id
            && previous_frame.active_transition_id.is_none()
            && current.active_transition_id.is_none()
        {
            markers.push(StateMarker {
                time_secs: current.time_secs,
                label: current.current_state_id.to_string(),
            });
        }
        previous = Some(current);
    }
    markers
}

#[derive(Debug)]
pub(crate) struct TimeAxisViewState {
    pub pixels_per_second: f32,
    pub scroll_x: f32,
    pub initialized: bool,
    pub auto_follow: bool,
}

impl Default for TimeAxisViewState {
    fn default() -> Self {
        Self {
            pixels_per_second: DEFAULT_PIXELS_PER_SECOND,
            scroll_x: 0.0,
            initialized: false,
            auto_follow: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum XAxisGesture {
    None,
    ZoomFactor(f32),
    Pan(f32),
}

pub(crate) fn x_axis_gesture(
    scroll_delta: egui::Vec2,
    zoom_factor: f32,
    shift_pressed: bool,
) -> XAxisGesture {
    if zoom_factor.is_finite() && zoom_factor > 0.0 && (zoom_factor - 1.0).abs() > 0.0001 {
        return XAxisGesture::ZoomFactor(zoom_factor);
    }
    if shift_pressed {
        let horizontal_delta = if scroll_delta.x.abs() >= scroll_delta.y.abs() {
            scroll_delta.x
        } else {
            scroll_delta.y
        };
        if horizontal_delta.abs() > 0.5 {
            return XAxisGesture::Pan(horizontal_delta);
        }
    }
    XAxisGesture::None
}

pub(crate) fn resolve_auto_follow_scroll(
    scroll_x: f32,
    max_scroll: f32,
    was_auto_following: bool,
    user_changed_view: bool,
) -> (f32, bool) {
    let scroll_x = scroll_x.clamp(0.0, max_scroll);
    if was_auto_following && !user_changed_view {
        return (max_scroll, true);
    }
    let auto_follow = max_scroll - scroll_x <= AUTO_FOLLOW_THRESHOLD_PX;
    if auto_follow {
        (max_scroll, true)
    } else {
        (scroll_x, false)
    }
}

pub(crate) fn paint_series(
    painter: &egui::Painter,
    plot_rect: egui::Rect,
    grid_origin_x: f32,
    content_edge_pad: f32,
    start_time: f64,
    pixels_per_second: f32,
    series: &CurveSeries,
) {
    let mut segment = Vec::new();
    let mut previous_frame_index = None;
    for sample in &series.samples {
        let is_contiguous =
            previous_frame_index.is_none_or(|previous| sample.frame_index == previous + 1);
        if !is_contiguous {
            paint_segment(painter, &segment, series.color);
            segment.clear();
        }
        let x = grid_origin_x
            + content_edge_pad
            + ((sample.time_secs - start_time) as f32 * pixels_per_second);
        let y = egui::lerp(
            plot_rect.bottom()..=plot_rect.top(),
            sample.normalized_value,
        );
        segment.push(egui::pos2(x, y));
        previous_frame_index = Some(sample.frame_index);
    }
    paint_segment(painter, &segment, series.color);
}

fn paint_segment(painter: &egui::Painter, points: &[egui::Pos2], color: egui::Color32) {
    match points {
        [] => {}
        [point] => {
            painter.circle_filled(*point, 1.8, color);
        }
        _ => {
            painter.add(egui::Shape::line(
                points.to_vec(),
                egui::Stroke::new(1.5_f32, color),
            ));
        }
    }
}

pub(crate) fn paint_state_markers(
    painter: &egui::Painter,
    plot_rect: egui::Rect,
    grid_origin_x: f32,
    content_edge_pad: f32,
    start_time: f64,
    pixels_per_second: f32,
    markers: &[StateMarker],
    show_labels: bool,
) {
    let line_color = egui::Color32::from_rgb(255, 166, 64);
    let text_color = egui::Color32::from_rgb(255, 219, 168);
    for (index, marker) in markers.iter().enumerate() {
        let x = grid_origin_x
            + content_edge_pad
            + ((marker.time_secs - start_time) as f32 * pixels_per_second);
        if x < plot_rect.left() || x > plot_rect.right() {
            continue;
        }
        painter.line_segment(
            [
                egui::pos2(x, plot_rect.top()),
                egui::pos2(x, plot_rect.bottom()),
            ],
            egui::Stroke::new(1.0_f32, line_color),
        );
        if !show_labels {
            continue;
        }

        let galley = painter.layout_no_wrap(
            marker.label.clone(),
            egui::FontId::proportional(10.0),
            text_color,
        );
        let minimum_x = plot_rect.left() + 4.0;
        let maximum_x = (plot_rect.right() - galley.size().x - 8.0).max(minimum_x);
        let label_x = (x + 4.0).clamp(minimum_x, maximum_x);
        let label_y = plot_rect.top() + 5.0 + (index % 3) as f32 * 18.0;
        let label_pos = egui::pos2(label_x, label_y);
        let background =
            egui::Rect::from_min_size(label_pos, galley.size()).expand2(egui::vec2(4.0, 2.0));
        painter.rect_filled(
            background,
            egui::CornerRadius::same(3),
            egui::Color32::from_rgba_unmultiplied(58, 34, 13, 230),
        );
        painter.galley(label_pos, galley, egui::Color32::PLACEHOLDER);
    }
}
