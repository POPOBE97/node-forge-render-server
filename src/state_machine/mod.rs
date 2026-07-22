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
    values
}

/// Apply state-machine overrides to a scene's node params (in-place).
///
/// This patches `node.params[paramName]` for each override key that
/// matches a node in the scene.  Used to inject runtime animation
/// values before uniform packing.
///
/// Override keys reference node IDs from the unexpanded scene (e.g.
/// `FloatInput_53`).  After group expansion those nodes live under
/// namespaced IDs like `GroupInstance_59/FloatInput_53`.  We first
/// try an exact match and fall back to a suffix match so that
/// group-internal nodes are correctly targeted.
pub fn apply_overrides(scene: &mut SceneDSL, overrides: &HashMap<OverrideKey, serde_json::Value>) {
    if overrides.is_empty() {
        return;
    }

    for (key, value) in overrides {
        let mut applied = false;
        for node in &mut scene.nodes {
            if key.node_id == node.id || node.id.ends_with(&format!("/{}", key.node_id)) {
                node.params.insert(key.param_name.clone(), value.clone());
                applied = true;
            }
        }

        if applied {
            continue;
        }

        // Prepared scenes no longer contain GroupInstance nodes. Resolve a
        // state-local Mutation write against the retained canonical group
        // binding and patch the namespaced clone directly.
        for group in &scene.groups {
            for binding in &group.input_bindings {
                if binding.group_port_id != key.param_name {
                    continue;
                }
                let expanded_id = format!("{}/{}", key.node_id, binding.to.node_id);
                if let Some(node) = scene.nodes.iter_mut().find(|node| node.id == expanded_id) {
                    node.params
                        .insert(binding.to.port_id.clone(), value.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{Endpoint, GroupDSL, GroupInputBinding, Metadata, Node};

    #[test]
    fn group_instance_override_reaches_expanded_packed_input() {
        let mut scene = SceneDSL {
            version: "2.0".to_string(),
            metadata: Metadata {
                name: "group mutation".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![Node {
                id: "instance/ilight".to_string(),
                node_type: "IntelligentLight".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                outputs: vec![],
                input_bindings: vec![],
                wgsl_override: None,
            }],
            connections: vec![],
            outputs: None,
            groups: vec![GroupDSL {
                id: "light".to_string(),
                name: None,
                inputs: vec![],
                outputs: vec![],
                nodes: vec![],
                connections: vec![],
                input_bindings: vec![GroupInputBinding {
                    group_port_id: "positions".to_string(),
                    to: Endpoint {
                        node_id: "ilight".to_string(),
                        port_id: "positions".to_string(),
                    },
                }],
                output_bindings: vec![],
            }],
            assets: HashMap::new(),
            state_machine: None,
            debug_artifacts: None,
        };
        let packed =
            serde_json::Value::Array((0..11).map(|_| serde_json::json!([1.0, 2.0])).collect());
        apply_overrides(
            &mut scene,
            &HashMap::from([(OverrideKey::new("instance", "positions"), packed.clone())]),
        );
        assert_eq!(scene.nodes[0].params.get("positions"), Some(&packed));
    }
}
