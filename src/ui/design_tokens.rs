use rust_wgpu_fiber::eframe::egui::{self, Color32, FontId, RichText};

use super::typography;

pub const FONT_SIZE_9: f32 = 11.0;
pub const FONT_SIZE_11: f32 = 13.0;
pub const FONT_SIZE_13: f32 = 15.0;
pub const CONTROL_ROW_HEIGHT: f32 = 26.0;
pub const RESOURCE_ACTIVE_BG: Color32 = Color32::from_gray(40);

pub const LINE_THICKNESS_05: f32 = 0.5;
pub const LINE_THICKNESS_1: f32 = 1.0;
pub const LINE_THICKNESS_2: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontWeight {
    Light,
    Normal,
    Medium,
    Bold,
}

impl FontWeight {
    pub fn css_weight(self) -> f32 {
        match self {
            Self::Light => 300.0,
            Self::Normal => 400.0,
            Self::Medium => 500.0,
            Self::Bold => 700.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextRole {
    SectionTitle,
    AttributeTitle,
    ActiveItemTitle,
    InactiveItemTitle,
    ValueLabel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyleToken {
    pub size: f32,
    pub weight: FontWeight,
    pub color: Color32,
}

pub fn text_style(role: TextRole) -> TextStyleToken {
    match role {
        TextRole::SectionTitle => TextStyleToken {
            size: FONT_SIZE_11,
            weight: FontWeight::Medium,
            color: white(90),
        },
        TextRole::AttributeTitle => TextStyleToken {
            size: FONT_SIZE_9,
            weight: FontWeight::Normal,
            color: white(60),
        },
        TextRole::ActiveItemTitle => TextStyleToken {
            size: FONT_SIZE_11,
            weight: FontWeight::Medium,
            color: white(90),
        },
        TextRole::InactiveItemTitle => TextStyleToken {
            size: FONT_SIZE_11,
            weight: FontWeight::Normal,
            color: white(60),
        },
        TextRole::ValueLabel => TextStyleToken {
            size: FONT_SIZE_11,
            weight: FontWeight::Normal,
            color: white(80),
        },
    }
}

pub fn font_id(size: f32, weight: FontWeight) -> FontId {
    FontId::new(
        size,
        typography::mi_sans_family_for_weight(weight.css_weight()),
    )
}

pub fn rich_text(text: impl Into<String>, role: TextRole) -> RichText {
    let style = text_style(role);
    RichText::new(text)
        .font(font_id(style.size, style.weight))
        .color(style.color)
}

pub fn white(step: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(255, 255, 255, alpha_from_step(step))
}

pub fn black(step: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(0, 0, 0, alpha_from_step(step))
}

pub fn radius(px: u8) -> egui::CornerRadius {
    let canonical = (px.clamp(2, 24) / 2) * 2;
    egui::CornerRadius::same(canonical)
}

fn alpha_from_step(step: u8) -> u8 {
    let canonical = canonical_step(step);
    ((canonical as f32 / 100.0) * 255.0).round() as u8
}

fn canonical_step(step: u8) -> u8 {
    let clamped = step.clamp(10, 100);
    let rounded = (((clamped as f32) / 10.0).round() * 10.0) as u8;
    rounded.clamp(10, 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_black_step_mapping_matches_expected_alpha() {
        assert_eq!(white(90).a(), 230);
        assert_eq!(white(10).a(), 26);
        assert_eq!(black(80).a(), 204);
        assert_eq!(black(100).a(), 255);
    }

    #[test]
    fn radius_is_even_and_clamped() {
        assert_eq!(radius(1), egui::CornerRadius::same(2));
        assert_eq!(radius(5), egui::CornerRadius::same(4));
        assert_eq!(radius(24), egui::CornerRadius::same(24));
        assert_eq!(radius(31), egui::CornerRadius::same(24));
    }

    #[test]
    fn semantic_text_roles_match_contract() {
        let section = text_style(TextRole::SectionTitle);
        assert_eq!(section.size, FONT_SIZE_11);
        assert_eq!(section.weight, FontWeight::Medium);
        assert_eq!(section.color, white(90));

        let attr = text_style(TextRole::AttributeTitle);
        assert_eq!(attr.size, FONT_SIZE_9);
        assert_eq!(attr.weight, FontWeight::Normal);
        assert_eq!(attr.color, white(60));
    }
}
