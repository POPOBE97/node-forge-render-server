//! Mutation inner-graph compiler and evaluator.
//!
//! Takes a `MutationDefinition`, resolves input bindings, evaluates
//! the inner-node DAG in topological order, and produces output values
//! via output bindings.
//!
//! Supported inner-node types (v1):
//! - `smPassThrough` — forwards its single input unchanged.
//! - `smMathOp`      — `add`, `sub`, `mul`, `div` on two inputs.
//! - `smLerp`        — `mix(a, b, t)`.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Result, bail};

use super::types::*;

// ---------------------------------------------------------------------------
// Typed animation value (v1 — Float only, stub for future expansion)
// ---------------------------------------------------------------------------

/// A typed value flowing through the mutation graph.
///
/// v1 supports only `Float`.  Future expansions: `Int`, `Bool`,
/// `Vec2`, `Vec3`, `Vec4`, `Color`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimValue {
    Float(f64),
}

impl AnimValue {
    /// Extract as `f64`, converting if possible.
    pub fn as_f64(self) -> f64 {
        match self {
            AnimValue::Float(v) => v,
        }
    }

    /// Convert to `serde_json::Value` for the override boundary.
    pub fn to_json(self) -> serde_json::Value {
        match self {
            AnimValue::Float(v) => serde_json::json!(v),
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

/// Legacy alias kept for backward compatibility with internal callers.
pub type MutationValue = f64;

/// Input context supplied to mutation evaluation.
pub struct MutationInputContext {
    /// Current parameter snapshot keyed by mutation-input port id.
    pub values: HashMap<String, MutationValue>,
    /// Monotonic scene time in seconds.
    pub scene_elapsed_time: f64,
    /// Time since the current state was entered, in seconds.
    pub local_elapsed_time: f64,
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

        let mut port_values: HashMap<(&str, &str), MutationValue> = HashMap::new();

        for b in &mutation.input_bindings {
            let value = resolve_input_binding_value(b, ctx);
            port_values.insert((b.to.node_id.as_str(), b.to.port_id.as_str()), value);
        }

        for node_id in &order {
            let node = nodes_by_id.get(node_id.as_str()).unwrap();

            for conn in &mutation.connections {
                if conn.to.node_id == *node_id {
                    if let Some(&val) =
                        port_values.get(&(conn.from.node_id.as_str(), conn.from.port_id.as_str()))
                    {
                        port_values.insert(
                            (conn.to.node_id.as_str(), conn.to.port_id.as_str()),
                            val,
                        );
                    }
                }
            }

            evaluate_inner_node(node, &mut port_values)?;
        }

        for b in &mutation.output_bindings {
            let val = port_values
                .get(&(b.from.node_id.as_str(), b.from.port_id.as_str()))
                .copied()
                .unwrap_or(0.0);
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
    match from_port_id {
        "sceneElapsedTime" => return ctx.scene_elapsed_time,
        "localElapsedTime" => return ctx.local_elapsed_time,
        _ => {}
    }

    // Check if the from_port_id matches a mutation input port and there's
    // a corresponding input binding with a source_ref.
    for b in &mutation.input_bindings {
        if b.port_id == from_port_id {
            return resolve_input_binding_value(b, ctx);
        }
    }

    // Fall back to the values map.
    ctx.values.get(from_port_id).copied().unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// Unified target resolution
// ---------------------------------------------------------------------------

/// Resolve the override target for an output port id.
///
/// Rules (in priority order):
/// 1. If `target_ref` is `Some`, use it.
/// 2. Otherwise, try to parse `port_id` itself as `"nodeId:paramName"`.
///
/// Returns `None` if neither produces a valid `OverrideKey`.
pub fn resolve_output_target(port_id: &str, target_ref: Option<&str>) -> Option<OverrideKey> {
    if let Some(tr) = target_ref {
        return OverrideKey::parse(tr);
    }
    OverrideKey::parse(port_id)
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
        if let Some(key) = resolve_output_target(&b.port_id, b.target_ref.as_deref()) {
            let s = format!("{}:{}", key.node_id, key.param_name);
            if seen.insert(s) {
                keys.push(key);
            }
        }
    }

    // From passthrough bindings.
    for pt in &mutation.passthrough_bindings {
        if let Some(key) = resolve_output_target(&pt.to_port_id, None) {
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
    // Check well-known time inputs.
    if let Some(ref source_ref) = binding.source_ref {
        if source_ref == "sceneElapsedTime" {
            return ctx.scene_elapsed_time;
        }
        if source_ref == "localElapsedTime" {
            return ctx.local_elapsed_time;
        }
    }

    // Look up by port id in the provided values map.
    ctx.values.get(&binding.port_id).copied().unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// Inner node evaluation
// ---------------------------------------------------------------------------

fn evaluate_inner_node<'a>(
    node: &'a MutationInnerNode,
    port_values: &mut HashMap<(&'a str, &'a str), MutationValue>,
) -> Result<()> {
    match node.node_type {
        MutationInnerNodeType::SmPassThrough => {
            // Forward first input port to first output port.
            let in_val = first_input_value(node, port_values);
            if let Some(out_port) = node.outputs.first() {
                port_values.insert((node.id.as_str(), out_port.id.as_str()), in_val);
            }
        }
        MutationInnerNodeType::SmMathOp => {
            let op = node
                .params
                .get("op")
                .and_then(|v| v.as_str())
                .unwrap_or("add");

            let a = get_port_value(node, "a", port_values)
                .or_else(|| nth_input_value(node, 0, port_values))
                .unwrap_or(0.0);
            let b = get_port_value(node, "b", port_values)
                .or_else(|| nth_input_value(node, 1, port_values))
                .unwrap_or(0.0);

            let result = match op {
                "add" => a + b,
                "sub" => a - b,
                "mul" => a * b,
                "div" => {
                    if b.abs() < f64::EPSILON {
                        0.0
                    } else {
                        a / b
                    }
                }
                other => {
                    bail!(
                        "mutation inner node '{}': unknown smMathOp op '{other}'",
                        node.id
                    );
                }
            };

            let out_port = node
                .outputs
                .first()
                .map(|p| p.id.as_str())
                .unwrap_or("result");
            port_values.insert((node.id.as_str(), out_port), result);
        }
        MutationInnerNodeType::SmLerp => {
            let a = get_port_value(node, "a", port_values)
                .or_else(|| nth_input_value(node, 0, port_values))
                .unwrap_or(0.0);
            let b = get_port_value(node, "b", port_values)
                .or_else(|| nth_input_value(node, 1, port_values))
                .unwrap_or(1.0);
            let t = get_port_value(node, "t", port_values)
                .or_else(|| nth_input_value(node, 2, port_values))
                .unwrap_or(0.5);

            let result = a + (b - a) * t.clamp(0.0, 1.0);

            let out_port = node
                .outputs
                .first()
                .map(|p| p.id.as_str())
                .unwrap_or("result");
            port_values.insert((node.id.as_str(), out_port), result);
        }
    }
    Ok(())
}

fn first_input_value<'a>(
    node: &'a MutationInnerNode,
    port_values: &HashMap<(&'a str, &'a str), MutationValue>,
) -> MutationValue {
    node.inputs
        .first()
        .and_then(|p| port_values.get(&(node.id.as_str(), p.id.as_str())).copied())
        .unwrap_or(0.0)
}

fn nth_input_value<'a>(
    node: &'a MutationInnerNode,
    index: usize,
    port_values: &HashMap<(&'a str, &'a str), MutationValue>,
) -> Option<MutationValue> {
    node.inputs
        .get(index)
        .and_then(|p| port_values.get(&(node.id.as_str(), p.id.as_str())).copied())
}

fn get_port_value<'a>(
    node: &'a MutationInnerNode,
    port_id: &'a str,
    port_values: &HashMap<(&'a str, &'a str), MutationValue>,
) -> Option<MutationValue> {
    port_values.get(&(node.id.as_str(), port_id)).copied()
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
        }
    }

    #[test]
    fn empty_mutation_produces_empty_outputs() {
        let m = empty_mutation();
        let result = evaluate_mutation(&m, &empty_ctx()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn pass_through_node() {
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "n1".into(),
            node_type: MutationInnerNodeType::SmPassThrough,
            params: HashMap::new(),
            inputs: vec![MutationPort {
                id: "in".into(),
                name: None,
                port_type: None,
            }],
            outputs: vec![MutationPort {
                id: "out".into(),
                name: None,
                port_type: None,
            }],
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "p_in".into(),
            to: MutationEndpoint {
                node_id: "n1".into(),
                port_id: "in".into(),
            },
            source_ref: None,
        });
        m.output_bindings.push(MutationOutputBinding {
            port_id: "p_out".into(),
            from: MutationEndpoint {
                node_id: "n1".into(),
                port_id: "out".into(),
            },
            target_ref: None,
        });

        let mut ctx = empty_ctx();
        ctx.values.insert("p_in".into(), 42.0);

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!((result["p_out"] - 42.0).abs() < f64::EPSILON);
    }

    #[test]
    fn math_op_add() {
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "add".into(),
            node_type: MutationInnerNodeType::SmMathOp,
            params: [("op".into(), serde_json::json!("add"))]
                .into_iter()
                .collect(),
            inputs: vec![
                MutationPort {
                    id: "a".into(),
                    name: None,
                    port_type: None,
                },
                MutationPort {
                    id: "b".into(),
                    name: None,
                    port_type: None,
                },
            ],
            outputs: vec![MutationPort {
                id: "result".into(),
                name: None,
                port_type: None,
            }],
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pa".into(),
            to: MutationEndpoint {
                node_id: "add".into(),
                port_id: "a".into(),
            },
            source_ref: None,
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pb".into(),
            to: MutationEndpoint {
                node_id: "add".into(),
                port_id: "b".into(),
            },
            source_ref: None,
        });
        m.output_bindings.push(MutationOutputBinding {
            port_id: "out".into(),
            from: MutationEndpoint {
                node_id: "add".into(),
                port_id: "result".into(),
            },
            target_ref: None,
        });

        let mut ctx = empty_ctx();
        ctx.values.insert("pa".into(), 3.0);
        ctx.values.insert("pb".into(), 7.0);

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!((result["out"] - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn lerp_node() {
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "lerp".into(),
            node_type: MutationInnerNodeType::SmLerp,
            params: HashMap::new(),
            inputs: vec![
                MutationPort {
                    id: "a".into(),
                    name: None,
                    port_type: None,
                },
                MutationPort {
                    id: "b".into(),
                    name: None,
                    port_type: None,
                },
                MutationPort {
                    id: "t".into(),
                    name: None,
                    port_type: None,
                },
            ],
            outputs: vec![MutationPort {
                id: "result".into(),
                name: None,
                port_type: None,
            }],
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pa".into(),
            to: MutationEndpoint {
                node_id: "lerp".into(),
                port_id: "a".into(),
            },
            source_ref: None,
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pb".into(),
            to: MutationEndpoint {
                node_id: "lerp".into(),
                port_id: "b".into(),
            },
            source_ref: None,
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "pt".into(),
            to: MutationEndpoint {
                node_id: "lerp".into(),
                port_id: "t".into(),
            },
            source_ref: None,
        });
        m.output_bindings.push(MutationOutputBinding {
            port_id: "out".into(),
            from: MutationEndpoint {
                node_id: "lerp".into(),
                port_id: "result".into(),
            },
            target_ref: None,
        });

        let mut ctx = empty_ctx();
        ctx.values.insert("pa".into(), 0.0);
        ctx.values.insert("pb".into(), 100.0);
        ctx.values.insert("pt".into(), 0.25);

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!((result["out"] - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn scene_elapsed_time_input() {
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "pt".into(),
            node_type: MutationInnerNodeType::SmPassThrough,
            params: HashMap::new(),
            inputs: vec![MutationPort {
                id: "in".into(),
                name: None,
                port_type: None,
            }],
            outputs: vec![MutationPort {
                id: "out".into(),
                name: None,
                port_type: None,
            }],
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "time_in".into(),
            to: MutationEndpoint {
                node_id: "pt".into(),
                port_id: "in".into(),
            },
            source_ref: Some("sceneElapsedTime".into()),
        });
        m.output_bindings.push(MutationOutputBinding {
            port_id: "time_out".into(),
            from: MutationEndpoint {
                node_id: "pt".into(),
                port_id: "out".into(),
            },
            target_ref: None,
        });

        let mut ctx = empty_ctx();
        ctx.scene_elapsed_time = 5.5;

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!((result["time_out"] - 5.5).abs() < f64::EPSILON);
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
            (result["FloatInput_53:value"] - 3.14).abs() < f64::EPSILON,
            "passthrough should wire sceneElapsedTime → output"
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
            (result["Foo_1:value"] - 7.77).abs() < f64::EPSILON,
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
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "ColorInput_7:value".into(),
            to: MutationEndpoint {
                node_id: "unused_node".into(),
                port_id: "unused_port".into(),
            },
            source_ref: None,
        });
        m.passthrough_bindings
            .push(super::MutationPassthroughBinding {
                from_port_id: "ColorInput_7:value".into(),
                to_port_id: "Out_1:value".into(),
            });

        let mut ctx = empty_ctx();
        ctx.values.insert("ColorInput_7:value".into(), 99.0);

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!(
            (result["Out_1:value"] - 99.0).abs() < f64::EPSILON,
            "passthrough should resolve through input binding"
        );
    }

    #[test]
    fn passthrough_skipped_when_output_binding_exists() {
        // If an output binding writes to the same port, the passthrough
        // should be skipped (output binding wins).
        let mut m = empty_mutation();
        m.nodes.push(MutationInnerNode {
            id: "pt".into(),
            node_type: MutationInnerNodeType::SmPassThrough,
            params: HashMap::new(),
            inputs: vec![MutationPort {
                id: "in".into(),
                name: None,
                port_type: None,
            }],
            outputs: vec![MutationPort {
                id: "out".into(),
                name: None,
                port_type: None,
            }],
        });
        m.input_bindings.push(MutationInputBinding {
            port_id: "graph_in".into(),
            to: MutationEndpoint {
                node_id: "pt".into(),
                port_id: "in".into(),
            },
            source_ref: None,
        });
        // Output binding writes to "Result:value" via inner graph.
        m.output_bindings.push(MutationOutputBinding {
            port_id: "Result:value".into(),
            from: MutationEndpoint {
                node_id: "pt".into(),
                port_id: "out".into(),
            },
            target_ref: None,
        });
        // Passthrough also targets the same port — should be skipped.
        m.passthrough_bindings
            .push(super::MutationPassthroughBinding {
                from_port_id: "sceneElapsedTime".into(),
                to_port_id: "Result:value".into(),
            });

        let mut ctx = empty_ctx();
        ctx.values.insert("graph_in".into(), 42.0);
        ctx.scene_elapsed_time = 999.0;

        let result = evaluate_mutation(&m, &ctx).unwrap();
        assert!(
            (result["Result:value"] - 42.0).abs() < f64::EPSILON,
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
            target_ref: None,
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
