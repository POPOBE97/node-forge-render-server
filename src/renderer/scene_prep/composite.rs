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
