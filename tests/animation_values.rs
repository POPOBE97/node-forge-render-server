use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use node_forge_render_server::animation::AnimationSession;
use node_forge_render_server::state_machine::{
    AnimationTraceFrame, AnimationTraceLog, EventSchedule, ScheduledEvent, TickSchedule,
    build_initial_values, canonicalize_json_value, round_f64, tracked_override_keys,
};
use node_forge_render_server::{asset_store, dsl};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn cases_root() -> PathBuf {
    manifest_dir().join("tests").join("cases")
}

fn discover_case_dirs() -> Vec<PathBuf> {
    let root = cases_root();
    let mut dirs = Vec::new();

    let entries = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("failed to read cases dir {}: {e}", root.display()));

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("SKIP_RENDER_CASE").exists() {
            continue;
        }
        dirs.push(path);
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
    if nforge.exists() {
        let (scene, _store) = asset_store::load_from_nforge(&nforge)
            .unwrap_or_else(|e| panic!("failed to load {}: {e:#}", nforge.display()));
        return Some(scene);
    }

    let scene_json = case_dir.join("scene.json");
    if scene_json.exists() {
        let scene = dsl::load_scene_from_path(&scene_json)
            .unwrap_or_else(|e| panic!("failed to load {}: {e:#}", scene_json.display()));
        return Some(scene);
    }

    None
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
) -> String {
    if expected.schema_version != actual.schema_version {
        return format!(
            "case {case_name}: schema_version mismatch expected={} actual={}",
            expected.schema_version, actual.schema_version
        );
    }
    if expected.start_secs != actual.start_secs {
        return format!(
            "case {case_name}: start_secs mismatch expected={} actual={}",
            expected.start_secs, actual.start_secs
        );
    }
    if expected.end_secs != actual.end_secs {
        return format!(
            "case {case_name}: end_secs mismatch expected={} actual={}",
            expected.end_secs, actual.end_secs
        );
    }
    if expected.fps != actual.fps {
        return format!(
            "case {case_name}: fps mismatch expected={} actual={}",
            expected.fps, actual.fps
        );
    }
    if expected.include_end != actual.include_end {
        return format!(
            "case {case_name}: include_end mismatch expected={} actual={}",
            expected.include_end, actual.include_end
        );
    }
    if expected.frame_count != actual.frame_count {
        return format!(
            "case {case_name}: frame_count mismatch expected={} actual={}",
            expected.frame_count, actual.frame_count
        );
    }
    if expected.tracked_keys != actual.tracked_keys {
        return format!(
            "case {case_name}: tracked_keys mismatch expected={:?} actual={:?}",
            expected.tracked_keys, actual.tracked_keys
        );
    }
    if expected.frames.len() != actual.frames.len() {
        return format!(
            "case {case_name}: frames length mismatch expected={} actual={}",
            expected.frames.len(),
            actual.frames.len()
        );
    }

    for idx in 0..expected.frames.len() {
        let e = &expected.frames[idx];
        let a = &actual.frames[idx];

        if e.frame_index != a.frame_index {
            return format!(
                "case {case_name}: frame {idx} frame_index mismatch expected={} actual={}",
                e.frame_index, a.frame_index
            );
        }
        if e.time_secs != a.time_secs {
            return format!(
                "case {case_name}: frame {idx} time_secs mismatch expected={} actual={}",
                e.time_secs, a.time_secs
            );
        }
        if e.dt_secs != a.dt_secs {
            return format!(
                "case {case_name}: frame {idx} dt_secs mismatch expected={} actual={}",
                e.dt_secs, a.dt_secs
            );
        }
        if e.current_state_id != a.current_state_id {
            return format!(
                "case {case_name}: frame {idx} current_state_id mismatch expected={} actual={}",
                e.current_state_id, a.current_state_id
            );
        }
        if e.state_local_time_secs != a.state_local_time_secs {
            return format!(
                "case {case_name}: frame {idx} state_local_time_secs mismatch expected={} actual={}",
                e.state_local_time_secs, a.state_local_time_secs
            );
        }
        if e.scene_time_secs != a.scene_time_secs {
            return format!(
                "case {case_name}: frame {idx} scene_time_secs mismatch expected={} actual={}",
                e.scene_time_secs, a.scene_time_secs
            );
        }
        if e.active_transition_id != a.active_transition_id {
            return format!(
                "case {case_name}: frame {idx} active_transition_id mismatch expected={:?} actual={:?}",
                e.active_transition_id, a.active_transition_id
            );
        }
        if e.transition_blend != a.transition_blend {
            return format!(
                "case {case_name}: frame {idx} transition_blend mismatch expected={:?} actual={:?}",
                e.transition_blend, a.transition_blend
            );
        }
        if e.finished != a.finished {
            return format!(
                "case {case_name}: frame {idx} finished mismatch expected={} actual={}",
                e.finished, a.finished
            );
        }
        if e.diagnostics != a.diagnostics {
            return format!(
                "case {case_name}: frame {idx} diagnostics mismatch expected={:?} actual={:?}",
                e.diagnostics, a.diagnostics
            );
        }

        let keys: BTreeSet<&String> = e.values.keys().chain(a.values.keys()).collect();
        for key in keys {
            let ev = e.values.get(key);
            let av = a.values.get(key);
            if ev != av {
                let evs = ev
                    .map(|v| {
                        serde_json::to_string(v).unwrap_or_else(|_| "<unserializable>".to_string())
                    })
                    .unwrap_or_else(|| "<missing>".to_string());
                let avs = av
                    .map(|v| {
                        serde_json::to_string(v).unwrap_or_else(|_| "<unserializable>".to_string())
                    })
                    .unwrap_or_else(|| "<missing>".to_string());
                return format!(
                    "case {case_name}: frame {idx} value key '{}' mismatch expected={} actual={}",
                    key, evs, avs
                );
            }
        }
    }

    format!("case {case_name}: trace mismatch but no focused diff found (compare full JSON)")
}

/// Generate a trace by driving `AnimationSession` — the same code path the
/// app uses at runtime.  This ensures the test exercises ValuePool, TaskPool,
/// Runloop, and the full session lifecycle rather than calling
/// `runtime.tick()` directly.
fn generate_trace_via_session(
    scene: &dsl::SceneDSL,
    schedule: &TickSchedule,
    event_schedule: &[ScheduledEvent],
) -> AnimationTraceLog {
    let mut session = AnimationSession::from_scene(scene)
        .expect("from_scene failed")
        .expect("scene has no state machine");

    let sm = session.runtime().definition();
    let tracked_key_set = tracked_override_keys(sm);
    let tracked_keys: Vec<String> = tracked_key_set.iter().cloned().collect();
    let initial_values = build_initial_values(scene, &tracked_keys);

    // Current values accumulator — starts with scene baselines, updated by
    // each step's active_overrides (mirrors how the app applies overrides).
    let mut current_values = initial_values;

    let samples = schedule.samples();
    let mut frames: Vec<AnimationTraceFrame> = Vec::with_capacity(samples.len());

    for sample in &samples {
        // Fire any scheduled events for this frame.
        for ev in event_schedule
            .iter()
            .filter(|e| e.frame_index == sample.frame_index)
        {
            session.fire_event(&ev.event_name);
        }

        // Drive the session exactly as the app does: step(dt).
        let step = session.step(sample.dt_secs);

        // Merge active overrides into current values (same as app's
        // apply_overrides path).
        for (key, value) in &step.active_overrides {
            let trace_key = format!("{}:{}", key.node_id, key.param_name);
            current_values.insert(trace_key, canonicalize_json_value(value));
        }

        // Snapshot tracked keys for this frame.
        let mut frame_values: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for key in &tracked_keys {
            let value = current_values
                .get(key)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            frame_values.insert(key.clone(), value);
        }

        frames.push(AnimationTraceFrame {
            frame_index: sample.frame_index,
            time_secs: round_f64(sample.time_secs),
            dt_secs: round_f64(sample.dt_secs),
            current_state_id: step.current_state_id.clone(),
            state_local_time_secs: round_f64(step.state_local_time_secs),
            scene_time_secs: round_f64(step.scene_time_secs),
            active_transition_id: step.active_transition_id.clone(),
            transition_blend: step.transition_blend.map(round_f64),
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

#[test]
fn animation_value_traces_match_goldens() {
    let update_goldens = std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v != "0");
    let schedule = TickSchedule::new(0.0, 10.0, 60, true).expect("valid fixed animation schedule");

    let mut checked_cases = 0usize;
    for case_dir in discover_case_dirs() {
        let name = case_name(&case_dir);
        let Some(scene) = load_case_scene(&case_dir) else {
            continue;
        };
        if scene.state_machine.is_none() {
            continue;
        }

        checked_cases += 1;

        let actual = {
            let events_path = case_dir.join("events.json");
            let event_schedule: Vec<ScheduledEvent> = if events_path.exists() {
                let text = std::fs::read_to_string(&events_path).unwrap_or_else(|e| {
                    panic!("case {name}: failed to read {}: {e}", events_path.display())
                });
                let es: EventSchedule = serde_json::from_str(&text).unwrap_or_else(|e| {
                    panic!(
                        "case {name}: failed to parse {}: {e}",
                        events_path.display()
                    )
                });
                es.events
            } else {
                Vec::new()
            };
            generate_trace_via_session(&scene, &schedule, &event_schedule)
        };

        let baseline_path = case_dir.join("animation_values.json");
        if update_goldens {
            write_trace(&baseline_path, &actual);
            continue;
        }

        if !baseline_path.exists() {
            panic!(
                "case {name}: missing animation trace baseline: {}\nrun UPDATE_GOLDENS=1 cargo test --test animation_values",
                baseline_path.display()
            );
        }

        let expected_text = std::fs::read_to_string(&baseline_path).unwrap_or_else(|e| {
            panic!(
                "case {name}: failed to read {}: {e}",
                baseline_path.display()
            )
        });
        let expected: AnimationTraceLog =
            serde_json::from_str(&expected_text).unwrap_or_else(|e| {
                panic!(
                    "case {name}: failed to parse {}: {e}",
                    baseline_path.display()
                )
            });

        if expected != actual {
            panic!(
                "{}\nbaseline={}",
                first_trace_mismatch(&name, &expected, &actual),
                baseline_path.display()
            );
        }
    }

    assert!(
        checked_cases > 0,
        "animation_values test found no state-machine cases under {}",
        cases_root().display()
    );
}
