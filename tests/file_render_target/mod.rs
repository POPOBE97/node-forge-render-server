use std::collections::HashMap;

use node_forge_render_server::dsl::{
    file_render_target, normalize_scene_defaults, Metadata, Node, SceneDSL,
};

#[test]
fn file_render_target_applies_scheme_defaults() {
    let mut scene = SceneDSL {
        version: "1".to_string(),
        metadata: Metadata {
            name: "test".to_string(),
            created: None,
            modified: None,
        },
        nodes: vec![Node {
            id: "rt".to_string(),
            node_type: "File".to_string(),
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

    normalize_scene_defaults(&mut scene).unwrap();

    let rt = file_render_target(&scene)
        .unwrap()
        .expect("expected File render target");
    assert_eq!(rt.directory, "");
    assert_eq!(rt.file_name, "output.png");
}
