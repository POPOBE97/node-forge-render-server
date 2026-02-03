//! Scene preparation and validation module.
//!
//! This module handles:
//! - Port type utilities for connection validation
//! - Auto-wrapping of primitive values into render passes
//! - Scene validation and topological sorting
//! - Composite layer ordering

use anyhow::{anyhow, bail, Context, Result};
use rust_wgpu_fiber::ResourceName;
use std::collections::HashMap;

use crate::{
    dsl::{
        find_node, incoming_connection, Connection, Endpoint, GroupDSL, InputBinding, Node,
        SceneDSL, SourceBinding,
    },
    graph::{topo_sort, upstream_reachable},
    renderer::types::{BakedValue, ValueType},
    renderer::utils::cpu_num_u32_min_1,
    schema,
    ts_runtime::TsRuntime,
};

fn group_by_id<'a>(groups: &'a [GroupDSL], id: &str) -> Option<&'a GroupDSL> {
    groups.iter().find(|g| g.id == id)
}

fn parse_group_id(node: &Node) -> Option<&str> {
    node.params.get("groupId").and_then(|v| v.as_str())
}

fn rewrite_node_input_bindings(node: &mut Node, node_id_map: &HashMap<String, String>) {
    for b in &mut node.input_bindings {
        if let Some(sb) = b.source_binding.as_mut() {
            if let Some(new_id) = node_id_map.get(&sb.node_id) {
                sb.node_id = new_id.clone();
            }
        }
    }
}

fn instance_input_sources(scene: &SceneDSL, instance: &Node) -> HashMap<String, Vec<Endpoint>> {
    // Collect both explicit inbound connections AND editor-exported inputBindings (sourceBinding).
    let mut inbound: HashMap<String, Vec<Endpoint>> = HashMap::new();

    for c in &scene.connections {
        if c.to.node_id == instance.id {
            inbound
                .entry(c.to.port_id.clone())
                .or_default()
                .push(c.from.clone());
        }
    }

    for b in &instance.input_bindings {
        if let Some(sb) = b.source_binding.as_ref() {
            inbound
                .entry(b.port_id.clone())
                .or_default()
                .push(Endpoint {
                    node_id: sb.node_id.clone(),
                    port_id: sb.output_port_id.clone(),
                });
        }
    }

    inbound
}

fn copy_image_file_params_into_image_texture(
    dst: &mut Node,
    data_url: Option<serde_json::Value>,
    path: Option<serde_json::Value>,
) {
    // Minimal contract: ImageFile provides {dataUrl, path}; ImageTexture consumes those as params.
    if let Some(v) = data_url {
        if v.as_str().is_some_and(|s| !s.trim().is_empty()) {
            let already = dst
                .params
                .get("dataUrl")
                .and_then(|x| x.as_str())
                .is_some_and(|s| !s.trim().is_empty());
            if !already {
                dst.params.insert("dataUrl".to_string(), v);
            }
        }
    }
    if let Some(v) = path {
        if v.as_str().is_some_and(|s| !s.trim().is_empty()) {
            let already = dst
                .params
                .get("path")
                .and_then(|x| x.as_str())
                .is_some_and(|s| !s.trim().is_empty());
            if !already {
                dst.params.insert("path".to_string(), v);
            }
        }
    }
}

fn inline_image_file_connections_into_image_textures(scene: &mut SceneDSL) -> Result<()> {
    // ImageTexture currently loads its image from params.{dataUrl,path} at runtime.
    // But the node scheme models image flow as a connection: ImageFile.image -> ImageTexture.image.
    // Inline that connection by copying the ImageFile params into the connected ImageTexture.
    //
    // This keeps authoring in the graph model while satisfying runtime expectations.
    let by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|n| (n.id.clone(), n))
        .collect();

    // Collect destinations we need to patch without holding overlapping borrows.
    let mut patches: Vec<(String, Option<serde_json::Value>, Option<serde_json::Value>)> =
        Vec::new();
    for c in &scene.connections {
        if c.to.port_id != "image" {
            continue;
        }
        let Some(dst) = by_id.get(&c.to.node_id) else {
            continue;
        };
        if dst.node_type != "ImageTexture" {
            continue;
        }

        let Some(src) = by_id.get(&c.from.node_id) else {
            bail!(
                "ImageTexture '{}' has image input from missing node '{}'",
                c.to.node_id,
                c.from.node_id
            );
        };
        if src.node_type != "ImageFile" {
            bail!(
                "ImageTexture '{}' image input must come from ImageFile, got {} (node {})",
                c.to.node_id,
                src.node_type,
                src.id
            );
        }

        // Note: prefer dataUrl if present; but copy both so runtime can fallback.
        let data_url = src.params.get("dataUrl").cloned();
        let path = src.params.get("path").cloned();
        patches.push((dst.id.clone(), data_url, path));
    }

    // Apply patches to the real scene.
    for (dst_id, data_url, path) in patches {
        let Some(dst) = scene.nodes.iter_mut().find(|n| n.id == dst_id) else {
            bail!(
                "missing ImageTexture node '{}' when inlining ImageFile",
                dst_id
            );
        };
        if dst.node_type != "ImageTexture" {
            bail!(
                "expected ImageTexture node '{}' when inlining ImageFile, got {}",
                dst_id,
                dst.node_type
            );
        }
        copy_image_file_params_into_image_texture(dst, data_url, path);
    }

    Ok(())
}

fn expand_group_instances(scene: &mut SceneDSL) -> Result<()> {
    // Expand all GroupInstance nodes into the main graph by cloning the referenced group
    // subgraph, rewriting node IDs, and wiring instance I/O using group bindings.
    //
    // This must run before upstream reachability filtering + scheme validation.
    let mut next_edge_id: u64 = 0;
    let mut next_edge = || {
        next_edge_id += 1;
        format!("sys.group.edge.{next_edge_id}")
    };

    loop {
        let Some((instance_id, group_id)) = scene
            .nodes
            .iter()
            .find(|n| n.node_type == "GroupInstance")
            .map(|n| {
                let gid = parse_group_id(n).map(|s| s.to_string());
                (n.id.clone(), gid)
            })
        else {
            break;
        };

        let group_id = group_id
            .as_deref()
            .ok_or_else(|| anyhow!("GroupInstance missing params.groupId ({instance_id})"))?;
        let group = group_by_id(&scene.groups, group_id)
            .ok_or_else(|| anyhow!("GroupInstance refers to missing group '{group_id}'"))?
            .clone();

        // Map group-local node IDs to cloned node IDs.
        let mut node_id_map: HashMap<String, String> = HashMap::new();
        for n in &group.nodes {
            node_id_map.insert(n.id.clone(), format!("{instance_id}/{}", n.id));
        }

        // 1) Clone group nodes into main scene.
        // Also rewrite any editor metadata bindings that reference group-local node IDs.
        for mut n in group.nodes.clone() {
            if let Some(new_id) = node_id_map.get(&n.id).cloned() {
                n.id = new_id;
            }
            rewrite_node_input_bindings(&mut n, &node_id_map);
            scene.nodes.push(n);
        }

        // 2) Clone group connections into main scene.
        for mut c in group.connections.clone() {
            c.id = next_edge();
            if let Some(new_from) = node_id_map.get(&c.from.node_id).cloned() {
                c.from.node_id = new_from;
            }
            if let Some(new_to) = node_id_map.get(&c.to.node_id).cloned() {
                c.to.node_id = new_to;
            }
            scene.connections.push(c);
        }

        // Gather existing inbound/outbound connections for this instance.
        // Note: for some editors, instance inputs may only be expressed via inputBindings,
        // so we collect those too.
        let inbound_sources_by_port = {
            let inst_node = scene
                .nodes
                .iter()
                .find(|n| n.id == instance_id)
                .cloned()
                .unwrap_or_else(|| Node {
                    id: instance_id.clone(),
                    node_type: "GroupInstance".to_string(),
                    params: HashMap::new(),
                    inputs: Vec::new(),
                    outputs: Vec::new(),
                    input_bindings: Vec::new(),
                });
            instance_input_sources(scene, &inst_node)
        };

        let mut inbound_by_port: HashMap<String, Vec<Connection>> = HashMap::new();
        let mut outbound_by_port: HashMap<String, Vec<Connection>> = HashMap::new();
        for c in &scene.connections {
            if c.to.node_id == instance_id {
                inbound_by_port
                    .entry(c.to.port_id.clone())
                    .or_default()
                    .push(c.clone());
            }
            if c.from.node_id == instance_id {
                outbound_by_port
                    .entry(c.from.port_id.clone())
                    .or_default()
                    .push(c.clone());
            }
        }

        // 3) Wire instance inputs using group.input_bindings.
        // For each group input port, forward the instance's incoming connection(s)
        // into the cloned subgraph endpoint.
        for b in &group.input_bindings {
            let Some(target_new_node_id) = node_id_map.get(&b.to.node_id) else {
                bail!(
                    "group '{}' inputBindings references missing node '{}'",
                    group_id,
                    b.to.node_id
                );
            };

            let mut any = false;
            let mut sources: Vec<Endpoint> = Vec::new();
            if let Some(inbounds) = inbound_by_port.get(&b.group_port_id) {
                sources.extend(inbounds.iter().map(|c| c.from.clone()));
            }
            if let Some(extra) = inbound_sources_by_port.get(&b.group_port_id) {
                sources.extend(extra.iter().cloned());
            }

            // Special-case: ImageFile -> ImageTexture.image is not part of the bundled node scheme.
            // Instead, inline by copying ImageFile params into the ImageTexture node.
            if b.to.port_id == "image" {
                // Use the first available source.
                let src_ep = sources.first().cloned().ok_or_else(|| {
                    anyhow!(
                        "GroupInstance '{}' missing required input '{}' for group '{}'",
                        instance_id,
                        b.group_port_id,
                        group_id
                    )
                })?;

                // Read the source node first (immutable borrow), then mutate the destination.
                let (data_url, path) = {
                    let src_node = scene
                        .nodes
                        .iter()
                        .find(|n| n.id == src_ep.node_id)
                        .ok_or_else(|| {
                            anyhow!("missing source node '{}' for group input", src_ep.node_id)
                        })?;
                    if src_node.node_type != "ImageFile" {
                        bail!(
                            "unsupported group image binding: expected ImageFile, got {} (node {})",
                            src_node.node_type,
                            src_node.id
                        );
                    }

                    (
                        src_node.params.get("dataUrl").cloned(),
                        src_node.params.get("path").cloned(),
                    )
                };

                // Find the cloned target node.
                let Some(dst) = scene.nodes.iter_mut().find(|n| n.id == *target_new_node_id) else {
                    bail!(
                        "group '{}' inputBindings target node '{}' missing after clone",
                        group_id,
                        target_new_node_id
                    );
                };
                if dst.node_type != "ImageTexture" {
                    bail!(
                        "group '{}' inputBindings maps {} -> {}.image, but target node type is {}",
                        group_id,
                        b.group_port_id,
                        dst.id,
                        dst.node_type
                    );
                }

                copy_image_file_params_into_image_texture(dst, data_url, path);

                any = true;
            } else {
                for src in sources {
                    any = true;
                    scene.connections.push(Connection {
                        id: next_edge(),
                        from: src,
                        to: Endpoint {
                            node_id: target_new_node_id.clone(),
                            port_id: b.to.port_id.clone(),
                        },
                    });
                }
            }

            if !any {
                // No upstream connection provided for this input. This is an authoring error.
                bail!(
                    "GroupInstance '{}' missing required input '{}' for group '{}'",
                    instance_id,
                    b.group_port_id,
                    group_id
                );
            }
        }

        // 4) Wire instance outputs using group.output_bindings.
        // Any connection from instance.out_X is redirected to the cloned source endpoint.
        for b in &group.output_bindings {
            let Some(source_new_node_id) = node_id_map.get(&b.from.node_id) else {
                bail!(
                    "group '{}' outputBindings references missing node '{}'",
                    group_id,
                    b.from.node_id
                );
            };

            let Some(outbounds) = outbound_by_port.get(&b.group_port_id) else {
                // Output can be unused; allow it.
                continue;
            };

            for out_conn in outbounds {
                scene.connections.push(Connection {
                    id: next_edge(),
                    from: Endpoint {
                        node_id: source_new_node_id.clone(),
                        port_id: b.from.port_id.clone(),
                    },
                    to: out_conn.to.clone(),
                });
            }
        }

        // 5) Remove the instance node and its original connections.
        scene
            .nodes
            .retain(|n| !(n.id == instance_id && n.node_type == "GroupInstance"));
        scene
            .connections
            .retain(|c| !(c.from.node_id == instance_id || c.to.node_id == instance_id));
    }

    Ok(())
}

/// Check if a port type spec contains a specific type.
pub fn port_type_contains(t: &schema::PortTypeSpec, candidate: &str) -> bool {
    match t {
        schema::PortTypeSpec::One(s) => s == candidate,
        schema::PortTypeSpec::Many(v) => v.iter().any(|s| s == candidate),
    }
}

/// Check if a port type spec contains any of the candidate types.
pub fn port_type_contains_any_of(t: &schema::PortTypeSpec, candidates: &[&str]) -> bool {
    candidates.iter().any(|c| port_type_contains(t, c))
}

/// Get the output port type for a node.
pub fn get_from_port_type(
    scheme: &schema::NodeScheme,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    port_id: &str,
) -> Option<schema::PortTypeSpec> {
    let node = nodes_by_id.get(node_id)?;

    if node.node_type == "DataParse" {
        let p = node.outputs.iter().find(|p| p.id == port_id)?;
        let ty = p.port_type.as_ref()?;
        return Some(schema::PortTypeSpec::One(ty.clone()));
    }

    let ty = scheme.nodes.get(&node.node_type)?.outputs.get(port_id)?;
    Some(ty.clone())
}

/// Get the input port type for a node.
pub fn get_to_port_type(
    scheme: &schema::NodeScheme,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    port_id: &str,
) -> Option<schema::PortTypeSpec> {
    let node = nodes_by_id.get(node_id)?;
    let node_scheme = scheme.nodes.get(&node.node_type)?;

    if let Some(t) = node_scheme.inputs.get(port_id) {
        return Some(t.clone());
    }

    // Composite supports dynamic layer inputs (dynamic_*) that behave like its base pass input.
    if node.node_type == "Composite" && port_id.starts_with("dynamic_") {
        if let Some(pass_ty) = node_scheme.inputs.get("pass") {
            return Some(pass_ty.clone());
        }
        return Some(schema::PortTypeSpec::One("pass".to_string()));
    }

    None
}

/// If a `pass`-typed input is driven by a primitive shader value (color/vec*/float/int/bool),
/// synthesize a default fullscreen RenderPass (and geometry) and rewire the connection.
pub fn auto_wrap_primitive_pass_inputs(scene: &mut SceneDSL, scheme: &schema::NodeScheme) {
    let nodes_by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|n| (n.id.clone(), n))
        .collect();

    // Best-effort: infer output target size from outputs.composite -> Composite.target -> RenderTexture.
    let mut target_size: Option<[f32; 2]> = None;
    if let Some(outputs) = scene.outputs.as_ref() {
        if let Some(composite_id) = outputs.get("composite") {
            if let Some(conn) = incoming_connection(scene, composite_id, "target") {
                if let Some(tgt_node) = nodes_by_id.get(&conn.from.node_id) {
                    if tgt_node.node_type == "RenderTexture" {
                        let w = cpu_num_u32_min_1(scene, &nodes_by_id, tgt_node, "width", 1024)
                            .ok()
                            .unwrap_or(1024) as f32;
                        let h = cpu_num_u32_min_1(scene, &nodes_by_id, tgt_node, "height", 1024)
                            .ok()
                            .unwrap_or(1024) as f32;
                        target_size = Some([w, h]);
                    }
                }
            }
        }
    }
    let [tgt_w, tgt_h] = target_size.unwrap_or([1024.0, 1024.0]);

    #[derive(Clone)]
    struct WrapPlan {
        conn_index: usize,
        conn_id: String,
        original_from: Endpoint,
        pass_id: String,
        geo_id: String,
    }

    // Plan first (no mutation of vectors while iterating).
    let mut plans: Vec<WrapPlan> = Vec::new();
    for (idx, c) in scene.connections.iter().enumerate() {
        let Some(to_ty) = get_to_port_type(scheme, &nodes_by_id, &c.to.node_id, &c.to.port_id)
        else {
            continue;
        };
        if !port_type_contains(&to_ty, "pass") {
            continue;
        }

        let Some(from_ty) =
            get_from_port_type(scheme, &nodes_by_id, &c.from.node_id, &c.from.port_id)
        else {
            continue;
        };

        if port_type_contains(&from_ty, "pass") {
            continue;
        }

        // Only wrap if the pass input can accept this upstream type.
        // (The graph still needs a synthesized RenderPass to become executable.)
        // No legacy fallback: only wrap when the scheme's compatibility table allows it.
        let should_wrap = schema::port_types_compatible(scheme, &from_ty, &to_ty);

        if !should_wrap {
            continue;
        }

        plans.push(WrapPlan {
            conn_index: idx,
            conn_id: c.id.clone(),
            original_from: c.from.clone(),
            pass_id: format!("sys.auto.fullscreen.pass.{}", c.id),
            geo_id: format!("sys.auto.fullscreen.geo.{}", c.id),
        });
    }

    // Apply plans.
    let mut new_connections: Vec<Connection> = Vec::new();
    for p in &plans {
        let mut geo_params = HashMap::new();
        geo_params.insert("width".to_string(), serde_json::json!(tgt_w));
        geo_params.insert("height".to_string(), serde_json::json!(tgt_h));
        // Rect2DGeometry.x/y are treated as the geometry center in target pixel space
        // (bottom-left origin). For a fullscreen quad, center it at (w/2, h/2).
        geo_params.insert("x".to_string(), serde_json::json!(tgt_w * 0.5));
        geo_params.insert("y".to_string(), serde_json::json!(tgt_h * 0.5));

        scene.nodes.push(Node {
            id: p.geo_id.clone(),
            node_type: "Rect2DGeometry".to_string(),
            params: geo_params,
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        });
        scene.nodes.push(Node {
            id: p.pass_id.clone(),
            node_type: "RenderPass".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        });

        new_connections.push(Connection {
            id: format!("sys.auto.edge.geo.{}", p.conn_id),
            from: Endpoint {
                node_id: p.geo_id.clone(),
                port_id: "geometry".to_string(),
            },
            to: Endpoint {
                node_id: p.pass_id.clone(),
                port_id: "geometry".to_string(),
            },
        });
        new_connections.push(Connection {
            id: format!("sys.auto.edge.material.{}", p.conn_id),
            from: p.original_from.clone(),
            to: Endpoint {
                node_id: p.pass_id.clone(),
                port_id: "material".to_string(),
            },
        });

        if let Some(c) = scene.connections.get_mut(p.conn_index) {
            c.from.node_id = p.pass_id.clone();
            c.from.port_id = "pass".to_string();
        }
    }
    scene.connections.extend(new_connections);
}

/// Prepared scene with topologically sorted nodes and metadata.
pub struct PreparedScene {
    pub scene: SceneDSL,
    pub nodes_by_id: HashMap<String, Node>,
    pub ids: HashMap<String, ResourceName>,
    pub topo_order: Vec<String>,
    pub composite_layers_in_draw_order: Vec<String>,
    pub output_texture_node_id: String,
    pub output_texture_name: ResourceName,
    pub resolution: [u32; 2],

    pub baked_data_parse:
        HashMap<(String, String, String), Vec<crate::renderer::types::BakedValue>>,
}

fn map_baked_type(s: Option<&str>) -> Result<ValueType> {
    let Some(s) = s else {
        return Ok(ValueType::F32);
    };
    let t = s.to_ascii_lowercase();
    match t.as_str() {
        "float" | "f32" | "number" => Ok(ValueType::F32),
        "int" | "i32" => Ok(ValueType::I32),
        "uint" | "u32" => Ok(ValueType::U32),
        "bool" | "boolean" => Ok(ValueType::Bool),
        "vector2" | "vec2" => Ok(ValueType::Vec2),
        "vector3" | "vec3" => Ok(ValueType::Vec3),
        "vector4" | "vec4" | "color" => Ok(ValueType::Vec4),
        "texture" => Ok(ValueType::Texture2D),
        other => bail!("unsupported DataParse port type: {other}"),
    }
}

fn string_param<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
    node.params.get(key)?.as_str()
}

fn data_node_json(nodes_by_id: &HashMap<String, Node>, id: &str) -> Result<serde_json::Value> {
    let data_node = find_node(nodes_by_id, id)?;
    let text = string_param(data_node, "text").unwrap_or("");
    if text.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(text)
        .with_context(|| format!("failed to parse DataNode.text as JSON for {id}"))
}

fn resolve_binding_value(
    nodes_by_id: &HashMap<String, Node>,
    binding: &InputBinding,
    index_value: u32,
) -> Result<serde_json::Value> {
    let Some(SourceBinding {
        node_id,
        output_port_id,
        ..
    }) = binding.source_binding.as_ref()
    else {
        return Ok(serde_json::Value::Null);
    };

    match output_port_id.as_str() {
        "data" => data_node_json(nodes_by_id, node_id),
        "index" => Ok(serde_json::json!(index_value)),
        _ => Ok(serde_json::Value::Null),
    }
}

fn baked_from_json(ty: ValueType, v: &serde_json::Value) -> Result<BakedValue> {
    match ty {
        ValueType::F32 => Ok(BakedValue::F32(
            v.as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
        )),
        ValueType::I32 => Ok(BakedValue::I32(
            v.as_i64().ok_or_else(|| anyhow!("expected int"))? as i32,
        )),
        ValueType::U32 => Ok(BakedValue::U32(
            v.as_u64().ok_or_else(|| anyhow!("expected uint"))? as u32,
        )),
        ValueType::Bool => Ok(BakedValue::Bool(
            v.as_bool().ok_or_else(|| anyhow!("expected bool"))?,
        )),
        ValueType::Vec2 => {
            let arr = v.as_array().ok_or_else(|| anyhow!("expected array"))?;
            if arr.len() != 2 {
                bail!("expected vec2 array length 2, got {}", arr.len());
            }
            Ok(BakedValue::Vec2([
                arr[0].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
                arr[1].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
            ]))
        }
        ValueType::Vec3 => {
            let arr = v.as_array().ok_or_else(|| anyhow!("expected array"))?;
            if arr.len() != 3 {
                bail!("expected vec3 array length 3, got {}", arr.len());
            }
            Ok(BakedValue::Vec3([
                arr[0].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
                arr[1].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
                arr[2].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
            ]))
        }
        ValueType::Vec4 => {
            let arr = v.as_array().ok_or_else(|| anyhow!("expected array"))?;
            if arr.len() != 4 {
                bail!("expected vec4 array length 4, got {}", arr.len());
            }
            Ok(BakedValue::Vec4([
                arr[0].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
                arr[1].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
                arr[2].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
                arr[3].as_f64().ok_or_else(|| anyhow!("expected number"))? as f32,
            ]))
        }

        // DataParse outputs are baked CPU-side; GPU resources are not supported here.
        ValueType::Texture2D => bail!("cannot bake DataParse output type 'texture'"),
    }
}

pub(crate) fn bake_data_parse_nodes(
    nodes_by_id: &HashMap<String, Node>,
    pass_id: &str,
    instance_count: u32,
) -> Result<HashMap<(String, String, String), Vec<BakedValue>>> {
    let mut baked: HashMap<(String, String, String), Vec<BakedValue>> = HashMap::new();
    let mut rt = TsRuntime::new();

    for node in nodes_by_id.values() {
        if node.node_type != "DataParse" {
            continue;
        }

        let src = string_param(node, "source")
            .ok_or_else(|| anyhow!("DataParse missing params.source for {}", node.id))?;

        let port_types: HashMap<String, ValueType> = node
            .outputs
            .iter()
            .map(|p| {
                let ty = map_baked_type(p.port_type.as_deref()).with_context(|| {
                    format!("invalid output port type for {}.{}", node.id, p.id)
                })?;
                Ok((p.id.clone(), ty))
            })
            .collect::<Result<_>>()?;

        let capped_instance_count = instance_count.min(1024);
        for i in 0..capped_instance_count {
            let mut bindings_src = String::new();
            for b in &node.input_bindings {
                let val = resolve_binding_value(nodes_by_id, b, i).with_context(|| {
                    format!(
                        "failed to resolve input binding {} for {}",
                        b.variable_name, node.id
                    )
                })?;
                let json = serde_json::to_string(&val)?;
                bindings_src.push_str(&format!("const {} = {};\n", b.variable_name, json));
            }
            if !node
                .input_bindings
                .iter()
                .any(|b| b.variable_name == "index")
            {
                bindings_src.push_str(&format!("const index = {};\n", i));
            }

            let mut user_src = src.to_string();
            user_src = user_src.replace(" as vec2", "");
            user_src = user_src.replace(" as vec3", "");
            user_src = user_src.replace(" as vec4", "");
            user_src = user_src.replace(" as int", "");
            user_src = user_src.replace(" as i32", "");
            user_src = user_src.replace(" as uint", "");
            user_src = user_src.replace(" as u32", "");
            user_src = user_src.replace(" as float", "");
            user_src = user_src.replace(" as f32", "");
            user_src = user_src.replace(" as number", "");
            user_src = user_src.replace(" as bool", "");
            user_src = user_src.replace(" as boolean", "");

            let script_body = format!("{bindings_src}\n{user_src}\n");
            let script = format!("(function() {{\n{}\n}})()", script_body);
            let out: serde_json::Value = match rt.eval_script(&script) {
                Ok(v) => v,
                Err(_) => serde_json::Value::Object(serde_json::Map::new()),
            };
            let out_obj = out.as_object();

            for p in &node.outputs {
                let key = p.name.as_deref().unwrap_or(p.id.as_str());
                let ty = *port_types
                    .get(&p.id)
                    .ok_or_else(|| anyhow!("missing port type"))?;
                let v = out_obj
                    .and_then(|o| o.get(key))
                    .unwrap_or(&serde_json::Value::Null);
                let baked_v = baked_from_json(ty, v).unwrap_or_else(|_| match ty {
                    ValueType::F32 => BakedValue::F32(0.0),
                    ValueType::I32 => BakedValue::I32(0),
                    ValueType::U32 => BakedValue::U32(0),
                    ValueType::Bool => BakedValue::Bool(false),
                    ValueType::Vec2 => BakedValue::Vec2([0.0, 0.0]),
                    ValueType::Vec3 => BakedValue::Vec3([0.0, 0.0, 0.0]),
                    ValueType::Vec4 => BakedValue::Vec4([0.0, 0.0, 0.0, 0.0]),
                    ValueType::Texture2D => BakedValue::Vec4([0.0, 0.0, 0.0, 0.0]),
                });

                baked
                    .entry((pass_id.to_string(), node.id.clone(), p.id.clone()))
                    .or_default()
                    .push(baked_v);
            }
        }
    }

    Ok(baked)
}

/// Prepare a scene for rendering by validating, tree-shaking, and sorting nodes.
pub fn prepare_scene(input: &SceneDSL) -> Result<PreparedScene> {
    // Expand group instances before any filtering/validation.
    let mut expanded = input.clone();
    expand_group_instances(&mut expanded)?;

    // 1) Locate the RenderTarget-category node. Without it, the graph has no "main" entry.
    let scheme = schema::load_default_scheme()?;
    let render_targets: Vec<&Node> = expanded
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
        bail!("missing RenderTarget category node (e.g. Screen/File)");
    }
    if render_targets.len() != 1 {
        let ids: Vec<String> = render_targets
            .iter()
            .map(|n| format!("{} ({})", n.id, n.node_type))
            .collect();
        bail!(
            "expected exactly 1 RenderTarget node, got {}: {}",
            render_targets.len(),
            ids.join(", ")
        );
    }

    let render_target_id = render_targets[0].id.clone();

    // 2) Keep only the upstream subgraph that contributes to the RenderTarget.
    // This avoids validation/compile failures caused by unrelated leftover subgraphs.
    let keep = upstream_reachable(&expanded, &render_target_id);

    let nodes: Vec<Node> = expanded
        .nodes
        .iter()
        .cloned()
        .filter(|n| keep.contains(&n.id))
        .collect();
    let connections = expanded
        .connections
        .iter()
        .cloned()
        .filter(|c| keep.contains(&c.from.node_id) && keep.contains(&c.to.node_id))
        .collect();
    let scene = SceneDSL {
        version: expanded.version.clone(),
        metadata: expanded.metadata.clone(),
        nodes,
        connections,
        outputs: expanded.outputs.clone(),
        // Groups are not needed after expansion; keep for completeness.
        groups: expanded.groups.clone(),
    };

    // Coerce primitive shader values into passes by synthesizing a fullscreen RenderPass.
    let mut scene = scene;
    auto_wrap_primitive_pass_inputs(&mut scene, &scheme);

    // Inline ImageFile -> ImageTexture.image connections into ImageTexture params.
    inline_image_file_connections_into_image_textures(&mut scene)?;

    // 3) The RenderTarget must be driven by Composite.pass.
    let output_node_id: String = incoming_connection(&scene, &render_target_id, "pass")
        .map(|c| c.from.node_id.clone())
        .ok_or_else(|| anyhow!("RenderTarget.pass has no incoming connection"))?;

    // 4) Validate only the kept subgraph.
    schema::validate_scene_against(&scene, &scheme)?;

    let nodes_by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|n| (n.id.clone(), n))
        .collect();

    let mut ids: HashMap<String, ResourceName> = HashMap::new();
    for n in &scene.nodes {
        ids.insert(n.id.clone(), n.id.clone().into());
    }

    let topo_order = topo_sort(&scene)?;

    let composite_layers_in_draw_order =
        composite_layers_in_draw_order(&scene, &nodes_by_id, &output_node_id)?;

    let output_node = find_node(&nodes_by_id, &output_node_id)?;
    if output_node.node_type != "Composite" {
        bail!(
            "RenderTarget.pass must come from Composite, got {}",
            output_node.node_type
        );
    }

    // New DSL contract: output target must be provided by Composite.target.
    let output_texture_node_id: String = incoming_connection(&scene, &output_node_id, "target")
        .map(|c| c.from.node_id.clone())
        .ok_or_else(|| anyhow!("Composite.target has no incoming connection"))?;

    let output_texture_name: ResourceName = ids
        .get(&output_texture_node_id)
        .cloned()
        .ok_or_else(|| anyhow!("missing name for node: {}", output_texture_node_id))?;

    let output_texture_node = find_node(&nodes_by_id, &output_texture_node_id)?;
    if output_texture_node.node_type != "RenderTexture" {
        bail!(
            "Composite.target must come from RenderTexture, got {}",
            output_texture_node.node_type
        );
    }

    let width = cpu_num_u32_min_1(&scene, &nodes_by_id, output_texture_node, "width", 1024)?;
    let height = cpu_num_u32_min_1(&scene, &nodes_by_id, output_texture_node, "height", 1024)?;
    let resolution = [width, height];

    let baked_data_parse = bake_data_parse_nodes(&nodes_by_id, "__global", 1)?;

    Ok(PreparedScene {
        scene,
        nodes_by_id,
        ids,
        topo_order,
        composite_layers_in_draw_order,
        output_texture_node_id,
        output_texture_name,
        resolution,
        baked_data_parse,
    })
}

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
