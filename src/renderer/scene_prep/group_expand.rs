use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use crate::dsl::{Connection, Endpoint, GroupDSL, Node, SceneDSL};

use super::image_inline::copy_image_file_params_into_image_texture;

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

pub(crate) fn expand_group_instances(scene: &mut SceneDSL) -> Result<usize> {
    // Expand all GroupInstance nodes into the main graph by cloning the referenced group
    // subgraph, rewriting node IDs, and wiring instance I/O using group bindings.
    //
    // This must run before upstream reachability filtering + scheme validation.
    let mut next_edge_id: u64 = 0;
    let mut next_edge = || {
        next_edge_id += 1;
        format!("sys.group.edge.{next_edge_id}")
    };

    let mut expanded_count: usize = 0;

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

        expanded_count += 1;
    }

    Ok(expanded_count)
}
