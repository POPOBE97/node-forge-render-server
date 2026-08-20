//! State and Derivation graph compiler and evaluator.
//!
//! Takes a `DerivationDefinition`, resolves input bindings, evaluates
//! the inner-node DAG in topological order, and produces output values
//! via output bindings.
//!
//! Supported inner-node types (v1):
//! - `FloatInput`     — emits its constant `value` parameter.
//! - `PackArray`      — packs connected input values into `Packed<T>`.
//! - `MathAdd`        — adds connected inputs.
//! - `MathSubtract`   — subtracts connected inputs in order.
//! - `MathMultiply`   — multiplies connected inputs in order.
//! - `MathDivide`     — divides connected inputs in order.
//! - `Lerp`           — `mix(a, b, t)`.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

use anyhow::{Result, bail};

use super::{motion::MotionEngine, types::*};

const GRAPH_FRAME_BUDGET: Duration = Duration::from_millis(4);

// ---------------------------------------------------------------------------
// Typed animation value
// ---------------------------------------------------------------------------

/// A typed value flowing through the graph_definition graph.
///
#[derive(Debug, Clone, PartialEq)]
pub enum AnimValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Vec2([f64; 2]),
    Vec3([f64; 3]),
    Vec4([f64; 4]),
    Color([f64; 4]),
    BezierCurve([[f64; 2]; 4]),
    NormalizedBezierCurve([f64; 4]),
    Packed(Vec<AnimValue>),
}

impl AnimValue {
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        Self::from_json_typed(value, None)
    }

    pub fn from_json_typed(value: &serde_json::Value, port_type: Option<&str>) -> Option<Self> {
        if let Some(element_type) = port_type
            .and_then(|value| value.strip_prefix("packed<"))
            .and_then(|value| value.strip_suffix('>'))
        {
            return Some(Self::Packed(
                value
                    .as_array()?
                    .iter()
                    .map(|item| Self::from_json_typed(item, Some(element_type)))
                    .collect::<Option<Vec<_>>>()?,
            ));
        }
        if port_type == Some("bool") {
            return value.as_bool().map(Self::Bool);
        }
        if port_type == Some("int") {
            return value.as_i64().map(Self::Int);
        }
        if let Some(value) = value.as_f64() {
            return Some(Self::Float(value));
        }
        let values = value.as_array()?;
        match port_type {
            Some("vector2") => Some(Self::Vec2([
                values.first()?.as_f64()?,
                values.get(1)?.as_f64()?,
            ])),
            Some("vector3") => Some(Self::Vec3([
                values.first()?.as_f64()?,
                values.get(1)?.as_f64()?,
                values.get(2)?.as_f64()?,
            ])),
            Some("vector4") => Some(Self::Vec4([
                values.first()?.as_f64()?,
                values.get(1)?.as_f64()?,
                values.get(2)?.as_f64()?,
                values.get(3)?.as_f64()?,
            ])),
            Some("color") => Some(Self::Color([
                values.first()?.as_f64()?,
                values.get(1)?.as_f64()?,
                values.get(2)?.as_f64()?,
                values.get(3)?.as_f64()?,
            ])),
            Some("normalizedBezierCurve") => Some(Self::NormalizedBezierCurve([
                values.first()?.as_f64()?,
                values.get(1)?.as_f64()?,
                values.get(2)?.as_f64()?,
                values.get(3)?.as_f64()?,
            ])),
            Some("bezierCurve") => {
                if values.len() != 4 {
                    return None;
                }
                let points = values
                    .iter()
                    .map(|point| {
                        let point = point.as_array()?;
                        if point.len() != 2 {
                            return None;
                        }
                        Some([point.first()?.as_f64()?, point.get(1)?.as_f64()?])
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(Self::BezierCurve(points.try_into().ok()?))
            }
            _ if values.len() == 2 => Some(Self::Vec2([values[0].as_f64()?, values[1].as_f64()?])),
            _ if values.len() == 3 => Some(Self::Vec3([
                values[0].as_f64()?,
                values[1].as_f64()?,
                values[2].as_f64()?,
            ])),
            _ if values.len() == 4 => Some(Self::Vec4([
                values[0].as_f64()?,
                values[1].as_f64()?,
                values[2].as_f64()?,
                values[3].as_f64()?,
            ])),
            _ => Some(Self::Packed(
                values
                    .iter()
                    .map(Self::from_json)
                    .collect::<Option<Vec<_>>>()?,
            )),
        }
    }

    /// Extract as `f64`, converting if possible.
    pub fn as_f64(self) -> Option<f64> {
        match self {
            AnimValue::Float(v) => Some(v),
            AnimValue::Int(v) => Some(v as f64),
            AnimValue::Bool(_)
            | AnimValue::Vec2(_)
            | AnimValue::Vec3(_)
            | AnimValue::Vec4(_)
            | AnimValue::Color(_)
            | AnimValue::BezierCurve(_)
            | AnimValue::NormalizedBezierCurve(_)
            | AnimValue::Packed(_) => None,
        }
    }

    /// Convert to `serde_json::Value` for the override boundary.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            AnimValue::Float(v) => serde_json::json!(v),
            AnimValue::Int(v) => serde_json::json!(v),
            AnimValue::Bool(v) => serde_json::json!(v),
            AnimValue::Vec2(v) => serde_json::json!([v[0], v[1]]),
            AnimValue::Vec3(v) => serde_json::json!([v[0], v[1], v[2]]),
            AnimValue::Vec4(v) => serde_json::json!([v[0], v[1], v[2], v[3]]),
            AnimValue::Color(v) => serde_json::json!([v[0], v[1], v[2], v[3]]),
            AnimValue::BezierCurve(v) => serde_json::json!([
                [v[0][0], v[0][1]],
                [v[1][0], v[1][1]],
                [v[2][0], v[2][1]],
                [v[3][0], v[3][1]],
            ]),
            AnimValue::NormalizedBezierCurve(v) => {
                serde_json::json!([v[0], v[1], v[2], v[3]])
            }
            AnimValue::Packed(values) => {
                serde_json::Value::Array(values.iter().map(AnimValue::to_json).collect())
            }
        }
    }

    pub fn zero_like(&self) -> Self {
        match self {
            Self::Float(_) => Self::Float(0.0),
            Self::Int(_) => Self::Int(0),
            Self::Bool(_) => Self::Bool(false),
            Self::Vec2(_) => Self::Vec2([0.0; 2]),
            Self::Vec3(_) => Self::Vec3([0.0; 3]),
            Self::Vec4(_) => Self::Vec4([0.0; 4]),
            Self::Color(_) => Self::Color([0.0; 4]),
            Self::BezierCurve(_) => Self::BezierCurve([[0.0; 2]; 4]),
            Self::NormalizedBezierCurve(_) => Self::NormalizedBezierCurve([0.0; 4]),
            Self::Packed(values) => Self::Packed(values.iter().map(Self::zero_like).collect()),
        }
    }
}

impl Default for AnimValue {
    fn default() -> Self {
        AnimValue::Float(0.0)
    }
}

impl From<f64> for AnimValue {
    fn from(v: f64) -> Self {
        AnimValue::Float(v)
    }
}

impl From<[f64; 2]> for AnimValue {
    fn from(v: [f64; 2]) -> Self {
        AnimValue::Vec2(v)
    }
}

impl From<[f64; 4]> for AnimValue {
    fn from(v: [f64; 4]) -> Self {
        AnimValue::Color(v)
    }
}

pub type GraphValue = AnimValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphEvaluationPhase {
    All,
    Target,
    Render,
}

/// Input context supplied to graph_definition evaluation.
pub struct GraphInputContext {
    /// Current parameter snapshot keyed by graph_definition-input port id.
    pub values: HashMap<String, GraphValue>,
    /// Monotonic scene time in seconds.
    pub scene_elapsed_time: f64,
    /// Time since the current state was entered, in seconds.
    pub local_elapsed_time: f64,
    /// Latest mouse position in render-target frag pixel coordinates.
    pub mouse_position: Option<MousePosition>,
    /// Current full render-target size in bottom-left ScenePx space.
    pub scene_size: Option<SceneSize>,
    pub dt: f64,
}

/// Evaluate a graph_definition definition given its input context.
///
/// Returns a map from graph_definition-output port id → computed value.
pub fn evaluate_graph(
    graph_definition: &DerivationDefinition,
    ctx: &GraphInputContext,
) -> Result<HashMap<String, GraphValue>> {
    let mut motion_engine = MotionEngine::new();
    evaluate_graph_with_motion(graph_definition, ctx, &mut motion_engine)
}

pub fn evaluate_graph_with_motion(
    graph_definition: &DerivationDefinition,
    ctx: &GraphInputContext,
    motion_engine: &mut MotionEngine,
) -> Result<HashMap<String, GraphValue>> {
    evaluate_graph_with_motion_phase(
        graph_definition,
        ctx,
        motion_engine,
        GraphEvaluationPhase::All,
    )
}

pub fn evaluate_graph_with_motion_phase(
    graph_definition: &DerivationDefinition,
    ctx: &GraphInputContext,
    motion_engine: &mut MotionEngine,
    phase: GraphEvaluationPhase,
) -> Result<HashMap<String, GraphValue>> {
    let has_inner_graph =
        !graph_definition.nodes.is_empty() || !graph_definition.connections.is_empty();
    let has_passthroughs =
        phase != GraphEvaluationPhase::Target && !graph_definition.passthrough_bindings.is_empty();

    // Fast path: nothing to evaluate.
    if !has_inner_graph && !has_passthroughs {
        return Ok(HashMap::new());
    }

    let mut outputs: HashMap<String, GraphValue> = HashMap::new();

    // ── Evaluate inner graph (if any) ──────────────────────────────────
    if has_inner_graph {
        let nodes_by_id: HashMap<&str, &GraphInnerNode> = graph_definition
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n))
            .collect();

        let required_nodes = required_node_ids(graph_definition, phase);
        let order = topological_sort(&graph_definition.nodes, &graph_definition.connections)?;
        let deadline = Instant::now() + GRAPH_FRAME_BUDGET;

        let mut port_values: HashMap<(&str, &str), GraphValue> = HashMap::new();

        for b in &graph_definition.input_bindings {
            let value = resolve_input_binding_value(b, ctx);
            port_values.insert((b.to.node_id.as_str(), b.to.port_id.as_str()), value);
        }

        for node_id in &order {
            if !required_nodes.contains(node_id.as_str()) {
                continue;
            }
            let remaining_budget = deadline.saturating_duration_since(Instant::now());
            if remaining_budget.is_zero() {
                bail!(
                    "Graph '{}' exceeded its 4ms frame budget",
                    graph_definition.id
                );
            }
            let node = nodes_by_id.get(node_id.as_str()).unwrap();

            for conn in &graph_definition.connections {
                if conn.to.node_id == *node_id {
                    if let Some(val) =
                        port_values.get(&(conn.from.node_id.as_str(), conn.from.port_id.as_str()))
                    {
                        port_values.insert(
                            (conn.to.node_id.as_str(), conn.to.port_id.as_str()),
                            val.clone(),
                        );
                    }
                }
            }

            evaluate_inner_node(
                graph_definition,
                node,
                &mut port_values,
                motion_engine,
                ctx,
                remaining_budget,
                phase,
            )?;
            if Instant::now() >= deadline {
                bail!(
                    "Graph '{}' exceeded its 4ms frame budget",
                    graph_definition.id
                );
            }
        }

        if phase != GraphEvaluationPhase::Target {
            for b in &graph_definition.output_bindings {
                let is_motion_output = nodes_by_id
                    .get(b.from.node_id.as_str())
                    .and_then(|node| node.outputs.iter().find(|port| port.id == b.from.port_id))
                    .is_some_and(|port| port.motion == Some(true));
                if is_motion_output {
                    continue;
                }
                let val = port_values
                    .get(&(b.from.node_id.as_str(), b.from.port_id.as_str()))
                    .cloned()
                    .unwrap_or_default();
                outputs.insert(b.uniform.id(), val);
            }
        }
    }

    // ── Apply passthrough bindings ─────────────────────────────────────
    // Passthroughs map an input boundary port directly to an output port.
    // They only write to output ports not already written by output bindings.
    if phase != GraphEvaluationPhase::Target {
        for pt in &graph_definition.passthrough_bindings {
            let uniform_id = pt.uniform.id();
            if outputs.contains_key(&uniform_id) {
                // Output already written by an output binding — skip (validation
                // catches duplicates as errors, but be defensive at runtime).
                continue;
            }
            let value = resolve_passthrough_input_value(pt.source.id(), graph_definition, ctx);
            outputs.insert(uniform_id, value);
        }
    }

    for port in &graph_definition.outputs {
        let Some(expected) = port.array_length else {
            continue;
        };
        let Some(value) = outputs.get(&port.id) else {
            continue;
        };
        if !matches!(value, GraphValue::Packed(values) if values.len() == expected) {
            bail!(
                "Graph '{}' output '{}' must contain exactly {} elements",
                graph_definition.id,
                port.id,
                expected
            );
        }
    }

    Ok(outputs)
}

fn required_node_ids(
    graph_definition: &DerivationDefinition,
    phase: GraphEvaluationPhase,
) -> HashSet<&str> {
    if phase == GraphEvaluationPhase::All {
        return graph_definition
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect();
    }

    let mut required: HashSet<&str> = match phase {
        GraphEvaluationPhase::Target => graph_definition
            .nodes
            .iter()
            .filter(|node| node.outputs.iter().any(|port| port.motion == Some(true)))
            .map(|node| node.id.as_str())
            .collect(),
        GraphEvaluationPhase::Render => graph_definition
            .output_bindings
            .iter()
            .filter_map(|binding| {
                let node = graph_definition
                    .nodes
                    .iter()
                    .find(|node| node.id == binding.from.node_id)?;
                let port = node
                    .outputs
                    .iter()
                    .find(|port| port.id == binding.from.port_id)?;
                (port.motion != Some(true)).then_some(node.id.as_str())
            })
            .collect(),
        GraphEvaluationPhase::All => unreachable!(),
    };

    loop {
        let previous_len = required.len();
        for connection in &graph_definition.connections {
            if required.contains(connection.to.node_id.as_str()) {
                required.insert(connection.from.node_id.as_str());
            }
        }
        if required.len() == previous_len {
            return required;
        }
    }
}

/// Resolve the value for a passthrough binding's input port.
///
/// Checks well-known built-in references first (the input port id itself
/// may be a well-known name like `"sceneElapsedTime"`), then falls back to
/// matching an input port on the graph_definition boundary, then the values map.
fn resolve_passthrough_input_value(
    from_port_id: &str,
    graph_definition: &DerivationDefinition,
    ctx: &GraphInputContext,
) -> GraphValue {
    // Check well-known built-in ids.
    if let Some(value) = resolve_builtin_value(from_port_id, ctx) {
        return value;
    }

    // Check if the from_port_id matches a graph_definition input port and a
    // corresponding input binding.
    for b in &graph_definition.input_bindings {
        if b.source.id() == from_port_id {
            return resolve_input_binding_value(b, ctx);
        }
    }

    // Fall back to the values map.
    ctx.values.get(from_port_id).cloned().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Unified target resolution
// ---------------------------------------------------------------------------

/// Resolve the override target for an output port id.
///
/// The latest graph_definition format uses the graph_definition output port id itself as
/// `"nodeId:paramName"`.
pub fn resolve_output_target(port_id: &str) -> Option<OverrideKey> {
    OverrideKey::parse(port_id)
}

pub fn expand_output_target_keys(port_id: &str) -> Vec<OverrideKey> {
    resolve_output_target(port_id).into_iter().collect()
}

pub fn expand_output_overrides(
    port_id: &str,
    value: &GraphValue,
) -> Vec<(OverrideKey, serde_json::Value)> {
    resolve_output_target(port_id)
        .map(|key| vec![(key, value.to_json())])
        .unwrap_or_default()
}

/// Collect all override target keys that a graph_definition can produce.
///
/// This is the single source of truth for both runtime override mapping
/// and trace tracked-key discovery.
pub fn all_output_target_keys(graph_definition: &DerivationDefinition) -> Vec<OverrideKey> {
    let mut keys = Vec::new();
    let mut seen = HashSet::new();

    // From output bindings.
    for b in &graph_definition.output_bindings {
        let key = b.uniform.key();
        let s = format!("{}:{}", key.node_id, key.param_name);
        if seen.insert(s) {
            keys.push(key);
        }
    }

    // From passthrough bindings.
    for pt in &graph_definition.passthrough_bindings {
        let key = pt.uniform.key();
        let s = format!("{}:{}", key.node_id, key.param_name);
        if seen.insert(s) {
            keys.push(key);
        }
    }

    keys
}

// ---------------------------------------------------------------------------
// Input binding resolution
// ---------------------------------------------------------------------------

fn resolve_input_binding_value(
    binding: &DerivationInputBinding,
    ctx: &GraphInputContext,
) -> GraphValue {
    if let Some(value) = resolve_builtin_value(binding.source.id(), ctx) {
        return value;
    }

    // Fall back to the current animated/root parameter snapshot.
    ctx.values
        .get(binding.source.id())
        .cloned()
        .unwrap_or_default()
}

fn resolve_builtin_value(name: &str, ctx: &GraphInputContext) -> Option<GraphValue> {
    match name {
        "sceneElapsedTime" => Some(ctx.scene_elapsed_time.into()),
        "localElapsedTime" => Some(ctx.local_elapsed_time.into()),
        "scene.size" => Some(
            ctx.scene_size
                .map(|size| [size.width, size.height])
                .unwrap_or([0.0, 0.0])
                .into(),
        ),
        "mouse.position" => Some(
            ctx.mouse_position
                .map(|p| [p.x, p.y])
                .unwrap_or([0.0, 0.0])
                .into(),
        ),
        "mouse.position.x" => Some(ctx.mouse_position.map(|p| p.x).unwrap_or(0.0).into()),
        "mouse.position.y" => Some(ctx.mouse_position.map(|p| p.y).unwrap_or(0.0).into()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Inner node evaluation
// ---------------------------------------------------------------------------

fn evaluate_inner_node<'a>(
    graph_definition: &DerivationDefinition,
    node: &'a GraphInnerNode,
    port_values: &mut HashMap<(&'a str, &'a str), GraphValue>,
    motion_engine: &mut MotionEngine,
    ctx: &GraphInputContext,
    remaining_budget: Duration,
    phase: GraphEvaluationPhase,
) -> Result<()> {
    match node.node_type {
        GraphInnerNodeType::FloatInput => {
            let value = node
                .params
                .get("value")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            write_output_if_declared_or_default(node, port_values, "value", value.into());
        }
        GraphInnerNodeType::PackArray => {
            let packed = node
                .inputs
                .iter()
                .filter_map(|input| get_port_value(node, input.id.as_str(), port_values))
                .collect();
            write_output_if_declared_or_default(
                node,
                port_values,
                "packed",
                AnimValue::Packed(packed),
            );
        }
        GraphInnerNodeType::MutationFunction | GraphInnerNodeType::DerivationFunction => {
            let input = node
                .inputs
                .iter()
                .map(|port| get_port_value(node, port.id.as_str(), port_values).unwrap_or_default())
                .collect::<Vec<_>>();
            let result = super::graph_function::evaluate_function(
                &format!(
                    "{}:{}",
                    if node.node_type == GraphInnerNodeType::MutationFunction {
                        "state"
                    } else {
                        "derivation"
                    },
                    graph_definition.id
                ),
                &node.id,
                &input,
                remaining_budget,
            )?;
            if result.len() != node.outputs.len() {
                bail!(
                    "Graph Function '{}' returned {} outputs for {} declared ports",
                    node.id,
                    result.len(),
                    node.outputs.len()
                );
            }
            for (output, result) in node.outputs.iter().zip(result) {
                let value = if output.motion == Some(true) {
                    let key = motion_output_key(graph_definition, node, output)?;
                    let value = match (phase, result) {
                        (
                            GraphEvaluationPhase::Render,
                            super::graph_function::FunctionOutput::Motion(_),
                        ) => motion_engine.physical_value(&key).ok_or_else(|| {
                            anyhow::anyhow!(
                                "MotionEngine has no physical value for '{}.{}'",
                                node.id,
                                output.id
                            )
                        })?,
                        (
                            GraphEvaluationPhase::Render,
                            super::graph_function::FunctionOutput::Value(_),
                        ) => bail!(
                            "Graph Function '{}.{}' must return a Motion plan",
                            node.id,
                            output.id
                        ),
                        (
                            GraphEvaluationPhase::All | GraphEvaluationPhase::Target,
                            super::graph_function::FunctionOutput::Motion(plan),
                        ) => motion_engine.apply_mutation_plan(&key, plan, ctx.dt)?,
                        (
                            GraphEvaluationPhase::All | GraphEvaluationPhase::Target,
                            super::graph_function::FunctionOutput::Value(_),
                        ) => bail!(
                            "Graph Function '{}.{}' must return a Motion plan",
                            node.id,
                            output.id
                        ),
                    };
                    GraphValue::from_json_typed(&value, output.port_type.as_deref()).ok_or_else(
                        || {
                            anyhow::anyhow!(
                                "MotionEngine returned an incompatible value for '{}.{}'",
                                node.id,
                                output.id
                            )
                        },
                    )?
                } else {
                    match result {
                        super::graph_function::FunctionOutput::Value(value) => value,
                        super::graph_function::FunctionOutput::Motion(_) => bail!(
                            "Graph Function '{}.{}' returned Motion from a plain output",
                            node.id,
                            output.id
                        ),
                    }
                };
                write_output_if_declared_or_default(node, port_values, output.id.as_str(), value);
            }
        }
        GraphInnerNodeType::MathAdd => {
            let inputs = ordered_input_values(node, port_values, &["a", "b"]);
            let result: f64 = inputs.into_iter().sum();
            write_output_if_declared_or_default(node, port_values, "result", result.into());
        }
        GraphInnerNodeType::MathSubtract => {
            let inputs = ordered_input_values(node, port_values, &[]);
            let first = inputs.first().copied().unwrap_or(0.0);
            let rest = inputs.iter().skip(1).sum::<f64>();
            write_output_if_declared_or_default(node, port_values, "result", (first - rest).into());
        }
        GraphInnerNodeType::MathMultiply => {
            let inputs = ordered_input_values(node, port_values, &[]);
            let result = if inputs.is_empty() {
                0.0
            } else {
                inputs.into_iter().fold(1.0, |acc, value| acc * value)
            };
            write_output_if_declared_or_default(node, port_values, "result", result.into());
        }
        GraphInnerNodeType::MathDivide => {
            let inputs = ordered_input_values(node, port_values, &[]);
            let mut iter = inputs.into_iter();
            let mut result = iter.next().unwrap_or(0.0);
            for divisor in iter {
                if divisor.abs() < f64::EPSILON {
                    result = 0.0;
                    break;
                }
                result /= divisor;
            }
            write_output_if_declared_or_default(node, port_values, "result", result.into());
        }
        GraphInnerNodeType::Lerp => {
            let a = input_value_by_id_or_index(node, port_values, "a", 1).unwrap_or(0.0);
            let b = input_value_by_id_or_index(node, port_values, "b", 2).unwrap_or(1.0);
            let t = input_value_by_id_or_index(node, port_values, "t", 0).unwrap_or(0.5);
            write_output_if_declared_or_default(
                node,
                port_values,
                "result",
                (a + (b - a) * t.clamp(0.0, 1.0)).into(),
            );
        }
    }
    Ok(())
}

fn motion_output_key(
    graph_definition: &DerivationDefinition,
    node: &GraphInnerNode,
    output: &GraphPort,
) -> Result<StateParamKey> {
    let mut bindings = graph_definition
        .output_bindings
        .iter()
        .filter(|binding| binding.from.node_id == node.id && binding.from.port_id == output.id);
    let binding = bindings.next().ok_or_else(|| {
        anyhow::anyhow!(
            "Motion output '{}.{}' must bind to a declaration output",
            node.id,
            output.id
        )
    })?;
    if bindings.next().is_some() {
        bail!(
            "Motion output '{}.{}' must bind to exactly one declaration output",
            node.id,
            output.id
        );
    }
    if binding.uniform.node_id.is_empty() {
        Ok(StateParamKey::new(&binding.uniform.param_id))
    } else {
        bail!(
            "Motion output '{}.{}' cannot bind to GPU uniform '{}'",
            node.id,
            output.id,
            binding.uniform.id()
        )
    }
}

fn write_output_if_declared_or_default<'a>(
    node: &'a GraphInnerNode,
    port_values: &mut HashMap<(&'a str, &'a str), GraphValue>,
    port_id: &'a str,
    value: GraphValue,
) {
    if node.outputs.is_empty() || node.outputs.iter().any(|p| p.id == port_id) {
        port_values.insert((node.id.as_str(), port_id), value);
    }
}

fn input_value_by_id_or_index<'a>(
    node: &'a GraphInnerNode,
    port_values: &HashMap<(&'a str, &'a str), GraphValue>,
    port_id: &'a str,
    index: usize,
) -> Option<f64> {
    get_port_value(node, port_id, port_values)
        .and_then(AnimValue::as_f64)
        .or_else(|| {
            node.inputs
                .get(index)
                .and_then(|p| port_values.get(&(node.id.as_str(), p.id.as_str())).cloned())
                .and_then(AnimValue::as_f64)
        })
}

fn ordered_input_values<'a>(
    node: &'a GraphInnerNode,
    port_values: &HashMap<(&'a str, &'a str), GraphValue>,
    fallback_port_ids: &[&'a str],
) -> Vec<f64> {
    if !node.inputs.is_empty() {
        return node
            .inputs
            .iter()
            .map(|p| {
                port_values
                    .get(&(node.id.as_str(), p.id.as_str()))
                    .cloned()
                    .and_then(AnimValue::as_f64)
                    .unwrap_or(0.0)
            })
            .collect();
    }

    fallback_port_ids
        .iter()
        .map(|port_id| {
            get_port_value(node, port_id, port_values)
                .and_then(AnimValue::as_f64)
                .unwrap_or(0.0)
        })
        .collect()
}

fn get_port_value<'a>(
    node: &'a GraphInnerNode,
    port_id: &'a str,
    port_values: &HashMap<(&'a str, &'a str), GraphValue>,
) -> Option<GraphValue> {
    port_values.get(&(node.id.as_str(), port_id)).cloned()
}

// ---------------------------------------------------------------------------
// Topological sort
// ---------------------------------------------------------------------------

fn topological_sort(
    nodes: &[GraphInnerNode],
    connections: &[GraphConnection],
) -> Result<Vec<String>> {
    let node_ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let id_set: HashSet<&str> = node_ids.iter().copied().collect();

    // Adjacency: in-degree per node.
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut successors: HashMap<&str, Vec<&str>> = HashMap::new();
    for id in &node_ids {
        in_degree.insert(id, 0);
        successors.insert(id, Vec::new());
    }

    for c in connections {
        let from = c.from.node_id.as_str();
        let to = c.to.node_id.as_str();
        if !id_set.contains(from) || !id_set.contains(to) {
            continue; // skip dangling connections (validation catches this separately)
        }
        *in_degree.entry(to).or_insert(0) += 1;
        successors.entry(from).or_default().push(to);
    }

    let mut queue: VecDeque<&str> = VecDeque::new();
    for (&id, &deg) in &in_degree {
        if deg == 0 {
            queue.push_back(id);
        }
    }

    // Sort queue for determinism (scene order).
    let mut queue: VecDeque<&str> = {
        let mut v: Vec<&str> = queue.into_iter().collect();
        v.sort();
        v.into_iter().collect()
    };

    let mut order: Vec<String> = Vec::with_capacity(nodes.len());
    while let Some(id) = queue.pop_front() {
        order.push(id.to_string());
        if let Some(succs) = successors.get(id) {
            let mut next_ready: Vec<&str> = Vec::new();
            for &s in succs {
                if let Some(deg) = in_degree.get_mut(s) {
                    *deg -= 1;
                    if *deg == 0 {
                        next_ready.push(s);
                    }
                }
            }
            // Sort for determinism.
            next_ready.sort();
            for s in next_ready {
                queue.push_back(s);
            }
        }
    }

    if order.len() != nodes.len() {
        bail!(
            "graph_definition inner graph contains a cycle ({} of {} nodes sorted)",
            order.len(),
            nodes.len()
        );
    }

    Ok(order)
}

#[cfg(test)]
mod value_contract_tests {
    use std::collections::HashMap;

    use super::{AnimValue, GraphInputContext, resolve_builtin_value};
    use crate::state_machine::types::{MousePosition, SceneSize};

    #[test]
    fn parses_curve_types_without_collapsing_their_identity() {
        let bezier = serde_json::json!([[0, 0], [0.25, 0.1], [0.75, 0.9], [1, 1]]);
        assert!(matches!(
            AnimValue::from_json_typed(&bezier, Some("bezierCurve")),
            Some(AnimValue::BezierCurve(_))
        ));

        let normalized = serde_json::json!([0, 0.2, 0.8, 1]);
        assert!(matches!(
            AnimValue::from_json_typed(&normalized, Some("normalizedBezierCurve")),
            Some(AnimValue::NormalizedBezierCurve(_))
        ));
    }

    #[test]
    fn mouse_position_vector_is_bottom_left_scene_px() {
        let ctx = GraphInputContext {
            values: HashMap::new(),
            scene_elapsed_time: 0.0,
            local_elapsed_time: 0.0,
            mouse_position: Some(MousePosition { x: 321.0, y: 654.0 }),
            scene_size: Some(SceneSize {
                width: 1080.0,
                height: 2400.0,
            }),
            dt: 0.0,
        };

        assert_eq!(
            resolve_builtin_value("mouse.position", &ctx),
            Some(AnimValue::Vec2([321.0, 654.0]))
        );
        assert_eq!(
            resolve_builtin_value("mouse.position.y", &ctx),
            Some(AnimValue::Float(654.0))
        );
        assert_eq!(
            resolve_builtin_value("scene.size", &ctx),
            Some(AnimValue::Vec2([1080.0, 2400.0]))
        );
    }
}
