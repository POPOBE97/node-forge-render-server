use rust_wgpu_fiber::eframe::egui::{self, Margin};

use crate::ui::design_tokens::{self, TextRole};

const PANEL_BG: egui::Color32 = egui::Color32::from_rgb(0x17, 0x17, 0x17);
const SECTION_TO_PANEL_GAP: f32 = 12.0;
const COLUMN_GAP: f32 = 8.0;
const LABEL_TO_CONTROL_GAP: f32 = 4.0;

pub fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    section_with_header_action(ui, title, |_| {}, body);
}

pub fn section_with_header_action(
    ui: &mut egui::Ui,
    title: &str,
    header_action: impl FnOnce(&mut egui::Ui),
    body: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.label(design_tokens::rich_text(title, TextRole::SectionTitle));
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            header_action,
        );
    });
    ui.add_space(SECTION_TO_PANEL_GAP);
    egui::Frame::new()
        .fill(PANEL_BG)
        .inner_margin(Margin::same(0))
        .show(ui, body);
}

pub fn row(ui: &mut egui::Ui, left: impl FnOnce(&mut egui::Ui), right: impl FnOnce(&mut egui::Ui)) {
    let full_width = ui.available_width();
    let col_width = ((full_width - COLUMN_GAP).max(0.0)) * 0.5;
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(col_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            left,
        );
        ui.add_space(COLUMN_GAP);
        ui.allocate_ui_with_layout(
            egui::vec2(col_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            right,
        );
    });
}

pub fn cell(ui: &mut egui::Ui, label: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.label(design_tokens::rich_text(label, TextRole::AttributeTitle));
    ui.add_space(LABEL_TO_CONTROL_GAP);
    body(ui);
}

pub fn empty_cell(ui: &mut egui::Ui) {
    ui.add_space(1.0);
}
