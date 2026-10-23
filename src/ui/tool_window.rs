//! Shared rhythm for auxiliary workspaces. Apply to a local Ui only: the main
//! render window deliberately keeps its own layout and context-wide style.
use rust_wgpu_fiber::eframe::egui;

use super::{
    button::{self, ButtonOptions, ButtonSize, ButtonVariant},
    design_tokens::{self, FontWeight, TextRole},
};

pub(crate) const INSET: i8 = 12;
pub(crate) const GAP: f32 = 8.0;
pub(crate) const ROW_HEIGHT: f32 = 32.0;
pub(crate) const SECTION_GAP: f32 = 20.0;
pub(crate) const PANEL: egui::Color32 = egui::Color32::from_gray(23);
pub(crate) const CANVAS: egui::Color32 = egui::Color32::from_gray(16);

pub(crate) fn apply(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.spacing.item_spacing = egui::vec2(GAP, 4.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = design_tokens::CONTROL_ROW_HEIGHT;
    style.spacing.icon_width = 18.0;
    style.spacing.icon_spacing = 10.0;
    style.spacing.indent = 18.0;
    style.visuals.panel_fill = PANEL;
    for (role, size, weight) in [
        (egui::TextStyle::Body, 13.0, FontWeight::Normal),
        (egui::TextStyle::Button, 13.0, FontWeight::Medium),
        (egui::TextStyle::Small, 11.0, FontWeight::Normal),
        (egui::TextStyle::Heading, 15.0, FontWeight::Medium),
    ] {
        style
            .text_styles
            .insert(role, design_tokens::font_id(size, weight));
    }
}

pub(crate) fn panel_frame() -> egui::Frame {
    egui::Frame::NONE.fill(PANEL).inner_margin(INSET)
}

pub(crate) fn toolbar_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(PANEL)
        .inner_margin(egui::Margin::symmetric(INSET, 8))
}

pub(crate) fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::window(&ctx.global_style())
        .inner_margin(INSET)
        .corner_radius(8)
}

pub(crate) fn heading(ui: &mut egui::Ui, title: &str) {
    ui.label(design_tokens::rich_text(title, TextRole::SectionTitle));
}

pub(crate) fn caption(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(design_tokens::rich_text(text, TextRole::AttributeTitle));
}

pub(crate) fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(SECTION_GAP - ui.spacing().item_spacing.y);
    heading(ui, title);
    ui.add_space(4.0);
}

pub(crate) fn action(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    button::button(
        ui,
        ButtonOptions {
            size: ButtonSize::Small,
            variant: ButtonVariant::Secondary,
            enabled,
            ..ButtonOptions::new(label)
        },
    )
}

pub(crate) fn badge(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    egui::Frame::NONE
        .fill(design_tokens::white(10))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(design_tokens::rich_text(text, TextRole::AttributeTitle))
        })
        .inner
}

pub(crate) fn empty_state(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            heading(ui, title);
            ui.add_space(4.0);
            caption(ui, detail);
        });
    });
}
