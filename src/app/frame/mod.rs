mod advance;
pub(super) mod commands;
mod finalize;
mod ingest;
mod interaction_bridge;
mod present;
mod render_analysis;
pub(super) mod request_keys;

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use rust_wgpu_fiber::eframe::{self, egui};

use crate::app::{
    matrix_render,
    types::{App, TestMode},
};
use crate::metric_log;
use crate::perf_log::FrameTimer;

static SHORTWIRE_PASTE_DEBUG_MARKER_PRINTED: AtomicBool = AtomicBool::new(false);

fn apply_dark_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(egui::Color32::from_rgba_unmultiplied(255, 255, 255, 204));
    ctx.set_visuals(visuals);
}

pub(super) fn run(app: &mut App, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
    if !SHORTWIRE_PASTE_DEBUG_MARKER_PRINTED.swap(true, Ordering::Relaxed) {
        eprintln!("[shortwire-paste] debug instrumentation active");
    }

    let timer = FrameTimer::new();
    let ctx = ui.ctx().clone();
    apply_dark_visuals(&ctx);

    let frame_time = ctx.input(|input| input.time);
    let render_state = frame.wgpu_render_state().unwrap();
    let mut renderer_guard = render_state.renderer.as_ref().write();

    let t0 = Instant::now();
    let ingest = ingest::run(app, &ctx, render_state, &mut renderer_guard, frame_time);
    interaction_bridge::broadcast_payloads(app, &ingest.queued_interaction_payloads);
    let ingest_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t1 = Instant::now();
    let matrix_poll = if app.shell.test_mode == TestMode::Matrix {
        matrix_render::poll_matrix_rebuild(
            &mut app.shell.matrix_state,
            render_state,
            &mut renderer_guard,
            app.canvas.display.texture_filter,
            app.canvas.display.hdr_preview_clamp_enabled,
        )
    } else {
        matrix_render::MatrixPollResult::default()
    };
    let advance = advance::run(app);
    let advance_ms = t1.elapsed().as_secs_f64() * 1000.0;

    let t2 = Instant::now();
    render_analysis::run(
        app,
        render_state,
        &mut renderer_guard,
        &ingest,
        &advance,
        matrix_poll.added_cells > 0,
    );
    let analysis_ms = t2.elapsed().as_secs_f64() * 1000.0;

    let t3 = Instant::now();
    let present = present::run(app, ui, &ctx, render_state, &mut renderer_guard, &ingest);
    let present_ms = t3.elapsed().as_secs_f64() * 1000.0;

    let t4 = Instant::now();
    finalize::run(app, &ctx, &advance, &present);
    let finalize_ms = t4.elapsed().as_secs_f64() * 1000.0;

    let total_ms = timer.elapsed_ms();
    metric_log!(
        "[frame] #{} total={:.2}ms | ingest={:.2}ms advance={:.2}ms analysis={:.2}ms present={:.2}ms finalize={:.2}ms",
        timer.frame_number(),
        total_ms,
        ingest_ms,
        advance_ms,
        analysis_ms,
        present_ms,
        finalize_ms,
    );
}
