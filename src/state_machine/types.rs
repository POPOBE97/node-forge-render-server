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
#[serde(deny_unknown_fields)]
pub struct StateMachine {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "stateParams")]
    pub state_params: Vec<StateParamDeclaration>,
    #[serde(rename = "stateParamGraph")]
    pub state_param_graph: StateParamGraph,
    #[serde(default)]
    pub states: Vec<AnimationState>,
    #[serde(default)]
    pub transitions: Vec<AnimationTransition>,
    #[serde(default, rename = "derivationBindings")]
    pub derivation_bindings: Vec<DerivationStateBinding>,
    #[serde(default)]
    pub derivations: Vec<DerivationDefinition>,
    #[serde(default, rename = "motionGraphs")]
    pub motion_graphs: Vec<TransitionMotionGraph>,
    #[serde(default, rename = "initialStateId")]
    pub initial_state_id: Option<String>,
    /// Editor-only viewport metadata — ignored at runtime.
    #[serde(default)]
    pub viewport: Option<Viewport>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct StateParamDeclaration {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: String,
    #[serde(rename = "defaultValue")]
    pub default_value: serde_json::Value,
    #[serde(default, rename = "arrayLength")]
    pub array_length: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct StateParamGraph {
    #[serde(rename = "rootNodePosition")]
    pub root_node_position: Position,
    #[serde(rename = "declarationPositions")]
    pub declaration_positions: HashMap<String, Position>,
    #[serde(default)]
    pub viewport: Option<Viewport>,
}

impl Default for StateParamGraph {
    fn default() -> Self {
        Self {
            root_node_position: Position {
                x: -320.0,
                y: -120.0,
            },
            declaration_positions: HashMap::new(),
            viewport: None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum StateValueSource {
    StateParam {
        #[serde(rename = "stateParamId")]
        state_param_id: String,
    },
    FrameInput {
        #[serde(rename = "frameInputId")]
        frame_input_id: String,
    },
}

impl StateValueSource {
    pub fn id(&self) -> &str {
        match self {
            Self::StateParam { state_param_id } => state_param_id,
            Self::FrameInput { frame_input_id } => frame_input_id,
        }
    }

    pub fn state_param_id(&self) -> Option<&str> {
        match self {
            Self::StateParam { state_param_id } => Some(state_param_id),
            Self::FrameInput { .. } => None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct GpuUniformRef {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "paramId")]
    pub param_id: String,
}

impl GpuUniformRef {
    pub fn key(&self) -> OverrideKey {
        OverrideKey::new(&self.node_id, &self.param_id)
    }

    pub fn id(&self) -> String {
        format!("{}:{}", self.node_id, self.param_id)
    }
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
    DerivationNode,
}

/// A single state in the state graph.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AnimationState {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub position: Option<Position>,
    #[serde(default, rename = "stateParamOverrides")]
    pub state_param_overrides: HashMap<String, serde_json::Value>,
    #[serde(rename = "type")]
    pub state_type: AnimationStateType,
    /// Private Mutation graph owned by a regular animation State.
    #[serde(default, rename = "mutationGraph")]
    pub mutation_graph: Option<StateMutationGraph>,
    /// Shared Derivation referenced by a standalone `derivationNode`.
    #[serde(default, rename = "derivationId")]
    pub derivation_id: Option<String>,
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

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct DerivationStateBinding {
    pub id: String,
    #[serde(rename = "stateId")]
    pub state_id: String,
    #[serde(rename = "derivationNodeId")]
    pub derivation_node_id: String,
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

fn default_spring_bounce() -> f64 {
    0.1
}

fn default_timeline_duration() -> f64 {
    0.3
}

impl TransitionMotionNode {
    pub fn id(&self) -> &str {
        match self {
            Self::Spring { id, .. }
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
            Self::Spring { .. }
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
#[serde(deny_unknown_fields)]
pub struct TransitionMotionGraph {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(skip)]
    pub inputs: Vec<GraphPort>,
    #[serde(skip)]
    pub outputs: Vec<GraphPort>,
    #[serde(default)]
    pub nodes: Vec<TransitionMotionNode>,
    #[serde(default)]
    pub connections: Vec<GraphConnection>,
    #[serde(default, rename = "inputBindings")]
    pub input_bindings: Vec<TransitionMotionInputBinding>,
    #[serde(default, rename = "outputBindings")]
    pub output_bindings: Vec<TransitionMotionOutputBinding>,
    #[serde(default, rename = "passthroughBindings")]
    pub passthrough_bindings: Vec<TransitionMotionPassthroughBinding>,
    #[serde(default, rename = "conditionBinding")]
    pub condition_binding: Option<TransitionConditionBinding>,
    #[serde(default)]
    pub layout: Option<TransitionMotionGraphLayout>,
    #[serde(default)]
    pub viewport: Option<Viewport>,
}

impl TransitionMotionGraph {
    /// Build the canonical `Any -> Instant -> Any` graph used for edges that
    /// should update all properties without interpolation.
    pub fn instant(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: "Instant".into(),
            inputs: vec![],
            outputs: vec![],
            nodes: vec![TransitionMotionNode::Instant {
                id: "motion".into(),
                position: Position::default(),
                label: None,
            }],
            connections: vec![],
            input_bindings: vec![TransitionMotionInputBinding {
                source: StateValueSource::StateParam {
                    state_param_id: "*".into(),
                },
                to: GraphEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
            }],
            output_bindings: vec![TransitionMotionOutputBinding {
                state_param_id: "*".into(),
                from: GraphEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
            }],
            passthrough_bindings: vec![],
            condition_binding: None,
            layout: None,
            viewport: None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TransitionMotionInputBinding {
    pub source: StateValueSource,
    pub to: GraphEndpoint,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TransitionMotionOutputBinding {
    #[serde(rename = "stateParamId")]
    pub state_param_id: String,
    pub from: GraphEndpoint,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TransitionMotionPassthroughBinding {
    #[serde(rename = "stateParamId")]
    pub state_param_id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "source", rename_all = "lowercase", deny_unknown_fields)]
pub enum TransitionConditionBinding {
    Input { input: StateValueSource },
    Node { from: GraphEndpoint },
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransitionMotionGraphLayout {
    pub input_position: Position,
    pub output_position: Position,
    pub condition_output_position: Position,
    #[serde(default)]
    pub input_collapsed: bool,
    #[serde(default)]
    pub output_collapsed: bool,
    #[serde(default)]
    pub condition_output_collapsed: bool,
}

// ---------------------------------------------------------------------------
// State Mutation and Render Derivation graphs
// ---------------------------------------------------------------------------

/// A reusable, stateless render Derivation graph.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct DerivationDefinition {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(skip)]
    pub inputs: Vec<GraphPort>,
    #[serde(skip)]
    pub outputs: Vec<GraphPort>,
    #[serde(default)]
    pub nodes: Vec<GraphInnerNode>,
    #[serde(default)]
    pub connections: Vec<GraphConnection>,
    #[serde(default, rename = "inputBindings")]
    pub input_bindings: Vec<DerivationInputBinding>,
    #[serde(default, rename = "outputBindings")]
    pub output_bindings: Vec<DerivationOutputBinding>,
    #[serde(default, rename = "passthroughBindings")]
    pub passthrough_bindings: Vec<DerivationPassthroughBinding>,
    #[serde(default)]
    pub layout: Option<DerivationGraphLayout>,
    /// Editor-only viewport metadata — ignored at runtime.
    #[serde(default)]
    pub viewport: Option<Viewport>,
}

/// A private Mutation graph embedded in one regular State.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct StateMutationGraph {
    #[serde(skip)]
    pub inputs: Vec<GraphPort>,
    #[serde(skip)]
    pub outputs: Vec<GraphPort>,
    #[serde(default)]
    pub nodes: Vec<GraphInnerNode>,
    #[serde(default)]
    pub connections: Vec<GraphConnection>,
    #[serde(default, rename = "inputBindings")]
    pub input_bindings: Vec<StateMutationInputBinding>,
    #[serde(default, rename = "outputBindings")]
    pub output_bindings: Vec<StateMutationOutputBinding>,
    pub layout: StateMutationGraphLayout,
    #[serde(default)]
    pub viewport: Option<Viewport>,
}

impl StateMutationGraph {
    pub fn as_executable(&self, state_id: &str) -> DerivationDefinition {
        DerivationDefinition {
            id: state_id.to_string(),
            name: format!("{state_id} Mutation"),
            inputs: self.inputs.clone(),
            outputs: self.outputs.clone(),
            nodes: self.nodes.clone(),
            connections: self.connections.clone(),
            input_bindings: self
                .input_bindings
                .iter()
                .map(|binding| DerivationInputBinding {
                    source: binding.source.clone(),
                    to: binding.to.clone(),
                })
                .collect(),
            output_bindings: self
                .output_bindings
                .iter()
                .map(|binding| DerivationOutputBinding {
                    uniform: GpuUniformRef {
                        node_id: String::new(),
                        param_id: binding.state_param_id.clone(),
                    },
                    from: binding.from.clone(),
                })
                .collect(),
            passthrough_bindings: Vec::new(),
            layout: None,
            viewport: self.viewport,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct StateMutationInputBinding {
    pub source: StateValueSource,
    pub to: GraphEndpoint,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct StateMutationOutputBinding {
    #[serde(rename = "stateParamId")]
    pub state_param_id: String,
    pub from: GraphEndpoint,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateMutationGraphLayout {
    #[serde(default)]
    pub parameter_positions: HashMap<String, Position>,
    pub runtime_input_position: Position,
    pub output_position: Position,
    #[serde(default)]
    pub runtime_input_collapsed: bool,
    #[serde(default)]
    pub output_collapsed: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DerivationGraphLayout {
    pub input_position: Position,
    pub output_position: Position,
    #[serde(default)]
    pub input_collapsed: bool,
    #[serde(default)]
    pub output_collapsed: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct GraphPort {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "type", default)]
    pub port_type: Option<String>,
    #[serde(default, rename = "arrayLength")]
    pub array_length: Option<usize>,
    #[serde(default)]
    pub motion: Option<bool>,
}

/// Supported inner-node types for mutation subgraphs (v1).
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum GraphInnerNodeType {
    #[serde(rename = "FloatInput")]
    FloatInput,
    #[serde(rename = "MutationFunction")]
    MutationFunction,
    #[serde(rename = "DerivationFunction")]
    DerivationFunction,
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
pub struct GraphInnerNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: GraphInnerNodeType,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub inputs: Vec<GraphPort>,
    #[serde(default)]
    pub outputs: Vec<GraphPort>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GraphConnection {
    pub id: String,
    pub from: GraphEndpoint,
    pub to: GraphEndpoint,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GraphEndpoint {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "portId")]
    pub port_id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DerivationInputBinding {
    pub source: StateValueSource,
    pub to: GraphEndpoint,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DerivationOutputBinding {
    pub uniform: GpuUniformRef,
    pub from: GraphEndpoint,
}

/// A direct boundary-to-boundary passthrough binding.
///
/// Maps an input port value directly to an output port without requiring
/// inner nodes.  Typically used for wiring built-in time ports
/// (e.g. `sceneElapsedTime`) straight to override targets.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DerivationPassthroughBinding {
    pub source: StateValueSource,
    pub uniform: GpuUniformRef,
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

/// Stable semantic identity owned by the State Machine.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StateParamKey(pub String);

impl StateParamKey {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A typed key for runtime parameter overrides produced by the state machine.
///
/// Encodes `nodeId:paramName` — the same format used by the editor's
/// State Mutation and Derivation interface ports.
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
        DerivationInputBinding, DerivationOutputBinding, DerivationPassthroughBinding,
        GraphInnerNodeType, GraphPort, TimelinePreset, TransitionMotionNode,
    };

    #[test]
    fn derivation_input_binding_parses_editor_port_name() {
        let parsed: DerivationInputBinding = serde_json::from_value(serde_json::json!({
            "source": {
                "kind": "stateParam",
                "stateParamId": "position_x"
            },
            "to": {
                "nodeId": "mouse",
                "portId": "position.x",
            },
        }))
        .expect("editor input binding should deserialize");

        assert_eq!(parsed.source.state_param_id(), Some("position_x"));
        assert_eq!(parsed.to.node_id, "mouse");
        assert_eq!(parsed.to.port_id, "position.x");
    }

    #[test]
    fn derivation_output_binding_parses_editor_port_name() {
        let parsed: DerivationOutputBinding = serde_json::from_value(serde_json::json!({
            "uniform": {
                "nodeId": "Vector2Input_74",
                "paramId": "x"
            },
            "from": {
                "nodeId": "mouse",
                "portId": "position.x",
            },
        }))
        .expect("editor output binding should deserialize");

        assert_eq!(parsed.uniform.id(), "Vector2Input_74:x");
        assert_eq!(parsed.from.node_id, "mouse");
        assert_eq!(parsed.from.port_id, "position.x");
    }

    #[test]
    fn derivation_passthrough_binding_parses_editor_port_names() {
        let parsed: DerivationPassthroughBinding = serde_json::from_value(serde_json::json!({
            "source": {
                "kind": "frameInput",
                "frameInputId": "sceneElapsedTime"
            },
            "uniform": {
                "nodeId": "FloatInput_53",
                "paramId": "value"
            }
        }))
        .expect("editor passthrough binding should deserialize");

        assert_eq!(parsed.source.id(), "sceneElapsedTime");
        assert_eq!(parsed.uniform.id(), "FloatInput_53:value");
    }

    #[test]
    fn pack_array_inner_node_type_deserializes() {
        let parsed: GraphInnerNodeType = serde_json::from_value(serde_json::json!("PackArray"))
            .expect("PackArray inner node type should deserialize");

        assert_eq!(parsed, GraphInnerNodeType::PackArray);
    }

    #[test]
    fn packed_port_type_deserializes() {
        let parsed: GraphPort = serde_json::from_value(serde_json::json!({
            "id": "packed",
            "name": "Packed",
            "type": "packed<float>",
        }))
        .expect("packed graph port should deserialize");

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
