use std::collections::HashMap;

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;

use crate::dsl::{parse_f32, parse_u32, parse_texture_format, Connection, Node, SceneDSL};

const DEFAULT_NODE_SCHEME_JSON: &str = include_str!("../assets/node-scheme.json");

#[derive(Debug, Clone)]
pub struct NodeScheme {
    pub nodes: HashMap<String, NodeTypeScheme>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawNodeScheme {
    Legacy(LegacyNodeScheme),
    Generated(GeneratedNodeScheme),
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyNodeScheme {
    #[allow(dead_code)]
    pub version: String,
    pub nodes: HashMap<String, NodeTypeScheme>,
}

#[derive(Debug, Clone, Deserialize)]
struct GeneratedNodeScheme {
    #[serde(rename = "schemaVersion")]
    #[allow(dead_code)]
    pub schema_version: u32,
    #[serde(rename = "generatedAt")]
    #[allow(dead_code)]
    pub generated_at: Option<String>,
    #[serde(default)]
    pub nodes: Vec<GeneratedNodeDef>,
}

#[derive(Debug, Clone, Deserialize)]
struct GeneratedNodeDef {
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(default)]
    pub inputs: Vec<GeneratedPort>,
    #[serde(default)]
    pub outputs: Vec<GeneratedPort>,
}

#[derive(Debug, Clone, Deserialize)]
struct GeneratedPort {
    pub id: String,
    #[serde(rename = "type")]
    pub port_type: PortTypeSpec,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeTypeScheme {
    #[serde(default)]
    pub inputs: HashMap<String, PortTypeSpec>,
    #[serde(default)]
    pub outputs: HashMap<String, PortTypeSpec>,
    #[serde(default)]
    pub params: HashMap<String, ParamScheme>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    Float,
    F32,
    Int,
    U32,
    String,
    TextureFormat,
    Json,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ParamScheme {
    #[serde(rename = "type")]
    pub ty: ParamType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PortTypeSpec {
    One(String),
    Many(Vec<String>),
}

impl PortTypeSpec {
    fn overlaps(&self, other: &PortTypeSpec) -> bool {
        match (self, other) {
            (PortTypeSpec::One(a), PortTypeSpec::One(b)) => a == b,
            (PortTypeSpec::One(a), PortTypeSpec::Many(bs)) => bs.iter().any(|b| b == a),
            (PortTypeSpec::Many(as_), PortTypeSpec::One(b)) => as_.iter().any(|a| a == b),
            (PortTypeSpec::Many(as_), PortTypeSpec::Many(bs)) => {
                as_.iter().any(|a| bs.iter().any(|b| b == a))
            }
        }
    }
}

pub fn load_default_scheme() -> Result<NodeScheme> {
    let scheme: RawNodeScheme = serde_json::from_str(DEFAULT_NODE_SCHEME_JSON)
        .map_err(|e| anyhow!("failed to parse assets/node-scheme.json: {e}"))?;
    Ok(match scheme {
        RawNodeScheme::Legacy(s) => NodeScheme { nodes: s.nodes },
        RawNodeScheme::Generated(s) => {
            let mut nodes: HashMap<String, NodeTypeScheme> = HashMap::new();
            for n in s.nodes {
                let inputs: HashMap<String, PortTypeSpec> = n
                    .inputs
                    .into_iter()
                    .map(|p| (p.id, p.port_type))
                    .collect();
                let outputs: HashMap<String, PortTypeSpec> = n
                    .outputs
                    .into_iter()
                    .map(|p| (p.id, p.port_type))
                    .collect();
                nodes.insert(
                    n.node_type,
                    NodeTypeScheme {
                        inputs,
                        outputs,
                        params: HashMap::new(),
                    },
                );
            }
            NodeScheme { nodes }
        }
    })
}

pub fn validate_scene(scene: &SceneDSL) -> Result<()> {
    let scheme = load_default_scheme()?;
    validate_scene_against(scene, &scheme)
}

pub fn validate_scene_against(scene: &SceneDSL, scheme: &NodeScheme) -> Result<()> {
    let mut nodes_by_id: HashMap<&str, &Node> = HashMap::new();
    for n in &scene.nodes {
        nodes_by_id.insert(n.id.as_str(), n);
    }

    let mut errors: Vec<String> = Vec::new();

    for n in &scene.nodes {
        let Some(node_scheme) = scheme.nodes.get(&n.node_type) else {
            errors.push(format!("unknown node type '{}' at node '{}'", n.node_type, n.id));
            continue;
        };

        // Required params + basic type checking.
        for (param_name, param_scheme) in &node_scheme.params {
            if !param_scheme.required {
                continue;
            }
            if !n.params.contains_key(param_name) {
                errors.push(format!(
                    "missing required param '{}.{}' (type {:?})",
                    n.id, param_name, param_scheme.ty
                ));
            }
        }

        for (k, v) in &n.params {
            let Some(param_scheme) = node_scheme.params.get(k) else {
                // Forward-compatible: ignore unknown params (node editor may add extra metadata).
                continue;
            };
            if let Err(msg) = validate_param_value(n, k, v, param_scheme.ty) {
                errors.push(msg);
            }
        }
    }

    for c in &scene.connections {
        validate_connection(c, &nodes_by_id, scheme, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        bail!("scene failed scheme validation ({} error(s)):\n- {}", errors.len(), errors.join("\n- "))
    }
}

fn validate_param_value(
    node: &Node,
    key: &str,
    value: &serde_json::Value,
    ty: ParamType,
) -> std::result::Result<(), String> {
    let ok = match ty {
        ParamType::Float | ParamType::F32 => parse_f32(&node.params, key).is_some(),
        ParamType::Int => value.as_i64().is_some() || value.as_u64().is_some(),
        ParamType::U32 => parse_u32(&node.params, key).is_some(),
        ParamType::String => value.as_str().is_some(),
        ParamType::TextureFormat => {
            // Reuse existing parser for supported values.
            parse_texture_format(&node.params).is_ok()
        }
        ParamType::Json => true,
    };

    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid param type for '{}.{}': expected {:?}, got {}",
            node.id,
            key,
            ty,
            value
        ))
    }
}

fn validate_connection(
    c: &Connection,
    nodes_by_id: &HashMap<&str, &Node>,
    scheme: &NodeScheme,
    errors: &mut Vec<String>,
) {
    let Some(from_node) = nodes_by_id.get(c.from.node_id.as_str()).copied() else {
        errors.push(format!("connection '{}' references missing from.nodeId '{}'", c.id, c.from.node_id));
        return;
    };
    let Some(to_node) = nodes_by_id.get(c.to.node_id.as_str()).copied() else {
        errors.push(format!("connection '{}' references missing to.nodeId '{}'", c.id, c.to.node_id));
        return;
    };

    let Some(from_scheme) = scheme.nodes.get(&from_node.node_type) else {
        // Unknown node type already reported in node loop.
        return;
    };
    let Some(to_scheme) = scheme.nodes.get(&to_node.node_type) else {
        return;
    };

    let Some(from_ty) = from_scheme.outputs.get(&c.from.port_id) else {
        errors.push(format!(
            "connection '{}' uses unknown from port '{}.{}' (type {})",
            c.id, c.from.node_id, c.from.port_id, from_node.node_type
        ));
        return;
    };

    let Some(to_ty) = to_scheme.inputs.get(&c.to.port_id) else {
        errors.push(format!(
            "connection '{}' uses unknown to port '{}.{}' (type {})",
            c.id, c.to.node_id, c.to.port_id, to_node.node_type
        ));
        return;
    };

    if !from_ty.overlaps(to_ty) {
        errors.push(format!(
            "connection '{}' type mismatch: '{}.{}' ({}) -> '{}.{}' ({})",
            c.id,
            c.from.node_id,
            c.from.port_id,
            port_type_spec_to_string(from_ty),
            c.to.node_id,
            c.to.port_id,
            port_type_spec_to_string(to_ty)
        ));
    }
}

fn port_type_spec_to_string(t: &PortTypeSpec) -> String {
    match t {
        PortTypeSpec::One(s) => s.clone(),
        PortTypeSpec::Many(v) => format!("[{}]", v.join(", ")),
    }
}
