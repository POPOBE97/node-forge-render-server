use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use crate::dsl::{Connection, Endpoint, Node, NodePort, SceneDSL};

const GEOMETRY_PARAM_KEYS: &[&str] = &["size", "position", "radius", "smooth"];
const TEXTURE_PARAM_KEYS: &[&str] = &[
    "assetId",
    "interpolation",
    "extension",
    "addressModeU",
    "addressModeV",
    "magFilter",
    "minFilter",
    "mipmapFilter",
    "encoderSpace",
    "alphaMode",
    "aspectCorrection",
];

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

/// Lower the author-facing ImagePass macro node into the canonical render graph.
///
/// The RenderPass keeps the authored node id so all downstream pass/depth connections and resource
/// names remain stable. Its private Rect2DGeometry and ImageTexture nodes reuse the existing
/// geometry, smooth-corner, sampler, color-space, alpha, and pass implementations unchanged.
pub(crate) fn expand_image_passes(scene: &mut SceneDSL) -> Result<usize> {
    let image_passes: Vec<Node> = scene
        .nodes
        .iter()
        .filter(|node| node.node_type == "ImagePass")
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

    for image_pass in &image_passes {
        let has_alpha_connection = scene.connections.iter().any(|connection| {
            connection.to.node_id == image_pass.id && connection.to.port_id == "alpha"
        });
        let geometry_id = format!("sys.image-pass.{}.geometry", image_pass.id);
        let texture_id = format!("sys.image-pass.{}.texture", image_pass.id);
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

        let mut internal_node_ids = vec![
            geometry_id.clone(),
            texture_id.clone(),
            alpha_min_id.clone(),
            alpha_max_id.clone(),
            alpha_clamp_id.clone(),
            alpha_multiply_id.clone(),
        ];
        if !has_alpha_connection {
            internal_node_ids.push(alpha_default_id.clone());
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
        for internal_id in internal_connection_ids {
            if !connection_ids.insert(internal_id.clone()) {
                bail!(
                    "ImagePass '{}' cannot reserve internal connection id '{}'",
                    image_pass.id,
                    internal_id
                );
            }
        }

        let geometry_params = selected_params(&image_pass.params, GEOMETRY_PARAM_KEYS);
        let texture_params = selected_params(&image_pass.params, TEXTURE_PARAM_KEYS);
        let geometry_bindings = image_pass
            .input_bindings
            .iter()
            .filter(|binding| binding.port_id == "size" || binding.port_id == "position")
            .cloned()
            .collect();
        let texture_bindings = image_pass
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

        let Some(render_pass) = scene.nodes.iter_mut().find(|node| node.id == image_pass.id) else {
            bail!("missing ImagePass '{}' during expansion", image_pass.id);
        };
        render_pass.node_type = "RenderPass".to_string();
        render_pass.inputs.clear();
        render_pass.outputs.clear();
        render_pass.input_bindings = image_pass
            .input_bindings
            .iter()
            .filter(|binding| binding.port_id == "camera")
            .cloned()
            .collect();

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
            id: texture_id.clone(),
            node_type: "ImageTexture".to_string(),
            params: texture_params,
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_bindings: texture_bindings,
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
            if connection.to.node_id != image_pass.id {
                continue;
            }
            match connection.to.port_id.as_str() {
                "image" => connection.to.node_id.clone_from(&texture_id),
                "size" | "position" => connection.to.node_id.clone_from(&geometry_id),
                "alpha" => {
                    connection.to.node_id.clone_from(&alpha_clamp_id);
                    connection.to.port_id = "value".to_string();
                }
                _ => {}
            }
        }

        scene.connections.push(Connection {
            id: geometry_edge_id,
            from: Endpoint {
                node_id: geometry_id,
                port_id: "geometry".to_string(),
            },
            to: Endpoint {
                node_id: image_pass.id.clone(),
                port_id: "geometry".to_string(),
            },
        });
        scene.connections.push(Connection {
            id: color_edge_id,
            from: Endpoint {
                node_id: texture_id,
                port_id: "color".to_string(),
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
                node_id: image_pass.id.clone(),
                port_id: "material".to_string(),
            },
        });
    }

    Ok(image_passes.len())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::dsl::{Connection, Endpoint, Metadata, Node, SceneDSL};

    use super::expand_image_passes;

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
                        "radius": 24.0,
                        "smooth": 0.75,
                        "assetId": "hero",
                        "addressModeU": "clamp-to-edge",
                        "addressModeV": "clamp-to-edge",
                        "depthTest": true,
                        "msaaSampleCount": 4
                    }),
                ),
                node("composite", "Composite", json!({})),
            ],
            connections: vec![
                edge("image-edge", "image", "image", "hero", "image"),
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

        let geometry = scene
            .nodes
            .iter()
            .find(|node| node.id == "sys.image-pass.hero.geometry")
            .expect("private geometry");
        assert_eq!(geometry.node_type, "Rect2DGeometry");
        assert_eq!(geometry.params.get("radius"), Some(&json!(24.0)));
        assert_eq!(geometry.params.get("smooth"), Some(&json!(0.75)));
        assert!(!geometry.params.contains_key("depthTest"));

        let texture = scene
            .nodes
            .iter()
            .find(|node| node.id == "sys.image-pass.hero.texture")
            .expect("private texture");
        assert_eq!(texture.node_type, "ImageTexture");
        assert_eq!(
            texture.params.get("addressModeU"),
            Some(&json!("clamp-to-edge"))
        );
        assert!(!texture.params.contains_key("radius"));

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
            connection.id == "image-edge"
                && connection.to.node_id == "sys.image-pass.hero.texture"
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
            connection.from.node_id == "sys.image-pass.hero.texture"
                && connection.from.port_id == "color"
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
                    "id": "hero",
                    "type": "ImagePass",
                    "params": {
                        "radius": 24.0,
                        "smooth": 0.75,
                        "assetId": "",
                        "addressModeU": "clamp-to-edge",
                        "addressModeV": "clamp-to-edge",
                        "magFilter": "linear",
                        "minFilter": "linear",
                        "mipmapFilter": "linear",
                        "encoderSpace": "srgb",
                        "alphaMode": "straight",
                        "aspectCorrection": "fill",
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
        let (_, bundle) = bundles
            .iter()
            .find(|(pass_id, _)| pass_id == "hero")
            .expect("ImagePass public pass bundle");

        assert!(
            bundle
                .module
                .contains("ImageTexture sys.image-pass.hero.texture.color")
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
    }
}
