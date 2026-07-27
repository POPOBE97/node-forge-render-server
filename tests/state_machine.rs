//! Integration test: parse and validate the state machine from the back-pin-pin test case,
//! compile a runtime, and tick it.

use node_forge_render_server::dsl;
use node_forge_render_server::state_machine;

mod support;

fn back_pin_pin_scene() -> dsl::SceneDSL {
    support::load_render_case_scene("back-pin-pin")
}

fn editor_glass_nforge_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("node-forge-editor")
        .join("examples")
        .join("glass.nforge")
}

#[test]
fn apply_overrides_targets_only_exact_uniform_declarations() {
    let mut scene = back_pin_pin_scene();
    let template = scene
        .nodes
        .first()
        .expect("fixture must contain a node")
        .clone();
    let mut declaration = template.clone();
    declaration.id = "DeclaredUniform".into();
    declaration
        .params
        .insert("value".into(), serde_json::json!(1.0));
    let mut expanded_consumer = template;
    expanded_consumer.id = "GroupInstance/DeclaredUniform".into();
    expanded_consumer
        .params
        .insert("value".into(), serde_json::json!(2.0));
    scene.nodes = vec![declaration, expanded_consumer];

    let overrides = std::collections::HashMap::from([(
        state_machine::OverrideKey::new("DeclaredUniform", "value"),
        serde_json::json!(3.0),
    )]);
    state_machine::apply_overrides(&mut scene, &overrides);

    assert_eq!(scene.nodes[0].params["value"], serde_json::json!(3.0));
    assert_eq!(
        scene.nodes[1].params["value"],
        serde_json::json!(2.0),
        "consumer suffixes must not become implicit declaration targets"
    );
}

#[test]
fn back_pin_pin_scene_parses_state_machine() {
    let scene = back_pin_pin_scene();
    assert!(
        scene.state_machine.is_some(),
        "back-pin-pin scene should contain a stateMachine"
    );
    let sm = scene.state_machine.as_ref().unwrap();
    assert_eq!(sm.id, "sm_mmamfug8_2");
    assert_eq!(sm.states.len(), 8);
    assert_eq!(sm.mutations.len(), 1);
    assert_eq!(sm.mutation_bindings.len(), 1);
    assert_eq!(sm.initial_state_id.as_deref(), Some("st_mmamj2am_3"));
}

#[test]
fn back_pin_pin_state_machine_validates() {
    let scene = back_pin_pin_scene();
    let sm = scene.state_machine.as_ref().unwrap();
    state_machine::validate(sm).expect("state machine should be valid");
}

#[test]
fn back_pin_pin_compile_and_tick() {
    let scene = back_pin_pin_scene();
    let mut rt = state_machine::compile_from_scene(&scene)
        .expect("compile should succeed")
        .expect("runtime should be Some because scene has a state machine");

    assert_eq!(rt.current_state_id(), "st_mmamj2am_3");

    // Without mousedown event the transition to the Mutation-bound State should NOT fire.
    let result = rt.tick(0.016, &Default::default(), &vec![]);
    assert_eq!(result.current_state_id, "st_mmamj2am_3");

    // Fire mousedown — the logical state changes immediately. This transition
    // has no state target values, so the AnimationEngine has no channels to run;
    // the target State's post-motion Mutation starts on the same tick.
    let result = rt.tick(0.016, &Default::default(), &vec!["mousedown".into()]);
    assert_eq!(result.current_state_id, "st_mmamj4me_7");
    assert_eq!(result.active_transition_id, None);

    // The state-local time Mutation continues to update independently.
    let result = rt.tick(2.4, &Default::default(), &vec![]);
    assert_eq!(result.current_state_id, "st_mmamj4me_7");
    assert!(!result.finished);
}

#[test]
fn editor_glass_nforge_any_state_mousedown_updates_mouse_override() {
    use node_forge_render_server::state_machine::types::{
        AnimationStateType, TransitionMotionNode,
    };

    let path = editor_glass_nforge_path();
    if !path.exists() {
        eprintln!(
            "Skipping editor glass.nforge state-machine test; file not found at {}",
            path.display()
        );
        return;
    }

    let (scene, _asset_store) =
        node_forge_render_server::asset_store::load_from_nforge(&path).unwrap();
    let (transition_id, transition_source, transition_target, initial_state_id) = {
        let sm = scene
            .state_machine
            .as_ref()
            .expect("glass.nforge should contain a stateMachine");

        let initial_state_id = sm
            .initial_state_id
            .clone()
            .expect("glass.nforge should have an initial state");

        let mousedown_to_mutation = sm
            .transitions
            .iter()
            .find(|transition| {
                let source_type = sm
                    .states
                    .iter()
                    .find(|state| state.id == transition.source)
                    .map(|state| state.resolved_type());
                let trigger_matches = sm
                    .motion_graphs
                    .iter()
                    .find(|graph| graph.id == transition.motion_graph_id)
                    .is_some_and(|graph| {
                        graph.nodes.iter().any(|node| {
                            matches!(
                                node,
                                TransitionMotionNode::EventTrigger { event_type, .. }
                                    if event_type == "mousedown"
                            )
                        })
                    });

                source_type == Some(AnimationStateType::AnimationState)
                    && sm
                        .mutation_bindings
                        .iter()
                        .any(|binding| binding.state_id == transition.target)
                    && trigger_matches
            })
            .expect("glass.nforge should have a mousedown transition to a Mutation-bound State");

        (
            mousedown_to_mutation.id.clone(),
            mousedown_to_mutation.source.clone(),
            mousedown_to_mutation.target.clone(),
            initial_state_id,
        )
    };

    let mut rt = state_machine::compile_from_scene(&scene)
        .expect("compile should succeed")
        .expect("runtime should be Some because glass.nforge has a state machine");
    assert_eq!(rt.current_state_id(), initial_state_id);

    let idle = rt.tick(0.016, &Default::default(), &vec![]);
    assert_eq!(idle.current_state_id, transition_source);
    assert_eq!(idle.active_transition_id, None);

    let triggered = rt.tick(0.016, &Default::default(), &vec!["mousedown".into()]);
    assert!(
        triggered.current_state_id == transition_source
            || triggered.current_state_id == transition_target,
        "mousedown should either start or complete the transition"
    );
    if triggered.current_state_id == transition_source {
        assert_eq!(
            triggered.active_transition_id.as_deref(),
            Some(transition_id.as_str())
        );
    }

    rt.set_mouse_position(state_machine::MousePosition { x: 111.0, y: 222.0 });
    let completed = rt.tick(0.4, &Default::default(), &vec![]);
    assert_eq!(completed.current_state_id, transition_target);
    assert_eq!(
        completed
            .overrides
            .get(&state_machine::OverrideKey::new("Vector2Input_80", "x")),
        Some(&serde_json::json!(111.0))
    );
    assert_eq!(
        completed
            .overrides
            .get(&state_machine::OverrideKey::new("Vector2Input_80", "y")),
        Some(&serde_json::json!(222.0))
    );

    rt.set_mouse_position(state_machine::MousePosition { x: 333.0, y: 444.0 });
    let dragged = rt.tick(0.016, &Default::default(), &vec!["mousemove".into()]);
    assert_eq!(dragged.current_state_id, transition_target);
    assert_eq!(
        dragged
            .overrides
            .get(&state_machine::OverrideKey::new("Vector2Input_80", "x")),
        Some(&serde_json::json!(333.0))
    );
    assert_eq!(
        dragged
            .overrides
            .get(&state_machine::OverrideKey::new("Vector2Input_80", "y")),
        Some(&serde_json::json!(444.0))
    );

    let returned = rt.tick(0.016, &Default::default(), &vec!["mouseup".into()]);
    assert_eq!(returned.current_state_id, transition_source);
    assert_eq!(
        returned
            .overrides
            .get(&state_machine::OverrideKey::new("Vector2Input_80", "x")),
        Some(&serde_json::json!(333.0)),
        "Transition interruption must preserve the MotionEngine physical value"
    );
    assert_eq!(
        returned
            .overrides
            .get(&state_machine::OverrideKey::new("Vector2Input_80", "y")),
        Some(&serde_json::json!(444.0)),
        "Transition interruption must preserve the MotionEngine physical value"
    );
    let returning_value = returned
        .overrides
        .get(&state_machine::OverrideKey::new("FloatInput_81", "value"))
        .and_then(serde_json::Value::as_f64)
        .expect("return Timeline should emit a numeric presentation value");
    assert!(
        returning_value > 0.0,
        "Transition error must preserve opacity continuity before settling"
    );

    let settled = rt.tick(0.4, &Default::default(), &vec![]);
    assert_eq!(
        settled
            .overrides
            .get(&state_machine::OverrideKey::new("FloatInput_81", "value")),
        Some(&serde_json::json!(0.0))
    );
}

#[test]
fn back_pin_pin_apply_overrides_no_crash() {
    let mut scene = back_pin_pin_scene();
    let mut rt = state_machine::compile_from_scene(&scene).unwrap().unwrap();

    // Fire mousedown and advance past transition to get mutation overrides.
    rt.tick(0.016, &Default::default(), &vec!["mousedown".into()]);
    let result = rt.tick(0.7, &Default::default(), &vec![]);
    state_machine::apply_overrides(&mut scene, &result.overrides);

    // Verify scene is still intact.
    assert!(!scene.nodes.is_empty());
}

#[test]
fn back_pin_pin_state_types_correct() {
    use node_forge_render_server::state_machine::types::AnimationStateType;

    let scene = back_pin_pin_scene();
    let sm = scene.state_machine.as_ref().unwrap();

    let types: Vec<(String, AnimationStateType)> = sm
        .states
        .iter()
        .map(|s| (s.id.clone(), s.resolved_type()))
        .collect();

    assert!(
        types
            .iter()
            .any(|(_, t)| *t == AnimationStateType::EntryState)
    );
    assert!(
        types
            .iter()
            .any(|(_, t)| *t == AnimationStateType::AnyState)
    );
    assert!(
        types
            .iter()
            .any(|(_, t)| *t == AnimationStateType::ExitState)
    );
    assert!(sm.states.iter().any(|state| state.mutation_id.is_some()));
}

#[test]
fn back_pin_pin_state_owned_mutations_reference_valid_definitions() {
    let scene = back_pin_pin_scene();
    let sm = scene.state_machine.as_ref().unwrap();

    let mutation_ids: Vec<&str> = sm.mutations.iter().map(|m| m.id.as_str()).collect();
    for s in &sm.states {
        if s.mutation_id.is_some() {
            let mid = s.mutation_id.as_deref().unwrap();
            assert!(
                mutation_ids.contains(&mid),
                "state '{}' references missing mutation '{}'",
                s.id,
                mid,
            );
        }
    }
}

#[test]
fn doubao_nforge_executes_shared_driver_function_into_motion_targets() {
    let path = support::render_case_archive("doubao-voice-interaction");
    let (scene, _asset_store) =
        node_forge_render_server::asset_store::load_from_nforge(&path).unwrap();
    let function = state_machine::mutation_function::installed_document_functions()
        .into_iter()
        .find(|function| {
            function.scope == "mutation:mutation_ilight_idle"
                && function.node_id == "function_ilight_idle"
        })
        .expect("shared Intelligent Light Mutation Function must be installed");
    assert_eq!(
        function
            .inputs
            .iter()
            .filter(|input| input.motion)
            .count(),
        11,
        "every derived Intelligent Light uniform must be a MotionParam"
    );
    assert_eq!(
        function.source.matches(".setTo(").count(),
        11,
        "every derived Intelligent Light uniform must be written explicitly"
    );
    let mut runtime = state_machine::compile_from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao scene should have a state machine");

    let frame = runtime.tick(1.0 / 60.0, &Default::default(), &vec![]);
    assert_eq!(frame.current_state_id, "st_mrerw3qg_6");
    assert!(frame.diagnostics.is_empty(), "{:?}", frame.diagnostics);

    let positions = frame
        .overrides
        .get(&state_machine::OverrideKey::new(
            "Vector2ArrayInput_IntelligentLightPositions",
            "value",
        ))
        .and_then(serde_json::Value::as_array)
        .expect("shared Intelligent Light Mutation must set packed positions")
        .clone();
    let colors = frame
        .overrides
        .get(&state_machine::OverrideKey::new(
            "ColorArrayInput_IntelligentLightColors",
            "value",
        ))
        .and_then(serde_json::Value::as_array)
        .expect("shared Intelligent Light Mutation must set packed colors");
    assert_eq!(positions.len(), 11);
    assert_eq!(colors.len(), 11);
    assert!(positions.iter().all(|value| {
        value.as_array().is_some_and(|components| {
            components.len() == 2
                && components
                    .iter()
                    .all(|component| component.as_f64().is_some())
        })
    }));
    assert!(colors.iter().all(|value| {
        value.as_array().is_some_and(|components| {
            components.len() == 4
                && components
                    .iter()
                    .all(|component| component.as_f64().is_some())
        })
    }));

    let next_frame = runtime.tick(1.0 / 60.0, &Default::default(), &vec![]);
    let next_positions = next_frame
        .overrides
        .get(&state_machine::OverrideKey::new(
            "Vector2ArrayInput_IntelligentLightPositions",
            "value",
        ))
        .and_then(serde_json::Value::as_array)
        .expect("MotionEngine must retain the next packed position target");
    assert_ne!(
        &positions, next_positions,
        "scene-time-driven setTo calls must update MotionEngine across frames"
    );
}

#[test]
fn doubao_thinking_mutation_retargets_snap_springs() {
    let path = support::render_case_archive("doubao-voice-interaction");
    let (scene, _asset_store) =
        node_forge_render_server::asset_store::load_from_nforge(&path).unwrap();
    let mut runtime = state_machine::compile_from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao scene should have a state machine");
    runtime.force_state("st_thinking").unwrap();

    let mut minimum_target = f64::INFINITY;
    let mut maximum_target = f64::NEG_INFINITY;
    let mut minimum_position_x = f64::INFINITY;
    let mut maximum_position_x = f64::NEG_INFINITY;
    for frame_index in 0..120 {
        let frame = runtime.tick(1.0 / 60.0, &Default::default(), &vec![]);
        assert!(frame.diagnostics.is_empty(), "{:?}", frame.diagnostics);
        let channel = frame
            .motion_channels
            .iter()
            .find(|channel| {
                channel.key == "FloatInput_IntelligentLightSnapTargetTPrimary:value"
            })
            .expect("Thinking snap target must have a MotionEngine channel");
        assert_eq!(channel.mutation_driver, "spring");
        assert_eq!(channel.transition_error, vec![0.0]);
        let target = channel.target_value[0];
        minimum_target = minimum_target.min(target);
        maximum_target = maximum_target.max(target);
        let position_x = frame
            .overrides
            .get(&state_machine::OverrideKey::new(
                "Vector2ArrayInput_IntelligentLightPositions",
                "value",
            ))
            .and_then(serde_json::Value::as_array)
            .and_then(|positions| positions.first())
            .and_then(serde_json::Value::as_array)
            .and_then(|position| position.first())
            .and_then(serde_json::Value::as_f64)
            .expect("Thinking Mutation must update rendered Intelligent Light positions");
        minimum_position_x = minimum_position_x.min(position_x);
        maximum_position_x = maximum_position_x.max(position_x);
        assert!(
            frame_index == 0 || target != 0.5,
            "forced-state preview must not reset the Mutation spring every frame"
        );
    }
    assert!(minimum_target < 0.48, "{minimum_target}");
    assert!(maximum_target > 0.52, "{maximum_target}");
    assert!(
        maximum_position_x - minimum_position_x > 5.0,
        "visible positions did not follow the snap spring: {minimum_position_x}..{maximum_position_x}"
    );
}

#[test]
fn doubao_listening_to_thinking_keeps_following_after_transition() {
    let path = support::render_case_archive("doubao-voice-interaction");
    let (scene, _asset_store) =
        node_forge_render_server::asset_store::load_from_nforge(&path).unwrap();
    let mut runtime = state_machine::compile_from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao scene should have a state machine");

    let key_down = state_machine::FiredEvent {
        event_type: "keydown".into(),
        key: Some(" ".into()),
        ..Default::default()
    };
    runtime.tick(1.0 / 60.0, &Default::default(), &vec![key_down]);
    for _ in 0..30 {
        if runtime.current_state_id() == "st_listening" {
            break;
        }
        runtime.tick(1.0 / 60.0, &Default::default(), &vec![]);
    }
    assert_eq!(runtime.current_state_id(), "st_listening");

    let key_up = state_machine::FiredEvent {
        event_type: "keyup".into(),
        key: Some(" ".into()),
        ..Default::default()
    };
    let entered = runtime.tick(1.0 / 60.0, &Default::default(), &vec![key_up]);
    assert_eq!(entered.current_state_id, "st_thinking");

    let mut saw_active_transition = false;
    let mut saw_completed_transition = false;
    let mut post_transition_minimum_target = f64::INFINITY;
    let mut post_transition_maximum_target = f64::NEG_INFINITY;
    let mut minimum_position_x = f64::INFINITY;
    let mut maximum_position_x = f64::NEG_INFINITY;
    for _ in 0..180 {
        let frame = runtime.tick(1.0 / 60.0, &Default::default(), &vec![]);
        assert!(frame.diagnostics.is_empty(), "{:?}", frame.diagnostics);
        let snap = frame
            .motion_channels
            .iter()
            .find(|channel| {
                channel.key == "FloatInput_IntelligentLightSnapTargetTPrimary:value"
            })
            .expect("Thinking snap target must have a MotionEngine channel");
        if frame.active_transition_id.is_some() {
            saw_active_transition = true;
        } else if frame.current_state_id == "st_thinking" {
            saw_completed_transition = true;
            post_transition_minimum_target =
                post_transition_minimum_target.min(snap.target_value[0]);
            post_transition_maximum_target =
                post_transition_maximum_target.max(snap.target_value[0]);
        }
        let position_x = frame
            .overrides
            .get(&state_machine::OverrideKey::new(
                "Vector2ArrayInput_IntelligentLightPositions",
                "value",
            ))
            .and_then(serde_json::Value::as_array)
            .and_then(|positions| positions.first())
            .and_then(serde_json::Value::as_array)
            .and_then(|position| position.first())
            .and_then(serde_json::Value::as_f64)
            .expect("Thinking Mutation must update rendered Intelligent Light positions");
        minimum_position_x = minimum_position_x.min(position_x);
        maximum_position_x = maximum_position_x.max(position_x);
    }
    assert!(saw_active_transition);
    assert!(saw_completed_transition);
    assert!(
        post_transition_maximum_target - post_transition_minimum_target > 0.02,
        "Mutation spring stopped after Transition completion"
    );
    assert!(
        maximum_position_x - minimum_position_x > 5.0,
        "visible positions did not move through the transition"
    );
}
