use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use crate::dsl::{Connection, Endpoint, Node, NodePort, SceneDSL};

const GEOMETRY_PARAM_KEYS: &[&str] = &["size", "position", "smooth"];
const IMAGE_PASS_BLUR_EXPANDED_PARAM: &str = "sys.imagePassBlurExpanded";
pub(crate) const IMAGE_PASS_TEXTURE_SIZE_SOURCE_PARAM: &str = "sys.imagePassTextureSizeSource";

fn selected_params(
    source: &HashMap<String, serde_json::Value>,
    keys: &[&str],
) -> HashMap<String, serde_json::Value> {
    keys.iter()
        .filter_map(|key| {
            source
                .get(*key)
                .cloned()
                .map(|value| ((*key).to_string(), value))
        })
        .collect()
}

fn typed_port(id: &str, port_type: &str) -> NodePort {
    NodePort {
        id: id.to_string(),
        name: None,
        port_type: Some(port_type.to_string()),
        array_length: None,
    }
}

fn float_input_node(id: String, value: serde_json::Value) -> Node {
    Node {
        id,
        node_type: "FloatInput".to_string(),
        params: HashMap::from([("value".to_string(), value)]),
        inputs: Vec::new(),
        outputs: Vec::new(),
        input_bindings: Vec::new(),
        wgsl_override: None,
    }
}

fn numeric_param_f32(node: &Node, key: &str, default: f32) -> f32 {
    node.params
        .get(key)
        .and_then(|value| {
            value
                .as_f64()
                .map(|value| value as f32)
                .or_else(|| value.as_i64().map(|value| value as f32))
                .or_else(|| value.as_u64().map(|value| value as f32))
        })
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

fn gaussian_image_pass_params(image_pass: &Node) -> HashMap<String, serde_json::Value> {
    HashMap::from([
        (
            "blurRadius".to_string(),
            image_pass
                .params
                .get("blurRadius")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(0)),
        ),
        ("mode".to_string(), serde_json::json!("standard")),
        ("extend".to_string(), serde_json::json!(false)),
        (
            "blend_preset".to_string(),
            serde_json::json!("premul_alpha"),
        ),
        ("blendfunc".to_string(), serde_json::json!("add")),
        ("src_factor".to_string(), serde_json::json!("one")),
        (
            "dst_factor".to_string(),
            serde_json::json!("one-minus-src-alpha"),
        ),
        ("src_alpha_factor".to_string(), serde_json::json!("one")),
        (
            "dst_alpha_factor".to_string(),
            serde_json::json!("one-minus-src-alpha"),
        ),
        (
            IMAGE_PASS_BLUR_EXPANDED_PARAM.to_string(),
            serde_json::json!(true),
        ),
    ])
}

/// Lower the author-facing ImagePass macro node into the canonical render graph.
///
/// Static zero blur lowers the authored id directly to a RenderPass. An active or potentially
/// dynamic blur keeps the authored ImagePass id as the Gaussian layer and moves the source draw to
/// a private RenderPass. Generated geometry, sampling material, and source-pass ids remain private.
pub(crate) fn expand_image_passes(scene: &mut SceneDSL) -> Result<usize> {
    let image_passes: Vec<Node> = scene
        .nodes
        .iter()
        .filter(|node| {
            node.node_type == "ImagePass"
                && node
                    .params
                    .get(IMAGE_PASS_BLUR_EXPANDED_PARAM)
                    .and_then(|value| value.as_bool())
                    != Some(true)
        })
        .cloned()
        .collect();
    if image_passes.is_empty() {
        return Ok(0);
    }

    let mut node_ids: HashSet<String> = scene.nodes.iter().map(|node| node.id.clone()).collect();
    let mut connection_ids: HashSet<String> = scene
        .connections
        .iter()
        .map(|connection| connection.id.clone())
        .collect();
    let dynamic_render_keys = scene
        .state_machine
        .as_ref()
        .map(crate::state_machine::dynamic_render_keys)
        .unwrap_or_default();

    for image_pass in &image_passes {
        let has_pass_output_route = scene.connections.iter().any(|connection| {
            connection.from.node_id == image_pass.id && connection.from.port_id == "pass"
        });
        let has_texture_output_route = scene.connections.iter().any(|connection| {
            connection.from.node_id == image_pass.id && connection.from.port_id == "texture"
        });
        // The render planner splits simultaneous public pass/texture consumers before expansion.
        // A texture-only invocation owns a GeoSize-local source surface; pass invocations retain
        // the downstream TargetContext. Direct preparation without route splitting defaults to
        // the pass domain when both outputs are present.
        let local_texture_invocation = has_texture_output_route && !has_pass_output_route;
        let has_authored_size = image_pass.params.contains_key("size")
            || image_pass
                .input_bindings
                .iter()
                .any(|binding| binding.port_id == "size")
            || scene.connections.iter().any(|connection| {
                connection.to.node_id == image_pass.id && connection.to.port_id == "size"
            });
        let infer_texture_size = local_texture_invocation && !has_authored_size;
        let has_alpha_connection = scene.connections.iter().any(|connection| {
            connection.to.node_id == image_pass.id && connection.to.port_id == "alpha"
        });
        let has_blur_radius_connection = scene.connections.iter().any(|connection| {
            connection.to.node_id == image_pass.id && connection.to.port_id == "blurRadius"
        });
        let blur_radius_key = format!("{}:blurRadius", image_pass.id);
        let blur_enabled = numeric_param_f32(image_pass, "blurRadius", 0.0) > 0.0
            || has_blur_radius_connection
            || dynamic_render_keys.contains(&blur_radius_key);
        let geometry_id = format!("sys.image-pass.{}.geometry", image_pass.id);
        let sample_id = format!("sys.image-pass.{}.sample", image_pass.id);
        let render_id = format!("sys.image-pass.{}.render", image_pass.id);
        let alpha_default_id = format!("sys.image-pass.{}.alpha.default", image_pass.id);
        let alpha_min_id = format!("sys.image-pass.{}.alpha.min", image_pass.id);
        let alpha_max_id = format!("sys.image-pass.{}.alpha.max", image_pass.id);
        let alpha_clamp_id = format!("sys.image-pass.{}.alpha.clamp", image_pass.id);
        let alpha_multiply_id = format!("sys.image-pass.{}.alpha.multiply", image_pass.id);
        let geometry_edge_id = format!("sys.image-pass.{}.geometry.edge", image_pass.id);
        let color_edge_id = format!("sys.image-pass.{}.color.edge", image_pass.id);
        let alpha_default_edge_id = format!("sys.image-pass.{}.alpha.default.edge", image_pass.id);
        let alpha_min_edge_id = format!("sys.image-pass.{}.alpha.min.edge", image_pass.id);
        let alpha_max_edge_id = format!("sys.image-pass.{}.alpha.max.edge", image_pass.id);
        let alpha_clamp_edge_id = format!("sys.image-pass.{}.alpha.clamp.edge", image_pass.id);
        let material_edge_id = format!("sys.image-pass.{}.material.edge", image_pass.id);
        let render_edge_id = format!("sys.image-pass.{}.render.edge", image_pass.id);

        let mut internal_node_ids = vec![
            geometry_id.clone(),
            sample_id.clone(),
            alpha_min_id.clone(),
            alpha_max_id.clone(),
            alpha_clamp_id.clone(),
            alpha_multiply_id.clone(),
        ];
        if !has_alpha_connection {
            internal_node_ids.push(alpha_default_id.clone());
        }
        if blur_enabled {
            internal_node_ids.push(render_id.clone());
        }
        for internal_id in internal_node_ids {
            if !node_ids.insert(internal_id.clone()) {
                bail!(
                    "ImagePass '{}' cannot reserve internal node id '{}'",
                    image_pass.id,
                    internal_id
                );
            }
        }
        let mut internal_connection_ids = vec![
            geometry_edge_id.clone(),
            color_edge_id.clone(),
            alpha_min_edge_id.clone(),
            alpha_max_edge_id.clone(),
            alpha_clamp_edge_id.clone(),
            material_edge_id.clone(),
        ];
        if !has_alpha_connection {
            internal_connection_ids.push(alpha_default_edge_id.clone());
        }
        if blur_enabled {
            internal_connection_ids.push(render_edge_id.clone());
        }
        for internal_id in internal_connection_ids {
            if !connection_ids.insert(internal_id.clone()) {
                bail!(
                    "ImagePass '{}' cannot reserve internal connection id '{}'",
                    image_pass.id,
                    internal_id
                );
            }
        }

        let mut geometry_params = selected_params(&image_pass.params, GEOMETRY_PARAM_KEYS);
        if let Some(corner_radius) = image_pass.params.get("cornerRadius").cloned() {
            geometry_params.insert("radius".to_string(), corner_radius);
        }
        let geometry_bindings = image_pass
            .input_bindings
            .iter()
            .filter(|binding| binding.port_id == "size" || binding.port_id == "position")
            .cloned()
            .collect();
        let sample_bindings = image_pass
            .input_bindings
            .iter()
            .filter(|binding| binding.port_id == "image")
            .cloned()
            .collect();
        let alpha_bindings = image_pass
            .input_bindings
            .iter()
            .filter(|binding| binding.port_id == "alpha")
            .cloned()
            .map(|mut binding| {
                binding.port_id = "value".to_string();
                binding
            })
            .collect();
        let blur_bindings = image_pass
            .input_bindings
            .iter()
            .filter(|binding| binding.port_id == "blurRadius")
            .cloned()
            .collect();
        let camera_bindings = image_pass
            .input_bindings
            .iter()
            .filter(|binding| binding.port_id == "camera")
            .cloned()
            .collect::<Vec<_>>();

        let Some(public_node) = scene.nodes.iter_mut().find(|node| node.id == image_pass.id) else {
            bail!("missing ImagePass '{}' during expansion", image_pass.id);
        };
        public_node.inputs.clear();
        public_node.outputs.clear();
        if blur_enabled {
            public_node.params = gaussian_image_pass_params(image_pass);
            public_node.input_bindings = blur_bindings;
            public_node.wgsl_override = None;
        } else {
            public_node.node_type = "RenderPass".to_string();
            public_node.input_bindings = camera_bindings.clone();
            if infer_texture_size {
                public_node.params.insert(
                    IMAGE_PASS_TEXTURE_SIZE_SOURCE_PARAM.to_string(),
                    serde_json::json!(sample_id),
                );
            }
        }

        let render_node_id = if blur_enabled {
            let mut render_params = image_pass.params.clone();
            if infer_texture_size {
                render_params.insert(
                    IMAGE_PASS_TEXTURE_SIZE_SOURCE_PARAM.to_string(),
                    serde_json::json!(sample_id),
                );
            }
            scene.nodes.push(Node {
                id: render_id.clone(),
                node_type: "RenderPass".to_string(),
                params: render_params,
                inputs: Vec::new(),
                outputs: Vec::new(),
                input_bindings: camera_bindings,
                wgsl_override: image_pass.wgsl_override.clone(),
            });
            render_id.clone()
        } else {
            image_pass.id.clone()
        };

        scene.nodes.push(Node {
            id: geometry_id.clone(),
            node_type: "Rect2DGeometry".to_string(),
            params: geometry_params,
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_bindings: geometry_bindings,
            wgsl_override: None,
        });
        scene.nodes.push(Node {
            id: sample_id.clone(),
            node_type: "MathClosure".to_string(),
            params: HashMap::from([(
                "source".to_string(),
                serde_json::json!("output = samplePass(image, uv);"),
            )]),
            inputs: vec![NodePort {
                id: "image".to_string(),
                name: Some("image".to_string()),
                port_type: Some("texture".to_string()),
                array_length: None,
            }],
            outputs: vec![NodePort {
                id: "output".to_string(),
                name: Some("Output".to_string()),
                port_type: Some("color".to_string()),
                array_length: None,
            }],
            input_bindings: sample_bindings,
            wgsl_override: None,
        });
        if !has_alpha_connection {
            scene.nodes.push(float_input_node(
                alpha_default_id.clone(),
                image_pass
                    .params
                    .get("alpha")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!(1.0)),
            ));
        }
        scene.nodes.push(float_input_node(
            alpha_min_id.clone(),
            serde_json::json!(0.0),
        ));
        scene.nodes.push(float_input_node(
            alpha_max_id.clone(),
            serde_json::json!(1.0),
        ));
        scene.nodes.push(Node {
            id: alpha_clamp_id.clone(),
            node_type: "MathClamp".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            outputs: vec![typed_port("result", "float")],
            input_bindings: alpha_bindings,
            wgsl_override: None,
        });
        scene.nodes.push(Node {
            id: alpha_multiply_id.clone(),
            node_type: "MathMultiply".to_string(),
            params: HashMap::new(),
            inputs: vec![typed_port("color", "color"), typed_port("alpha", "float")],
            outputs: vec![typed_port("result", "color")],
            input_bindings: Vec::new(),
            wgsl_override: None,
        });

        for connection in &mut scene.connections {
            if connection.to.node_id == image_pass.id {
                match connection.to.port_id.as_str() {
                    "image" => connection.to.node_id.clone_from(&sample_id),
                    "size" | "position" => connection.to.node_id.clone_from(&geometry_id),
                    "alpha" => {
                        connection.to.node_id.clone_from(&alpha_clamp_id);
                        connection.to.port_id = "value".to_string();
                    }
                    "camera" if blur_enabled => {
                        connection.to.node_id.clone_from(&render_id);
                    }
                    _ => {}
                }
            }
            if blur_enabled
                && connection.from.node_id == image_pass.id
                && connection.from.port_id == "depth"
            {
                connection.from.node_id.clone_from(&render_id);
            }
        }

        scene.connections.push(Connection {
            id: geometry_edge_id,
            from: Endpoint {
                node_id: geometry_id,
                port_id: "geometry".to_string(),
            },
            to: Endpoint {
                node_id: render_node_id.clone(),
                port_id: "geometry".to_string(),
            },
        });
        scene.connections.push(Connection {
            id: color_edge_id,
            from: Endpoint {
                node_id: sample_id,
                port_id: "output".to_string(),
            },
            to: Endpoint {
                node_id: alpha_multiply_id.clone(),
                port_id: "color".to_string(),
            },
        });
        if !has_alpha_connection {
            scene.connections.push(Connection {
                id: alpha_default_edge_id,
                from: Endpoint {
                    node_id: alpha_default_id,
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: alpha_clamp_id.clone(),
                    port_id: "value".to_string(),
                },
            });
        }
        scene.connections.push(Connection {
            id: alpha_min_edge_id,
            from: Endpoint {
                node_id: alpha_min_id,
                port_id: "value".to_string(),
            },
            to: Endpoint {
                node_id: alpha_clamp_id.clone(),
                port_id: "min".to_string(),
            },
        });
        scene.connections.push(Connection {
            id: alpha_max_edge_id,
            from: Endpoint {
                node_id: alpha_max_id,
                port_id: "value".to_string(),
            },
            to: Endpoint {
                node_id: alpha_clamp_id.clone(),
                port_id: "max".to_string(),
            },
        });
        scene.connections.push(Connection {
            id: alpha_clamp_edge_id,
            from: Endpoint {
                node_id: alpha_clamp_id,
                port_id: "result".to_string(),
            },
            to: Endpoint {
                node_id: alpha_multiply_id.clone(),
                port_id: "alpha".to_string(),
            },
        });
        scene.connections.push(Connection {
            id: material_edge_id,
            from: Endpoint {
                node_id: alpha_multiply_id,
                port_id: "result".to_string(),
            },
            to: Endpoint {
                node_id: render_node_id.clone(),
                port_id: "material".to_string(),
            },
        });
        if blur_enabled {
            scene.connections.push(Connection {
                id: render_edge_id,
                from: Endpoint {
                    node_id: render_id,
                    port_id: if local_texture_invocation {
                        "texture".to_string()
                    } else {
                        "pass".to_string()
                    },
                },
                to: Endpoint {
                    node_id: image_pass.id.clone(),
                    port_id: if local_texture_invocation {
                        "texture".to_string()
                    } else {
                        "pass".to_string()
                    },
                },
            });
        }
    }

    Ok(image_passes.len())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::dsl::{Connection, Endpoint, Metadata, Node, SceneDSL};

    use super::{IMAGE_PASS_TEXTURE_SIZE_SOURCE_PARAM, expand_image_passes};

    fn node(id: &str, node_type: &str, params: serde_json::Value) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params: serde_json::from_value(params).expect("object params"),
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_bindings: Vec::new(),
            wgsl_override: None,
        }
    }

    fn edge(
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

    #[test]
    fn lowers_image_pass_to_canonical_nodes_and_preserves_public_outputs() {
        let mut scene = SceneDSL {
            version: "6.0".to_string(),
            metadata: Metadata {
                name: "image-pass".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("image", "ImageFile", json!({ "assetId": "hero" })),
                node(
                    "image-texture",
                    "ImageTexture",
                    json!({
                        "addressModeU": "clamp-to-edge",
                        "addressModeV": "clamp-to-edge"
                    }),
                ),
                node("size", "Vector2Input", json!({ "x": 320.0, "y": 180.0 })),
                node(
                    "position",
                    "Vector2Input",
                    json!({ "x": 640.0, "y": 360.0 }),
                ),
                node(
                    "hero",
                    "ImagePass",
                    json!({
                        "cornerRadius": 24.0,
                        "blurRadius": 0,
                        "smooth": 0.75,
                        "depthTest": true,
                        "msaaSampleCount": 4
                    }),
                ),
                node("composite", "Composite", json!({})),
            ],
            connections: vec![
                edge("image-edge", "image", "image", "image-texture", "image"),
                edge("texture-edge", "image-texture", "texture", "hero", "image"),
                edge("size-edge", "size", "vector", "hero", "size"),
                edge("position-edge", "position", "vector", "hero", "position"),
                edge("pass-edge", "hero", "pass", "composite", "pass"),
            ],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };

        assert_eq!(expand_image_passes(&mut scene).expect("expand"), 1);
        assert_eq!(expand_image_passes(&mut scene).expect("idempotent"), 0);

        let render_pass = scene
            .nodes
            .iter()
            .find(|node| node.id == "hero")
            .expect("public pass node");
        assert_eq!(render_pass.node_type, "RenderPass");
        assert_eq!(render_pass.params.get("depthTest"), Some(&json!(true)));
        assert!(
            scene
                .nodes
                .iter()
                .all(|node| node.id != "sys.image-pass.hero.render")
        );

        let geometry = scene
            .nodes
            .iter()
            .find(|node| node.id == "sys.image-pass.hero.geometry")
            .expect("private geometry");
        assert_eq!(geometry.node_type, "Rect2DGeometry");
        assert_eq!(geometry.params.get("radius"), Some(&json!(24.0)));
        assert_eq!(geometry.params.get("smooth"), Some(&json!(0.75)));
        assert!(!geometry.params.contains_key("depthTest"));

        let sample = scene
            .nodes
            .iter()
            .find(|node| node.id == "sys.image-pass.hero.sample")
            .expect("private texture sample");
        assert_eq!(sample.node_type, "MathClosure");
        assert_eq!(sample.inputs[0].port_type.as_deref(), Some("texture"));
        assert_eq!(sample.outputs[0].port_type.as_deref(), Some("color"));

        let alpha_default = scene
            .nodes
            .iter()
            .find(|node| node.id == "sys.image-pass.hero.alpha.default")
            .expect("private default alpha");
        assert_eq!(alpha_default.node_type, "FloatInput");
        assert_eq!(alpha_default.params.get("value"), Some(&json!(1.0)));

        let alpha_clamp = scene
            .nodes
            .iter()
            .find(|node| node.id == "sys.image-pass.hero.alpha.clamp")
            .expect("private alpha clamp");
        assert_eq!(alpha_clamp.node_type, "MathClamp");
        assert_eq!(alpha_clamp.outputs[0].port_type.as_deref(), Some("float"));

        let alpha_multiply = scene
            .nodes
            .iter()
            .find(|node| node.id == "sys.image-pass.hero.alpha.multiply")
            .expect("private alpha multiply");
        assert_eq!(alpha_multiply.node_type, "MathMultiply");
        assert_eq!(
            alpha_multiply
                .inputs
                .iter()
                .map(|port| (port.id.as_str(), port.port_type.as_deref()))
                .collect::<Vec<_>>(),
            vec![("color", Some("color")), ("alpha", Some("float"))]
        );
        assert_eq!(
            alpha_multiply.outputs[0].port_type.as_deref(),
            Some("color")
        );

        assert!(scene.connections.iter().any(|connection| {
            connection.id == "texture-edge"
                && connection.to.node_id == "sys.image-pass.hero.sample"
                && connection.to.port_id == "image"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.id == "size-edge"
                && connection.to.node_id == "sys.image-pass.hero.geometry"
                && connection.to.port_id == "size"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.id == "pass-edge"
                && connection.from.node_id == "hero"
                && connection.from.port_id == "pass"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.from.node_id == "sys.image-pass.hero.geometry"
                && connection.from.port_id == "geometry"
                && connection.to.node_id == "hero"
                && connection.to.port_id == "geometry"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.from.node_id == "sys.image-pass.hero.sample"
                && connection.from.port_id == "output"
                && connection.to.node_id == "sys.image-pass.hero.alpha.multiply"
                && connection.to.port_id == "color"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.from.node_id == "sys.image-pass.hero.alpha.default"
                && connection.from.port_id == "value"
                && connection.to.node_id == "sys.image-pass.hero.alpha.clamp"
                && connection.to.port_id == "value"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.from.node_id == "sys.image-pass.hero.alpha.clamp"
                && connection.from.port_id == "result"
                && connection.to.node_id == "sys.image-pass.hero.alpha.multiply"
                && connection.to.port_id == "alpha"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.from.node_id == "sys.image-pass.hero.alpha.multiply"
                && connection.from.port_id == "result"
                && connection.to.node_id == "hero"
                && connection.to.port_id == "material"
        }));
    }

    #[test]
    fn blur_path_preserves_public_blur_identity_and_routes_source_only_ports() {
        let mut scene = SceneDSL {
            version: "6.0".to_string(),
            metadata: Metadata {
                name: "image-pass-blur".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("blur", "IntInput", json!({ "value": 0 })),
                node("camera", "OrthographicCamera", json!({})),
                node(
                    "hero",
                    "ImagePass",
                    json!({
                        "cornerRadius": 24.0,
                        "smooth": 0.75,
                        "blurRadius": 0,
                        "depthTest": true
                    }),
                ),
                node("composite", "Composite", json!({})),
                node("depth-consumer", "Composite", json!({})),
            ],
            connections: vec![
                edge("blur-edge", "blur", "value", "hero", "blurRadius"),
                edge("camera-edge", "camera", "camera", "hero", "camera"),
                edge("pass-edge", "hero", "pass", "composite", "pass"),
                edge("depth-edge", "hero", "depth", "depth-consumer", "pass"),
            ],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };

        assert_eq!(expand_image_passes(&mut scene).expect("expand"), 1);
        assert_eq!(expand_image_passes(&mut scene).expect("idempotent"), 0);

        let public_blur = scene
            .nodes
            .iter()
            .find(|node| node.id == "hero")
            .expect("public ImagePass blur node");
        assert_eq!(public_blur.node_type, "ImagePass");
        assert_eq!(public_blur.params.get("blurRadius"), Some(&json!(0)));
        assert_eq!(public_blur.params.get("extend"), Some(&json!(false)));

        let source_render = scene
            .nodes
            .iter()
            .find(|node| node.id == "sys.image-pass.hero.render")
            .expect("private source RenderPass");
        assert_eq!(source_render.node_type, "RenderPass");
        assert_eq!(source_render.params.get("depthTest"), Some(&json!(true)));

        let geometry = scene
            .nodes
            .iter()
            .find(|node| node.id == "sys.image-pass.hero.geometry")
            .expect("private geometry");
        assert_eq!(geometry.params.get("radius"), Some(&json!(24.0)));
        assert!(!geometry.params.contains_key("cornerRadius"));

        assert!(scene.connections.iter().any(|connection| {
            connection.id == "blur-edge"
                && connection.to.node_id == "hero"
                && connection.to.port_id == "blurRadius"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.id == "camera-edge"
                && connection.to.node_id == "sys.image-pass.hero.render"
                && connection.to.port_id == "camera"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.id == "pass-edge"
                && connection.from.node_id == "hero"
                && connection.from.port_id == "pass"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.id == "depth-edge"
                && connection.from.node_id == "sys.image-pass.hero.render"
                && connection.from.port_id == "depth"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.from.node_id == "sys.image-pass.hero.render"
                && connection.from.port_id == "pass"
                && connection.to.node_id == "hero"
                && connection.to.port_id == "pass"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.from.node_id == "sys.image-pass.hero.geometry"
                && connection.to.node_id == "sys.image-pass.hero.render"
                && connection.to.port_id == "geometry"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.from.node_id == "sys.image-pass.hero.alpha.multiply"
                && connection.to.node_id == "sys.image-pass.hero.render"
                && connection.to.port_id == "material"
        }));
    }

    #[test]
    fn blur_texture_route_materializes_its_private_draw_in_geosize_domain() {
        let mut scene = SceneDSL {
            version: "6.0".to_string(),
            metadata: Metadata {
                name: "image-pass-texture".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node(
                    "hero",
                    "ImagePass",
                    json!({ "size": [320.0, 180.0], "blurRadius": 10 }),
                ),
                node("sampler", "TextureSampler", json!({})),
            ],
            connections: vec![edge(
                "texture-edge",
                "hero",
                "texture",
                "sampler",
                "texture",
            )],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };

        assert_eq!(expand_image_passes(&mut scene).expect("expand"), 1);
        assert!(scene.connections.iter().any(|connection| {
            connection.id == "texture-edge"
                && connection.from.node_id == "hero"
                && connection.from.port_id == "texture"
        }));
        assert!(scene.connections.iter().any(|connection| {
            connection.from.node_id == "sys.image-pass.hero.render"
                && connection.from.port_id == "texture"
                && connection.to.node_id == "hero"
                && connection.to.port_id == "texture"
        }));
    }

    #[test]
    fn texture_route_without_authored_size_inherits_the_image_texture_extent() {
        let mut scene = SceneDSL {
            version: "6.0".to_string(),
            metadata: Metadata {
                name: "image-pass-inferred-size".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("source", "ImageTexture", json!({ "assetId": "hero" })),
                node("hero", "ImagePass", json!({ "blurRadius": 0 })),
                node("sampler", "TextureSampler", json!({})),
            ],
            connections: vec![
                edge("image-edge", "source", "texture", "hero", "image"),
                edge("texture-edge", "hero", "texture", "sampler", "texture"),
            ],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };

        assert_eq!(expand_image_passes(&mut scene).expect("expand"), 1);
        let render_pass = scene
            .nodes
            .iter()
            .find(|node| node.id == "hero")
            .expect("lowered RenderPass");
        assert_eq!(
            render_pass.params.get(IMAGE_PASS_TEXTURE_SIZE_SOURCE_PARAM),
            Some(&json!("sys.image-pass.hero.sample"))
        );
    }

    #[test]
    fn texture_route_with_authored_size_does_not_inherit_the_image_texture_extent() {
        let mut scene = SceneDSL {
            version: "6.0".to_string(),
            metadata: Metadata {
                name: "image-pass-explicit-size".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("source", "ImageTexture", json!({ "assetId": "hero" })),
                node(
                    "hero",
                    "ImagePass",
                    json!({ "size": [320.0, 180.0], "blurRadius": 0 }),
                ),
                node("sampler", "TextureSampler", json!({})),
            ],
            connections: vec![
                edge("image-edge", "source", "texture", "hero", "image"),
                edge("texture-edge", "hero", "texture", "sampler", "texture"),
            ],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };

        assert_eq!(expand_image_passes(&mut scene).expect("expand"), 1);
        let render_pass = scene
            .nodes
            .iter()
            .find(|node| node.id == "hero")
            .expect("lowered RenderPass");
        assert!(
            !render_pass
                .params
                .contains_key(IMAGE_PASS_TEXTURE_SIZE_SOURCE_PARAM)
        );
    }

    #[test]
    fn animation_target_keeps_zero_radius_on_the_gaussian_path() {
        let mut scene = SceneDSL {
            version: "6.0".to_string(),
            metadata: Metadata {
                name: "image-pass-animated-blur".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![node(
                "hero",
                "ImagePass",
                json!({ "cornerRadius": 0.0, "blurRadius": 0 }),
            )],
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: Some(
                serde_json::from_value(json!({
                    "id": "sm",
                    "name": "animated blur",
                    "stateParams": [{
                        "id": "hero:blurRadius",
                        "name": "Blur Radius",
                        "type": "int",
                        "defaultValue": 0
                    }],
                    "stateParamGraph": {
                        "rootNodePosition": { "x": 0.0, "y": 0.0 },
                        "declarationPositions": {}
                    },
                    "states": [],
                    "transitions": [],
                    "derivations": [],
                    "motionGraphs": []
                }))
                .expect("state machine"),
            ),
            debug_artifacts: None,
        };

        assert_eq!(expand_image_passes(&mut scene).expect("expand"), 1);
        assert_eq!(
            scene
                .nodes
                .iter()
                .find(|node| node.id == "hero")
                .map(|node| node.node_type.as_str()),
            Some("ImagePass")
        );
        assert!(
            scene
                .nodes
                .iter()
                .any(|node| node.id == "sys.image-pass.hero.render")
        );
    }

    #[test]
    fn connected_alpha_replaces_the_private_default_source() {
        let mut scene = SceneDSL {
            version: "6.0".to_string(),
            metadata: Metadata {
                name: "image-pass-connected-alpha".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("alpha", "FloatInput", json!({ "value": 1.5 })),
                node("hero", "ImagePass", json!({ "alpha": 0.25 })),
            ],
            connections: vec![edge("alpha-edge", "alpha", "value", "hero", "alpha")],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };

        assert_eq!(expand_image_passes(&mut scene).expect("expand"), 1);

        assert!(
            scene
                .nodes
                .iter()
                .all(|node| node.id != "sys.image-pass.hero.alpha.default")
        );
        let value_inputs = scene
            .connections
            .iter()
            .filter(|connection| {
                connection.to.node_id == "sys.image-pass.hero.alpha.clamp"
                    && connection.to.port_id == "value"
            })
            .collect::<Vec<_>>();
        assert_eq!(value_inputs.len(), 1);
        assert_eq!(value_inputs[0].id, "alpha-edge");
        assert_eq!(value_inputs[0].from.node_id, "alpha");
        assert_eq!(value_inputs[0].from.port_id, "value");
    }

    #[test]
    fn image_pass_builds_image_sampling_dynamic_geometry_and_corner_coverage_wgsl() {
        let scene: SceneDSL = serde_json::from_value(json!({
            "version": "6.0",
            "metadata": { "name": "image-pass-wgsl" },
            "nodes": [
                {
                    "id": "target",
                    "type": "RenderTexture",
                    "params": { "width": 1280, "height": 720, "format": "rgba8unorm" }
                },
                {
                    "id": "size",
                    "type": "Vector2Input",
                    "params": { "x": 320.0, "y": 180.0 }
                },
                {
                    "id": "position",
                    "type": "Vector2Input",
                    "params": { "x": 640.0, "y": 360.0 }
                },
                {
                    "id": "alpha",
                    "type": "FloatInput",
                    "params": { "value": -0.5 }
                },
                {
                    "id": "source",
                    "type": "ImageTexture",
                    "params": {
                        "assetId": "",
                        "addressModeU": "clamp-to-edge",
                        "addressModeV": "clamp-to-edge",
                        "magFilter": "linear",
                        "minFilter": "linear",
                        "mipmapFilter": "linear",
                        "encoderSpace": "srgb",
                        "alphaMode": "straight"
                    }
                },
                {
                    "id": "hero",
                    "type": "ImagePass",
                    "params": {
                        "cornerRadius": 24.0,
                        "blurRadius": 0,
                        "smooth": 0.75,
                        "msaaSampleCount": 1,
                        "culling": "none",
                        "depthTest": false,
                        "loadOp": "clear",
                        "clearColor": [0.0, 0.0, 0.0, 0.0],
                        "blend_preset": "premul_alpha",
                        "blendfunc": "add",
                        "src_factor": "one",
                        "dst_factor": "one-minus-src-alpha",
                        "src_alpha_factor": "one",
                        "dst_alpha_factor": "one-minus-src-alpha"
                    }
                },
                { "id": "composite", "type": "Composite" },
                {
                    "id": "screen",
                    "type": "Screen",
                    "params": { "width": 1280, "height": 720 }
                }
            ],
            "connections": [
                {
                    "id": "image-edge",
                    "from": { "nodeId": "source", "portId": "texture" },
                    "to": { "nodeId": "hero", "portId": "image" }
                },
                {
                    "id": "size-edge",
                    "from": { "nodeId": "size", "portId": "vector" },
                    "to": { "nodeId": "hero", "portId": "size" }
                },
                {
                    "id": "position-edge",
                    "from": { "nodeId": "position", "portId": "vector" },
                    "to": { "nodeId": "hero", "portId": "position" }
                },
                {
                    "id": "alpha-edge",
                    "from": { "nodeId": "alpha", "portId": "value" },
                    "to": { "nodeId": "hero", "portId": "alpha" }
                },
                {
                    "id": "hero-layer",
                    "from": { "nodeId": "hero", "portId": "pass" },
                    "to": { "nodeId": "composite", "portId": "pass" }
                },
                {
                    "id": "target-edge",
                    "from": { "nodeId": "target", "portId": "texture" },
                    "to": { "nodeId": "composite", "portId": "target" }
                },
                {
                    "id": "screen-edge",
                    "from": { "nodeId": "composite", "portId": "pass" },
                    "to": { "nodeId": "screen", "portId": "pass" }
                }
            ],
            "outputs": null,
            "groups": [],
            "assets": {}
        }))
        .expect("scene should deserialize");

        let prepared = crate::renderer::scene_prep::prepare_scene(&scene).expect("prepare scene");
        assert!(
            prepared
                .scene
                .nodes
                .iter()
                .all(|node| node.node_type != "ImagePass")
        );

        let bundles = crate::renderer::wgsl::build_all_pass_wgsl_bundles_from_scene(&scene)
            .expect("build pass WGSL");
        assert!(
            bundles
                .iter()
                .all(|(pass_id, _)| !pass_id.starts_with("sys.blur.hero."))
        );
        let (_, bundle) = bundles
            .iter()
            .find(|(pass_id, _)| pass_id == "hero")
            .expect("ImagePass public pass bundle");

        assert!(
            bundle
                .module
                .contains("textureSample(img_tex_source, img_samp_source")
        );
        assert!(
            bundle
                .module
                .contains("Rect2DGeometry smooth corner coverage")
        );
        assert!(bundle.module.contains("clamp("));
        assert!(bundle.module.contains("* vec4f(clamp("));
        let graph_schema = bundle.graph_schema.as_ref().expect("graph schema");
        assert!(
            graph_schema
                .fields
                .iter()
                .any(|field| field.node_id == "size")
        );
        assert!(
            graph_schema
                .fields
                .iter()
                .any(|field| field.node_id == "position")
        );
        assert!(
            graph_schema
                .fields
                .iter()
                .any(|field| field.node_id == "alpha")
        );

        let mut blurred_scene = scene.clone();
        blurred_scene
            .nodes
            .iter_mut()
            .find(|node| node.id == "hero")
            .expect("ImagePass")
            .params
            .insert("blurRadius".to_string(), json!(10));
        let blurred_prepared = crate::renderer::scene_prep::prepare_scene(&blurred_scene)
            .expect("prepare blurred ImagePass");
        assert!(
            blurred_prepared
                .scene
                .nodes
                .iter()
                .any(|node| node.id == "hero" && node.node_type == "ImagePass")
        );
        assert!(blurred_prepared.scene.connections.iter().any(|connection| {
            connection.from.node_id == "sys.image-pass.hero.render"
                && connection.from.port_id == "pass"
                && connection.to.node_id == "hero"
                && connection.to.port_id == "pass"
        }));

        let blurred_bundles =
            crate::renderer::wgsl::build_all_pass_wgsl_bundles_from_scene(&blurred_scene)
                .expect("build blurred ImagePass WGSL");
        assert!(blurred_bundles.iter().any(|(pass_id, _)| {
            pass_id == "sys.blur.hero.h.ds1.pass" || pass_id.starts_with("sys.blur.hero.h.route1")
        }));
    }
}
