use rust_wgpu_fiber::eframe::egui::{self, Color32, Rect, pos2};

pub struct ViewportIndicator<'a> {
    pub icon: &'a str,
    pub tooltip: &'a str,
}

pub fn draw_viewport_indicators(
    ui: &mut egui::Ui,
    canvas_rect: Rect,
    indicators: &[ViewportIndicator<'_>],
) {
    if indicators.is_empty() {
        return;
    }

    let item_size = egui::vec2(20.0, 20.0);
    let gap = 6.0;
    let right_pad = 8.0;
    let top_pad = 8.0;

    for (idx, indicator) in indicators.iter().enumerate() {
        let x = canvas_rect.max.x - right_pad - item_size.x - idx as f32 * (item_size.x + gap);
        let y = canvas_rect.min.y + top_pad;
        let rect = Rect::from_min_size(pos2(x, y), item_size);
        let response = ui.allocate_rect(rect, egui::Sense::hover());
        response.on_hover_text(indicator.tooltip);

        ui.painter().rect(
            rect,
            egui::CornerRadius::same(5),
            Color32::from_rgba_unmultiplied(0, 0, 0, 180),
            egui::Stroke::new(1.0, Color32::from_gray(52)),
            egui::StrokeKind::Outside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            indicator.icon,
            egui::FontId::new(
                11.0,
                crate::ui::typography::mi_sans_family_for_weight(600.0),
            ),
            Color32::from_gray(220),
        );
    }
}