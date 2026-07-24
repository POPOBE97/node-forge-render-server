use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use node_forge_render_server::animation::{AnimationSession, AnimationStep};
use node_forge_render_server::state_machine::types::{
    AnimationState, AnimationStateType, AnimationTransition, EventModifiers, MutationEndpoint,
    Position, StateMachine, TransitionConditionBinding, TransitionMotionGraph,
    TransitionMotionNode,
};
use node_forge_render_server::state_machine::{
    AnimationTraceFrame, AnimationTraceLog, EventSchedule, FiredEvent, ScheduledEvent,
    TickSchedule, build_initial_values, canonicalize_json_value, round_f64, tracked_override_keys,
};
use node_forge_render_server::{asset_store, dsl};

mod support;

fn event_motion_graph(id: &str, event_type: &str) -> TransitionMotionGraph {
    let mut graph = TransitionMotionGraph::instant(id);
    graph.nodes.push(TransitionMotionNode::EventTrigger {
        id: "trigger".into(),
        position: Position::default(),
        label: None,
        event_type: event_type.into(),
        key: None,
        modifiers: EventModifiers::default(),
        ignore_repeat: true,
    });
    graph.condition_binding = Some(TransitionConditionBinding::Node {
        from: MutationEndpoint {
            node_id: "trigger".into(),
            port_id: "fired".into(),
        },
    });
    graph
}

fn space_event(event_type: &str) -> FiredEvent {
    FiredEvent {
        event_type: event_type.into(),
        key: Some(" ".into()),
        ..Default::default()
    }
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn cases_root() -> PathBuf {
    manifest_dir().join("tests").join("fixtures").join("render")
}

fn discover_case_dirs() -> Vec<PathBuf> {
    let root = cases_root();
    let mut dirs = Vec::new();

    for group in ["editor-examples", "renderer-only"] {
        let group_dir = root.join(group);
        let entries = std::fs::read_dir(&group_dir)
            .unwrap_or_else(|e| panic!("failed to read cases dir {}: {e}", group_dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || path.join("SKIP_RENDER_CASE").exists() {
                continue;
            }
            dirs.push(path);
        }
    }

    dirs.sort();
    dirs
}

fn case_name(case_dir: &Path) -> String {
    case_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("<unknown>")
        .to_string()
}

fn load_case_scene(case_dir: &Path) -> Option<dsl::SceneDSL> {
    let nforge = case_dir.join("scene.nforge");
    if !nforge.exists() {
        return None;
    }
    let (scene, _store) = asset_store::load_from_nforge(&nforge)
        .unwrap_or_else(|e| panic!("failed to load {}: {e:#}", nforge.display()));
    Some(scene)
}

fn write_trace(path: &Path, trace: &AnimationTraceLog) {
    let text = serde_json::to_string_pretty(trace)
        .unwrap_or_else(|e| panic!("failed to serialize trace {}: {e}", path.display()));
    std::fs::write(path, format!("{text}\n"))
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
}

fn first_trace_mismatch(
    case_name: &str,
    expected: &AnimationTraceLog,
    actual: &AnimationTraceLog,
) -> Option<String> {
    if expected.schema_version != actual.schema_version {
        return Some(format!(
            "case {case_name}: schema_version mismatch expected={} actual={}",
            expected.schema_version, actual.schema_version
        ));
    }
    if expected.start_secs != actual.start_secs {
        return Some(format!(
            "case {case_name}: start_secs mismatch expected={} actual={}",
            expected.start_secs, actual.start_secs
        ));
    }
    if expected.end_secs != actual.end_secs {
        return Some(format!(
            "case {case_name}: end_secs mismatch expected={} actual={}",
            expected.end_secs, actual.end_secs
        ));
    }
    if expected.fps != actual.fps {
        return Some(format!(
            "case {case_name}: fps mismatch expected={} actual={}",
            expected.fps, actual.fps
        ));
    }
    if expected.include_end != actual.include_end {
        return Some(format!(
            "case {case_name}: include_end mismatch expected={} actual={}",
            expected.include_end, actual.include_end
        ));
    }
    if expected.frame_count != actual.frame_count {
        return Some(format!(
            "case {case_name}: frame_count mismatch expected={} actual={}",
            expected.frame_count, actual.frame_count
        ));
    }
    if expected.tracked_keys != actual.tracked_keys {
        return Some(format!(
            "case {case_name}: tracked_keys mismatch expected={:?} actual={:?}",
            expected.tracked_keys, actual.tracked_keys
        ));
    }
    if expected.frames.len() != actual.frames.len() {
        return Some(format!(
            "case {case_name}: frames length mismatch expected={} actual={}",
            expected.frames.len(),
            actual.frames.len()
        ));
    }

    for (i, (ef, af)) in expected.frames.iter().zip(actual.frames.iter()).enumerate() {
        if ef.frame_index != af.frame_index {
            return Some(format!(
                "case {case_name} frame {i}: frame_index mismatch expected={} actual={}",
                ef.frame_index, af.frame_index
            ));
        }
        if ef.time_secs != af.time_secs {
            return Some(format!(
                "case {case_name} frame {i}: time_secs mismatch expected={} actual={}",
                ef.time_secs, af.time_secs
            ));
        }
        if ef.dt_secs != af.dt_secs {
            return Some(format!(
                "case {case_name} frame {i}: dt_secs mismatch expected={} actual={}",
                ef.dt_secs, af.dt_secs
            ));
        }
        if ef.current_state_id != af.current_state_id {
            return Some(format!(
                "case {case_name} frame {i}: current_state_id mismatch expected={} actual={}",
                ef.current_state_id, af.current_state_id
            ));
        }
        if ef.state_local_times != af.state_local_times {
            return Some(format!(
                "case {case_name} frame {i}: state_local_times mismatch expected={:?} actual={:?}",
                ef.state_local_times, af.state_local_times
            ));
        }
        if ef.scene_time_secs != af.scene_time_secs {
            return Some(format!(
                "case {case_name} frame {i}: scene_time_secs mismatch expected={} actual={}",
                ef.scene_time_secs, af.scene_time_secs
            ));
        }
        if ef.active_transition_id != af.active_transition_id {
            return Some(format!(
                "case {case_name} frame {i}: active_transition_id mismatch expected={:?} actual={:?}",
                ef.active_transition_id, af.active_transition_id
            ));
        }
        if ef.finished != af.finished {
            return Some(format!(
                "case {case_name} frame {i}: finished mismatch expected={} actual={}",
                ef.finished, af.finished
            ));
        }
        if ef.values != af.values {
            // Find first differing key for a helpful message.
            let all_keys: BTreeSet<&String> = ef.values.keys().chain(af.values.keys()).collect();
            for key in all_keys {
                let ev = ef.values.get(key);
                let av = af.values.get(key);
                if ev != av {
                    return Some(format!(
                        "case {case_name} frame {i}: values[{key}] mismatch expected={:?} actual={:?}",
                        ev, av
                    ));
                }
            }
        }
    }

    None
}

/// Generate a trace using `AnimationSession` (fixed-step clock) instead of
/// the raw `generate_trace_for_scene_with_events` path.
fn generate_trace_via_session(
    scene: &dsl::SceneDSL,
    schedule: &TickSchedule,
    event_schedule: &[ScheduledEvent],
) -> AnimationTraceLog {
    let mut session = AnimationSession::from_scene(scene)
        .expect("failed to build AnimationSession")
        .expect("scene has no stateMachine");

    let tracked_key_set = tracked_override_keys(session.runtime().definition());
    let tracked_keys: Vec<String> = tracked_key_set.iter().cloned().collect();

    let mut current_values = build_initial_values(scene, &tracked_keys);
    let mut frames: Vec<AnimationTraceFrame> = Vec::with_capacity(schedule.frame_count());

    for sample in schedule.samples() {
        // Fire events scheduled for this frame.
        for ev in event_schedule {
            if ev.frame_index == sample.frame_index {
                session.fire_event(&ev.event_name);
            }
        }

        let step = session.step(sample.dt_secs);

        // Apply overrides to current values.
        for (key, value) in &step.active_overrides {
            let trace_key = format!("{}:{}", key.node_id, key.param_name);
            current_values.insert(trace_key, canonicalize_json_value(value));
        }

        let mut frame_values: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for key in &tracked_keys {
            let value = current_values
                .get(key)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            frame_values.insert(key.clone(), canonicalize_json_value(&value));
        }

        let state_local_times: BTreeMap<String, f64> = step
            .state_local_times
            .iter()
            .map(|(k, v)| (k.clone(), round_f64(*v)))
            .collect();

        frames.push(AnimationTraceFrame {
            frame_index: sample.frame_index,
            time_secs: round_f64(sample.time_secs),
            dt_secs: round_f64(sample.dt_secs),
            current_state_id: step.current_state_id.clone(),
            state_local_times,
            scene_time_secs: round_f64(step.scene_time_secs),
            active_transition_id: step.active_transition_id.clone(),
            motion_channels: step.motion_channels.clone(),
            finished: step.finished,
            diagnostics: step.diagnostics.clone(),
            values: frame_values,
        });
    }

    AnimationTraceLog {
        schema_version: 1,
        start_secs: round_f64(schedule.start_secs),
        end_secs: round_f64(schedule.end_secs),
        fps: schedule.fps,
        include_end: schedule.include_end,
        frame_count: frames.len(),
        tracked_keys,
        frames,
    }
}

fn sticky_override_test_scene() -> dsl::SceneDSL {
    dsl::SceneDSL {
        version: "1.0".into(),
        metadata: dsl::Metadata {
            name: "Sticky Override Test".into(),
            created: None,
            modified: None,
        },
        nodes: vec![dsl::Node {
            id: "Target".into(),
            node_type: "FloatInput".into(),
            params: [("value".into(), serde_json::json!(0.0))]
                .into_iter()
                .collect(),
            inputs: vec![],
            outputs: vec![],
            input_bindings: vec![],
            wgsl_override: None,
        }],
        connections: vec![],
        outputs: None,
        groups: vec![],
        assets: HashMap::new(),
        state_machine: Some(StateMachine {
            id: "sm_sticky".into(),
            name: "Sticky".into(),
            states: vec![
                AnimationState {
                    id: "entry".into(),
                    name: "Entry".into(),
                    position: None,
                    parameter_overrides: Default::default(),
                    state_type: AnimationStateType::EntryState,
                    mutation_id: None,
                },
                AnimationState {
                    id: "any".into(),
                    name: "Any".into(),
                    position: None,
                    parameter_overrides: Default::default(),
                    state_type: AnimationStateType::AnyState,
                    mutation_id: None,
                },
                AnimationState {
                    id: "exit".into(),
                    name: "Exit".into(),
                    position: None,
                    parameter_overrides: Default::default(),
                    state_type: AnimationStateType::ExitState,
                    mutation_id: None,
                },
                AnimationState {
                    id: "a".into(),
                    name: "A".into(),
                    position: None,
                    parameter_overrides: [("Target:value".into(), serde_json::json!(5.0))]
                        .into_iter()
                        .collect(),
                    state_type: AnimationStateType::AnimationState,
                    mutation_id: None,
                },
                AnimationState {
                    id: "b".into(),
                    name: "B".into(),
                    position: None,
                    parameter_overrides: Default::default(),
                    state_type: AnimationStateType::AnimationState,
                    mutation_id: None,
                },
            ],
            transitions: vec![
                AnimationTransition {
                    id: "entry_to_a".into(),
                    source: "entry".into(),
                    target: "a".into(),
                    motion_graph_id: "motion_entry_to_a".into(),
                },
                AnimationTransition {
                    id: "a_to_b".into(),
                    source: "a".into(),
                    target: "b".into(),
                    motion_graph_id: "motion_a_to_b".into(),
                },
            ],
            mutation_bindings: vec![],
            mutations: vec![],
            motion_graphs: vec![
                TransitionMotionGraph::instant("motion_entry_to_a"),
                event_motion_graph("motion_a_to_b", "go"),
            ],
            initial_state_id: Some("entry".into()),
            viewport: None,
        }),
        debug_artifacts: None,
    }
}

#[test]
fn animation_session_keeps_values_when_next_state_omits_override() {
    let scene = sticky_override_test_scene();
    let mut session = AnimationSession::from_scene(&scene)
        .expect("animation session should compile")
        .expect("scene should have a stateMachine");

    let first = session.step(0.0);
    assert_eq!(first.current_state_id, "a");
    assert_eq!(
        first
            .active_overrides
            .get(&node_forge_render_server::state_machine::OverrideKey::new(
                "Target", "value"
            )),
        Some(&serde_json::json!(5.0))
    );

    session.fire_event("go");
    let second = session.step(1.0 / 60.0);
    assert_eq!(second.current_state_id, "b");
    assert_eq!(
        second
            .active_overrides
            .get(&node_forge_render_server::state_machine::OverrideKey::new(
                "Target", "value"
            )),
        Some(&serde_json::json!(5.0))
    );

    let restores = session.reset();
    assert_eq!(
        restores.get(&node_forge_render_server::state_machine::OverrideKey::new(
            "Target", "value"
        )),
        Some(&serde_json::json!(0.0))
    );
}

#[test]
fn doubao_off_to_idle_fixture_uses_per_property_springs_and_snaps() {
    let case_dir = support::render_case_dir("doubao-voice-interaction");
    let scene = load_case_scene(&case_dir).expect("doubao fixture should load");
    let mut session = AnimationSession::from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao fixture should have a state machine");

    let entered_off = session.step(0.0);
    assert_eq!(entered_off.current_state_id, "st_mrerw3qg_6");
    let mut settled_off = entered_off;
    for _ in 0..180 {
        settled_off = session.step(1.0 / 60.0);
    }
    assert_eq!(settled_off.current_state_id, "st_mrerw3qg_6");
    assert_eq!(
        settled_off.active_overrides.get(
            &node_forge_render_server::state_machine::OverrideKey::new("Vector2Input_35", "x")
        ),
        Some(&serde_json::json!(216.0))
    );

    session.fire_event(space_event("keydown"));
    session.step(0.1);
    session.fire_event(space_event("keyup"));
    let started = session.step(0.0);
    assert_eq!(started.current_state_id, "st_mrerxocx_8");
    assert_eq!(
        started.active_transition_id.as_deref(),
        Some("tr_mrery48v_a"),
        "channels={:?} diagnostics={:?}",
        started.motion_channels,
        started.diagnostics
    );

    let drivers: BTreeMap<&str, &str> = started
        .motion_channels
        .iter()
        .map(|channel| (channel.key.as_str(), channel.driver.as_str()))
        .collect();
    for key in [
        "FloatInput_40:value",
        "FloatInput_41:value",
        "Vector2Input_35:x",
        "Vector2Input_35:y",
        "Vector2Input_36:y",
        "Vector2Input_38:x",
        "Vector2Input_38:y",
    ] {
        assert_eq!(drivers.get(key), Some(&"spring"), "wrong driver for {key}");
    }

    let mut completed = started;
    for _ in 0..240 {
        if completed.active_transition_id.is_none() {
            break;
        }
        completed = session.step(1.0 / 60.0);
    }
    assert_eq!(completed.active_transition_id, None);
    assert_eq!(completed.current_state_id, "st_mrerxocx_8");

    let expected = [
        ("FloatInput_38", "value", serde_json::json!(480.0)),
        ("FloatInput_39", "value", serde_json::json!(0.0)),
        ("FloatInput_40", "value", serde_json::json!(0.0)),
        ("FloatInput_41", "value", serde_json::json!(512.0)),
        ("Vector2Input_35", "x", serde_json::json!(1008.0)),
        ("Vector2Input_35", "y", serde_json::json!(168.0)),
        ("Vector2Input_36", "x", serde_json::json!(540.0)),
        ("Vector2Input_36", "y", serde_json::json!(186.0)),
        ("FloatInput_37", "value", serde_json::json!(60.0)),
        ("Vector2Input_38", "x", serde_json::json!(1008.0)),
        ("Vector2Input_38", "y", serde_json::json!(168.0)),
        ("FloatInput_42", "value", serde_json::json!(1.0)),
        ("FloatInput_43", "value", serde_json::json!(1.0)),
        ("FloatInput_44", "value", serde_json::json!(0.3)),
        ("FloatInput_45", "value", serde_json::json!(0.0)),
        ("FloatInput_46", "value", serde_json::json!(1.0)),
        ("FloatInput_47", "value", serde_json::json!(1.0)),
        ("FloatInput_48", "value", serde_json::json!(0.0)),
        ("FloatInput_49", "value", serde_json::json!(0.0)),
        ("FloatInput_50", "value", serde_json::json!(0.0)),
    ];
    for (node_id, param_name, value) in expected {
        assert_eq!(
            completed.active_overrides.get(
                &node_forge_render_server::state_machine::OverrideKey::new(node_id, param_name)
            ),
            Some(&value),
            "final snap mismatch for {node_id}:{param_name}"
        );
    }
}

#[test]
fn doubao_listening_transitions_animate_ui_opacity_and_snap_all_channels() {
    let case_dir = support::render_case_dir("doubao-voice-interaction");
    let scene = load_case_scene(&case_dir).expect("doubao fixture should load");
    for (from_node, from_port, to_node, to_port) in [
        (
            "ImageTexture_InputBarUI",
            "color",
            "ShaderMaterial_InputBarUI",
            "param:ui_color",
        ),
        ("FloatInput_42", "value", "GroupInstance_51", "in_2"),
        (
            "GroupInstance_51",
            "out_0",
            "node_default_composite",
            "dynamic_input_bar_ui",
        ),
    ] {
        assert!(
            scene
                .connections
                .iter()
                .chain(
                    scene
                        .groups
                        .iter()
                        .flat_map(|group| group.connections.iter()),
                )
                .any(|connection| {
                    connection.from.node_id == from_node
                        && connection.from.port_id == from_port
                        && connection.to.node_id == to_node
                        && connection.to.port_id == to_port
                }),
            "missing Listening UI connection {from_node}.{from_port} -> {to_node}.{to_port}"
        );
    }
    let composite = scene
        .nodes
        .iter()
        .find(|node| node.id == "node_default_composite")
        .expect("doubao fixture should have a Composite node");
    assert_eq!(
        composite
            .inputs
            .iter()
            .map(|port| port.id.as_str())
            .collect::<Vec<_>>(),
        [
            "dynamic_1783678358530_1",
            "dynamic_input_bar_ui",
            "dynamic_1784530828769_2",
            "dynamic_ptt_prompt",
        ],
        "Composite dynamic inputs must remain Glass -> UI -> Light -> PTT Prompt"
    );

    let settle = |session: &mut AnimationSession| {
        let mut snapshot = session.step(0.0);
        for _ in 0..240 {
            if snapshot.active_transition_id.is_none() {
                break;
            }
            snapshot = session.step(1.0 / 60.0);
        }
        snapshot
    };

    let enter_off = |session: &mut AnimationSession| {
        let entered = session.step(0.0);
        assert_eq!(entered.current_state_id, "st_mrerw3qg_6");
        settle(session)
    };

    let assert_listening_values = |snapshot: &AnimationStep| {
        let expected = [
            ("FloatInput_42", serde_json::json!(0.0)),
            ("FloatInput_43", serde_json::json!(1.0)),
            ("FloatInput_44", serde_json::json!(0.3)),
            ("FloatInput_45", serde_json::json!(0.0)),
            ("FloatInput_46", serde_json::json!(1.0)),
            ("FloatInput_47", serde_json::json!(0.0)),
            ("FloatInput_48", serde_json::json!(1.0)),
            ("FloatInput_49", serde_json::json!(1.0)),
            ("FloatInput_50", serde_json::json!(1.0)),
        ];
        for (node_id, value) in expected {
            assert_eq!(
                snapshot.active_overrides.get(
                    &node_forge_render_server::state_machine::OverrideKey::new(node_id, "value")
                ),
                Some(&value),
                "Listening final snap mismatch for {node_id}:value"
            );
        }
    };

    let mut off_session = AnimationSession::from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao fixture should have a state machine");
    let settled_off = enter_off(&mut off_session);
    assert_eq!(settled_off.current_state_id, "st_mrerw3qg_6");

    off_session.fire_event(space_event("keydown"));
    off_session.step(0.0);
    let off_to_listening = off_session.step(0.21);
    assert_eq!(off_to_listening.current_state_id, "st_listening");
    assert_eq!(
        off_to_listening.active_transition_id.as_deref(),
        Some("tr_off_to_listening")
    );
    let off_drivers: BTreeMap<&str, &str> = off_to_listening
        .motion_channels
        .iter()
        .map(|channel| (channel.key.as_str(), channel.driver.as_str()))
        .collect();
    for key in [
        "FloatInput_40:value",
        "FloatInput_41:value",
        "Vector2Input_35:x",
        "Vector2Input_35:y",
        "Vector2Input_36:y",
        "Vector2Input_38:x",
        "Vector2Input_38:y",
    ] {
        assert_eq!(
            off_drivers.get(key),
            Some(&"spring"),
            "wrong Off -> Listening driver for {key}"
        );
    }
    let completed_off_to_listening = settle(&mut off_session);
    assert_eq!(completed_off_to_listening.active_transition_id, None);
    assert_listening_values(&completed_off_to_listening);

    let mut idle_session = AnimationSession::from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao fixture should have a state machine");
    enter_off(&mut idle_session);
    idle_session.fire_event(space_event("keydown"));
    idle_session.step(0.1);
    idle_session.fire_event(space_event("keyup"));
    idle_session.step(0.0);
    let idle = settle(&mut idle_session);
    assert_eq!(idle.current_state_id, "st_mrerxocx_8");
    assert_eq!(
        idle.active_overrides
            .get(&node_forge_render_server::state_machine::OverrideKey::new(
                "FloatInput_42",
                "value"
            )),
        Some(&serde_json::json!(1.0))
    );

    idle_session.fire_event(space_event("keydown"));
    idle_session.step(0.0);
    let idle_to_listening = idle_session.step(0.21);
    assert_eq!(idle_to_listening.current_state_id, "st_listening");
    assert_eq!(
        idle_to_listening.active_transition_id.as_deref(),
        Some("tr_idle_to_listening")
    );
    let ui_opacity = idle_to_listening
        .motion_channels
        .iter()
        .find(|channel| channel.key == "FloatInput_42:value")
        .expect("InputBarUiOpacity should have a motion channel");
    assert_eq!(ui_opacity.driver, "spring");
    assert!(!ui_opacity.completed);

    let completed_idle_to_listening = settle(&mut idle_session);
    assert_eq!(completed_idle_to_listening.active_transition_id, None);
    assert_listening_values(&completed_idle_to_listening);
}

#[test]
fn doubao_shared_intelligent_light_mutation_advances_with_global_scene_time() {
    let case_dir = support::render_case_dir("doubao-voice-interaction");
    let scene = load_case_scene(&case_dir).expect("doubao fixture should load");
    let machine = scene
        .state_machine
        .as_ref()
        .expect("doubao fixture should have a state machine");
    assert_eq!(
        machine.mutations.len(),
        1,
        "expected one shared Mutation scope"
    );
    assert_eq!(machine.mutation_bindings.len(), 7);
    assert!(
        machine
            .mutation_bindings
            .iter()
            .all(|binding| binding.mutation_node_id == "mutation_node_st_mrerxocx_8"),
        "all logical States must use the shared Intelligent Light Mutation node"
    );

    let mut session = AnimationSession::from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao fixture should have a state machine");
    let mut snapshot = session.step(0.0);
    for _ in 0..240 {
        if snapshot.active_transition_id.is_none() {
            break;
        }
        snapshot = session.step(1.0 / 60.0);
    }
    assert_eq!(snapshot.current_state_id, "st_mrerw3qg_6");

    session.fire_event(space_event("keydown"));
    session.step(0.1);
    session.fire_event(space_event("keyup"));
    session.step(0.0);
    for _ in 0..240 {
        snapshot = session.step(1.0 / 60.0);
        if snapshot.current_state_id == "st_mrerxocx_8" && snapshot.active_transition_id.is_none() {
            break;
        }
    }
    assert_eq!(snapshot.current_state_id, "st_mrerxocx_8");
    assert_eq!(snapshot.active_transition_id, None);

    let positions_key = node_forge_render_server::state_machine::OverrideKey::new(
        "PackedInput_IntelligentLightPositions",
        "value",
    );
    let before = snapshot
        .active_overrides
        .get(&positions_key)
        .cloned()
        .expect("shared Mutation should produce Intelligent Light positions");
    assert_eq!(
        before.as_array().map(Vec::len),
        Some(11),
        "Intelligent Light must produce exactly 11 positions"
    );
    let before_scene_time = snapshot.scene_time_secs;

    let advanced = session.step(1.0 / 30.0);
    let after = advanced
        .active_overrides
        .get(&positions_key)
        .expect("positions should remain available on subsequent Idle frames");
    assert_ne!(
        &before, after,
        "Idle positions must advance across fixed-step boundaries"
    );
    assert!(advanced.scene_time_secs > before_scene_time);
    assert!(
        advanced.diagnostics.is_empty(),
        "Mutation evaluation emitted diagnostics: {:?}",
        advanced.diagnostics
    );
}

#[test]
fn forced_doubao_state_keeps_mutation_running_without_routing() {
    let case_dir = support::render_case_dir("doubao-voice-interaction");
    let scene = load_case_scene(&case_dir).expect("doubao fixture should load");
    let mut session = AnimationSession::from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao fixture should have a state machine");
    let state_id = "st_mrerw3qg_6";

    let initial = session
        .force_state(state_id)
        .expect("ordinary State should be forceable");
    assert_eq!(initial.current_state_id, state_id);
    assert_eq!(initial.active_transition_id, None);

    session.fire_event(space_event("keydown"));
    let advanced = session.step(0.5);
    assert_eq!(advanced.current_state_id, state_id);
    assert_eq!(advanced.active_transition_id, None);
    assert_eq!(advanced.scene_time_secs, 0.5);
    assert_eq!(advanced.state_local_times.get(state_id), Some(&0.5));
    assert!(
        advanced.active_overrides.contains_key(
            &node_forge_render_server::state_machine::OverrideKey::new(
                "PackedInput_IntelligentLightPositions",
                "value",
            ),
        ),
        "forced State should continue evaluating its Mutation overlay"
    );
}

#[test]
fn doubao_idle_intelligent_light_matches_voice_interaction_for_ten_seconds() {
    let case_dir = support::render_case_dir("doubao-voice-interaction");
    let (mut scene, _asset_store) = support::load_render_case("doubao-voice-interaction");
    let mut machine = scene
        .state_machine
        .take()
        .expect("doubao fixture should have a state machine");
    machine.initial_state_id = Some("st_mrerxocx_8".into());
    let mut runtime = node_forge_render_server::state_machine::StateMachineRuntime::new(machine);
    let no_events = Vec::new();

    let golden_path = support::expected_path(&case_dir, "idle_voice_interaction_golden.json");
    let golden: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", golden_path.display())),
    )
    .unwrap_or_else(|error| panic!("failed to parse {}: {error}", golden_path.display()));
    assert_eq!(
        golden["motionParameters"].as_array().map(Vec::len),
        Some(69)
    );
    let frames = golden["frames"]
        .as_array()
        .expect("Idle golden must contain frames");
    assert_eq!(
        frames.len(),
        601,
        "10 seconds at 60 fps must include 601 frames"
    );

    let positions_key = node_forge_render_server::state_machine::OverrideKey::new(
        "PackedInput_IntelligentLightPositions",
        "value",
    );
    let colors_key = node_forge_render_server::state_machine::OverrideKey::new(
        "PackedInput_IntelligentLightColors",
        "value",
    );
    let mut max_position_error = 0.0_f64;
    let mut max_color_error = 0.0_f64;

    for (index, expected) in frames.iter().enumerate() {
        let dt = if index == 0 { 0.0 } else { 1.0 / 60.0 };
        let actual = runtime.tick(dt, &Default::default(), &no_events);
        assert_eq!(actual.current_state_id, "st_mrerxocx_8");
        assert!(
            actual.diagnostics.is_empty(),
            "frame {index} Mutation diagnostics: {:?}",
            actual.diagnostics
        );
        for (key, field, max_error) in [
            (&positions_key, "positions", &mut max_position_error),
            (&colors_key, "colors", &mut max_color_error),
        ] {
            let actual_rows = actual.overrides[key]
                .as_array()
                .unwrap_or_else(|| panic!("frame {index} has no packed {field}"));
            let expected_rows = expected[field]
                .as_array()
                .unwrap_or_else(|| panic!("golden frame {index} has no {field}"));
            assert_eq!(actual_rows.len(), expected_rows.len());
            for (row_index, (actual_row, expected_row)) in
                actual_rows.iter().zip(expected_rows).enumerate()
            {
                let actual_components = actual_row.as_array().unwrap();
                let expected_components = expected_row.as_array().unwrap();
                assert_eq!(actual_components.len(), expected_components.len());
                for (component_index, (actual_value, expected_value)) in actual_components
                    .iter()
                    .zip(expected_components)
                    .enumerate()
                {
                    let error =
                        (actual_value.as_f64().unwrap() - expected_value.as_f64().unwrap()).abs();
                    *max_error = (*max_error).max(error);
                    assert!(
                        error <= 1.0e-5,
                        "frame {index} {field}[{row_index}][{component_index}] error {error}"
                    );
                }
            }
        }
    }

    assert!(max_position_error <= 1.0e-5);
    assert_eq!(max_color_error, 0.0);
}

#[test]
fn animation_value_traces_match_goldens() {
    let mut failures: Vec<String> = Vec::new();

    for case_dir in discover_case_dirs() {
        let name = case_name(&case_dir);
        let golden_path = support::expected_path(&case_dir, "animation_values.json");
        if !golden_path.exists() {
            continue;
        }

        let scene = match load_case_scene(&case_dir) {
            Some(s) => s,
            None => {
                failures.push(format!("case {name}: no scene.nforge"));
                continue;
            }
        };

        // Load golden text and extract schedule metadata (top-level
        // fields only) so we can generate the actual trace even if
        // the golden uses an older frame schema.
        let golden_text = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|e| panic!("case {name}: failed to read golden: {e}"));
        let golden_json: serde_json::Value = serde_json::from_str(&golden_text)
            .unwrap_or_else(|e| panic!("case {name}: failed to parse golden JSON: {e}"));

        let start_secs = golden_json["start_secs"].as_f64().unwrap_or(0.0);
        let end_secs = golden_json["end_secs"].as_f64().unwrap_or(10.0);
        let fps = golden_json["fps"].as_u64().unwrap_or(60) as u32;
        let include_end = golden_json["include_end"].as_bool().unwrap_or(true);

        // Load event schedule if present.
        let events_path = case_dir.join("events.json");
        let event_schedule: Vec<ScheduledEvent> = if events_path.exists() {
            let text = std::fs::read_to_string(&events_path)
                .unwrap_or_else(|e| panic!("case {name}: failed to read events.json: {e}"));
            let es: EventSchedule = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("case {name}: failed to parse events.json: {e}"));
            es.events
        } else {
            vec![]
        };

        // Build schedule from golden metadata.
        let schedule = TickSchedule::new(start_secs, end_secs, fps, include_end)
            .unwrap_or_else(|e| panic!("case {name}: invalid schedule from golden: {e}"));

        // Generate actual trace via AnimationSession (the actual run path)
        // so the test validates the same code path the app uses at runtime.
        let actual = generate_trace_via_session(&scene, &schedule, &event_schedule);

        // Always write actual to out/.
        let out_dir = case_dir.join("out");
        std::fs::create_dir_all(&out_dir)
            .unwrap_or_else(|e| panic!("case {name}: failed to create out dir: {e}"));
        let out_path = out_dir.join("animation_values.json");
        write_trace(&out_path, &actual);

        // Try to parse golden into the current schema for comparison.
        // If the golden uses an older schema, report it as a mismatch
        // (the user needs to update the golden).
        match serde_json::from_value::<AnimationTraceLog>(golden_json) {
            Ok(golden) => {
                if let Some(mismatch) = first_trace_mismatch(&name, &golden, &actual) {
                    failures.push(format!(
                        "{mismatch}\n  golden: {}\n  actual: {}",
                        golden_path.display(),
                        out_path.display()
                    ));
                }
            }
            Err(e) => {
                failures.push(format!(
                    "case {name}: golden schema mismatch (needs update): {e}\n  golden: {}\n  actual: {}",
                    golden_path.display(),
                    out_path.display()
                ));
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "animation value trace mismatches:\n\n{}",
            failures.join("\n\n")
        );
    }
}
