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
        if ef.transition_blend != af.transition_blend {
            return Some(format!(
                "case {case_name} frame {i}: transition_blend mismatch expected={:?} actual={:?}",
                ef.transition_blend, af.transition_blend
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
    let mut failures: Vec<String> = Vec::new();

    for case_dir in discover_case_dirs() {
        let name = case_name(&case_dir);
        let golden_path = case_dir.join("animation_values.json");
        if !golden_path.exists() {
            continue;
        }

        let scene = match load_case_scene(&case_dir) {
            Some(s) => s,
            None => {
                failures.push(format!("case {name}: no scene.json or scene.nforge"));
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
