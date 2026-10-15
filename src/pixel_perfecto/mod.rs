//! Pixel Perfecto is an independent image-registration workspace, not a SceneDSL editor.
pub mod calibration;
pub mod detection;
mod gpu;
mod grid;
mod measure;
mod rulers;
pub mod io;
pub mod model;
mod ui;

pub use calibration::CalibrationResult;
pub use model::{ImageLayer, PixelPerfectoDocument};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use rust_wgpu_fiber::eframe::{self, egui, egui_wgpu, wgpu};

pub struct ToolWindow {
    workspace: Arc<Mutex<ui::Workspace>>,
    focus: bool,
    initialized: bool,
}
const VIEWPORT: &str = "pixel-perfecto";
pub fn open_tool_window(window: &mut Option<ToolWindow>, state: &egui_wgpu::RenderState) {
    if let Some(w) = window.as_mut() {
        w.focus = true;
        return;
    }
    *window = Some(ToolWindow {
        workspace: Arc::new(Mutex::new(ui::Workspace::new(state.clone()))),
        focus: true,
        initialized: false,
    });
}
pub fn show_tool_window(ctx: &egui::Context, window: &mut Option<ToolWindow>) {
    if window
        .as_ref()
        .is_some_and(|w| w.workspace.lock().is_ok_and(|s| s.closed))
    {
        *window = None;
        return;
    }
    let Some(w) = window.as_mut() else {
        return;
    };
    if ctx.input(|i| i.viewport().close_requested()) {
        if let Ok(mut state) = w.workspace.lock() {
            if state.needs_close_confirmation() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                state.request_parent_close();
                w.focus = true;
            }
        }
    }
    let state = w.workspace.clone();
    let id = egui::ViewportId::from_hash_of(VIEWPORT);
    let mut builder = egui::ViewportBuilder::default()
        .with_title("Pixel Perfecto")
        .with_min_inner_size(egui::vec2(980., 620.));
    if !w.initialized {
        builder = builder.with_inner_size(egui::vec2(1440., 900.));
        w.initialized = true;
    }
    ctx.show_viewport_deferred(id, builder, move |ui, class| {
        if let Ok(mut state) = state.lock() {
            if class == egui::ViewportClass::EmbeddedWindow {
                let mut open = true;
                egui::Window::new("Pixel Perfecto")
                    .open(&mut open)
                    .default_size(egui::vec2(1200., 800.))
                    .show(ui.ctx(), |ui| state.show(ui));
                if !open {
                    state.request_close();
                }
            } else {
                state.show(ui);
            }
        }
    });
    if w.focus {
        ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        w.focus = false;
    }
}

struct Standalone {
    workspace: ui::Workspace,
}
impl eframe::App for Standalone {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.workspace.show(ui);
    }
}
pub fn run_standalone(paths: Vec<PathBuf>) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Pixel Perfecto")
            .with_inner_size(egui::vec2(1440., 900.))
            .with_min_inner_size(egui::vec2(980., 620.)),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            preferred_surface_format: Some(wgpu::TextureFormat::Rgba16Float),
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "Pixel Perfecto",
        options,
        Box::new(move |cc| {
            crate::ui::fonts::configure_egui_fonts(&cc.egui_ctx);
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            let mut workspace = ui::Workspace::new(
                cc.wgpu_render_state
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("GPU renderer unavailable"))?,
            );
            workspace.open_paths(paths, cc.egui_ctx.clone());
            Ok(Box::new(Standalone { workspace }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("Pixel Perfecto: {e}"))
}
