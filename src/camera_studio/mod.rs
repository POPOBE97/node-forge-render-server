//! Native camera acquisition workspace, independent of SceneDSL and animation evaluation.
mod access;
mod gphoto;
pub mod model;
pub mod service;
mod ui;

use rust_wgpu_fiber::eframe::{self, egui};
use std::sync::{Arc, Mutex};

pub use gphoto::GPhoto;
pub use model::*;
pub use service::CameraService;

/// A connection-only diagnostic: never starts preview, exposure, focus, recording or download.
pub fn check_connection(
    release_occupied: bool,
    notice: impl FnMut(String),
) -> CameraResult<CameraInfo> {
    let mut backend = GPhoto::load()?;
    let info = access::connect_with_recovery(
        &mut backend,
        release_occupied,
        access::release_camera_clients,
        notice,
    )?;
    backend.disconnect()?;
    Ok(info)
}

const VIEWPORT: &str = "camera-studio";

pub struct ToolWindow {
    workspace: Arc<Mutex<ui::Workspace>>,
    focus: bool,
    initialized: bool,
}

pub fn open_tool_window(window: &mut Option<ToolWindow>) {
    if let Some(window) = window {
        window.focus = true;
        return;
    }
    *window = Some(ToolWindow {
        workspace: Arc::new(Mutex::new(ui::Workspace::new())),
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
    let Some(w) = window else {
        return;
    };
    if ctx.input(|i| i.viewport().close_requested()) {
        if let Ok(mut state) = w.workspace.lock() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            state.request_close(Some(ctx.viewport_id()));
            w.focus = true;
        }
    }
    let state = w.workspace.clone();
    let id = egui::ViewportId::from_hash_of(VIEWPORT);
    let mut builder = egui::ViewportBuilder::default()
        .with_title("Camera Studio — Node Forge")
        .with_min_inner_size(egui::vec2(1000.0, 680.0));
    if !w.initialized {
        builder = builder.with_inner_size(egui::vec2(1400.0, 900.0));
        w.initialized = true;
    }
    ctx.show_viewport_deferred(id, builder, move |ui, class| {
        if let Ok(mut state) = state.lock() {
            if class == egui::ViewportClass::EmbeddedWindow {
                let mut open = true;
                egui::Window::new("Camera Studio")
                    .open(&mut open)
                    .default_size(egui::vec2(1200.0, 800.0))
                    .show(ui.ctx(), |ui| state.show(ui));
                if !open {
                    state.request_close(None);
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
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.workspace.show(ui);
    }
}

pub fn run_standalone(review: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    run_window(review, None)
}

/// Render the real window with isolated sample data. No camera service is created.
pub fn run_ui_preview(path: std::path::PathBuf, state: String) -> anyhow::Result<()> {
    run_window(None, Some((path, state)))
}

fn run_window(
    review: Option<std::path::PathBuf>,
    fixture: Option<(std::path::PathBuf, String)>,
) -> anyhow::Result<()> {
    eframe::run_native(
        "Camera Studio",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("Camera Studio — Node Forge")
                .with_inner_size(egui::vec2(1400.0, 900.0))
                .with_min_inner_size(egui::vec2(1000.0, 680.0)),
            renderer: eframe::Renderer::Wgpu,
            ..Default::default()
        },
        Box::new(move |cc| {
            crate::ui::fonts::configure_egui_fonts(&cc.egui_ctx);
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            let mut workspace = if let Some((path, state)) = fixture {
                ui::Workspace::preview_fixture(&cc.egui_ctx, path, &state)?
            } else {
                ui::Workspace::new()
            };
            if let Some(path) = review {
                workspace.open_photo(path);
            }
            Ok(Box::new(Standalone { workspace }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("Camera Studio: {e}"))
}
