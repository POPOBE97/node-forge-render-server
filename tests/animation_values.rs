use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use node_forge_render_server::animation::{AnimationSession, AnimationStep};
use node_forge_render_server::state_machine::types::{
    AnimationState, AnimationStateType, AnimationTransition, DerivationDefinition,
    DerivationPassthroughBinding, DerivationStateBinding, EventModifiers, GpuUniformRef,
    GraphEndpoint, GraphPort, Position, StateMachine, StateMutationGraph, StateMutationGraphLayout,
    StateParamDeclaration, StateParamGraph, StateValueSource, TimelineMotionNode,
    TransitionConditionBinding, TransitionMotionGraph, TransitionMotionNode,
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
        from: GraphEndpoint {
            node_id: "trigger".into(),
            port_id: "fired".into(),
        },
    });
    graph
}

fn empty_state_mutation() -> StateMutationGraph {
    let target = GraphPort {
        id: "target_value".into(),
        name: Some("Target".into()),
        port_type: Some("float".into()),
        array_length: None,
        motion: None,
    };
    StateMutationGraph {
        inputs: vec![target.clone()],
        outputs: vec![target],
        nodes: vec![],
        connections: vec![],
        input_bindings: vec![],
        output_bindings: vec![],
        layout: StateMutationGraphLayout {
            parameter_positions: HashMap::new(),
            runtime_input_position: Position::default(),
            output_position: Position::default(),
            runtime_input_collapsed: false,
            output_collapsed: false,
        },
        viewport: None,
    }
}

fn space_event(event_type: &str) -> FiredEvent {
    FiredEvent {
        event_type: event_type.into(),
        key: Some(" ".into()),
        ..Default::default()
    }
}

fn state_param_id<'a>(machine: &'a StateMachine, name: &str) -> &'a str {
    machine
        .state_params
        .iter()
        .find(|param| param.name == name)
        .map(|param| param.id.as_str())
        .unwrap_or_else(|| panic!("missing State Param '{name}'"))
}

fn state_override<'a>(
    machine: &'a StateMachine,
    state_id: &str,
    state_param_id: &str,
) -> &'a serde_json::Value {
    let state = machine
        .states
        .iter()
        .find(|state| state.id == state_id)
        .unwrap_or_else(|| panic!("missing canonical State '{state_id}'"));
    state
        .state_param_overrides
        .get(state_param_id)
        .unwrap_or_else(|| {
            panic!(
                "State '{state_id}' has no canonical override for State Param '{state_param_id}'"
            )
        })
}

fn passthrough_state_param_for_uniform<'a>(
    machine: &'a StateMachine,
    state_id: &str,
    node_id: &str,
    param_name: &str,
) -> &'a str {
    let any_state_id = machine
        .states
        .iter()
        .find(|state| state.state_type == AnimationStateType::AnyState)
        .map(|state| state.id.as_str())
        .expect("State Machine should have an Any State");
    let binding = machine
        .derivation_bindings
        .iter()
        .find(|binding| binding.state_id == state_id)
        .or_else(|| {
            machine
                .derivation_bindings
                .iter()
                .find(|binding| binding.state_id == any_state_id)
        })
        .unwrap_or_else(|| {
            panic!("State '{state_id}' has no direct or Any State fallback Derivation binding")
        });
    let derivation_id = machine
        .states
        .iter()
        .find(|state| state.id == binding.derivation_node_id)
        .and_then(|state| state.derivation_id.as_deref())
        .unwrap_or_else(|| {
            panic!(
                "Derivation node '{}' for State '{state_id}' is missing",
                binding.derivation_node_id
            )
        });
    let derivation = machine
        .derivations
        .iter()
        .find(|derivation| derivation.id == derivation_id)
        .unwrap_or_else(|| panic!("missing Derivation '{derivation_id}'"));
    let passthrough = derivation
        .passthrough_bindings
        .iter()
        .find(|binding| {
            binding.uniform.node_id == node_id && binding.uniform.param_id == param_name
        })
        .unwrap_or_else(|| {
            panic!(
                "Derivation '{derivation_id}' has no State Param passthrough to \
                 '{node_id}:{param_name}'"
            )
        });
    match &passthrough.source {
        StateValueSource::StateParam { state_param_id } => state_param_id,
        StateValueSource::FrameInput { frame_input_id } => {
            panic!(
                "GPU uniform '{node_id}:{param_name}' is sourced from frame input \
                 '{frame_input_id}', not a State Param"
            )
        }
    }
}

fn assert_state_override_values(
    snapshot: &AnimationStep,
    machine: &StateMachine,
    state_id: &str,
    keys: &[(&str, &str)],
) {
    for &(node_id, param_name) in keys {
        let state_param_id =
            passthrough_state_param_for_uniform(machine, state_id, node_id, param_name);
        let expected = state_override(machine, state_id, state_param_id);
        let actual = snapshot
            .active_overrides
            .get(&node_forge_render_server::state_machine::OverrideKey::new(
                node_id, param_name,
            ))
            .unwrap_or_else(|| {
                panic!("final snapshot has no override for {state_id} {node_id}:{param_name}")
            });
        if let (Some(actual), Some(expected)) = (actual.as_f64(), expected.as_f64()) {
            assert!(
                (actual - expected).abs() <= 1.0e-9,
                "final snap mismatch for {state_id} {node_id}:{param_name}: \
                 expected {expected}, got {actual}"
            );
        } else {
            assert_eq!(
                actual, expected,
                "final snap mismatch for {state_id} {node_id}:{param_name}"
            );
        }
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
        version: "4.0".into(),
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
            state_params: vec![StateParamDeclaration {
                id: "target_value".into(),
                name: "Target value".into(),
                param_type: "float".into(),
                default_value: serde_json::json!(0.0),
                array_length: None,
            }],
            state_param_graph: StateParamGraph {
                declaration_positions: [("target_value".into(), Position::default())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            states: vec![
                AnimationState {
                    id: "entry".into(),
                    name: "Entry".into(),
                    position: None,
                    state_param_overrides: Default::default(),
                    state_type: AnimationStateType::EntryState,
                    mutation_graph: None,
                    derivation_id: None,
                },
                AnimationState {
                    id: "any".into(),
                    name: "Any".into(),
                    position: None,
                    state_param_overrides: Default::default(),
                    state_type: AnimationStateType::AnyState,
                    mutation_graph: None,
                    derivation_id: None,
                },
                AnimationState {
                    id: "exit".into(),
                    name: "Exit".into(),
                    position: None,
                    state_param_overrides: Default::default(),
                    state_type: AnimationStateType::ExitState,
                    mutation_graph: None,
                    derivation_id: None,
                },
                AnimationState {
                    id: "a".into(),
                    name: "A".into(),
                    position: None,
                    state_param_overrides: [("target_value".into(), serde_json::json!(5.0))]
                        .into_iter()
                        .collect(),
                    state_type: AnimationStateType::AnimationState,
                    mutation_graph: Some(empty_state_mutation()),
                    derivation_id: None,
                },
                AnimationState {
                    id: "b".into(),
                    name: "B".into(),
                    position: None,
                    state_param_overrides: Default::default(),
                    state_type: AnimationStateType::AnimationState,
                    mutation_graph: Some(empty_state_mutation()),
                    derivation_id: None,
                },
                AnimationState {
                    id: "target_derivation_node".into(),
                    name: "Target Derivation".into(),
                    position: None,
                    state_param_overrides: Default::default(),
                    state_type: AnimationStateType::DerivationNode,
                    mutation_graph: None,
                    derivation_id: Some("target_derivation".into()),
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
            derivation_bindings: vec![
                DerivationStateBinding {
                    id: "derive_a".into(),
                    state_id: "a".into(),
                    derivation_node_id: "target_derivation_node".into(),
                },
                DerivationStateBinding {
                    id: "derive_b".into(),
                    state_id: "b".into(),
                    derivation_node_id: "target_derivation_node".into(),
                },
            ],
            derivations: vec![DerivationDefinition {
                id: "target_derivation".into(),
                name: "Target Derivation".into(),
                inputs: vec![],
                outputs: vec![],
                nodes: vec![],
                connections: vec![],
                input_bindings: vec![],
                output_bindings: vec![],
                passthrough_bindings: vec![DerivationPassthroughBinding {
                    source: StateValueSource::StateParam {
                        state_param_id: "target_value".into(),
                    },
                    uniform: GpuUniformRef {
                        node_id: "Target".into(),
                        param_id: "value".into(),
                    },
                }],
                layout: None,
                viewport: None,
            }],
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
    let _function_registry = support::function_registry_lock();
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
fn pinned_state_change_uses_authored_transition_without_resetting_the_clock() {
    let _function_registry = support::function_registry_lock();
    let mut scene = sticky_override_test_scene();
    let machine = scene
        .state_machine
        .as_mut()
        .expect("test scene should have a state machine");
    machine
        .states
        .iter_mut()
        .find(|state| state.id == "b")
        .expect("test scene should have State B")
        .state_param_overrides
        .insert("target_value".into(), serde_json::json!(10.0));
    let graph = machine
        .motion_graphs
        .iter_mut()
        .find(|graph| graph.id == "motion_a_to_b")
        .expect("test scene should have the A-to-B Motion Graph");
    assert!(
        graph.condition_binding.is_some(),
        "pin selection should explicitly choose the route without firing its event condition"
    );
    let motion_node = graph
        .nodes
        .iter_mut()
        .find(|node| node.id() == "motion")
        .expect("A-to-B Motion Graph should have a motion node");
    *motion_node = TransitionMotionNode::Linear {
        timeline: TimelineMotionNode {
            id: "motion".into(),
            position: Position::default(),
            label: None,
            duration: 1.0,
            delay: 0.0,
            blending: None,
        },
    };

    let mut session = AnimationSession::from_scene(&scene)
        .expect("animation session should compile")
        .expect("scene should have a State Machine");
    let pinned_a = session
        .force_state("a")
        .expect("State A should be pinnable");
    assert_eq!(pinned_a.current_state_id, "a");
    assert_eq!(pinned_a.active_transition_id, None);
    session.step(0.25);

    let started = session
        .force_state("b")
        .expect("State B should be pinnable");
    assert_eq!(started.current_state_id, "b");
    assert_eq!(started.active_transition_id.as_deref(), Some("a_to_b"));
    assert_eq!(started.scene_time_secs, 0.25);
    let started_channel = started
        .motion_channels
        .iter()
        .find(|channel| channel.key == "target_value")
        .expect("target_value should have a MotionEngine channel");
    assert_eq!(started_channel.value, vec![5.0]);
    assert_eq!(started_channel.target_value, vec![10.0]);
    assert_eq!(started_channel.transition_driver, "timeline");

    let midway = session.step(0.5);
    let midway_channel = midway
        .motion_channels
        .iter()
        .find(|channel| channel.key == "target_value")
        .expect("target_value should remain visible during the Transition");
    assert_eq!(midway.active_transition_id.as_deref(), Some("a_to_b"));
    assert!(midway_channel.value[0] > 5.0 && midway_channel.value[0] < 10.0);
}

#[test]
fn doubao_off_to_idle_fixture_uses_per_property_springs_and_snaps() {
    let _function_registry = support::function_registry_lock();
    let case_dir = support::render_case_dir("doubao-voice-interaction");
    let scene = load_case_scene(&case_dir).expect("doubao fixture should load");
    let machine = scene
        .state_machine
        .as_ref()
        .expect("doubao fixture should have a state machine");
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
    assert_state_override_values(
        &settled_off,
        machine,
        "st_mrerw3qg_6",
        &[("Vector2Input_35", "x")],
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
    for name in [
        "GradientBlurStartSigma",
        "GradientBlurEndSigma",
        "InputBarSizePx.x",
        "InputBarSizePx.y",
        "InputBarPositionPx.y",
        "LightBloomSizePx.x",
        "LightBloomSizePx.y",
    ] {
        let key = state_param_id(machine, name);
        assert_eq!(
            drivers.get(key),
            Some(&"spring"),
            "wrong driver for State Param '{name}' ({key})"
        );
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

    assert_state_override_values(
        &completed,
        machine,
        "st_mrerxocx_8",
        &[
            ("FloatInput_38", "value"),
            ("FloatInput_39", "value"),
            ("FloatInput_40", "value"),
            ("FloatInput_41", "value"),
            ("Vector2Input_35", "x"),
            ("Vector2Input_35", "y"),
            ("Vector2Input_36", "x"),
            ("Vector2Input_36", "y"),
            ("FloatInput_37", "value"),
            ("Vector2Input_38", "x"),
            ("Vector2Input_38", "y"),
            ("FloatInput_42", "value"),
            ("FloatInput_43", "value"),
            ("FloatInput_44", "value"),
            ("FloatInput_45", "value"),
            ("FloatInput_46", "value"),
            ("FloatInput_47", "value"),
            ("FloatInput_48", "value"),
            ("FloatInput_49", "value"),
            ("FloatInput_50", "value"),
        ],
    );
}

#[test]
fn doubao_listening_transitions_animate_ui_opacity_and_snap_all_channels() {
    let _function_registry = support::function_registry_lock();
    let case_dir = support::render_case_dir("doubao-voice-interaction");
    let scene = load_case_scene(&case_dir).expect("doubao fixture should load");
    let machine = scene
        .state_machine
        .as_ref()
        .expect("doubao fixture should have a state machine");
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
            "dynamic_1784530828769_2",
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
            "dynamic_voice_dots",
            "dynamic_1785383865833_5",
        ],
        "Composite dynamic inputs must remain Caustic -> Glass -> UI -> Light -> Voice Dots -> PTT Prompt"
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
        assert_state_override_values(
            snapshot,
            machine,
            "st_listening",
            &[
                ("FloatInput_42", "value"),
                ("FloatInput_43", "value"),
                ("FloatInput_44", "value"),
                ("FloatInput_45", "value"),
                ("FloatInput_46", "value"),
                ("FloatInput_47", "value"),
                ("FloatInput_48", "value"),
                ("FloatInput_49", "value"),
                ("FloatInput_50", "value"),
            ],
        );
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
    for name in [
        "GradientBlurStartSigma",
        "GradientBlurEndSigma",
        "InputBarSizePx.x",
        "InputBarSizePx.y",
        "InputBarPositionPx.y",
        "LightBloomSizePx.x",
        "LightBloomSizePx.y",
    ] {
        let key = state_param_id(machine, name);
        assert_eq!(
            off_drivers.get(key),
            Some(&"spring"),
            "wrong Off -> Listening driver for State Param '{name}' ({key})"
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
    assert_state_override_values(
        &idle,
        machine,
        "st_mrerxocx_8",
        &[("FloatInput_42", "value")],
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
        .find(|channel| channel.key == state_param_id(machine, "InputBarUiOpacity"))
        .expect("InputBarUiOpacity should have a motion channel");
    assert_eq!(ui_opacity.driver, "spring");
    assert!(!ui_opacity.completed);

    let completed_idle_to_listening = settle(&mut idle_session);
    assert_eq!(completed_idle_to_listening.active_transition_id, None);
    assert_listening_values(&completed_idle_to_listening);
}

#[test]
fn doubao_shared_intelligent_light_derivation_advances_with_global_scene_time() {
    let _function_registry = support::function_registry_lock();
    let case_dir = support::render_case_dir("doubao-voice-interaction");
    let scene = load_case_scene(&case_dir).expect("doubao fixture should load");
    let machine = scene
        .state_machine
        .as_ref()
        .expect("doubao fixture should have a state machine");
    let derivation_by_node: BTreeMap<&str, &str> = machine
        .states
        .iter()
        .filter_map(|state| {
            state
                .derivation_id
                .as_deref()
                .map(|derivation_id| (state.id.as_str(), derivation_id))
        })
        .collect();
    let derivation_by_state: BTreeMap<&str, &str> = machine
        .derivation_bindings
        .iter()
        .map(|binding| {
            let derivation_id = derivation_by_node
                .get(binding.derivation_node_id.as_str())
                .unwrap_or_else(|| {
                    panic!(
                        "binding '{}' references Derivation node '{}' without a derivationId",
                        binding.id, binding.derivation_node_id
                    )
                });
            (binding.state_id.as_str(), *derivation_id)
        })
        .collect();
    let any_state_id = machine
        .states
        .iter()
        .find(|state| state.state_type == AnimationStateType::AnyState)
        .map(|state| state.id.as_str())
        .expect("doubao fixture should have an Any State");
    assert_eq!(
        derivation_by_state.keys().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([any_state_id, "st_thinking"]),
        "doubao should use one Any State fallback plus the Thinking override"
    );
    let shared_derivation = derivation_by_state[any_state_id];
    for state_id in [
        "st_mrerw3qg_6",
        "st_mrerxocx_8",
        "st_listening",
        "st_speaking",
        "st_push_to_talk",
        "st_push_to_talk_cancel",
    ] {
        assert_eq!(
            machine
                .derivation_bindings
                .iter()
                .find(|binding| binding.state_id == state_id)
                .and_then(|binding| derivation_by_node.get(binding.derivation_node_id.as_str()))
                .copied()
                .unwrap_or(shared_derivation),
            shared_derivation,
            "State '{state_id}' must inherit the shared Intelligent Light Derivation"
        );
    }
    assert_ne!(
        derivation_by_state["st_thinking"], shared_derivation,
        "Thinking must own an independent Derivation"
    );
    assert_eq!(
        machine
            .derivations
            .iter()
            .map(|derivation| derivation.id.as_str())
            .collect::<BTreeSet<_>>(),
        derivation_by_state
            .values()
            .copied()
            .collect::<BTreeSet<_>>(),
        "every declared Derivation must be bound, with no undeclared bindings"
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
        "Vector2ArrayInput_IntelligentLightPositions",
        "value",
    );
    let before = snapshot
        .active_overrides
        .get(&positions_key)
        .cloned()
        .expect("shared Derivation should produce Intelligent Light positions");
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
        "Derivation evaluation emitted diagnostics: {:?}",
        advanced.diagnostics
    );
}

#[test]
fn forced_doubao_state_keeps_derivation_running_without_routing() {
    let _function_registry = support::function_registry_lock();
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
                "Vector2ArrayInput_IntelligentLightPositions",
                "value",
            ),
        ),
        "forced State should continue evaluating its Derivation outputs"
    );
}

#[test]
fn leaving_a_derivation_state_restores_the_authored_gpu_uniform_once() {
    let _function_registry = support::function_registry_lock();
    let scene = support::load_render_case_scene("back-pin-pin");
    let machine = scene.state_machine.as_ref().expect("state machine");
    let binding = machine
        .derivation_bindings
        .first()
        .expect("fixture should bind a Derivation");
    let derivation_node = machine
        .states
        .iter()
        .find(|state| state.id == binding.derivation_node_id)
        .expect("Derivation node");
    let derivation_id = derivation_node
        .derivation_id
        .as_deref()
        .expect("Derivation definition ID");
    let output = machine
        .derivations
        .iter()
        .find(|derivation| derivation.id == derivation_id)
        .and_then(|derivation| {
            derivation
                .output_bindings
                .first()
                .map(|binding| binding.uniform.clone())
                .or_else(|| {
                    derivation
                        .passthrough_bindings
                        .first()
                        .map(|binding| binding.uniform.clone())
                })
        })
        .expect("Derivation GPU output");
    let unbound_state = machine
        .states
        .iter()
        .find(|state| {
            state.resolved_type() == AnimationStateType::AnimationState
                && !machine
                    .derivation_bindings
                    .iter()
                    .any(|candidate| candidate.state_id == state.id)
        })
        .expect("fixture should have an unbound State");
    let key = node_forge_render_server::state_machine::OverrideKey::new(
        &output.node_id,
        &output.param_id,
    );
    let authored = scene
        .nodes
        .iter()
        .find(|node| node.id == output.node_id)
        .and_then(|node| node.params.get(&output.param_id))
        .cloned()
        .expect("authored GPU uniform value");

    let mut session = AnimationSession::from_scene(&scene)
        .expect("state machine should compile")
        .expect("animation session");
    let derived = session
        .force_state(&binding.state_id)
        .expect("Derivation-bound State should be forceable");
    assert!(
        derived.active_overrides.contains_key(&key),
        "Derivation-bound State should write its GPU uniform"
    );

    let restore = session
        .force_state(&unbound_state.id)
        .expect("unbound State should be forceable");
    assert_eq!(
        restore.active_overrides.get(&key),
        Some(&authored),
        "leaving the Derivation binding should restore the shader-authored value"
    );
    assert!(
        session.step(0.0).active_overrides.get(&key).is_none(),
        "authored-value restoration should be a one-frame GPU write, not MotionEngine state"
    );
}

#[test]
fn doubao_push_to_talk_keeps_bottom_geometry_and_suppresses_white_layers() {
    let scene = support::load_render_case_scene("doubao-voice-interaction");
    let machine = scene
        .state_machine
        .as_ref()
        .expect("doubao fixture should have a state machine");
    let state = |state_id: &str| {
        machine
            .states
            .iter()
            .find(|state| state.id == state_id)
            .unwrap_or_else(|| panic!("missing state {state_id}"))
    };
    let value = |state_id: &str, param_name: &str| {
        let param = machine
            .state_params
            .iter()
            .find(|param| param.name == param_name)
            .unwrap_or_else(|| panic!("missing State Param {param_name}"));
        state(state_id)
            .state_param_overrides
            .get(&param.id)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_else(|| panic!("missing {param_name} override for {state_id}"))
    };

    assert_eq!(value("st_push_to_talk", "InputBarPositionPx.y"), 144.0);
    assert_eq!(
        value("st_push_to_talk_cancel", "InputBarPositionPx.y"),
        180.0
    );
    for state_id in ["st_push_to_talk", "st_push_to_talk_cancel"] {
        assert_eq!(value(state_id, "VoiceDotOpacity"), 0.0);
        assert_eq!(value(state_id, "VoiceDotProgress"), 0.0);
        assert_eq!(value(state_id, "VoiceDotResponse"), 0.0);
    }
    assert_eq!(
        value("st_push_to_talk", "IntelligentLightParticleOpacity"),
        0.35
    );
    for state_id in ["st_push_to_talk", "st_push_to_talk_cancel"] {
        assert_eq!(value(state_id, "LightClipBloomProgress"), 1.0);
    }
    for state_id in [
        "st_mrerw3qg_6",
        "st_mrerxocx_8",
        "st_listening",
        "st_thinking",
        "st_speaking",
    ] {
        assert_eq!(value(state_id, "LightClipBloomProgress"), 0.0);
    }

    let prompt_y = scene
        .groups
        .iter()
        .find(|group| group.id == "PttPrompt")
        .and_then(|group| {
            group
                .nodes
                .iter()
                .find(|node| node.id == "Rect2DGeometry_PttPrompt")
        })
        .and_then(|node| node.params.get("position"))
        .and_then(serde_json::Value::as_array)
        .and_then(|position| position.get(1))
        .and_then(serde_json::Value::as_f64)
        .expect("PTT prompt y position");
    assert_eq!(prompt_y, 300.0);
}

#[test]
fn doubao_blob_radius_uses_full_size_state_values_and_target_local_output() {
    let _function_registry = support::function_registry_lock();
    for (energy, expected_radius) in [(0.0, 26.88), (1.0, 29.4)] {
        let mut scene = support::load_render_case_scene("doubao-voice-interaction");
        scene
            .state_machine
            .as_mut()
            .expect("doubao fixture should have a state machine")
            .state_params
            .iter_mut()
            .find(|param| param.name == "TotalEnergy")
            .expect("TotalEnergy State Param")
            .default_value = serde_json::json!(energy);
        let mut session = AnimationSession::from_scene(&scene)
            .expect("doubao state machine should compile")
            .expect("doubao fixture should have a state machine");
        let frame = session
            .force_state("st_push_to_talk")
            .expect("PushToTalk should be forceable");
        let actual = frame
            .active_overrides
            .get(&node_forge_render_server::state_machine::OverrideKey::new(
                "FloatInput_IntelligentLightBlobRadiusLocalPx",
                "value",
            ))
            .and_then(serde_json::Value::as_f64)
            .expect("Derivation should produce target-local blob radius");
        assert!(
            (actual - expected_radius).abs() < 1e-5,
            "energy={energy}: expected {expected_radius}, got {actual}"
        );
    }
}

#[test]
fn doubao_idle_intelligent_light_positions_remain_target_local_for_ten_seconds() {
    let _function_registry = support::function_registry_lock();
    let (mut scene, _asset_store) = support::load_render_case("doubao-voice-interaction");
    let mut machine = scene
        .state_machine
        .take()
        .expect("doubao fixture should have a state machine");
    machine
        .states
        .iter_mut()
        .find(|state| state.id == "st_mrerxocx_8")
        .expect("doubao fixture should have an Idle State");
    machine.initial_state_id = Some("st_mrerxocx_8".into());
    scene.state_machine = Some(machine);
    let mut runtime = node_forge_render_server::state_machine::compile_from_scene(&scene)
        .expect("doubao state machine should compile")
        .expect("doubao fixture should have a state machine");
    let no_events = Vec::new();

    let positions_key = node_forge_render_server::state_machine::OverrideKey::new(
        "Vector2ArrayInput_IntelligentLightPositions",
        "value",
    );
    let mut first_positions = None;
    let mut last_positions = None;

    for index in 0..=600 {
        let dt = if index == 0 { 0.0 } else { 1.0 / 60.0 };
        let actual = runtime.tick(dt, &Default::default(), &no_events);
        assert_eq!(actual.current_state_id, "st_mrerxocx_8");
        assert!(
            actual.diagnostics.is_empty(),
            "frame {index} Derivation diagnostics: {:?}",
            actual.diagnostics
        );
        let positions = actual.overrides[&positions_key]
            .as_array()
            .unwrap_or_else(|| panic!("frame {index} has no packed positions"));
        assert_eq!(positions.len(), 11);
        for (position_index, position) in positions.iter().enumerate() {
            let components = position.as_array().expect("position must be vec2");
            assert_eq!(components.len(), 2);
            for (axis, component) in components.iter().enumerate() {
                let value = component.as_f64().expect("position must be numeric");
                assert!(
                    value.is_finite(),
                    "frame {index} position {position_index} axis {axis} must be finite"
                );
            }
        }
        if index == 0 {
            first_positions = Some(positions.clone());
        }
        last_positions = Some(positions.clone());
    }
    assert_ne!(
        first_positions, last_positions,
        "Idle local positions must animate"
    );
}

#[test]
fn animation_value_traces_match_goldens() {
    let _function_registry = support::function_registry_lock();
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
