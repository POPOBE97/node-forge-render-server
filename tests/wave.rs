use node_forge_render_server::{dsl::SceneDSL, renderer::ShaderSpaceBuilder};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
use serde_json::{Value, json};

fn scene(progress: f64, feather: f64, normal: bool) -> SceneDSL {
    let edges = [
        ("progress", "value", "wave", "progress"),
        ("wave", "height", "pack", "height"),
        ("wave", "dir", "pack", "dir"),
        ("wave", "normal", "pack", "normal"),
        ("pack", "output", "draw", "material"),
        ("geometry", "geometry", "draw", "geometry"),
        ("draw", "pass", "composite", "pass"),
        ("target", "texture", "composite", "target"),
        ("composite", "texture", "screen", "texture"),
    ];
    serde_json::from_value(json!({
        "version":"6.1", "workingColorSpace":"linear-srgb", "dpi":2,
        "metadata":{"name":"wave field"},
        "nodes":[
            {"id":"progress", "type":"FloatInput", "params":{"value":progress}},
            {"id":"wave", "type":"Wave", "params":{
                "origin":[16.25,16.25], "inputUnits":{"origin":"dp"},
                "amplitude":0.05, "frequency":14, "decay":2, "speed":0.8,
                "damping":0.7, "front_feather":feather
            }},
            {"id":"pack", "type":"MathClosure", "params":{
                "source":if normal { "output = vec4(normal, 1.0);" } else { "output = vec4(height, dir, 1.0);" },
                "outputType":"vector4"
            }, "inputs":[
                {"id":"height", "name":"height", "type":"float"},
                {"id":"dir", "name":"dir", "type":"vector2"},
                {"id":"normal", "name":"normal", "type":"vector3"}
            ], "outputs":[{"id":"output", "name":"Output", "type":"vector4"}]},
            {"id":"geometry", "type":"Rect2DGeometry", "params":{"size":[65,65], "position":[32.5,32.5]}},
            {"id":"draw", "type":"RenderPass"},
            {"id":"target", "type":"RenderTexture", "params":{"width":65,"height":65,"format":"rgba16float"}},
            {"id":"composite", "type":"Composite"},
            {"id":"screen", "type":"Screen", "params":{"width":65,"height":65,"outputColorSpace":"srgb"}}
        ],
        "connections":edges.into_iter().enumerate().map(|(i,(a,b,c,d))| json!({
            "id":format!("edge.{i}"), "from":{"nodeId":a,"portId":b}, "to":{"nodeId":c,"portId":d}
        })).collect::<Vec<_>>()
    })).unwrap()
}

fn render(gpu: &HeadlessRenderer, scene: &SceneDSL) -> Vec<f32> {
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
    pixels
}

// Independent scalar reference; normals below use numerical spatial differences.
fn height(x: f64, y: f64, progress: f64, feather: f64) -> f64 {
    let r = x.hypot(y);
    let t = progress - r / 52.0;
    if t <= 0.0 {
        return 0.0;
    }
    let front = if feather > 0.0 {
        let u = (t * 52.0 / feather).clamp(0.0, 1.0);
        6.0 * u.powi(5) - 15.0 * u.powi(4) + 10.0 * u.powi(3)
    } else {
        1.0
    };
    3.25 * (-2.0 * t - r * 0.7 / 65.0).exp() * (14.0 * t).sin() * front
}

#[test]
fn wave_gpu_height_direction_and_normal_match_spatial_finite_differences() {
    let gpu =
        HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("native GPU required");
    for (progress, feather) in [(0.65, 0.0), (0.65, 8.0), (0.0, 8.0), (-1.0, 0.0)] {
        let field = render(&gpu, &scene(progress, feather, false));
        let normals = render(&gpu, &scene(progress, feather, true));
        for y in 0..65 {
            for x in 0..65 {
                let dx = x as f64 - 32.0;
                let dy = 32.0 - y as f64; // Texture readback rows are top-down.
                let i = (y * 65 + x) * 4;
                let expected_height = height(dx, dy, progress, feather);
                assert!(
                    (field[i] as f64 - expected_height).abs() < 0.0015,
                    "height at {x},{y}: {} != {expected_height}",
                    field[i]
                );
                let r = dx.hypot(dy).max(0.000001);
                for (actual, expected) in [(field[i + 1], dx / r), (field[i + 2], dy / r)] {
                    assert!((actual as f64 - expected).abs() < 0.0006);
                }
                let step = 0.0001;
                let gx = (height(dx + step, dy, progress, feather)
                    - height(dx - step, dy, progress, feather))
                    / (2.0 * step);
                let gy = (height(dx, dy + step, progress, feather)
                    - height(dx, dy - step, progress, feather))
                    / (2.0 * step);
                let length = (gx * gx + gy * gy + 1.0).sqrt();
                for (c, expected) in [-gx / length, -gy / length, 1.0 / length]
                    .into_iter()
                    .enumerate()
                {
                    assert!(
                        (normals[i + c] as f64 - expected).abs() < 0.0006,
                        "normal at {x},{y}, component {c}: {} != {expected}",
                        normals[i + c]
                    );
                }
            }
        }
    }
}

fn set(scene: &mut SceneDSL, id: &str, key: &str, value: Value) {
    scene
        .nodes
        .iter_mut()
        .find(|n| n.id == id)
        .unwrap()
        .params
        .insert(key.into(), value);
}

#[test]
fn wave_normal_drives_pbr_and_has_finite_zero_amplitude_and_zero_speed_limits() {
    let gpu =
        HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("native GPU required");
    let mut input = scene(0.65, 8.0, true);
    set(&mut input, "wave", "amplitude", json!(0));
    assert!(
        render(&gpu, &input)
            .chunks_exact(4)
            .all(|p| p == [0.0, 0.0, 1.0, 1.0])
    );
    set(&mut input, "wave", "amplitude", json!(0.05));
    set(&mut input, "wave", "speed", json!(0));
    assert!(
        render(&gpu, &input)
            .chunks_exact(4)
            .all(|p| p == [0.0, 0.0, 1.0, 1.0])
    );

    input = scene(0.65, 8.0, true);
    input.nodes.push(
        serde_json::from_value(json!({"id":"pbr","type":"PBRMaterial",
        "params":{"baseColor":[0.1,0.4,0.6,1],"roughness":0.3,"lightDirection":[-0.8,0.4,1]}}))
        .unwrap(),
    );
    input
        .connections
        .retain(|e| e.to.node_id != "draw" || e.to.port_id != "material");
    for (id, a, b, c, d) in [
        ("normal", "wave", "normal", "pbr", "normal"),
        ("material", "pbr", "material", "draw", "material"),
    ] {
        input.connections.push(
            serde_json::from_value(json!({"id":id,
            "from":{"nodeId":a,"portId":b},"to":{"nodeId":c,"portId":d}}))
            .unwrap(),
        );
    }
    let lit = render(&gpu, &input);
    set(&mut input, "pbr", "normalStrength", json!(0));
    let flat = render(&gpu, &input);
    assert!(
        lit.iter()
            .zip(&flat)
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            > 10.0
    );
    set(&mut input, "pbr", "normalStrength", json!(1));
    set(&mut input, "pbr", "lightDirection", json!([0.8, -0.4, 1]));
    let relit = render(&gpu, &input);
    assert!(
        lit.iter()
            .zip(&relit)
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            > 10.0
    );
    assert!(
        lit.chunks_exact(4)
            .zip(relit.chunks_exact(4))
            .all(|(a, b)| a[3] == b[3])
    );
}
