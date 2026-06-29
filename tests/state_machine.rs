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

fn editor_glass_nforge_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("node-forge-editor")
        .join("packages")
        .join("editor")
        .join("assets")
        .join("examples")
        .join("glass.nforge")
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
    assert_eq!(sm.states.len(), 7);
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

    // Fire mousedown — transition fires (delay + duration = 2.3s total).
    let result = rt.tick(0.016, &Default::default(), &vec!["mousedown".into()]);
    assert_eq!(result.current_state_id, "st_mmamj2am_3");
    assert!(result.active_transition_id.is_some());

    // Advance past delay (0.3s) + duration (2.0s) = 2.3s total.
    let result = rt.tick(2.4, &Default::default(), &vec![]);
    assert_eq!(result.current_state_id, "st_mmamj4me_7");
    assert!(!result.finished);
}

#[test]
fn editor_glass_nforge_mousedown_transition_fires_from_entry_state() {
    use node_forge_render_server::state_machine::types::{AnimationStateType, TransitionCondition};

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
    let sm = scene
        .state_machine
        .as_ref()
        .expect("glass.nforge should contain a stateMachine");

    let mousedown_to_mutation = sm
        .transitions
        .iter()
        .find(|transition| {
            let source_type = sm
                .states
                .iter()
                .find(|state| state.id == transition.source)
                .map(|state| state.resolved_type());
            let target_type = sm
                .states
                .iter()
                .find(|state| state.id == transition.target)
                .map(|state| state.resolved_type());
            let trigger_matches = matches!(
                transition.trigger.as_ref(),
                Some(TransitionCondition::Event { event_name }) if event_name == "mousedown"
            );

            matches!(
                source_type,
                Some(AnimationStateType::AnyState | AnimationStateType::EntryState)
            ) && target_type == Some(AnimationStateType::MutationNode)
                && trigger_matches
        })
        .expect("glass.nforge should have an Entry/Any -> Mutation mousedown transition");

    let mut rt = state_machine::compile_from_scene(&scene)
        .expect("compile should succeed")
        .expect("runtime should be Some because glass.nforge has a state machine");
    let initial_state_id = rt.current_state_id().to_string();

    let idle = rt.tick(0.016, &Default::default(), &vec![]);
    assert_eq!(idle.current_state_id, initial_state_id);
    assert_eq!(idle.active_transition_id, None);

    let triggered = rt.tick(0.016, &Default::default(), &vec!["mousedown".into()]);
    assert_eq!(triggered.current_state_id, initial_state_id);
    assert_eq!(
        triggered.active_transition_id.as_deref(),
        Some(mousedown_to_mutation.id.as_str())
    );

    let completed = rt.tick(0.4, &Default::default(), &vec![]);
    assert_eq!(completed.current_state_id, mousedown_to_mutation.target);
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
