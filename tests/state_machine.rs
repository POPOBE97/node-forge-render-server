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
        Some(&serde_json::json!(0)),
        "Mutation output must not feed the next frame after its state is left"
    );
    assert_eq!(
        returned
            .overrides
            .get(&state_machine::OverrideKey::new("Vector2Input_80", "y")),
        Some(&serde_json::json!(0)),
        "Mutation output must not feed the next frame after its state is left"
    );
    let returning_value = returned
        .overrides
        .get(&state_machine::OverrideKey::new("FloatInput_81", "value"))
        .and_then(serde_json::Value::as_f64)
        .expect("return Timeline should emit a numeric presentation value");
    assert_eq!(
        returning_value, 0.0,
        "returning to a state with no new opacity target must preserve its motion snapshot"
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
fn doubao_nforge_executes_shared_driver_function_to_packed_outputs() {
    let path = support::render_case_archive("doubao-voice-interaction");
    let (scene, _asset_store) =
        node_forge_render_server::asset_store::load_from_nforge(&path).unwrap();
    let mut runtime = state_machine::compile_from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao scene should have a state machine");

    let frame = runtime.tick(1.0 / 60.0, &Default::default(), &vec![]);
    assert_eq!(frame.current_state_id, "st_mrerw3qg_6");
    assert!(frame.diagnostics.is_empty(), "{:?}", frame.diagnostics);

    let positions = frame
        .overrides
        .get(&state_machine::OverrideKey::new(
            "PackedInput_IntelligentLightPositions",
            "value",
        ))
        .and_then(serde_json::Value::as_array)
        .expect("shared Intelligent Light Mutation must output packed positions");
    let colors = frame
        .overrides
        .get(&state_machine::OverrideKey::new(
            "PackedInput_IntelligentLightColors",
            "value",
        ))
        .and_then(serde_json::Value::as_array)
        .expect("shared Intelligent Light Mutation must output packed colors");
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
}
