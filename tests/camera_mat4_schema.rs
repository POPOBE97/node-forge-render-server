use std::collections::HashMap;

use node_forge_render_server::{
    dsl::{Connection, Endpoint, Metadata, Node, SceneDSL},
    schema::{PortTypeSpec, load_default_scheme, validate_scene_against},
};
use serde_json::json;

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
        input_bindings: Vec::new(),
        outputs: Vec::new(),
    }
}

fn scene(nodes: Vec<Node>, connections: Vec<Connection>) -> SceneDSL {
    SceneDSL {
        version: "1.0".to_string(),
        metadata: Metadata {
            name: "camera-mat4-schema".to_string(),
            created: None,
            modified: None,
        },
        nodes,
        connections,
        outputs: Some(HashMap::new()),
        groups: Vec::new(),
        assets: Default::default(),
    }
}

#[test]
fn scheme_exposes_mat4_port_type_contract() {
    let scheme = load_default_scheme().expect("load default scheme");

    let mat4_compat = scheme
        .port_type_compatibility
        .get("mat4")
        .expect("mat4 compatibility entry");
    assert_eq!(mat4_compat, &vec!["mat4".to_string()]);

    let any_compat = scheme
        .port_type_compatibility
        .get("any")
        .expect("any compatibility entry");
    assert!(any_compat.iter().any(|t| t == "mat4"));
}

#[test]
fn scheme_exposes_camera_nodes_and_camera_inputs() {
    let scheme = load_default_scheme().expect("load default scheme");

    for node_type in ["PerspectiveCamera", "OrthographicCamera"] {
        let node_scheme = scheme
            .nodes
            .get(node_type)
            .unwrap_or_else(|| panic!("missing {node_type} in scheme"));
        match node_scheme.outputs.get("camera") {
            Some(PortTypeSpec::One(port_type)) => assert_eq!(port_type, "mat4"),
            other => panic!("{node_type}.outputs.camera must be mat4, got {other:?}"),
        }
    }

    for node_type in [
        "RenderPass",
        "GuassianBlurPass",
        "GradientBlur",
        "Downsample",
        "Upsample",
        "Composite",
    ] {
        let node_scheme = scheme
            .nodes
            .get(node_type)
            .unwrap_or_else(|| panic!("missing {node_type} in scheme"));
        match node_scheme.inputs.get("camera") {
            Some(PortTypeSpec::One(port_type)) => assert_eq!(port_type, "mat4"),
            other => panic!("{node_type}.inputs.camera must be mat4, got {other:?}"),
        }
    }

    for node_type in ["SetTransform", "TransformGeometry"] {
        let node_scheme = scheme
            .nodes
            .get(node_type)
            .unwrap_or_else(|| panic!("missing {node_type} in scheme"));
        match node_scheme.inputs.get("matrix") {
            Some(PortTypeSpec::One(port_type)) => assert_eq!(port_type, "mat4"),
            other => panic!("{node_type}.inputs.matrix must be mat4, got {other:?}"),
        }
    }
}

#[test]
fn validation_rejects_invalid_inline_camera_mat4() {
    let scheme = load_default_scheme().expect("load default scheme");
    let scene = scene(
        vec![node("rp", "RenderPass", json!({"camera": [1.0, 0.0]}))],
        vec![],
    );

    let err = validate_scene_against(&scene, &scheme).expect_err("camera mat4 should fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("rp.camera"));
    assert!(msg.contains("length 16"));
}

#[test]
fn validation_rejects_invalid_camera_node_params() {
    let scheme = load_default_scheme().expect("load default scheme");
    let scene = scene(
        vec![node(
            "cam",
            "PerspectiveCamera",
            json!({"near": 0.0, "far": 100.0}),
        )],
        vec![],
    );

    let err = validate_scene_against(&scene, &scheme).expect_err("invalid near/far should fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("cam.near"));
}

#[test]
fn validation_rejects_non_mat4_connection_to_camera_input() {
    let scheme = load_default_scheme().expect("load default scheme");
    let scene = scene(
        vec![
            node("f", "FloatInput", json!({"value": 1.0})),
            node("rp", "RenderPass", json!({})),
        ],
        vec![Connection {
            id: "c1".to_string(),
            from: Endpoint {
                node_id: "f".to_string(),
                port_id: "value".to_string(),
            },
            to: Endpoint {
                node_id: "rp".to_string(),
                port_id: "camera".to_string(),
            },
        }],
    );

    let err = validate_scene_against(&scene, &scheme).expect_err("non-mat4 should fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("type mismatch"));
    assert!(msg.contains("rp.camera"));
}
