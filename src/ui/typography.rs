use rust_wgpu_fiber::eframe::egui::{self, FontFamily, FontId, RichText, Ui};

/// Map a CSS-like numeric font weight to a specific MiSans font face.
///
/// Notes:
/// - egui doesn't have variable font-weight; weight selection is done by choosing
///   a specific font family/name.
/// - Your `configure_egui_fonts` registers these names (MiSans-*) into egui.
pub fn mi_sans_family_for_weight(weight: f32) -> FontFamily {
    let name = if weight >= 900.0 {
        "MiSans-Heavy"
    } else if weight >= 700.0 {
        "MiSans-Bold"
    } else if weight >= 600.0 {
        "MiSans-Demibold"
    } else if weight >= 500.0 {
        "MiSans-Medium"
    } else if weight >= 400.0 {
        // Prefer Regular for typical body text.
        "MiSans-Regular"
    } else if weight >= 300.0 {
        "MiSans-Light"
    } else if weight >= 200.0 {
        "MiSans-ExtraLight"
    } else {
        "MiSans-Thin"
    };

    FontFamily::Name(name.into())
}

/// Create RichText with CSS-like weight + size.
pub fn text(text: impl Into<String>, weight: f32, size: f32) -> RichText {
    let font_id = FontId::new(size, mi_sans_family_for_weight(weight));
    RichText::new(text).font(font_id)
}

/// Convenience: `label("text", 600.0, 16.0)`.
pub fn label(ui: &mut Ui, text: impl Into<String>, weight: f32, size: f32) -> egui::Response {
    ui.label(self::text(text, weight, size))
}
