use std::collections::HashMap;

use anyhow::{Result, bail};

use crate::dsl::{Node, SceneDSL, find_node, incoming_connection};

/// Determine the draw order for composite layers.
pub fn composite_layers_in_draw_order(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    composite_node_id: &str,
) -> Result<Vec<String>> {
    let composite_node = find_node(nodes_by_id, composite_node_id)?;
    if composite_node.node_type != "Composite" {
        bail!("expected Composite node, got {}", composite_node.node_type);
    }

    let mut layers: Vec<String> = Vec::new();

    // Static layer
    if let Some(conn) = incoming_connection(scene, composite_node_id, "pass") {
        layers.push(conn.from.node_id.clone());
    }

    // Dynamic layers (sorted by parameter index or port name)
    let port_order: HashMap<&str, usize> = composite_node
        .inputs
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), i))
        .collect();

    let mut dynamic: Vec<(String, String)> = Vec::new();
    for conn in &scene.connections {
        if conn.to.node_id == composite_node_id && conn.to.port_id.starts_with("dynamic_") {
            dynamic.push((conn.to.port_id.clone(), conn.from.node_id.clone()));
        }
    }
    dynamic.sort_by(|a, b| {
        let a_idx = port_order.get(a.0.as_str()).copied().unwrap_or(usize::MAX);
        let b_idx = port_order.get(b.0.as_str()).copied().unwrap_or(usize::MAX);
        a_idx.cmp(&b_idx).then_with(|| a.0.cmp(&b.0))
    });

    for (_, node_id) in dynamic {
        layers.push(node_id);
    }

    Ok(layers)
}

/// Build draw-order layer lists for every Composite node in the scene.
pub fn composition_layers_by_id(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
) -> Result<HashMap<String, Vec<String>>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut ids: Vec<String> = nodes_by_id
        .values()
        .filter(|n| n.node_type == "Composite")
        .map(|n| n.id.clone())
        .collect();
    ids.sort();

    for id in ids {
        out.insert(
            id.clone(),
            composite_layers_in_draw_order(scene, nodes_by_id, &id)?,
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::dsl::{Connection, Endpoint, Metadata, Node, NodePort, SceneDSL};

    use super::composite_layers_in_draw_order;

    #[test]
    fn composite_draw_order_is_pass_then_dynamic_indices() {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: Metadata {
                name: "test".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                Node {
                    id: "out".to_string(),
                    node_type: "Composite".to_string(),
                    params: HashMap::new(),
                    inputs: vec![
                        NodePort {
                            id: "dynamic_1".to_string(),
                            name: Some("image2".to_string()),
                            port_type: Some("color".to_string()),
                        },
                        NodePort {
                            id: "dynamic_0".to_string(),
                            name: Some("image1".to_string()),
                            port_type: Some("color".to_string()),
                        },
                    ],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                Node {
                    id: "p_img".to_string(),
                    node_type: "RenderPass".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                Node {
                    id: "p0".to_string(),
                    node_type: "RenderPass".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                Node {
                    id: "p1".to_string(),
                    node_type: "RenderPass".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
            ],
            connections: vec![
                Connection {
                    id: "c_img".to_string(),
                    from: Endpoint {
                        node_id: "p_img".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "out".to_string(),
                        port_id: "pass".to_string(),
                    },
                },
                Connection {
                    id: "c_dyn1".to_string(),
                    from: Endpoint {
                        node_id: "p1".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "out".to_string(),
                        port_id: "dynamic_1".to_string(),
                    },
                },
                Connection {
                    id: "c_dyn0".to_string(),
                    from: Endpoint {
                        node_id: "p0".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "out".to_string(),
                        port_id: "dynamic_0".to_string(),
                    },
                },
            ],
            outputs: Some(HashMap::from([(
                String::from("composite"),
                String::from("out"),
            )])),
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
        };

        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();

        let got = composite_layers_in_draw_order(&scene, &nodes_by_id, "out").unwrap();
        // inputs array order: dynamic_1 then dynamic_0
        assert_eq!(got, vec!["p_img", "p1", "p0"]);
    }
}
