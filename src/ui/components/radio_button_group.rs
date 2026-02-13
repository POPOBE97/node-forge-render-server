use std::hash::Hash;

use rust_wgpu_fiber::eframe::egui::{self, Margin};

use crate::ui::typography;

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

    let container_color = egui::Color32::from_rgb(0x30, 0x30, 0x30);
    let active_bg = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 204);
    let inactive_text = egui::Color32::from_rgb(0xAA, 0xAA, 0xAA);
    let active_text = egui::Color32::WHITE;

    let mut changed = false;
    let id = ui.make_persistent_id(id_source);
    egui::Frame::new()
        .fill(container_color)
        .stroke(egui::Stroke::new(1.0, container_color))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(Margin::same(1))
        .show(ui, |ui| {
            let available_w = ui.available_width();
            let item_gap = 2.0;
            let item_h = 15.0;
            let item_w = if options.len() > 1 {
                (available_w - item_gap * ((options.len() - 1) as f32)) / options.len() as f32
            } else {
                available_w
            }
            .max(0.0);

            ui.spacing_mut().item_spacing.x = item_gap;
            ui.spacing_mut().button_padding = egui::vec2(2.0, 0.0);

            ui.push_id(id, |ui| {
                ui.horizontal(|ui| {
                    for option in options {
                        let is_selected = *selected == option.value;
                        let text = egui::RichText::new(option.label)
                            .font(egui::FontId::new(
                                10.0,
                                typography::mi_sans_family_for_weight(if is_selected {
                                    500.0
                                } else {
                                    400.0
                                }),
                            ))
                            .color(if is_selected {
                                active_text
                            } else {
                                inactive_text
                            });

                        let button = egui::Button::new(text)
                            .fill(if is_selected {
                                active_bg
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .corner_radius(egui::CornerRadius::same(4))
                            .stroke(egui::Stroke::NONE)
                            .min_size(egui::vec2(item_w, item_h));

                        if ui.add_sized([item_w, item_h], button).clicked() {
                            changed |= apply_selection(selected, option.value);
                        }
                    }
                });
            });
        });

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
