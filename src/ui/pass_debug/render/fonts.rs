use rust_wgpu_fiber::eframe::egui;

pub(crate) const PASS_DEBUG_TREE_FONT_SIZE: f32 = 13.0;
pub(crate) const PASS_DEBUG_CODE_FONT_SIZE: f32 = 13.0;
pub(crate) const PASS_DEBUG_LINE_NUMBER_FONT_SIZE: f32 = 11.5;

pub(crate) fn pass_debug_mono_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("geist_mono".into()))
}
