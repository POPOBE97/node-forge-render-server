//! A self-contained texture source lowered to the existing chart render graph.
//! The bundled graph, materials, and fonts are generated from the canonical editor example.
use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, ensure};

use crate::{
    dsl::{Node, SceneDSL},
    renderer::utils::cpu_num_f32,
};

const REFERENCE_PREFIX: &str = "builtin:procedural-test-texture/";
const CHART: &str = include_str!("../../../assets/procedural-test-texture/scene.json");
const PATTERN: &str =
    include_str!("../../../assets/procedural-test-texture/chart.pattern.material.wgsl");
const HORIZONTAL: &str =
    include_str!("../../../assets/procedural-test-texture/chart.blur.horizontal.material.wgsl");
const VERTICAL: &str =
    include_str!("../../../assets/procedural-test-texture/chart.blur.vertical.material.wgsl");

pub(crate) fn material_source(reference: &str) -> Option<String> {
    let name = reference.strip_prefix(REFERENCE_PREFIX)?;
    // Preserve the authored shader's numeric RGB. Like the source graph,
    // it is interpreted in the enclosing scene's working color space.
    Some(match name {
        "chart.pattern.material.wgsl" => PATTERN.to_string(),
        "chart.blur.horizontal.material.wgsl" => HORIZONTAL.to_string(),
        "chart.blur.vertical.material.wgsl" => VERTICAL.to_string(),
        _ => return None,
    })
}

pub(crate) fn font_bytes(reference: &str) -> Option<&'static [u8]> {
    Some(match reference.strip_prefix(REFERENCE_PREFIX)? {
        "525979822591" => {
            include_bytes!("../../../assets/procedural-test-texture/525979822591.ttf")
        }
        "d72db21f9242" => {
            include_bytes!("../../../assets/procedural-test-texture/d72db21f9242.ttf")
        }
        "6aa32ded55e1" => include_bytes!("../../../assets/fonts/MiSans/MiSans-Regular.ttf"),
        _ => return None,
    })
}

fn dimension(
    scene: &SceneDSL,
    nodes: &HashMap<String, Node>,
    node: &Node,
    port: &str,
    default: f32,
) -> Result<u32> {
    let value = cpu_num_f32(scene, nodes, node, port, default)?;
    ensure!(
        value.is_finite() && value.fract() == 0.0 && (1.0..=8192.0).contains(&value),
        "ProceduralTestTexture {}.{port} must be an integer in [1, 8192]",
        node.id
    );
    Ok(value as u32)
}

pub(crate) fn expand(scene: &mut SceneDSL) -> Result<()> {
    let sources: Vec<Node> = scene
        .nodes
        .iter()
        .filter(|node| node.node_type == "ProceduralTestTexture")
        .cloned()
        .collect();
    if sources.is_empty() {
        return Ok(());
    }
    let nodes_by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.clone()))
        .collect();
    let mut ids: HashSet<String> = nodes_by_id.keys().cloned().collect();
    let mut edge_ids: HashSet<String> = scene
        .connections
        .iter()
        .map(|edge| edge.id.clone())
        .collect();

    for source in sources {
        let width = dimension(scene, &nodes_by_id, &source, "width", 1260.0)?;
        let height = dimension(scene, &nodes_by_id, &source, "height", 2800.0)?;
        let mut size_sources = HashMap::new();
        for edge in scene
            .connections
            .iter()
            .filter(|edge| edge.to.node_id == source.id)
        {
            ensure!(
                matches!(edge.to.port_id.as_str(), "width" | "height"),
                "unsupported ProceduralTestTexture input {}",
                edge.to.port_id
            );
            size_sources.insert(format!("chart.{}", edge.to.port_id), edge.from.clone());
        }
        for edge in scene
            .connections
            .iter()
            .filter(|edge| edge.from.node_id == source.id)
        {
            ensure!(
                edge.from.port_id == "texture",
                "ProceduralTestTexture exposes only texture"
            );
        }
        let mut chart: SceneDSL =
            serde_json::from_str(CHART).context("invalid bundled procedural test chart")?;
        super::group_expand::expand_group_instances(&mut chart)?;
        let prefix = format!("sys.procedural-test.{}.", source.id);
        let rename = |id: &str| {
            if id == "chart.output" {
                source.id.clone()
            } else {
                format!("{prefix}{id}")
            }
        };
        for node in &mut chart.nodes {
            match node.id.as_str() {
                "chart.width" => {
                    node.params.insert("value".into(), width.into());
                }
                "chart.height" => {
                    node.params.insert("value".into(), height.into());
                }
                _ => {}
            }
            node.params.retain(|key, _| !key.starts_with("__dedup_"));
            node.id = rename(&node.id);
            ensure!(
                node.id == source.id || ids.insert(node.id.clone()),
                "procedural test node ID collision: {}",
                node.id
            );
            for binding in &mut node.input_bindings {
                if let Some(endpoint) = &mut binding.source_binding {
                    endpoint.node_id = rename(&endpoint.node_id);
                }
            }
        }
        for edge in &mut chart.connections {
            edge.id = format!("{prefix}edge.{}", edge.id);
            ensure!(
                edge_ids.insert(edge.id.clone()),
                "procedural test connection ID collision: {}",
                edge.id
            );
            // Fan the authored size edge out to the target, layout, and label. Input
            // declaration nodes have no input ports and cannot act as proxy receivers.
            if let Some(endpoint) = size_sources.get(&edge.from.node_id) {
                edge.from = endpoint.clone();
            } else {
                edge.from.node_id = rename(&edge.from.node_id);
            }
            edge.to.node_id = rename(&edge.to.node_id);
        }
        scene
            .connections
            .retain(|edge| edge.to.node_id != source.id);
        scene.nodes.retain(|node| node.id != source.id);
        scene.nodes.extend(chart.nodes);
        scene.connections.extend(chart.connections);
    }
    Ok(())
}
