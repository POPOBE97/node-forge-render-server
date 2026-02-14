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
    Folder,
    Trash,
}

#[derive(Clone, Copy, Debug)]
pub enum ButtonGroupPosition {
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
    pub group_position: ButtonGroupPosition,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GroupButtonBehavior {
    pub draw_group_hover_border: bool,
    pub truncate_primary_middle: bool,
}

pub struct GroupButtonOptions<'a> {
    pub primary: ButtonOptions<'a>,
    pub secondary: Option<ButtonOptions<'a>>,
    pub behavior: GroupButtonBehavior,
}

pub struct GroupButtonResponse {
    pub primary: egui::Response,
    pub secondary: Option<egui::Response>,
}

const ICON_LABEL_GAP_PX: f32 = 8.0;

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
            group_position: ButtonGroupPosition::Single,
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
    group_position: ButtonGroupPosition,
) -> egui::CornerRadius {
    match group_position {
        ButtonGroupPosition::Single => base,
        ButtonGroupPosition::First => egui::CornerRadius {
            nw: base.nw,
            ne: 0,
            sw: base.sw,
            se: 0,
        },
        ButtonGroupPosition::Middle => egui::CornerRadius::same(0),
        ButtonGroupPosition::Last => egui::CornerRadius {
            nw: 0,
            ne: base.ne,
            sw: 0,
            se: base.se,
        },
    }
}

fn button_text(options: ButtonOptions<'_>) -> String {
    if options.icon_kind.is_some() {
        return " ".to_string();
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

fn button_text_with_label(options: ButtonOptions<'_>, label: &str) -> String {
    if options.icon_kind.is_some() {
        return " ".to_string();
    }
    match options.variant {
        ButtonVariant::Icon => options.icon.unwrap_or("◉").to_string(),
        ButtonVariant::WithIcon => match options.icon {
            Some(icon) if !label.is_empty() => format!("{} {}", icon, label),
            Some(icon) => icon.to_string(),
            None => label.to_string(),
        },
        ButtonVariant::Spinner => {
            if label.is_empty() {
                "◌".to_string()
            } else {
                format!("◌ {}", label)
            }
        }
        _ => label.to_string(),
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

fn estimate_button_width(ui: &egui::Ui, options: ButtonOptions<'_>, label: &str) -> f32 {
    let icon_only = matches!(options.variant, ButtonVariant::Icon)
        || (label.is_empty() && (options.icon.is_some() || options.icon_kind.is_some()));
    let size_token = design_tokens::button_size_token(options.size);
    let text_size = if icon_only {
        size_token.icon_size
    } else {
        size_token.font_size
    };
    if options.icon_kind.is_some() && !label.is_empty() {
        let text_width = ui
            .painter()
            .layout_no_wrap(
                label.to_string(),
                design_tokens::font_id(text_size, FontWeight::Medium),
                Color32::WHITE,
            )
            .size()
            .x;
        let content = size_token.horizontal_padding * 2.0
            + size_token.icon_size
            + ICON_LABEL_GAP_PX
            + text_width;
        return content.max(button_min_width(size_token, options.variant, icon_only));
    }
    let text = button_text_with_label(options, label);
    let galley = ui.painter().layout_no_wrap(
        text,
        design_tokens::font_id(text_size, FontWeight::Medium),
        Color32::WHITE,
    );
    let content = galley.size().x + size_token.horizontal_padding * 2.0;
    content.max(button_min_width(size_token, options.variant, icon_only))
}

fn truncate_middle_to_width(
    ui: &egui::Ui,
    source: &str,
    max_width: f32,
    options: ButtonOptions<'_>,
) -> String {
    if source.is_empty() {
        return String::new();
    }
    if estimate_button_width(ui, options, source) <= max_width {
        return source.to_string();
    }

    let chars: Vec<char> = source.chars().collect();
    if chars.len() <= 1 {
        return "…".to_string();
    }

    let make_candidate = |keep: usize| {
        let left = keep.div_ceil(2);
        let right = keep / 2;
        let left_part: String = chars.iter().take(left).collect();
        let right_part: String = chars
            .iter()
            .rev()
            .take(right)
            .copied()
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        format!("{}…{}", left_part, right_part)
    };

    let mut lo = 1usize;
    let mut hi = chars.len();
    let mut best = "…".to_string();
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let candidate = make_candidate(mid);
        if estimate_button_width(ui, options, &candidate) <= max_width {
            best = candidate;
            lo = mid + 1;
        } else {
            if mid == 0 {
                break;
            }
            hi = mid.saturating_sub(1);
        }
    }
    best
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

fn draw_folder_icon(painter: &egui::Painter, icon_rect: egui::Rect, color: Color32) {
    let stroke = egui::Stroke::new(design_tokens::LINE_THICKNESS_1, color);
    let x0 = icon_rect.left() + icon_rect.width() * 0.14;
    let x1 = icon_rect.right() - icon_rect.width() * 0.14;
    let y_top = icon_rect.top() + icon_rect.height() * 0.32;
    let y_bottom = icon_rect.bottom() - icon_rect.height() * 0.16;
    let tab_left = x0 + icon_rect.width() * 0.12;
    let tab_right = tab_left + icon_rect.width() * 0.24;
    let tab_top = icon_rect.top() + icon_rect.height() * 0.16;

    let points = vec![
        egui::pos2(x0, y_top),
        egui::pos2(tab_left, y_top),
        egui::pos2(tab_left, tab_top),
        egui::pos2(tab_right, tab_top),
        egui::pos2(tab_right + icon_rect.width() * 0.06, y_top),
        egui::pos2(x1, y_top),
        egui::pos2(x1, y_bottom),
        egui::pos2(x0, y_bottom),
    ];
    painter.add(egui::Shape::closed_line(points, stroke));
}

fn draw_trash_icon(painter: &egui::Painter, icon_rect: egui::Rect, color: Color32) {
    let stroke = egui::Stroke::new(design_tokens::LINE_THICKNESS_1, color);
    let body_left = icon_rect.left() + icon_rect.width() * 0.28;
    let body_right = icon_rect.right() - icon_rect.width() * 0.28;
    let body_top = icon_rect.top() + icon_rect.height() * 0.34;
    let body_bottom = icon_rect.bottom() - icon_rect.height() * 0.16;

    painter.rect_stroke(
        egui::Rect::from_min_max(
            egui::pos2(body_left, body_top),
            egui::pos2(body_right, body_bottom),
        ),
        egui::CornerRadius::same(1),
        stroke,
        egui::StrokeKind::Middle,
    );

    let lid_y = icon_rect.top() + icon_rect.height() * 0.24;
    let lid_left = body_left - icon_rect.width() * 0.08;
    let lid_right = body_right + icon_rect.width() * 0.08;
    painter.line_segment(
        [egui::pos2(lid_left, lid_y), egui::pos2(lid_right, lid_y)],
        stroke,
    );

    let handle_w = icon_rect.width() * 0.18;
    let handle_y = lid_y - icon_rect.height() * 0.09;
    painter.line_segment(
        [
            egui::pos2(icon_rect.center().x - handle_w * 0.5, handle_y),
            egui::pos2(icon_rect.center().x + handle_w * 0.5, handle_y),
        ],
        stroke,
    );

    let col1 = body_left + (body_right - body_left) * 0.33;
    let col2 = body_left + (body_right - body_left) * 0.67;
    let col_top = body_top + icon_rect.height() * 0.08;
    let col_bottom = body_bottom - icon_rect.height() * 0.08;
    painter.line_segment([egui::pos2(col1, col_top), egui::pos2(col1, col_bottom)], stroke);
    painter.line_segment([egui::pos2(col2, col_top), egui::pos2(col2, col_bottom)], stroke);
}

fn paint_button_icon(
    ui: &egui::Ui,
    rect: egui::Rect,
    icon_kind: ButtonIcon,
    icon_size: f32,
    horizontal_padding: f32,
    has_text_label: bool,
    color: Color32,
) -> egui::Rect {
    let icon_rect = if has_text_label {
        let max_square = rect.width().min(rect.height());
        let side = icon_size.min(max_square).max(6.0);
        let center_x = (rect.left() + horizontal_padding + side * 0.5)
            .max(rect.left() + side * 0.5)
            .min(rect.right() - side * 0.5 - 1.0);
        egui::Rect::from_center_size(egui::pos2(center_x, rect.center().y), egui::vec2(side, side))
    } else {
        icon_square_rect(rect, icon_size)
    };
    let painter = ui.painter();
    match icon_kind {
        ButtonIcon::Eye => draw_eye_icon(painter, icon_rect, color, true, false),
        ButtonIcon::EyeOff => draw_eye_icon(painter, icon_rect, color, true, true),
        ButtonIcon::Folder => draw_folder_icon(painter, icon_rect, color),
        ButtonIcon::Trash => draw_trash_icon(painter, icon_rect, color),
    }
    icon_rect
}

pub fn button(ui: &mut egui::Ui, options: ButtonOptions<'_>) -> egui::Response {
    let icon_only = is_icon_only(options);
    let has_icon_and_text = options.icon_kind.is_some() && !options.label.is_empty();
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
    let font_id = design_tokens::font_id(text_size, FontWeight::Medium);
    let label = egui::RichText::new(button_text(options))
        .font(font_id.clone())
        .color(text);

    let min_width = if has_icon_and_text {
        let text_width = ui
            .painter()
            .layout_no_wrap(options.label.to_string(), font_id.clone(), text)
            .size()
            .x;
        (size_token.horizontal_padding * 2.0
            + size_token.icon_size
            + ICON_LABEL_GAP_PX
            + text_width)
            .max(button_min_width(size_token, options.variant, icon_only))
    } else {
        button_min_width(size_token, options.variant, icon_only)
    };

    let button = egui::Button::new(label)
        .frame(true)
        .corner_radius(corner_radius)
        .min_size(egui::vec2(min_width, size_token.height));

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
            let icon_rect = paint_button_icon(
                ui,
                response.rect,
                icon_kind,
                size_token.icon_size,
                size_token.horizontal_padding,
                !options.label.is_empty(),
                text,
            );
            if has_icon_and_text {
                let galley = ui
                    .painter()
                    .layout_no_wrap(options.label.to_string(), font_id.clone(), text);
                let text_pos = egui::pos2(
                    icon_rect.right() + ICON_LABEL_GAP_PX,
                    response.rect.center().y - galley.size().y * 0.5,
                );
                ui.painter().galley(text_pos, galley, text);
            }
        }
        response
    })
    .inner
}

pub fn group_button(ui: &mut egui::Ui, mut options: GroupButtonOptions<'_>) -> GroupButtonResponse {
    let has_secondary = options.secondary.is_some();
    options.primary.group_position = if has_secondary {
        ButtonGroupPosition::First
    } else {
        ButtonGroupPosition::Single
    };
    if let Some(secondary) = options.secondary.as_mut() {
        secondary.group_position = ButtonGroupPosition::Last;
    }

    let truncated_primary_label = if options.behavior.truncate_primary_middle
        && !options.primary.label.is_empty()
    {
        let secondary_width = options
            .secondary
            .as_ref()
            .map(|secondary| estimate_button_width(ui, *secondary, secondary.label))
            .unwrap_or(0.0);
        let max_primary_width = (ui.available_width() - secondary_width).max(0.0);
        Some(truncate_middle_to_width(
            ui,
            options.primary.label,
            max_primary_width,
            options.primary,
        ))
    } else {
        None
    };

    let primary_label = truncated_primary_label
        .as_deref()
        .unwrap_or(options.primary.label);
    let primary = ButtonOptions {
        label: primary_label,
        ..options.primary
    };

    let is_rtl = matches!(ui.layout().main_dir(), egui::Direction::RightToLeft);
    let old_spacing_x = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 0.0;
    let (primary_resp, secondary_resp) = if is_rtl {
        let secondary_resp = options.secondary.map(|secondary| button(ui, secondary));
        let primary_resp = button(ui, primary);
        (primary_resp, secondary_resp)
    } else {
        let primary_resp = button(ui, primary);
        let secondary_resp = options.secondary.map(|secondary| button(ui, secondary));
        (primary_resp, secondary_resp)
    };
    ui.spacing_mut().item_spacing.x = old_spacing_x;

    let group_rect = if let Some(secondary) = secondary_resp.as_ref() {
        primary_resp.rect.union(secondary.rect)
    } else {
        primary_resp.rect
    };

    if has_secondary {
        let separator_x = primary_resp.rect.right() - design_tokens::LINE_THICKNESS_1 * 0.5;
        ui.painter().line_segment(
            [
                egui::pos2(separator_x, group_rect.top() + design_tokens::LINE_THICKNESS_1),
                egui::pos2(separator_x, group_rect.bottom() - design_tokens::LINE_THICKNESS_1),
            ],
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, design_tokens::white(20)),
        );
    }

    let group_hovered = primary_resp.hovered()
        || secondary_resp.as_ref().is_some_and(|response| response.hovered());

    if has_secondary && options.behavior.draw_group_hover_border {
        ui.painter().rect_stroke(
            group_rect,
            design_tokens::button_corner_radius(options.primary.variant),
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, design_tokens::white(20)),
            egui::StrokeKind::Inside,
        );
    }

    GroupButtonResponse {
        primary: primary_resp,
        secondary: secondary_resp,
    }
}