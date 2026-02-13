use std::hash::Hash;

use rust_wgpu_fiber::eframe::egui;

use crate::ui::design_tokens;

pub const VALUE_SLIDER_HEIGHT: f32 = 18.0;
const INDICATOR_WIDTH: f32 = 2.0;
const INDICATOR_HEIGHT: f32 = 12.0;

pub struct ValueSliderOutput {
    pub response: egui::Response,
    pub changed: bool,
    pub formatted_value: String,
}

pub fn clamp_to_range(value: f32, min: f32, max: f32) -> f32 {
    if min > max {
        return value;
    }
    value.clamp(min, max)
}

pub fn normalized_value(value: f32, min: f32, max: f32) -> f32 {
    if (max - min).abs() <= f32::EPSILON {
        0.0
    } else {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    }
}

pub fn value_from_normalized(t: f32, min: f32, max: f32) -> f32 {
    min + (max - min) * t.clamp(0.0, 1.0)
}

pub fn value_slider(
    ui: &mut egui::Ui,
    id_source: impl Hash,
    value: &mut f32,
    min: f32,
    max: f32,
    formatter: Option<&dyn Fn(f32) -> String>,
) -> ValueSliderOutput {
    let desired_size = egui::vec2(ui.available_width(), VALUE_SLIDER_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let id = ui.make_persistent_id(id_source);
    let mut response = ui.interact(rect, id, egui::Sense::click_and_drag());
    response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    let mut changed = false;
    if (response.clicked() || response.dragged())
        && let Some(pointer) = ui.ctx().pointer_latest_pos()
    {
        let t = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let next = clamp_to_range(value_from_normalized(t, min, max), min, max);
        if (*value - next).abs() > f32::EPSILON {
            *value = next;
            changed = true;
        }
    }

    let t = normalized_value(*value, min, max);
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        design_tokens::radius(4),
        design_tokens::white(15),
        egui::Stroke::new(design_tokens::LINE_THICKNESS_05, design_tokens::white(20)),
        egui::StrokeKind::Inside,
    );

    let indicator_x = rect.left() + rect.width() * t;
    let indicator_rect = egui::Rect::from_center_size(
        egui::pos2(indicator_x, rect.center().y),
        egui::vec2(INDICATOR_WIDTH, INDICATOR_HEIGHT),
    );
    painter.rect_filled(
        indicator_rect,
        design_tokens::radius(2),
        design_tokens::white(90),
    );

    let formatted_value = formatter
        .map(|f| f(*value))
        .unwrap_or_else(|| format!("{:.3}", *value));

    ValueSliderOutput {
        response,
        changed,
        formatted_value,
    }
}

#[cfg(test)]
mod tests {
    use super::{clamp_to_range, normalized_value, value_from_normalized};

    #[test]
    fn clamp_to_range_clamps_values() {
        assert_eq!(clamp_to_range(-1.0, 0.0, 1.0), 0.0);
        assert_eq!(clamp_to_range(2.0, 0.0, 1.0), 1.0);
        assert_eq!(clamp_to_range(0.4, 0.0, 1.0), 0.4);
    }

    #[test]
    fn normalized_round_trip_is_stable() {
        let v = 0.2;
        let n = normalized_value(v, 0.0, 1.0);
        let back = value_from_normalized(n, 0.0, 1.0);
        assert!((back - v).abs() < 1e-6);
    }
}
