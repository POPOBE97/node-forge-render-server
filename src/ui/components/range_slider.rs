use std::hash::Hash;

use rust_wgpu_fiber::eframe::egui;

use crate::ui::design_tokens;

use super::value_slider::{
    VALUE_SLIDER_HEIGHT, clamp_to_range, normalized_value, value_from_normalized,
};

const EDGE_INSET_X: f32 = 8.0;
const EDGE_INSET_Y: f32 = 6.0;
const INDICATOR_WIDTH: f32 = 2.0;
const INDICATOR_HEIGHT: f32 = VALUE_SLIDER_HEIGHT - EDGE_INSET_Y * 2.0;
const FILL_HEIGHT: f32 = INDICATOR_HEIGHT;
const SLIDER_RADIUS: u8 = design_tokens::BORDER_RADIUS_SMALL as u8;
const HANDLE_GRAB_HALF_WIDTH: f32 = 8.0;

fn left_only_radius(px: u8) -> egui::CornerRadius {
    let canonical = (px.clamp(2, 24) / 2) * 2;
    egui::CornerRadius {
        nw: canonical,
        ne: 0,
        sw: canonical,
        se: 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragHandle {
    Min,
    Max,
}

pub struct RangeSliderOutput {
    pub response: egui::Response,
    pub changed: bool,
    pub formatted_min: String,
    pub formatted_max: String,
}

/// Two-thumb range slider. Visually mirrors `value_slider`:
/// rounded track on the left, value label area expected on the right.
pub fn range_slider(
    ui: &mut egui::Ui,
    id_source: impl Hash,
    min_value: &mut f32,
    max_value: &mut f32,
    domain_min: f32,
    domain_max: f32,
    formatter: Option<&dyn Fn(f32) -> String>,
) -> RangeSliderOutput {
    let desired_size = egui::vec2(ui.available_width(), VALUE_SLIDER_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let track_rect = rect.shrink2(egui::vec2(EDGE_INSET_X, EDGE_INSET_Y));
    let id = ui.make_persistent_id(id_source);
    let mut response = ui.interact(rect, id, egui::Sense::click_and_drag());
    response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    // Make sure min <= max coming in.
    if *min_value > *max_value {
        std::mem::swap(min_value, max_value);
    }
    let mut lo = clamp_to_range(*min_value, domain_min, domain_max);
    let mut hi = clamp_to_range(*max_value, domain_min, domain_max);

    let drag_state_id = id.with("drag_handle");
    let mut active_handle: Option<DragHandle> =
        ui.ctx().memory(|mem| mem.data.get_temp(drag_state_id));

    let mut changed = false;

    if response.drag_stopped() || !response.dragged() {
        if !response.dragged() && active_handle.is_some() {
            // Clear stale drag state when interaction is no longer active.
            ui.ctx().memory_mut(|mem| {
                mem.data.remove::<Option<DragHandle>>(drag_state_id);
            });
            active_handle = None;
        }
    }

    if (response.clicked() || response.drag_started() || response.dragged())
        && let Some(pointer) = ui.ctx().pointer_latest_pos()
    {
        let width = track_rect.width().max(f32::EPSILON);
        let t = ((pointer.x - track_rect.left()) / width).clamp(0.0, 1.0);
        let pointer_value = value_from_normalized(t, domain_min, domain_max);

        if active_handle.is_none() && (response.clicked() || response.drag_started()) {
            // Decide which handle to grab — prefer whichever is closer to pointer.
            let lo_t = normalized_value(lo, domain_min, domain_max);
            let hi_t = normalized_value(hi, domain_min, domain_max);
            let lo_x = track_rect.left() + width * lo_t;
            let hi_x = track_rect.left() + width * hi_t;
            let dist_lo = (pointer.x - lo_x).abs();
            let dist_hi = (pointer.x - hi_x).abs();
            let handle = if dist_lo <= dist_hi {
                DragHandle::Min
            } else {
                DragHandle::Max
            };
            active_handle = Some(handle);
            ui.ctx().memory_mut(|mem| {
                mem.data.insert_temp(drag_state_id, handle);
            });
        }

        if let Some(handle) = active_handle {
            match handle {
                DragHandle::Min => {
                    let next = clamp_to_range(pointer_value, domain_min, hi);
                    if (lo - next).abs() > f32::EPSILON {
                        lo = next;
                        changed = true;
                    }
                }
                DragHandle::Max => {
                    let next = clamp_to_range(pointer_value, lo, domain_max);
                    if (hi - next).abs() > f32::EPSILON {
                        hi = next;
                        changed = true;
                    }
                }
            }
        }
    }

    if changed {
        *min_value = lo;
        *max_value = hi;
    }

    if response.drag_stopped() {
        ui.ctx().memory_mut(|mem| {
            mem.data.remove::<Option<DragHandle>>(drag_state_id);
        });
    }

    let painter = ui.painter_at(rect);
    let border_stroke = if response.hovered() || response.dragged() {
        egui::Stroke::new(design_tokens::LINE_THICKNESS_05, design_tokens::white(20))
    } else {
        egui::Stroke::NONE
    };
    painter.rect(
        rect,
        left_only_radius(SLIDER_RADIUS),
        design_tokens::RESOURCE_ACTIVE_BG,
        border_stroke,
        egui::StrokeKind::Inside,
    );

    let lo_t = normalized_value(lo, domain_min, domain_max);
    let hi_t = normalized_value(hi, domain_min, domain_max);
    let lo_x = track_rect.left() + track_rect.width() * lo_t;
    let hi_x = track_rect.left() + track_rect.width() * hi_t;
    let center_y = rect.center().y;

    // Filled segment between the two handles.
    if hi_x > lo_x {
        let fill_rect = egui::Rect::from_min_max(
            egui::pos2(lo_x, center_y - FILL_HEIGHT * 0.5),
            egui::pos2(hi_x, center_y + FILL_HEIGHT * 0.5),
        );
        painter.rect_filled(
            fill_rect,
            design_tokens::radius(2),
            design_tokens::white(40),
        );
    }

    // Two indicator pills.
    for x in [lo_x, hi_x] {
        let indicator_rect = egui::Rect::from_center_size(
            egui::pos2(x, center_y),
            egui::vec2(INDICATOR_WIDTH, INDICATOR_HEIGHT),
        );
        painter.rect_filled(
            indicator_rect,
            design_tokens::radius(2),
            design_tokens::white(90),
        );
    }

    let _ = HANDLE_GRAB_HALF_WIDTH; // reserved for future hit-test refinement
    let formatted_min = formatter
        .map(|f| f(lo))
        .unwrap_or_else(|| format!("{:.3}", lo));
    let formatted_max = formatter
        .map(|f| f(hi))
        .unwrap_or_else(|| format!("{:.3}", hi));

    RangeSliderOutput {
        response,
        changed,
        formatted_min,
        formatted_max,
    }
}

#[cfg(test)]
mod tests {
    use super::super::value_slider::{clamp_to_range, normalized_value, value_from_normalized};

    #[test]
    fn min_clamped_below_max() {
        // Simulate dragging the min handle to a value above max.
        let hi = 0.6_f32;
        let target = 0.9_f32;
        let lo = clamp_to_range(target, 0.0, hi);
        assert!(lo <= hi);
        assert_eq!(lo, hi);
    }

    #[test]
    fn max_clamped_above_min() {
        let lo = 0.4_f32;
        let target = 0.1_f32;
        let hi = clamp_to_range(target, lo, 1.0);
        assert!(hi >= lo);
        assert_eq!(hi, lo);
    }

    #[test]
    fn normalized_round_trip_is_stable() {
        let v = 0.42_f32;
        let n = normalized_value(v, 0.0, 1.0);
        let back = value_from_normalized(n, 0.0, 1.0);
        assert!((back - v).abs() < 1e-6);
    }
}
