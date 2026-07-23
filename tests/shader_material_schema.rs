use std::collections::HashMap;

use node_forge_render_server::{
    dsl::{Connection, Endpoint, Metadata, Node, NodePort, SceneDSL},
    schema::{load_default_scheme, validate_scene_against},
};

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

fn connection(
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

fn scene(shader_input: NodePort, to_port: &str) -> SceneDSL {
    let mut shader = node("GroupInstance_32/ShaderMaterial_32", "ShaderMaterial");
    shader.inputs.push(shader_input);

    SceneDSL {
        version: "1.0".to_string(),
        metadata: Metadata {
            name: "shader-material-schema".to_string(),
            created: None,
            modified: None,
        },
        nodes: vec![
            node("IntelligentLight_30", "IntelligentLight"),
            node("PassTexture_31", "PassTexture"),
            shader,
        ],
        connections: vec![
            connection(
                "sys.group.edge.4",
                "IntelligentLight_30",
                "pass",
                "PassTexture_31",
                "pass",
            ),
            connection(
                "sys.group.edge.5",
                "PassTexture_31",
                "texture",
                "GroupInstance_32/ShaderMaterial_32",
                to_port,
            ),
        ],
        outputs: None,
        groups: Vec::new(),
        assets: Default::default(),
        state_machine: None,
        debug_artifacts: None,
    }
}

#[test]
fn rejects_direct_pass_to_shader_material_resource() {
    let mut scene = scene(
        NodePort {
            id: "resource:intelli_tex".to_string(),
            name: Some("intelli_tex".to_string()),
            port_type: Some("sampledTexture".to_string()),
            array_length: None,
        },
        "resource:intelli_tex",
    );
    scene.nodes.retain(|node| node.id != "PassTexture_31");
    scene.connections = vec![connection(
        "sys.group.edge.5",
        "IntelligentLight_30",
        "pass",
        "GroupInstance_32/ShaderMaterial_32",
        "resource:intelli_tex",
    )];

    let error = validate_scene_against(&scene, &load_default_scheme().expect("load scheme"))
        .expect_err("pass-to-sampledTexture must require an explicit PassTexture");
    assert!(error.to_string().contains("type mismatch"));
}

#[test]
fn accepts_reflected_shader_material_resource_port_after_group_expansion() {
    let scene = scene(
        NodePort {
            id: "resource:intelli_tex".to_string(),
            name: Some("intelli_tex".to_string()),
            port_type: Some("sampledTexture".to_string()),
            array_length: None,
        },
        "resource:intelli_tex",
    );

    validate_scene_against(&scene, &load_default_scheme().expect("load scheme"))
        .expect("reflected sampled resource port should validate");
}

#[test]
fn rejects_shader_material_port_not_present_in_reflected_inputs() {
    let scene = scene(
        NodePort {
            id: "resource:other".to_string(),
            name: Some("other".to_string()),
            port_type: Some("sampledTexture".to_string()),
            array_length: None,
        },
        "resource:intelli_tex",
    );

    let error = validate_scene_against(&scene, &load_default_scheme().expect("load scheme"))
        .expect_err("undeclared resource port must fail");
    assert!(
        error
            .to_string()
            .contains("unknown to port 'GroupInstance_32/ShaderMaterial_32.resource:intelli_tex'")
    );
}

#[test]
fn rejects_shader_material_resource_with_forged_value_type() {
    let scene = scene(
        NodePort {
            id: "resource:intelli_tex".to_string(),
            name: Some("intelli_tex".to_string()),
            port_type: Some("float".to_string()),
            array_length: None,
        },
        "resource:intelli_tex",
    );

    let error = validate_scene_against(&scene, &load_default_scheme().expect("load scheme"))
        .expect_err("resource port must use sampledTexture");
    assert!(error.to_string().contains("uses unknown to port"));
}
