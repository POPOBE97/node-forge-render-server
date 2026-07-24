mod canvas;
pub(crate) mod display_metrics;
mod frame;
mod input_scope;
mod interaction_report;
mod layout_math;
pub(crate) mod matrix_render;
mod scene_runtime;
mod texture_bridge;
mod types;
mod window_mode;

pub use types::{
    AnalysisTab, App, AppInit, ClippingSettings, DiffMetricMode, DiffStats, QualifierChannel,
    QualifierSettings, RefImageAlphaMode, RefImageMode, ResourcePoolInfo, SampledPixel,
    ShortwirePastedReferenceImage, ShortwireReferenceImage, StateControlSelection, TestMode,
};

use rust_wgpu_fiber::eframe::{self, egui};

pub fn default_main_window_size(window_resolution: [u32; 2]) -> egui::Vec2 {
    window_mode::sidebar_window_size(window_resolution, crate::ui::debug_sidebar::SIDEBAR_WIDTH)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        frame::run(self, ui, frame);
    }
}
