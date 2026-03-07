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

use crate::{app::interaction_report, protocol::InteractionEventPayload};

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

pub(super) fn collect_interaction_events(
    app: &mut App,
    ctx: &egui::Context,
) -> Vec<InteractionEventPayload> {
    let frame_events = ctx.input(|i| i.events.clone());
    let interaction_clean_state = interaction_report::is_clean_rendering_state(
        app.canvas.display.preview_texture_name.is_some(),
        app.canvas.reference.ref_image.is_some(),
    );
    let canvas_rect = app
        .canvas
        .interactions
        .last_canvas_rect
        .unwrap_or_else(|| ctx.available_rect());
    let pointer_hover_pos = ctx.input(|i| i.pointer.hover_pos());

    let mut payloads = interaction_report::collect_interaction_payloads(
        frame_events.as_slice(),
        canvas_rect,
        pointer_hover_pos,
        interaction_clean_state,
        &mut app.canvas.interactions.canvas_event_focus_latched,
    );

    for payload in &mut payloads {
        app.canvas.interactions.interaction_event_seq = app
            .canvas
            .interactions
            .interaction_event_seq
            .saturating_add(1);
        payload.seq = app.canvas.interactions.interaction_event_seq;
        if let Some(session) = app.animation_session.as_mut() {
            session.fire_event(&payload.event_type);
        }
    }

    payloads
}

pub(super) fn show(
    app: &mut App,
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    frame: WindowModeFrame,
    now: f64,
    pre_collected_payloads: Vec<InteractionEventPayload>,
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
    presenter::show_canvas(
        app,
        ctx,
        ui,
        render_state,
        renderer,
        frame,
        now,
        pre_collected_payloads,
    )
}
