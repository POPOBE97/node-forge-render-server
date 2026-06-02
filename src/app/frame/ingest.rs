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
                if app.runtime.force_continuous_redraw {
                    eprintln!(
                        "[capture] metal capture started; continuous redraw already forced by CLI flag"
                    );
                } else {
                    eprintln!("[capture] enabling continuous redraw for active capture session");
                }
            } else {
                if app.runtime.force_continuous_redraw {
                    eprintln!(
                        "[capture] metal capture stopped; CLI-forced continuous redraw remains enabled"
                    );
                } else {
                    eprintln!("[capture] disabling continuous redraw after capture session");
                }
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

        if let Some(ref scene) = app.runtime.uniform_scene {
            app.shell.resource_pools = crate::app::types::extract_resource_pools(scene);
            app.shell
                .matrix_config
                .selected_pool_ids
                .retain(|id| app.shell.resource_pools.iter().any(|p| p.node_id == *id));
        }

        if app.shell.test_mode == crate::app::types::TestMode::Matrix
            && !app.shell.matrix_config.selected_pool_ids.is_empty()
        {
            if let Some(ref scene) = app.runtime.uniform_scene {
                let params = crate::app::matrix_render::MatrixBuildParams {
                    scene,
                    config: &app.shell.matrix_config,
                    resource_pools: &app.shell.resource_pools,
                    device: std::sync::Arc::new(render_state.device.clone()),
                    queue: std::sync::Arc::new(render_state.queue.clone()),
                    adapter: Some(&render_state.adapter),
                    asset_store: &app.core.asset_store,
                };
                if let Err(e) = crate::app::matrix_render::rebuild_matrix(
                    params,
                    render_state,
                    renderer,
                    &mut app.shell.matrix_state,
                ) {
                    eprintln!("[matrix] rebuild on scene update failed: {e:#}");
                }
                if app.canvas.display.hdr_preview_clamp_enabled {
                    crate::app::matrix_render::sync_matrix_hdr_clamp(
                        &mut app.shell.matrix_state,
                        render_state,
                        renderer,
                        true,
                        app.canvas.display.texture_filter,
                    );
                }
            }
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
