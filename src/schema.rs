use std::{borrow::Cow, collections::HashMap};

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;

use crate::dsl::{Connection, Node, SceneDSL, parse_f32, parse_texture_format, parse_u32};

const DEFAULT_NODE_SCHEME_JSON: &str = include_str!("../assets/node-scheme.json");

#[derive(Debug, Clone)]
pub struct NodeScheme {
    pub nodes: HashMap<String, NodeTypeScheme>,
    pub port_type_compatibility: HashMap<String, Vec<String>>,
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
    #[serde(rename = "portTypeCompatibility", default)]
    pub port_type_compatibility: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub nodes: Vec<GeneratedNodeDef>,
}

#[derive(Debug, Clone, Deserialize)]
struct GeneratedNodeDef {
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub inputs: Vec<GeneratedPort>,
    #[serde(default)]
    pub outputs: Vec<GeneratedPort>,
    #[serde(rename = "defaultParams", default)]
    pub default_params: HashMap<String, serde_json::Value>,
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
    pub category: Option<String>,
    #[serde(default)]
    pub inputs: HashMap<String, PortTypeSpec>,
    #[serde(default)]
    pub outputs: HashMap<String, PortTypeSpec>,
    #[serde(default)]
    pub default_params: HashMap<String, serde_json::Value>,
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
    // Intentionally no helper methods here: all connectability is driven by
    // the editor-exported `portTypeCompatibility` table.
}

fn normalize_port_type_name(t: &str) -> &str {
    match t {
        // Common aliases used by some editors/schemes.
        "vec2" => "vector2",
        "vec3" => "vector3",
        "vec4" => "vector4",
        other => other,
    }
}

fn port_types_compatible_via_table(
    from: &PortTypeSpec,
    to: &PortTypeSpec,
    port_type_compatibility: &HashMap<String, Vec<String>>,
) -> bool {
    // No legacy fallback: generated schemes must provide a compatibility table.
    if port_type_compatibility.is_empty() {
        return false;
    }

    // Compatibility table is keyed by *destination/input* type.
    // Example: "float": ["float", "int"] means a float input can accept int outputs.
    fn atomic_compatible(from_ty: &str, to_ty: &str, table: &HashMap<String, Vec<String>>) -> bool {
        let from_ty = normalize_port_type_name(from_ty);
        let to_ty = normalize_port_type_name(to_ty);

        // `any` is a wildcard on either side.
        if from_ty == "any" || to_ty == "any" {
            return true;
        }

        if from_ty == to_ty {
            return true;
        }

        table.get(to_ty).is_some_and(|allowed| {
            allowed
                .iter()
                .any(|s| normalize_port_type_name(s) == from_ty)
        })
    }

    match (from, to) {
        (PortTypeSpec::One(a), PortTypeSpec::One(b)) => {
            atomic_compatible(a, b, port_type_compatibility)
        }
        (PortTypeSpec::One(a), PortTypeSpec::Many(bs)) => bs
            .iter()
            .any(|b| atomic_compatible(a, b, port_type_compatibility)),
        (PortTypeSpec::Many(as_), PortTypeSpec::One(b)) => as_
            .iter()
            .any(|a| atomic_compatible(a, b, port_type_compatibility)),
        (PortTypeSpec::Many(as_), PortTypeSpec::Many(bs)) => as_.iter().any(|a| {
            bs.iter()
                .any(|b| atomic_compatible(a, b, port_type_compatibility))
        }),
    }
}

/// Shared port type compatibility check used by both DSL validation and runtime helpers.
///
/// Semantics:
/// - Compatibility is driven by the editor-exported `portTypeCompatibility` table,
///   interpreted as **input type -> allowed output types**.
/// - `any` is treated as a wildcard on either side.
/// - There is intentionally no legacy fallback: if the table is missing/empty, we reject.
pub(crate) fn port_types_compatible(
    scheme: &NodeScheme,
    from: &PortTypeSpec,
    to: &PortTypeSpec,
) -> bool {
    port_types_compatible_via_table(from, to, &scheme.port_type_compatibility)
}

pub fn load_default_scheme() -> Result<NodeScheme> {
    let scheme: RawNodeScheme = serde_json::from_str(DEFAULT_NODE_SCHEME_JSON)
        .map_err(|e| anyhow!("failed to parse assets/node-scheme.json: {e}"))?;
    Ok(match scheme {
        RawNodeScheme::Legacy(s) => NodeScheme {
            nodes: s.nodes,
            port_type_compatibility: HashMap::new(),
        },
        RawNodeScheme::Generated(s) => {
            let mut nodes: HashMap<String, NodeTypeScheme> = HashMap::new();
            for n in s.nodes {
                let inputs: HashMap<String, PortTypeSpec> =
                    n.inputs.into_iter().map(|p| (p.id, p.port_type)).collect();
                let outputs: HashMap<String, PortTypeSpec> =
                    n.outputs.into_iter().map(|p| (p.id, p.port_type)).collect();
                nodes.insert(
                    n.node_type,
                    NodeTypeScheme {
                        category: n.category,
                        inputs,
                        outputs,
                        default_params: n.default_params,
                        params: HashMap::new(),
                    },
                );
            }
            NodeScheme {
                nodes,
                port_type_compatibility: s.port_type_compatibility,
            }
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
            errors.push(format!(
                "unknown node type '{}' at node '{}'",
                n.node_type, n.id
            ));
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
        bail!(
            "scene failed scheme validation ({} error(s)):\n- {}",
            errors.len(),
            errors.join("\n- ")
        )
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
            node.id, key, ty, value
        ))
    }
}

fn validate_connection(
    c: &Connection,
    nodes_by_id: &HashMap<&str, &Node>,
    scheme: &NodeScheme,
    errors: &mut Vec<String>,
) {
    fn math_closure_input_port_type(node: &Node, port_id: &str) -> Option<PortTypeSpec> {
        let p = node.inputs.iter().find(|p| p.id == port_id)?;
        let ty = p.port_type.as_ref()?;
        Some(PortTypeSpec::One(ty.clone()))
    }

    fn math_multiply_output_port_type(node: &Node, port_id: &str) -> Option<PortTypeSpec> {
        let p = node.outputs.iter().find(|p| p.id == port_id)?;
        let ty = p.port_type.as_ref()?;
        Some(PortTypeSpec::One(ty.clone()))
    }

    let Some(from_node) = nodes_by_id.get(c.from.node_id.as_str()).copied() else {
        errors.push(format!(
            "connection '{}' references missing from.nodeId '{}'",
            c.id, c.from.node_id
        ));
        return;
    };
    let Some(to_node) = nodes_by_id.get(c.to.node_id.as_str()).copied() else {
        errors.push(format!(
            "connection '{}' references missing to.nodeId '{}'",
            c.id, c.to.node_id
        ));
        return;
    };

    let Some(from_scheme) = scheme.nodes.get(&from_node.node_type) else {
        // Unknown node type already reported in node loop.
        return;
    };
    let Some(to_scheme) = scheme.nodes.get(&to_node.node_type) else {
        return;
    };

    let from_ty: Cow<'_, PortTypeSpec> = if from_node.node_type == "MathMultiply" {
        // MathMultiply output is instance-defined: editor now exports inferred output type in
        // node.outputs[0].type (at least for the `result` port).
        // If missing, fall back to scheme (usually `any`).
        if let Some(spec) = math_multiply_output_port_type(from_node, &c.from.port_id) {
            Cow::Owned(spec)
        } else if let Some(t) = from_scheme.outputs.get(&c.from.port_id) {
            Cow::Borrowed(t)
        } else {
            errors.push(format!(
                "connection '{}' uses unknown from port '{}.{}' (type {})",
                c.id, c.from.node_id, c.from.port_id, from_node.node_type
            ));
            return;
        }
    } else if let Some(t) = from_scheme.outputs.get(&c.from.port_id) {
        Cow::Borrowed(t)
    } else {
        errors.push(format!(
            "connection '{}' uses unknown from port '{}.{}' (type {})",
            c.id, c.from.node_id, c.from.port_id, from_node.node_type
        ));
        return;
    };

    let to_ty: Cow<'_, PortTypeSpec> = if let Some(t) = to_scheme.inputs.get(&c.to.port_id) {
        Cow::Borrowed(t)
    } else if to_node.node_type == "Composite" && c.to.port_id.starts_with("dynamic_") {
        // Composite supports dynamic layer inputs (dynamic_*) that behave like its base pass input.
        // These ports are instance-defined so they won't appear in the static scheme.
        match to_scheme.inputs.get("pass") {
            Some(pass_ty) => Cow::Borrowed(pass_ty),
            None => Cow::Owned(PortTypeSpec::One("pass".to_string())),
        }
    } else if to_node.node_type == "MathClosure" {
        // MathClosure inputs are instance-defined (node.inputs in the DSL export).
        if let Some(spec) = math_closure_input_port_type(to_node, &c.to.port_id) {
            Cow::Owned(spec)
        } else {
            errors.push(format!(
                "connection '{}' uses unknown to port '{}.{}' (type {})",
                c.id, c.to.node_id, c.to.port_id, to_node.node_type
            ));
            return;
        }
    } else if to_node.node_type == "MathMultiply" {
        // MathMultiply inputs are instance-defined (node.inputs in the DSL export).
        // If the node didn't include `inputs`, accept any incoming connections.
        if let Some(p) = to_node.inputs.iter().find(|p| p.id == c.to.port_id) {
            if let Some(ty) = p.port_type.as_ref() {
                Cow::Owned(PortTypeSpec::One(ty.clone()))
            } else {
                Cow::Owned(PortTypeSpec::One("any".to_string()))
            }
        } else {
            Cow::Owned(PortTypeSpec::One("any".to_string()))
        }
    } else {
        errors.push(format!(
            "connection '{}' uses unknown to port '{}.{}' (type {})",
            c.id, c.to.node_id, c.to.port_id, to_node.node_type
        ));
        return;
    };

    let compatible = port_types_compatible(scheme, from_ty.as_ref(), to_ty.as_ref());

    if !compatible {
        errors.push(format!(
            "connection '{}' type mismatch: '{}.{}' ({}) -> '{}.{}' ({})",
            c.id,
            c.from.node_id,
            c.from.port_id,
            port_type_spec_to_string(from_ty.as_ref()),
            c.to.node_id,
            c.to.port_id,
            port_type_spec_to_string(to_ty.as_ref())
        ));
    }
}

fn port_type_spec_to_string(t: &PortTypeSpec) -> String {
    match t {
        PortTypeSpec::One(s) => s.clone(),
        PortTypeSpec::Many(v) => format!("[{}]", v.join(", ")),
    }
}
