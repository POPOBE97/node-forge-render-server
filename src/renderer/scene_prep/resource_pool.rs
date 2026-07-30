use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow, bail};

use crate::{
    dsl::{Endpoint, Node, SceneDSL, incoming_connection},
    renderer::pass_source::resolve_pass_source_ref,
};

/// Project pass-typed ResourcePool outputs to the currently selected input.
///
/// This runs before reachability pruning so inactive pass branches never enter
/// the active render plan. The unprojected expanded scene is retained
/// separately by scene preparation for Matrix variants.
pub(super) fn project_selected_pass_resource_pools(
    scene: &mut SceneDSL,
    render_target_id: &str,
) -> Result<usize> {
    let nodes_by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|node| (node.id.clone(), node))
        .collect();
    let mut visited_nodes = HashSet::new();
    let mut resolved_by_pool = HashMap::new();
    let mut rewired = 0usize;
    project_upstream_from_node(
        scene,
        &nodes_by_id,
        render_target_id,
        &mut visited_nodes,
        &mut resolved_by_pool,
        &mut rewired,
    )?;

    for node in &mut scene.nodes {
        for binding in &mut node.input_bindings {
            let Some(source_binding) = binding.source_binding.as_mut() else {
                continue;
            };
            if source_binding.output_port_id != "output" {
                continue;
            }
            let Some(endpoint) = resolved_by_pool.get(&source_binding.node_id) else {
                continue;
            };
            source_binding.node_id = endpoint.node_id.clone();
            source_binding.output_port_id = endpoint.port_id.clone();
        }
    }

    Ok(rewired)
}

fn project_upstream_from_node(
    scene: &mut SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    visited_nodes: &mut HashSet<String>,
    resolved_by_pool: &mut HashMap<String, Endpoint>,
    rewired: &mut usize,
) -> Result<()> {
    if !visited_nodes.insert(node_id.to_string()) {
        return Ok(());
    }

    let incoming: Vec<usize> = scene
        .connections
        .iter()
        .enumerate()
        .filter(|(_, connection)| connection.to.node_id == node_id)
        .map(|(index, _)| index)
        .collect();
    for connection_index in incoming {
        let source = scene.connections[connection_index].from.clone();
        let source_is_pass_pool = source.port_id == "output"
            && nodes_by_id
                .get(&source.node_id)
                .is_some_and(is_pass_resource_pool);
        let active_source = if source_is_pass_pool {
            let mut visiting = HashSet::new();
            let endpoint = resolve_selected_pass_endpoint(
                scene,
                nodes_by_id,
                &source.node_id,
                &mut visiting,
                resolved_by_pool,
            )?;
            // Validate the final endpoint against the canonical pass-source rules.
            resolve_pass_source_ref(scene, nodes_by_id, &endpoint).map_err(|error| {
                anyhow!(
                    "pass ResourcePool '{}' selected invalid source '{}.{}': {error}",
                    source.node_id,
                    endpoint.node_id,
                    endpoint.port_id
                )
            })?;
            scene.connections[connection_index].from = endpoint.clone();
            *rewired += 1;
            endpoint
        } else {
            source
        };

        project_upstream_from_node(
            scene,
            nodes_by_id,
            &active_source.node_id,
            visited_nodes,
            resolved_by_pool,
            rewired,
        )?;
    }

    Ok(())
}

fn is_pass_resource_pool(node: &Node) -> bool {
    node.node_type == "ResourcePool"
        && node
            .outputs
            .iter()
            .any(|output| output.id == "output" && output.port_type.as_deref() == Some("pass"))
}

fn resolve_selected_pass_endpoint(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    pool_id: &str,
    visiting: &mut HashSet<String>,
    cache: &mut HashMap<String, Endpoint>,
) -> Result<Endpoint> {
    if let Some(endpoint) = cache.get(pool_id) {
        return Ok(endpoint.clone());
    }
    if !visiting.insert(pool_id.to_string()) {
        bail!("cycle detected while resolving pass ResourcePool '{pool_id}'");
    }

    let pool = nodes_by_id
        .get(pool_id)
        .ok_or_else(|| anyhow!("pass ResourcePool node '{pool_id}' does not exist"))?;
    if !is_pass_resource_pool(pool) {
        bail!("node '{pool_id}' is not a pass-typed ResourcePool");
    }

    let dynamic_inputs: Vec<&str> = pool
        .inputs
        .iter()
        .filter(|port| port.id != "selectedIndex")
        .map(|port| port.id.as_str())
        .collect();
    if dynamic_inputs.is_empty() {
        bail!("pass ResourcePool '{pool_id}' has no selectable inputs");
    }

    let selected_index =
        crate::dsl::resolve_input_i64(scene, nodes_by_id, pool_id, "selectedIndex")?
            .unwrap_or(0)
            .max(0) as usize;
    let selected_index = selected_index.min(dynamic_inputs.len() - 1);
    let selected_port = dynamic_inputs[selected_index];
    let selected_connection =
        incoming_connection(scene, pool_id, selected_port).ok_or_else(|| {
            anyhow!(
                "pass ResourcePool '{pool_id}' selected input '{selected_port}' at index \
{selected_index}, but that input is not connected"
            )
        })?;

    let endpoint = if nodes_by_id
        .get(&selected_connection.from.node_id)
        .is_some_and(is_pass_resource_pool)
        && selected_connection.from.port_id == "output"
    {
        resolve_selected_pass_endpoint(
            scene,
            nodes_by_id,
            &selected_connection.from.node_id,
            visiting,
            cache,
        )?
    } else {
        selected_connection.from.clone()
    };

    visiting.remove(pool_id);
    cache.insert(pool_id.to_string(), endpoint.clone());
    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use crate::dsl::{Connection, Endpoint, Metadata, Node, NodePort, SceneDSL};

    use super::project_selected_pass_resource_pools;

    fn node(id: &str, node_type: &str) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_bindings: Vec::new(),
            wgsl_override: None,
        }
    }

    fn pass_pool(id: &str, selected_index: i64, input_count: usize) -> Node {
        let mut pool = node(id, "ResourcePool");
        pool.params
            .insert("selectedIndex".to_string(), json!(selected_index));
        pool.inputs = (0..input_count)
            .map(|index| NodePort {
                id: format!("input_{index}"),
                name: Some(format!("Input {index}")),
                port_type: Some("pass".to_string()),
                array_length: None,
            })
            .collect();
        pool.outputs.push(NodePort {
            id: "output".to_string(),
            name: Some("Output".to_string()),
            port_type: Some("pass".to_string()),
            array_length: None,
        });
        pool
    }

    fn edge(
        id: &str,
        from_node: &str,
        from_port: &str,
        to_node: &str,
        to_port: &str,
    ) -> Connection {
        Connection {
            id: id.to_string(),
            from: Endpoint {
                node_id: from_node.to_string(),
                port_id: from_port.to_string(),
            },
            to: Endpoint {
                node_id: to_node.to_string(),
                port_id: to_port.to_string(),
            },
        }
    }

    fn scene(nodes: Vec<Node>, connections: Vec<Connection>) -> SceneDSL {
        SceneDSL {
            version: "4.0".to_string(),
            metadata: Metadata {
                name: "pass pool projection".to_string(),
                created: None,
                modified: None,
            },
            nodes,
            connections,
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        }
    }

    #[test]
    fn projects_only_the_selected_pass_and_clamps_indices() {
        for (selected_index, expected) in
            [(-4, "pass_a"), (0, "pass_a"), (1, "pass_b"), (9, "pass_b")]
        {
            let mut scene = scene(
                vec![
                    node("pass_a", "RenderPass"),
                    node("pass_b", "RenderPass"),
                    pass_pool("pool", selected_index, 2),
                    node("composite", "Composite"),
                ],
                vec![
                    edge("a-pool", "pass_a", "pass", "pool", "input_0"),
                    edge("b-pool", "pass_b", "pass", "pool", "input_1"),
                    edge("pool-out", "pool", "output", "composite", "pass"),
                ],
            );

            assert_eq!(
                project_selected_pass_resource_pools(&mut scene, "composite").unwrap(),
                1
            );
            let output = scene
                .connections
                .iter()
                .find(|connection| connection.id == "pool-out")
                .unwrap();
            assert_eq!(output.from.node_id, expected);
            assert_eq!(output.from.port_id, "pass");
        }
    }

    #[test]
    fn resolves_nested_pass_pools() {
        let mut scene = scene(
            vec![
                node("pass_a", "RenderPass"),
                node("pass_b", "RenderPass"),
                pass_pool("inner", 1, 2),
                pass_pool("outer", 0, 1),
                node("composite", "Composite"),
            ],
            vec![
                edge("a-inner", "pass_a", "pass", "inner", "input_0"),
                edge("b-inner", "pass_b", "pass", "inner", "input_1"),
                edge("inner-outer", "inner", "output", "outer", "input_0"),
                edge("outer-out", "outer", "output", "composite", "pass"),
            ],
        );

        project_selected_pass_resource_pools(&mut scene, "composite").unwrap();

        let output = scene
            .connections
            .iter()
            .find(|connection| connection.id == "outer-out")
            .unwrap();
        assert_eq!(output.from.node_id, "pass_b");
    }

    #[test]
    fn rejects_empty_unconnected_and_cyclic_pass_pools() {
        let mut empty = scene(
            vec![pass_pool("empty", 0, 0), node("composite", "Composite")],
            vec![edge("empty-out", "empty", "output", "composite", "pass")],
        );
        assert!(
            project_selected_pass_resource_pools(&mut empty, "composite")
                .unwrap_err()
                .to_string()
                .contains("no selectable inputs")
        );

        let mut unconnected = scene(
            vec![pass_pool("pool", 0, 1), node("composite", "Composite")],
            vec![edge("pool-out", "pool", "output", "composite", "pass")],
        );
        assert!(
            project_selected_pass_resource_pools(&mut unconnected, "composite")
                .unwrap_err()
                .to_string()
                .contains("not connected")
        );

        let mut cyclic = scene(
            vec![
                pass_pool("a", 0, 1),
                pass_pool("b", 0, 1),
                node("composite", "Composite"),
            ],
            vec![
                edge("a-b", "a", "output", "b", "input_0"),
                edge("b-a", "b", "output", "a", "input_0"),
                edge("a-out", "a", "output", "composite", "pass"),
            ],
        );
        assert!(
            project_selected_pass_resource_pools(&mut cyclic, "composite")
                .unwrap_err()
                .to_string()
                .contains("cycle detected")
        );

        let mut non_pass = scene(
            vec![
                node("value", "FloatInput"),
                pass_pool("pool", 0, 1),
                node("composite", "Composite"),
            ],
            vec![
                edge("value-pool", "value", "value", "pool", "input_0"),
                edge("pool-out", "pool", "output", "composite", "pass"),
            ],
        );
        assert!(
            project_selected_pass_resource_pools(&mut non_pass, "composite")
                .unwrap_err()
                .to_string()
                .contains("must resolve to a pass producer")
        );
    }

    #[test]
    fn ignores_invalid_pass_pools_outside_the_active_branch() {
        let mut scene = scene(
            vec![
                node("active_pass", "RenderPass"),
                pass_pool("active_pool", 0, 2),
                pass_pool("inactive_broken_pool", 0, 1),
                node("composite", "Composite"),
            ],
            vec![
                edge(
                    "active-input",
                    "active_pass",
                    "pass",
                    "active_pool",
                    "input_0",
                ),
                edge(
                    "active-output",
                    "active_pool",
                    "output",
                    "composite",
                    "pass",
                ),
                edge(
                    "inactive-option",
                    "inactive_broken_pool",
                    "output",
                    "active_pool",
                    "input_1",
                ),
            ],
        );

        assert_eq!(
            project_selected_pass_resource_pools(&mut scene, "composite").unwrap(),
            1
        );
    }
}
