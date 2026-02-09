use std::collections::HashMap;

use crate::{
    dsl::{Connection, Endpoint, Node, SceneDSL, incoming_connection},
    renderer::utils::cpu_num_u32_min_1,
    schema,
};

/// Check if a port type spec contains a specific type.
fn port_type_contains(t: &schema::PortTypeSpec, candidate: &str) -> bool {
    match t {
        schema::PortTypeSpec::One(s) => s == candidate,
        schema::PortTypeSpec::Many(v) => v.iter().any(|s| s == candidate),
    }
}

/// Get the output port type for a node.
fn get_from_port_type(
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
fn get_to_port_type(
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
pub(crate) fn auto_wrap_primitive_pass_inputs(
    scene: &mut SceneDSL,
    scheme: &schema::NodeScheme,
) -> usize {
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
        geo_params.insert("size".to_string(), serde_json::json!([tgt_w, tgt_h]));
        // Rect2DGeometry.position is the geometry center in target pixel space
        // (bottom-left origin). For a fullscreen quad, center it at (w/2, h/2).
        geo_params.insert(
            "position".to_string(),
            serde_json::json!([tgt_w * 0.5, tgt_h * 0.5]),
        );

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

    plans.len()
}
