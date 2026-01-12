use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail, Context, Result};
use rust_wgpu_fiber::eframe::wgpu::TextureFormat;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SceneDSL {
    pub version: String,
    pub metadata: Metadata,
    pub nodes: Vec<Node>,
    pub connections: Vec<Connection>,
    pub outputs: Option<HashMap<String, String>>,
}

/// Drops nodes that do not participate in any connection, to avoid later stages
/// (scheme validation / compilation) tripping over editor leftovers.
///
/// Keep set includes:
/// - Any node referenced by `connections` (as either `from` or `to`)
/// - Any node referenced by `outputs` values
pub fn treeshake_unlinked_nodes(scene: &SceneDSL) -> SceneDSL {
    let mut keep: HashSet<&str> = HashSet::new();

    for c in &scene.connections {
        keep.insert(c.from.node_id.as_str());
        keep.insert(c.to.node_id.as_str());
    }

    if let Some(outputs) = scene.outputs.as_ref() {
        for node_id in outputs.values() {
            keep.insert(node_id.as_str());
        }
    }

    let nodes: Vec<Node> = scene
        .nodes
        .iter()
        .cloned()
        .filter(|n| keep.contains(n.id.as_str()))
        .collect();

    SceneDSL {
        version: scene.version.clone(),
        metadata: scene.metadata.clone(),
        nodes,
        connections: scene.connections.clone(),
        outputs: scene.outputs.clone(),
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Metadata {
    pub name: String,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,

    // Optional editor metadata used for ordering / UI; we only consume `inputs` ordering
    // for Composite draw order.
    #[serde(default)]
    pub inputs: Vec<NodePort>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NodePort {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "type", default)]
    pub port_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Connection {
    pub id: String,
    pub from: Endpoint,
    pub to: Endpoint,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Endpoint {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "portId")]
    pub port_id: String,
}

pub fn load_scene_from_default_asset() -> Result<SceneDSL> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("node-forge-example.1.json");
    load_scene_from_path(path)
}

pub fn load_scene_from_path(path: impl AsRef<std::path::Path>) -> Result<SceneDSL> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read DSL json at {}", path.display()))?;
    let scene: SceneDSL = serde_json::from_str(&text).context("failed to parse DSL json")?;
    Ok(scene)
}

pub fn find_node<'a>(nodes_by_id: &'a HashMap<String, Node>, node_id: &str) -> Result<&'a Node> {
    nodes_by_id
        .get(node_id)
        .ok_or_else(|| anyhow!("node not found: {node_id}"))
}

pub fn incoming_connection<'a>(
    scene: &'a SceneDSL,
    to_node_id: &str,
    to_port_id: &str,
) -> Option<&'a Connection> {
    scene
        .connections
        .iter()
        .find(|c| c.to.node_id == to_node_id && c.to.port_id == to_port_id)
}

pub fn parse_u32(params: &HashMap<String, serde_json::Value>, key: &str) -> Option<u32> {
    params
        .get(key)
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
}

pub fn parse_f32(params: &HashMap<String, serde_json::Value>, key: &str) -> Option<f32> {
    match params.get(key) {
        Some(v) => v
            .as_f64()
            .map(|x| x as f32)
            .or_else(|| v.as_u64().map(|x| x as f32))
            .or_else(|| v.as_i64().map(|x| x as f32)),
        None => None,
    }
}

pub fn parse_str<'a>(params: &'a HashMap<String, serde_json::Value>, key: &str) -> Option<&'a str> {
    params.get(key).and_then(|v| v.as_str())
}

pub fn parse_texture_format(params: &HashMap<String, serde_json::Value>) -> Result<TextureFormat> {
    let fmt = params
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("rgba8unorm")
        .to_ascii_lowercase();
    match fmt.as_str() {
        "rgba8unorm" => Ok(TextureFormat::Rgba8Unorm),
        "rgba8unormsrgb" | "rgba8unorm_srgb" => Ok(TextureFormat::Rgba8UnormSrgb),
        other => bail!("unsupported RenderTexture.format: {other}"),
    }
}

pub fn screen_resolution(scene: &SceneDSL) -> Option<[u32; 2]> {
    let screen = scene.nodes.iter().find(|n| n.node_type == "Screen")?;
    let w = parse_u32(&screen.params, "width")?;
    let h = parse_u32(&screen.params, "height")?;
    if w == 0 || h == 0 {
        return None;
    }
    Some([w, h])
}
