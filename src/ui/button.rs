use rust_wgpu_fiber::eframe::egui::{self, Color32};

use super::design_tokens::{self, FontWeight};

pub use super::design_tokens::{ButtonSize, ButtonVariant};

#[derive(Clone, Copy, Debug)]
pub struct ButtonVisualOverride {
    pub bg: Color32,
    pub hover_bg: Color32,
    pub active_bg: Color32,
    pub text: Color32,
    pub border: Color32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonIcon {
    Eye,
    EyeOff,
}

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
pub struct ButtonOptions<'a> {
    pub label: &'a str,
    pub tooltip: Option<&'a str>,
    pub variant: ButtonVariant,
    pub size: ButtonSize,
    pub enabled: bool,
    pub icon: Option<&'a str>,
    pub icon_kind: Option<ButtonIcon>,
    pub visual_override: Option<ButtonVisualOverride>,
    pub group_position: TailwindButtonGroupPosition,
}

pub fn apply_response_affordance(
    mut response: egui::Response,
    tooltip: Option<&str>,
    clickable: bool,
) -> egui::Response {
    if let Some(tooltip) = tooltip {
        response = response.on_hover_text(tooltip);
    }
    if clickable {
        response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    response
}

impl<'a> ButtonOptions<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            tooltip: None,
            variant: ButtonVariant::Default,
            size: ButtonSize::Default,
            enabled: true,
            icon: None,
            icon_kind: None,
            visual_override: None,
            group_position: TailwindButtonGroupPosition::Single,
        }
    }
}

fn is_icon_only(options: ButtonOptions<'_>) -> bool {
    matches!(options.variant, ButtonVariant::Icon)
        || (options.label.is_empty() && (options.icon.is_some() || options.icon_kind.is_some()))
}

fn apply_enabled(color: Color32, enabled: bool) -> Color32 {
    if enabled {
        color
    } else {
        color.gamma_multiply(design_tokens::BUTTON_DISABLED_GAMMA)
    }
}

fn corner_radius_for_group(
    base: egui::CornerRadius,
    group_position: TailwindButtonGroupPosition,
) -> egui::CornerRadius {
    match group_position {
        TailwindButtonGroupPosition::Single => base,
        TailwindButtonGroupPosition::First => egui::CornerRadius {
            nw: base.nw,
            ne: 0,
            sw: base.sw,
            se: 0,
        },
        TailwindButtonGroupPosition::Middle => egui::CornerRadius::same(0),
        TailwindButtonGroupPosition::Last => egui::CornerRadius {
            nw: 0,
            ne: base.ne,
            sw: 0,
            se: base.se,
        },
    }
}

fn button_text(options: ButtonOptions<'_>) -> String {
    if options.icon_kind.is_some() {
        if options.label.is_empty() {
            return " ".to_string();
        }
        return options.label.to_string();
    }
    match options.variant {
        ButtonVariant::Icon => options.icon.unwrap_or("◉").to_string(),
        ButtonVariant::WithIcon => match options.icon {
            Some(icon) if !options.label.is_empty() => format!("{} {}", icon, options.label),
            Some(icon) => icon.to_string(),
            None => options.label.to_string(),
        },
        ButtonVariant::Spinner => {
            if options.label.is_empty() {
                "◌".to_string()
            } else {
                format!("◌ {}", options.label)
            }
        }
        _ => options.label.to_string(),
    }
}

fn button_min_width(
    size_token: design_tokens::ButtonSizeToken,
    variant: ButtonVariant,
    icon_only: bool,
) -> f32 {
    match variant {
        ButtonVariant::Spinner => size_token.height * 1.2,
        _ if icon_only => size_token.height,
        _ => 0.0,
    }
}

fn icon_square_rect(rect: egui::Rect, preferred_side: f32) -> egui::Rect {
    let max_square = rect.width().min(rect.height());
    let side = preferred_side.min(max_square).max(6.0);
    egui::Rect::from_center_size(rect.center(), egui::vec2(side, side))
}

fn draw_eye_icon(
    painter: &egui::Painter,
    icon_rect: egui::Rect,
    color: Color32,
    stroked: bool,
    strike: bool,
) {
    let center = icon_rect.center();
    let rx = icon_rect.width() * 0.42;
    let ry = icon_rect.height() * 0.27;
    let mut points = Vec::with_capacity(20);
    for i in 0..20 {
        let t = std::f32::consts::TAU * (i as f32 / 20.0);
        points.push(egui::pos2(center.x + rx * t.cos(), center.y + ry * t.sin()));
    }
    if stroked {
        painter.add(egui::Shape::closed_line(
            points,
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, color),
        ));
    }
    painter.circle_filled(center, icon_rect.width() * 0.11, color);
    if strike {
        let a = egui::pos2(
            icon_rect.left() + icon_rect.width() * 0.18,
            icon_rect.bottom() - 1.0,
        );
        let b = egui::pos2(
            icon_rect.right() - icon_rect.width() * 0.18,
            icon_rect.top() + 1.0,
        );
        painter.line_segment(
            [a, b],
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, color),
        );
    }
}

fn paint_button_icon(
    ui: &egui::Ui,
    rect: egui::Rect,
    icon_kind: ButtonIcon,
    icon_size: f32,
    color: Color32,
) {
    let icon_rect = icon_square_rect(rect, icon_size);
    let painter = ui.painter();
    match icon_kind {
        ButtonIcon::Eye => draw_eye_icon(painter, icon_rect, color, true, false),
        ButtonIcon::EyeOff => draw_eye_icon(painter, icon_rect, color, true, true),
    }
}

pub fn button(ui: &mut egui::Ui, options: ButtonOptions<'_>) -> egui::Response {
    let icon_only = is_icon_only(options);
    let size_token = design_tokens::button_size_token(options.size);
    let visual = options.visual_override.map_or_else(
        || design_tokens::button_visual_token(options.variant),
        |v| design_tokens::ButtonVisualToken {
            bg: v.bg,
            hover_bg: v.hover_bg,
            active_bg: v.active_bg,
            text: v.text,
            border: v.border,
        },
    );

    let bg = apply_enabled(visual.bg, options.enabled);
    let hover_bg = apply_enabled(visual.hover_bg, options.enabled);
    let active_bg = apply_enabled(visual.active_bg, options.enabled);
    let text = apply_enabled(visual.text, options.enabled);
    let border = apply_enabled(visual.border, options.enabled);

    let corner_radius = corner_radius_for_group(
        design_tokens::button_corner_radius(options.variant),
        options.group_position,
    );

    let text_size = if icon_only {
        size_token.icon_size
    } else {
        size_token.font_size
    };
    let label = egui::RichText::new(button_text(options))
        .font(design_tokens::font_id(text_size, FontWeight::Medium))
        .color(text);

    let button = egui::Button::new(label)
        .frame(true)
        .corner_radius(corner_radius)
        .min_size(egui::vec2(
            button_min_width(size_token, options.variant, icon_only),
            size_token.height,
        ));

    ui.scope(|ui| {
        let mut style = ui.style().as_ref().clone();
        style.spacing.button_padding =
            egui::vec2(size_token.horizontal_padding, size_token.vertical_padding);

        style.visuals.widgets.inactive.bg_fill = bg;
        style.visuals.widgets.hovered.bg_fill = hover_bg;
        style.visuals.widgets.active.bg_fill = active_bg;
        style.visuals.widgets.noninteractive.bg_fill = bg;

        style.visuals.widgets.inactive.weak_bg_fill = bg;
        style.visuals.widgets.hovered.weak_bg_fill = hover_bg;
        style.visuals.widgets.active.weak_bg_fill = active_bg;

        style.visuals.widgets.inactive.fg_stroke =
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, text);
        style.visuals.widgets.hovered.fg_stroke =
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, text);
        style.visuals.widgets.active.fg_stroke =
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, text);
        style.visuals.widgets.noninteractive.fg_stroke =
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, text);

        style.visuals.widgets.inactive.bg_stroke =
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, border);
        style.visuals.widgets.hovered.bg_stroke =
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, border);
        style.visuals.widgets.active.bg_stroke =
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, border);
        style.visuals.widgets.noninteractive.bg_stroke =
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, border);

        style.visuals.widgets.hovered.expansion = style.visuals.widgets.inactive.expansion;
        style.visuals.widgets.active.expansion = style.visuals.widgets.inactive.expansion;
        ui.set_style(style);

        let mut response = ui.add_enabled(options.enabled, button);
        response = apply_response_affordance(response, options.tooltip, options.enabled);
        if let Some(icon_kind) = options.icon_kind {
            paint_button_icon(ui, response.rect, icon_kind, size_token.icon_size, text);
        }
        response
    })
    .inner
}

fn tailwind_to_variant(variant: TailwindButtonVariant) -> ButtonVariant {
    match variant {
        TailwindButtonVariant::Destructive => ButtonVariant::Destructive,
        TailwindButtonVariant::Connected => ButtonVariant::Secondary,
        TailwindButtonVariant::Idle => ButtonVariant::Default,
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
    button(
        ui,
        ButtonOptions {
            label,
            tooltip: Some(title),
            variant: tailwind_to_variant(variant),
            size: ButtonSize::Small,
            enabled,
            icon: None,
            icon_kind: None,
            visual_override: None,
            group_position,
        },
    )
}
