//! Mutation inner-graph compiler and evaluator.
//!
//! Takes a `MutationDefinition`, resolves input bindings, evaluates
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

use super::types::*;

const MUTATION_GRAPH_FRAME_BUDGET: Duration = Duration::from_millis(4);

// ---------------------------------------------------------------------------
// Typed animation value
// ---------------------------------------------------------------------------

/// A typed value flowing through the mutation graph.
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
                values[0].as_f64()?,
                values[1].as_f64()?,
                values[2].as_f64()?,
                values[3].as_f64()?,
            ])),
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
            AnimValue::Packed(values) => {
                serde_json::Value::Array(values.iter().map(AnimValue::to_json).collect())
            }
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

pub type MutationValue = AnimValue;

/// Input context supplied to mutation evaluation.
pub struct MutationInputContext {
    /// Current parameter snapshot keyed by mutation-input port id.
    pub values: HashMap<String, MutationValue>,
    /// Monotonic scene time in seconds.
    pub scene_elapsed_time: f64,
    /// Time since the current state was entered, in seconds.
    pub local_elapsed_time: f64,
    /// Latest mouse position in render-target frag pixel coordinates.
    pub mouse_position: Option<MousePosition>,
}

/// Evaluate a mutation definition given its input context.
///
/// Returns a map from mutation-output port id → computed value.
pub fn evaluate_mutation(
    mutation: &MutationDefinition,
    ctx: &MutationInputContext,
) -> Result<HashMap<String, MutationValue>> {
    let has_inner_graph = !mutation.nodes.is_empty() || !mutation.connections.is_empty();
    let has_passthroughs = !mutation.passthrough_bindings.is_empty();

    // Fast path: nothing to evaluate.
    if !has_inner_graph && !has_passthroughs {
        return Ok(HashMap::new());
    }

    let mut outputs: HashMap<String, MutationValue> = HashMap::new();

    // ── Evaluate inner graph (if any) ──────────────────────────────────
    if has_inner_graph {
        let nodes_by_id: HashMap<&str, &MutationInnerNode> =
            mutation.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        let order = topological_sort(&mutation.nodes, &mutation.connections)?;
        for node in &mutation.nodes {
            if node.node_type == MutationInnerNodeType::MutationFunction {
                super::mutation_function::prepare(&mutation.id, &node.id)?;
            }
        }
        let deadline = Instant::now() + MUTATION_GRAPH_FRAME_BUDGET;

        let mut port_values: HashMap<(&str, &str), MutationValue> = HashMap::new();

        for b in &mutation.input_bindings {
            let value = resolve_input_binding_value(b, ctx);
            port_values.insert((b.to.node_id.as_str(), b.to.port_id.as_str()), value);
        }

        for node_id in &order {
            let remaining_budget = deadline.saturating_duration_since(Instant::now());
            if remaining_budget.is_zero() {
                bail!(
                    "Mutation graph '{}' exceeded its 4ms frame budget",
                    mutation.id
                );
            }
            let node = nodes_by_id.get(node_id.as_str()).unwrap();

            for conn in &mutation.connections {
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

            evaluate_inner_node(&mutation.id, node, &mut port_values, remaining_budget)?;
            if Instant::now() >= deadline {
                bail!(
                    "Mutation graph '{}' exceeded its 4ms frame budget",
                    mutation.id
                );
            }
        }

        for b in &mutation.output_bindings {
            let val = port_values
                .get(&(b.from.node_id.as_str(), b.from.port_id.as_str()))
                .cloned()
                .unwrap_or_default();
            outputs.insert(b.port_id.clone(), val);
        }
    }

    // ── Apply passthrough bindings ─────────────────────────────────────
    // Passthroughs map an input boundary port directly to an output port.
    // They only write to output ports not already written by output bindings.
    for pt in &mutation.passthrough_bindings {
        if outputs.contains_key(&pt.to_port_id) {
            // Output already written by an output binding — skip (validation
            // catches duplicates as errors, but be defensive at runtime).
            continue;
        }
        let value = resolve_passthrough_input_value(&pt.from_port_id, mutation, ctx);
        outputs.insert(pt.to_port_id.clone(), value);
    }

    for port in &mutation.outputs {
        let Some(expected) = port.array_length else {
            continue;
        };
        let Some(value) = outputs.get(&port.id) else {
            continue;
        };
        if !matches!(value, MutationValue::Packed(values) if values.len() == expected) {
            bail!(
                "Mutation '{}' output '{}' must contain exactly {} elements",
                mutation.id,
                port.id,
                expected
            );
        }
    }

    Ok(outputs)
}

/// Resolve the value for a passthrough binding's input port.
///
/// Checks well-known built-in references first (the input port id itself
/// may be a well-known name like `"sceneElapsedTime"`), then falls back to
/// matching an input port on the mutation boundary, then the values map.
fn resolve_passthrough_input_value(
    from_port_id: &str,
    mutation: &MutationDefinition,
    ctx: &MutationInputContext,
) -> MutationValue {
    // Check well-known built-in ids.
    if let Some(value) = resolve_builtin_value(from_port_id, ctx) {
        return value;
    }

    // Check if the from_port_id matches a mutation input port and a
    // corresponding input binding.
    for b in &mutation.input_bindings {
        if b.port_id == from_port_id {
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
/// The latest mutation format uses the mutation output port id itself as
/// `"nodeId:paramName"`.
pub fn resolve_output_target(port_id: &str) -> Option<OverrideKey> {
    OverrideKey::parse(port_id)
}

pub fn expand_output_target_keys(port_id: &str) -> Vec<OverrideKey> {
    resolve_output_target(port_id).into_iter().collect()
}

pub fn expand_output_overrides(
    port_id: &str,
    value: &MutationValue,
) -> Vec<(OverrideKey, serde_json::Value)> {
    resolve_output_target(port_id)
        .map(|key| vec![(key, value.to_json())])
        .unwrap_or_default()
}

/// Collect all override target keys that a mutation can produce.
///
/// This is the single source of truth for both runtime override mapping
/// and trace tracked-key discovery.
pub fn all_output_target_keys(mutation: &MutationDefinition) -> Vec<OverrideKey> {
    let mut keys = Vec::new();
    let mut seen = HashSet::new();

    // From output bindings.
    for b in &mutation.output_bindings {
        for key in expand_output_target_keys(&b.port_id) {
            let s = format!("{}:{}", key.node_id, key.param_name);
            if seen.insert(s) {
                keys.push(key);
            }
        }
    }

    // From passthrough bindings.
    for pt in &mutation.passthrough_bindings {
        for key in expand_output_target_keys(&pt.to_port_id) {
            let s = format!("{}:{}", key.node_id, key.param_name);
            if seen.insert(s) {
                keys.push(key);
            }
        }
    }

    keys
}

// ---------------------------------------------------------------------------
// Input binding resolution
// ---------------------------------------------------------------------------

fn resolve_input_binding_value(
    binding: &MutationInputBinding,
    ctx: &MutationInputContext,
) -> MutationValue {
    if let Some(value) = resolve_builtin_value(&binding.port_id, ctx) {
        return value;
    }

    // Fall back to the current animated/root parameter snapshot.
    ctx.values
        .get(&binding.port_id)
        .cloned()
        .unwrap_or_default()
}

fn resolve_builtin_value(name: &str, ctx: &MutationInputContext) -> Option<MutationValue> {
    match name {
        "sceneElapsedTime" => Some(ctx.scene_elapsed_time.into()),
        "localElapsedTime" => Some(ctx.local_elapsed_time.into()),
        "mouse.position.x" => Some(ctx.mouse_position.map(|p| p.x).unwrap_or(0.0).into()),
        "mouse.position.y" => Some(ctx.mouse_position.map(|p| p.y).unwrap_or(0.0).into()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Inner node evaluation
// ---------------------------------------------------------------------------

fn evaluate_inner_node<'a>(
    mutation_id: &str,
    node: &'a MutationInnerNode,
    port_values: &mut HashMap<(&'a str, &'a str), MutationValue>,
    remaining_budget: Duration,
) -> Result<()> {
    match node.node_type {
        MutationInnerNodeType::FloatInput => {
            let value = node
                .params
                .get("value")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            write_output_if_declared_or_default(node, port_values, "value", value.into());
        }
        MutationInnerNodeType::PackArray => {
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
        MutationInnerNodeType::MutationFunction => {
            let input = serde_json::Value::Object(
                node.inputs
                    .iter()
                    .map(|port| {
                        (
                            port.id.clone(),
                            get_port_value(node, port.id.as_str(), port_values)
                                .unwrap_or_default()
                                .to_json(),
                        )
                    })
                    .collect(),
            );
            let result = super::mutation_function::evaluate(
                mutation_id,
                &node.id,
                &input,
                remaining_budget,
            )?;
            let object = result.as_object().ok_or_else(|| {
                anyhow::anyhow!("Mutation Function '{}' must return an object", node.id)
            })?;
            for output in &node.outputs {
                let value = object.get(&output.id).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Mutation Function '{}' omitted output '{}'",
                        node.id,
                        output.id
                    )
                })?;
                let value = AnimValue::from_json_typed(value, output.port_type.as_deref())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Mutation Function '{}.{}' returned a value incompatible with '{}'",
                            node.id,
                            output.id,
                            output.port_type.as_deref().unwrap_or("any")
                        )
                    })?;
                if let Some(expected) = output.array_length
                    && !matches!(&value, AnimValue::Packed(values) if values.len() == expected)
                {
                    bail!(
                        "Mutation Function '{}.{}' must return exactly {} elements",
                        node.id,
                        output.id,
                        expected
                    );
                }
                write_output_if_declared_or_default(node, port_values, output.id.as_str(), value);
            }
        }
        MutationInnerNodeType::MathAdd => {
            let inputs = ordered_input_values(node, port_values, &["a", "b"]);
            let result: f64 = inputs.into_iter().sum();
            write_output_if_declared_or_default(node, port_values, "result", result.into());
        }
        MutationInnerNodeType::MathSubtract => {
            let inputs = ordered_input_values(node, port_values, &[]);
            let first = inputs.first().copied().unwrap_or(0.0);
            let rest = inputs.iter().skip(1).sum::<f64>();
            write_output_if_declared_or_default(node, port_values, "result", (first - rest).into());
        }
        MutationInnerNodeType::MathMultiply => {
            let inputs = ordered_input_values(node, port_values, &[]);
            let result = if inputs.is_empty() {
                0.0
            } else {
                inputs.into_iter().fold(1.0, |acc, value| acc * value)
            };
            write_output_if_declared_or_default(node, port_values, "result", result.into());
        }
        MutationInnerNodeType::MathDivide => {
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
        MutationInnerNodeType::Lerp => {
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

fn write_output_if_declared_or_default<'a>(
    node: &'a MutationInnerNode,
    port_values: &mut HashMap<(&'a str, &'a str), MutationValue>,
    port_id: &'a str,
    value: MutationValue,
) {
    if node.outputs.is_empty() || node.outputs.iter().any(|p| p.id == port_id) {
        port_values.insert((node.id.as_str(), port_id), value);
    }
}

fn input_value_by_id_or_index<'a>(
    node: &'a MutationInnerNode,
    port_values: &HashMap<(&'a str, &'a str), MutationValue>,
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
    node: &'a MutationInnerNode,
    port_values: &HashMap<(&'a str, &'a str), MutationValue>,
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
    node: &'a MutationInnerNode,
    port_id: &'a str,
    port_values: &HashMap<(&'a str, &'a str), MutationValue>,
) -> Option<MutationValue> {
    port_values.get(&(node.id.as_str(), port_id)).cloned()
}

// ---------------------------------------------------------------------------
// Topological sort
// ---------------------------------------------------------------------------

fn topological_sort(
    nodes: &[MutationInnerNode],
    connections: &[MutationConnection],
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
            "mutation inner graph contains a cycle ({} of {} nodes sorted)",
            order.len(),
            nodes.len()
        );
    }

    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_mutation() -> MutationDefinition {
        MutationDefinition {
            id: "m1".into(),
            name: "Test".into(),
            inputs: vec![],
            outputs: vec![],
            nodes: vec![],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![],
            passthrough_bindings: vec![],
            viewport: None,
        }
    }

    fn empty_ctx() -> MutationInputContext {
        MutationInputContext {
            values: HashMap::new(),
            scene_elapsed_time: 0.0,
            local_elapsed_time: 0.0,
            mouse_position: None,
        }
    }

    fn output_f64(outputs: &HashMap<String, MutationValue>, key: &str) -> f64 {
        outputs
            .get(key)
            .cloned()
            .and_then(AnimValue::as_f64)
            .unwrap_or(f64::NAN)
    }

    fn output_packed_len(outputs: &HashMap<String, MutationValue>, key: &str) -> usize {
        match outputs.get(key).cloned() {
            Some(AnimValue::Packed(values)) => values.len(),
            other => panic!("expected packed output for {key}, got {other:?}"),
        }
    }

    #[test]
    fn empty_mutation_produces_empty_outputs() {
        let m = empty_mutation();
        let result = evaluate_mutation(&m, &empty_ctx()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn float_input_node_outputs_constant_value() {
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "float".into(),
            node_type: MutationInnerNodeType::FloatInput,
            params: [("value".into(), serde_json::json!(2.5))]
                .into_iter()
                .collect(),
            inputs: vec![],
            outputs: vec![MutationPort {
                id: "value".into(),
                name: Some("Value".into()),
                port_type: Some("float".into()),
                array_length: None,
            }],
        });
        m.output_bindings.push(MutationOutputBinding {
            port_id: "out".into(),
            from: MutationEndpoint {
                node_id: "float".into(),
                port_id: "value".into(),
            },
        });

        let result = evaluate_mutation(&m, &empty_ctx()).unwrap();
        assert!((output_f64(&result, "out") - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn pack_array_node_preserves_input_order() {
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "pack".into(),
            node_type: MutationInnerNodeType::PackArray,
            params: HashMap::new(),
            inputs: vec![
                MutationPort {
                    id: "input1".into(),
                    name: None,
                    port_type: Some("float".into()),
                    array_length: None,
                },
                MutationPort {
                    id: "input2".into(),
                    name: None,
                    port_type: Some("float".into()),
                    array_length: None,
                },
            ],
            outputs: vec![MutationPort {
                id: "packed".into(),
                name: None,
                port_type: Some("packed<float>".into()),
                array_length: None,
            }],
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pa".into(),
            to: MutationEndpoint {
                node_id: "pack".into(),
                port_id: "input1".into(),
            },
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pb".into(),
            to: MutationEndpoint {
                node_id: "pack".into(),
                port_id: "input2".into(),
            },
        });
        m.output_bindings.push(MutationOutputBinding {
            port_id: "packed_out".into(),
            from: MutationEndpoint {
                node_id: "pack".into(),
                port_id: "packed".into(),
            },
        });

        let mut ctx = empty_ctx();
        ctx.values.insert("pa".into(), 3.0.into());
        ctx.values.insert("pb".into(), 7.0.into());

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert_eq!(output_packed_len(&result, "packed_out"), 2);

        match result.get("packed_out").cloned() {
            Some(AnimValue::Packed(values)) => {
                assert_eq!(values[0], AnimValue::Float(3.0));
                assert_eq!(values[1], AnimValue::Float(7.0));
            }
            other => panic!("expected packed output, got {other:?}"),
        }
    }

    #[test]
    fn math_add_node() {
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "add".into(),
            node_type: MutationInnerNodeType::MathAdd,
            params: HashMap::new(),
            inputs: vec![
                MutationPort {
                    id: "a".into(),
                    name: None,
                    port_type: None,
                    array_length: None,
                },
                MutationPort {
                    id: "b".into(),
                    name: None,
                    port_type: None,
                    array_length: None,
                },
            ],
            outputs: vec![MutationPort {
                id: "result".into(),
                name: None,
                port_type: None,
                array_length: None,
            }],
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pa".into(),
            to: MutationEndpoint {
                node_id: "add".into(),
                port_id: "a".into(),
            },
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pb".into(),
            to: MutationEndpoint {
                node_id: "add".into(),
                port_id: "b".into(),
            },
        });
        m.output_bindings.push(MutationOutputBinding {
            port_id: "p_out".into(),
            from: MutationEndpoint {
                node_id: "add".into(),
                port_id: "result".into(),
            },
        });

        let mut ctx = empty_ctx();
        ctx.values.insert("pa".into(), 40.0.into());
        ctx.values.insert("pb".into(), 2.0.into());

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!((output_f64(&result, "p_out") - 42.0).abs() < f64::EPSILON);
    }

    #[test]
    fn math_multiply_dynamic_inputs() {
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "mul".into(),
            node_type: MutationInnerNodeType::MathMultiply,
            params: HashMap::new(),
            inputs: vec![
                MutationPort {
                    id: "dynamic_fixed_1".into(),
                    name: None,
                    port_type: None,
                    array_length: None,
                },
                MutationPort {
                    id: "dynamic_fixed_2".into(),
                    name: None,
                    port_type: None,
                    array_length: None,
                },
            ],
            outputs: vec![MutationPort {
                id: "result".into(),
                name: None,
                port_type: None,
                array_length: None,
            }],
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pa".into(),
            to: MutationEndpoint {
                node_id: "mul".into(),
                port_id: "dynamic_fixed_1".into(),
            },
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pb".into(),
            to: MutationEndpoint {
                node_id: "mul".into(),
                port_id: "dynamic_fixed_2".into(),
            },
        });
        m.output_bindings.push(MutationOutputBinding {
            port_id: "out".into(),
            from: MutationEndpoint {
                node_id: "mul".into(),
                port_id: "result".into(),
            },
        });

        let mut ctx = empty_ctx();
        ctx.values.insert("pa".into(), 3.0.into());
        ctx.values.insert("pb".into(), 7.0.into());

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!((output_f64(&result, "out") - 21.0).abs() < f64::EPSILON);
    }

    #[test]
    fn lerp_node() {
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "lerp".into(),
            node_type: MutationInnerNodeType::Lerp,
            params: HashMap::new(),
            inputs: vec![
                MutationPort {
                    id: "t".into(),
                    name: None,
                    port_type: None,
                    array_length: None,
                },
                MutationPort {
                    id: "a".into(),
                    name: None,
                    port_type: None,
                    array_length: None,
                },
                MutationPort {
                    id: "b".into(),
                    name: None,
                    port_type: None,
                    array_length: None,
                },
            ],
            outputs: vec![MutationPort {
                id: "result".into(),
                name: None,
                port_type: None,
                array_length: None,
            }],
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pa".into(),
            to: MutationEndpoint {
                node_id: "lerp".into(),
                port_id: "a".into(),
            },
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pb".into(),
            to: MutationEndpoint {
                node_id: "lerp".into(),
                port_id: "b".into(),
            },
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pt".into(),
            to: MutationEndpoint {
                node_id: "lerp".into(),
                port_id: "t".into(),
            },
        });
        m.output_bindings.push(MutationOutputBinding {
            port_id: "out".into(),
            from: MutationEndpoint {
                node_id: "lerp".into(),
                port_id: "result".into(),
            },
        });

        let mut ctx = empty_ctx();
        ctx.values.insert("pa".into(), 0.0.into());
        ctx.values.insert("pb".into(), 100.0.into());
        ctx.values.insert("pt".into(), 0.25.into());

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!((output_f64(&result, "out") - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn math_divide_by_zero_outputs_zero() {
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "div".into(),
            node_type: MutationInnerNodeType::MathDivide,
            params: HashMap::new(),
            inputs: vec![
                MutationPort {
                    id: "dynamic_fixed_1".into(),
                    name: None,
                    port_type: None,
                    array_length: None,
                },
                MutationPort {
                    id: "dynamic_fixed_2".into(),
                    name: None,
                    port_type: None,
                    array_length: None,
                },
            ],
            outputs: vec![MutationPort {
                id: "result".into(),
                name: None,
                port_type: None,
                array_length: None,
            }],
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "num".into(),
            to: MutationEndpoint {
                node_id: "div".into(),
                port_id: "dynamic_fixed_1".into(),
            },
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "den".into(),
            to: MutationEndpoint {
                node_id: "div".into(),
                port_id: "dynamic_fixed_2".into(),
            },
        });
        m.output_bindings.push(MutationOutputBinding {
            port_id: "out".into(),
            from: MutationEndpoint {
                node_id: "div".into(),
                port_id: "result".into(),
            },
        });

        let mut ctx = empty_ctx();
        ctx.values.insert("num".into(), 5.5.into());
        ctx.values.insert("den".into(), 0.0.into());

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!((output_f64(&result, "out") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn passthrough_mouse_position_outputs_latest_frag_pixel_position() {
        let mut m = empty_mutation();
        m.passthrough_bindings
            .push(super::MutationPassthroughBinding {
                from_port_id: "mouse.position.x".into(),
                to_port_id: "out_x".into(),
            });
        m.passthrough_bindings
            .push(super::MutationPassthroughBinding {
                from_port_id: "mouse.position.y".into(),
                to_port_id: "out_y".into(),
            });

        let mut ctx = empty_ctx();
        ctx.mouse_position = Some(MousePosition { x: 123.0, y: 456.0 });

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!((output_f64(&result, "out_x") - 123.0).abs() < f64::EPSILON);
        assert!((output_f64(&result, "out_y") - 456.0).abs() < f64::EPSILON);
    }

    #[test]
    fn math_subtract_dynamic_inputs() {
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "sub".into(),
            node_type: MutationInnerNodeType::MathSubtract,
            params: HashMap::new(),
            inputs: vec![
                MutationPort {
                    id: "dynamic_fixed_1".into(),
                    name: None,
                    port_type: None,
                    array_length: None,
                },
                MutationPort {
                    id: "dynamic_fixed_2".into(),
                    name: None,
                    port_type: None,
                    array_length: None,
                },
            ],
            outputs: vec![MutationPort {
                id: "result".into(),
                name: None,
                port_type: None,
                array_length: None,
            }],
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "a".into(),
            to: MutationEndpoint {
                node_id: "sub".into(),
                port_id: "dynamic_fixed_1".into(),
            },
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "b".into(),
            to: MutationEndpoint {
                node_id: "sub".into(),
                port_id: "dynamic_fixed_2".into(),
            },
        });
        m.output_bindings.push(MutationOutputBinding {
            port_id: "out".into(),
            from: MutationEndpoint {
                node_id: "sub".into(),
                port_id: "result".into(),
            },
        });

        let mut ctx = empty_ctx();
        ctx.values.insert("a".into(), 88.0.into());
        ctx.values.insert("b".into(), 9.0.into());

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!((output_f64(&result, "out") - 79.0).abs() < f64::EPSILON);
    }

    // ── Passthrough binding tests ──────────────────────────────────────

    #[test]
    fn passthrough_scene_elapsed_time() {
        let mut m = empty_mutation();
        // Add an output port matching the passthrough target.
        m.outputs.push(super::MutationPort {
            id: "FloatInput_53:value".into(),
            name: Some("uTime.value".into()),
            port_type: Some("float".into()),
            array_length: None,
        });
        m.passthrough_bindings
            .push(super::MutationPassthroughBinding {
                from_port_id: "sceneElapsedTime".into(),
                to_port_id: "FloatInput_53:value".into(),
            });

        let mut ctx = empty_ctx();
        ctx.scene_elapsed_time = 3.14;

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!(
            (output_f64(&result, "FloatInput_53:value") - 3.14).abs() < f64::EPSILON,
            "passthrough should wire sceneElapsedTime → output"
        );
    }

    #[test]
    fn inner_graph_input_binding_prefers_scene_elapsed_time_builtin() {
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "add".into(),
            node_type: MutationInnerNodeType::MathAdd,
            params: HashMap::new(),
            inputs: vec![
                MutationPort {
                    id: "a".into(),
                    name: None,
                    port_type: Some("float".into()),
                    array_length: None,
                },
                MutationPort {
                    id: "b".into(),
                    name: None,
                    port_type: Some("float".into()),
                    array_length: None,
                },
            ],
            outputs: vec![MutationPort {
                id: "result".into(),
                name: None,
                port_type: Some("float".into()),
                array_length: None,
            }],
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "sceneElapsedTime".into(),
            to: MutationEndpoint {
                node_id: "add".into(),
                port_id: "a".into(),
            },
        });
        m.output_bindings.push(MutationOutputBinding {
            port_id: "Time:value".into(),
            from: MutationEndpoint {
                node_id: "add".into(),
                port_id: "result".into(),
            },
        });

        let mut ctx = empty_ctx();
        ctx.scene_elapsed_time = 1.25;
        ctx.values
            .insert("sceneElapsedTime".into(), MutationValue::from(99.0));

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!(
            (output_f64(&result, "Time:value") - 1.25).abs() < f64::EPSILON,
            "built-in scene time must reach inner graph inputs before external values"
        );
    }

    #[test]
    fn passthrough_local_elapsed_time() {
        let mut m = empty_mutation();
        m.passthrough_bindings
            .push(super::MutationPassthroughBinding {
                from_port_id: "localElapsedTime".into(),
                to_port_id: "Foo_1:value".into(),
            });

        let mut ctx = empty_ctx();
        ctx.local_elapsed_time = 7.77;

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!(
            (output_f64(&result, "Foo_1:value") - 7.77).abs() < f64::EPSILON,
            "passthrough should wire localElapsedTime → output"
        );
    }

    #[test]
    fn passthrough_from_input_binding() {
        let mut m = empty_mutation();
        // Add an input port with a binding that maps to an input
        m.inputs.push(super::MutationPort {
            id: "ColorInput_7:value".into(),
            name: Some("Color Input.value".into()),
            port_type: Some("float".into()),
            array_length: None,
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "ColorInput_7:value".into(),
            to: MutationEndpoint {
                node_id: "unused_node".into(),
                port_id: "unused_port".into(),
            },
        });
        m.passthrough_bindings
            .push(super::MutationPassthroughBinding {
                from_port_id: "ColorInput_7:value".into(),
                to_port_id: "Out_1:value".into(),
            });

        let mut ctx = empty_ctx();
        ctx.values.insert("ColorInput_7:value".into(), 99.0.into());

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!(
            (output_f64(&result, "Out_1:value") - 99.0).abs() < f64::EPSILON,
            "passthrough should resolve through input binding"
        );
    }

    #[test]
    fn passthrough_skipped_when_output_binding_exists() {
        // If an output binding writes to the same port, the passthrough
        // should be skipped (output binding wins).
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "float".into(),
            node_type: MutationInnerNodeType::FloatInput,
            params: [("value".into(), serde_json::json!(42.0))]
                .into_iter()
                .collect(),
            inputs: vec![],
            outputs: vec![MutationPort {
                id: "value".into(),
                name: None,
                port_type: None,
                array_length: None,
            }],
        });
        // Output binding writes to "Result:value" via inner graph.
        m.output_bindings.push(MutationOutputBinding {
            port_id: "Result:value".into(),
            from: MutationEndpoint {
                node_id: "float".into(),
                port_id: "value".into(),
            },
        });
        // Passthrough also targets the same port — should be skipped.
        m.passthrough_bindings
            .push(super::MutationPassthroughBinding {
                from_port_id: "sceneElapsedTime".into(),
                to_port_id: "Result:value".into(),
            });

        let mut ctx = empty_ctx();
        ctx.scene_elapsed_time = 999.0;

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!(
            (output_f64(&result, "Result:value") - 42.0).abs() < f64::EPSILON,
            "output binding should win over passthrough"
        );
    }

    #[test]
    fn all_output_target_keys_includes_passthroughs() {
        let mut m = empty_mutation();
        m.output_bindings.push(MutationOutputBinding {
            port_id: "A:x".into(),
            from: MutationEndpoint {
                node_id: "n".into(),
                port_id: "o".into(),
            },
        });
        m.passthrough_bindings
            .push(super::MutationPassthroughBinding {
                from_port_id: "sceneElapsedTime".into(),
                to_port_id: "B:y".into(),
            });

        let keys = all_output_target_keys(&m);
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].node_id, "A");
        assert_eq!(keys[0].param_name, "x");
        assert_eq!(keys[1].node_id, "B");
        assert_eq!(keys[1].param_name, "y");
    }
}
