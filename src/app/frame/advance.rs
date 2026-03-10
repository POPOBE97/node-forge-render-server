use crate::{
    app::{scene_runtime, types::App},
    state_machine,
};

use super::interaction_bridge;

pub(super) struct AdvancePhase {
    pub animation_values_changed: bool,
    pub time_driven_scene: bool,
    pub animation_session_active: bool,
    pub should_redraw_scene: bool,
}

pub(super) fn run(app: &mut App) -> AdvancePhase {
    let raw_t = app.runtime.start.elapsed().as_secs_f32();
    let delta_t = (raw_t - app.runtime.time_last_raw_secs).max(0.0);
    app.runtime.time_last_raw_secs = raw_t;

    let effective_dt = if app.runtime.time_updates_enabled && app.runtime.animation_playing {
        delta_t
    } else {
        0.0
    };
    let anim_step = if app.runtime.animation_playing {
        app.runtime
            .animation_session
            .as_mut()
            .map(|session| session.step(effective_dt as f64))
    } else {
        None
    };

    let mut animation_values_changed = false;
    let mut animation_current_state_id: Option<String> = None;
    let mut animation_active_transition_id: Option<String> = None;
    if let Some(step) = anim_step {
        animation_current_state_id = Some(step.current_state_id.clone());
        animation_active_transition_id = step.active_transition_id.clone();

        // Force a full override re-apply when resuming from pause.
        // While paused the timeline hover preview may have dirtied
        // uniform_scene with values from an arbitrary frame.  The
        // session's own `needs_redraw` flag won't catch this because
        // from its perspective the overrides haven't changed.
        let resuming_from_pause = effective_dt > 0.0
            && !app.runtime.time_updates_enabled_prev_frame;

        if step.needs_redraw || resuming_from_pause {
            animation_values_changed = true;
            if let Some(ref mut uniform_scene) = app.runtime.uniform_scene {
                state_machine::apply_overrides(uniform_scene, &step.active_overrides);
            }
            if let Some(ref uniform_scene) = app.runtime.uniform_scene {
                let _ = scene_runtime::apply_graph_uniform_updates_parts(
                    &mut app.core.passes,
                    &mut app.core.shader_space,
                    uniform_scene,
                );
            }
            app.runtime.scene_redraw_pending = true;
        }
        app.runtime.time_value_secs = step.scene_time_secs as f32;
        interaction_bridge::update_debug_state(app, &step);

        // Record timeline frame for the debug sidebar timeline tab.
        // Skip recording when paused (effective_dt == 0) to avoid
        // duplicate frames at the same scene_time.
        if effective_dt > 0.0 && step.needs_redraw
        && let Some(ref mut buf) = app.runtime.timeline_buffer {
            let presentation_time = buf.elapsed_secs();
            // Resolve transition source/target names from the session definition.
            let (tsrc, ttgt) = app
                .runtime
                .animation_session
                .as_ref()
                .and_then(|sess| {
                    let def = sess.runtime().definition();
                    step.active_transition_id.as_ref().and_then(|tid| {
                        def.transitions.iter().find(|t| t.id == *tid).map(|t| {
                            let src = def
                                .states
                                .iter()
                                .find(|s| s.id == t.source)
                                .map(|s| s.name.clone())
                                .unwrap_or_else(|| t.source.clone());
                            let tgt = def
                                .states
                                .iter()
                                .find(|s| s.id == t.target)
                                .map(|s| s.name.clone())
                                .unwrap_or_else(|| t.target.clone());
                            (Some(src), Some(tgt))
                        })
                    })
                })
                .unwrap_or((None, None));
            buf.push(crate::animation::TimelineFrame {
                presentation_time_secs: presentation_time,
                scene_time_secs: step.scene_time_secs,
                current_state_id: step.current_state_id.clone(),
                active_transition_id: step.active_transition_id.clone(),
                transition_blend: step.transition_blend,
                transition_source_name: tsrc,
                transition_target_name: ttgt,
                state_local_times: step.state_local_times.clone(),
                diagnostics: step.diagnostics.clone(),
                active_overrides: step.active_overrides.clone(),
            });
        }
        app.runtime.last_live_overrides = Some(step.active_overrides.clone());
    } else if app.runtime.time_updates_enabled {
        app.runtime.time_value_secs += delta_t;
    }

    interaction_bridge::sync_animation_state(
        app,
        animation_current_state_id.as_deref(),
        animation_active_transition_id.as_deref(),
    );

    let time_driven_scene = app.runtime.scene_uses_time && app.runtime.time_updates_enabled;
    let animation_session_active = app.runtime.animation_playing
        && app
            .runtime
            .animation_session
            .as_ref()
            .is_some_and(|session| session.is_active());

    // Track for next frame's pause→play edge detection.
    app.runtime.time_updates_enabled_prev_frame = app.runtime.time_updates_enabled;

    AdvancePhase {
        animation_values_changed,
        time_driven_scene,
        animation_session_active,
        should_redraw_scene: app.runtime.scene_redraw_pending
            || time_driven_scene
            || app.runtime.capture_redraw_active,
    }
}
