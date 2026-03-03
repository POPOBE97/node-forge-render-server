use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::dsl::SceneDSL;

use super::runtime::TickResult;
use super::timeline::TickSchedule;
use super::types::{OverrideKey, StateMachine};

const TRACE_SCHEMA_VERSION: u32 = 1;
const ROUND_PRECISION: f64 = 1_000_000.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnimationTraceLog {
    pub schema_version: u32,
    pub start_secs: f64,
    pub end_secs: f64,
    pub fps: u32,
    pub include_end: bool,
    pub frame_count: usize,
    pub tracked_keys: Vec<String>,
    pub frames: Vec<AnimationTraceFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnimationTraceFrame {
    pub frame_index: usize,
    pub time_secs: f64,
    pub dt_secs: f64,
    pub current_state_id: String,
    pub state_local_time_secs: f64,
    pub scene_time_secs: f64,
    pub active_transition_id: Option<String>,
    pub transition_blend: Option<f64>,
    pub finished: bool,
    pub diagnostics: Vec<String>,
    pub values: BTreeMap<String, serde_json::Value>,
}

pub fn generate_trace_for_scene(
    scene: &SceneDSL,
    schedule: &TickSchedule,
) -> Result<AnimationTraceLog> {
    let mut runtime =
        super::compile_from_scene(scene)?.ok_or_else(|| anyhow!("scene has no stateMachine"))?;

    let tracked_key_set = tracked_override_keys(runtime.definition());
    let tracked_keys: Vec<String> = tracked_key_set.iter().cloned().collect();

    let mut current_values = build_initial_values(scene, &tracked_keys);
    let mut frames: Vec<AnimationTraceFrame> = Vec::with_capacity(schedule.frame_count());

    for sample in schedule.samples() {
        let result = runtime.tick(sample.dt_secs, &HashMap::new(), &Vec::new());
        apply_overrides_to_values(&mut current_values, &result);

        let mut frame_values: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for key in &tracked_keys {
            let value = current_values
                .get(key)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            frame_values.insert(key.clone(), canonicalize_json_value(&value));
        }

        frames.push(AnimationTraceFrame {
            frame_index: sample.frame_index,
            time_secs: round_f64(sample.time_secs),
            dt_secs: round_f64(sample.dt_secs),
            current_state_id: result.current_state_id.clone(),
            state_local_time_secs: round_f64(result.state_local_time_secs),
            scene_time_secs: round_f64(result.scene_time_secs),
            active_transition_id: result.active_transition_id.clone(),
            transition_blend: result.transition_blend.map(round_f64),
            finished: result.finished,
            diagnostics: result.diagnostics.clone(),
            values: frame_values,
        });
    }

    Ok(AnimationTraceLog {
        schema_version: TRACE_SCHEMA_VERSION,
        start_secs: round_f64(schedule.start_secs),
        end_secs: round_f64(schedule.end_secs),
        fps: schedule.fps,
        include_end: schedule.include_end,
        frame_count: frames.len(),
        tracked_keys,
        frames,
    })
}

fn tracked_override_keys(sm: &StateMachine) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();

    for state in &sm.states {
        for key in state.parameter_overrides.keys() {
            if OverrideKey::parse(key).is_some() {
                keys.insert(key.clone());
            }
        }
    }

    for mutation in &sm.mutations {
        for binding in &mutation.output_bindings {
            if let Some(target_ref) = binding.target_ref.as_deref()
                && OverrideKey::parse(target_ref).is_some()
            {
                keys.insert(target_ref.to_string());
            }
        }
    }

    keys
}

fn build_initial_values(
    scene: &SceneDSL,
    tracked_keys: &[String],
) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();

    for key in tracked_keys {
        let value = match OverrideKey::parse(key) {
            Some(parsed) => scene
                .nodes
                .iter()
                .find(|n| n.id == parsed.node_id)
                .and_then(|n| n.params.get(parsed.param_name.as_str()))
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            None => serde_json::Value::Null,
        };
        out.insert(key.clone(), canonicalize_json_value(&value));
    }

    out
}

fn apply_overrides_to_values(
    values: &mut BTreeMap<String, serde_json::Value>,
    result: &TickResult,
) {
    for (key, value) in &result.overrides {
        let trace_key = format!("{}:{}", key.node_id, key.param_name);
        values.insert(trace_key, canonicalize_json_value(value));
    }
}

fn round_f64(v: f64) -> f64 {
    let rounded = (v * ROUND_PRECISION).round() / ROUND_PRECISION;
    if rounded == -0.0 { 0.0 } else { rounded }
}

fn round_json_number(n: &serde_json::Number) -> serde_json::Number {
    if n.is_i64() || n.is_u64() {
        return n.clone();
    }

    match n.as_f64() {
        Some(v) => serde_json::Number::from_f64(round_f64(v)).unwrap_or_else(|| n.clone()),
        None => n.clone(),
    }
}

fn canonicalize_json_value(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Null => serde_json::Value::Null,
        serde_json::Value::Bool(b) => serde_json::Value::Bool(*b),
        serde_json::Value::Number(n) => serde_json::Value::Number(round_json_number(n)),
        serde_json::Value::String(s) => serde_json::Value::String(s.clone()),
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(canonicalize_json_value).collect())
        }
        serde_json::Value::Object(map) => {
            let mut sorted: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            let mut out = serde_json::Map::with_capacity(sorted.len());
            for (k, value) in sorted {
                out.insert(k.clone(), canonicalize_json_value(value));
            }
            serde_json::Value::Object(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl;
    use std::path::PathBuf;

    fn back_pin_pin_scene() -> dsl::SceneDSL {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("cases")
            .join("back-pin-pin")
            .join("scene.json");
        dsl::load_scene_from_path(path).expect("load back-pin-pin scene")
    }

    #[test]
    fn trace_keys_are_sorted_and_dense() {
        let scene = back_pin_pin_scene();
        let schedule = TickSchedule::new(0.0, 1.0, 10, true).unwrap();
        let trace = generate_trace_for_scene(&scene, &schedule).unwrap();

        let mut sorted = trace.tracked_keys.clone();
        sorted.sort();
        assert_eq!(trace.tracked_keys, sorted);

        for frame in &trace.frames {
            assert_eq!(frame.values.len(), trace.tracked_keys.len());
            for key in &trace.tracked_keys {
                assert!(frame.values.contains_key(key));
            }
        }
    }

    #[test]
    fn trace_includes_runtime_metadata() {
        let scene = back_pin_pin_scene();
        let schedule = TickSchedule::new(0.0, 0.1, 10, true).unwrap();
        let trace = generate_trace_for_scene(&scene, &schedule).unwrap();

        let first = trace.frames.first().unwrap();
        assert_eq!(first.frame_index, 0);
        assert!((first.time_secs - 0.0).abs() < 1e-12);
        assert!((first.dt_secs - 0.0).abs() < 1e-12);

        let last = trace.frames.last().unwrap();
        assert!((last.time_secs - 0.1).abs() < 1e-6);
        assert!(last.scene_time_secs >= first.scene_time_secs);
    }
}
