use node_forge_render_server::animation::{AnimationSession, AnimationStep};
use node_forge_render_server::state_machine::{FiredEvent, MousePosition, OverrideKey};

mod support;

const POSITIONS_KEY: &str = "Vector2ArrayInput_IntelligentLightPositions:value";
const SNAP_PRIMARY_PARAM_ID: &str = "sp_5e510047b6cb8f4d";
const SNAP_SECONDARY_PARAM_ID: &str = "sp_653078b6ffd2ce9c";
const PTT_ORB_PARAM_ID: &str = "sp_ptt_object_scene_px";
const PTT_ORB_ANCHOR_PARAM_ID: &str = "sp_ptt_orb_anchor_position_px";

fn space_event(event_type: &str) -> FiredEvent {
    FiredEvent {
        event_type: event_type.into(),
        key: Some(" ".into()),
        ..Default::default()
    }
}

fn pointer_event(event_type: &str) -> FiredEvent {
    FiredEvent {
        event_type: event_type.into(),
        ..Default::default()
    }
}

fn assert_vec2_close(actual: &[f64], expected: [f64; 2], message: &str) {
    assert_eq!(
        actual.len(),
        2,
        "{message}: expected a vec2, got {actual:?}"
    );
    assert!(
        (actual[0] - expected[0]).abs() <= 1.0e-8 && (actual[1] - expected[1]).abs() <= 1.0e-8,
        "{message}: expected {expected:?}, got {actual:?}"
    );
}

fn rubber_band_distance(distance: f64) -> f64 {
    const MAX_DISTANCE: f64 = 400.0;
    const COEFFICIENT: f64 = 0.1;
    (MAX_DISTANCE * COEFFICIENT * distance) / (MAX_DISTANCE + COEFFICIENT * distance)
}

fn session_starting_in(
    scene: &node_forge_render_server::dsl::SceneDSL,
    state_id: &str,
) -> AnimationSession {
    let mut scene = scene.clone();
    scene
        .state_machine
        .as_mut()
        .expect("doubao fixture should have a state machine")
        .initial_state_id = Some(state_id.into());
    AnimationSession::from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao fixture should have a state machine")
}

fn settle(session: &mut AnimationSession) -> AnimationStep {
    let mut step = session.step(0.0);
    for _ in 0..360 {
        if step.active_transition_id.is_none() {
            return step;
        }
        step = session.step(1.0 / 60.0);
    }
    panic!("transition did not settle: {:?}", step.active_transition_id);
}

fn channel<'a>(
    step: &'a AnimationStep,
    key: &str,
) -> &'a node_forge_render_server::state_machine::MotionChannelDebug {
    step.motion_channels
        .iter()
        .find(|channel| channel.key == key)
        .unwrap_or_else(|| panic!("missing MotionEngine channel '{key}'"))
}

fn position_x(step: &AnimationStep) -> f64 {
    let key = node_forge_render_server::state_machine::OverrideKey::parse(POSITIONS_KEY)
        .expect("positions key should be canonical");
    step.active_overrides
        .get(&key)
        .and_then(serde_json::Value::as_array)
        .and_then(|positions| positions.first())
        .and_then(serde_json::Value::as_array)
        .and_then(|position| position.first())
        .and_then(serde_json::Value::as_f64)
        .expect("render-derived positions should contain a numeric first x")
}

fn uniform_number(step: &AnimationStep, node_id: &str, param_id: &str) -> f64 {
    step.active_overrides
        .get(&OverrideKey::new(node_id, param_id))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_else(|| panic!("missing numeric override '{node_id}:{param_id}'"))
}

#[test]
fn doubao_ptt_and_cancel_share_orb_targeting_and_return_spring() {
    let scene = support::load_render_case_scene("doubao-voice-interaction");
    let machine = scene
        .state_machine
        .as_ref()
        .expect("doubao fixture should have a state machine");

    let push_to_talk = machine
        .states
        .iter()
        .find(|state| state.id == "st_push_to_talk")
        .expect("PushToTalk State");
    let cancel = machine
        .states
        .iter()
        .find(|state| state.id == "st_push_to_talk_cancel")
        .expect("PushToTalkCancel State");
    for (param_id, expected) in [
        (PTT_ORB_PARAM_ID, serde_json::json!([540, 144])),
        (PTT_ORB_ANCHOR_PARAM_ID, serde_json::json!([540, 100])),
    ] {
        let push_to_talk_value = push_to_talk
            .state_param_overrides
            .get(param_id)
            .unwrap_or_else(|| panic!("PushToTalk should override '{param_id}'"));
        let cancel_value = cancel
            .state_param_overrides
            .get(param_id)
            .unwrap_or_else(|| panic!("PushToTalkCancel should override '{param_id}'"));
        assert_eq!(push_to_talk_value, &expected);
        assert_eq!(cancel_value, push_to_talk_value);
    }

    for (state_id, function_id) in [
        ("st_push_to_talk", "function_ptt_orb_motion"),
        ("st_push_to_talk_cancel", "function_ptt_cancel_orb_motion"),
    ] {
        let mutation = machine
            .states
            .iter()
            .find(|state| state.id == state_id)
            .and_then(|state| state.mutation_graph.as_ref())
            .unwrap_or_else(|| panic!("'{state_id}' should own a Mutation graph"));
        assert!(mutation.output_bindings.iter().any(|binding| {
            binding.state_param_id == PTT_ORB_PARAM_ID
                && binding.from.node_id == function_id
                && binding.from.port_id == "objectScenePx"
        }));
    }

    for transition_id in [
        "tr_push_to_talk_to_idle",
        "tr_push_to_talk_to_thinking",
        "tr_cancel_to_idle",
    ] {
        let transition = machine
            .transitions
            .iter()
            .find(|transition| transition.id == transition_id)
            .unwrap_or_else(|| panic!("missing transition '{transition_id}'"));
        let graph = machine
            .motion_graphs
            .iter()
            .find(|graph| graph.id == transition.motion_graph_id)
            .expect("PTT exit should own a Motion Graph");
        let return_spring = graph
            .nodes
            .iter()
            .find(|node| node.id() == "timing_ptt_orb_return")
            .expect("PTT exit should own an orb return spring");
        let node_forge_render_server::state_machine::types::TransitionMotionNode::Spring {
            duration,
            bounce,
            ..
        } = return_spring
        else {
            panic!("PTT orb return timing node should be a spring");
        };
        assert!((*duration - 0.42).abs() <= f64::EPSILON);
        assert!((*bounce - 0.16).abs() <= f64::EPSILON);
        assert!(graph.input_bindings.iter().any(|binding| {
            binding.source.id() == PTT_ORB_PARAM_ID && binding.to.node_id == "timing_ptt_orb_return"
        }));
    }

    let mut sampled_targets = Vec::new();
    for state_id in ["st_push_to_talk", "st_push_to_talk_cancel"] {
        let mut session = AnimationSession::from_scene(&scene)
            .expect("doubao state machine should compile")
            .expect("doubao fixture should have a state machine");
        session
            .force_state(state_id)
            .unwrap_or_else(|error| panic!("'{state_id}' should be forceable: {error}"));
        settle(&mut session);

        session.update_mouse_position(MousePosition { x: 540.0, y: 100.0 });
        let centered = session.step(1.0 / 60.0);
        let centered_channel = channel(&centered, PTT_ORB_PARAM_ID);
        assert_vec2_close(
            &centered_channel.target_value,
            [540.0, 100.0],
            "pointer at anchor",
        );
        let centered_local_y =
            uniform_number(&centered, "Vector2Input_PointerLightEffectLocalPx", "y");

        session.update_mouse_position(MousePosition { x: 570.0, y: 100.0 });
        let near = session.step(1.0 / 60.0);
        let near_channel = channel(&near, PTT_ORB_PARAM_ID);
        let expected_near_x = 540.0 + rubber_band_distance(30.0);
        assert_vec2_close(
            &near_channel.target_value,
            [expected_near_x, 100.0],
            "near pointer uses the zero-free-radius rubber band",
        );

        session.update_mouse_position(MousePosition { x: 540.0, y: 130.0 });
        let near_up = session.step(1.0 / 60.0);
        let near_up_channel = channel(&near_up, PTT_ORB_PARAM_ID);
        let expected_near_y = 100.0 + rubber_band_distance(30.0);
        assert_vec2_close(
            &near_up_channel.target_value,
            [540.0, expected_near_y],
            "upward pointer uses the same radial rubber band",
        );
        assert!(
            uniform_number(&near_up, "Vector2Input_PointerLightEffectLocalPx", "y")
                > centered_local_y,
            "moving the ScenePx pointer upward must increase LightEffect LocalPx Y"
        );

        session.update_mouse_position(MousePosition {
            x: 1140.0,
            y: 100.0,
        });
        let resisted = session.step(1.0 / 60.0);
        let expected_far_x = 540.0 + rubber_band_distance(600.0);
        let resisted_channel = channel(&resisted, PTT_ORB_PARAM_ID);
        assert_eq!(resisted_channel.transition_driver, "hold");
        assert_vec2_close(
            &resisted_channel.target_value,
            [expected_far_x, 100.0],
            "far pointer uses the shared rubber band",
        );
        assert!(expected_far_x - 540.0 < 400.0);
        assert!(expected_far_x - 540.0 < 600.0);

        sampled_targets.push((
            near_channel.target_value.clone(),
            near_up_channel.target_value.clone(),
            resisted_channel.target_value.clone(),
        ));

        let outgoing = resisted_channel.value.clone();
        let returning = session
            .force_state("st_mrerxocx_8")
            .unwrap_or_else(|error| panic!("Idle should be forceable from '{state_id}': {error}"));
        let returning_channel = channel(&returning, PTT_ORB_PARAM_ID);
        assert_eq!(returning_channel.transition_driver, "spring");
        assert_vec2_close(
            &returning_channel.value,
            [outgoing[0], outgoing[1]],
            "return spring preserves the outgoing physical value at dt=0",
        );

        let returned = settle(&mut session);
        let returned_channel = channel(&returned, PTT_ORB_PARAM_ID);
        assert_vec2_close(
            &returned_channel.value,
            [540.0, 144.0],
            "return spring settles at the authored resting object position",
        );
    }
    assert_eq!(sampled_targets[0], sampled_targets[1]);
}

#[test]
fn doubao_ptt_cancel_round_trip_is_continuous_at_dt_zero() {
    let scene = support::load_render_case_scene("doubao-voice-interaction");
    for event_type in ["mousemove", "touchmove"] {
        let mut session = session_starting_in(&scene, "st_push_to_talk");
        settle(&mut session);

        session.update_mouse_position(MousePosition { x: 780.0, y: 481.0 });
        let push_to_talk_outgoing = session.step(1.0 / 60.0);
        let push_to_talk_channel = channel(&push_to_talk_outgoing, PTT_ORB_PARAM_ID).clone();
        session.fire_event(pointer_event(event_type));
        let entered_cancel = session.step(0.0);
        assert_eq!(entered_cancel.current_state_id, "st_push_to_talk_cancel");
        assert_eq!(
            entered_cancel.active_transition_id.as_deref(),
            Some("tr_push_to_talk_to_cancel")
        );
        assert!(entered_cancel.diagnostics.is_empty());
        let cancel_channel = channel(&entered_cancel, PTT_ORB_PARAM_ID);
        assert_vec2_close(
            &cancel_channel.value,
            [push_to_talk_channel.value[0], push_to_talk_channel.value[1]],
            &format!("{event_type}: PushToTalk -> Cancel keeps the physical value continuous"),
        );
        assert_vec2_close(
            &cancel_channel.target_value,
            [
                push_to_talk_channel.target_value[0],
                push_to_talk_channel.target_value[1],
            ],
            &format!("{event_type}: PushToTalk -> Cancel keeps the Mutation target identical"),
        );

        settle(&mut session);
        session.update_mouse_position(MousePosition { x: 780.0, y: 480.0 });
        let cancel_outgoing = session.step(1.0 / 60.0);
        let cancel_channel = channel(&cancel_outgoing, PTT_ORB_PARAM_ID).clone();
        session.fire_event(pointer_event(event_type));
        let returned_to_push_to_talk = session.step(0.0);
        assert_eq!(returned_to_push_to_talk.current_state_id, "st_push_to_talk");
        assert_eq!(
            returned_to_push_to_talk.active_transition_id.as_deref(),
            Some("tr_cancel_to_push_to_talk")
        );
        assert!(returned_to_push_to_talk.diagnostics.is_empty());
        let push_to_talk_channel = channel(&returned_to_push_to_talk, PTT_ORB_PARAM_ID);
        assert_vec2_close(
            &push_to_talk_channel.value,
            [cancel_channel.value[0], cancel_channel.value[1]],
            &format!("{event_type}: Cancel -> PushToTalk keeps the physical value continuous"),
        );
        assert_vec2_close(
            &push_to_talk_channel.target_value,
            [
                cancel_channel.target_value[0],
                cancel_channel.target_value[1],
            ],
            &format!("{event_type}: Cancel -> PushToTalk keeps the Mutation target identical"),
        );
    }
}

#[test]
fn doubao_ptt_release_and_cancel_events_route_to_the_expected_states() {
    let scene = support::load_render_case_scene("doubao-voice-interaction");
    for (state_id, event_type, expected_state_id) in [
        ("st_push_to_talk", "mouseup", "st_thinking"),
        ("st_push_to_talk", "touchend", "st_thinking"),
        ("st_push_to_talk", "touchcancel", "st_mrerxocx_8"),
        ("st_push_to_talk_cancel", "mouseup", "st_mrerxocx_8"),
        ("st_push_to_talk_cancel", "touchend", "st_mrerxocx_8"),
        ("st_push_to_talk_cancel", "touchcancel", "st_mrerxocx_8"),
    ] {
        let mut session = session_starting_in(&scene, state_id);
        settle(&mut session);
        session.fire_event(pointer_event(event_type));
        let released = session.step(0.0);
        assert_eq!(
            released.current_state_id, expected_state_id,
            "'{event_type}' from '{state_id}'"
        );
        assert!(
            released.diagnostics.is_empty(),
            "'{event_type}' from '{state_id}': {:?}",
            released.diagnostics
        );
    }
}

#[test]
fn doubao_positions_are_derived_from_physical_p_without_a_motion_channel() {
    let scene = support::load_render_case_scene("doubao-voice-interaction");
    let machine = scene
        .state_machine
        .as_ref()
        .expect("doubao fixture should have a state machine");
    let transition = machine
        .transitions
        .iter()
        .find(|transition| transition.id == "tr_idle_to_listening")
        .expect("doubao fixture should have Idle -> Listening");
    let graph = machine
        .motion_graphs
        .iter()
        .find(|graph| graph.id == transition.motion_graph_id)
        .expect("Idle -> Listening should own a Motion Graph");

    assert!(
        machine
            .state_params
            .iter()
            .all(|param| param.name != "IntelligentLightPositions"),
        "derived positions must not be a State Param"
    );
    assert!(
        graph
            .input_bindings
            .iter()
            .all(|binding| binding.source.id() != POSITIONS_KEY)
            && graph
                .output_bindings
                .iter()
                .all(|binding| binding.state_param_id != POSITIONS_KEY),
        "derived positions must not have a property-specific Transition route"
    );
    assert!(
        graph
            .input_bindings
            .iter()
            .any(|binding| binding.source.id() == SNAP_PRIMARY_PARAM_ID),
        "the State-owned snap parameter should retain its authored Transition spring"
    );

    let mut session = AnimationSession::from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao fixture should have a state machine");
    assert_eq!(settle(&mut session).current_state_id, "st_mrerw3qg_6");

    session.fire_event(space_event("keydown"));
    session.step(0.1);
    session.fire_event(space_event("keyup"));
    session.step(0.0);
    assert_eq!(settle(&mut session).current_state_id, "st_mrerxocx_8");

    session.fire_event(space_event("keydown"));
    session.step(0.0);
    let mut first_active = None;
    for _ in 0..60 {
        let step = session.step(1.0 / 60.0);
        if step.active_transition_id.as_deref() == Some("tr_idle_to_listening") {
            first_active = Some(step);
            break;
        }
    }
    let first_active = first_active.expect("Space hold should start Idle -> Listening");
    assert!(
        first_active.diagnostics.is_empty(),
        "{:?}",
        first_active.diagnostics
    );

    let snap = channel(&first_active, SNAP_PRIMARY_PARAM_ID);
    assert_eq!(snap.transition_driver, "spring");
    assert!(
        first_active
            .motion_channels
            .iter()
            .all(|channel| channel.key != POSITIONS_KEY),
        "positions must never enter MotionEngine"
    );
    let first_x = position_x(&first_active);

    let mut later = first_active;
    for _ in 0..5 {
        later = session.step(1.0 / 60.0);
    }
    assert_ne!(
        position_x(&later),
        first_x,
        "positions must be recomputed from their changing physical-P dependencies"
    );
}

#[test]
fn doubao_listening_to_thinking_solves_q_before_transition_error() {
    for settle_listening in [false, true] {
        let scene = support::load_render_case_scene("doubao-voice-interaction");
        let machine = scene
            .state_machine
            .as_ref()
            .expect("doubao fixture should have a state machine");
        let thinking = machine
            .states
            .iter()
            .find(|state| state.id == "st_thinking")
            .expect("doubao fixture should have Thinking");
        let thinking_primary = thinking
            .state_param_overrides
            .get(SNAP_PRIMARY_PARAM_ID)
            .and_then(serde_json::Value::as_f64)
            .expect("Thinking should override primary snap");
        let thinking_secondary = thinking
            .state_param_overrides
            .get(SNAP_SECONDARY_PARAM_ID)
            .and_then(serde_json::Value::as_f64)
            .expect("Thinking should override secondary snap");
        for transition_id in ["tr_listening_to_thinking", "tr_push_to_talk_to_thinking"] {
            let transition = machine
                .transitions
                .iter()
                .find(|transition| transition.id == transition_id)
                .unwrap_or_else(|| panic!("missing transition '{transition_id}'"));
            let graph = machine
                .motion_graphs
                .iter()
                .find(|graph| graph.id == transition.motion_graph_id)
                .expect("transition should own a Motion Graph");
            for key in [SNAP_PRIMARY_PARAM_ID, SNAP_SECONDARY_PARAM_ID] {
                assert!(
                    graph
                        .input_bindings
                        .iter()
                        .all(|binding| binding.source.id() != key)
                        && graph
                            .output_bindings
                            .iter()
                            .all(|binding| binding.state_param_id != key),
                    "'{transition_id}' must let State-owned '{key}' use Any"
                );
            }
        }
        let thinking_mutation = thinking
            .mutation_graph
            .as_ref()
            .expect("Thinking should own a private Mutation graph");
        let thinking_function = thinking_mutation
            .nodes
            .iter()
            .find(|node| node.id == "function_thinking")
            .expect("Thinking Mutation Function must exist");
        assert_eq!(
            thinking_function
                .inputs
                .iter()
                .map(|port| port.id.as_str())
                .collect::<Vec<_>>(),
            vec!["localElapsedTime", "primary", "secondary"]
        );
        assert!(
            thinking_function
                .outputs
                .iter()
                .filter(|port| matches!(port.id.as_str(), "primary" | "secondary"))
                .all(|port| port.motion == Some(true)),
            "Thinking Function must return Motion values"
        );
        for (mutation_port_id, function_port_id) in [
            (SNAP_PRIMARY_PARAM_ID, "primary"),
            (SNAP_SECONDARY_PARAM_ID, "secondary"),
        ] {
            assert!(
                thinking_mutation.input_bindings.iter().any(|binding| {
                    binding.source.id() == mutation_port_id
                        && binding.to.node_id == "function_thinking"
                        && binding.to.port_id == function_port_id
                }),
                "Mutation Inputs must feed S directly to Thinking.{function_port_id}"
            );
            assert!(
                thinking_mutation.output_bindings.iter().any(|binding| {
                    binding.state_param_id == mutation_port_id
                        && binding.from.node_id == "function_thinking"
                        && binding.from.port_id == function_port_id
                }),
                "Thinking.{function_port_id} must bind its Q to the declaration output"
            );
        }
        let thinking_derivation = machine
            .derivations
            .iter()
            .find(|derivation| derivation.id == "mutation_ms2xj2gx_6")
            .expect("doubao fixture should have the Thinking Derivation");
        let derivation_function = thinking_derivation
            .nodes
            .iter()
            .find(|node| node.id == "function_ilight_thinking")
            .expect("Thinking render Derivation Function must exist");
        assert!(
            derivation_function
                .outputs
                .iter()
                .all(|port| port.motion != Some(true)),
            "Derivation Function must return only ordinary render values"
        );
        for (state_port_id, function_port_id) in [
            (SNAP_PRIMARY_PARAM_ID, "snapTargetPrimary"),
            (SNAP_SECONDARY_PARAM_ID, "snapTargetSecondary"),
        ] {
            assert!(
                thinking_derivation.input_bindings.iter().any(|binding| {
                    binding.source.id() == state_port_id
                        && binding.to.node_id == "function_ilight_thinking"
                        && binding.to.port_id == function_port_id
                }),
                "Derivation Function must consume final physical P for '{state_port_id}'"
            );
        }
        assert!(
            thinking_derivation
                .nodes
                .iter()
                .all(|node| node.id != "function_thinking"),
            "Mutation Function must not remain in the render Derivation"
        );
        let mut session = AnimationSession::from_scene(&scene)
            .expect("doubao state machine should compile")
            .expect("doubao fixture should have a state machine");
        assert_eq!(settle(&mut session).current_state_id, "st_mrerw3qg_6");

        session.fire_event(space_event("keydown"));
        session.step(0.0);
        let entered_listening = session.step(0.21);
        assert_eq!(entered_listening.current_state_id, "st_listening");
        if settle_listening {
            assert_eq!(settle(&mut session).current_state_id, "st_listening");
            for _ in 0..180 {
                session.step(1.0 / 60.0);
            }
        }

        let outgoing_listening = session.step(0.0);
        let outgoing_snap = channel(&outgoing_listening, SNAP_PRIMARY_PARAM_ID).value[0];
        session.fire_event(space_event("keyup"));
        let entered_thinking = session.step(0.0);
        assert_eq!(entered_thinking.current_state_id, "st_thinking");
        assert_eq!(
            entered_thinking.active_transition_id.as_deref(),
            Some("tr_listening_to_thinking")
        );
        let initial_snap = channel(&entered_thinking, SNAP_PRIMARY_PARAM_ID);
        assert_eq!(initial_snap.state_value, vec![thinking_primary]);
        assert_eq!(initial_snap.mutation_driver, "spring");
        assert_eq!(initial_snap.transition_driver, "spring");
        assert!(initial_snap.transition_error[0].abs() > 1.0e-6);
        assert!(
            (initial_snap.value[0]
                - (initial_snap.target_value[0] - initial_snap.transition_error[0]))
                .abs()
                <= 1.0e-9,
            "physical snap must satisfy P=Q-E"
        );
        assert!(
            entered_thinking
                .motion_channels
                .iter()
                .all(|channel| channel.key != POSITIONS_KEY),
            "positions must remain a derived render value"
        );
        assert!(
            (initial_snap.value[0] - outgoing_snap).abs() <= 1.0e-8,
            "dt=0 State handoff must preserve the semantic physical value before running the new pure Derivation"
        );

        let mut at_100ms = entered_thinking;
        for _ in 0..6 {
            at_100ms = session.step(1.0 / 60.0);
        }
        let snap_at_100ms = channel(&at_100ms, SNAP_PRIMARY_PARAM_ID);
        assert_eq!(snap_at_100ms.mutation_driver, "spring");
        assert_eq!(snap_at_100ms.transition_driver, "spring");
        assert_eq!(snap_at_100ms.state_value, vec![thinking_primary]);
        let secondary_at_100ms = channel(&at_100ms, SNAP_SECONDARY_PARAM_ID);
        assert_eq!(secondary_at_100ms.state_value, vec![thinking_secondary]);
        assert!(
            (snap_at_100ms.value[0]
                - (snap_at_100ms.target_value[0] - snap_at_100ms.transition_error[0]))
                .abs()
                <= 1.0e-9
        );

        let completed = settle(&mut session);
        assert_eq!(completed.current_state_id, "st_thinking");
        assert_eq!(completed.active_transition_id, None);
        let after_completion = session.step(0.1);
        let continuing_snap = channel(&after_completion, SNAP_PRIMARY_PARAM_ID);
        assert_eq!(continuing_snap.mutation_driver, "spring");
        assert_eq!(continuing_snap.transition_driver, "hold");
        assert_eq!(continuing_snap.transition_error, vec![0.0]);
    }
}

#[test]
fn doubao_listening_to_thinking_fixed_time_trace() {
    let scene = support::load_render_case_scene("doubao-voice-interaction");
    let expected_snap_state = scene
        .state_machine
        .as_ref()
        .and_then(|machine| {
            machine
                .states
                .iter()
                .find(|state| state.id == "st_thinking")
        })
        .and_then(|state| state.state_param_overrides.get(SNAP_PRIMARY_PARAM_ID))
        .and_then(serde_json::Value::as_f64)
        .expect("Thinking should override primary snap");
    let mut session = AnimationSession::from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao fixture should have a state machine");
    assert_eq!(settle(&mut session).current_state_id, "st_mrerw3qg_6");

    session.fire_event(space_event("keydown"));
    session.step(0.21);
    assert_eq!(settle(&mut session).current_state_id, "st_listening");
    let outgoing_listening = session.step(0.0);
    let outgoing_snap = channel(&outgoing_listening, SNAP_PRIMARY_PARAM_ID).value[0];
    session.fire_event(space_event("keyup"));

    let sample_frames = [0usize, 6, 12, 18, 33, 48];
    let mut samples = Vec::new();
    let mut step = session.step(0.0);
    for frame in 0..=48 {
        if sample_frames.contains(&frame) {
            samples.push((frame, step.clone()));
        }
        if frame < 48 {
            step = session.step(1.0 / 60.0);
        }
    }

    let expected = [
        (0.450000, 0.450000, 0.150000, 0.300000, -6.800000),
        (0.450000, 0.440524, 0.097320, 0.343204, -1.711705),
        (0.450000, 0.427383, 0.036301, 0.391083, 6.252066),
        (0.450000, 0.421226, 0.006890, 0.414337, 8.195413),
        (0.450000, 0.440943, -0.001568, 0.442510, 10.242329),
        (0.450000, 0.478415, 0.000000, 0.478415, 12.723778),
    ];
    println!("| t | value | S | Q | E | P/render | Mutation | Transition | active |");
    println!("|---:|---|---:|---:|---:|---:|---|---|---|");
    let mut trace_mismatches = Vec::new();
    for ((frame, step), expected) in samples.iter().zip(expected) {
        let sample = channel(step, SNAP_PRIMARY_PARAM_ID);
        let state = sample.state_value.first().copied().unwrap_or(f64::NAN);
        let target = sample.target_value.first().copied().unwrap_or(f64::NAN);
        let error = sample.transition_error.first().copied().unwrap_or(f64::NAN);
        let physical = sample.value.first().copied().unwrap_or(f64::NAN);
        println!(
            "| {}ms | snap.primary | {:.6} | {:.6} | {:.6} | {:.6} | {} | {} | {} |",
            frame * 1000 / 60,
            state,
            target,
            error,
            physical,
            sample.mutation_driver,
            sample.transition_driver,
            step.active_transition_id.as_deref().unwrap_or("none")
        );
        println!(
            "| {}ms | positions[0].x | — | — | — | {:.6} | derived | — | {} |",
            frame * 1000 / 60,
            position_x(step),
            step.active_transition_id.as_deref().unwrap_or("none")
        );
        assert!(
            (physical - (target - error)).abs() <= 1.0e-8,
            "snap must satisfy P=Q-E at frame {frame}"
        );
        assert!(
            step.motion_channels
                .iter()
                .all(|channel| channel.key != POSITIONS_KEY),
            "positions must not own Q/E/P at frame {frame}"
        );
        for (actual, expected) in [
            (state, expected.0),
            (target, expected.1),
            (error, expected.2),
            (physical, expected.3),
            (position_x(step), expected.4),
        ] {
            if (actual - expected).abs() > 1.0e-5 {
                trace_mismatches.push(format!("frame {frame}: expected {expected}, got {actual}"));
            }
        }
    }
    assert!(
        trace_mismatches.is_empty(),
        "fixed trace changed:\n{}",
        trace_mismatches.join("\n")
    );
    assert!(
        (channel(&samples[0].1, SNAP_PRIMARY_PARAM_ID).value[0] - outgoing_snap).abs() <= 1.0e-8,
        "Listening -> Thinking must preserve the semantic physical value at dt=0"
    );

    let initial_snap = channel(&samples[0].1, SNAP_PRIMARY_PARAM_ID);
    assert_eq!(initial_snap.state_value, vec![expected_snap_state]);
    assert_eq!(initial_snap.mutation_driver, "spring");
    assert_eq!(initial_snap.transition_driver, "spring");
    let final_snap = channel(&samples.last().unwrap().1, SNAP_PRIMARY_PARAM_ID);
    assert_eq!(final_snap.mutation_driver, "spring");
    let mut completion_frame = 48usize;
    let mut completed = step;
    while completed.active_transition_id.is_some() && completion_frame < 360 {
        completed = session.step(1.0 / 60.0);
        completion_frame += 1;
    }
    println!(
        "Transition active duration: {}ms; snap mutation driver at completion: {}",
        completion_frame * 1000 / 60,
        channel(&completed, SNAP_PRIMARY_PARAM_ID).mutation_driver
    );
    assert_eq!(completion_frame, 79, "Transition active duration changed");
    assert_eq!(completed.active_transition_id, None);
    assert_eq!(
        channel(&completed, SNAP_PRIMARY_PARAM_ID).transition_driver,
        "hold"
    );
}
