use node_forge_render_server::{dsl::SceneDSL, renderer::ShaderSpaceBuilder};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
use serde_json::json;

fn scene(external_shape: bool) -> SceneDSL {
    let mut scene: SceneDSL = serde_json::from_value(json!({
        "version": "6.0", "workingColorSpace": "linear-srgb", "metadata":{"name":"glass lighting"},
        "nodes": [
            {"id":"geometry", "type":"Rect2DGeometry", "params":{"size":[32,32], "position":[16,16]}},
            {"id":"background", "type":"ColorInput", "params":{"value":[0.1,0.3,0.4,1]}},
            {"id":"background.pass", "type":"RenderPass", "params":{}},
            {"id":"light", "type":"ColorInput", "params":{"value":[0,0,0,1]}},
            {"id":"light.pass", "type":"RenderPass", "params":{}},
            {"id":"radius", "type":"FloatInput", "params":{"value":0}},
            {"id":"glass", "type":"GlassMaterial", "params":{
                "uAlpha":0.4, "uGeoPxRadius":0, "uShapeEdgePx":2, "uShapeThicknessPx":0,
                "uShapeReflectOffsetPx":0, "uReflectStrength":1, "uRefractIorIOR":1,
                "uDirectionalLightDirection":[0,0,1], "uDirectionalLightAngleRange":0.5,
                "uDirectionalLightIntensity":0, "uDirectionalLightOppositeIntensity":0,
                "uInnerGlassColor":[0.02,0.42,0.55,1], "uInnerColorMix":1, "uInnerColorPow":1,
                "uInnerColorWhite":0, "uInnerBottom":0, "uBgColorSaturation":1, "uBgColorBrightness":0,
                "uBlendLuminanceAmount":0, "uDebugFixNeutralVibrancy":0, "uBionicUnShade":0
            }},
            {"id":"draw", "type":"RenderPass", "params":{"blend_preset":"premul_alpha"}},
            {"id":"target", "type":"RenderTexture", "params":{"width":32, "height":32, "format":"rgba16float"}},
            {"id":"composite", "type":"Composite", "params":{}},
            {"id":"screen", "type":"Screen", "params":{"width":32, "height":32, "outputColorSpace":"srgb"}}
        ], "connections":[], "outputs":null
    })).unwrap();
    let mut edges = vec![
        ("geometry", "geometry", "background.pass", "geometry"),
        ("background", "color", "background.pass", "material"),
        ("background.pass", "texture", "glass", "uBgTex"),
        ("geometry", "geometry", "light.pass", "geometry"),
        ("light", "color", "light.pass", "material"),
        ("radius", "value", "glass", "uGeoPxRadius"),
        ("geometry", "geometry", "draw", "geometry"),
        ("glass", "material", "draw", "material"),
        ("draw", "pass", "composite", "pass"),
        ("target", "texture", "composite", "target"),
        ("composite", "texture", "screen", "texture"),
    ];
    if external_shape {
        scene.nodes.extend([
            serde_json::from_value(
                json!({"id":"distance", "type":"FloatInput", "params":{"value":-100}}),
            )
            .unwrap(),
            serde_json::from_value(
                json!({"id":"normal", "type":"Vector3Input", "params":{"x":0,"y":0,"z":1}}),
            )
            .unwrap(),
        ]);
        edges.extend([
            ("distance", "value", "glass", "uSdfDistance"),
            ("normal", "vector", "glass", "uNormal"),
        ]);
    }
    scene.connections = edges
        .into_iter()
        .enumerate()
        .map(|(i, (a, b, c, d))| {
            serde_json::from_value(json!({"id":format!("edge.{i}"),
            "from":{"nodeId":a,"portId":b}, "to":{"nodeId":c,"portId":d}}))
            .unwrap()
        })
        .collect();
    scene
}

fn render(gpu: &HeadlessRenderer, scene: &SceneDSL) -> [f32; 4] {
    let built = ShaderSpaceBuilder::new(gpu.device.clone(), gpu.queue.clone())
        .with_adapter(gpu.adapter.clone())
        .build(scene)
        .unwrap();
    built.shader_space.render();
    let pixels = built
        .shader_space
        .read_texture_rgba16f("target")
        .unwrap()
        .channels;
    assert!(pixels.iter().all(|v| v.is_finite()));
    pixels[(16 * 32 + 16) * 4..(16 * 32 + 16) * 4 + 4]
        .try_into()
        .unwrap()
}

fn oklab(pixel: [f32; 4]) -> [f32; 3] {
    let [r, g, b] = [
        pixel[0] / pixel[3],
        pixel[1] / pixel[3],
        pixel[2] / pixel[3],
    ];
    let l = (0.41222147 * r + 0.53633254 * g + 0.05144599 * b).cbrt();
    let m = (0.21190350 * r + 0.68069955 * g + 0.10739696 * b).cbrt();
    let s = (0.08830246 * r + 0.28171884 * g + 0.62997870 * b).cbrt();
    [
        0.21045426 * l + 0.7936178 * m - 0.004072047 * s,
        1.9779985 * l - 2.4285922 * m + 0.4505937 * s,
        0.02590404 * l + 0.78277177 * m - 0.80867577 * s,
    ]
}

#[test]
fn glass_combines_lights_and_authored_curve_once_without_changing_chroma_or_alpha() {
    let gpu =
        HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("native GPU required");
    for external_shape in [false, true] {
        let unlit_scene = scene(external_shape);
        let unlit = render(&gpu, &unlit_scene);
        assert!(
            (unlit[3] - 0.4).abs() < 0.001,
            "shape={external_shape}, unlit={unlit:?}"
        );
        let [l, a, b] = oklab(unlit);
        for (main, opposite, map, amount) in [
            (0.0, 0.0, 0.0, 0.0),
            (0.5, 0.0, 0.0, 0.0),
            (0.0, 0.5, 0.0, 0.0),
            (0.0, 0.0, 0.5, 0.0),
            (0.15, 0.1, 0.5, 0.6),
            // HDR output catches sequential remaps that clamp L between light sources.
            (2.0, 2.0, 1.0, 0.6),
        ] {
            let mut lit_scene = unlit_scene.clone();
            let glass = lit_scene
                .nodes
                .iter_mut()
                .find(|n| n.id == "glass")
                .unwrap();
            glass
                .params
                .insert("uDirectionalLightIntensity".into(), json!(main));
            glass
                .params
                .insert("uDirectionalLightOppositeIntensity".into(), json!(opposite));
            glass
                .params
                .insert("uBlendLuminanceAmount".into(), json!(amount));
            glass
                .params
                .insert("uBlendLuminanceValues".into(), json!([0.0, 0.45, 0.8, 1.0]));
            lit_scene
                .nodes
                .iter_mut()
                .find(|n| n.id == "light")
                .unwrap()
                .params
                .insert("value".into(), json!([map, 0, 0, 1]));
            lit_scene.connections.push(
                serde_json::from_value(json!({"id":"light.map",
                "from":{"nodeId":"light.pass","portId":"texture"},
                "to":{"nodeId":"glass","portId":"uLightTex"}}))
                .unwrap(),
            );
            let lit = render(&gpu, &lit_scene);
            let [actual_l, actual_a, actual_b] = oklab(lit);
            let curve_l =
                3.0 * (1.0 - l).powi(2) * l * 0.45 + 3.0 * (1.0 - l) * l * l * 0.8 + l.powi(3);
            let expected_l = l + (curve_l - l) * amount + 0.2 * (main + opposite + map * map);
            assert!(
                (actual_l - expected_l).abs() < 0.002,
                "shape={external_shape}, lights={main}/{opposite}/{map}, L={actual_l}, expected={expected_l}"
            );
            assert!(
                (actual_a - a).abs() < 0.002 && (actual_b - b).abs() < 0.002,
                "lighting must preserve OKLab chroma"
            );
            assert!(
                (lit[3] - unlit[3]).abs() < 0.001,
                "lighting must preserve alpha"
            );
        }
    }
}
