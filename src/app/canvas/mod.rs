pub mod actions;
pub mod display;
pub mod ops;
pub mod pixel_overlay;
pub mod presenter;
pub mod reducer;
pub mod reference;
pub mod state;
pub mod viewport;

use rust_wgpu_fiber::eframe::{egui, egui_wgpu};

use super::{types::App, window_mode::WindowModeFrame};

pub(super) fn is_pan_zoom_animating(app: &App) -> bool {
    viewport::is_pan_zoom_animating(app)
}

pub(super) fn sync_reference_from_scene(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
) {
    reference::sync_from_scene(app, ctx, render_state);
}

pub(super) fn clear_reference(app: &mut App) {
    reference::clear_reference(app);
}

pub(super) fn pick_reference_image_from_dialog(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
) -> anyhow::Result<bool> {
    reference::pick_reference_image_from_dialog(app, ctx, render_state)
}

pub(super) fn show(
    app: &mut App,
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    frame: WindowModeFrame,
    now: f64,
) -> actions::CanvasFrameResult {
    display::flush_deferred_frees(app, renderer);
    if let Err(err) = reducer::apply_action(
        app,
        render_state,
        renderer,
        actions::CanvasAction::PollClipboardOp { now },
    ) {
        eprintln!("[canvas] failed to poll clipboard op: {err:#}");
    }
    presenter::show_canvas(app, ctx, ui, render_state, renderer, frame, now)
}
