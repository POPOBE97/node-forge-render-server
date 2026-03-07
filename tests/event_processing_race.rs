//! Event processing race condition tests.
//!
//! **Property 1 — Validates: Requirements 1.1, 2.1, 2.2**
//!
//! Bug condition exploration: demonstrates the race condition between
//! `session.step()` and `session.fire_event()`. When events are fired BEFORE
//! `step()`, they are consumed in the same frame. When `step()` runs first
//! (the buggy ordering in `App::update()`), events queued afterward are NOT
//! consumed until the next `step()` call.
//!
//! **Property 2 — Validates: Requirements 3.1, 3.4, 3.5**
//!
//! Preservation: non-interaction frame behavior. When no interaction events
//! are present, `session.step(dt)` with empty `pending_events` produces
//! deterministic, stable results — unchanged `current_state_id`, deterministic
//! `scene_time_secs`, and `needs_redraw` based solely on time-driven logic.

use std::collections::HashMap;

use proptest::prelude::*;

use node_forge_render_server::animation::AnimationSession;
use node_forge_render_server::dsl::{Metadata, SceneDSL};
use node_forge_render_server::state_machine::types::{
    AnimationState, AnimationStateType, AnimationTransition, EasingKind, StateMachine,
    TransitionCondition,
};

/// Build a minimal scene with a state machine that transitions from "entry"
/// to "target" on a given event name (instant transition: delay=0, duration=0).
fn scene_with_event_transition(event_name: &str) -> SceneDSL {
    let sm = StateMachine {
        id: "sm_test".into(),
        name: "Test SM".into(),
        states: vec![
            AnimationState {
                id: "entry".into(),
                name: "Entry".into(),
                position: None,
                parameter_overrides: Default::default(),
                state_type: Some(AnimationStateType::EntryState),
                mutation_id: None,
            },
            AnimationState {
                id: "any".into(),
                name: "Any".into(),
                position: None,
                parameter_overrides: Default::default(),
                state_type: Some(AnimationStateType::AnyState),
                mutation_id: None,
            },
            AnimationState {
                id: "exit".into(),
                name: "Exit".into(),
                position: None,
                parameter_overrides: Default::default(),
                state_type: Some(AnimationStateType::ExitState),
                mutation_id: None,
            },
            AnimationState {
                id: "target".into(),
                name: "Target".into(),
                position: None,
                parameter_overrides: Default::default(),
                state_type: Some(AnimationStateType::AnimationState),
                mutation_id: None,
            },
        ],
        transitions: vec![AnimationTransition {
            id: "tr_event".into(),
            source: "entry".into(),
            target: "target".into(),
            trigger: None,
            condition: Some(TransitionCondition::Event {
                event_name: event_name.to_string(),
            }),
            delay: 0.0,
            duration: 0.0,
            easing: EasingKind::Linear,
        }],
        mutations: vec![],
        initial_state_id: Some("entry".into()),
        viewport: None,
    };

    SceneDSL {
        version: "2.0".into(),
        metadata: Metadata {
            name: "Test Scene".into(),
            created: None,
            modified: None,
        },
        nodes: vec![],
        connections: vec![],
        outputs: None,
        groups: vec![],
        assets: HashMap::new(),
        state_machine: Some(sm),
    }
}

/// Strategy for generating valid interaction event names.
fn event_name_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("mousedown".to_string()),
        Just("mouseup".to_string()),
        Just("keydown".to_string()),
        Just("keyup".to_string()),
        Just("wheel".to_string()),
        Just("touchstart".to_string()),
        Just("click".to_string()),
    ]
}

/// Strategy for generating dt values large enough to trigger at least one
/// fixed-step tick. The clock runs at 60fps (step_secs ≈ 0.01667s), so we
/// generate values from ~17ms to ~100ms to guarantee tick_count >= 1.
fn effective_dt_strategy() -> impl Strategy<Value = f64> {
    (17u64..100).prop_map(|ms| ms as f64 / 1000.0)
}

// ---------------------------------------------------------------------------
// Property 1: CORRECT ordering — fire_event BEFORE step → event consumed
// ---------------------------------------------------------------------------

proptest! {
    /// **Validates: Requirements 2.1, 2.2**
    ///
    /// For any interaction event name and frame dt (large enough to trigger
    /// a tick), firing the event BEFORE calling `step()` results in the event
    /// being consumed in that same step — the state machine transitions from
    /// "entry" to "target".
    #[test]
    fn correct_ordering_fire_before_step_consumes_event(
        event_name in event_name_strategy(),
        dt in effective_dt_strategy(),
    ) {
        let scene = scene_with_event_transition(&event_name);
        let mut session = AnimationSession::from_scene(&scene)
            .expect("session should build")
            .expect("session should be Some");

        // First step with dt=0 to initialize (fires the initial dt=0 tick).
        let init = session.step(0.0);
        assert_eq!(init.current_state_id, "entry", "should start at entry");

        // CORRECT ordering: fire event, then step.
        session.fire_event(&event_name);
        let result = session.step(dt);

        // The event should have been consumed — with delay=0 and duration=0,
        // the transition is instant so we land at "target".
        prop_assert_eq!(
            result.current_state_id, "target",
            "correct ordering: event '{}' with dt={} should transition to 'target' in same step",
            event_name, dt
        );
    }
}

// ---------------------------------------------------------------------------
// Property 1 (Bug Condition): BUGGY ordering — step BEFORE fire_event
// → event NOT consumed in that step.
//
// This is now a regression guard for the ordering contract. The app must
// keep using the "fire before step" ordering validated above; this companion
// test documents and verifies the incorrect behavior of the inverse ordering.
// ---------------------------------------------------------------------------

proptest! {
    /// **Validates: Requirements 1.1, 2.1, 2.2**
    ///
    /// For any interaction event name and frame dt, calling `step()` BEFORE
    /// `fire_event()` means the event is NOT consumed in that step.
    #[test]
    fn buggy_ordering_step_before_fire_event_loses_event(
        event_name in event_name_strategy(),
        dt in effective_dt_strategy(),
    ) {
        let scene = scene_with_event_transition(&event_name);
        let mut session = AnimationSession::from_scene(&scene)
            .expect("session should build")
            .expect("session should be Some");

        // First step with dt=0 to initialize.
        let init = session.step(0.0);
        assert_eq!(init.current_state_id, "entry", "should start at entry");

        // BUGGY ordering (mirrors App::update): step first, then fire event.
        let result = session.step(dt);
        session.fire_event(&event_name);

        prop_assert_eq!(
            result.current_state_id, "entry",
            "buggy ordering: event '{}' with dt={} should remain at 'entry' because \
             step() drained the empty pending_events before fire_event() queued anything",
            event_name, dt
        );
    }
}

// ===========================================================================
// Property 2: Preservation — Non-Interaction Frame Behavior
// ===========================================================================

/// Build a minimal scene with a state machine (entry → target on "mousedown")
/// but NO events will be fired — used to test the no-event preservation path.
fn scene_for_preservation() -> SceneDSL {
    scene_with_event_transition("mousedown")
}

/// Strategy for generating dt values that span a wider range, including
/// sub-tick values (where no tick fires) and multi-tick values.
/// Range: 0.001s (1ms) to 0.200s (200ms).
fn preservation_dt_strategy() -> impl Strategy<Value = f64> {
    (1u64..200).prop_map(|ms| ms as f64 / 1000.0)
}

// ---------------------------------------------------------------------------
// Property 2a: No-event step preserves current_state_id
// ---------------------------------------------------------------------------

proptest! {
    /// **Validates: Requirements 3.1**
    ///
    /// For any positive dt, when no interaction events are present,
    /// `session.step(dt)` with empty `pending_events` preserves the
    /// `current_state_id` — the state machine does not transition without
    /// an event trigger.
    #[test]
    fn no_event_step_preserves_state_id(
        dt in preservation_dt_strategy(),
    ) {
        let scene = scene_for_preservation();
        let mut session = AnimationSession::from_scene(&scene)
            .expect("session should build")
            .expect("session should be Some");

        // Initialize: first step with dt=0.
        let init = session.step(0.0);
        prop_assert_eq!(
            init.current_state_id, "entry",
            "should start at entry state"
        );

        // Step with no events — state should remain "entry".
        let result = session.step(dt);
        prop_assert_eq!(
            result.current_state_id, "entry",
            "no-event step with dt={} should preserve state_id at 'entry'",
            dt
        );
    }
}

// ---------------------------------------------------------------------------
// Property 2b: No-event step produces deterministic scene_time_secs
// ---------------------------------------------------------------------------

proptest! {
    /// **Validates: Requirements 3.1**
    ///
    /// For any positive dt, two identically-constructed sessions stepped with
    /// the same dt (and no events) produce the same `scene_time_secs`.
    /// This confirms the fixed-step clock is deterministic.
    #[test]
    fn no_event_step_deterministic_scene_time(
        dt in preservation_dt_strategy(),
    ) {
        let scene = scene_for_preservation();

        let mut session_a = AnimationSession::from_scene(&scene)
            .expect("session should build")
            .expect("session should be Some");
        let mut session_b = AnimationSession::from_scene(&scene)
            .expect("session should build")
            .expect("session should be Some");

        // Initialize both.
        session_a.step(0.0);
        session_b.step(0.0);

        // Step both with the same dt, no events.
        let result_a = session_a.step(dt);
        let result_b = session_b.step(dt);

        prop_assert!(
            (result_a.scene_time_secs - result_b.scene_time_secs).abs() < 1e-12,
            "scene_time_secs should be deterministic: {} vs {} for dt={}",
            result_a.scene_time_secs, result_b.scene_time_secs, dt
        );
    }
}

// ---------------------------------------------------------------------------
// Property 2c: No-event step — needs_redraw is false (no override changes)
// ---------------------------------------------------------------------------

proptest! {
    /// **Validates: Requirements 3.1, 3.4**
    ///
    /// For any positive dt, when no interaction events are present and the
    /// state machine has no time-driven mutations, `session.step(dt)` with
    /// empty `pending_events` produces `needs_redraw: false` because no
    /// override values change.
    #[test]
    fn no_event_step_needs_redraw_false(
        dt in preservation_dt_strategy(),
    ) {
        let scene = scene_for_preservation();
        let mut session = AnimationSession::from_scene(&scene)
            .expect("session should build")
            .expect("session should be Some");

        // Initialize.
        let init = session.step(0.0);
        // After init, active_overrides is empty (no mutations in this scene).
        prop_assert!(!init.needs_redraw || init.active_overrides.is_empty(),
            "init step should have empty overrides for this scene");

        // Step with no events.
        let result = session.step(dt);
        prop_assert!(
            !result.needs_redraw,
            "no-event step with dt={} should have needs_redraw=false (no override changes)",
            dt
        );
    }
}

// ---------------------------------------------------------------------------
// Property 2d: No-event step — active_overrides unchanged across frames
// ---------------------------------------------------------------------------

proptest! {
    /// **Validates: Requirements 3.1**
    ///
    /// For any sequence of positive dt values (simulating multiple frames),
    /// when no interaction events are present, `active_overrides` remains
    /// stable across all frames.
    #[test]
    fn no_event_multi_step_overrides_stable(
        dts in prop::collection::vec(preservation_dt_strategy(), 1..10),
    ) {
        let scene = scene_for_preservation();
        let mut session = AnimationSession::from_scene(&scene)
            .expect("session should build")
            .expect("session should be Some");

        // Initialize.
        let init = session.step(0.0);
        let baseline_overrides = init.active_overrides.clone();

        // Step through multiple frames with no events.
        for (i, &dt) in dts.iter().enumerate() {
            let result = session.step(dt);
            prop_assert_eq!(
                &result.active_overrides, &baseline_overrides,
                "frame {} (dt={}): active_overrides should remain stable with no events",
                i, dt
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Property 2e: No-event step — session remains active (not finished)
// ---------------------------------------------------------------------------

proptest! {
    /// **Validates: Requirements 3.1, 3.5**
    ///
    /// For any positive dt, when no interaction events are present, the
    /// session remains active (not finished) because the state machine
    /// stays at the entry state and never reaches the exit state.
    #[test]
    fn no_event_step_session_remains_active(
        dt in preservation_dt_strategy(),
    ) {
        let scene = scene_for_preservation();
        let mut session = AnimationSession::from_scene(&scene)
            .expect("session should build")
            .expect("session should be Some");

        // Initialize.
        session.step(0.0);

        // Step with no events.
        let result = session.step(dt);
        prop_assert!(
            result.active,
            "no-event step with dt={} should keep session active",
            dt
        );
        prop_assert!(
            !result.finished,
            "no-event step with dt={} should not mark session as finished",
            dt
        );
    }
}

// ---------------------------------------------------------------------------
// Property 2f: No-event step — full AnimationStep equivalence (determinism)
// ---------------------------------------------------------------------------

proptest! {
    /// **Validates: Requirements 3.1, 3.4, 3.5**
    ///
    /// For any positive dt, two identically-constructed sessions stepped with
    /// the same dt (and no events) produce fully equivalent `AnimationStep`
    /// results: same `current_state_id`, same `active_overrides`, same
    /// `needs_redraw`, same `scene_time_secs`, same `active`, same `finished`.
    #[test]
    fn no_event_step_full_equivalence(
        dt in preservation_dt_strategy(),
    ) {
        let scene = scene_for_preservation();

        let mut session_a = AnimationSession::from_scene(&scene)
            .expect("session should build")
            .expect("session should be Some");
        let mut session_b = AnimationSession::from_scene(&scene)
            .expect("session should build")
            .expect("session should be Some");

        // Initialize both.
        session_a.step(0.0);
        session_b.step(0.0);

        // Step both with the same dt, no events.
        let a = session_a.step(dt);
        let b = session_b.step(dt);

        prop_assert_eq!(&a.current_state_id, &b.current_state_id,
            "current_state_id should match");
        prop_assert_eq!(&a.active_overrides, &b.active_overrides,
            "active_overrides should match");
        prop_assert_eq!(a.needs_redraw, b.needs_redraw,
            "needs_redraw should match");
        prop_assert!((a.scene_time_secs - b.scene_time_secs).abs() < 1e-12,
            "scene_time_secs should match: {} vs {}", a.scene_time_secs, b.scene_time_secs);
        prop_assert_eq!(a.active, b.active, "active should match");
        prop_assert_eq!(a.finished, b.finished, "finished should match");
        prop_assert_eq!(&a.state_local_times, &b.state_local_times,
            "state_local_times should match");
        prop_assert_eq!(a.transition_blend, b.transition_blend,
            "transition_blend should match");
        prop_assert_eq!(a.active_transition_id, b.active_transition_id,
            "active_transition_id should match");
    }
}
