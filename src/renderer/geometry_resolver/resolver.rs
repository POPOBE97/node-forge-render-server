use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Result, anyhow, bail};
use rust_wgpu_fiber::ResourceName;

use crate::{
    asset_store::AssetStore,
    dsl::{Node, SceneDSL, find_node, incoming_connection},
    renderer::{
        geometry_resolver::types::{
            CoordDomain, NodeRole, ResolvedCompositionContext, ResolvedDrawContext,
            ResolvedGeometry, ResolvedGeometrySource, ResolvedSceneContexts,
            is_composition_route_node_type, is_draw_pass_node_type, is_pass_like_node_type,
        },
        render_plan::resolve_geometry_for_render_pass,
        scene_prep::composite_layers_in_draw_order,
        utils::cpu_num_u32_min_1,
    },
};

fn nearest_downstream_composition(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    start_node_id: &str,
    live_pass_like_nodes: &HashSet<String>,
) -> Result<Option<String>> {
    let mut queue: VecDeque<String> = VecDeque::new();
    let mut visited: HashSet<String> = HashSet::new();
    queue.push_back(start_node_id.to_string());

    while let Some(node_id) = queue.pop_front() {
        if !live_pass_like_nodes.contains(&node_id) {
            continue;
        }
        if !visited.insert(node_id.clone()) {
            continue;
        }
        let node = find_node(nodes_by_id, &node_id)?;
        if is_composition_route_node_type(&node.node_type) {
            return Ok(Some(node_id));
        }

        for conn in &scene.connections {
            if conn.from.node_id != node_id {
                continue;
            }
            if conn.from.port_id != "pass" {
                continue;
            }
            queue.push_back(conn.to.node_id.clone());
        }
    }

    Ok(None)
}

fn collect_live_pass_like_nodes(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    composition_layers_by_id: &HashMap<String, Vec<String>>,
) -> HashSet<String> {
    let mut live: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    for (composition_id, layers) in composition_layers_by_id {
        live.insert(composition_id.clone());
        for layer_id in layers {
            if let Some(layer_node) = nodes_by_id.get(layer_id) {
                if is_pass_like_node_type(&layer_node.node_type) {
                    queue.push_back(layer_id.clone());
                }
            }
        }
    }

    while let Some(node_id) = queue.pop_front() {
        if !live.insert(node_id.clone()) {
            continue;
        }

        for conn in &scene.connections {
            if conn.to.node_id != node_id {
                continue;
            }
            if conn.from.port_id != "pass" {
                continue;
            }
            let Some(from_node) = nodes_by_id.get(&conn.from.node_id) else {
                continue;
            };
            if is_pass_like_node_type(&from_node.node_type) {
                queue.push_back(conn.from.node_id.clone());
            }
        }
    }

    live
}

fn build_coord_domain(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    ids: &HashMap<String, ResourceName>,
    composition_node_id: &str,
    default_resolution: [u32; 2],
) -> Result<CoordDomain> {
    let target_conn =
        incoming_connection(scene, composition_node_id, "target").ok_or_else(|| {
            anyhow!("Composition.target has no incoming connection for '{composition_node_id}'")
        })?;
    let render_texture_node_id = target_conn.from.node_id.clone();
    let target_node = find_node(nodes_by_id, &render_texture_node_id)?;
    if target_node.node_type != "RenderTexture" {
        bail!(
            "Composition.target must come from RenderTexture, got {} for '{}'",
            target_node.node_type,
            composition_node_id
        );
    }

    let width = cpu_num_u32_min_1(
        scene,
        nodes_by_id,
        target_node,
        "width",
        default_resolution[0],
    )?;
    let height = cpu_num_u32_min_1(
        scene,
        nodes_by_id,
        target_node,
        "height",
        default_resolution[1],
    )?;
    let texture_name = ids
        .get(&render_texture_node_id)
        .cloned()
        .ok_or_else(|| anyhow!("missing resource name for node '{render_texture_node_id}'"))?;

    Ok(CoordDomain {
        composition_node_id: composition_node_id.to_string(),
        render_texture_node_id,
        texture_name,
        size_px: [width as f32, height as f32],
    })
}

fn resolve_draw_geometry(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    ids: &HashMap<String, ResourceName>,
    pass_node_id: &str,
    coord_size_px: [f32; 2],
    asset_store: Option<&AssetStore>,
) -> Result<ResolvedGeometry> {
    let pass_node = find_node(nodes_by_id, pass_node_id)?;
    if pass_node.node_type != "RenderPass" {
        return Ok(ResolvedGeometry {
            size_px: coord_size_px,
            center_px: [coord_size_px[0] * 0.5, coord_size_px[1] * 0.5],
            source: ResolvedGeometrySource::FullscreenFallback,
        });
    }

    let Some(geo_conn) = incoming_connection(scene, pass_node_id, "geometry") else {
        return Ok(ResolvedGeometry {
            size_px: coord_size_px,
            center_px: [coord_size_px[0] * 0.5, coord_size_px[1] * 0.5],
            source: ResolvedGeometrySource::FullscreenFallback,
        });
    };
    let geometry_node_id = geo_conn.from.node_id.clone();
    let (_geo_buf, geo_w, geo_h, geo_x, geo_y, ..) = resolve_geometry_for_render_pass(
        scene,
        nodes_by_id,
        ids,
        &geometry_node_id,
        coord_size_px,
        None,
        asset_store,
    )?;

    Ok(ResolvedGeometry {
        size_px: [geo_w, geo_h],
        // Preserve resolved geometry placement for both processing chains and composition.
        center_px: [geo_x, geo_y],
        source: ResolvedGeometrySource::DirectGeometry(geometry_node_id),
    })
}

pub fn resolve_scene_draw_contexts(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    ids: &HashMap<String, ResourceName>,
    default_resolution: [u32; 2],
    asset_store: Option<&AssetStore>,
) -> Result<ResolvedSceneContexts> {
    let mut out = ResolvedSceneContexts::default();
    let mut composition_layers_by_id: HashMap<String, Vec<String>> = HashMap::new();

    for (node_id, node) in nodes_by_id {
        let role = if is_draw_pass_node_type(&node.node_type) {
            NodeRole::DrawPass
        } else if is_composition_route_node_type(&node.node_type) {
            NodeRole::CompositionRoute
        } else {
            NodeRole::Other
        };
        out.node_roles.insert(node_id.clone(), role);
    }

    for (node_id, node) in nodes_by_id {
        if !is_composition_route_node_type(&node.node_type) {
            continue;
        }
        let coord_domain =
            build_coord_domain(scene, nodes_by_id, ids, node_id, default_resolution)?;
        let layers = composite_layers_in_draw_order(scene, nodes_by_id, node_id)?;
        composition_layers_by_id.insert(node_id.clone(), layers.clone());
        for src_id in &layers {
            out.composition_consumers_by_source
                .entry(src_id.clone())
                .or_default()
                .push(node_id.clone());
        }
        out.composition_contexts.insert(
            node_id.clone(),
            ResolvedCompositionContext {
                composition_node_id: node_id.clone(),
                target_texture_node_id: coord_domain.render_texture_node_id.clone(),
                target_texture_name: coord_domain.texture_name.clone(),
                target_size_px: coord_domain.size_px,
                layer_node_ids: layers,
            },
        );
    }

    let live_pass_like_nodes =
        collect_live_pass_like_nodes(scene, nodes_by_id, &composition_layers_by_id);

    for conn in &scene.connections {
        if !live_pass_like_nodes.contains(&conn.from.node_id)
            || !live_pass_like_nodes.contains(&conn.to.node_id)
        {
            continue;
        }
        let Some(from_node) = nodes_by_id.get(&conn.from.node_id) else {
            continue;
        };
        if !is_draw_pass_node_type(&from_node.node_type) {
            continue;
        }
        let Some(to_node) = nodes_by_id.get(&conn.to.node_id) else {
            continue;
        };
        if !(is_draw_pass_node_type(&to_node.node_type)
            || is_composition_route_node_type(&to_node.node_type))
        {
            continue;
        }

        let composition_node_id = if is_composition_route_node_type(&to_node.node_type) {
            conn.to.node_id.clone()
        } else {
            let nearest = nearest_downstream_composition(
                scene,
                nodes_by_id,
                &conn.to.node_id,
                &live_pass_like_nodes,
            )?;
            let Some(composition_node_id) = nearest else {
                // Dead pass branch: not consumed by any composition layer.
                continue;
            };
            composition_node_id
        };

        let coord_domain = build_coord_domain(
            scene,
            nodes_by_id,
            ids,
            &composition_node_id,
            default_resolution,
        )?;
        let geometry = resolve_draw_geometry(
            scene,
            nodes_by_id,
            ids,
            &conn.from.node_id,
            coord_domain.size_px,
            asset_store,
        )?;
        out.draw_contexts.push(ResolvedDrawContext {
            pass_node_id: conn.from.node_id.clone(),
            downstream_node_id: conn.to.node_id.clone(),
            downstream_port_id: conn.to.port_id.clone(),
            coord_domain,
            geometry,
        });
    }

    // Keep deterministic ordering for consumers.
    for consumer_ids in out.composition_consumers_by_source.values_mut() {
        consumer_ids.sort();
        consumer_ids.dedup();
    }

    // Sanity: pass-like nodes must have role classification.
    for (node_id, node) in nodes_by_id {
        if is_pass_like_node_type(&node.node_type) && !out.node_roles.contains_key(node_id) {
            bail!("internal: missing role classification for '{node_id}'");
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use crate::dsl::{Connection, Endpoint, Metadata, Node, SceneDSL};

    use super::*;

    fn node(id: &str, node_type: &str, params: serde_json::Value) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params: params
                .as_object()
                .cloned()
                .map(|m| m.into_iter().collect())
                .unwrap_or_default(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_bindings: Vec::new(),
        }
    }

    fn conn(
        id: &str,
        from_node: &str,
        from_port: &str,
        to_node: &str,
        to_port: &str,
    ) -> Connection {
        Connection {
            id: id.to_string(),
            from: Endpoint {
                node_id: from_node.to_string(),
                port_id: from_port.to_string(),
            },
            to: Endpoint {
                node_id: to_node.to_string(),
                port_id: to_port.to_string(),
            },
        }
    }

    fn ids_for(nodes: &[Node]) -> HashMap<String, ResourceName> {
        nodes
            .iter()
            .map(|n| (n.id.clone(), n.id.clone().into()))
            .collect()
    }

    fn scene(nodes: Vec<Node>, connections: Vec<Connection>) -> SceneDSL {
        SceneDSL {
            version: "1.0.0".to_string(),
            metadata: Metadata {
                name: "test".to_string(),
                created: None,
                modified: None,
            },
            nodes,
            connections,
            outputs: None,
            groups: Vec::new(),
            assets: HashMap::new(),
        }
    }

    #[test]
    fn coord_inference_uses_closest_downstream_composition_on_branch() {
        let nodes = vec![
            node("rp", "RenderPass", json!({})),
            node("ds", "Downsample", json!({})),
            node("comp_a", "Composite", json!({})),
            node("comp_b", "Composite", json!({})),
            node(
                "rt_a",
                "RenderTexture",
                json!({"width": 100, "height": 200}),
            ),
            node(
                "rt_b",
                "RenderTexture",
                json!({"width": 300, "height": 400}),
            ),
        ];
        let ids = ids_for(&nodes);
        let scene = scene(
            nodes.clone(),
            vec![
                conn("c1", "rp", "pass", "ds", "source"),
                conn("c2", "ds", "pass", "comp_a", "pass"),
                conn("c3", "rp", "pass", "comp_b", "pass"),
                conn("c4", "rt_a", "texture", "comp_a", "target"),
                conn("c5", "rt_b", "texture", "comp_b", "target"),
            ],
        );
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

        let resolved =
            resolve_scene_draw_contexts(&scene, &nodes_by_id, &ids, [1920, 1080], None).unwrap();
        let rp_to_ds = resolved
            .draw_contexts
            .iter()
            .find(|c| c.pass_node_id == "rp" && c.downstream_node_id == "ds")
            .unwrap();
        assert_eq!(rp_to_ds.coord_domain.composition_node_id, "comp_a");
        assert_eq!(rp_to_ds.coord_domain.size_px, [100.0, 200.0]);

        let rp_to_comp_b = resolved
            .draw_contexts
            .iter()
            .find(|c| c.pass_node_id == "rp" && c.downstream_node_id == "comp_b")
            .unwrap();
        assert_eq!(rp_to_comp_b.coord_domain.composition_node_id, "comp_b");
        assert_eq!(rp_to_comp_b.coord_domain.size_px, [300.0, 400.0]);
    }

    #[test]
    fn rect2d_missing_size_and_position_falls_back_to_coord_domain() {
        let nodes = vec![
            node("rect", "Rect2DGeometry", json!({})),
            node("rp", "RenderPass", json!({})),
            node("comp", "Composite", json!({})),
            node("rt", "RenderTexture", json!({"width": 320, "height": 240})),
        ];
        let ids = ids_for(&nodes);
        let scene = scene(
            nodes.clone(),
            vec![
                conn("c1", "rect", "geometry", "rp", "geometry"),
                conn("c2", "rp", "pass", "comp", "pass"),
                conn("c3", "rt", "texture", "comp", "target"),
            ],
        );
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

        let resolved =
            resolve_scene_draw_contexts(&scene, &nodes_by_id, &ids, [1920, 1080], None).unwrap();
        let rp_ctx = resolved
            .draw_contexts
            .iter()
            .find(|c| c.pass_node_id == "rp" && c.downstream_node_id == "comp")
            .unwrap();
        assert_eq!(rp_ctx.geometry.size_px, [320.0, 240.0]);
        assert_eq!(rp_ctx.geometry.center_px, [160.0, 120.0]);
    }

    #[test]
    fn processing_edges_preserve_render_pass_geometry_center() {
        let nodes = vec![
            node(
                "rect",
                "Rect2DGeometry",
                json!({
                    "size": {"x": 40.0, "y": 30.0},
                    "position": {"x": 12.0, "y": 18.0}
                }),
            ),
            node("rp", "RenderPass", json!({})),
            node("ds", "Downsample", json!({})),
            node("comp", "Composite", json!({})),
            node("rt", "RenderTexture", json!({"width": 320, "height": 240})),
        ];
        let ids = ids_for(&nodes);
        let scene = scene(
            nodes.clone(),
            vec![
                conn("c1", "rect", "geometry", "rp", "geometry"),
                conn("c2", "rp", "pass", "ds", "source"),
                conn("c3", "ds", "pass", "comp", "pass"),
                conn("c4", "rt", "texture", "comp", "target"),
            ],
        );
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

        let resolved =
            resolve_scene_draw_contexts(&scene, &nodes_by_id, &ids, [1920, 1080], None).unwrap();
        let rp_to_ds = resolved
            .draw_contexts
            .iter()
            .find(|c| c.pass_node_id == "rp" && c.downstream_node_id == "ds")
            .unwrap();
        assert_eq!(rp_to_ds.geometry.size_px, [40.0, 30.0]);
        assert_eq!(rp_to_ds.geometry.center_px, [12.0, 18.0]);
    }

    #[test]
    fn unconnected_processing_chain_is_treeshaken_for_coord_inference() {
        let nodes = vec![
            node("rp_live", "RenderPass", json!({})),
            node("rp_dead", "RenderPass", json!({})),
            node("ds_dead", "Downsample", json!({})),
            node("comp", "Composite", json!({})),
            node("rt", "RenderTexture", json!({"width": 320, "height": 240})),
        ];
        let ids = ids_for(&nodes);
        let scene = scene(
            nodes.clone(),
            vec![
                conn("c1", "rp_live", "pass", "comp", "pass"),
                conn("c2", "rt", "texture", "comp", "target"),
                conn("c3", "rp_dead", "pass", "ds_dead", "source"),
            ],
        );
        let nodes_by_id: HashMap<String, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

        let resolved =
            resolve_scene_draw_contexts(&scene, &nodes_by_id, &ids, [1920, 1080], None).unwrap();
        assert!(
            resolved
                .draw_contexts
                .iter()
                .any(|c| c.pass_node_id == "rp_live" && c.downstream_node_id == "comp")
        );
        assert!(
            !resolved
                .draw_contexts
                .iter()
                .any(|c| c.pass_node_id == "rp_dead" || c.downstream_node_id == "ds_dead")
        );
    }
}
