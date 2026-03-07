mod canvas;
mod frame;
mod interaction_report;
mod layout_math;
mod scene_runtime;
mod texture_bridge;
mod types;
mod window_mode;

pub use types::{
    AnalysisTab, App, AppInit, ClippingSettings, DiffMetricMode, DiffStats, RefImageMode,
    SampledPixel,
};

use rust_wgpu_fiber::eframe::{self, egui};

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        frame::run(self, ctx, frame);
    }
}
