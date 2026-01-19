use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow, bail};
use rust_wgpu_fiber::eframe::wgpu::TextureFormat;
use serde::{Deserialize, Serialize};

use crate::schema;

#[derive(Debug, Clone)]
pub struct FileRenderTarget {
    pub directory: String,
    pub file_name: String,
}

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

    // Optional editor metadata used for ordering / UI.
    #[serde(default)]
    pub inputs: Vec<NodePort>,
    #[serde(default)]
    pub outputs: Vec<NodePort>,

    #[serde(default, rename = "inputBindings")]
    pub input_bindings: Vec<InputBinding>,
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
    let mut scene: SceneDSL = serde_json::from_str(&text).context("failed to parse DSL json")?;

    // Normalize params with defaults from the bundled node scheme.
    // This keeps older/hand-written DSL compatible when nodes omit parameters.
    normalize_scene_defaults(&mut scene)?;

    Ok(scene)
}

pub fn normalize_scene_defaults(scene: &mut SceneDSL) -> Result<()> {
    let scheme = schema::load_default_scheme()?;
    apply_node_default_params(scene, &scheme);
    Ok(())
}

fn apply_node_default_params(scene: &mut SceneDSL, scheme: &schema::NodeScheme) {
    for node in &mut scene.nodes {
        let Some(node_scheme) = scheme.nodes.get(&node.node_type) else {
            continue;
        };
        if node_scheme.default_params.is_empty() {
            continue;
        }

        let mut merged = node_scheme.default_params.clone();
        for (k, v) in std::mem::take(&mut node.params) {
            merged.insert(k, v);
        }
        node.params = merged;
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
    let nodes_by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|n| (n.id.clone(), n))
        .collect();

    let screen = scene.nodes.iter().find(|n| n.node_type == "Screen")?;

    // Prefer graph-driven values (incoming connections), fallback to inline params.
    let w = resolve_input_u32(scene, &nodes_by_id, &screen.id, "width")
        .ok()
        .flatten()
        .or_else(|| parse_u32(&screen.params, "width"))
        .or_else(|| {
            parse_f32(&screen.params, "width").and_then(|v| {
                if v.is_finite() {
                    Some(v.max(0.0).floor() as u32)
                } else {
                    None
                }
            })
        })?;
    let h = resolve_input_u32(scene, &nodes_by_id, &screen.id, "height")
        .ok()
        .flatten()
        .or_else(|| parse_u32(&screen.params, "height"))
        .or_else(|| {
            parse_f32(&screen.params, "height").and_then(|v| {
                if v.is_finite() {
                    Some(v.max(0.0).floor() as u32)
                } else {
                    None
                }
            })
        })?;
    if w == 0 || h == 0 {
        return None;
    }
    Some([w, h])
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
///
/// This is intentionally minimal: it only implements the node subset needed for
/// CPU-side parameter resolution (e.g. RenderTexture.width/height).
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

fn eval_data_parse_scalar(
    _scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    out_port: &str,
) -> Result<f64> {
    let src = node
        .params
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if src.trim().is_empty() {
        return Ok(0.0);
    }

    let mut bindings_src = String::new();
    for b in &node.input_bindings {
        let val = match b
            .source_binding
            .as_ref()
            .map(|sb| sb.output_port_id.as_str())
        {
            Some("data") => {
                let data_node_id = b
                    .source_binding
                    .as_ref()
                    .map(|sb| sb.node_id.as_str())
                    .unwrap_or("");
                let data_node = find_node(nodes_by_id, data_node_id)?;
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
            Some("index") => serde_json::json!(0u32),
            _ => serde_json::Value::Null,
        };

        let json = serde_json::to_string(&val)?;
        bindings_src.push_str(&format!("const {} = {};\n", b.variable_name, json));
    }

    if !node
        .input_bindings
        .iter()
        .any(|b| b.variable_name == "index")
    {
        bindings_src.push_str("const index = 0;\n");
    }

    let user_src = strip_data_parse_type_assertions(src);

    let script_body = format!("{bindings_src}\n{user_src}\n");
    let script = format!("(function() {{\n{}\n}})()", script_body);

    let mut rt = crate::ts_runtime::TsRuntime::new();
    let out: serde_json::Value = rt
        .eval_script(&script)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    let out_obj = out.as_object();

    let out_value = node
        .outputs
        .iter()
        .find(|p| p.id == out_port)
        .and_then(|p| p.name.as_deref())
        .and_then(|name| out_obj.and_then(|o| o.get(name)))
        .or_else(|| out_obj.and_then(|o| o.get(out_port)))
        .unwrap_or(&serde_json::Value::Null);

    Ok(out_value.as_f64().unwrap_or(0.0))
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
    Ok(parse_f64(&node.params, port_id))
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
        "FloatInput" | "IntInput" => parse_f64(&node.params, "value").unwrap_or(0.0),
        "MathAdd" => {
            if out_port != "result" {
                bail!("unsupported MathAdd output port: {out_port}");
            }
            let a = resolve_input_f64_inner(scene, nodes_by_id, node_id, "a", cache, visiting)?
                .unwrap_or(0.0);
            let b = resolve_input_f64_inner(scene, nodes_by_id, node_id, "b", cache, visiting)?
                .unwrap_or(0.0);
            a + b
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
                }
            }

            if values.len() < 2 {
                // keep behavior consistent with shader path: unknown output when not enough inputs
                0.0
            } else {
                values.into_iter().fold(1.0, |acc, v| acc * v)
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
        "DataParse" => eval_data_parse_scalar(scene, nodes_by_id, node, out_port).unwrap_or(0.0),
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
