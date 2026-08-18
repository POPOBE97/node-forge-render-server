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
    let _function_registry = support::function_registry_lock();
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
                assert_eq!(
                    bundle
                        .module
                        .matches("fn legacy_particle_linear_to_srgb_channel_")
                        .count(),
                    1,
                    "the legacy OETF must remain local to the voice particle material"
                );
                assert_eq!(
                    bundle
                        .module
                        .matches("fn legacy_particle_srgb_to_linear_channel_")
                        .count(),
                    1,
                    "the legacy EOTF must remain local to the voice particle material"
                );
                assert!(
                    bundle
                        .module
                        .contains("legacy_particle_linear_premul_to_srgb_premul_")
                );
                assert!(bundle.module.contains("legacy_particle_srgb_to_linear_"));
                assert!(
                    !bundle
                        .module
                        .contains("fn srgb_to_linear_GroupInstance_32_ShaderMaterial_32"),
                    "canonical color inputs must not regain a generic shader-local decode"
                );
                assert!(
                    !bundle
                        .module
                        .contains("fn linear_to_srgb_GroupInstance_32_ShaderMaterial_32"),
                    "encoded-domain conversion must stay explicitly particle-scoped"
                );
            }
        }
    }
}

fn srgb_to_linear_channel(value: f32) -> f32 {
    let nonnegative = value.max(0.0);
    if nonnegative <= 0.04045 {
        nonnegative / 12.92
    } else {
        ((nonnegative + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb_channel(value: f32) -> f32 {
    let nonnegative = value.max(0.0);
    if nonnegative <= 0.0031308 {
        nonnegative * 12.92
    } else {
        1.055 * nonnegative.powf(1.0 / 2.4) - 0.055
    }
}

fn premultiply(color: [f32; 4]) -> [f32; 4] {
    let alpha = color[3].clamp(0.0, 1.0);
    [color[0] * alpha, color[1] * alpha, color[2] * alpha, alpha]
}

fn mix_color(from: [f32; 4], to: [f32; 4], amount: f32) -> [f32; 4] {
    std::array::from_fn(|channel| from[channel] + (to[channel] - from[channel]) * amount)
}

fn historical_particle_color(
    noise_srgb: [f32; 4],
    particle_srgb: [f32; 4],
    noise_amount: f32,
    gain: f32,
) -> [f32; 4] {
    let working = mix_color(
        premultiply(noise_srgb),
        premultiply(particle_srgb),
        noise_amount,
    );
    let alpha = working[3].clamp(0.0, 1.0);
    [
        srgb_to_linear_channel((working[0] * gain.max(0.0)).max(0.0)) * alpha,
        srgb_to_linear_channel((working[1] * gain.max(0.0)).max(0.0)) * alpha,
        srgb_to_linear_channel((working[2] * gain.max(0.0)).max(0.0)) * alpha,
        alpha,
    ]
}

fn migrate_color_to_linear(color_srgb: [f32; 4]) -> [f32; 4] {
    [
        srgb_to_linear_channel(color_srgb[0]),
        srgb_to_linear_channel(color_srgb[1]),
        srgb_to_linear_channel(color_srgb[2]),
        color_srgb[3],
    ]
}

fn reconstruct_srgb_premul(linear_premul: [f32; 4]) -> [f32; 4] {
    let alpha = linear_premul[3].clamp(0.0, 1.0);
    if alpha <= 0.000001 {
        return [0.0; 4];
    }
    [
        linear_to_srgb_channel((linear_premul[0] / alpha).max(0.0)) * alpha,
        linear_to_srgb_channel((linear_premul[1] / alpha).max(0.0)) * alpha,
        linear_to_srgb_channel((linear_premul[2] / alpha).max(0.0)) * alpha,
        alpha,
    ]
}

fn migrated_particle_color_with_legacy_domain(
    noise_linear: [f32; 4],
    particle_linear: [f32; 4],
    noise_amount: f32,
    gain: f32,
) -> [f32; 4] {
    let noise_linear_premul = premultiply(noise_linear);
    let particle_linear_premul = premultiply(particle_linear);
    let working = mix_color(
        reconstruct_srgb_premul(noise_linear_premul),
        reconstruct_srgb_premul(particle_linear_premul),
        noise_amount,
    );
    let alpha = working[3].clamp(0.0, 1.0);
    [
        srgb_to_linear_channel((working[0] * gain.max(0.0)).max(0.0)) * alpha,
        srgb_to_linear_channel((working[1] * gain.max(0.0)).max(0.0)) * alpha,
        srgb_to_linear_channel((working[2] * gain.max(0.0)).max(0.0)) * alpha,
        alpha,
    ]
}

#[test]
fn voice_particle_legacy_srgb_domain_survives_linear_color_migration() {
    const TOLERANCE: f32 = 1.0 / 512.0;
    const NOISE_RGB: [f32; 3] = [0.4, 0.239_215_69, 0.890_196_1];
    const PARTICLE_RGB: [f32; 3] = [1.0, 1.0, 1.0];
    const NOISE_PARAM_ID: &str = "sp_65594896dc81803a";
    const PARTICLE_PARAM_ID: &str = "sp_0a32fb88e75b5264";

    let _function_registry = support::function_registry_lock();
    let scene = support::load_render_case_scene("doubao-voice-interaction");
    let state = scene
        .state_machine
        .as_ref()
        .and_then(|state_machine| {
            state_machine
                .states
                .iter()
                .find(|state| state.id == "st_push_to_talk")
        })
        .expect("PushToTalk state");
    let fixture_color = |state_param_id: &str| -> [f32; 4] {
        let channels = state
            .state_param_overrides
            .get(state_param_id)
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("missing color override {state_param_id}"));
        std::array::from_fn(|channel| {
            channels[channel]
                .as_f64()
                .unwrap_or_else(|| panic!("invalid color channel {state_param_id}[{channel}]"))
                as f32
        })
    };
    let fixture_noise_linear = fixture_color(NOISE_PARAM_ID);
    let fixture_particle_linear = fixture_color(PARTICLE_PARAM_ID);
    let historical_noise_srgb = [NOISE_RGB[0], NOISE_RGB[1], NOISE_RGB[2], 1.0];
    let historical_particle_srgb = [PARTICLE_RGB[0], PARTICLE_RGB[1], PARTICLE_RGB[2], 1.0];
    for (fixture, expected) in [
        (
            fixture_noise_linear,
            migrate_color_to_linear(historical_noise_srgb),
        ),
        (
            fixture_particle_linear,
            migrate_color_to_linear(historical_particle_srgb),
        ),
    ] {
        for channel in 0..4 {
            assert!(
                (fixture[channel] - expected[channel]).abs() <= f32::EPSILON * 4.0,
                "fixture channel {channel} was not migrated with the sRGB EOTF"
            );
        }
    }

    for alpha in [0.0_f32, 0.5, 1.0] {
        let noise_srgb = [NOISE_RGB[0], NOISE_RGB[1], NOISE_RGB[2], alpha];
        let particle_srgb = [PARTICLE_RGB[0], PARTICLE_RGB[1], PARTICLE_RGB[2], alpha];
        let noise_linear = [
            fixture_noise_linear[0],
            fixture_noise_linear[1],
            fixture_noise_linear[2],
            alpha,
        ];
        let particle_linear = [
            fixture_particle_linear[0],
            fixture_particle_linear[1],
            fixture_particle_linear[2],
            alpha,
        ];
        for noise_amount in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            for gain in [0.0_f32, 1.0, 1.55] {
                let historical =
                    historical_particle_color(noise_srgb, particle_srgb, noise_amount, gain);
                let migrated = migrated_particle_color_with_legacy_domain(
                    noise_linear,
                    particle_linear,
                    noise_amount,
                    gain,
                );
                for channel in 0..4 {
                    let delta = (historical[channel] - migrated[channel]).abs();
                    assert!(
                        delta <= TOLERANCE,
                        "alpha={alpha}, noise={noise_amount}, gain={gain}, channel={channel}: \
                         historical={}, migrated={}, delta={delta}",
                        historical[channel],
                        migrated[channel],
                    );
                }
            }
        }
    }
}
