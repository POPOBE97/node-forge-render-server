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
    #[serde(default, rename = "motionGraphs")]
    pub motion_graphs: Vec<TransitionMotionGraph>,
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
    #[serde(rename = "type")]
    pub state_type: AnimationStateType,
    /// Optional state-local post-motion Mutation graph.
    #[serde(default, rename = "mutationId")]
    pub mutation_id: Option<String>,
}

impl AnimationState {
    pub fn resolved_type(&self) -> AnimationStateType {
        self.state_type
    }
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AnimationTransition {
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(rename = "motionGraphId")]
    pub motion_graph_id: String,
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

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TimelinePreset {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    SineIn,
    SineOut,
    SineInOut,
    CosineIn,
    CosineOut,
    CosineInOut,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "camelCase")]
pub enum RepeatMode {
    #[default]
    PingPong,
    Restart,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TimelineBlending {
    #[serde(rename = "type")]
    pub blend_type: TimelineBlendingType,
    #[serde(default = "default_blend_duration")]
    pub duration: f64,
    #[serde(default)]
    pub easing: EasingKind,
}

fn default_blend_duration() -> f64 {
    0.1
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TimelineBlendingType {
    Tween,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TimelineMotionNode {
    pub id: String,
    #[serde(default)]
    pub position: Position,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default = "default_timeline_duration")]
    pub duration: f64,
    #[serde(default)]
    pub delay: f64,
    #[serde(default)]
    pub blending: Option<TimelineBlending>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TransitionMotionNode {
    #[serde(rename = "RepeatTimeline")]
    RepeatTimeline {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        from: f64,
        #[serde(default = "default_repeat_to")]
        to: f64,
        #[serde(default = "default_repeat_duration")]
        duration: f64,
        #[serde(default)]
        easing: TimelinePreset,
        #[serde(default)]
        mode: RepeatMode,
    },
    #[serde(rename = "SpringFollow")]
    SpringFollow {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
        #[serde(default = "default_spring_duration")]
        duration: f64,
        #[serde(default = "default_spring_bounce")]
        bounce: f64,
    },
    Spring {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
        #[serde(default = "default_spring_duration")]
        duration: f64,
        #[serde(default = "default_spring_bounce")]
        bounce: f64,
        #[serde(default)]
        delay: f64,
    },
    Linear {
        #[serde(flatten)]
        timeline: TimelineMotionNode,
    },
    #[serde(rename = "ease-in")]
    EaseIn {
        #[serde(flatten)]
        timeline: TimelineMotionNode,
    },
    #[serde(rename = "ease-out")]
    EaseOut {
        #[serde(flatten)]
        timeline: TimelineMotionNode,
    },
    #[serde(rename = "ease-in-out")]
    EaseInOut {
        #[serde(flatten)]
        timeline: TimelineMotionNode,
    },
    #[serde(rename = "sine-in")]
    SineIn {
        #[serde(flatten)]
        timeline: TimelineMotionNode,
    },
    #[serde(rename = "sine-out")]
    SineOut {
        #[serde(flatten)]
        timeline: TimelineMotionNode,
    },
    #[serde(rename = "sine-in-out")]
    SineInOut {
        #[serde(flatten)]
        timeline: TimelineMotionNode,
    },
    #[serde(rename = "cosine-in")]
    CosineIn {
        #[serde(flatten)]
        timeline: TimelineMotionNode,
    },
    #[serde(rename = "cosine-out")]
    CosineOut {
        #[serde(flatten)]
        timeline: TimelineMotionNode,
    },
    #[serde(rename = "cosine-in-out")]
    CosineInOut {
        #[serde(flatten)]
        timeline: TimelineMotionNode,
    },
    Instant {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
    },
    #[serde(rename = "EventTrigger")]
    EventTrigger {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
        #[serde(rename = "eventType")]
        event_type: String,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        modifiers: EventModifiers,
        #[serde(default = "default_true", rename = "ignoreRepeat")]
        ignore_repeat: bool,
    },
    #[serde(rename = "Logic")]
    Logic {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
        op: LogicOp,
    },
    #[serde(rename = "BoolInput")]
    BoolInput {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        value: bool,
    },
    #[serde(rename = "FloatInput")]
    FloatInput {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        value: f64,
    },
    #[serde(rename = "MathAdd")]
    MathAdd {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
    },
    #[serde(rename = "MathSubtract")]
    MathSubtract {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
    },
    #[serde(rename = "MathMultiply")]
    MathMultiply {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
    },
    #[serde(rename = "MathDivide")]
    MathDivide {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
    },
    #[serde(rename = "Lerp")]
    Lerp {
        id: String,
        #[serde(default)]
        position: Position,
        #[serde(default)]
        label: Option<String>,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventModifiers {
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub meta: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
pub enum LogicOp {
    #[serde(rename = "and")]
    And,
    #[serde(rename = "or")]
    Or,
    #[serde(rename = "not")]
    Not,
    #[serde(rename = "equal")]
    Equal,
    #[serde(rename = "notEqual")]
    NotEqual,
    #[serde(rename = "greater")]
    Greater,
    #[serde(rename = "greaterEqual")]
    GreaterEqual,
    #[serde(rename = "less")]
    Less,
    #[serde(rename = "lessEqual")]
    LessEqual,
}

fn default_spring_duration() -> f64 {
    0.45
}

fn default_repeat_to() -> f64 {
    1.0
}

fn default_repeat_duration() -> f64 {
    1.0
}

fn default_spring_bounce() -> f64 {
    0.1
}

fn default_timeline_duration() -> f64 {
    0.3
}

impl TransitionMotionNode {
    pub fn id(&self) -> &str {
        match self {
            Self::RepeatTimeline { id, .. }
            | Self::SpringFollow { id, .. }
            | Self::Spring { id, .. }
            | Self::Instant { id, .. }
            | Self::EventTrigger { id, .. }
            | Self::Logic { id, .. }
            | Self::BoolInput { id, .. }
            | Self::FloatInput { id, .. }
            | Self::MathAdd { id, .. }
            | Self::MathSubtract { id, .. }
            | Self::MathMultiply { id, .. }
            | Self::MathDivide { id, .. }
            | Self::Lerp { id, .. } => id,
            Self::Linear { timeline }
            | Self::EaseIn { timeline }
            | Self::EaseOut { timeline }
            | Self::EaseInOut { timeline }
            | Self::SineIn { timeline }
            | Self::SineOut { timeline }
            | Self::SineInOut { timeline }
            | Self::CosineIn { timeline }
            | Self::CosineOut { timeline }
            | Self::CosineInOut { timeline } => &timeline.id,
        }
    }

    pub fn timeline(&self) -> Option<(TimelinePreset, &TimelineMotionNode)> {
        Some(match self {
            Self::Linear { timeline } => (TimelinePreset::Linear, timeline),
            Self::EaseIn { timeline } => (TimelinePreset::EaseIn, timeline),
            Self::EaseOut { timeline } => (TimelinePreset::EaseOut, timeline),
            Self::EaseInOut { timeline } => (TimelinePreset::EaseInOut, timeline),
            Self::SineIn { timeline } => (TimelinePreset::SineIn, timeline),
            Self::SineOut { timeline } => (TimelinePreset::SineOut, timeline),
            Self::SineInOut { timeline } => (TimelinePreset::SineInOut, timeline),
            Self::CosineIn { timeline } => (TimelinePreset::CosineIn, timeline),
            Self::CosineOut { timeline } => (TimelinePreset::CosineOut, timeline),
            Self::CosineInOut { timeline } => (TimelinePreset::CosineInOut, timeline),
            Self::Spring { .. }
            | Self::RepeatTimeline { .. }
            | Self::SpringFollow { .. }
            | Self::Instant { .. }
            | Self::EventTrigger { .. }
            | Self::Logic { .. }
            | Self::BoolInput { .. }
            | Self::FloatInput { .. }
            | Self::MathAdd { .. }
            | Self::MathSubtract { .. }
            | Self::MathMultiply { .. }
            | Self::MathDivide { .. }
            | Self::Lerp { .. } => return None,
        })
    }

    pub fn is_timing(&self) -> bool {
        matches!(
            self,
            Self::RepeatTimeline { .. }
                | Self::SpringFollow { .. }
                | Self::Spring { .. }
                | Self::Linear { .. }
                | Self::EaseIn { .. }
                | Self::EaseOut { .. }
                | Self::EaseInOut { .. }
                | Self::SineIn { .. }
                | Self::SineOut { .. }
                | Self::SineInOut { .. }
                | Self::CosineIn { .. }
                | Self::CosineOut { .. }
                | Self::CosineInOut { .. }
                | Self::Instant { .. }
        )
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TransitionMotionGraph {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub inputs: Vec<MutationPort>,
    #[serde(default)]
    pub outputs: Vec<MutationPort>,
    #[serde(default)]
    pub nodes: Vec<TransitionMotionNode>,
    #[serde(default)]
    pub connections: Vec<MutationConnection>,
    #[serde(default, rename = "inputBindings")]
    pub input_bindings: Vec<TransitionMotionInputBinding>,
    #[serde(default, rename = "outputBindings")]
    pub output_bindings: Vec<TransitionMotionOutputBinding>,
    #[serde(default, rename = "passthroughBindings")]
    pub passthrough_bindings: Vec<TransitionMotionPassthroughBinding>,
    #[serde(default, rename = "conditionBinding")]
    pub condition_binding: Option<TransitionConditionBinding>,
    #[serde(default)]
    pub viewport: Option<Viewport>,
}

impl TransitionMotionGraph {
    /// Build the canonical `Any -> Instant -> Any` graph used for edges that
    /// should update all properties without interpolation.
    pub fn instant(id: impl Into<String>) -> Self {
        let port = MutationPort {
            id: "*".into(),
            name: Some("Any".into()),
            port_type: Some("any".into()),
        };
        Self {
            id: id.into(),
            name: "Instant".into(),
            inputs: vec![port.clone()],
            outputs: vec![port],
            nodes: vec![TransitionMotionNode::Instant {
                id: "motion".into(),
                position: Position::default(),
                label: None,
            }],
            connections: vec![],
            input_bindings: vec![TransitionMotionInputBinding {
                port_id: "*".into(),
                to: MutationEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
            }],
            output_bindings: vec![TransitionMotionOutputBinding {
                port_id: "*".into(),
                from: MutationEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
            }],
            passthrough_bindings: vec![],
            condition_binding: None,
            viewport: None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TransitionMotionInputBinding {
    #[serde(rename = "motionPortId")]
    pub port_id: String,
    pub to: MutationEndpoint,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TransitionMotionOutputBinding {
    #[serde(rename = "motionPortId")]
    pub port_id: String,
    pub from: MutationEndpoint,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TransitionMotionPassthroughBinding {
    #[serde(rename = "inputPortId")]
    pub from_port_id: String,
    #[serde(rename = "outputPortId")]
    pub to_port_id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum TransitionConditionBinding {
    Input {
        #[serde(rename = "inputPortId")]
        input_port_id: String,
    },
    Node {
        from: MutationEndpoint,
    },
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
    #[serde(default, rename = "passthroughBindings")]
    pub passthrough_bindings: Vec<MutationPassthroughBinding>,
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
    #[serde(rename = "FloatInput")]
    FloatInput,
    #[serde(rename = "MutationFunction")]
    MutationFunction,
    #[serde(rename = "PackArray")]
    PackArray,
    #[serde(rename = "MathAdd")]
    MathAdd,
    #[serde(rename = "MathSubtract")]
    MathSubtract,
    #[serde(rename = "MathMultiply")]
    MathMultiply,
    #[serde(rename = "MathDivide")]
    MathDivide,
    #[serde(rename = "Lerp")]
    Lerp,
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
    #[serde(rename = "mutationPortId")]
    pub port_id: String,
    /// Where the value is fed into the inner graph.
    pub to: MutationEndpoint,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MutationOutputBinding {
    /// Port on the mutation boundary.
    #[serde(rename = "mutationPortId")]
    pub port_id: String,
    /// Where the value comes from in the inner graph.
    pub from: MutationEndpoint,
}

/// A direct boundary-to-boundary passthrough binding.
///
/// Maps an input port value directly to an output port without requiring
/// inner nodes.  Typically used for wiring built-in time ports
/// (e.g. `sceneElapsedTime`) straight to override targets.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MutationPassthroughBinding {
    /// Input port id on the mutation boundary (source of value).
    #[serde(rename = "inputPortId")]
    pub from_port_id: String,
    /// Output port id on the mutation boundary (destination).
    #[serde(rename = "outputPortId")]
    pub to_port_id: String,
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
// Runtime input snapshots
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MousePosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RuntimeInputSnapshot {
    pub mouse_position: Option<MousePosition>,
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

#[cfg(test)]
mod tests {
    use super::{
        MutationInnerNodeType, MutationInputBinding, MutationOutputBinding,
        MutationPassthroughBinding, MutationPort, TimelinePreset, TransitionMotionNode,
    };

    #[test]
    fn mutation_input_binding_parses_editor_port_name() {
        let parsed: MutationInputBinding = serde_json::from_value(serde_json::json!({
            "mutationPortId": "Vector2Input_74:x",
            "to": {
                "nodeId": "mouse",
                "portId": "position.x",
            },
        }))
        .expect("editor input binding should deserialize");

        assert_eq!(parsed.port_id, "Vector2Input_74:x");
        assert_eq!(parsed.to.node_id, "mouse");
        assert_eq!(parsed.to.port_id, "position.x");
    }

    #[test]
    fn mutation_output_binding_parses_editor_port_name() {
        let parsed: MutationOutputBinding = serde_json::from_value(serde_json::json!({
            "mutationPortId": "Vector2Input_74:x",
            "from": {
                "nodeId": "mouse",
                "portId": "position.x",
            },
        }))
        .expect("editor output binding should deserialize");

        assert_eq!(parsed.port_id, "Vector2Input_74:x");
        assert_eq!(parsed.from.node_id, "mouse");
        assert_eq!(parsed.from.port_id, "position.x");
    }

    #[test]
    fn mutation_passthrough_binding_parses_editor_port_names() {
        let parsed: MutationPassthroughBinding = serde_json::from_value(serde_json::json!({
            "inputPortId": "sceneElapsedTime",
            "outputPortId": "FloatInput_53:value",
        }))
        .expect("editor passthrough binding should deserialize");

        assert_eq!(parsed.from_port_id, "sceneElapsedTime");
        assert_eq!(parsed.to_port_id, "FloatInput_53:value");
    }

    #[test]
    fn pack_array_inner_node_type_deserializes() {
        let parsed: MutationInnerNodeType = serde_json::from_value(serde_json::json!("PackArray"))
            .expect("PackArray inner node type should deserialize");

        assert_eq!(parsed, MutationInnerNodeType::PackArray);
    }

    #[test]
    fn packed_port_type_deserializes() {
        let parsed: MutationPort = serde_json::from_value(serde_json::json!({
            "id": "packed",
            "name": "Packed",
            "type": "packed<float>",
        }))
        .expect("packed mutation port should deserialize");

        assert_eq!(parsed.port_type.as_deref(), Some("packed<float>"));
    }

    #[test]
    fn timeline_presets_are_independent_flat_motion_node_types() {
        let cases = [
            ("linear", TimelinePreset::Linear),
            ("ease-in", TimelinePreset::EaseIn),
            ("ease-out", TimelinePreset::EaseOut),
            ("ease-in-out", TimelinePreset::EaseInOut),
            ("sine-in", TimelinePreset::SineIn),
            ("sine-out", TimelinePreset::SineOut),
            ("sine-in-out", TimelinePreset::SineInOut),
            ("cosine-in", TimelinePreset::CosineIn),
            ("cosine-out", TimelinePreset::CosineOut),
            ("cosine-in-out", TimelinePreset::CosineInOut),
        ];

        for (node_type, expected_curve) in cases {
            let node: TransitionMotionNode = serde_json::from_value(serde_json::json!({
                "id": "motion",
                "type": node_type,
                "position": { "x": 10.0, "y": 20.0 },
                "duration": 0.4,
                "delay": 0.1,
                "blending": {
                    "type": "tween",
                    "duration": 0.12,
                    "easing": "ease-in-out"
                }
            }))
            .unwrap_or_else(|error| panic!("failed to deserialize {node_type}: {error}"));
            let (curve, timeline) = node.timeline().expect("expected timeline-based node");
            assert_eq!(curve, expected_curve);
            assert_eq!(timeline.duration, 0.4);

            let serialized = serde_json::to_value(&node).expect("motion node should serialize");
            assert_eq!(serialized["type"], node_type);
            assert_eq!(serialized["duration"], 0.4);
            assert!(serialized.get("curve").is_none());
            assert!(serialized.get("timeline").is_none());
        }
    }
}
