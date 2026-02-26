use std::collections::HashMap;

use node_forge_render_server::{
    dsl::{Metadata, Node, SceneDSL, normalize_scene_defaults, resolve_input_f64},
    schema::{PortTypeSpec, load_default_scheme},
};
use serde_json::json;

fn expected_blend_defaults() -> [(&'static str, serde_json::Value); 6] {
    [
        ("blend_preset", json!("premul_alpha")),
        ("blendfunc", json!("add")),
        ("src_factor", json!("one")),
        ("dst_factor", json!("one-minus-src-alpha")),
        ("src_alpha_factor", json!("one")),
        ("dst_alpha_factor", json!("one-minus-src-alpha")),
    ]
}

fn node(id: &str, node_type: &str) -> Node {
    Node {
        id: id.to_string(),
        node_type: node_type.to_string(),
        params: HashMap::new(),
        inputs: Vec::new(),
        input_bindings: Vec::new(),
        outputs: Vec::new(),
    }
}

#[test]
fn pass_nodes_expose_premul_blend_defaults_in_scheme() {
    let scheme = load_default_scheme().expect("load default scheme");
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
        for (key, value) in expected_blend_defaults() {
            assert_eq!(
                node_scheme.default_params.get(key),
                Some(&value),
                "{node_type}.defaultParams.{key} mismatch"
            );
        }
    }
}

#[test]
fn normalization_merges_premul_blend_defaults_for_pass_nodes() {
    let mut scene = SceneDSL {
        version: "1".to_string(),
        metadata: Metadata {
            name: "pass-blend-defaults".to_string(),
            created: None,
            modified: None,
        },
        nodes: vec![
            node("rp", "RenderPass"),
            node("gb", "GuassianBlurPass"),
            node("grb", "GradientBlur"),
            node("ds", "Downsample"),
            node("us", "Upsample"),
            node("comp", "Composite"),
        ],
        connections: Vec::new(),
        outputs: None,
        groups: Vec::new(),
        assets: Default::default(),
    };

    normalize_scene_defaults(&mut scene).expect("normalize scene defaults");

    for n in &scene.nodes {
        for (key, value) in expected_blend_defaults() {
            assert_eq!(
                n.params.get(key),
                Some(&value),
                "{}.{} default not merged",
                n.node_type,
                key
            );
        }
    }
}

#[test]
fn upsample_schema_ports_and_defaults_match_contract() {
    let scheme = load_default_scheme().expect("load default scheme");
    let node_scheme = scheme
        .nodes
        .get("Upsample")
        .expect("missing Upsample in scheme");

    let assert_input_type = |port_id: &str, expected: &str| match node_scheme.inputs.get(port_id) {
        Some(PortTypeSpec::One(actual)) => {
            assert_eq!(actual, expected, "Upsample input {port_id} type mismatch")
        }
        _ => panic!("Upsample input {port_id} missing or not a singular type"),
    };

    let assert_output_type = |port_id: &str, expected: &str| match node_scheme.outputs.get(port_id)
    {
        Some(PortTypeSpec::One(actual)) => {
            assert_eq!(actual, expected, "Upsample output {port_id} type mismatch")
        }
        _ => panic!("Upsample output {port_id} missing or not a singular type"),
    };

    assert_input_type("targetSize", "vector2");
    assert_input_type("source", "pass");
    assert_input_type("camera", "mat4");
    assert_input_type("address_mode", "any");
    assert_input_type("filter", "any");
    assert_input_type("blend_preset", "any");
    assert_output_type("output", "pass");

    let expected_defaults = [
        ("address_mode", json!("clamp-to-edge")),
        ("filter", json!("linear")),
        ("blend_preset", json!("premul_alpha")),
        ("blendfunc", json!("add")),
        ("src_factor", json!("one")),
        ("dst_factor", json!("one-minus-src-alpha")),
        ("src_alpha_factor", json!("one")),
        ("dst_alpha_factor", json!("one-minus-src-alpha")),
    ];
    for (key, value) in expected_defaults {
        assert_eq!(
            node_scheme.default_params.get(key),
            Some(&value),
            "Upsample.defaultParams.{key} mismatch"
        );
    }
}

#[test]
fn scheme_exposes_input_port_defaults() {
    let scheme = load_default_scheme().expect("load default scheme");

    let downsample = scheme
        .nodes
        .get("Downsample")
        .expect("missing Downsample in scheme");
    assert_eq!(
        downsample.input_defaults.get("sampling"),
        Some(&json!("Mirror"))
    );

    let gaussian = scheme
        .nodes
        .get("GuassianBlurPass")
        .expect("missing GuassianBlurPass in scheme");
    assert_eq!(gaussian.input_defaults.get("radius"), Some(&json!(5)));
}

#[test]
fn normalization_merges_input_defaults_before_default_params() {
    let mut scene = SceneDSL {
        version: "1".to_string(),
        metadata: Metadata {
            name: "input-default-merge".to_string(),
            created: None,
            modified: None,
        },
        nodes: vec![node("ds", "Downsample")],
        connections: Vec::new(),
        outputs: None,
        groups: Vec::new(),
        assets: Default::default(),
    };

    normalize_scene_defaults(&mut scene).expect("normalize scene defaults");
    let downsample = scene.nodes.iter().find(|n| n.id == "ds").unwrap();
    assert_eq!(downsample.params.get("sampling"), Some(&json!("Mirror")));
}

#[test]
fn resolve_input_uses_port_default_when_param_missing() {
    let scene = SceneDSL {
        version: "1".to_string(),
        metadata: Metadata {
            name: "port-default-resolve".to_string(),
            created: None,
            modified: None,
        },
        nodes: vec![Node {
            id: "gb".to_string(),
            node_type: "GuassianBlurPass".to_string(),
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
    let nodes_by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|n| (n.id.clone(), n))
        .collect();

    let radius = resolve_input_f64(&scene, &nodes_by_id, "gb", "radius")
        .expect("resolve radius")
        .expect("radius fallback");
    assert_eq!(radius, 5.0);
}
