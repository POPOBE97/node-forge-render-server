use rust_wgpu_fiber::eframe::egui::{self, Color32, Rect, pos2};

use super::{button::apply_response_affordance, design_tokens};

#[derive(Clone, Copy, Debug)]
pub enum ViewportIndicatorKind {
    Text,
    Spinner,
    Success,
    Failure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewportIndicatorInteraction {
    HoverOnly,
    Clickable,
}

pub struct ViewportIndicator<'a> {
    pub icon: &'a str,
    pub tooltip: &'a str,
    pub kind: ViewportIndicatorKind,
}

#[derive(Clone, Debug)]
pub enum ViewportIndicatorContent {
    Compact {
        icon: String,
        tooltip: String,
        kind: ViewportIndicatorKind,
    },
    TextBadge {
        text: String,
        tooltip: String,
    },
}

#[derive(Clone, Debug)]
pub struct ViewportIndicatorEntry {
    pub key: String,
    pub order: i32,
    pub visible: bool,
    pub animated: bool,
    pub interaction: ViewportIndicatorInteraction,
    pub callback_id: Option<String>,
    pub content: ViewportIndicatorContent,
    pub allow_overflow_collapse: bool,
}

impl ViewportIndicatorEntry {
    pub fn compact(
        key: impl Into<String>,
        order: i32,
        visible: bool,
        indicator: ViewportIndicator<'_>,
    ) -> Self {
        Self {
            key: key.into(),
            order,
            visible,
            animated: true,
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            content: ViewportIndicatorContent::Compact {
                icon: indicator.icon.to_string(),
                tooltip: indicator.tooltip.to_string(),
                kind: indicator.kind,
            },
            allow_overflow_collapse: false,
        }
    }

    pub fn text_badge(
        key: impl Into<String>,
        order: i32,
        visible: bool,
        text: impl Into<String>,
        tooltip: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            order,
            visible,
            animated: true,
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            content: ViewportIndicatorContent::TextBadge {
                text: text.into(),
                tooltip: tooltip.into(),
            },
            allow_overflow_collapse: false,
        }
    }
}

#[derive(Default)]
pub struct ViewportIndicatorManager {
    entries: Vec<ViewportIndicatorEntry>,
}

pub struct ViewportIndicatorRenderResult {
    pub clicked_callback_ids: Vec<String>,
    pub needs_repaint: bool,
}

impl ViewportIndicatorManager {
    pub fn begin_frame(&mut self) {
        self.entries.clear();
    }

    pub fn register(&mut self, entry: ViewportIndicatorEntry) {
        self.entries.push(entry);
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        canvas_rect: Rect,
        now: f64,
    ) -> ViewportIndicatorRenderResult {
        let mut entries = std::mem::take(&mut self.entries);
        entries.sort_by_key(|entry| entry.order);

        let mut clicked_callback_ids = Vec::new();
        let mut any_animating = false;

        let mut occupied_width = 0.0;
        let y = canvas_rect.min.y + VIEWPORT_INDICATOR_TOP_PAD;
        let right_edge = canvas_rect.max.x - VIEWPORT_INDICATOR_RIGHT_PAD;

        for entry in &entries {
            let anim_t = if entry.animated {
                ctx.animate_bool(egui::Id::new(format!("ui.viewport.indicator.{}", entry.key)), entry.visible)
            } else if entry.visible {
                1.0
            } else {
                0.0
            };

            if anim_t <= 0.001 {
                continue;
            }

            any_animating |= entry.animated && anim_t < 0.999;

            let width = entry.width(ui);
            let slide_x = if entry.animated {
                (1.0 - anim_t) * 8.0
            } else {
                0.0
            };
            let x = right_edge - width - occupied_width + slide_x;
            let rect = Rect::from_min_size(
                pos2(x, y),
                egui::vec2(width, VIEWPORT_INDICATOR_ITEM_SIZE),
            );

            let response = match &entry.content {
                ViewportIndicatorContent::Compact {
                    icon,
                    tooltip,
                    kind,
                } => {
                    let indicator = ViewportIndicator {
                        icon,
                        tooltip,
                        kind: *kind,
                    };
                    draw_viewport_indicator_at(
                        ui,
                        rect,
                        &indicator,
                        now,
                        anim_t,
                        entry.interaction,
                    )
                }
                ViewportIndicatorContent::TextBadge { text, tooltip } => {
                    draw_text_badge_at(ui, rect, text, tooltip, anim_t, entry.interaction)
                }
            };

            if response.clicked()
                && matches!(entry.interaction, ViewportIndicatorInteraction::Clickable)
                && let Some(callback_id) = &entry.callback_id
            {
                clicked_callback_ids.push(callback_id.clone());
            }

            occupied_width += anim_t * (width + VIEWPORT_INDICATOR_GAP);
        }

        self.entries = entries;

        ViewportIndicatorRenderResult {
            clicked_callback_ids,
            needs_repaint: any_animating,
        }
    }
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
    interaction: ViewportIndicatorInteraction,
) -> egui::Response {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.001 {
        return ui.allocate_rect(rect, egui::Sense::hover());
    }

    let response = ui.allocate_rect(
        rect,
        match interaction {
            ViewportIndicatorInteraction::HoverOnly => egui::Sense::hover(),
            ViewportIndicatorInteraction::Clickable => egui::Sense::click(),
        },
    );
    let response = apply_response_affordance(
        response,
        Some(indicator.tooltip),
        matches!(interaction, ViewportIndicatorInteraction::Clickable),
    );

    let (bg_color, border_color, text_color) = match indicator.kind {
        ViewportIndicatorKind::Success => (
            design_tokens::indicator_success_bg(),
            design_tokens::indicator_success_border(),
            design_tokens::indicator_success_fg(),
        ),
        ViewportIndicatorKind::Failure => (
            design_tokens::indicator_failure_bg(),
            design_tokens::indicator_failure_border(),
            design_tokens::indicator_failure_fg(),
        ),
        _ => (
            design_tokens::indicator_neutral_bg(),
            design_tokens::indicator_neutral_border(),
            design_tokens::indicator_neutral_fg(),
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

    response
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
        let _ = draw_viewport_indicator_at(
            ui,
            rect,
            indicator,
            now,
            1.0,
            ViewportIndicatorInteraction::HoverOnly,
        );
    }
}

fn draw_text_badge_at(
    ui: &mut egui::Ui,
    rect: Rect,
    text: &str,
    tooltip: &str,
    alpha: f32,
    interaction: ViewportIndicatorInteraction,
) -> egui::Response {
    let alpha = alpha.clamp(0.0, 1.0);
    let response = ui.allocate_rect(
        rect,
        match interaction {
            ViewportIndicatorInteraction::HoverOnly => egui::Sense::hover(),
            ViewportIndicatorInteraction::Clickable => egui::Sense::click(),
        },
    );
    let response = apply_response_affordance(
        response,
        Some(tooltip),
        matches!(interaction, ViewportIndicatorInteraction::Clickable),
    );

    ui.painter().rect(
        rect,
        egui::CornerRadius::same(6),
        with_alpha(Color32::from_rgba_unmultiplied(0, 0, 0, 176), alpha),
        egui::Stroke::new(
            1.0,
            with_alpha(Color32::from_rgba_unmultiplied(52, 52, 52, 220), alpha),
        ),
        egui::StrokeKind::Outside,
    );

    ui.painter().text(
        pos2(rect.min.x + 7.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::new(
            10.0,
            crate::ui::typography::mi_sans_family_for_weight(500.0),
        ),
        with_alpha(Color32::from_rgba_unmultiplied(220, 220, 220, 220), alpha),
    );

    response
}

impl ViewportIndicatorEntry {
    fn width(&self, ui: &egui::Ui) -> f32 {
        match &self.content {
            ViewportIndicatorContent::Compact { .. } => VIEWPORT_INDICATOR_ITEM_SIZE,
            ViewportIndicatorContent::TextBadge { text, .. } => {
                let galley = ui.painter().layout_no_wrap(
                    text.clone(),
                    egui::FontId::new(
                        10.0,
                        crate::ui::typography::mi_sans_family_for_weight(500.0),
                    ),
                    Color32::WHITE,
                );
                galley.size().x + 14.0
            }
        }
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
            Color32::from_rgba_unmultiplied(
                design_tokens::indicator_neutral_fg().r(),
                design_tokens::indicator_neutral_fg().g(),
                design_tokens::indicator_neutral_fg().b(),
                dot_alpha,
            ),
        );
    }
}

fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    let a = ((color.a() as f32) * alpha.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}
