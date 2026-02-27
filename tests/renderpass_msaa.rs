use std::collections::HashMap;

use node_forge_render_server::{
    dsl::{Connection, Endpoint, Metadata, Node, SceneDSL, normalize_scene_defaults},
    schema::{load_default_scheme, validate_scene_against},
};
use serde_json::json;

fn make_render_pass_scene(msaa_sample_count: serde_json::Value) -> SceneDSL {
    SceneDSL {
        version: "1".to_string(),
        metadata: Metadata {
            name: "renderpass-msaa".to_string(),
            created: None,
            modified: None,
        },
        nodes: vec![Node {
            id: "rp".to_string(),
            node_type: "RenderPass".to_string(),
            params: HashMap::from([("msaaSampleCount".to_string(), msaa_sample_count)]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        }],
        connections: Vec::new(),
        outputs: None,
        groups: Vec::new(),
        assets: Default::default(),
    }
}

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
        version: "1".to_string(),
        metadata: Metadata {
            name: "renderpass-schema".to_string(),
            created: None,
            modified: None,
        },
        nodes,
        connections,
        outputs: None,
        groups: Vec::new(),
        assets: Default::default(),
    }
}

#[test]
fn render_pass_scheme_exposes_msaa_input_and_default() {
    let scheme = load_default_scheme().expect("load default scheme");
    let render_pass = scheme
        .nodes
        .get("RenderPass")
        .expect("RenderPass node in scheme");

    assert!(
        render_pass.inputs.contains_key("msaaSampleCount"),
        "RenderPass.inputs must contain msaaSampleCount"
    );
    assert_eq!(
        render_pass.default_params.get("msaaSampleCount"),
        Some(&json!(1)),
        "RenderPass.defaultParams.msaaSampleCount must be 1"
    );
}

#[test]
fn render_pass_msaa_default_is_applied_by_normalization() {
    let mut scene = SceneDSL {
        version: "1".to_string(),
        metadata: Metadata {
            name: "renderpass-msaa-default".to_string(),
            created: None,
            modified: None,
        },
        nodes: vec![Node {
            id: "rp".to_string(),
            node_type: "RenderPass".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        }],
        connections: Vec::new(),
        outputs: None,
        groups: Vec::new(),
        assets: Default::default(),
    };

    normalize_scene_defaults(&mut scene).expect("normalize scene defaults");

    assert_eq!(
        scene.nodes[0].params.get("msaaSampleCount"),
        Some(&json!(1)),
        "RenderPass.msaaSampleCount default must be merged from schema"
    );
}

#[test]
fn render_pass_msaa_validation_accepts_allowed_values() {
    let scheme = load_default_scheme().expect("load default scheme");
    for value in [1, 2, 4, 8] {
        let scene = make_render_pass_scene(json!(value));
        validate_scene_against(&scene, &scheme)
            .unwrap_or_else(|e| panic!("value {value} should validate, got error: {e:#}"));
    }
}

#[test]
fn render_pass_msaa_validation_rejects_invalid_values() {
    let scheme = load_default_scheme().expect("load default scheme");
    let scene = make_render_pass_scene(json!(0));
    let err = validate_scene_against(&scene, &scheme).expect_err("value 0 should fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("RenderPass.msaaSampleCount"));
    assert!(msg.contains("must be one of 1,2,4,8"));
}

#[test]
fn render_pass_culling_validation_accepts_allowed_values() {
    let scheme = load_default_scheme().expect("load default scheme");
    for value in ["none", "front", "back"] {
        let scene = scene(vec![node("rp", "RenderPass", json!({"culling": value}))], vec![]);
        validate_scene_against(&scene, &scheme)
            .unwrap_or_else(|e| panic!("culling={value} should validate, got error: {e:#}"));
    }
}

#[test]
fn render_pass_culling_validation_rejects_invalid_values() {
    let scheme = load_default_scheme().expect("load default scheme");
    let scene = scene(
        vec![node("rp", "RenderPass", json!({"culling": "invalid"}))],
        vec![],
    );
    let err = validate_scene_against(&scene, &scheme).expect_err("invalid culling should fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("RenderPass.culling"));
    assert!(msg.contains("none,front,back"));
}

#[test]
fn render_pass_depth_test_validation_rejects_non_boolean() {
    let scheme = load_default_scheme().expect("load default scheme");
    let scene = scene(
        vec![node("rp", "RenderPass", json!({"depthTest": "true"}))],
        vec![],
    );
    let err =
        validate_scene_against(&scene, &scheme).expect_err("non-boolean depthTest should fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("RenderPass.depthTest"));
    assert!(msg.contains("expected boolean"));
}

#[test]
fn render_pass_depth_output_rejected_when_depth_test_is_false() {
    let scheme = load_default_scheme().expect("load default scheme");
    let scene = scene(
        vec![
            node("rp", "RenderPass", json!({"depthTest": false})),
            node("comp", "Composite", json!({})),
        ],
        vec![Connection {
            id: "c_depth".to_string(),
            from: Endpoint {
                node_id: "rp".to_string(),
                port_id: "depth".to_string(),
            },
            to: Endpoint {
                node_id: "comp".to_string(),
                port_id: "pass".to_string(),
            },
        }],
    );

    let err = validate_scene_against(&scene, &scheme)
        .expect_err("RenderPass.depth should fail when depthTest=false");
    let msg = format!("{err:#}");
    assert!(msg.contains("rp.depth"));
    assert!(msg.contains("depthTest"));
}

#[test]
fn render_pass_depth_output_allowed_when_depth_test_is_true() {
    let scheme = load_default_scheme().expect("load default scheme");
    let scene = scene(
        vec![
            node("rp", "RenderPass", json!({"depthTest": true})),
            node("comp", "Composite", json!({})),
        ],
        vec![Connection {
            id: "c_depth".to_string(),
            from: Endpoint {
                node_id: "rp".to_string(),
                port_id: "depth".to_string(),
            },
            to: Endpoint {
                node_id: "comp".to_string(),
                port_id: "pass".to_string(),
            },
        }],
    );

    validate_scene_against(&scene, &scheme)
        .expect("RenderPass.depth should validate when depthTest=true");
}
