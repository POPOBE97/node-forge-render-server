use node_forge_render_server::animation::{AnimationSession, AnimationStep};
use node_forge_render_server::state_machine::FiredEvent;

mod support;

const POSITIONS_KEY: &str = "Vector2ArrayInput_IntelligentLightPositions:value";
const SNAP_PRIMARY_KEY: &str = "FloatInput_IntelligentLightSnapTargetTPrimary:value";

fn space_event(event_type: &str) -> FiredEvent {
    FiredEvent {
        event_type: event_type.into(),
        key: Some(" ".into()),
        ..Default::default()
    }
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
        graph.inputs.iter().all(|port| port.id != POSITIONS_KEY)
            && graph.outputs.iter().all(|port| port.id != POSITIONS_KEY),
        "derived positions must not have an authored Transition boundary port"
    );
    assert!(
        graph
            .input_bindings
            .iter()
            .all(|binding| binding.port_id != POSITIONS_KEY)
            && graph
                .output_bindings
                .iter()
                .all(|binding| binding.port_id != POSITIONS_KEY),
        "derived positions must not have a property-specific Transition route"
    );
    assert!(
        graph
            .input_bindings
            .iter()
            .any(|binding| binding.port_id == SNAP_PRIMARY_KEY),
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

    let snap = channel(&first_active, SNAP_PRIMARY_KEY);
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
            .parameter_overrides
            .get(SNAP_PRIMARY_KEY)
            .and_then(serde_json::Value::as_f64)
            .expect("Thinking should override primary snap");
        let thinking_secondary = thinking
            .parameter_overrides
            .get("FloatInput_IntelligentLightSnapTargetTSecondary:value")
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
            for key in [
                SNAP_PRIMARY_KEY,
                "FloatInput_IntelligentLightSnapTargetTSecondary:value",
            ] {
                assert!(
                    graph
                        .input_bindings
                        .iter()
                        .all(|binding| binding.port_id != key)
                        && graph
                            .output_bindings
                            .iter()
                            .all(|binding| binding.port_id != key),
                    "'{transition_id}' must let State-owned '{key}' use Any"
                );
            }
        }
        let thinking_mutation = machine
            .mutations
            .iter()
            .find(|mutation| mutation.id == "mutation_ms2xj2gx_6")
            .expect("doubao fixture should have the Thinking Mutation");
        let thinking_function = thinking_mutation
            .nodes
            .iter()
            .find(|node| node.id == "function_thinking")
            .expect("Thinking target Function must exist");
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
            (SNAP_PRIMARY_KEY, "primary"),
            (
                "FloatInput_IntelligentLightSnapTargetTSecondary:value",
                "secondary",
            ),
        ] {
            assert!(
                thinking_mutation.input_bindings.iter().any(|binding| {
                    binding.port_id == mutation_port_id
                        && binding.to.node_id == "function_thinking"
                        && binding.to.port_id == function_port_id
                }),
                "Mutation Inputs must feed S directly to Thinking.{function_port_id}"
            );
            assert!(
                thinking_mutation.output_bindings.iter().any(|binding| {
                    binding.port_id == mutation_port_id
                        && binding.from.node_id == "function_thinking"
                        && binding.from.port_id == function_port_id
                }),
                "Thinking.{function_port_id} must bind its Q to the declaration output"
            );
        }
        for (mutation_port_id, function_port_id) in [
            (SNAP_PRIMARY_KEY, "snapTargetPrimary"),
            (
                "FloatInput_IntelligentLightSnapTargetTSecondary:value",
                "snapTargetSecondary",
            ),
        ] {
            assert!(
                thinking_mutation.connections.iter().any(|connection| {
                    connection.from.node_id == "function_thinking"
                        && connection.from.port_id
                            == if function_port_id == "snapTargetPrimary" {
                                "primary"
                            } else {
                                "secondary"
                            }
                        && connection.to.node_id == "function_ilight_thinking"
                        && connection.to.port_id == function_port_id
                }),
                "downstream Function must visibly consume M1(S) for '{mutation_port_id}'"
            );
        }
        assert!(
            thinking_mutation.input_bindings.iter().all(|binding| {
                !(binding.to.node_id == "function_ilight_thinking"
                    && matches!(
                        binding.to.port_id.as_str(),
                        "snapTargetPrimary" | "snapTargetSecondary"
                    ))
            }),
            "downstream Q dependencies must not be hidden as duplicate boundary bindings"
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
        session.fire_event(space_event("keyup"));
        let entered_thinking = session.step(0.0);
        assert_eq!(entered_thinking.current_state_id, "st_thinking");
        assert_eq!(
            entered_thinking.active_transition_id.as_deref(),
            Some("tr_listening_to_thinking")
        );
        let initial_snap = channel(&entered_thinking, SNAP_PRIMARY_KEY);
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
            (position_x(&entered_thinking) - position_x(&outgoing_listening)).abs() <= 1.0e-8,
            "dt=0 State handoff must preserve positions because their physical dependencies are continuous"
        );

        let mut at_100ms = entered_thinking;
        for _ in 0..6 {
            at_100ms = session.step(1.0 / 60.0);
        }
        let snap_at_100ms = channel(&at_100ms, SNAP_PRIMARY_KEY);
        assert_eq!(snap_at_100ms.mutation_driver, "spring");
        assert_eq!(snap_at_100ms.transition_driver, "spring");
        assert_eq!(snap_at_100ms.state_value, vec![thinking_primary]);
        let secondary_at_100ms = channel(
            &at_100ms,
            "FloatInput_IntelligentLightSnapTargetTSecondary:value",
        );
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
        let continuing_snap = channel(&after_completion, SNAP_PRIMARY_KEY);
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
        .and_then(|state| state.parameter_overrides.get(SNAP_PRIMARY_KEY))
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
    let outgoing_position_x = position_x(&outgoing_listening);
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
        (0.350000, 0.350000, 0.050000, 0.300000, 42.800000),
        (0.350000, 0.360085, 0.032440, 0.327645, 37.607028),
        (0.350000, 0.378861, 0.012100, 0.366761, 29.058241),
        (0.350000, 0.401348, 0.002297, 0.399052, 25.771770),
        (0.350000, 0.476790, 0.000000, 0.476790, 19.739991),
        (0.350000, 0.556887, 0.000000, 0.556887, 13.735355),
    ];
    println!("| t | value | S | Q | E | P/render | Mutation | Transition | active |");
    println!("|---:|---|---:|---:|---:|---:|---|---|---|");
    for ((frame, step), expected) in samples.iter().zip(expected) {
        let sample = channel(step, SNAP_PRIMARY_KEY);
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
            assert!(
                (actual - expected).abs() <= 1.0e-5,
                "fixed trace changed at frame {frame}: expected {expected}, got {actual}"
            );
        }
    }
    assert!(
        (position_x(&samples[0].1) - outgoing_position_x).abs() <= 1.0e-8,
        "Listening -> Thinking must not jump positions at dt=0"
    );

    let initial_snap = channel(&samples[0].1, SNAP_PRIMARY_KEY);
    assert_eq!(initial_snap.state_value, vec![expected_snap_state]);
    assert_eq!(initial_snap.mutation_driver, "spring");
    assert_eq!(initial_snap.transition_driver, "spring");
    let final_snap = channel(&samples.last().unwrap().1, SNAP_PRIMARY_KEY);
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
        channel(&completed, SNAP_PRIMARY_KEY).mutation_driver
    );
    assert_eq!(completion_frame, 79, "Transition active duration changed");
    assert_eq!(completed.active_transition_id, None);
    assert_eq!(
        channel(&completed, SNAP_PRIMARY_KEY).transition_driver,
        "hold"
    );
}
