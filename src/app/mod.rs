mod canvas;
pub(crate) mod display_metrics;
mod frame;
mod interaction_report;
mod layout_math;
pub(crate) mod matrix_render;
mod scene_runtime;
mod texture_bridge;
mod types;
mod window_mode;

pub use types::{
    AnalysisTab, App, AppInit, ClippingSettings, DiffMetricMode, DiffStats, QualifierChannel,
    QualifierSettings, RefImageMode, ResourcePoolInfo, SampledPixel, TestMode,
};

use rust_wgpu_fiber::eframe::{self, egui};

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        frame::run(self, ui, frame);
    }
}
