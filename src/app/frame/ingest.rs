use rust_wgpu_fiber::eframe::{egui, egui_wgpu};

use crate::{
    app::{canvas, scene_runtime, texture_bridge, types::App},
    protocol,
};

use super::interaction_bridge;

pub(super) struct IngestPhase {
    pub frame_time: f64,
    pub queued_interaction_payloads: Vec<protocol::InteractionEventPayload>,
    pub did_rebuild_shader_space: bool,
}

pub(super) fn run(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    frame_time: f64,
) -> IngestPhase {
    let mut latest_capture_state = None;
    if let Some(capture_state_rx) = app.runtime.capture_state_rx.as_ref() {
        while let Ok(capture_active) = capture_state_rx.try_recv() {
            latest_capture_state = Some(capture_active);
        }
    }
    if let Some(capture_active) = latest_capture_state {
        if app.runtime.capture_redraw_active != capture_active {
            if capture_active {
                eprintln!("[capture] enabling continuous redraw for active capture session");
            } else {
                eprintln!("[capture] disabling continuous redraw after capture session");
            }
        }
        app.runtime.capture_redraw_active = capture_active;
        if capture_active {
            app.runtime.scene_redraw_pending = true;
        }
    }

    let mut did_rebuild_shader_space = false;
    if let Some(update) = scene_runtime::drain_latest_scene_update(app) {
        let apply_result = scene_runtime::apply_scene_update(app, ctx, render_state, update);
        app.runtime.scene_redraw_pending = true;
        app.canvas.invalidation.preview_source_changed();
        if apply_result.did_rebuild_shader_space {
            let filter = apply_result
                .texture_filter_override
                .unwrap_or(app.canvas.display.texture_filter);
            let texture_name = app.core.output_texture_name.clone();
            texture_bridge::sync_output_texture(app, render_state, renderer, &texture_name, filter);
            did_rebuild_shader_space = true;
        }
        if apply_result.reset_viewport {
            app.canvas.viewport.pending_view_reset = true;
        }
    }

    canvas::sync_reference_from_scene(app, ctx, render_state);

    IngestPhase {
        frame_time,
        queued_interaction_payloads: interaction_bridge::collect_early_canvas_interactions(
            app, ctx,
        ),
        did_rebuild_shader_space,
    }
}
