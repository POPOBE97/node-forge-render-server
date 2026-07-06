use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::dsl::{Connection, GroupDSL, InputBinding, Node, NodePort, SceneDSL, resolve_input_f32};
use crate::renderer::types::{
    GraphBindingKind, GraphField, GraphFieldKind, GraphSchema, PassBindings,
};
use crate::renderer::utils::sanitize_wgsl_ident;

pub fn graph_field_kind_for_node_type(node_type: &str) -> Option<GraphFieldKind> {
    match node_type {
        "FloatInput" => Some(GraphFieldKind::F32),
        "MidiInput" => Some(GraphFieldKind::F32),
        "IntInput" => Some(GraphFieldKind::I32),
        "BoolInput" => Some(GraphFieldKind::Bool),
        "Vector2Input" => Some(GraphFieldKind::Vec2),
        "Vector3Input" => Some(GraphFieldKind::Vec3),
        "Vector4Input" => Some(GraphFieldKind::Vec4),
        "ColorInput" => Some(GraphFieldKind::Vec4Color),
        _ => None,
    }
}

pub fn graph_field_name(node_id: &str) -> String {
    let base = sanitize_wgsl_ident(node_id);
    let hash = hash_bytes(node_id.as_bytes());
    let suffix = format!(
        "{:02x}{:02x}{:02x}{:02x}",
        hash[0], hash[1], hash[2], hash[3]
    );
    format!("node_{}_{}", base, suffix)
}

pub fn build_graph_schema(kinds_by_node_id: &BTreeMap<String, GraphFieldKind>) -> GraphSchema {
    build_graph_schema_with_field_names(kinds_by_node_id, &BTreeMap::new())
}

pub fn build_graph_schema_with_field_names(
    kinds_by_node_id: &BTreeMap<String, GraphFieldKind>,
    field_names_by_node_id: &BTreeMap<String, String>,
) -> GraphSchema {
    let mut used_names: BTreeSet<String> = BTreeSet::new();
    let mut fields: Vec<GraphField> = Vec::with_capacity(kinds_by_node_id.len());

    for (node_id, kind) in kinds_by_node_id {
        let base = field_names_by_node_id
            .get(node_id)
            .cloned()
            .unwrap_or_else(|| graph_field_name(node_id));
        let mut field_name = base.clone();
        let mut suffix: u32 = 2;
        while !used_names.insert(field_name.clone()) {
            field_name = format!("{base}_{suffix}");
            suffix += 1;
        }

        fields.push(GraphField {
            node_id: node_id.clone(),
            field_name,
            kind: *kind,
        });
    }

    GraphSchema {
        size_bytes: (fields.len() as u64) * 16,
        fields,
    }
}

pub fn choose_graph_binding_kind(
    size_bytes: u64,
    max_uniform_bytes: u64,
    max_storage_bytes: u64,
) -> Result<GraphBindingKind> {
    if size_bytes <= max_uniform_bytes {
        return Ok(GraphBindingKind::Uniform);
    }
    if size_bytes <= max_storage_bytes {
        return Ok(GraphBindingKind::StorageRead);
    }
    bail!(
        "graph input buffer size {size_bytes} exceeds device limits (uniform={max_uniform_bytes}, storage={max_storage_bytes})"
    )
}

fn parse_json_number_f32(v: &Value) -> Option<f32> {
    v.as_f64()
        .map(|x| x as f32)
        .or_else(|| v.as_i64().map(|x| x as f32))
        .or_else(|| v.as_u64().map(|x| x as f32))
}

fn parse_const_f32(node: &Node) -> Option<f32> {
    parse_json_number_f32(node.params.get("value")?)
        .or_else(|| parse_json_number_f32(node.params.get("x")?))
        .or_else(|| parse_json_number_f32(node.params.get("v")?))
}

fn parse_midi_input_f32(node: &Node) -> f32 {
    let default = node
        .params
        .get("defaultValue")
        .and_then(parse_json_number_f32)
        .or_else(|| node.params.get("value").and_then(parse_json_number_f32))
        .unwrap_or(0.0);

    let Some(raw) = node.params.get("rawValue").and_then(parse_json_number_f32) else {
        return default;
    };

    let min = node
        .params
        .get("minValue")
        .and_then(parse_json_number_f32)
        .unwrap_or(0.0);
    let max = node
        .params
        .get("maxValue")
        .and_then(parse_json_number_f32)
        .unwrap_or(1.0);
    let t = raw.clamp(0.0, 127.0) / 127.0;
    min + (max - min) * t
}

fn parse_const_bool(node: &Node) -> Option<bool> {
    node.params.get("value")?.as_bool()
}

fn parse_vec4_value_array(node: &Node, key: &str) -> Option<[f32; 4]> {
    let arr = node.params.get(key)?.as_array()?;
    let get = |i: usize, default: f32| -> f32 {
        arr.get(i)
            .and_then(parse_json_number_f32)
            .unwrap_or(default)
    };
    Some([get(0, 0.0), get(1, 0.0), get(2, 0.0), get(3, 1.0)])
}

fn parse_const_vec(node: &Node, keys: [&str; 4]) -> Option<[f32; 4]> {
    let x = parse_json_number_f32(node.params.get(keys[0])?)?;
    let y = node
        .params
        .get(keys[1])
        .and_then(parse_json_number_f32)
        .unwrap_or(0.0);
    let z = node
        .params
        .get(keys[2])
        .and_then(parse_json_number_f32)
        .unwrap_or(0.0);
    let w = node
        .params
        .get(keys[3])
        .and_then(parse_json_number_f32)
        .unwrap_or(1.0);
    Some([x, y, z, w])
}

/// Read `Vector4Input` params as `[x, y, z, w]`. Unlike [`parse_const_vec`]
/// (which leaves the `w` channel defaulting to `1.0` for color-style packing),
/// every channel here defaults to `0.0` — `Vector4Input` has no color semantics.
fn parse_vec4_xyzw(node: &Node) -> Option<[f32; 4]> {
    let read = |key: &str| {
        node.params
            .get(key)
            .and_then(parse_json_number_f32)
            .unwrap_or(0.0)
    };
    Some([read("x"), read("y"), read("z"), read("w")])
}

fn resolve_graph_input_component(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    port_id: &str,
    fallback: f32,
) -> f32 {
    resolve_input_f32(scene, nodes_by_id, &node.id, port_id)
        .ok()
        .flatten()
        .unwrap_or(fallback)
}

fn write_f32_slot(dst: &mut [u8], slot_index: usize, values: [f32; 4]) {
    let base = slot_index * 16;
    for (i, v) in values.into_iter().enumerate() {
        dst[base + i * 4..base + (i + 1) * 4].copy_from_slice(&v.to_ne_bytes());
    }
}

fn write_i32_slot(dst: &mut [u8], slot_index: usize, values: [i32; 4]) {
    let base = slot_index * 16;
    for (i, v) in values.into_iter().enumerate() {
        dst[base + i * 4..base + (i + 1) * 4].copy_from_slice(&v.to_ne_bytes());
    }
}

pub fn pack_graph_values(scene: &SceneDSL, schema: &GraphSchema) -> Result<Vec<u8>> {
    if schema.is_empty() {
        return Ok(Vec::new());
    }

    let nodes_by_id: HashMap<&str, &Node> = scene
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect::<HashMap<_, _>>();
    let needs_resolved_inputs = schema.fields.iter().any(|field| {
        matches!(
            field.kind,
            GraphFieldKind::Vec2 | GraphFieldKind::Vec3 | GraphFieldKind::Vec4
        )
    });
    let resolved_nodes_by_id = needs_resolved_inputs.then(|| {
        scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect::<HashMap<_, _>>()
    });

    let mut bytes = vec![0_u8; schema.size_bytes as usize];
    for (slot_index, field) in schema.fields.iter().enumerate() {
        let node = nodes_by_id
            .get(field.node_id.as_str())
            .copied()
            .ok_or_else(|| anyhow!("graph uniform node not found: {}", field.node_id))?;

        match field.kind {
            GraphFieldKind::F32 => {
                let v = if node.node_type == "MidiInput" {
                    parse_midi_input_f32(node)
                } else {
                    parse_const_f32(node).unwrap_or(0.0)
                };
                write_f32_slot(&mut bytes, slot_index, [v, 0.0, 0.0, 0.0]);
            }
            GraphFieldKind::I32 => {
                let v = parse_const_f32(node).unwrap_or(0.0) as i32;
                write_i32_slot(&mut bytes, slot_index, [v, 0, 0, 0]);
            }
            GraphFieldKind::Bool => {
                let v = if parse_const_bool(node).unwrap_or(false) {
                    1
                } else {
                    0
                };
                write_i32_slot(&mut bytes, slot_index, [v, 0, 0, 0]);
            }
            GraphFieldKind::Vec2 => {
                let fallback =
                    parse_const_vec(node, ["x", "y", "z", "w"]).unwrap_or([0.0, 0.0, 0.0, 0.0]);
                let v = if let Some(resolved) = resolved_nodes_by_id.as_ref() {
                    [
                        resolve_graph_input_component(scene, resolved, node, "x", fallback[0]),
                        resolve_graph_input_component(scene, resolved, node, "y", fallback[1]),
                        0.0,
                        0.0,
                    ]
                } else {
                    fallback
                };
                write_f32_slot(&mut bytes, slot_index, [v[0], v[1], 0.0, 0.0]);
            }
            GraphFieldKind::Vec3 => {
                let fallback =
                    parse_const_vec(node, ["x", "y", "z", "w"]).unwrap_or([0.0, 0.0, 0.0, 0.0]);
                let v = if let Some(resolved) = resolved_nodes_by_id.as_ref() {
                    [
                        resolve_graph_input_component(scene, resolved, node, "x", fallback[0]),
                        resolve_graph_input_component(scene, resolved, node, "y", fallback[1]),
                        resolve_graph_input_component(scene, resolved, node, "z", fallback[2]),
                        0.0,
                    ]
                } else {
                    fallback
                };
                write_f32_slot(&mut bytes, slot_index, [v[0], v[1], v[2], 0.0]);
            }
            GraphFieldKind::Vec4 => {
                let fallback = parse_vec4_xyzw(node).unwrap_or([0.0, 0.0, 0.0, 0.0]);
                let v = if let Some(resolved) = resolved_nodes_by_id.as_ref() {
                    [
                        resolve_graph_input_component(scene, resolved, node, "x", fallback[0]),
                        resolve_graph_input_component(scene, resolved, node, "y", fallback[1]),
                        resolve_graph_input_component(scene, resolved, node, "z", fallback[2]),
                        resolve_graph_input_component(scene, resolved, node, "w", fallback[3]),
                    ]
                } else {
                    fallback
                };
                write_f32_slot(&mut bytes, slot_index, v);
            }
            GraphFieldKind::Vec4Color => {
                let v = parse_vec4_value_array(node, "value").unwrap_or([1.0, 0.0, 1.0, 1.0]);
                write_f32_slot(&mut bytes, slot_index, v);
            }
        }
    }

    Ok(bytes)
}

fn is_value_driven_input_node(node_type: &str) -> bool {
    matches!(
        node_type,
        "BoolInput"
            | "FloatInput"
            | "MidiInput"
            | "IntInput"
            | "Vector2Input"
            | "Vector3Input"
            | "Vector4Input"
            | "ColorInput"
    )
}

fn collect_ignored_input_value_node_ids_for_pass_bindings(
    scene: &SceneDSL,
    pass_bindings: &[PassBindings],
) -> BTreeSet<String> {
    let nodes_by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|node| (node.id.clone(), node))
        .collect();
    let mut ignored_input_value_node_ids: BTreeSet<String> = BTreeSet::new();

    for pass in pass_bindings {
        let Some(binding) = pass.graph_binding.as_ref() else {
            continue;
        };

        for field in &binding.schema.fields {
            ignored_input_value_node_ids.insert(field.node_id.clone());

            let component_ports: &[&str] = match nodes_by_id
                .get(field.node_id.as_str())
                .map(|node| node.node_type.as_str())
            {
                Some("Vector2Input") => &["x", "y"],
                Some("Vector3Input") => &["x", "y", "z"],
                Some("Vector4Input") => &["x", "y", "z", "w"],
                _ => &[],
            };

            let mut visiting_outputs: BTreeSet<(String, String)> = BTreeSet::new();
            for port_id in component_ports {
                collect_scalar_input_dependencies_for_input_port(
                    scene,
                    &nodes_by_id,
                    field.node_id.as_str(),
                    port_id,
                    &mut ignored_input_value_node_ids,
                    &mut visiting_outputs,
                );
            }
        }
    }

    ignored_input_value_node_ids
}

fn collect_scalar_input_dependencies_for_input_port(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    port_id: &str,
    ignored_input_value_node_ids: &mut BTreeSet<String>,
    visiting_outputs: &mut BTreeSet<(String, String)>,
) {
    let Some(conn) = crate::dsl::incoming_connection(scene, node_id, port_id) else {
        return;
    };

    collect_scalar_input_dependencies_for_output_port(
        scene,
        nodes_by_id,
        conn.from.node_id.as_str(),
        conn.from.port_id.as_str(),
        ignored_input_value_node_ids,
        visiting_outputs,
    );
}

fn collect_scalar_input_dependencies_for_output_port(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    out_port: &str,
    ignored_input_value_node_ids: &mut BTreeSet<String>,
    visiting_outputs: &mut BTreeSet<(String, String)>,
) {
    let visit_key = (node_id.to_string(), out_port.to_string());
    if !visiting_outputs.insert(visit_key.clone()) {
        return;
    }

    let Some(node) = nodes_by_id.get(node_id) else {
        let _ = visiting_outputs.remove(&visit_key);
        return;
    };

    match node.node_type.as_str() {
        "BoolInput" | "FloatInput" | "IntInput" | "MidiInput" if out_port == "value" => {
            ignored_input_value_node_ids.insert(node.id.clone());
        }
        "MathAdd" | "MathSubtract" | "MathMultiply" | "MathDivide" if out_port == "result" => {
            if node.inputs.is_empty() {
                for input_port in ["a", "b"] {
                    collect_scalar_input_dependencies_for_input_port(
                        scene,
                        nodes_by_id,
                        node_id,
                        input_port,
                        ignored_input_value_node_ids,
                        visiting_outputs,
                    );
                }
            } else {
                for input_port in &node.inputs {
                    collect_scalar_input_dependencies_for_input_port(
                        scene,
                        nodes_by_id,
                        node_id,
                        input_port.id.as_str(),
                        ignored_input_value_node_ids,
                        visiting_outputs,
                    );
                }
            }
        }
        "MathClamp" if out_port == "result" => {
            for input_port in ["value", "min", "max"] {
                collect_scalar_input_dependencies_for_input_port(
                    scene,
                    nodes_by_id,
                    node_id,
                    input_port,
                    ignored_input_value_node_ids,
                    visiting_outputs,
                );
            }
        }
        "MathPower" if out_port == "result" => {
            for input_port in ["base", "exponent"] {
                collect_scalar_input_dependencies_for_input_port(
                    scene,
                    nodes_by_id,
                    node_id,
                    input_port,
                    ignored_input_value_node_ids,
                    visiting_outputs,
                );
            }
        }
        "ResourcePool" => {
            let dynamic_inputs: Vec<&str> = node
                .inputs
                .iter()
                .filter(|port| port.id != "selectedIndex")
                .map(|port| port.id.as_str())
                .collect();
            let selected_index =
                crate::dsl::resolve_input_i64(scene, nodes_by_id, node_id, "selectedIndex")
                    .ok()
                    .flatten()
                    .unwrap_or(0)
                    .max(0) as usize;
            let selected_index = selected_index.min(dynamic_inputs.len().saturating_sub(1));
            if let Some(selected_port) = dynamic_inputs.get(selected_index) {
                collect_scalar_input_dependencies_for_input_port(
                    scene,
                    nodes_by_id,
                    node_id,
                    selected_port,
                    ignored_input_value_node_ids,
                    visiting_outputs,
                );
            }
        }
        "DataParse" => {
            for conn in scene
                .connections
                .iter()
                .filter(|conn| conn.to.node_id == node_id)
            {
                collect_scalar_input_dependencies_for_output_port(
                    scene,
                    nodes_by_id,
                    conn.from.node_id.as_str(),
                    conn.from.port_id.as_str(),
                    ignored_input_value_node_ids,
                    visiting_outputs,
                );
            }
        }
        _ => {}
    }

    let _ = visiting_outputs.remove(&visit_key);
}

fn canonicalized_params(
    node: &Node,
    ignored_input_value_node_ids: &BTreeSet<String>,
) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Value> = BTreeMap::new();
    for (k, v) in &node.params {
        if k.starts_with("__dedup_") {
            continue;
        }
        if ignored_input_value_node_ids.contains(node.id.as_str())
            && is_value_driven_input_node(node.node_type.as_str())
            && matches!(
                k.as_str(),
                "value"
                    | "x"
                    | "y"
                    | "z"
                    | "w"
                    | "v"
                    | "rawValue"
                    | "defaultValue"
                    | "minValue"
                    | "maxValue"
            )
        {
            continue;
        }
        out.insert(k.clone(), v.clone());
    }
    out
}

fn sort_node_ports(ports: &[NodePort]) -> Vec<NodePort> {
    let mut out = ports.to_vec();
    out.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.name.cmp(&b.name)));
    out
}

fn sort_input_bindings(bindings: &[InputBinding]) -> Vec<InputBinding> {
    let mut out = bindings.to_vec();
    out.sort_by(|a, b| {
        a.port_id
            .cmp(&b.port_id)
            .then_with(|| a.variable_name.cmp(&b.variable_name))
            .then_with(|| a.binding_type.cmp(&b.binding_type))
            .then_with(|| {
                let sa = a
                    .source_binding
                    .as_ref()
                    .map(|s| {
                        (
                            s.node_id.as_str(),
                            s.output_port_id.as_str(),
                            s.output_label.as_deref().unwrap_or(""),
                        )
                    })
                    .unwrap_or(("", "", ""));
                let sb = b
                    .source_binding
                    .as_ref()
                    .map(|s| {
                        (
                            s.node_id.as_str(),
                            s.output_port_id.as_str(),
                            s.output_label.as_deref().unwrap_or(""),
                        )
                    })
                    .unwrap_or(("", "", ""));
                sa.cmp(&sb)
            })
    });
    out
}

fn sort_connections(connections: &[Connection]) -> Vec<Connection> {
    let mut out = connections.to_vec();
    out.sort_by(|a, b| {
        (
            a.from.node_id.as_str(),
            a.from.port_id.as_str(),
            a.to.node_id.as_str(),
            a.to.port_id.as_str(),
            a.id.as_str(),
        )
            .cmp(&(
                b.from.node_id.as_str(),
                b.from.port_id.as_str(),
                b.to.node_id.as_str(),
                b.to.port_id.as_str(),
                b.id.as_str(),
            ))
    });
    out
}

fn canonicalize_nodes(
    nodes: &[Node],
    ignored_input_value_node_ids: &BTreeSet<String>,
) -> Vec<Value> {
    let mut sorted = nodes.to_vec();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));

    sorted
        .into_iter()
        .map(|n| {
            json!({
                "id": n.id,
                "type": n.node_type,
                "params": canonicalized_params(&n, ignored_input_value_node_ids),
                "inputs": sort_node_ports(&n.inputs),
                "outputs": sort_node_ports(&n.outputs),
                "inputBindings": sort_input_bindings(&n.input_bindings),
            })
        })
        .collect()
}

fn canonicalize_groups(
    groups: &[GroupDSL],
    ignored_input_value_node_ids: &BTreeSet<String>,
) -> Vec<Value> {
    let mut sorted = groups.to_vec();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));

    sorted
        .into_iter()
        .map(|g| {
            let mut input_bindings = g.input_bindings.clone();
            input_bindings.sort_by(|a, b| {
                (
                    a.group_port_id.as_str(),
                    a.to.node_id.as_str(),
                    a.to.port_id.as_str(),
                )
                    .cmp(&(
                        b.group_port_id.as_str(),
                        b.to.node_id.as_str(),
                        b.to.port_id.as_str(),
                    ))
            });

            let mut output_bindings = g.output_bindings.clone();
            output_bindings.sort_by(|a, b| {
                (
                    a.group_port_id.as_str(),
                    a.from.node_id.as_str(),
                    a.from.port_id.as_str(),
                )
                    .cmp(&(
                        b.group_port_id.as_str(),
                        b.from.node_id.as_str(),
                        b.from.port_id.as_str(),
                    ))
            });

            json!({
                "id": g.id,
                "name": g.name,
                "inputs": sort_node_ports(&g.inputs),
                "outputs": sort_node_ports(&g.outputs),
                "nodes": canonicalize_nodes(&g.nodes, ignored_input_value_node_ids),
                "connections": sort_connections(&g.connections),
                "inputBindings": input_bindings,
                "outputBindings": output_bindings,
            })
        })
        .collect()
}

fn canonical_scene_value(
    scene: &SceneDSL,
    ignored_input_value_node_ids: &BTreeSet<String>,
) -> Value {
    let outputs_sorted: BTreeMap<String, String> = scene
        .outputs
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect();

    json!({
        "version": scene.version,
        "nodes": canonicalize_nodes(&scene.nodes, ignored_input_value_node_ids),
        "connections": sort_connections(&scene.connections),
        "outputs": outputs_sorted,
        "groups": canonicalize_groups(&scene.groups, ignored_input_value_node_ids),
    })
}

fn collect_all_value_driven_input_node_ids(scene: &SceneDSL) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for n in &scene.nodes {
        if is_value_driven_input_node(n.node_type.as_str()) {
            out.insert(n.id.clone());
        }
    }
    for g in &scene.groups {
        for n in &g.nodes {
            if is_value_driven_input_node(n.node_type.as_str()) {
                out.insert(n.id.clone());
            }
        }
    }
    out
}

fn graph_field_kind_label(kind: GraphFieldKind) -> &'static str {
    match kind {
        GraphFieldKind::F32 => "f32",
        GraphFieldKind::I32 => "i32",
        GraphFieldKind::Bool => "bool",
        GraphFieldKind::Vec2 => "vec2",
        GraphFieldKind::Vec3 => "vec3",
        GraphFieldKind::Vec4 => "vec4",
        GraphFieldKind::Vec4Color => "vec4_color",
    }
}

fn graph_binding_kind_label(kind: GraphBindingKind) -> &'static str {
    match kind {
        GraphBindingKind::Uniform => "uniform",
        GraphBindingKind::StorageRead => "storage_read",
    }
}

fn canonical_graph_bindings_value(pass_bindings: &[PassBindings]) -> Vec<Value> {
    let mut sorted: Vec<&PassBindings> = pass_bindings
        .iter()
        .filter(|p| p.graph_binding.is_some())
        .collect();
    sorted.sort_by(|a, b| a.pass_id.cmp(&b.pass_id));

    sorted
        .into_iter()
        .filter_map(|p| {
            let binding = p.graph_binding.as_ref()?;
            let mut fields = binding.schema.fields.clone();
            fields.sort_by(|a, b| {
                a.field_name
                    .cmp(&b.field_name)
                    .then_with(|| a.node_id.cmp(&b.node_id))
            });
            let fields_value: Vec<Value> = fields
                .into_iter()
                .map(|f| {
                    json!({
                        "nodeId": f.node_id,
                        "fieldName": f.field_name,
                        "kind": graph_field_kind_label(f.kind),
                    })
                })
                .collect();

            Some(json!({
                "passId": p.pass_id,
                "bufferName": binding.buffer_name.as_str(),
                "kind": graph_binding_kind_label(binding.kind),
                "sizeBytes": binding.schema.size_bytes,
                "fields": fields_value,
            }))
        })
        .collect()
}

pub fn compute_pipeline_signature(scene: &SceneDSL) -> [u8; 32] {
    let maybe_prepared = crate::renderer::scene_prep::prepare_scene(scene).ok();
    let (scene_for_sig, ignored_input_value_node_ids) =
        if let Some(prepared) = maybe_prepared.as_ref() {
            (
                &prepared.scene,
                collect_all_value_driven_input_node_ids(&prepared.scene),
            )
        } else {
            (scene, collect_all_value_driven_input_node_ids(scene))
        };

    let payload = json!({
        "scene": canonical_scene_value(scene_for_sig, &ignored_input_value_node_ids),
        "templateGeneration": crate::renderer::node_compiler::template_loader::generation(),
    });
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    hash_bytes(&bytes)
}

pub fn compute_pipeline_signature_for_pass_bindings(
    scene: &SceneDSL,
    pass_bindings: &[PassBindings],
) -> [u8; 32] {
    let ignored_input_value_node_ids =
        collect_ignored_input_value_node_ids_for_pass_bindings(scene, pass_bindings);

    let payload = json!({
        "scene": canonical_scene_value(scene, &ignored_input_value_node_ids),
        "graphBindings": canonical_graph_bindings_value(pass_bindings),
        "templateGeneration": crate::renderer::node_compiler::template_loader::generation(),
    });
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    hash_bytes(&bytes)
}

pub fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    fn fnv1a64_with_seed(bytes: &[u8], seed: u64) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64 ^ seed;
        for &b in bytes {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    let h0 = fnv1a64_with_seed(bytes, 0x0000_0000_0000_0000);
    let h1 = fnv1a64_with_seed(bytes, 0x9e37_79b9_7f4a_7c15);
    let h2 = fnv1a64_with_seed(bytes, 0xc2b2_ae3d_27d4_eb4f);
    let h3 = fnv1a64_with_seed(bytes, 0x1656_67b1_9e37_79f9);

    let mut out = [0_u8; 32];
    out[0..8].copy_from_slice(&h0.to_le_bytes());
    out[8..16].copy_from_slice(&h1.to_le_bytes());
    out[16..24].copy_from_slice(&h2.to_le_bytes());
    out[24..32].copy_from_slice(&h3.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{Connection, Endpoint, Metadata};
    use crate::renderer::types::{GraphBinding, Params, PassBindings};
    use rust_wgpu_fiber::ResourceName;

    fn make_node(id: &str, node_type: &str, params: serde_json::Value) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params: serde_json::from_value(params).unwrap_or_default(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_bindings: Vec::new(),
            wgsl_override: None,
        }
    }

    fn base_scene() -> SceneDSL {
        SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "sig".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                make_node("FloatInput_1", "FloatInput", json!({"value": 1.0})),
                make_node("MathAdd_1", "MathAdd", json!({"unused": 2.0})),
            ],
            connections: vec![Connection {
                id: "c1".to_string(),
                from: Endpoint {
                    node_id: "FloatInput_1".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "MathAdd_1".to_string(),
                    port_id: "input1".to_string(),
                },
            }],
            outputs: Some(HashMap::from([(
                "composite".to_string(),
                "MathAdd_1".to_string(),
            )])),
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        }
    }

    #[test]
    fn field_name_is_deterministic() {
        let a = graph_field_name("FloatInput_1");
        let b = graph_field_name("FloatInput_1");
        assert_eq!(a, b);
        assert!(a.starts_with("node_"));
    }

    #[test]
    fn schema_build_is_deterministic() {
        let mut m: BTreeMap<String, GraphFieldKind> = BTreeMap::new();
        m.insert("b".to_string(), GraphFieldKind::F32);
        m.insert("a".to_string(), GraphFieldKind::Vec3);
        let s = build_graph_schema(&m);
        assert_eq!(s.fields.len(), 2);
        assert_eq!(s.fields[0].node_id, "a");
        assert_eq!(s.size_bytes, 32);
    }

    #[test]
    fn pack_graph_values_writes_expected_slots() {
        let scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "pack".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                make_node("FloatInput_1", "FloatInput", json!({"value": 3.0})),
                make_node("BoolInput_1", "BoolInput", json!({"value": true})),
            ],
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };

        let schema = GraphSchema {
            fields: vec![
                GraphField {
                    node_id: "FloatInput_1".to_string(),
                    field_name: "node_float".to_string(),
                    kind: GraphFieldKind::F32,
                },
                GraphField {
                    node_id: "BoolInput_1".to_string(),
                    field_name: "node_bool".to_string(),
                    kind: GraphFieldKind::Bool,
                },
            ],
            size_bytes: 32,
        };

        let bytes = pack_graph_values(&scene, &schema).unwrap();
        assert_eq!(bytes.len(), 32);

        let f0 = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        assert_eq!(f0, 3.0);
        let b0 = i32::from_ne_bytes(bytes[16..20].try_into().unwrap());
        assert_eq!(b0, 1);
    }

    #[test]
    fn pack_graph_values_maps_midi_raw_value_to_range() {
        let scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "midi".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                make_node(
                    "MidiInput_1",
                    "MidiInput",
                    json!({"rawValue": 127, "defaultValue": 0.25, "minValue": -1.0, "maxValue": 1.0}),
                ),
                make_node("MidiInput_2", "MidiInput", json!({"defaultValue": 0.25})),
            ],
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };

        let schema = GraphSchema {
            fields: vec![
                GraphField {
                    node_id: "MidiInput_1".to_string(),
                    field_name: "midi_one".to_string(),
                    kind: GraphFieldKind::F32,
                },
                GraphField {
                    node_id: "MidiInput_2".to_string(),
                    field_name: "midi_two".to_string(),
                    kind: GraphFieldKind::F32,
                },
            ],
            size_bytes: 32,
        };

        let bytes = pack_graph_values(&scene, &schema).unwrap();
        let mapped = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let fallback = f32::from_ne_bytes(bytes[16..20].try_into().unwrap());
        assert_eq!(mapped, 1.0);
        assert_eq!(fallback, 0.25);
    }

    #[test]
    fn pack_graph_values_resolves_vector4_input_component_connections() {
        let scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "midi-vector".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                make_node(
                    "MidiInput_1",
                    "MidiInput",
                    json!({"rawValue": 127, "defaultValue": 0.0, "minValue": 0.0, "maxValue": 2.0}),
                ),
                make_node(
                    "MidiInput_2",
                    "MidiInput",
                    json!({"defaultValue": 0.75, "minValue": 0.0, "maxValue": 2.0}),
                ),
                make_node(
                    "Vector4Input_1",
                    "Vector4Input",
                    json!({"x": 9.0, "y": 9.0, "z": 0.25, "w": 0.5}),
                ),
            ],
            connections: vec![
                Connection {
                    id: "c1".to_string(),
                    from: Endpoint {
                        node_id: "MidiInput_1".to_string(),
                        port_id: "value".to_string(),
                    },
                    to: Endpoint {
                        node_id: "Vector4Input_1".to_string(),
                        port_id: "x".to_string(),
                    },
                },
                Connection {
                    id: "c2".to_string(),
                    from: Endpoint {
                        node_id: "MidiInput_2".to_string(),
                        port_id: "value".to_string(),
                    },
                    to: Endpoint {
                        node_id: "Vector4Input_1".to_string(),
                        port_id: "y".to_string(),
                    },
                },
            ],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };

        let schema = GraphSchema {
            fields: vec![GraphField {
                node_id: "Vector4Input_1".to_string(),
                field_name: "vec".to_string(),
                kind: GraphFieldKind::Vec4,
            }],
            size_bytes: 16,
        };

        let bytes = pack_graph_values(&scene, &schema).unwrap();
        let values = [
            f32::from_ne_bytes(bytes[0..4].try_into().unwrap()),
            f32::from_ne_bytes(bytes[4..8].try_into().unwrap()),
            f32::from_ne_bytes(bytes[8..12].try_into().unwrap()),
            f32::from_ne_bytes(bytes[12..16].try_into().unwrap()),
        ];
        assert_eq!(values, [2.0, 0.75, 0.25, 0.5]);
    }

    #[test]
    fn signature_ignores_input_values_but_keeps_structure() {
        let mut s1 = base_scene();
        let mut s2 = base_scene();
        s2.nodes[0].params.insert("value".to_string(), json!(10.0));

        let h1 = compute_pipeline_signature(&s1);
        let h2 = compute_pipeline_signature(&s2);
        assert_eq!(h1, h2);

        s1.nodes[1].params.insert("unused".to_string(), json!(3.0));
        let h3 = compute_pipeline_signature(&s1);
        assert_ne!(h2, h3);
    }

    #[test]
    fn signature_for_pass_bindings_only_ignores_bound_input_values() {
        let mut scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "sig-bind".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                make_node("fast", "FloatInput", json!({"value": 1.0})),
                make_node("cpu", "FloatInput", json!({"value": 2.0})),
            ],
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };

        let pass = PassBindings {
            pass_id: "passA".to_string(),
            params_buffer: ResourceName::from("params.passA"),
            base_params: Params {
                target_size: [1.0, 1.0],
                geo_size: [1.0, 1.0],
                center: [0.5, 0.5],
                geo_translate: [0.0, 0.0],
                geo_scale: [1.0, 1.0],
                time: 0.0,
                _pad0: 0.0,
                color: [1.0, 1.0, 1.0, 1.0],
                camera: [
                    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                ],
                camera_position: [0.0, 0.0, 0.0, 0.0],
            },
            graph_binding: Some(GraphBinding {
                buffer_name: ResourceName::from("params.passA.graph"),
                kind: GraphBindingKind::Uniform,
                schema: GraphSchema {
                    fields: vec![GraphField {
                        node_id: "fast".to_string(),
                        field_name: graph_field_name("fast"),
                        kind: GraphFieldKind::F32,
                    }],
                    size_bytes: 16,
                },
            }),
            last_graph_hash: None,
            extension: None,
        };

        let h1 = compute_pipeline_signature_for_pass_bindings(&scene, &[pass.clone()]);

        scene.nodes[0]
            .params
            .insert("value".to_string(), json!(10.0));
        let h2 = compute_pipeline_signature_for_pass_bindings(&scene, &[pass.clone()]);
        assert_eq!(h1, h2, "bound input value should not affect signature");

        scene.nodes[1]
            .params
            .insert("value".to_string(), json!(10.0));
        let h3 = compute_pipeline_signature_for_pass_bindings(&scene, &[pass]);
        assert_ne!(h2, h3, "unbound input value must force rebuild");
    }

    #[test]
    fn signature_for_pass_bindings_ignores_upstream_values_of_bound_vector_inputs() {
        let mut scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "sig-vec4-bind".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                make_node(
                    "midi_a",
                    "MidiInput",
                    json!({"rawValue": 0, "defaultValue": 0.0, "minValue": 0.0, "maxValue": 1.0}),
                ),
                make_node(
                    "midi_b",
                    "MidiInput",
                    json!({"rawValue": 0, "defaultValue": 0.0, "minValue": 0.0, "maxValue": 1.0}),
                ),
                make_node(
                    "vec4",
                    "Vector4Input",
                    json!({"x": 0.0, "y": 0.0, "z": 0.0, "w": 0.0}),
                ),
            ],
            connections: vec![
                Connection {
                    id: "c1".to_string(),
                    from: Endpoint {
                        node_id: "midi_a".to_string(),
                        port_id: "value".to_string(),
                    },
                    to: Endpoint {
                        node_id: "vec4".to_string(),
                        port_id: "x".to_string(),
                    },
                },
                Connection {
                    id: "c2".to_string(),
                    from: Endpoint {
                        node_id: "midi_b".to_string(),
                        port_id: "value".to_string(),
                    },
                    to: Endpoint {
                        node_id: "vec4".to_string(),
                        port_id: "y".to_string(),
                    },
                },
            ],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };

        let pass = PassBindings {
            pass_id: "passA".to_string(),
            params_buffer: ResourceName::from("params.passA"),
            base_params: Params {
                target_size: [1.0, 1.0],
                geo_size: [1.0, 1.0],
                center: [0.5, 0.5],
                geo_translate: [0.0, 0.0],
                geo_scale: [1.0, 1.0],
                time: 0.0,
                _pad0: 0.0,
                color: [1.0, 1.0, 1.0, 1.0],
                camera: [
                    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                ],
                camera_position: [0.0, 0.0, 0.0, 0.0],
            },
            graph_binding: Some(GraphBinding {
                buffer_name: ResourceName::from("params.passA.graph"),
                kind: GraphBindingKind::Uniform,
                schema: GraphSchema {
                    fields: vec![GraphField {
                        node_id: "vec4".to_string(),
                        field_name: graph_field_name("vec4"),
                        kind: GraphFieldKind::Vec4,
                    }],
                    size_bytes: 16,
                },
            }),
            last_graph_hash: None,
            extension: None,
        };

        let h1 = compute_pipeline_signature_for_pass_bindings(&scene, &[pass.clone()]);
        scene.nodes[0]
            .params
            .insert("rawValue".to_string(), json!(127));
        let h2 = compute_pipeline_signature_for_pass_bindings(&scene, &[pass.clone()]);
        assert_eq!(
            h1, h2,
            "upstream MidiInput feeding a bound Vector4Input should stay on the uniform-only path"
        );

        scene
            .nodes
            .push(make_node("free", "FloatInput", json!({"value": 1.0})));
        let h3 = compute_pipeline_signature_for_pass_bindings(&scene, &[pass.clone()]);
        scene
            .nodes
            .last_mut()
            .expect("free input node")
            .params
            .insert("value".to_string(), json!(2.0));
        let h4 = compute_pipeline_signature_for_pass_bindings(&scene, &[pass]);
        assert_ne!(h3, h4, "still must rebuild for unrelated unbound inputs");
    }

    #[test]
    fn signature_for_pass_bindings_includes_binding_mode() {
        let scene = base_scene();
        let make_pass = |kind: GraphBindingKind| PassBindings {
            pass_id: "passA".to_string(),
            params_buffer: ResourceName::from("params.passA"),
            base_params: Params {
                target_size: [1.0, 1.0],
                geo_size: [1.0, 1.0],
                center: [0.5, 0.5],
                geo_translate: [0.0, 0.0],
                geo_scale: [1.0, 1.0],
                time: 0.0,
                _pad0: 0.0,
                color: [1.0, 1.0, 1.0, 1.0],
                camera: [
                    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                ],
                camera_position: [0.0, 0.0, 0.0, 0.0],
            },
            graph_binding: Some(GraphBinding {
                buffer_name: ResourceName::from("params.passA.graph"),
                kind,
                schema: GraphSchema {
                    fields: vec![GraphField {
                        node_id: "FloatInput_1".to_string(),
                        field_name: graph_field_name("FloatInput_1"),
                        kind: GraphFieldKind::F32,
                    }],
                    size_bytes: 16,
                },
            }),
            last_graph_hash: None,
            extension: None,
        };

        let h_ubo = compute_pipeline_signature_for_pass_bindings(
            &scene,
            &[make_pass(GraphBindingKind::Uniform)],
        );
        let h_ssbo = compute_pipeline_signature_for_pass_bindings(
            &scene,
            &[make_pass(GraphBindingKind::StorageRead)],
        );
        assert_ne!(h_ubo, h_ssbo);
    }
}
