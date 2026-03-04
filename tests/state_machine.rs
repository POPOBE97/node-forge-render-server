//! Integration test: parse and validate the state machine from the back-pin-pin test case,
//! compile a runtime, and tick it.

use node_forge_render_server::dsl;
use node_forge_render_server::state_machine;

fn scene_json_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("cases")
        .join("back-pin-pin")
        .join("scene.json")
}

#[test]
fn back_pin_pin_scene_parses_state_machine() {
    let scene = dsl::load_scene_from_path(scene_json_path()).unwrap();
    assert!(
        scene.state_machine.is_some(),
        "back-pin-pin scene should contain a stateMachine"
    );
    let sm = scene.state_machine.as_ref().unwrap();
    assert_eq!(sm.id, "sm_mmamfug8_2");
    assert_eq!(sm.states.len(), 4);
    assert_eq!(sm.mutations.len(), 1);
    assert_eq!(sm.initial_state_id.as_deref(), Some("st_mmamj2am_3"));
}

#[test]
fn back_pin_pin_state_machine_validates() {
    let scene = dsl::load_scene_from_path(scene_json_path()).unwrap();
    let sm = scene.state_machine.as_ref().unwrap();
    state_machine::validate(sm).expect("state machine should be valid");
}

#[test]
fn back_pin_pin_compile_and_tick() {
    let scene = dsl::load_scene_from_path(scene_json_path()).unwrap();
    let mut rt = state_machine::compile_from_scene(&scene)
        .expect("compile should succeed")
        .expect("runtime should be Some because scene has a state machine");

    assert_eq!(rt.current_state_id(), "st_mmamj2am_3");

    // Without mousedown event the entry→mutation transition should NOT fire.
    let result = rt.tick(0.016, &Default::default(), &vec![]);
    assert_eq!(result.current_state_id, "st_mmamj2am_3");

    // Fire mousedown — transition fires (delay + duration = 0.6s total).
    let result = rt.tick(0.016, &Default::default(), &vec!["mousedown".into()]);
    assert_eq!(result.current_state_id, "st_mmamj2am_3");
    assert!(result.active_transition_id.is_some());

    // Advance past delay (0.3s) + duration (0.3s) = 0.6s total.
    let result = rt.tick(0.7, &Default::default(), &vec![]);
    assert_eq!(result.current_state_id, "st_mmamj4me_7");
    assert!(!result.finished);
}

#[test]
fn back_pin_pin_apply_overrides_no_crash() {
    let mut scene = dsl::load_scene_from_path(scene_json_path()).unwrap();
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

    let scene = dsl::load_scene_from_path(scene_json_path()).unwrap();
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
    assert!(
        types
            .iter()
            .any(|(_, t)| *t == AnimationStateType::MutationNode)
    );
}

#[test]
fn back_pin_pin_mutation_node_references_valid_mutation() {
    use node_forge_render_server::state_machine::types::AnimationStateType;

    let scene = dsl::load_scene_from_path(scene_json_path()).unwrap();
    let sm = scene.state_machine.as_ref().unwrap();

    let mutation_ids: Vec<&str> = sm.mutations.iter().map(|m| m.id.as_str()).collect();
    for s in &sm.states {
        if s.resolved_type() == AnimationStateType::MutationNode {
            let mid = s.mutation_id.as_deref().unwrap();
            assert!(
                mutation_ids.contains(&mid),
                "mutationNode '{}' references missing mutation '{}'",
                s.id,
                mid,
            );
        }
    }
}
