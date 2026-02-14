use rust_wgpu_fiber::eframe::egui::{self, Color32, FontId, RichText};

use super::typography;

pub const FONT_SIZE_9: f32 = 9.0;
pub const FONT_SIZE_11: f32 = 11.0;
pub const FONT_SIZE_13: f32 = 13.0;
pub const FONT_SIZE_15: f32 = 15.0;
pub const CONTROL_ROW_HEIGHT: f32 = 28.0;
pub const RESOURCE_ACTIVE_BG: Color32 = Color32::from_gray(40);
pub const BUTTON_ICON_SIZE_DEFAULT: f32 = 13.0;
pub const BUTTON_DISABLED_GAMMA: f32 = 0.6;

pub const BORDER_RADIUS_XSMALL: f32 = 2.0;
pub const BORDER_RADIUS_SMALL: f32 = 4.0;
pub const BORDER_RADIUS_REGULAR: f32 = 8.0;

pub const LINE_THICKNESS_05: f32 = 0.5;
pub const LINE_THICKNESS_1: f32 = 1.0;
pub const LINE_THICKNESS_2: f32 = 2.0;

pub fn indicator_success_bg() -> Color32 {
    Color32::from_rgba_unmultiplied(18, 54, 32, 220)
}

pub fn indicator_success_border() -> Color32 {
    Color32::from_rgb(39, 106, 63)
}

pub fn indicator_success_fg() -> Color32 {
    Color32::from_rgb(133, 242, 172)
}

pub fn indicator_failure_bg() -> Color32 {
    Color32::from_rgba_unmultiplied(62, 20, 20, 220)
}

pub fn indicator_failure_border() -> Color32 {
    Color32::from_rgb(132, 43, 43)
}

pub fn indicator_failure_fg() -> Color32 {
    Color32::from_rgb(255, 118, 118)
}

pub fn indicator_neutral_bg() -> Color32 {
    Color32::from_rgba_unmultiplied(0, 0, 0, 180)
}

pub fn indicator_neutral_border() -> Color32 {
    Color32::from_gray(52)
}

pub fn indicator_neutral_fg() -> Color32 {
    Color32::from_gray(220)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonVariant {
    Default,
    Secondary,
    Outline,
    Ghost,
    Destructive,
    Icon,
    WithIcon,
    Rounded,
    Spinner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonSize {
    ExtraSmall,
    Small,
    Default,
    Large,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonSizeToken {
    pub height: f32,
    pub horizontal_padding: f32,
    pub vertical_padding: f32,
    pub font_size: f32,
    pub icon_size: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonVisualToken {
    pub bg: Color32,
    pub hover_bg: Color32,
    pub active_bg: Color32,
    pub text: Color32,
    pub border: Color32,
}

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
            size: FONT_SIZE_13,
            weight: FontWeight::Medium,
            color: white(90),
        },
        TextRole::AttributeTitle => TextStyleToken {
            size: FONT_SIZE_11,
            weight: FontWeight::Normal,
            color: white(60),
        },
        TextRole::ActiveItemTitle => TextStyleToken {
            size: FONT_SIZE_13,
            weight: FontWeight::Medium,
            color: white(90),
        },
        TextRole::InactiveItemTitle => TextStyleToken {
            size: FONT_SIZE_13,
            weight: FontWeight::Normal,
            color: white(60),
        },
        TextRole::ValueLabel => TextStyleToken {
            size: FONT_SIZE_13,
            weight: FontWeight::Normal,
            color: white(80),
        },
    }
}

pub fn button_size_token(size: ButtonSize) -> ButtonSizeToken {
    match size {
        ButtonSize::ExtraSmall => ButtonSizeToken {
            height: 22.0,
            horizontal_padding: 8.0,
            vertical_padding: 3.0,
            font_size: FONT_SIZE_9,
            icon_size: FONT_SIZE_11,
        },
        ButtonSize::Small => ButtonSizeToken {
            height: 26.0,
            horizontal_padding: 10.0,
            vertical_padding: 4.0,
            font_size: FONT_SIZE_11,
            icon_size: FONT_SIZE_13,
        },
        ButtonSize::Default => ButtonSizeToken {
            height: CONTROL_ROW_HEIGHT,
            horizontal_padding: 12.0,
            vertical_padding: 5.0,
            font_size: FONT_SIZE_13,
            icon_size: BUTTON_ICON_SIZE_DEFAULT,
        },
        ButtonSize::Large => ButtonSizeToken {
            height: 34.0,
            horizontal_padding: 14.0,
            vertical_padding: 6.0,
            font_size: FONT_SIZE_15,
            icon_size: FONT_SIZE_15,
        },
    }
}

pub fn button_visual_token(variant: ButtonVariant) -> ButtonVisualToken {
    match variant {
        ButtonVariant::Default
        | ButtonVariant::Icon
        | ButtonVariant::WithIcon
        | ButtonVariant::Rounded
        | ButtonVariant::Spinner => ButtonVisualToken {
            bg: white(20),
            hover_bg: white(30),
            active_bg: white(40),
            text: white(90),
            border: white(30),
        },
        ButtonVariant::Secondary => ButtonVisualToken {
            bg: white(10),
            hover_bg: white(20),
            active_bg: white(30),
            text: white(90),
            border: white(20),
        },
        ButtonVariant::Outline => ButtonVisualToken {
            bg: Color32::TRANSPARENT,
            hover_bg: white(10),
            active_bg: white(20),
            text: white(90),
            border: white(40),
        },
        ButtonVariant::Ghost => ButtonVisualToken {
            bg: Color32::TRANSPARENT,
            hover_bg: white(10),
            active_bg: white(20),
            text: white(80),
            border: Color32::TRANSPARENT,
        },
        ButtonVariant::Destructive => ButtonVisualToken {
            bg: Color32::from_rgb(0x7a, 0x17, 0x17),
            hover_bg: Color32::from_rgb(0x94, 0x1f, 0x1f),
            active_bg: Color32::from_rgb(0xaa, 0x2a, 0x2a),
            text: white(100),
            border: Color32::from_rgb(0xc1, 0x45, 0x45),
        },
    }
}

pub fn button_corner_radius(variant: ButtonVariant) -> egui::CornerRadius {
    match variant {
        // ButtonVariant::Rounded => radius(24),
        _ => radius(BORDER_RADIUS_SMALL as u8),
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
        assert_eq!(
            radius(5),
            egui::CornerRadius::same(BORDER_RADIUS_SMALL as u8)
        );
        assert_eq!(radius(24), egui::CornerRadius::same(24));
        assert_eq!(radius(31), egui::CornerRadius::same(24));
    }

    #[test]
    fn semantic_text_roles_match_contract() {
        let section = text_style(TextRole::SectionTitle);
        assert_eq!(section.size, FONT_SIZE_13);
        assert_eq!(section.weight, FontWeight::Medium);
        assert_eq!(section.color, white(90));

        let attr = text_style(TextRole::AttributeTitle);
        assert_eq!(attr.size, FONT_SIZE_11);
        assert_eq!(attr.weight, FontWeight::Normal);
        assert_eq!(attr.color, white(60));
    }

    #[test]
    fn button_size_tokens_match_contract() {
        let xs = button_size_token(ButtonSize::ExtraSmall);
        assert_eq!(xs.height, 22.0);
        assert_eq!(xs.font_size, FONT_SIZE_9);

        let md = button_size_token(ButtonSize::Default);
        assert_eq!(md.height, CONTROL_ROW_HEIGHT);
        assert_eq!(md.icon_size, BUTTON_ICON_SIZE_DEFAULT);
    }

    #[test]
    fn button_variant_visuals_match_contract() {
        let ghost = button_visual_token(ButtonVariant::Ghost);
        assert_eq!(ghost.bg, Color32::TRANSPARENT);
        assert_eq!(ghost.border, Color32::TRANSPARENT);

        let destructive = button_visual_token(ButtonVariant::Destructive);
        assert_eq!(destructive.text, white(100));
        assert_ne!(destructive.bg, Color32::TRANSPARENT);
    }

    #[test]
    fn button_corner_radius_matches_variant() {
        assert_eq!(
            button_corner_radius(ButtonVariant::Default),
            radius(BORDER_RADIUS_SMALL as u8)
        );
        assert_eq!(
            button_corner_radius(ButtonVariant::Rounded),
            radius(BORDER_RADIUS_SMALL as u8)
        );
    }
}
