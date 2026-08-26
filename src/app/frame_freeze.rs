use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use crate::{
    frame_freeze::{
        FRAME_FREEZE_SCHEMA_VERSION, FrameFreezeCase, FrameFreezeV1, LastSubmittedFrame,
        apply_param_values, apply_state_param_values, collect_scene_params, collect_state_params,
        fingerprint_hex,
    },
    protocol::now_millis,
    renderer,
    state_machine::OverrideKey,
};

use super::{
    canvas, scene_runtime,
    types::{App, FrameFreezePlayback, TestMode},
};

static FREEZE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn capture_context_is_plain_canvas(app: &App) -> bool {
    app.shell.test_mode == TestMode::Single
        && app.runtime.frame_freeze_playback.is_none()
        && app.canvas.display.pass_capture.is_none()
        && app.canvas.display.preview_texture_name.is_none()
        && app.shell.pass_shader_overrides.is_empty()
}

pub(crate) fn commit_submitted_frame(
    app: &mut App,
    state_values: &std::collections::HashMap<OverrideKey, Value>,
    scene_time_seconds: f32,
) {
    if !capture_context_is_plain_canvas(app) || app.runtime.frame_freeze_capture_blocked {
        app.runtime.last_submitted_frame = None;
        return;
    }
    let (Some(scene), Some(signature)) = (
        app.runtime.uniform_scene.as_ref(),
        app.runtime.last_pipeline_signature.as_ref(),
    ) else {
        app.runtime.last_submitted_frame = None;
        return;
    };
    app.runtime.last_submitted_frame = Some(LastSubmittedFrame {
        topology_fingerprint: fingerprint_hex(signature),
        payload: FrameFreezeV1 {
            schema_version: FRAME_FREEZE_SCHEMA_VERSION,
            scene_output_texture_name: app.core.scene_output_texture_name.as_str().to_string(),
            scene_time_seconds,
            state_params: collect_state_params(state_values),
            node_params: collect_scene_params(scene),
        },
    });
}

pub(crate) fn invalidate_submitted_frame(app: &mut App) {
    app.runtime.last_submitted_frame = None;
}

pub(crate) fn can_request_capture(app: &App) -> bool {
    capture_context_is_plain_canvas(app)
        && app.runtime.last_submitted_frame.is_some()
        && app.shell.pending_frame_freeze_case_id.is_none()
        && app.core.ws_hub.client_count() > 0
}

pub(crate) fn request_capture(app: &mut App) -> Result<()> {
    if !can_request_capture(app) {
        bail!("current canvas frame is not eligible for freeze capture");
    }
    let submitted = app
        .runtime
        .last_submitted_frame
        .clone()
        .ok_or_else(|| anyhow!("no submitted canvas frame is available"))?;
    let timestamp = now_millis();
    let sequence = FREEZE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let case_id = format!("freeze-{timestamp}-{sequence}");
    let case = FrameFreezeCase {
        case_id: case_id.clone(),
        name: format!("Freeze {timestamp}"),
        captured_at: timestamp.to_string(),
        topology_fingerprint: submitted.topology_fingerprint,
        payload: submitted.payload,
    };
    let request_id = format!("frame-freeze-{timestamp}-{sequence}");
    app.shell.pending_frame_freeze_case_id = Some(case_id.clone());
    app.shell.frame_freeze_status = Some(format!("Saving {}…", case.name));
    crate::ws::broadcast_frame_freeze_captured(&app.core.ws_hub, case, request_id);
    Ok(())
}

pub(crate) fn pending_capture_ready_to_activate(app: &App) -> Option<String> {
    let pending_case_id = app.shell.pending_frame_freeze_case_id.as_deref()?;
    app.shell
        .frame_freezes
        .iter()
        .any(|case| case.case_id == pending_case_id)
        .then(|| pending_case_id.to_string())
}

pub(crate) fn activate_case(app: &mut App, case_id: &str, now: f64) -> Result<()> {
    let case = app
        .shell
        .frame_freezes
        .iter()
        .find(|case| case.case_id == case_id)
        .cloned()
        .ok_or_else(|| anyhow!("unknown freeze case '{case_id}'"))?;
    crate::frame_freeze::validate_case(&case)?;
    if app.core.scene_output_texture_name.as_str() != case.payload.scene_output_texture_name {
        bail!(
            "freeze output texture mismatch: archive={} freeze={}",
            app.core.scene_output_texture_name,
            case.payload.scene_output_texture_name
        );
    }

    let (live_scene, live_time_seconds, live_time_updates_enabled) =
        if let Some(playback) = app.runtime.frame_freeze_playback.as_ref() {
            (
                playback.live_uniform_scene.clone(),
                playback.live_time_seconds,
                playback.live_time_updates_enabled,
            )
        } else {
            (
                app.runtime
                    .uniform_scene
                    .clone()
                    .ok_or_else(|| anyhow!("prepared renderer scene is unavailable"))?,
                app.runtime.time_value_secs,
                app.runtime.time_updates_enabled,
            )
        };

    if let Some(signature) = app.runtime.last_pipeline_signature.as_ref() {
        let current_fingerprint = fingerprint_hex(signature);
        if current_fingerprint != case.topology_fingerprint {
            eprintln!(
                "[frame-freeze] pipeline fingerprint changed; continuing with semantic replay validation: current={} freeze={}",
                current_fingerprint, case.topology_fingerprint
            );
        }
    }

    let mut frozen_scene = live_scene.clone();
    apply_state_param_values(&mut frozen_scene, &case.payload.state_params);
    apply_param_values(&mut frozen_scene, &case.payload.node_params)?;
    app.runtime.uniform_scene = Some(frozen_scene.clone());
    let materialize_result = (|| -> Result<()> {
        scene_runtime::apply_graph_uniform_updates_parts(
            &mut app.core.passes,
            &mut app.core.shader_space,
            &frozen_scene,
        )?;
        app.runtime.dynamic_gaussian_blur_dirty = !app.core.gaussian_blur_bundles.is_empty();
        scene_runtime::update_dynamic_gaussian_blur_bundles(app)?;
        Ok(())
    })();
    if let Err(error) = materialize_result {
        let _ = restore_live_scene(
            app,
            &live_scene,
            live_time_seconds,
            live_time_updates_enabled,
        );
        app.runtime.frame_freeze_playback = None;
        return Err(error);
    }
    app.runtime.time_value_secs = case.payload.scene_time_seconds;
    app.runtime.time_updates_enabled = false;
    app.runtime.frame_freeze_playback = Some(FrameFreezePlayback {
        active_case_id: case.case_id.clone(),
        live_uniform_scene: live_scene.clone(),
        live_time_seconds,
        live_time_updates_enabled,
    });
    app.shell.test_mode = TestMode::Single;
    if let Err(error) = render_frozen_scene(app, now) {
        let _ = restore_live_scene(
            app,
            &live_scene,
            live_time_seconds,
            live_time_updates_enabled,
        );
        app.runtime.frame_freeze_playback = None;
        return Err(error);
    }
    if app.shell.pending_frame_freeze_case_id.as_deref() == Some(case.case_id.as_str()) {
        app.shell.pending_frame_freeze_case_id = None;
    }
    app.shell.frame_freeze_status = None;
    Ok(())
}

fn restore_live_scene(
    app: &mut App,
    live_scene: &crate::dsl::SceneDSL,
    live_time_seconds: f32,
    live_time_updates_enabled: bool,
) -> Result<()> {
    app.runtime.uniform_scene = Some(live_scene.clone());
    scene_runtime::apply_graph_uniform_updates_parts(
        &mut app.core.passes,
        &mut app.core.shader_space,
        live_scene,
    )?;
    app.runtime.dynamic_gaussian_blur_dirty = !app.core.gaussian_blur_bundles.is_empty();
    scene_runtime::update_dynamic_gaussian_blur_bundles(app)?;
    app.runtime.time_value_secs = live_time_seconds;
    app.runtime.time_updates_enabled = live_time_updates_enabled;
    for pass in &mut app.core.passes {
        let mut params = pass.base_params;
        params.time = live_time_seconds;
        renderer::update_pass_params(&app.core.shader_space, pass, &params)?;
    }
    Ok(())
}

pub(crate) fn clear_case(app: &mut App, now: f64) -> Result<()> {
    let Some(playback) = app.runtime.frame_freeze_playback.take() else {
        return Ok(());
    };
    restore_live_scene(
        app,
        &playback.live_uniform_scene,
        playback.live_time_seconds,
        playback.live_time_updates_enabled,
    )?;
    render_frozen_scene(app, now)?;
    app.shell.frame_freeze_status = None;
    Ok(())
}

fn render_frozen_scene(app: &mut App, now: f64) -> Result<()> {
    let time = app.runtime.time_value_secs;
    for pass in &mut app.core.passes {
        let mut params = pass.base_params;
        params.time = time;
        renderer::update_pass_params(&app.core.shader_space, pass, &params)?;
    }
    let profile = canvas::draw_capture::render_profiled(app, false);
    app.runtime.latest_render_profile = Some(profile);
    app.runtime.scene_redraw_pending = false;
    app.runtime
        .render_texture_fps_tracker
        .record_scene_redraw(now);
    app.canvas.invalidation.mark_analysis_dirty();
    app.canvas.invalidation.mark_clipping_dirty();
    app.canvas.invalidation.mark_qualifier_dirty();
    app.canvas.invalidation.mark_pixel_overlay_dirty();
    Ok(())
}
