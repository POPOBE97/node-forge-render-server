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

/// A scalar value flowing through the mutation graph.
///
/// We use `f64` for all arithmetic — this matches the JSON number
/// representation and avoids premature precision loss.
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
    // Fast path: no inner nodes — output bindings can only reference
    // input bindings (pass-through wiring).
    if mutation.nodes.is_empty() && mutation.connections.is_empty() {
        return Ok(HashMap::new());
    }

    // Build node lookup.
    let nodes_by_id: HashMap<&str, &MutationInnerNode> =
        mutation.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // Topological order.
    let order = topological_sort(&mutation.nodes, &mutation.connections)?;

    // Port value store: (node_id, port_id) → value
    let mut port_values: HashMap<(&str, &str), MutationValue> = HashMap::new();

    // Seed input bindings: these deliver external values into inner-node input ports.
    for b in &mutation.input_bindings {
        let value = resolve_input_binding_value(b, ctx);
        port_values.insert((b.to.node_id.as_str(), b.to.port_id.as_str()), value);
    }

    // Also deliver connection-sourced values before evaluation — connections between
    // inner nodes will be resolved during the topo-order walk, but input bindings
    // target inner-node inputs directly.

    // Evaluate in topological order.
    for node_id in &order {
        let node = nodes_by_id.get(node_id.as_str()).unwrap();

        // Gather incoming connection values for this node's input ports.
        for conn in &mutation.connections {
            if conn.to.node_id == *node_id {
                if let Some(&val) =
                    port_values.get(&(conn.from.node_id.as_str(), conn.from.port_id.as_str()))
                {
                    port_values.insert((conn.to.node_id.as_str(), conn.to.port_id.as_str()), val);
                }
            }
        }

        evaluate_inner_node(node, &mut port_values)?;
    }

    // Resolve output bindings.
    let mut outputs: HashMap<String, MutationValue> = HashMap::new();
    for b in &mutation.output_bindings {
        let val = port_values
            .get(&(b.from.node_id.as_str(), b.from.port_id.as_str()))
            .copied()
            .unwrap_or(0.0);
        outputs.insert(b.port_id.clone(), val);
    }

    Ok(outputs)
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
}
