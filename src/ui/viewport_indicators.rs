use rust_wgpu_fiber::eframe::egui::{self, Color32, Rect, pos2};

#[derive(Clone, Copy, Debug)]
pub enum ViewportIndicatorKind {
    Text,
    Spinner,
    Success,
    Failure,
}

pub struct ViewportIndicator<'a> {
    pub icon: &'a str,
    pub tooltip: &'a str,
    pub kind: ViewportIndicatorKind,
}

pub const VIEWPORT_INDICATOR_ITEM_SIZE: f32 = 20.0;
pub const VIEWPORT_INDICATOR_GAP: f32 = 6.0;
pub const VIEWPORT_INDICATOR_RIGHT_PAD: f32 = 8.0;
pub const VIEWPORT_INDICATOR_TOP_PAD: f32 = 8.0;

pub fn draw_viewport_indicator_at(
    ui: &mut egui::Ui,
    rect: Rect,
    indicator: &ViewportIndicator<'_>,
    now: f64,
    alpha: f32,
) {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.001 {
        return;
    }

    let response = ui.allocate_rect(rect, egui::Sense::hover());
    response.on_hover_text(indicator.tooltip);

    let (bg_color, border_color, text_color) = match indicator.kind {
        ViewportIndicatorKind::Success => (
            Color32::from_rgba_unmultiplied(18, 54, 32, 220),
            Color32::from_rgb(39, 106, 63),
            Color32::from_rgb(133, 242, 172),
        ),
        ViewportIndicatorKind::Failure => (
            Color32::from_rgba_unmultiplied(62, 20, 20, 220),
            Color32::from_rgb(132, 43, 43),
            Color32::from_rgb(255, 118, 118),
        ),
        _ => (
            Color32::from_rgba_unmultiplied(0, 0, 0, 180),
            Color32::from_gray(52),
            Color32::from_gray(220),
        ),
    };

    ui.painter().rect(
        rect,
        egui::CornerRadius::same(5),
        with_alpha(bg_color, alpha),
        egui::Stroke::new(1.0, with_alpha(border_color, alpha)),
        egui::StrokeKind::Outside,
    );

    match indicator.kind {
        ViewportIndicatorKind::Spinner => {
            draw_spinner(ui, rect, now as f32, alpha);
        }
        _ => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                indicator.icon,
                egui::FontId::new(
                    11.0,
                    crate::ui::typography::mi_sans_family_for_weight(600.0),
                ),
                with_alpha(text_color, alpha),
            );
        }
    }
}

pub fn draw_viewport_indicators(
    ui: &mut egui::Ui,
    canvas_rect: Rect,
    indicators: &[ViewportIndicator<'_>],
    now: f64,
) {
    if indicators.is_empty() {
        return;
    }

    let item_size = egui::vec2(VIEWPORT_INDICATOR_ITEM_SIZE, VIEWPORT_INDICATOR_ITEM_SIZE);
    let gap = VIEWPORT_INDICATOR_GAP;
    let right_pad = VIEWPORT_INDICATOR_RIGHT_PAD;
    let top_pad = VIEWPORT_INDICATOR_TOP_PAD;

    for (idx, indicator) in indicators.iter().enumerate() {
        let x = canvas_rect.max.x - right_pad - item_size.x - idx as f32 * (item_size.x + gap);
        let y = canvas_rect.min.y + top_pad;
        let rect = Rect::from_min_size(pos2(x, y), item_size);
        draw_viewport_indicator_at(ui, rect, indicator, now, 1.0);
    }
}

fn draw_spinner(ui: &egui::Ui, rect: Rect, now: f32, alpha: f32) {
    let painter = ui.painter();
    let center = rect.center();
    let radius = 5.0;
    let dot_radius = 1.6;
    let steps = 10;
    let spin = now * 8.0;

    for step in 0..steps {
        let t = step as f32 / steps as f32;
        let angle = spin + t * std::f32::consts::TAU;
        let dot_alpha = ((step + 1) as f32 / steps as f32 * 220.0 * alpha.clamp(0.0, 1.0)) as u8;
        let dot = pos2(
            center.x + angle.cos() * radius,
            center.y + angle.sin() * radius,
        );
        painter.circle_filled(
            dot,
            dot_radius,
            Color32::from_rgba_unmultiplied(220, 220, 220, dot_alpha),
        );
    }
}

fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    let a = ((color.a() as f32) * alpha.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}