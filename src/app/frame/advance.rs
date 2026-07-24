use crate::{
    animation::AnimationStep,
    app::{
        scene_runtime,
        types::{App, StateControlSelection},
    },
    state_machine,
    state_machine::types::StateMachine,
};

use super::interaction_bridge;

pub(super) struct AdvancePhase {
    pub animation_values_changed: bool,
    pub time_driven_scene: bool,
    pub animation_session_active: bool,
    pub should_redraw_scene: bool,
    pub frame_uniform_values:
        std::collections::HashMap<crate::state_machine::OverrideKey, serde_json::Value>,
}

pub(super) fn run(app: &mut App) -> AdvancePhase {
    let raw_t = app.runtime.start.elapsed().as_secs_f32();
    let delta_t = (raw_t - app.runtime.time_last_raw_secs).max(0.0);
    app.runtime.time_last_raw_secs = raw_t;

    let state_control_selection = app.runtime.state_control_selection.clone();
    let state_control_active = state_control_selection.is_some();
    let playing = matches!(state_control_selection, Some(StateControlSelection::Play));

    let effective_dt = if app.runtime.time_updates_enabled && state_control_active {
        delta_t
    } else {
        0.0
    };
    let anim_step = if state_control_active {
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
        animation_active_transition_id = step.active_transition_id.clone();
        animation_current_state_id = Some(interaction_sync_state_id(
            &step,
            app.runtime
                .animation_session
                .as_ref()
                .map(|session| session.runtime().definition()),
        ));

        // Force a full override re-apply when resuming from pause.
        // While paused the timeline hover preview may have dirtied
        // uniform_scene with values from an arbitrary frame.  The
        // session's own `needs_redraw` flag won't catch this because
        // from its perspective the overrides haven't changed.
        let resuming_from_pause =
            effective_dt > 0.0 && !app.runtime.time_updates_enabled_prev_frame;

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
        if playing
            && effective_dt > 0.0
            && step.needs_redraw
            && let Some(ref mut buf) = app.runtime.timeline_buffer
        {
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
                motion_channels: step.motion_channels.clone(),
                transition_source_name: tsrc,
                transition_target_name: ttgt,
                state_local_times: step.state_local_times.clone(),
                diagnostics: step.diagnostics.clone(),
                active_overrides: step.active_overrides.clone(),
            });
        }
        app.runtime.last_live_overrides = Some(step.active_overrides.clone());
        if playing && step.finished {
            scene_runtime::clear_state_control(app);
            animation_current_state_id = None;
            animation_active_transition_id = None;
            animation_values_changed = true;
        }
    } else if app.runtime.time_updates_enabled {
        app.runtime.time_value_secs += delta_t;
    }

    interaction_bridge::sync_animation_state(
        app,
        animation_current_state_id.as_deref(),
        animation_active_transition_id.as_deref(),
    );

    let time_driven_scene = app.runtime.scene_uses_time && app.runtime.time_updates_enabled;
    let animation_session_active = app.runtime.state_control_selection.is_some()
        && app
            .runtime
            .animation_session
            .as_ref()
            .is_some_and(|session| session.is_active());

    let frame_uniform_values = collect_frame_uniform_values(app);

    // Track for next frame's pause→play edge detection.
    app.runtime.time_updates_enabled_prev_frame = app.runtime.time_updates_enabled;

    AdvancePhase {
        animation_values_changed,
        time_driven_scene,
        animation_session_active,
        should_redraw_scene: app.runtime.scene_redraw_pending
            || time_driven_scene
            || app.runtime.capture_redraw_active
            || app.runtime.force_continuous_redraw,
        frame_uniform_values,
    }
}

fn collect_frame_uniform_values(
    app: &App,
) -> std::collections::HashMap<crate::state_machine::OverrideKey, serde_json::Value> {
    let Some(session) = app.runtime.animation_session.as_ref() else {
        return std::collections::HashMap::new();
    };
    session.presentation_snapshot()
}

fn interaction_sync_state_id(step: &AnimationStep, _sm: Option<&StateMachine>) -> String {
    step.current_state_id.clone()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::state_machine::types::{
        AnimationState, AnimationStateType, AnimationTransition, Position,
    };

    fn state(id: &str, state_type: AnimationStateType) -> AnimationState {
        AnimationState {
            id: id.to_string(),
            name: id.to_string(),
            position: Some(Position { x: 0.0, y: 0.0 }),
            parameter_overrides: HashMap::new(),
            state_type,
            mutation_id: None,
        }
    }

    fn base_step(active_transition_id: Option<&str>) -> AnimationStep {
        AnimationStep {
            active_overrides: HashMap::new(),
            needs_redraw: false,
            scene_time_secs: 0.0,
            active: true,
            diagnostics: Vec::new(),
            current_state_id: "entry".into(),
            active_transition_id: active_transition_id.map(str::to_string),
            state_local_times: Default::default(),
            motion_channels: Vec::new(),
            finished: false,
        }
    }

    #[test]
    fn any_state_transition_reports_runtime_current_state_for_interaction_sync() {
        let sm = StateMachine {
            id: "sm".into(),
            name: "State Machine".into(),
            states: vec![
                state("entry", AnimationStateType::EntryState),
                state("any", AnimationStateType::AnyState),
                state("exit", AnimationStateType::ExitState),
                state("mutation", AnimationStateType::AnimationState),
            ],
            transitions: vec![AnimationTransition {
                id: "tr_any_mutation".into(),
                source: "any".into(),
                target: "mutation".into(),
                motion_graph_id: "motion".into(),
            }],
            mutation_bindings: Vec::new(),
            mutations: Vec::new(),
            motion_graphs: Vec::new(),
            initial_state_id: Some("entry".into()),
            viewport: None,
        };

        let mut step = base_step(Some("tr_any_mutation"));
        step.current_state_id = "mutation".into();
        assert_eq!(interaction_sync_state_id(&step, Some(&sm)), "mutation");
    }

    #[test]
    fn non_any_transition_reports_runtime_current_state_for_interaction_sync() {
        let sm = StateMachine {
            id: "sm".into(),
            name: "State Machine".into(),
            states: vec![
                state("entry", AnimationStateType::EntryState),
                state("any", AnimationStateType::AnyState),
                state("exit", AnimationStateType::ExitState),
                state("mutation", AnimationStateType::AnimationState),
            ],
            transitions: vec![AnimationTransition {
                id: "tr_entry_mutation".into(),
                source: "entry".into(),
                target: "mutation".into(),
                motion_graph_id: "motion".into(),
            }],
            mutation_bindings: Vec::new(),
            mutations: Vec::new(),
            motion_graphs: Vec::new(),
            initial_state_id: Some("entry".into()),
            viewport: None,
        };

        assert_eq!(
            interaction_sync_state_id(&base_step(Some("tr_entry_mutation")), Some(&sm)),
            "entry"
        );
    }
}
