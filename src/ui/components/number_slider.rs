use std::hash::Hash;

use rust_wgpu_fiber::eframe::egui;

use crate::ui::{components::value_slider, design_tokens};

const SLIDER_VALUE_GAP: f32 = 0.0;
const VALUE_LABEL_TEXT_PADDING_X: f32 = 4.0;
const VALUE_LABEL_DIVIDER_WIDTH: f32 = 1.0;

#[derive(Clone, Copy)]
pub struct NumberSliderConfig<'a> {
    pub formatter: Option<&'a dyn Fn(f32) -> String>,
    pub divider_color: egui::Color32,
    pub value_label_width: Option<f32>,
}

impl<'a> NumberSliderConfig<'a> {
    pub fn new(divider_color: egui::Color32) -> Self {
        Self {
            formatter: None,
            divider_color,
            value_label_width: None,
        }
    }

    pub fn formatter(mut self, formatter: Option<&'a dyn Fn(f32) -> String>) -> Self {
        self.formatter = formatter;
        self
    }

    pub fn value_label_width(mut self, width: f32) -> Self {
        self.value_label_width = Some(width);
        self
    }
}

pub fn value_label_width_for(ui: &egui::Ui, sample: &str) -> f32 {
    let text_style = design_tokens::text_style(design_tokens::TextRole::ValueLabel);
    let label_font = design_tokens::font_id(text_style.size, text_style.weight);
    let text_width = ui
        .painter()
        .layout_no_wrap(sample.to_string(), label_font, text_style.color)
        .size()
        .x
        .ceil();
    text_width + VALUE_LABEL_TEXT_PADDING_X * 2.0 + VALUE_LABEL_DIVIDER_WIDTH
}

pub fn default_value_label_width(ui: &egui::Ui) -> f32 {
    value_label_width_for(ui, "100%")
}

pub fn slider_with_value(
    ui: &mut egui::Ui,
    id_source: impl Hash,
    value: &mut f32,
    min: f32,
    max: f32,
    config: NumberSliderConfig<'_>,
) -> bool {
    let mut changed = false;
    let mut formatted_value = config
        .formatter
        .map(|f| f(*value))
        .unwrap_or_else(|| format!("{:.3}", *value));
    let label_width = config
        .value_label_width
        .unwrap_or_else(|| default_value_label_width(ui));
    let slider_width = (ui.available_width() - SLIDER_VALUE_GAP - label_width).max(0.0);
    let text_style = design_tokens::text_style(design_tokens::TextRole::ValueLabel);
    let label_font = design_tokens::font_id(text_style.size, text_style.weight);

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), design_tokens::CONTROL_ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = SLIDER_VALUE_GAP;
            ui.allocate_ui_with_layout(
                egui::vec2(slider_width, design_tokens::CONTROL_ROW_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(slider_width, value_slider::VALUE_SLIDER_HEIGHT),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let out = value_slider::value_slider(
                                ui,
                                id_source,
                                value,
                                min,
                                max,
                                config.formatter,
                            );
                            changed = out.changed;
                            formatted_value = out.formatted_value;
                        },
                    );
                },
            );
            draw_static_value_label(
                ui,
                label_width,
                formatted_value.as_str(),
                text_style.color,
                &label_font,
                config.divider_color,
            );
        },
    );

    changed
}

pub fn slider_with_editable_value(
    ui: &mut egui::Ui,
    id_source: impl Hash + Clone,
    value: &mut f32,
    min: f32,
    max: f32,
    step: f32,
    config: NumberSliderConfig<'_>,
) -> bool {
    let mut changed = false;
    let mut formatted_value = config
        .formatter
        .map(|f| f(*value))
        .unwrap_or_else(|| format!("{:.3}", *value));
    let label_width = config
        .value_label_width
        .unwrap_or_else(|| default_value_label_width(ui));
    let slider_width = (ui.available_width() - SLIDER_VALUE_GAP - label_width).max(0.0);
    let text_style = design_tokens::text_style(design_tokens::TextRole::ValueLabel);
    let label_font = design_tokens::font_id(text_style.size, text_style.weight);
    let text_id = ui.make_persistent_id((id_source.clone(), "editable_value"));

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), design_tokens::CONTROL_ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = SLIDER_VALUE_GAP;
            ui.allocate_ui_with_layout(
                egui::vec2(slider_width, design_tokens::CONTROL_ROW_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(slider_width, value_slider::VALUE_SLIDER_HEIGHT),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let out = value_slider::value_slider(
                                ui,
                                id_source.clone(),
                                value,
                                min,
                                max,
                                config.formatter,
                            );
                            if out.changed {
                                let next = quantize_value(*value, min, max, step);
                                if (*value - next).abs() > f32::EPSILON {
                                    *value = next;
                                }
                                changed = true;
                            }
                            formatted_value = config
                                .formatter
                                .map(|f| f(*value))
                                .unwrap_or_else(|| out.formatted_value);
                        },
                    );
                },
            );
            if let Some(next) = show_editable_value_label(
                ui,
                text_id,
                label_width,
                formatted_value.as_str(),
                *value,
                min,
                max,
                step,
                text_style.color,
                &label_font,
                config.formatter,
                config.divider_color,
            ) {
                *value = next;
                changed = true;
            }
        },
    );

    changed
}

fn draw_static_value_label(
    ui: &mut egui::Ui,
    width: f32,
    value: &str,
    color: egui::Color32,
    label_font: &egui::FontId,
    divider_color: egui::Color32,
) {
    let (text_rect, _) =
        draw_value_label_frame(ui, width, egui::Sense::hover(), false, divider_color);
    let galley = ui
        .painter()
        .layout_no_wrap(value.to_owned(), label_font.clone(), color);
    let text_pos = egui::pos2(
        text_rect.center().x - galley.size().x * 0.5,
        text_rect.center().y - galley.size().y * 0.5 - 0.25,
    );
    ui.painter().galley(text_pos, galley, color);
}

#[allow(clippy::too_many_arguments)]
fn show_editable_value_label(
    ui: &mut egui::Ui,
    text_id: egui::Id,
    width: f32,
    formatted_value: &str,
    current_value: f32,
    min: f32,
    max: f32,
    step: f32,
    color: egui::Color32,
    label_font: &egui::FontId,
    formatter: Option<&dyn Fn(f32) -> String>,
    divider_color: egui::Color32,
) -> Option<f32> {
    let text_focused = ui.memory(|mem| mem.has_focus(text_id));
    let (text_rect, _) =
        draw_value_label_frame(ui, width, egui::Sense::click(), text_focused, divider_color);
    let mut text = ui
        .memory(|mem| mem.data.get_temp::<String>(text_id))
        .unwrap_or_else(|| formatted_value.to_string());
    if !text_focused {
        text = formatted_value.to_string();
    }

    let edit_response = ui
        .scope_builder(egui::UiBuilder::new().max_rect(text_rect), |ui| {
            ui.add_sized(
                text_rect.size(),
                egui::TextEdit::singleline(&mut text)
                    .id(text_id)
                    .font(label_font.clone())
                    .text_color(color)
                    .horizontal_align(egui::Align::Center)
                    .vertical_align(egui::Align::Center)
                    .desired_width(text_rect.width())
                    .margin(egui::Margin::same(0))
                    .frame(egui::Frame::NONE),
            )
        })
        .inner;

    let mut edited = None;
    if edit_response.changed()
        && let Ok(parsed) = text.trim().parse::<f32>()
    {
        let next = quantize_value(parsed, min, max, step);
        if (current_value - next).abs() > f32::EPSILON {
            edited = Some(next);
        }
    }
    if edit_response.lost_focus() && text.trim().parse::<f32>().is_err() {
        text = formatter
            .map(|f| f(current_value))
            .unwrap_or_else(|| format!("{:.3}", current_value));
    }
    ui.memory_mut(|mem| mem.data.insert_temp(text_id, text));
    edited
}

fn draw_value_label_frame(
    ui: &mut egui::Ui,
    width: f32,
    sense: egui::Sense,
    force_border: bool,
    divider_color: egui::Color32,
) -> (egui::Rect, egui::Response) {
    let (label_rect, label_response) =
        ui.allocate_exact_size(egui::vec2(width, value_slider::VALUE_SLIDER_HEIGHT), sense);
    let label_border_stroke = if label_response.hovered() || force_border {
        egui::Stroke::new(design_tokens::LINE_THICKNESS_05, design_tokens::white(20))
    } else {
        egui::Stroke::NONE
    };
    let painter = ui.painter_at(label_rect);
    painter.rect(
        label_rect,
        right_only_radius(design_tokens::BORDER_RADIUS_SMALL as u8),
        design_tokens::RESOURCE_ACTIVE_BG,
        label_border_stroke,
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [
            egui::pos2(label_rect.left(), label_rect.top()),
            egui::pos2(label_rect.left(), label_rect.bottom()),
        ],
        egui::Stroke::new(VALUE_LABEL_DIVIDER_WIDTH, divider_color),
    );

    let text_rect = egui::Rect::from_min_max(
        egui::pos2(
            label_rect.left() + VALUE_LABEL_DIVIDER_WIDTH + VALUE_LABEL_TEXT_PADDING_X,
            label_rect.top(),
        ),
        egui::pos2(
            label_rect.right() - VALUE_LABEL_TEXT_PADDING_X,
            label_rect.bottom(),
        ),
    );
    (text_rect, label_response)
}

fn right_only_radius(px: u8) -> egui::CornerRadius {
    let canonical = (px.clamp(2, 24) / 2) * 2;
    egui::CornerRadius {
        nw: 0,
        ne: canonical,
        sw: 0,
        se: canonical,
    }
}

fn quantize_value(value: f32, min: f32, max: f32, step: f32) -> f32 {
    let clamped = value_slider::clamp_to_range(value, min, max);
    if step > 0.0 && step.is_finite() {
        (clamped / step).round() * step
    } else {
        clamped
    }
    .clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::quantize_value;

    #[test]
    fn quantize_value_clamps_and_steps() {
        assert_eq!(quantize_value(1.26, 0.0, 2.0, 0.5), 1.5);
        assert_eq!(quantize_value(-1.0, 0.0, 2.0, 0.5), 0.0);
        assert_eq!(quantize_value(3.0, 0.0, 2.0, 0.5), 2.0);
    }
}
