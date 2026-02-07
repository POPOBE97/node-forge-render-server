use rust_wgpu_fiber::eframe::egui::{self, Color32};

#[derive(Clone, Copy, Debug)]
pub enum TailwindButtonVariant {
    Destructive,
    Connected,
    Idle,
}

#[derive(Clone, Copy, Debug)]
pub enum TailwindButtonGroupPosition {
    Single,
    First,
    Middle,
    Last,
}

#[derive(Clone, Copy, Debug)]
struct TailwindButtonVisuals {
    bg: Color32,
    hover_bg: Color32,
    text: Color32,
}

fn tailwind_button_visuals(variant: TailwindButtonVariant) -> TailwindButtonVisuals {
    match variant {
        TailwindButtonVariant::Destructive => TailwindButtonVisuals {
            bg: Color32::from_rgb(0xEF, 0x44, 0x44),
            hover_bg: Color32::from_rgba_unmultiplied(0xEF, 0x44, 0x44, 230),
            text: Color32::WHITE,
        },
        TailwindButtonVariant::Connected => TailwindButtonVisuals {
            bg: Color32::from_rgba_unmultiplied(0x59, 0x8C, 0x5C, 77),
            hover_bg: Color32::from_rgba_unmultiplied(0x59, 0x8C, 0x5C, 77),
            text: Color32::from_rgb(0x63, 0xC7, 0x63),
        },
        TailwindButtonVariant::Idle => TailwindButtonVisuals {
            bg: Color32::from_rgb(48, 48, 48),
            hover_bg: Color32::from_rgb(0x40, 0x40, 0x40),
            text: Color32::from_rgb(0xE6, 0xE6, 0xE6),
        },
    }
}

pub fn tailwind_button(
    ui: &mut egui::Ui,
    label: &str,
    title: &str,
    variant: TailwindButtonVariant,
    group_position: TailwindButtonGroupPosition,
    enabled: bool,
) -> egui::Response {
    let visuals = tailwind_button_visuals(variant);
    let bg = if enabled {
        visuals.bg
    } else {
        visuals.bg.gamma_multiply(0.6)
    };
    let hover_bg = if enabled {
        visuals.hover_bg
    } else {
        visuals.hover_bg.gamma_multiply(0.6)
    };
    let text = if enabled {
        visuals.text
    } else {
        visuals.text.gamma_multiply(0.6)
    };

    let stroke = egui::Stroke::new(1.0, Color32::from_white_alpha(26)).color;

    let font_id = egui::FontId::proportional(12.0);
    let label = egui::RichText::new(label).font(font_id).color(text);
    let corner_radius = match group_position {
        TailwindButtonGroupPosition::Single => egui::CornerRadius::same(6),
        TailwindButtonGroupPosition::First => egui::CornerRadius {
            nw: 12,
            ne: 0,
            sw: 12,
            se: 0,
        },
        TailwindButtonGroupPosition::Middle => egui::CornerRadius::same(0),
        TailwindButtonGroupPosition::Last => egui::CornerRadius {
            nw: 0,
            ne: 12,
            sw: 0,
            se: 12,
        },
    };

    let button = egui::Button::new(label)
        .frame(true)
        .corner_radius(corner_radius);

    ui.scope(|ui| {
        let mut style = ui.style().as_ref().clone();
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.visuals.widgets.inactive.bg_fill = bg;
        style.visuals.widgets.inactive.weak_bg_fill = bg;
        style.visuals.widgets.hovered.bg_fill = hover_bg;
        style.visuals.widgets.active.bg_fill = hover_bg;

        style.visuals.widgets.hovered.expansion = style.visuals.widgets.inactive.expansion;
        style.visuals.widgets.active.expansion = style.visuals.widgets.inactive.expansion;

        style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, stroke);
        style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, stroke);
        style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, stroke);
        style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, stroke);

        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
        style.visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
        style.visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
        style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;

        style.visuals.widgets.noninteractive.bg_fill = bg;
        ui.set_style(style);

        let response = ui.add_enabled(enabled, button).on_hover_text(title);
        if !response.hovered() {
            let stroke = egui::Stroke::new(1.0, stroke);
            ui.painter().rect_stroke(
                response.rect,
                corner_radius,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        response
    })
    .inner
}
