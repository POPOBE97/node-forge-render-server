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
pub mod mutation;
pub mod runtime;
pub mod timeline;
pub mod trace;
pub mod types;
pub mod validation;

// Re-export key types for ergonomic use.
pub use runtime::{ExternalParams, FiredEvents, StateMachineRuntime, TickResult};
pub use timeline::{TickSample, TickSchedule, evenly_spaced_samples};
pub use trace::{AnimationTraceFrame, AnimationTraceLog, generate_trace_for_scene};
pub use types::{OverrideKey, StateMachine};
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

    Ok(Some(StateMachineRuntime::new(sm)))
}

/// Apply state-machine overrides to a scene's node params (in-place).
///
/// This patches `node.params[paramName]` for each override key that
/// matches a node in the scene.  Used to inject runtime animation
/// values before uniform packing.
pub fn apply_overrides(scene: &mut SceneDSL, overrides: &HashMap<OverrideKey, serde_json::Value>) {
    if overrides.is_empty() {
        return;
    }

    for node in &mut scene.nodes {
        for (key, value) in overrides {
            if key.node_id == node.id {
                node.params.insert(key.param_name.clone(), value.clone());
            }
        }
    }
}
