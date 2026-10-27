use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow, bail};
use rust_wgpu_fiber::wgpu::TextureFormat;
use serde::{Deserialize, Serialize};

use crate::schema;

#[derive(Debug, Clone)]
pub struct FileRenderTarget {
    pub directory: String,
    pub file_name: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AssetEntry {
    pub path: String,
    #[serde(rename = "originalName", default)]
    pub original_name: String,
    #[serde(rename = "mimeType", default)]
    pub mime_type: String,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DebugArtifacts {
    pub version: u32,
    #[serde(default)]
    pub items: HashMap<String, DebugArtifactItem>,
}

impl Default for DebugArtifacts {
    fn default() -> Self {
        Self {
            version: 1,
            items: HashMap::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DebugArtifactItem {
    pub id: String,
    pub anchor: DebugArtifactAnchor,
    pub role: DebugArtifactRole,
    pub name: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(
        rename = "contentHash",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub content_hash: Option<String>,
    #[serde(rename = "slotKey", default, skip_serializing_if = "Option::is_none")]
    pub slot_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DebugArtifactRole {
    ReferenceCode,
    Patch,
    Note,
    Image,
    Attachment,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DebugArtifactAnchor {
    Pass {
        #[serde(rename = "passName")]
        pass_name: String,
    },
    DependencyTarget {
        #[serde(rename = "passName")]
        pass_name: String,
        target: DebugTargetAnchor,
    },
    DependencyOccurrence {
        #[serde(rename = "passName")]
        pass_name: String,
        target: DebugTargetAnchor,
        #[serde(rename = "occurrenceKey")]
        occurrence_key: String,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DebugTargetAnchor {
    pub scope: String,
    pub name: String,
    pub kind: String,
    #[serde(
        rename = "sourceExcerptHash",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub source_excerpt_hash: Option<String>,
    #[serde(
        rename = "targetIdHint",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub target_id_hint: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SceneDSL {
    #[serde(deserialize_with = "deserialize_scene_version")]
    pub version: String,
    #[serde(rename = "workingColorSpace")]
    pub working_color_space: crate::color::WorkingColorSpace,
    #[serde(deserialize_with = "deserialize_dpi")]
    pub dpi: f64,
    pub metadata: Metadata,
    pub nodes: Vec<Node>,
    pub connections: Vec<Connection>,
    pub outputs: Option<HashMap<String, String>>,
    #[serde(default)]
    pub groups: Vec<GroupDSL>,
    #[serde(default)]
    pub assets: HashMap<String, AssetEntry>,
    #[serde(default, rename = "stateMachine")]
    pub state_machine: Option<crate::state_machine::types::StateMachine>,
    #[serde(
        default,
        rename = "debugArtifacts",
        skip_serializing_if = "Option::is_none"
    )]
    pub debug_artifacts: Option<DebugArtifacts>,
}

fn deserialize_dpi<'de, D>(deserializer: D) -> Result<f64, D::Error>
where D: serde::Deserializer<'de> {
    let dpi = f64::deserialize(deserializer)?;
    if !dpi.is_finite() || dpi <= 0.0 {
        return Err(serde::de::Error::custom("dpi must be finite and positive"));
    }
    Ok(dpi)
}

fn deserialize_scene_version<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = String::deserialize(deserializer)?;
    if version != "6.1" {
        return Err(serde::de::Error::custom(format!(
            "unsupported SceneDSL version '{version}' (expected 6.1)"
        )));
    }
    Ok(version)
}

/// A reusable subgraph definition referenced by `GroupInstance` nodes.
///
/// This matches the editor-exported `groups` array in SceneDSL JSON.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GroupDSL {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub inputs: Vec<NodePort>,
    #[serde(default)]
    pub outputs: Vec<NodePort>,

    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub connections: Vec<Connection>,

    #[serde(default, rename = "inputBindings")]
    pub input_bindings: Vec<GroupInputBinding>,
    #[serde(default, rename = "outputBindings")]
    pub output_bindings: Vec<GroupOutputBinding>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GroupInputBinding {
    #[serde(rename = "groupPortId")]
    pub group_port_id: String,
    pub to: Endpoint,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GroupOutputBinding {
    #[serde(rename = "groupPortId")]
    pub group_port_id: String,
    pub from: Endpoint,
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
        working_color_space: scene.working_color_space,
        dpi: scene.dpi,
        metadata: scene.metadata.clone(),
        nodes,
        connections: scene.connections.clone(),
        outputs: scene.outputs.clone(),
        groups: scene.groups.clone(),
        assets: scene.assets.clone(),
        state_machine: scene.state_machine.clone(),
        debug_artifacts: scene.debug_artifacts.clone(),
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Metadata {
    pub name: String,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub node_type: String,
    pub params: HashMap<String, serde_json::Value>,

    // Optional editor metadata used for ordering / UI.
    pub inputs: Vec<NodePort>,
    pub outputs: Vec<NodePort>,

    pub input_bindings: Vec<InputBinding>,

    /// Optional WGSL material override. `materials/<nodeId>.wgsl` is a logical
    /// resource reference with an opaque ID; native working-copy paths are
    /// resolved by node-forge-resource-store. Standalone overrides may be files.
    pub wgsl_override: Option<String>,
}

impl Serialize for Node {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct WireNode<'a> {
            id: &'a str,
            #[serde(rename = "type")]
            node_type: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            label: Option<&'a str>,
            params: HashMap<&'a String, &'a serde_json::Value>,
            inputs: &'a [NodePort],
            outputs: &'a [NodePort],
            #[serde(rename = "inputBindings")]
            input_bindings: &'a [InputBinding],
            #[serde(rename = "wgslOverride", skip_serializing_if = "Option::is_none")]
            wgsl_override: Option<&'a String>,
        }

        let label = self
            .params
            .get("__node_label")
            .and_then(|value| value.as_str());
        let params = self
            .params
            .iter()
            .filter(|(key, _)| key.as_str() != "__node_label")
            .collect();

        WireNode {
            id: &self.id,
            node_type: &self.node_type,
            label,
            params,
            inputs: &self.inputs,
            outputs: &self.outputs,
            input_bindings: &self.input_bindings,
            wgsl_override: self.wgsl_override.as_ref(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireNode {
            id: String,
            #[serde(rename = "type")]
            node_type: String,
            #[serde(default)]
            label: Option<String>,
            #[serde(default)]
            params: HashMap<String, serde_json::Value>,
            #[serde(default)]
            inputs: Vec<NodePort>,
            #[serde(default)]
            outputs: Vec<NodePort>,
            #[serde(default, rename = "inputBindings")]
            input_bindings: Vec<InputBinding>,
            #[serde(default, rename = "wgslOverride")]
            wgsl_override: Option<String>,
        }

        let wire = WireNode::deserialize(deserializer)?;
        let mut params = wire.params;
        if let Some(label) = wire.label.map(|label| label.trim().to_string()) {
            if !label.is_empty() {
                params.insert("__node_label".to_string(), serde_json::Value::String(label));
            }
        }

        Ok(Self {
            id: wire.id,
            node_type: wire.node_type,
            params,
            inputs: wire.inputs,
            outputs: wire.outputs,
            input_bindings: wire.input_bindings,
            wgsl_override: wire.wgsl_override,
        })
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct InputBinding {
    #[serde(rename = "portId")]
    pub port_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(rename = "variableName")]
    pub variable_name: String,
    #[serde(rename = "type", default)]
    pub binding_type: Option<String>,
    #[serde(rename = "sourceBinding")]
    pub source_binding: Option<SourceBinding>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SourceBinding {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "outputPortId")]
    pub output_port_id: String,
    #[serde(default, rename = "outputLabel")]
    pub output_label: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NodePort {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "type", default)]
    pub port_type: Option<String>,
    #[serde(default, rename = "arrayLength")]
    pub array_length: Option<usize>,
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
    let raw_scene: serde_json::Value =
        serde_json::from_str(&text).context("failed to parse DSL json")?;
    let mut scene: SceneDSL = serde_json::from_str(&text).context("failed to parse DSL json")?;

    materialize_scene_node_labels_from_raw_json(&mut scene, &raw_scene);

    // Normalize params with defaults from the bundled node scheme.
    // This keeps older/hand-written DSL compatible when nodes omit parameters.
    normalize_scene_defaults(&mut scene)?;

    Ok(scene)
}

pub fn materialize_scene_node_labels_from_raw_json(
    scene: &mut SceneDSL,
    raw_scene: &serde_json::Value,
) {
    if let Some(raw_nodes) = raw_scene.get("nodes").and_then(|v| v.as_array()) {
        materialize_node_labels_from_raw_nodes(&mut scene.nodes, raw_nodes);
    }

    if let Some(raw_groups) = raw_scene.get("groups").and_then(|v| v.as_array()) {
        for (group, raw_group) in scene.groups.iter_mut().zip(raw_groups.iter()) {
            if let Some(raw_group_nodes) = raw_group.get("nodes").and_then(|v| v.as_array()) {
                materialize_node_labels_from_raw_nodes(&mut group.nodes, raw_group_nodes);
            }
        }
    }
}

pub fn materialize_node_labels_from_raw_nodes(nodes: &mut [Node], raw_nodes: &[serde_json::Value]) {
    let mut label_by_id: HashMap<&str, String> = HashMap::new();
    for raw_node in raw_nodes {
        let Some(node_id) = raw_node.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(label) = raw_node.get("label").and_then(|v| v.as_str()) else {
            continue;
        };
        let label = label.trim();
        if label.is_empty() {
            continue;
        }
        label_by_id.insert(node_id, label.to_string());
    }

    for node in nodes {
        if let Some(label) = label_by_id.get(node.id.as_str()) {
            node.params.insert(
                "label".to_string(),
                serde_json::Value::String(label.clone()),
            );
        }
    }
}

pub fn normalize_scene_defaults(scene: &mut SceneDSL) -> Result<()> {
    let scheme = schema::load_default_scheme()?;
    screen_color_space(scene)?;
    apply_node_default_params(scene, &scheme);
    normalize_scene_color_literals(scene, &scheme);
    Ok(())
}

/// Source-tagged colors are converted exactly once; linear numeric values are untouched.
pub(crate) fn normalize_scene_color_literals(scene: &mut SceneDSL, scheme: &schema::NodeScheme) {
    fn normalize(value: &mut serde_json::Value, space: crate::color::WorkingColorSpace, packed: bool) {
        if packed {
            if let Some(items) = value.as_array_mut() {
                for item in items { normalize(item, space, false); }
            }
        } else if let Some(rgba) = value.as_str().and_then(|s| crate::color::hex_to_working(s, space)) {
            *value = serde_json::json!(rgba);
        }
    }
    fn node_colors(node: &mut Node, scheme: &schema::NodeScheme, space: crate::color::WorkingColorSpace) {
        let mut ports = Vec::new();
        if let Some(def) = scheme.nodes.get(&node.node_type) {
            for (id, ty) in &def.inputs {
                if let schema::PortTypeSpec::One(ty) = ty { ports.push((id.clone(), ty.clone())); }
            }
        }
        ports.extend(node.inputs.iter().filter_map(|p| p.port_type.as_ref().map(|ty| (p.id.clone(), ty.clone()))));
        ports.extend(node.input_bindings.iter().filter_map(|p| p.binding_type.as_ref().map(|ty| (p.port_id.clone(), ty.clone()))));
        match node.node_type.as_str() {
            "ColorInput" => ports.push(("value".into(), "color".into())),
            "ColorArrayInput" => ports.push(("value".into(), "packed<color>".into())),
            _ => {}
        }
        for (id, ty) in ports {
            if matches!(ty.as_str(), "color" | "packed<color>") {
                if let Some(value) = node.params.get_mut(&id) { normalize(value, space, ty == "packed<color>"); }
            }
        }
    }
    for node in &mut scene.nodes { node_colors(node, scheme, scene.working_color_space); }
    for group in &mut scene.groups {
        for node in &mut group.nodes { node_colors(node, scheme, scene.working_color_space); }
    }
}

fn apply_node_default_params(scene: &mut SceneDSL, scheme: &schema::NodeScheme) {
    fn apply_one(node: &mut Node, scheme: &schema::NodeScheme) {
        let Some(node_scheme) = scheme.nodes.get(&node.node_type) else {
            let has_label = node
                .params
                .get("label")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.trim().is_empty());
            if !has_label {
                node.params.insert(
                    "label".to_string(),
                    serde_json::Value::String(node.node_type.clone()),
                );
            }
            return;
        };

        let has_label = node
            .params
            .get("label")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty());
        if !has_label {
            let fallback = node_scheme
                .label
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(node.node_type.as_str())
                .to_string();
            node.params
                .insert("label".to_string(), serde_json::Value::String(fallback));
        }

        if node_scheme.default_params.is_empty() && node_scheme.input_defaults.is_empty() {
            return;
        }

        let mut merged = node_scheme.input_defaults.clone();
        merged.extend(node_scheme.default_params.clone());
        for (k, v) in std::mem::take(&mut node.params) {
            merged.insert(k, v);
        }
        node.params = merged;
    }

    for node in &mut scene.nodes {
        apply_one(node, scheme);
    }
    for g in &mut scene.groups {
        for node in &mut g.nodes {
            apply_one(node, scheme);
        }
    }
}

/// If the scene's (single) RenderTarget node is `File`, return its directory/fileName parameters.
///
/// Note: if params are missing, falls back to scheme defaults (`directory=""`, `fileName="output.png"`).
pub fn file_render_target(scene: &SceneDSL) -> Result<Option<FileRenderTarget>> {
    let scheme = schema::load_default_scheme()?;
    let render_targets: Vec<&Node> = scene
        .nodes
        .iter()
        .filter(|n| {
            scheme
                .nodes
                .get(&n.node_type)
                .and_then(|s| s.category.as_deref())
                == Some("RenderTarget")
        })
        .collect();

    if render_targets.is_empty() {
        return Ok(None);
    }
    if render_targets.len() != 1 {
        bail!(
            "expected exactly 1 RenderTarget node, got {}",
            render_targets.len()
        );
    }

    let rt = render_targets[0];
    if rt.node_type != "File" {
        return Ok(None);
    }

    let directory = parse_str(&rt.params, "directory").unwrap_or("").to_string();
    let file_name = parse_str(&rt.params, "fileName")
        .or_else(|| parse_str(&rt.params, "filename"))
        .unwrap_or("output.png")
        .to_string();

    Ok(Some(FileRenderTarget {
        directory,
        file_name,
    }))
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

pub fn node_display_label(node: &Node) -> String {
    node.params
        .get("__node_label")
        .or_else(|| node.params.get("label"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| node.node_type.clone())
}

pub fn node_display_label_with_id(node: &Node) -> String {
    format!("{} ({})", node_display_label(node), node.id)
}

pub fn parse_texture_format(params: &HashMap<String, serde_json::Value>) -> Result<TextureFormat> {
    let fmt_raw = params
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("rgba8unorm")
        .to_ascii_lowercase();

    // Scene JSON uses kebab-case (e.g. `rgba8unorm-srgb`) while some older DSL examples
    // used snake-case (`rgba8unorm_srgb`) or a collapsed token (`rgba8unormsrgb`).
    // Normalize those variants into a single key.
    let fmt = fmt_raw.replace(['-', '_'], "");

    match fmt.as_str() {
        "rgba8unorm" => Ok(TextureFormat::Rgba8Unorm),
        "rgba8unormsrgb" => Ok(TextureFormat::Rgba8UnormSrgb),
        "rgba16float" => Ok(TextureFormat::Rgba16Float),
        "bgra8unorm" => Ok(TextureFormat::Bgra8Unorm),
        "bgra8unormsrgb" => Ok(TextureFormat::Bgra8UnormSrgb),
        _ => bail!("unsupported RenderTexture.format: {}", fmt_raw),
    }
}

pub fn screen_resolution(scene: &SceneDSL) -> Option<[u32; 2]> {
    let prepared = crate::renderer::scene_prep::prepare_scene(scene).ok()?;
    if prepared.output_target_node_id.is_some() {
        return Some(prepared.resolution);
    }
    // A processing output owns its extent too. Use the same CPU render planner
    // as presentation instead of exposing the geometry planning default.
    use crate::renderer::render_plan::{planner::RenderPlanner, types::PlanBuildOptions};
    RenderPlanner::new(PlanBuildOptions {
        gpu_caps: Default::default(),
        presentation_mode: Default::default(),
        debug_dump_wgsl_dir: None,
    }).plan(scene, None, None).ok().map(|plan| plan.resolution)
}

pub fn screen_color_space(scene: &SceneDSL) -> anyhow::Result<crate::color::WorkingColorSpace> {
    use crate::color::WorkingColorSpace;
    let Some(screen) = scene.nodes.iter().find(|node| node.node_type == "Screen") else {
        return Ok(scene.working_color_space);
    };
    match screen.params.get("outputColorSpace").and_then(|value| value.as_str()) {
        Some("srgb") => Ok(WorkingColorSpace::Srgb),
        Some("display-p3") => Ok(WorkingColorSpace::DisplayP3),
        _ => anyhow::bail!("Screen.outputColorSpace must be srgb or display-p3"),
    }
}

fn parse_f64(params: &HashMap<String, serde_json::Value>, key: &str) -> Option<f64> {
    match params.get(key) {
        Some(v) => v
            .as_f64()
            .or_else(|| v.as_u64().map(|x| x as f64))
            .or_else(|| v.as_i64().map(|x| x as f64)),
        None => None,
    }
}

fn resolve_midi_input_f64(params: &HashMap<String, serde_json::Value>) -> f64 {
    let default = parse_f64(params, "defaultValue")
        .or_else(|| parse_f64(params, "value"))
        .unwrap_or(0.0);

    let Some(raw) = parse_f64(params, "rawValue") else {
        return default;
    };

    let min = parse_f64(params, "minValue").unwrap_or(0.0);
    let max = parse_f64(params, "maxValue").unwrap_or(1.0);
    let t = raw.clamp(0.0, 127.0) / 127.0;
    min + (max - min) * t
}

fn f64_to_u32_floor(v: f64) -> Option<u32> {
    if !v.is_finite() {
        return None;
    }
    let clamped = v.max(0.0).floor();
    if clamped > (u32::MAX as f64) {
        return None;
    }
    Some(clamped as u32)
}

/// Resolve a numeric *input* value for `node_id.port_id`.
///
/// DSL semantics:
/// - If there is an incoming connection to this input port, the connected upstream output wins.
/// - Otherwise, fallback to `node.params[port_id]` (inline input / default params).
/// - If still missing, fallback to bundled node-scheme input-port default value.
///
/// This is intentionally minimal: it only implements the node subset needed for
/// CPU-side parameter resolution (e.g. RenderTexture.width/height).
pub fn dp_to_px(value: f64, dpi: f64, integer: bool) -> Result<f64> {
    if !dpi.is_finite() || dpi <= 0.0 || !value.is_finite() {
        bail!("DP conversion requires a finite input and positive finite dpi");
    }
    if integer && (value.fract() != 0.0 || value < i32::MIN as f64 || value > i32::MAX as f64) {
        bail!("Int DP input must be a signed 32-bit integer");
    }
    let value = value * dpi;
    // Rust round is ties-away-from-zero; GPU inputs receive this already evaluated value.
    let value = if integer { value.round() } else { value };
    if !value.is_finite() || value.abs() > f32::MAX as f64
        || (integer && (value < i32::MIN as f64 || value > i32::MAX as f64)) {
        bail!("DP result is outside the output type's range");
    }
    Ok(value)
}

/// Read one numeric declaration component as an output (px for DP declarations).
pub fn numeric_input_output_component(
    scene: &SceneDSL, nodes: &HashMap<String, Node>, node: &Node, component: &str,
) -> Result<f64> {
    let value = resolve_input_f64(scene, nodes, &node.id, component)?.unwrap_or(0.0);
    Ok(value)
}

/// Numeric declaration output used by packed values and CPU consumers.
pub fn numeric_declaration_value(scene: &SceneDSL, node: &Node) -> Result<serde_json::Value> {
    let nodes = scene.nodes.iter().cloned().map(|n| (n.id.clone(), n)).collect();
    let keys: &[&str] = match node.node_type.as_str() {
        "Vector2Input" => &["x", "y"],
        "Vector3Input" => &["x", "y", "z"],
        "Vector4Input" => &["x", "y", "z", "w"],
        "FloatInput" | "IntInput" => &["value"],
        _ => return Ok(node.params.get("value").cloned().unwrap_or(serde_json::Value::Null)),
    };
    let values = keys.iter().map(|key| numeric_input_output_component(scene, &nodes, node, key)).collect::<Result<Vec<_>>>()?;
    if keys.len() == 1 { Ok(serde_json::json!(values[0])) } else { Ok(serde_json::json!(values)) }
}

/// Shared CPU vector resolution. Connections take precedence over inline values.
pub fn resolve_input_vec2(scene: &SceneDSL, nodes: &HashMap<String, Node>, node: &Node, port: &str) -> Result<Option<[f64; 2]>> {
    let computed;
    let value = if let Some(edge) = incoming_connection(scene, &node.id, port) {
        let source = find_node(nodes, &edge.from.node_id)?;
        match source.node_type.as_str() {
            "Vector2Input" if edge.from.port_id == "vector" => return Ok(Some([
                numeric_input_output_component(scene, nodes, source, "x")?,
                numeric_input_output_component(scene, nodes, source, "y")?,
            ])),
            "DataParse" => {
                computed = evaluate_data_parse(scene, nodes, source, 0,
                    &mut crate::renderer::scene_prep::data_parse_runtime::TsRuntime::new())?;
                data_parse_output(source, &computed, &edge.from.port_id)?
            }
            _ => bail!("{}.{} requires a CPU vector, got {}.{}", node.id, port, source.node_type, edge.from.port_id),
        }
    } else {
        let Some(value) = node.params.get(port) else { return Ok(None); };
        computed = crate::input_units::inline_value(node, port, value, scene.dpi)?;
        &computed
    };
    let xy = if let Some(array) = value.as_array() {
        if array.len() != 2 { bail!("{}.{} requires exactly two components", node.id, port); }
        [array[0].as_f64(), array[1].as_f64()]
    } else { [value.get("x").and_then(|v| v.as_f64()), value.get("y").and_then(|v| v.as_f64())] };
    let [Some(x), Some(y)] = xy else { bail!("{}.{} requires [x,y] or {{x,y}}", node.id, port); };
    if !x.is_finite() || !y.is_finite() { bail!("{}.{} must be finite", node.id, port); }
    Ok(Some([x, y]))
}

pub fn physical_texture_dimension(value: f64) -> Result<u32> {
    let rounded = value.round();
    if !value.is_finite() || value <= 0.0 || rounded < 1.0 || rounded > u32::MAX as f64 {
        bail!("texture dimension must round to a positive unsigned 32-bit integer, got {value}");
    }
    Ok(rounded as u32)
}

pub fn processing_target_size(scene: &SceneDSL, nodes: &HashMap<String, Node>, node: &Node) -> Result<[u32; 2]> {
    let [x, y] = resolve_input_vec2(scene, nodes, node, "targetSize")?
        .with_context(|| format!("missing {}.targetSize", node.id))?;
    Ok([physical_texture_dimension(x)?, physical_texture_dimension(y)?])
}

/// Resolve a texture's authored size, then convert exactly once at allocation boundary.
pub fn render_texture_size(scene: &SceneDSL, nodes: &HashMap<String, Node>, node: &Node) -> Result<[u32; 2]> {
    let mut size = [0; 2];
    for (index, port) in ["width", "height"].iter().enumerate() {
        let value = resolve_input_f64(scene, nodes, &node.id, port)?.unwrap_or(1080.0);
        size[index] = physical_texture_dimension(value)
            .with_context(|| format!("invalid RenderTexture {}.{}", node.id, port))?;
    }
    Ok(size)
}

pub fn resolve_input_f64(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    port_id: &str,
) -> Result<Option<f64>> {
    let mut cache: HashMap<(String, String), f64> = HashMap::new();
    let mut visiting: HashSet<(String, String)> = HashSet::new();
    resolve_input_f64_inner(
        scene,
        nodes_by_id,
        node_id,
        port_id,
        &mut cache,
        &mut visiting,
    )
}

pub fn resolve_input_f32(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    port_id: &str,
) -> Result<Option<f32>> {
    Ok(resolve_input_f64(scene, nodes_by_id, node_id, port_id)?
        .and_then(|v| if v.is_finite() { Some(v as f32) } else { None }))
}

fn f64_to_i64_floor(v: f64) -> Option<i64> {
    if !v.is_finite() {
        return None;
    }
    let floored = v.floor();
    if floored > (i64::MAX as f64) || floored < (i64::MIN as f64) {
        return None;
    }
    Some(floored as i64)
}

pub fn resolve_input_i64(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    port_id: &str,
) -> Result<Option<i64>> {
    Ok(resolve_input_f64(scene, nodes_by_id, node_id, port_id)?.and_then(f64_to_i64_floor))
}

pub fn resolve_input_u32(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    port_id: &str,
) -> Result<Option<u32>> {
    Ok(resolve_input_f64(scene, nodes_by_id, node_id, port_id)?.and_then(f64_to_u32_floor))
}

fn strip_data_parse_type_assertions(src: &str) -> String {
    src.replace(" as vec2", "")
        .replace(" as vec3", "")
        .replace(" as vec4", "")
        .replace(" as int", "")
        .replace(" as i32", "")
        .replace(" as uint", "")
        .replace(" as u32", "")
        .replace(" as float", "")
        .replace(" as f32", "")
        .replace(" as number", "")
        .replace(" as bool", "")
        .replace(" as boolean", "")
}

pub(crate) fn evaluate_data_parse(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    index: u32,
    runtime: &mut crate::renderer::scene_prep::data_parse_runtime::TsRuntime,
) -> Result<serde_json::Value> {
    evaluate_data_parse_inner(
        scene, nodes_by_id, node, index, runtime, &mut HashMap::new(), &mut HashSet::new(),
    )
}

fn evaluate_data_parse_inner(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    index: u32,
    runtime: &mut crate::renderer::scene_prep::data_parse_runtime::TsRuntime,
    cache: &mut HashMap<(String, String), f64>,
    visiting: &mut HashSet<(String, String)>,
) -> Result<serde_json::Value> {
    let key = (node.id.clone(), "@data-parse".to_string());
    if !visiting.insert(key.clone()) {
        bail!("cycle detected while evaluating DataParse {}", node.id);
    }
    let src = node
        .params
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut bindings_src = String::new();
    for b in &node.input_bindings {
        let connection = incoming_connection(scene, &node.id, &b.port_id);
        let val = match connection.map(|edge| &edge.from) {
            Some(source) if source.port_id == "data" => {
                let data_node = find_node(nodes_by_id, &source.node_id)?;
                let text = data_node
                    .params
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if text.trim().is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::from_str(text)?
                }
            }
            Some(source) if source.port_id == "index" => serde_json::json!(index),
            Some(source) if source.port_id == "vector" && nodes_by_id.get(&source.node_id)
                .is_some_and(|node| matches!(node.node_type.as_str(), "Vector2Input" | "Vector3Input" | "Vector4Input")) => {
                let node = find_node(nodes_by_id, &source.node_id)?;
                let keys: &[&str] = match node.node_type.as_str() {
                    "Vector2Input" => &["x", "y"],
                    "Vector3Input" => &["x", "y", "z"],
                    _ => &["x", "y", "z", "w"],
                };
                let values = keys.iter().map(|component| {
                    let value = resolve_input_f64_inner(scene, nodes_by_id, &node.id, component, cache, visiting)?.unwrap_or(0.0);
                    Ok(value)
                }).collect::<Result<Vec<_>>>()?;
                serde_json::json!(values)
            }
            Some(source) if nodes_by_id.get(&source.node_id).is_some_and(|node| node.node_type == "DataParse") => {
                let source_node = find_node(nodes_by_id, &source.node_id)?;
                let output = evaluate_data_parse_inner(scene, nodes_by_id, source_node, index, runtime, cache, visiting)?;
                data_parse_output(source_node, &output, &source.port_id)?.clone()
            }
            Some(source) => {
                let value = resolve_output_f64_inner(
                    scene, nodes_by_id, &source.node_id, &source.port_id, cache, visiting,
                )?;
                if b.binding_type.as_deref() == Some("bool") {
                    serde_json::json!(value != 0.0)
                } else {
                    serde_json::json!(value)
                }
            }
            None => crate::input_units::inline_value(node, &b.port_id, node.params.get(&b.port_id).unwrap_or(&serde_json::Value::Null), scene.dpi)?,
        };

        let json = serde_json::to_string(&val)?;
        bindings_src.push_str(&format!("const {} = {};\n", b.variable_name, json));
    }

    if !node
        .input_bindings
        .iter()
        .any(|b| b.variable_name == "index")
    {
        bindings_src.push_str(&format!("const index = {index};\n"));
    }

    let user_src = strip_data_parse_type_assertions(src);

    let script_body = format!("{bindings_src}\n{user_src}\n");
    let script = format!("(function() {{\n{}\n}})()", script_body);

    let result = runtime.eval_script(&script)
        .with_context(|| format!("evaluating DataParse {}", node.id));
    visiting.remove(&key);
    result
}

pub(crate) fn data_parse_output<'a>(
    node: &Node,
    output: &'a serde_json::Value,
    port_id: &str,
) -> Result<&'a serde_json::Value> {
    let port = node
        .outputs
        .iter()
        .find(|port| port.id == port_id)
        .with_context(|| format!("DataParse {} has no output {port_id}", node.id))?;
    output.get(port.name.as_deref().unwrap_or(port_id))
        .with_context(|| format!("DataParse {} did not return {port_id}", node.id))
}

fn resolve_input_f64_inner(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    port_id: &str,
    cache: &mut HashMap<(String, String), f64>,
    visiting: &mut HashSet<(String, String)>,
) -> Result<Option<f64>> {
    if let Some(conn) = incoming_connection(scene, node_id, port_id) {
        let v = resolve_output_f64_inner(
            scene,
            nodes_by_id,
            &conn.from.node_id,
            &conn.from.port_id,
            cache,
            visiting,
        )?;
        return Ok(Some(v));
    }

    let node = find_node(nodes_by_id, node_id)?;
    if let Some(v) = parse_f64(&node.params, port_id) {
        return Ok(crate::input_units::inline_value(node, port_id, &serde_json::json!(v), scene.dpi)?.as_f64());
    }

    let fallback = schema::load_default_scheme()
        .ok()
        .and_then(|scheme| {
            schema::input_port_default_value(&scheme, &node.node_type, port_id).cloned()
        })
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_i64().map(|x| x as f64))
                .or_else(|| v.as_u64().map(|x| x as f64))
        });
    fallback.map(|value| crate::input_units::inline_value(node, port_id, &serde_json::json!(value), scene.dpi).map(|value| value.as_f64())).transpose().map(Option::flatten)
}

fn resolve_output_f64_inner(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    out_port: &str,
    cache: &mut HashMap<(String, String), f64>,
    visiting: &mut HashSet<(String, String)>,
) -> Result<f64> {
    let key = (node_id.to_string(), out_port.to_string());
    if let Some(v) = cache.get(&key) {
        return Ok(*v);
    }
    if visiting.contains(&key) {
        bail!(
            "cycle detected while resolving scalar value at {}.{}",
            node_id,
            out_port
        );
    }
    visiting.insert(key.clone());

    let node = find_node(nodes_by_id, node_id)?;

    let computed = match node.node_type.as_str() {
        "BoolInput" => {
            if node
                .params
                .get("value")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                1.0
            } else {
                0.0
            }
        }
        "FloatInput" | "IntInput" => resolve_input_f64_inner(scene, nodes_by_id, node_id, "value", cache, visiting)?.unwrap_or(0.0),
        "MidiInput" => {
            if out_port != "value" {
                bail!("unsupported MidiInput output port: {out_port}");
            }
            resolve_midi_input_f64(&node.params)
        }
        "MathAdd" => {
            if out_port != "result" {
                bail!("unsupported MathAdd output port: {out_port}");
            }

            let mut input_port_ids: Vec<&str> = Vec::new();
            if !node.inputs.is_empty() {
                input_port_ids.extend(node.inputs.iter().map(|p| p.id.as_str()));
            } else {
                input_port_ids.extend(["a", "b"]);
            }

            let mut values: Vec<f64> = Vec::new();
            for port_id in input_port_ids {
                if let Some(conn) = incoming_connection(scene, node_id, port_id) {
                    let v = resolve_output_f64_inner(
                        scene,
                        nodes_by_id,
                        &conn.from.node_id,
                        &conn.from.port_id,
                        cache,
                        visiting,
                    )?;
                    values.push(v);
                } else if node.params.contains_key(port_id) {
                    if let Some(value) = resolve_input_f64_inner(scene, nodes_by_id, node_id, port_id, cache, visiting)? {
                        values.push(value);
                    }
                }
            }

            if values.len() < 2 {
                0.0
            } else {
                values.into_iter().fold(0.0, |acc, v| acc + v)
            }
        }
        "MathSubtract" => {
            if out_port != "result" {
                bail!("unsupported MathSubtract output port: {out_port}");
            }

            let mut input_port_ids: Vec<&str> = Vec::new();
            if !node.inputs.is_empty() {
                input_port_ids.extend(node.inputs.iter().map(|p| p.id.as_str()));
            } else {
                input_port_ids.extend(["a", "b"]);
            }

            let mut values: Vec<f64> = Vec::new();
            for port_id in input_port_ids {
                if let Some(conn) = incoming_connection(scene, node_id, port_id) {
                    let v = resolve_output_f64_inner(
                        scene,
                        nodes_by_id,
                        &conn.from.node_id,
                        &conn.from.port_id,
                        cache,
                        visiting,
                    )?;
                    values.push(v);
                } else if node.params.contains_key(port_id) {
                    if let Some(value) = resolve_input_f64_inner(scene, nodes_by_id, node_id, port_id, cache, visiting)? {
                        values.push(value);
                    }
                }
            }

            if values.len() < 2 {
                0.0
            } else {
                let mut iter = values.into_iter();
                let first = iter.next().unwrap_or(0.0);
                iter.fold(first, |acc, v| acc - v)
            }
        }
        "MathMultiply" => {
            if out_port != "result" {
                bail!("unsupported MathMultiply output port: {out_port}");
            }

            // Multiply all connected inputs (dynamic ports are stored in `node.inputs`).
            // For scalar evaluation we only support numeric inputs; missing/unknown inputs resolve
            // to 0.0, matching the existing behavior for unconnected math nodes.
            let mut input_port_ids: Vec<&str> = Vec::new();
            if !node.inputs.is_empty() {
                input_port_ids.extend(node.inputs.iter().map(|p| p.id.as_str()));
            } else {
                // Back-compat: older graphs used fixed a/b (and sometimes x/y aliases).
                input_port_ids.extend(["a", "b"]);
            }

            let mut values: Vec<f64> = Vec::new();
            for port_id in input_port_ids {
                if let Some(conn) = incoming_connection(scene, node_id, port_id) {
                    let v = resolve_output_f64_inner(
                        scene,
                        nodes_by_id,
                        &conn.from.node_id,
                        &conn.from.port_id,
                        cache,
                        visiting,
                    )?;
                    values.push(v);
                } else if node.params.contains_key(port_id) {
                    if let Some(value) = resolve_input_f64_inner(scene, nodes_by_id, node_id, port_id, cache, visiting)? {
                        values.push(value);
                    }
                }
            }

            if values.len() < 2 {
                // keep behavior consistent with shader path: unknown output when not enough inputs
                0.0
            } else {
                values.into_iter().fold(1.0, |acc, v| acc * v)
            }
        }
        "MathDivide" => {
            if out_port != "result" {
                bail!("unsupported MathDivide output port: {out_port}");
            }

            let mut input_port_ids: Vec<&str> = Vec::new();
            if !node.inputs.is_empty() {
                input_port_ids.extend(node.inputs.iter().map(|p| p.id.as_str()));
            } else {
                input_port_ids.extend(["a", "b"]);
            }

            let mut values: Vec<f64> = Vec::new();
            for port_id in input_port_ids {
                if let Some(conn) = incoming_connection(scene, node_id, port_id) {
                    let v = resolve_output_f64_inner(
                        scene,
                        nodes_by_id,
                        &conn.from.node_id,
                        &conn.from.port_id,
                        cache,
                        visiting,
                    )?;
                    values.push(v);
                } else if node.params.contains_key(port_id) {
                    if let Some(value) = resolve_input_f64_inner(scene, nodes_by_id, node_id, port_id, cache, visiting)? {
                        values.push(value);
                    }
                }
            }

            if values.len() < 2 {
                0.0
            } else {
                let mut iter = values.into_iter();
                let first = iter.next().unwrap_or(0.0);
                iter.fold(first, |acc, v| acc / v)
            }
        }
        "MathClamp" => {
            if out_port != "result" {
                bail!("unsupported MathClamp output port: {out_port}");
            }
            let v = resolve_input_f64_inner(scene, nodes_by_id, node_id, "value", cache, visiting)?
                .unwrap_or(0.5);
            let lo = resolve_input_f64_inner(scene, nodes_by_id, node_id, "min", cache, visiting)?
                .unwrap_or(0.0);
            let hi = resolve_input_f64_inner(scene, nodes_by_id, node_id, "max", cache, visiting)?
                .unwrap_or(1.0);
            v.clamp(lo.min(hi), lo.max(hi))
        }
        "MathPower" => {
            if out_port != "result" {
                bail!("unsupported MathPower output port: {out_port}");
            }
            let base =
                resolve_input_f64_inner(scene, nodes_by_id, node_id, "base", cache, visiting)?
                    .unwrap_or(2.0);
            let exp =
                resolve_input_f64_inner(scene, nodes_by_id, node_id, "exponent", cache, visiting)?
                    .unwrap_or(2.0);
            base.powf(exp)
        }
        "MathClosure" => {
            bail!(
                "MathClosure cannot be evaluated on CPU for scalar resolution (node={node_id}). \n\
This node contains user-provided source code; render-time evaluation must be sandboxed and is not implemented."
            )
        }
        "ResourcePool" => {
            let dynamic_inputs: Vec<&str> = node
                .inputs
                .iter()
                .filter(|p| p.id != "selectedIndex")
                .map(|p| p.id.as_str())
                .collect();

            if dynamic_inputs.is_empty() {
                0.0
            } else {
                let idx = resolve_input_f64_inner(
                    scene,
                    nodes_by_id,
                    node_id,
                    "selectedIndex",
                    cache,
                    visiting,
                )?
                .unwrap_or(0.0)
                .max(0.0) as usize;
                let idx = idx.min(dynamic_inputs.len() - 1);
                let selected_port = dynamic_inputs[idx];

                if let Some(conn) = incoming_connection(scene, node_id, selected_port) {
                    resolve_output_f64_inner(
                        scene,
                        nodes_by_id,
                        &conn.from.node_id,
                        &conn.from.port_id,
                        cache,
                        visiting,
                    )?
                } else {
                    0.0
                }
            }
        }
        "DataParse" => {
            let output = evaluate_data_parse_inner(
                scene, nodes_by_id, node, 0,
                &mut crate::renderer::scene_prep::data_parse_runtime::TsRuntime::new(),
                cache, visiting,
            )?;
            data_parse_output(node, &output, out_port)?.as_f64()
                .with_context(|| format!("DataParse {node_id}.{out_port} must return a number"))?
        }
        // For scalar parameter resolution, allow a fallback where the output port is stored in params.
        // (Useful for very simple constant nodes or hand-written DSL.)
        _ => parse_f64(&node.params, out_port).ok_or_else(|| {
            anyhow!(
                "unsupported scalar node/output: {}.{} ({})",
                node_id,
                out_port,
                node.node_type
            )
        })?,
    };

    visiting.remove(&key);
    cache.insert(key, computed);
    Ok(computed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_scene_asset_matches_current_schema() {
        let scene = load_scene_from_default_asset().expect("default scene should load");
        assert_eq!(scene.version, "6.1");
    }

    #[test]
    fn node_title_label_round_trips_as_top_level_metadata() {
        let node: Node = serde_json::from_value(json!({
            "id": "RenderPass_1",
            "type": "RenderPass",
            "label": "Beauty Pass",
            "params": { "name": "Render Pass" }
        }))
        .expect("node should deserialize");

        assert_eq!(
            node.params
                .get("__node_label")
                .and_then(|value| value.as_str()),
            Some("Beauty Pass")
        );
        assert_eq!(node_display_label(&node), "Beauty Pass");

        let serialized = serde_json::to_value(&node).expect("node should serialize");
        assert_eq!(serialized.get("label"), Some(&json!("Beauty Pass")));
        assert_eq!(
            serialized
                .get("params")
                .and_then(|params| params.get("__node_label")),
            None
        );
    }

    #[test]
    fn parse_texture_format_accepts_rgba16float_variants() {
        let mut params = HashMap::new();

        params.insert("format".to_string(), json!("rgba16float"));
        assert_eq!(
            parse_texture_format(&params).unwrap(),
            TextureFormat::Rgba16Float
        );

        params.insert("format".to_string(), json!("RGBA16FLOAT"));
        assert_eq!(
            parse_texture_format(&params).unwrap(),
            TextureFormat::Rgba16Float
        );

        params.insert("format".to_string(), json!("rgba16-float"));
        assert_eq!(
            parse_texture_format(&params).unwrap(),
            TextureFormat::Rgba16Float
        );

        params.insert("format".to_string(), json!("rgba16_float"));
        assert_eq!(
            parse_texture_format(&params).unwrap(),
            TextureFormat::Rgba16Float
        );
    }

    #[test]
    fn parse_texture_format_rejects_unsupported_format() {
        let params = HashMap::from([("format".to_string(), json!("rgb16float"))]);
        let err = parse_texture_format(&params).unwrap_err().to_string();
        assert!(err.contains("unsupported RenderTexture.format"));
        assert!(err.contains("rgb16float"));
    }

    #[test]
    fn data_parse_scalar_reads_edges_and_rejects_cycles_and_script_errors() {
        let mut scene: SceneDSL = serde_json::from_value(json!({
            "version":"6.1", "workingColorSpace": "linear-srgb", "dpi": 3.0, "metadata":{"name":"CPU layout"}, "nodes":[
                {"id":"width","type":"IntInput","params":{"value":1279}},
                {"id":"enabled","type":"BoolInput","params":{"value":true}},
                {"id":"layout","type":"DataParse", "inputs":[{"id":"w","type":"float"},{"id":"enabled","type":"bool"}],
                    "inputBindings":[{"portId":"w","variableName":"width","type":"float"},
                        {"portId":"enabled","variableName":"enabled","type":"bool"}],
                    "outputs":[{"id":"x","name":"center","type":"float"}],
                    "params":{"source":"return { center: enabled === true ? width * 0.5 : -1 };"}},
                {"id":"vector","type":"Vector2Input"}
            ], "connections":[
                {"id":"w","from":{"nodeId":"width","portId":"value"},"to":{"nodeId":"layout","portId":"w"}},
                {"id":"enabled","from":{"nodeId":"enabled","portId":"value"},"to":{"nodeId":"layout","portId":"enabled"}},
                {"id":"x","from":{"nodeId":"layout","portId":"x"},"to":{"nodeId":"vector","portId":"x"}}
            ], "outputs":null
        })).unwrap();
        let resolve = |scene: &SceneDSL| {
            let nodes = scene.nodes.iter().map(|node| (node.id.clone(), node.clone())).collect();
            resolve_input_f64(scene, &nodes, "vector", "x")
        };
        assert_eq!(resolve(&scene).unwrap(), Some(639.5));
        scene.connections[0].from = Endpoint { node_id: "layout".into(), port_id: "x".into() };
        assert!(format!("{:#}", resolve(&scene).unwrap_err()).contains("cycle detected"));
        scene.connections[0].from = Endpoint { node_id: "width".into(), port_id: "value".into() };
        scene.nodes[2].params.insert("source".into(), json!("throw new Error('invalid layout');"));
        assert!(format!("{:#}", resolve(&scene).unwrap_err()).contains("invalid layout"));
    }
}
