use std::collections::HashMap;

use node_forge_render_server::{
    dsl::{Metadata, Node, SceneDSL, normalize_scene_defaults},
    schema::load_default_scheme,
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
