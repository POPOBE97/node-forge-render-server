use std::time::Instant;

use rust_wgpu_fiber::eframe::{egui, egui_wgpu};

use crate::{
    app::{canvas, input_scope, scene_runtime, texture_bridge, types::App},
    protocol,
    ui::pass_debug_window,
};

use super::interaction_bridge;

pub(super) struct IngestPhase {
    pub frame_time: f64,
    pub queued_interaction_payloads: Vec<protocol::InteractionEventPayload>,
    pub did_rebuild_shader_space: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ClipboardPasteSignals {
    command_v_pressed: bool,
    command_v_released: bool,
    v_released: bool,
    paste_event: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ClipboardPasteInput {
    requested: bool,
    consume_v_key_events: bool,
}

fn clipboard_paste_signals(events: &[egui::Event]) -> ClipboardPasteSignals {
    let mut signals = ClipboardPasteSignals::default();
    for event in events {
        match event {
            egui::Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                modifiers,
            } if *key == egui::Key::V || *physical_key == Some(egui::Key::V) => {
                if *pressed && !*repeat && modifiers.command {
                    signals.command_v_pressed = true;
                } else if !*pressed {
                    signals.v_released = true;
                    signals.command_v_released |= modifiers.command;
                }
            }
            egui::Event::Paste(_) => signals.paste_event = true,
            _ => {}
        }
    }
    signals
}

fn projected_canvas_focus_latched(
    events: &[egui::Event],
    canvas_rect: egui::Rect,
    mut focus_latched: bool,
) -> bool {
    for event in events {
        match event {
            egui::Event::PointerButton { pos, pressed, .. } if *pressed => {
                focus_latched = canvas_rect.contains(*pos);
            }
            egui::Event::WindowFocused(false) => focus_latched = false,
            _ => {}
        }
    }
    focus_latched
}

fn canvas_clipboard_paste_scope_enabled(
    window_focused: bool,
    text_edit_focused: bool,
    pointer_pos: Option<egui::Pos2>,
    canvas_rect: Option<egui::Rect>,
    focus_latched: bool,
) -> bool {
    window_focused
        && !text_edit_focused
        && (focus_latched
            || pointer_pos
                .zip(canvas_rect)
                .is_some_and(|(position, rect)| rect.contains(position)))
}

fn reduce_clipboard_paste_gesture(
    signals: ClipboardPasteSignals,
    suppress_next_release: &mut bool,
) -> bool {
    let requested = signals.command_v_pressed
        || (signals.paste_event && !*suppress_next_release)
        || (signals.command_v_released && !*suppress_next_release);

    if signals.command_v_pressed || signals.paste_event {
        *suppress_next_release = true;
    }
    if signals.v_released {
        *suppress_next_release = false;
    }
    requested
}

fn clipboard_paste_input(app: &mut App, ctx: &egui::Context) -> ClipboardPasteInput {
    let (events, window_focused, pointer_pos) = ctx.input(|input| {
        (
            input.events.clone(),
            input.raw.focused,
            input.pointer.hover_pos(),
        )
    });
    let signals = clipboard_paste_signals(&events);
    let canvas_rect = app.canvas.interactions.last_canvas_rect;
    let projected_focus =
        canvas_rect.map_or(app.canvas.interactions.canvas_event_focus_latched, |rect| {
            projected_canvas_focus_latched(
                &events,
                rect,
                app.canvas.interactions.canvas_event_focus_latched,
            )
        });
    let scope_enabled = canvas_clipboard_paste_scope_enabled(
        window_focused,
        app.shell.command_palette.captures_input() || ctx.text_edit_focused(),
        pointer_pos,
        canvas_rect,
        projected_focus,
    );
    let release_of_captured_gesture = signals.v_released
        && app
            .canvas
            .interactions
            .suppress_next_clipboard_paste_release;
    let consume_v_key_events = release_of_captured_gesture
        || (scope_enabled
            && (signals.command_v_pressed || signals.command_v_released || signals.paste_event));

    if !scope_enabled {
        if signals.v_released {
            app.canvas
                .interactions
                .suppress_next_clipboard_paste_release = false;
        }
        return ClipboardPasteInput {
            requested: false,
            consume_v_key_events,
        };
    }

    let requested = reduce_clipboard_paste_gesture(
        signals,
        &mut app
            .canvas
            .interactions
            .suppress_next_clipboard_paste_release,
    );
    ClipboardPasteInput {
        requested,
        consume_v_key_events,
    }
}

pub(super) fn handle_clipboard_reference_paste(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
) {
    let shortwire_active = pass_debug_window::has_active_shortwire(&app.shell.pass_debug_windows);
    match canvas::reference::paste_reference_from_clipboard(
        app,
        ctx,
        render_state,
        shortwire_active,
    ) {
        Ok(Some(mut paste)) => {
            if let Some(shortwire_reference) = paste.shortwire_reference.take() {
                let patch_result = pass_debug_window::request_active_shortwire_diff_capture(
                    &mut app.shell.pass_debug_windows,
                    Some(shortwire_reference),
                );
                let Some(diff_capture) = patch_result.diff_capture else {
                    eprintln!(
                        "[reference-paste] Shortwire rejected clipboard reference; preserving existing Canvas Ref"
                    );
                    return;
                };

                canvas::reference::commit_clipboard_reference_paste(app, paste);
                eprintln!(
                    "[shortwire-diff] paste_queue_capture pass={} patch_key={}",
                    diff_capture.pass_name, diff_capture.patch_key,
                );
                app.shell.pending_shortwire_diff_capture = Some(diff_capture);
                app.canvas.invalidation.mark_diff_dirty();

                let mut should_persist_debug_artifacts = false;
                for (item, content_text) in patch_result.artifacts {
                    app.shell
                        .debug_artifacts
                        .upsert(item.clone(), Some(content_text.clone()));
                    crate::ws::broadcast_debug_artifact_upsert(
                        &app.core.ws_hub,
                        item,
                        Some(content_text),
                    );
                    should_persist_debug_artifacts = true;
                }
                for (item, bytes) in patch_result.binary_artifacts {
                    app.shell
                        .debug_artifacts
                        .upsert_bytes(item.clone(), bytes.clone());
                    crate::ws::broadcast_debug_artifact_binary_upload(
                        &app.core.ws_hub,
                        item,
                        bytes,
                    );
                    should_persist_debug_artifacts = true;
                }
                if should_persist_debug_artifacts {
                    app.persist_debug_artifacts_to_source_nforge();
                }
            } else {
                canvas::reference::commit_clipboard_reference_paste(app, paste);
            }
            ctx.request_repaint();
        }
        Ok(None) => {}
        Err(error) => eprintln!("[reference-paste] failed to paste clipboard image: {error:#}"),
    }
}

#[derive(Clone, Copy)]
enum AndroidScreencapClipboardShortcut {
    SourceProfile,
    FigmaSrgb,
}

fn android_screencap_clipboard_shortcut_pressed(
    ctx: &egui::Context,
) -> Option<AndroidScreencapClipboardShortcut> {
    ctx.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                modifiers,
            } if *pressed
                && !*repeat
                && modifiers.command
                && modifiers.alt
                && !modifiers.shift
                && !modifiers.ctrl =>
            {
                match (*key, *physical_key) {
                    (egui::Key::Num3, _) | (_, Some(egui::Key::Num3)) => {
                        Some(AndroidScreencapClipboardShortcut::SourceProfile)
                    }
                    (egui::Key::Num4, _) | (_, Some(egui::Key::Num4)) => {
                        Some(AndroidScreencapClipboardShortcut::FigmaSrgb)
                    }
                    _ => None,
                }
            }
            _ => None,
        })
    })
}

pub(super) fn run(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    frame_time: f64,
) -> IngestPhase {
    let screencap_shortcut = input_scope::debug_shortcuts_enabled(app, ctx)
        .then(|| android_screencap_clipboard_shortcut_pressed(ctx))
        .flatten();
    if let Some(shortcut) = screencap_shortcut {
        match shortcut {
            AndroidScreencapClipboardShortcut::SourceProfile => {
                canvas::ops::begin_android_screencap_clipboard_copy(
                    &mut app.canvas.async_ops,
                    frame_time,
                );
            }
            AndroidScreencapClipboardShortcut::FigmaSrgb => {
                canvas::ops::begin_android_screencap_srgb_clipboard_copy(
                    &mut app.canvas.async_ops,
                    frame_time,
                );
            }
        }
        ctx.request_repaint();
    }

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
    if let Some(update) = scene_runtime::next_scene_update(app) {
        let perf_trace = update.perf_trace().cloned();
        let perf_update_kind = update.perf_update_kind();
        let renderer_queue_ms = perf_trace
            .as_ref()
            .map(|trace| trace.enqueued_at.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        let apply_started_at = Instant::now();
        let apply_result = scene_runtime::apply_scene_update(app, ctx, render_state, update);
        let renderer_apply_ms = apply_started_at.elapsed().as_secs_f64() * 1000.0;
        let rebuild_reason =
            apply_result
                .did_rebuild_shader_space
                .then_some(match perf_update_kind {
                    "scene_delta" => "scene_delta_pipeline_signature_changed",
                    "scene_update" => "full_scene_pipeline_signature_changed",
                    "uniform_delta" => "uniform_fast_path_fallback",
                    _ => "scene_update_rebuild",
                });
        if let Some(trace) = perf_trace {
            crate::ws::broadcast_scene_perf_trace(
                &app.core.ws_hub,
                trace,
                renderer_queue_ms,
                renderer_apply_ms,
                apply_result.did_rebuild_shader_space,
                rebuild_reason,
                perf_update_kind,
            );
        }
        let matrix_update = apply_result.matrix_update.clone();
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

        if let Some(ref scene) = app.runtime.matrix_source_scene {
            app.shell.resource_pools = crate::app::types::extract_resource_pools(scene);
            app.shell
                .matrix_config
                .selected_pool_ids
                .retain(|id| app.shell.resource_pools.iter().any(|p| p.node_id == *id));
        }

        if app.shell.test_mode == crate::app::types::TestMode::Matrix
            && !app.shell.matrix_config.selected_pool_ids.is_empty()
        {
            if let Some(ref scene) = app.runtime.matrix_source_scene {
                let params = crate::app::matrix_render::MatrixBuildParams {
                    scene,
                    config: &app.shell.matrix_config,
                    resource_pools: &app.shell.resource_pools,
                    device: std::sync::Arc::new(render_state.device.clone()),
                    queue: std::sync::Arc::new(render_state.queue.clone()),
                    adapter: Some(&render_state.adapter),
                    asset_store: &app.core.asset_store,
                };
                let refresh_failed = match matrix_update {
                    scene_runtime::MatrixSceneUpdate::UniformOnly { updated_nodes } => {
                        match crate::app::matrix_render::refresh_matrix_cells_uniform_only(
                            &mut app.shell.matrix_state,
                            &updated_nodes,
                            render_state,
                            renderer,
                            app.canvas.display.texture_filter,
                            app.canvas.display.hdr_preview_clamp_enabled,
                        ) {
                            Ok(crate::app::matrix_render::MatrixUniformRefreshResult::Refreshed) => {
                                false
                            }
                            Ok(crate::app::matrix_render::MatrixUniformRefreshResult::NeedsFullRebuild) => true,
                            Err(e) => {
                                eprintln!(
                                    "[matrix] uniform-only refresh failed; falling back to rebuild: {e:#}"
                                );
                                true
                            }
                        }
                    }
                    scene_runtime::MatrixSceneUpdate::FullRebuild => true,
                    scene_runtime::MatrixSceneUpdate::None => false,
                };

                if refresh_failed
                    && let Err(e) = crate::app::matrix_render::start_matrix_rebuild(
                        params,
                        renderer,
                        &mut app.shell.matrix_state,
                    )
                {
                    eprintln!("[matrix] rebuild on scene update failed: {e:#}");
                }
            }
        }
    }

    canvas::sync_reference_from_scene(app, ctx, render_state);
    canvas::sync_android_reference_frame(app, ctx, render_state);

    let paste_input = clipboard_paste_input(app, ctx);
    if paste_input.requested {
        handle_clipboard_reference_paste(app, ctx, render_state);
    }

    IngestPhase {
        frame_time,
        queued_interaction_payloads: interaction_bridge::collect_early_canvas_interactions(
            app,
            ctx,
            paste_input.consume_v_key_events,
        ),
        did_rebuild_shader_space,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClipboardPasteSignals, canvas_clipboard_paste_scope_enabled,
        projected_canvas_focus_latched, reduce_clipboard_paste_gesture,
    };
    use rust_wgpu_fiber::eframe::egui;

    fn canvas_rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(300.0, 200.0))
    }

    #[test]
    fn clipboard_paste_gesture_deduplicates_press_and_release() {
        let mut suppress_release = false;
        assert!(reduce_clipboard_paste_gesture(
            ClipboardPasteSignals {
                command_v_pressed: true,
                ..Default::default()
            },
            &mut suppress_release,
        ));
        assert!(suppress_release);
        assert!(!reduce_clipboard_paste_gesture(
            ClipboardPasteSignals {
                command_v_released: true,
                v_released: true,
                ..Default::default()
            },
            &mut suppress_release,
        ));
        assert!(!suppress_release);
    }

    #[test]
    fn clipboard_paste_gesture_deduplicates_paste_event_and_release() {
        let mut suppress_release = false;
        assert!(reduce_clipboard_paste_gesture(
            ClipboardPasteSignals {
                paste_event: true,
                ..Default::default()
            },
            &mut suppress_release,
        ));
        assert!(!reduce_clipboard_paste_gesture(
            ClipboardPasteSignals {
                command_v_released: true,
                v_released: true,
                ..Default::default()
            },
            &mut suppress_release,
        ));
    }

    #[test]
    fn clipboard_paste_gesture_accepts_release_only_fallback() {
        let mut suppress_release = false;
        assert!(reduce_clipboard_paste_gesture(
            ClipboardPasteSignals {
                command_v_released: true,
                v_released: true,
                ..Default::default()
            },
            &mut suppress_release,
        ));
        assert!(!suppress_release);
    }

    #[test]
    fn clipboard_paste_scope_accepts_hover_or_latched_focus_but_not_text_editing() {
        let rect = canvas_rect();
        assert!(canvas_clipboard_paste_scope_enabled(
            true,
            false,
            Some(egui::pos2(120.0, 120.0)),
            Some(rect),
            false,
        ));
        assert!(canvas_clipboard_paste_scope_enabled(
            true,
            false,
            Some(egui::pos2(20.0, 20.0)),
            Some(rect),
            true,
        ));
        assert!(!canvas_clipboard_paste_scope_enabled(
            true,
            true,
            Some(egui::pos2(120.0, 120.0)),
            Some(rect),
            true,
        ));
        assert!(!canvas_clipboard_paste_scope_enabled(
            false,
            false,
            Some(egui::pos2(120.0, 120.0)),
            Some(rect),
            true,
        ));
    }

    #[test]
    fn outside_click_and_window_blur_clear_projected_canvas_focus() {
        let rect = canvas_rect();
        let outside_click = egui::Event::PointerButton {
            pos: egui::pos2(20.0, 20.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        assert!(!projected_canvas_focus_latched(
            &[outside_click],
            rect,
            true,
        ));
        assert!(!projected_canvas_focus_latched(
            &[egui::Event::WindowFocused(false)],
            rect,
            true,
        ));
    }
}
