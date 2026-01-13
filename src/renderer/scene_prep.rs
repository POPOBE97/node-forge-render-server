//! Scene preparation and validation module.
//!
//! This module handles:
//! - Port type utilities for connection validation
//! - Auto-wrapping of primitive values into render passes
//! - Scene validation and topological sorting
//! - Composite layer ordering

use std::collections::HashMap;
use anyhow::{anyhow, bail, Result};
use rust_wgpu_fiber::ResourceName;

use crate::{
    dsl::{
        find_node, incoming_connection, parse_f32, parse_u32,
        Connection, Endpoint, Node, SceneDSL,
    },
    graph::{topo_sort, upstream_reachable},
    schema,
};

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
                        let w = parse_f32(&tgt_node.params, "width")
                            .or_else(|| parse_u32(&tgt_node.params, "width").map(|x| x as f32))
                            .unwrap_or(1024.0)
                            .max(1.0);
                        let h = parse_f32(&tgt_node.params, "height")
                            .or_else(|| parse_u32(&tgt_node.params, "height").map(|x| x as f32))
                            .unwrap_or(1024.0)
                            .max(1.0);
                        target_size = Some([w, h]);
                    }
                }
            }
        }
    }
    let [tgt_w, tgt_h] = target_size.unwrap_or([1024.0, 1024.0]);

    let primitive_candidates: [&str; 10] = [
        "color",
        "vector2",
        "vector3",
        "vector4",
        "float",
        "int",
        "bool",
        // Common aliases used by some editors/schemes.
        "vec2",
        "vec3",
        "vec4",
    ];

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
        if !port_type_contains_any_of(&from_ty, &primitive_candidates) {
            continue;
        }

        plans.push(WrapPlan {
            conn_index: idx,
            conn_id: c.id.clone(),
            original_from: c.from.clone(),
            pass_id: format!("__auto_fullscreen_pass__{}", c.id),
            geo_id: format!("__auto_fullscreen_geo__{}", c.id),
        });
    }

    // Apply plans.
    let mut new_connections: Vec<Connection> = Vec::new();
    for p in &plans {
        let mut geo_params = HashMap::new();
        geo_params.insert("width".to_string(), serde_json::json!(tgt_w));
        geo_params.insert("height".to_string(), serde_json::json!(tgt_h));
        geo_params.insert("x".to_string(), serde_json::json!(0.0));
        geo_params.insert("y".to_string(), serde_json::json!(0.0));

        scene.nodes.push(Node {
            id: p.geo_id.clone(),
            node_type: "Rect2DGeometry".to_string(),
            params: geo_params,
            inputs: Vec::new(),
        });
        scene.nodes.push(Node {
            id: p.pass_id.clone(),
            node_type: "RenderPass".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
        });

        new_connections.push(Connection {
            id: format!("__auto_edge_geo__{}", p.conn_id),
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
            id: format!("__auto_edge_mat__{}", p.conn_id),
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
}

/// Prepare a scene for rendering by validating, tree-shaking, and sorting nodes.
pub fn prepare_scene(input: &SceneDSL) -> Result<PreparedScene> {
    // 1) Locate the RenderTarget-category node. Without it, the graph has no "main" entry.
    let scheme = schema::load_default_scheme()?;
    let render_targets: Vec<&Node> = input
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
    let keep = upstream_reachable(input, &render_target_id);

    let nodes: Vec<Node> = input
        .nodes
        .iter()
        .cloned()
        .filter(|n| keep.contains(&n.id))
        .collect();
    let connections = input
        .connections
        .iter()
        .cloned()
        .filter(|c| keep.contains(&c.from.node_id) && keep.contains(&c.to.node_id))
        .collect();
    let scene = SceneDSL {
        version: input.version.clone(),
        metadata: input.metadata.clone(),
        nodes,
        connections,
        outputs: input.outputs.clone(),
    };

    // Coerce primitive shader values into passes by synthesizing a fullscreen RenderPass.
    let mut scene = scene;
    auto_wrap_primitive_pass_inputs(&mut scene, &scheme);

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

    let width = parse_u32(&output_texture_node.params, "width").unwrap_or(1024);
    let height = parse_u32(&output_texture_node.params, "height").unwrap_or(1024);
    let resolution = [width, height];

    Ok(PreparedScene {
        scene,
        nodes_by_id,
        ids,
        topo_order,
        composite_layers_in_draw_order,
        output_texture_node_id,
        output_texture_name,
        resolution,
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
        bail!(
            "expected Composite node, got {}",
            composite_node.node_type
        );
    }

    let mut layers: Vec<String> = Vec::new();

    // Static layer
    if let Some(conn) = incoming_connection(scene, composite_node_id, "pass") {
        layers.push(conn.from.node_id.clone());
    }

    // Dynamic layers (sorted by parameter index or port name)
    let mut dynamic: Vec<(String, String)> = Vec::new();
    for conn in &scene.connections {
        if conn.to.node_id == composite_node_id && conn.to.port_id.starts_with("dynamic_") {
            dynamic.push((conn.to.port_id.clone(), conn.from.node_id.clone()));
        }
    }
    dynamic.sort_by(|a, b| a.0.cmp(&b.0));

    for (_, node_id) in dynamic {
        layers.push(node_id);
    }

    Ok(layers)
}
