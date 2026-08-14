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

    if let Some(port) = node.outputs.iter().find(|port| port.id == port_id) {
        if let Some(port_type) = port.port_type.as_ref() {
            return Some(schema::PortTypeSpec::One(port_type.clone()));
        }
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

    // Composite dynamic layers are always surface inputs. Older persisted dynamic ports can carry
    // the source expression's type, so resolve them to the base pass contract before consulting
    // node-local dynamic metadata.
    if node.node_type == "Composite" && port_id.starts_with("dynamic_") {
        if let Some(pass_ty) = node_scheme.inputs.get("pass") {
            return Some(pass_ty.clone());
        }
        return Some(schema::PortTypeSpec::One("pass".to_string()));
    }

    if let Some(port) = node.inputs.iter().find(|port| port.id == port_id) {
        if let Some(port_type) = port.port_type.as_ref() {
            return Some(schema::PortTypeSpec::One(port_type.clone()));
        }
    }

    None
}

/// Materialize every non-pass source connected to a `pass` input as a fullscreen RenderPass.
///
/// Shader values compile directly as the generated pass material. A raw `ImageTexture.texture`
/// source uses the same node's sampled `color` output, preserving its UV and sampler semantics.
pub(crate) fn materialize_pass_inputs(scene: &mut SceneDSL, scheme: &schema::NodeScheme) -> usize {
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
        blend_params: HashMap<String, serde_json::Value>,
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

        let blend_params = nodes_by_id
            .get(&c.to.node_id)
            .filter(|n| n.node_type == "Composite")
            .map(|n| {
                const BLEND_KEYS: &[&str] = &[
                    "blend_preset",
                    "blendfunc",
                    "src_factor",
                    "dst_factor",
                    "src_alpha_factor",
                    "dst_alpha_factor",
                ];
                n.params
                    .iter()
                    .filter(|(k, _)| BLEND_KEYS.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();

        let original_from = if port_type_contains(&from_ty, "texture") {
            let Some(source_node) = nodes_by_id.get(&c.from.node_id) else {
                continue;
            };
            if source_node.node_type != "ImageTexture" || c.from.port_id != "texture" {
                continue;
            }
            Endpoint {
                node_id: c.from.node_id.clone(),
                port_id: "color".to_string(),
            }
        } else {
            c.from.clone()
        };

        plans.push(WrapPlan {
            conn_index: idx,
            conn_id: c.id.clone(),
            original_from,
            pass_id: format!("sys.auto.fullscreen.pass.{}", c.id),
            geo_id: format!("sys.auto.fullscreen.geo.{}", c.id),
            blend_params,
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
            wgsl_override: None,
        });
        scene.nodes.push(Node {
            id: p.pass_id.clone(),
            node_type: "RenderPass".to_string(),
            params: p.blend_params.clone(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::dsl::{Connection, Endpoint, Metadata, Node, NodePort, SceneDSL};

    use super::materialize_pass_inputs;

    fn node(id: &str, node_type: &str) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        }
    }

    fn scene(nodes: Vec<Node>, from: Endpoint, to: Endpoint) -> SceneDSL {
        SceneDSL {
            version: "6.0".to_string(),
            metadata: Metadata {
                name: "materialize pass".to_string(),
                created: None,
                modified: None,
            },
            nodes,
            connections: vec![Connection {
                id: "edge".to_string(),
                from,
                to,
            }],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        }
    }

    #[test]
    fn materializes_raw_image_texture_through_its_color_output() {
        let mut scene = scene(
            vec![
                node("image", "ImageTexture"),
                node("composite", "Composite"),
            ],
            Endpoint {
                node_id: "image".to_string(),
                port_id: "texture".to_string(),
            },
            Endpoint {
                node_id: "composite".to_string(),
                port_id: "pass".to_string(),
            },
        );

        let count =
            materialize_pass_inputs(&mut scene, &crate::schema::load_default_scheme().unwrap());

        assert_eq!(count, 1);
        assert!(scene.connections.iter().any(|connection| {
            connection.from.node_id == "image"
                && connection.from.port_id == "color"
                && connection.to.node_id == "sys.auto.fullscreen.pass.edge"
                && connection.to.port_id == "material"
        }));
    }

    #[test]
    fn materializes_composite_dynamic_inputs_even_with_persisted_color_metadata() {
        let mut composite = node("composite", "Composite");
        composite.inputs.push(NodePort {
            id: "dynamic_color".to_string(),
            name: Some("Layer".to_string()),
            port_type: Some("color".to_string()),
            array_length: None,
        });
        let mut scene = scene(
            vec![node("color", "ColorInput"), composite],
            Endpoint {
                node_id: "color".to_string(),
                port_id: "color".to_string(),
            },
            Endpoint {
                node_id: "composite".to_string(),
                port_id: "dynamic_color".to_string(),
            },
        );

        let count =
            materialize_pass_inputs(&mut scene, &crate::schema::load_default_scheme().unwrap());

        assert_eq!(count, 1);
        assert!(scene.connections.iter().any(|connection| {
            connection.from.node_id == "sys.auto.fullscreen.pass.edge"
                && connection.from.port_id == "pass"
                && connection.to.node_id == "composite"
                && connection.to.port_id == "dynamic_color"
        }));
    }

    #[test]
    fn materializes_dynamic_custom_shader_pass_inputs_and_blur_sources() {
        let mut shader = node("shader", "ShaderMaterial");
        shader.inputs.push(NodePort {
            id: "resource:content".to_string(),
            name: Some("content".to_string()),
            port_type: Some("pass".to_string()),
            array_length: None,
        });
        let mut shader_scene = scene(
            vec![node("color", "ColorInput"), shader],
            Endpoint {
                node_id: "color".to_string(),
                port_id: "color".to_string(),
            },
            Endpoint {
                node_id: "shader".to_string(),
                port_id: "resource:content".to_string(),
            },
        );
        assert_eq!(
            materialize_pass_inputs(
                &mut shader_scene,
                &crate::schema::load_default_scheme().unwrap()
            ),
            1
        );

        let mut blur_scene = scene(
            vec![
                node("color", "ColorInput"),
                node("blur", "GuassianBlurPass"),
            ],
            Endpoint {
                node_id: "color".to_string(),
                port_id: "color".to_string(),
            },
            Endpoint {
                node_id: "blur".to_string(),
                port_id: "pass".to_string(),
            },
        );
        assert_eq!(
            materialize_pass_inputs(
                &mut blur_scene,
                &crate::schema::load_default_scheme().unwrap()
            ),
            1
        );
    }
}
