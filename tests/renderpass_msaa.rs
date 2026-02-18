use std::collections::HashMap;

use node_forge_render_server::{
    dsl::{Metadata, Node, SceneDSL, normalize_scene_defaults},
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
        Some(&json!(0)),
        "RenderPass.defaultParams.msaaSampleCount must be 0"
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
        Some(&json!(0)),
        "RenderPass.msaaSampleCount default must be merged from schema"
    );
}

#[test]
fn render_pass_msaa_validation_accepts_allowed_values() {
    let scheme = load_default_scheme().expect("load default scheme");
    for value in [0, 2, 4, 8] {
        let scene = make_render_pass_scene(json!(value));
        validate_scene_against(&scene, &scheme)
            .unwrap_or_else(|e| panic!("value {value} should validate, got error: {e:#}"));
    }
}

#[test]
fn render_pass_msaa_validation_rejects_invalid_values() {
    let scheme = load_default_scheme().expect("load default scheme");
    let scene = make_render_pass_scene(json!(3));
    let err = validate_scene_against(&scene, &scheme).expect_err("value 3 should fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("RenderPass.msaaSampleCount"));
    assert!(msg.contains("must be one of 0,2,4,8"));
}
