//! DSL type definitions for the state-machine animation system.
//!
//! These types map 1:1 to the `SceneDSL.stateMachine` JSON contract defined
//! by the editor.  They are intentionally self-contained so the state-machine
//! subsystem does not depend on the shader/render pipeline.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Top-level
// ---------------------------------------------------------------------------

/// Root of the state-machine definition embedded in `SceneDSL.stateMachine`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StateMachine {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub states: Vec<AnimationState>,
    #[serde(default)]
    pub transitions: Vec<AnimationTransition>,
    #[serde(default)]
    pub mutations: Vec<MutationDefinition>,
    #[serde(default, rename = "initialStateId")]
    pub initial_state_id: Option<String>,
    /// Editor-only viewport metadata — ignored at runtime.
    #[serde(default)]
    pub viewport: Option<Viewport>,
}

// ---------------------------------------------------------------------------
// States
// ---------------------------------------------------------------------------

/// Discriminant for built-in vs user-defined state types.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum AnimationStateType {
    EntryState,
    AnyState,
    ExitState,
    AnimationState,
    MutationNode,
}

/// A single state in the state graph.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AnimationState {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub position: Option<Position>,
    #[serde(default, rename = "parameterOverrides")]
    pub parameter_overrides: HashMap<String, serde_json::Value>,
    /// Discriminant.  Legacy scenes may omit this — see normalization.
    #[serde(rename = "type")]
    pub state_type: Option<AnimationStateType>,
    /// Only present when `state_type == MutationNode`.
    #[serde(default, rename = "mutationId")]
    pub mutation_id: Option<String>,
}

impl AnimationState {
    /// Resolved state type, applying legacy inference when `type` is missing.
    pub fn resolved_type(&self) -> AnimationStateType {
        if let Some(t) = self.state_type {
            return t;
        }
        // Legacy inference by name.
        let name_lower = self.name.to_ascii_lowercase();
        if name_lower == "entry" {
            return AnimationStateType::EntryState;
        }
        if name_lower == "any" {
            return AnimationStateType::AnyState;
        }
        if name_lower == "exit" {
            return AnimationStateType::ExitState;
        }
        if self.mutation_id.is_some() {
            return AnimationStateType::MutationNode;
        }
        AnimationStateType::AnimationState
    }
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AnimationTransition {
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub condition: Option<TransitionCondition>,
    /// Transition duration in seconds.  Defaults to 0.3.
    #[serde(default = "default_duration")]
    pub duration: f64,
    /// Easing curve.  Defaults to `EaseInOut`.
    #[serde(default)]
    pub easing: EasingKind,
}

fn default_duration() -> f64 {
    0.3
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EasingKind {
    Linear,
    EaseIn,
    EaseOut,
    #[default]
    EaseInOut,
}

// ---------------------------------------------------------------------------
// Transition conditions
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TransitionCondition {
    Trigger {
        #[serde(rename = "paramId")]
        param_id: String,
    },
    Bool {
        #[serde(rename = "paramId")]
        param_id: String,
        /// Defaults to `true` if missing.
        #[serde(default)]
        value: Option<bool>,
    },
    Threshold {
        #[serde(rename = "paramId")]
        param_id: String,
        value: f64,
    },
    Event {
        #[serde(rename = "eventName")]
        event_name: String,
    },
    Compound {
        op: CompoundOp,
        conditions: Vec<TransitionCondition>,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CompoundOp {
    And,
    Or,
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// A reusable mutation subgraph.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MutationDefinition {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub inputs: Vec<MutationPort>,
    #[serde(default)]
    pub outputs: Vec<MutationPort>,
    #[serde(default)]
    pub nodes: Vec<MutationInnerNode>,
    #[serde(default)]
    pub connections: Vec<MutationConnection>,
    #[serde(default, rename = "inputBindings")]
    pub input_bindings: Vec<MutationInputBinding>,
    #[serde(default, rename = "outputBindings")]
    pub output_bindings: Vec<MutationOutputBinding>,
    /// Editor-only viewport metadata — ignored at runtime.
    #[serde(default)]
    pub viewport: Option<Viewport>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MutationPort {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "type", default)]
    pub port_type: Option<String>,
}

/// Supported inner-node types for mutation subgraphs (v1).
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum MutationInnerNodeType {
    SmPassThrough,
    SmMathOp,
    SmLerp,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MutationInnerNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: MutationInnerNodeType,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub inputs: Vec<MutationPort>,
    #[serde(default)]
    pub outputs: Vec<MutationPort>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MutationConnection {
    pub id: String,
    pub from: MutationEndpoint,
    pub to: MutationEndpoint,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MutationEndpoint {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "portId")]
    pub port_id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MutationInputBinding {
    /// Port on the mutation boundary.
    #[serde(rename = "portId")]
    pub port_id: String,
    /// Where the value is fed into the inner graph.
    pub to: MutationEndpoint,
    /// Optional external reference key (e.g. `"nodeId:paramName"`).
    #[serde(default, rename = "sourceRef")]
    pub source_ref: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MutationOutputBinding {
    /// Port on the mutation boundary.
    #[serde(rename = "portId")]
    pub port_id: String,
    /// Where the value comes from in the inner graph.
    pub from: MutationEndpoint,
    /// Optional external target key (e.g. `"nodeId:paramName"`).
    #[serde(default, rename = "targetRef")]
    pub target_ref: Option<String>,
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

// ---------------------------------------------------------------------------
// Override key
// ---------------------------------------------------------------------------

/// A typed key for runtime parameter overrides produced by the state machine.
///
/// Encodes `nodeId:paramName` — the same format used by the editor's
/// mutation interface ports.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OverrideKey {
    pub node_id: String,
    pub param_name: String,
}

impl OverrideKey {
    pub fn new(node_id: impl Into<String>, param_name: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            param_name: param_name.into(),
        }
    }

    /// Parse from the editor convention `"nodeId:paramName"`.
    pub fn parse(s: &str) -> Option<Self> {
        let (node_id, param_name) = s.split_once(':')?;
        if node_id.is_empty() || param_name.is_empty() {
            return None;
        }
        Some(Self {
            node_id: node_id.to_string(),
            param_name: param_name.to_string(),
        })
    }
}
