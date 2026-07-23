//! State-machine animation system.
//!
//! This module is fully decoupled from the shader/render pipeline.
//! It parses, validates, and executes `SceneDSL.stateMachine` definitions,
//! producing runtime parameter overrides that the render loop can apply.
//!
//! # Module layout
//!
//! - [`types`]      — DSL types (serde structs matching the JSON contract)
//! - [`validation`] — Structural/semantic validation (fail-fast)
//! - [`mutation`]   — Mutation inner-graph compiler/evaluator
//! - [`runtime`]    — Tick-driven state-machine runtime
//! - [`easing`]     — Easing functions
//! - [`timeline`]   — Deterministic tick schedule helpers
//! - [`trace`]      — Animation value-trace generation
//!
//! # Quick start
//!
//! ```ignore
//! use crate::state_machine::{self, StateMachineRuntime};
//!
//! // Parse — `scene.state_machine` is deserialized via serde.
//! let sm: state_machine::StateMachine = ...;
//!
//! // Validate.
//! state_machine::validate(&sm)?;
//!
//! // Construct runtime.
//! let mut rt = StateMachineRuntime::new(sm);
//!
//! // Each frame:
//! let result = rt.tick(dt, &params, &events);
//! // Apply result.overrides to scene node params.
//! ```

pub mod easing;
pub mod motion;
pub mod mutation;
pub mod mutation_function;
pub mod runtime;
pub mod timeline;
pub mod trace;
pub mod types;
pub mod validation;

// Re-export key types for ergonomic use.
pub use motion::MotionChannelDebug;
pub use runtime::{ExternalParams, FiredEvent, FiredEvents, StateMachineRuntime, TickResult};
pub use timeline::{TickSample, TickSchedule, evenly_spaced_samples};
pub use trace::{
    AnimationTraceFrame, AnimationTraceLog, EventSchedule, ScheduledEvent, build_initial_values,
    canonicalize_json_value, generate_trace_for_scene, generate_trace_for_scene_with_events,
    round_f64, tracked_override_keys,
};
pub use types::{EventModifiers, MousePosition, OverrideKey, RuntimeInputSnapshot, StateMachine};
pub use validation::validate;

use std::collections::HashMap;

use anyhow::Result;

use crate::dsl::SceneDSL;

/// Build a `StateMachineRuntime` from a `SceneDSL`, if the scene contains
/// a `stateMachine` definition.
///
/// Returns `None` when the scene has no state machine, or an error if
/// validation fails.
pub fn compile_from_scene(scene: &SceneDSL) -> Result<Option<StateMachineRuntime>> {
    let sm = match scene.state_machine.as_ref() {
        Some(sm) => sm.clone(),
        None => return Ok(None),
    };

    validate(&sm)?;
    mutation_function::prepare_state_machine(&sm)?;

    Ok(Some(StateMachineRuntime::with_initial_values(
        sm,
        collect_scene_current_values(scene),
    )))
}

/// Collect the scene's complete current uniform/parameter snapshot. The
/// animation engine owns this map so State, Motion, Mutation, external deltas,
/// and renderer output all share one source of truth.
pub(crate) fn collect_scene_current_values(
    scene: &SceneDSL,
) -> HashMap<OverrideKey, serde_json::Value> {
    let mut values = HashMap::new();
    for node in &scene.nodes {
        for (param_name, value) in &node.params {
            values.insert(OverrideKey::new(&node.id, param_name), value.clone());
        }
    }
    for group in &scene.groups {
        for node in &group.nodes {
            for (param_name, value) in &node.params {
                values
                    .entry(OverrideKey::new(&node.id, param_name))
                    .or_insert_with(|| value.clone());
            }
        }
    }
    // PackedInput is an explicit uniform whose base value is assembled from
    // its ordered child connections. Seed that value into the immutable
    // motion snapshot so Mutation can read it without owning hidden state.
    let nodes_by_id = scene
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    for node in &scene.nodes {
        if node.node_type != "PackedInput" {
            continue;
        }
        let key = OverrideKey::new(&node.id, "value");
        if values.contains_key(&key) {
            continue;
        }
        let packed = node
            .inputs
            .iter()
            .map(|input| {
                let source = scene
                    .connections
                    .iter()
                    .find(|connection| {
                        connection.to.node_id == node.id && connection.to.port_id == input.id
                    })
                    .and_then(|connection| {
                        nodes_by_id.get(connection.from.node_id.as_str()).copied()
                    });
                let Some(source) = source else {
                    return serde_json::Value::Null;
                };
                match source.node_type.as_str() {
                    "Vector2Input" => serde_json::json!([
                        source.params.get("x").cloned().unwrap_or_default(),
                        source.params.get("y").cloned().unwrap_or_default()
                    ]),
                    "Vector3Input" => serde_json::json!([
                        source.params.get("x").cloned().unwrap_or_default(),
                        source.params.get("y").cloned().unwrap_or_default(),
                        source.params.get("z").cloned().unwrap_or_default()
                    ]),
                    "Vector4Input" => serde_json::json!([
                        source.params.get("x").cloned().unwrap_or_default(),
                        source.params.get("y").cloned().unwrap_or_default(),
                        source.params.get("z").cloned().unwrap_or_default(),
                        source.params.get("w").cloned().unwrap_or_default()
                    ]),
                    _ => source
                        .params
                        .get("value")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                }
            })
            .collect();
        values.insert(key, serde_json::Value::Array(packed));
    }
    values
}

/// Apply state-machine overrides to a scene's node params (in-place).
///
/// This patches `node.params[paramName]` for each override key that
/// matches a node in the scene.  Used to inject runtime animation
/// values before uniform packing.
///
/// Override keys are declaration-side identities and therefore match only
/// exact node IDs. Group expansion and consumer topology must not create an
/// implicit animation or Mutation target.
pub fn apply_overrides(scene: &mut SceneDSL, overrides: &HashMap<OverrideKey, serde_json::Value>) {
    if overrides.is_empty() {
        return;
    }

    for (key, value) in overrides {
        for node in &mut scene.nodes {
            if key.node_id == node.id {
                node.params.insert(key.param_name.clone(), value.clone());
            }
        }
    }
}
