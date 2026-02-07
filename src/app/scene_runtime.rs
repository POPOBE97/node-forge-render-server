use std::sync::Arc;

use rust_wgpu_fiber::eframe::{egui, egui_wgpu, wgpu};

use crate::{protocol, renderer, ws};

use super::types::App;

pub struct SceneApplyResult {
    pub did_rebuild_shader_space: bool,
    pub texture_filter_override: Option<wgpu::FilterMode>,
}

pub fn drain_latest_scene_update(app: &App) -> Option<ws::SceneUpdate> {
    let mut latest: Option<ws::SceneUpdate> = None;
    while let Ok(update) = app.scene_rx.try_recv() {
        latest = Some(update);
    }
    latest
}

pub fn apply_scene_resolution_to_window_state(
    current_window_resolution: [u32; 2],
    scene_screen_resolution: Option<[u32; 2]>,
    follow_scene_resolution_for_window: bool,
) -> ([u32; 2], Option<[f32; 2]>) {
    let Some([w, h]) = scene_screen_resolution else {
        return (current_window_resolution, None);
    };

    if [w, h] == current_window_resolution {
        return (current_window_resolution, None);
    }

    let next_window_resolution = [w, h];
    let maybe_resize = if follow_scene_resolution_for_window {
        Some([w as f32, h as f32])
    } else {
        None
    };

    (next_window_resolution, maybe_resize)
}

pub fn apply_scene_update(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    update: ws::SceneUpdate,
) -> SceneApplyResult {
    match update {
        ws::SceneUpdate::Parsed { scene, request_id } => {
            let (next_window_resolution, maybe_resize) = apply_scene_resolution_to_window_state(
                app.window_resolution,
                crate::dsl::screen_resolution(&scene),
                app.follow_scene_resolution_for_window,
            );
            app.window_resolution = next_window_resolution;

            if let Some([w, h]) = maybe_resize {
                let size = egui::vec2(w, h);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(size));
            }

            let build_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                renderer::ShaderSpaceBuilder::new(
                    Arc::new(render_state.device.clone()),
                    Arc::new(render_state.queue.clone()),
                )
                .with_options(renderer::ShaderSpaceBuildOptions {
                    presentation_mode: renderer::ShaderSpacePresentationMode::UiSdrDisplayEncode,
                    debug_dump_wgsl_dir: None,
                })
                .build(&scene)
            }));

            match build_result {
                Ok(Ok(result)) => {
                    app.shader_space = result.shader_space;
                    app.resolution = result.resolution;
                    app.passes = result.pass_bindings;
                    app.output_texture_name = result.present_output_texture;

                    if let Ok(mut g) = app.last_good.lock() {
                        *g = Some(scene);
                    }

                    SceneApplyResult {
                        did_rebuild_shader_space: true,
                        texture_filter_override: None,
                    }
                }
                Ok(Err(e)) => {
                    let message = format!("{e:#}");
                    eprintln!("[error-plane] scene build failed: {message}");
                    broadcast_error(app, request_id, "VALIDATION_ERROR", message);
                    apply_error_plane(app, render_state);
                    SceneApplyResult {
                        did_rebuild_shader_space: true,
                        texture_filter_override: None,
                    }
                }
                Err(panic_payload) => {
                    let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                        (*s).to_string()
                    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "(non-string panic payload)".to_string()
                    };
                    let message = format!("scene build panicked; showing error plane: {panic_msg}");
                    eprintln!("{message}");
                    broadcast_error(app, request_id, "PANIC", message);
                    apply_error_plane(app, render_state);
                    SceneApplyResult {
                        did_rebuild_shader_space: true,
                        texture_filter_override: Some(wgpu::FilterMode::Linear),
                    }
                }
            }
        }
        ws::SceneUpdate::ParseError {
            message,
            request_id,
        } => {
            eprintln!("[error-plane] scene parse error: {message}");
            broadcast_error(app, request_id, "PARSE_ERROR", message);
            apply_error_plane(app, render_state);
            SceneApplyResult {
                did_rebuild_shader_space: true,
                texture_filter_override: None,
            }
        }
    }
}

fn apply_error_plane(app: &mut App, render_state: &egui_wgpu::RenderState) {
    if let Ok(result) = renderer::ShaderSpaceBuilder::new(
        Arc::new(render_state.device.clone()),
        Arc::new(render_state.queue.clone()),
    )
    .build_error(app.resolution)
    {
        app.shader_space = result.shader_space;
        app.resolution = result.resolution;
        app.output_texture_name = result.present_output_texture;
        app.passes = result.pass_bindings;
    }
}

fn broadcast_error(app: &App, request_id: Option<String>, code: &str, message: String) {
    let msg = protocol::WSMessage {
        msg_type: "error".to_string(),
        timestamp: protocol::now_millis(),
        request_id,
        payload: Some(protocol::ErrorPayload {
            code: code.to_string(),
            message,
        }),
    };
    if let Ok(text) = serde_json::to_string(&msg) {
        app.ws_hub.broadcast(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_scene_resolution_updates_window_state_without_forcing_resize_by_default() {
        let (next, resize) =
            apply_scene_resolution_to_window_state([1024, 1024], Some([800, 600]), false);
        assert_eq!(next, [800, 600]);
        assert_eq!(resize, None);
    }

    #[test]
    fn apply_scene_resolution_can_request_resize_when_enabled() {
        let (next, resize) =
            apply_scene_resolution_to_window_state([1024, 1024], Some([800, 600]), true);
        assert_eq!(next, [800, 600]);
        assert_eq!(resize, Some([800.0, 600.0]));
    }

    #[test]
    fn apply_scene_resolution_is_noop_when_same_or_missing() {
        let (next, resize) =
            apply_scene_resolution_to_window_state([800, 600], Some([800, 600]), true);
        assert_eq!(next, [800, 600]);
        assert_eq!(resize, None);

        let (next, resize) = apply_scene_resolution_to_window_state([800, 600], None, true);
        assert_eq!(next, [800, 600]);
        assert_eq!(resize, None);
    }
}
