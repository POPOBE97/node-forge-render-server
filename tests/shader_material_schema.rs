use std::collections::HashMap;

use node_forge_render_server::{
    dsl::{Connection, Endpoint, Metadata, Node, NodePort, SceneDSL},
    renderer,
    renderer::validation,
    schema::{load_default_scheme, validate_scene_against},
};

mod support;

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
            node("TextureSampler_31", "TextureSampler"),
            shader,
        ],
        connections: vec![
            connection(
                "sys.group.edge.4",
                "IntelligentLight_30",
                "pass",
                "TextureSampler_31",
                "texture",
            ),
            connection(
                "sys.group.edge.5",
                "TextureSampler_31",
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
fn accepts_direct_pass_to_shader_material_resource() {
    let mut scene = scene(
        NodePort {
            id: "resource:intelli_tex".to_string(),
            name: Some("intelli_tex".to_string()),
            port_type: Some("texture".to_string()),
            array_length: None,
        },
        "resource:intelli_tex",
    );
    scene.nodes.retain(|node| node.id != "TextureSampler_31");
    scene.connections = vec![connection(
        "sys.group.edge.5",
        "IntelligentLight_30",
        "pass",
        "GroupInstance_32/ShaderMaterial_32",
        "resource:intelli_tex",
    )];

    validate_scene_against(&scene, &load_default_scheme().expect("load scheme"))
        .expect("direct pass should satisfy a custom shader resource");
}

#[test]
fn accepts_image_pass_texture_to_shader_material_resource() {
    let mut shader = node("shader", "ShaderMaterial");
    shader.inputs.push(NodePort {
        id: "resource:image".to_string(),
        name: Some("image".to_string()),
        port_type: Some("texture".to_string()),
        array_length: None,
    });
    let scene = SceneDSL {
        version: "6.0".to_string(),
        metadata: Metadata {
            name: "image-pass-shader-resource".to_string(),
            created: None,
            modified: None,
        },
        nodes: vec![node("image_pass", "ImagePass"), shader],
        connections: vec![connection(
            "image-pass-resource",
            "image_pass",
            "texture",
            "shader",
            "resource:image",
        )],
        outputs: None,
        groups: Vec::new(),
        assets: Default::default(),
        state_machine: None,
        debug_artifacts: None,
    };

    validate_scene_against(&scene, &load_default_scheme().expect("load scheme"))
        .expect("ImagePass.texture should satisfy a custom shader texture resource");
}

#[test]
fn accepts_texture_source_for_image_pass_image_input() {
    let scene = SceneDSL {
        version: "6.0".to_string(),
        metadata: Metadata {
            name: "image-pass-texture-input".to_string(),
            created: None,
            modified: None,
        },
        nodes: vec![
            node("source", "ImageTexture"),
            node("image_pass", "ImagePass"),
        ],
        connections: vec![connection(
            "image-pass-input",
            "source",
            "texture",
            "image_pass",
            "image",
        )],
        outputs: None,
        groups: Vec::new(),
        assets: Default::default(),
        state_machine: None,
        debug_artifacts: None,
    };

    validate_scene_against(&scene, &load_default_scheme().expect("load scheme"))
        .expect("ImagePass.image should accept a texture");
}

#[test]
fn accepts_reflected_shader_material_resource_port_after_group_expansion() {
    let scene = scene(
        NodePort {
            id: "resource:intelli_tex".to_string(),
            name: Some("intelli_tex".to_string()),
            port_type: Some("texture".to_string()),
            array_length: None,
        },
        "resource:intelli_tex",
    );

    validate_scene_against(&scene, &load_default_scheme().expect("load scheme"))
        .expect("reflected pass resource port should validate");
}

#[test]
fn rejects_shader_material_port_not_present_in_reflected_inputs() {
    let scene = scene(
        NodePort {
            id: "resource:other".to_string(),
            name: Some("other".to_string()),
            port_type: Some("texture".to_string()),
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
        .expect_err("resource port must use texture");
    assert!(error.to_string().contains("uses unknown to port"));
}

#[test]
fn rejects_custom_shader_resource_exposed_as_pass() {
    let scene = scene(
        NodePort {
            id: "resource:intelli_tex".to_string(),
            name: Some("intelli_tex".to_string()),
            port_type: Some("pass".to_string()),
            array_length: None,
        },
        "resource:intelli_tex",
    );

    let error = validate_scene_against(&scene, &load_default_scheme().expect("load scheme"))
        .expect_err("custom shader resources must be exposed as texture");
    assert!(error.to_string().contains("uses unknown to port"));
}

#[test]
fn aligned_voice_interaction_shaders_compile_without_a_gpu() {
    for case_name in ["intelligent-light", "doubao-voice-interaction"] {
        let (scene, assets) = support::load_render_case(case_name);
        let bundles =
            renderer::build_all_pass_wgsl_bundles_from_scene_with_assets(&scene, Some(&assets))
                .unwrap_or_else(|error| {
                    panic!("{case_name}: failed to build shader bundles: {error:#}")
                });

        for (pass_id, bundle) in bundles {
            validation::validate_wgsl_with_context(
                &bundle.module,
                &format!("{case_name}, pass {pass_id}"),
            )
            .unwrap_or_else(|error| panic!("{case_name}, pass {pass_id}: {error:#}"));

            if pass_id.contains("sys.ilight.") {
                assert!(
                    !bundle.module.contains("particle"),
                    "{case_name}, pass {pass_id}: particles must not be in IntelligentLight"
                );
            }
            if case_name == "doubao-voice-interaction"
                && pass_id == "GroupInstance_32/RenderPass_26"
            {
                assert!(bundle.module.contains("apply_particles"));
                assert!(!bundle.module.contains("pow(intelligent_light"));
                assert!(bundle.module.contains("let light_envelope = mix("));
                assert!(
                    bundle
                        .module
                        .contains("let light_gain = max(light_envelope * glow, 0.0)")
                );
                assert!(
                    bundle
                        .module
                        .contains("clamp(intelligent_light.a * light_gain, 0.0, 1.0)")
                );
            }
        }
    }
}
