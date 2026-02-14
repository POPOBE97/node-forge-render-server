use std::hash::Hash;

use rust_wgpu_fiber::eframe::egui::{self, Margin};

use crate::ui::{design_tokens, typography};

const GROUP_HEIGHT: f32 = design_tokens::CONTROL_ROW_HEIGHT;
const GROUP_PADDING: i8 = 3;
const ITEM_GAP: f32 = 2.0;
const ITEM_RADIUS: u8 = 4;
const ITEM_TEXT_SIZE: f32 = design_tokens::FONT_SIZE_13;

#[derive(Clone, Copy, Debug)]
pub struct RadioButtonOption<'a, T> {
    pub value: T,
    pub label: &'a str,
}

pub fn apply_selection<T: Copy + Eq>(selected: &mut T, next: T) -> bool {
    if *selected == next {
        return false;
    }
    *selected = next;
    true
}

pub fn radio_button_group<'a, T: Copy + Eq>(
    ui: &mut egui::Ui,
    id_source: impl Hash,
    selected: &mut T,
    options: &[RadioButtonOption<'a, T>],
) -> bool {
    if options.is_empty() {
        return false;
    }

    let container_color = design_tokens::RESOURCE_ACTIVE_BG;
    let active_bg = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 204);
    let inactive_text = egui::Color32::from_rgb(0xAA, 0xAA, 0xAA);
    let active_text = egui::Color32::WHITE;

    let mut changed = false;
    let available_w = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(available_w, GROUP_HEIGHT),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            let id = ui.make_persistent_id(id_source);
            egui::Frame::new()
                .fill(container_color)
                .stroke(egui::Stroke::NONE)
                .corner_radius(egui::CornerRadius::same(ITEM_RADIUS))
                .inner_margin(Margin::same(GROUP_PADDING))
                .show(ui, |ui| {
                    let available_w = ui.available_width();
                    let item_h = GROUP_HEIGHT - (GROUP_PADDING as f32 * 2.0);
                    let item_w = if options.len() > 1 {
                        (available_w - ITEM_GAP * ((options.len() - 1) as f32))
                            / options.len() as f32
                    } else {
                        available_w
                    }
                    .max(0.0);

                    ui.spacing_mut().item_spacing.x = ITEM_GAP;

                    ui.push_id(id, |ui| {
                        ui.horizontal_top(|ui| {
                            for option in options {
                                let is_selected = *selected == option.value;
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(item_w, item_h),
                                    egui::Sense::click(),
                                );
                                let response =
                                    response.on_hover_cursor(egui::CursorIcon::PointingHand);

                                if ui.is_rect_visible(rect) {
                                    ui.painter().rect_filled(
                                        rect,
                                        egui::CornerRadius::same(ITEM_RADIUS),
                                        if is_selected {
                                            active_bg
                                        } else {
                                            egui::Color32::TRANSPARENT
                                        },
                                    );

                                    let font = egui::FontId::new(
                                        ITEM_TEXT_SIZE,
                                        typography::mi_sans_family_for_weight(if is_selected {
                                            500.0
                                        } else {
                                            400.0
                                        }),
                                    );
                                    let color = if is_selected {
                                        active_text
                                    } else {
                                        inactive_text
                                    };

                                    let text_rect = rect.shrink2(egui::vec2(2.0, 2.0));
                                    let galley = ui.painter().layout_no_wrap(
                                        option.label.to_owned(),
                                        font,
                                        color,
                                    );
                                    let text_pos = egui::pos2(
                                        text_rect.center().x - galley.size().x * 0.5,
                                        text_rect.center().y - galley.size().y * 0.5 - 0.25,
                                    );
                                    ui.painter().galley(
                                        text_pos,
                                        galley,
                                        egui::Color32::PLACEHOLDER,
                                    );
                                }

                                if response.clicked() {
                                    changed |= apply_selection(selected, option.value);
                                }
                            }
                        });
                    });
                });
        },
    );

    changed
}

#[cfg(test)]
mod tests {
    use super::apply_selection;

    #[test]
    fn apply_selection_changes_on_new_value() {
        let mut selected = 1;
        assert!(apply_selection(&mut selected, 2));
        assert_eq!(selected, 2);
    }

    #[test]
    fn apply_selection_noop_on_same_value() {
        let mut selected = 2;
        assert!(!apply_selection(&mut selected, 2));
        assert_eq!(selected, 2);
    }
}
