mod advance;
pub(super) mod commands;
mod finalize;
mod ingest;
mod interaction_bridge;
mod present;
mod render_analysis;
pub(super) mod request_keys;

use rust_wgpu_fiber::eframe::{self, egui};

use crate::app::types::App;

fn apply_dark_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(egui::Color32::from_rgba_unmultiplied(255, 255, 255, 204));
    ctx.set_visuals(visuals);
}

pub(super) fn run(app: &mut App, ctx: &egui::Context, frame: &mut eframe::Frame) {
    apply_dark_visuals(ctx);

    let frame_time = ctx.input(|input| input.time);
    let render_state = frame.wgpu_render_state().unwrap();
    let mut renderer_guard = frame.wgpu_render_state().unwrap().renderer.as_ref().write();

    let ingest = ingest::run(app, ctx, render_state, &mut renderer_guard, frame_time);
    let advance = advance::run(app);
    render_analysis::run(app, render_state, &mut renderer_guard, &ingest, &advance);
    let present = present::run(app, ctx, render_state, &mut renderer_guard, &ingest);
    finalize::run(app, ctx, &advance, &present);
}
