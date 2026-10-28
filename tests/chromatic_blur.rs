use image::ImageEncoder;
use node_forge_render_server::{
    asset_store::{AssetData, AssetStore},
    dsl::{self, SceneDSL},
    renderer::{self, ShaderSpaceBuildResult, ShaderSpaceBuilder, graph_uniforms},
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig, wgpu};
use serde_json::json;

mod support;

fn gpu() -> Option<HeadlessRenderer> {
    let renderer = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("No GPU available for chromatic blur: {error:?}");
            return None;
        }
    };
    if renderer.adapter.get_info().backend == wgpu::Backend::Noop {
        eprintln!("Skipping chromatic pixels on the non-rendering noop backend");
        return None;
    }
    Some(renderer)
}

fn build(gpu: &HeadlessRenderer, scene: &SceneDSL, assets: AssetStore) -> ShaderSpaceBuildResult {
    ShaderSpaceBuilder::new(gpu.device.clone(), gpu.queue.clone())
        .with_adapter(gpu.adapter.clone())
        .with_asset_store(assets)
        .build(scene)
        .expect("build chromatic graph")
}

fn pixels(build: &ShaderSpaceBuildResult) -> Vec<f32> {
    build.shader_space.render();
    build
        .shader_space
        .read_texture_rgba16f(build.scene_output_texture.as_str())
        .expect("read linear chromatic output")
        .channels
}

fn max_difference(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(a, b)| {
            assert!(a.is_finite() && b.is_finite());
            (a - b).abs()
        })
        .fold(0.0, f32::max)
}

#[test]
fn chromatic_matches_research_and_updates_shared_center_without_rebuild() {
    let _registry = support::function_registry_lock();
    let Some(gpu) = gpu() else { return };
    let (reference, reference_assets) = support::load_render_case("blur-chromatic-node");
    let mut reference = build(&gpu, &reference, reference_assets);
    let (integrated, integrated_assets) = support::load_render_case("blur-chromatic");
    let mut integrated = build(&gpu, &integrated, integrated_assets);
    let local_size = integrated.pass_bindings.iter()
        .find(|pass| pass.pass_id == "sys.cb.chromatic.blur.radial.pass")
        .expect("chromatic source-local size").base_params.geo_size;
    let initial = pixels(&integrated);
    let delta = max_difference(&pixels(&reference), &initial);
    assert!(delta <= 0.002, "default pixel delta: {delta}");

    for center in [[0.25, 0.75], [0.0, 0.0], [1.0, 1.0]] {
        for (build, scale) in [(&mut reference, [1.0, 1.0]), (&mut integrated, local_size)] {
            let scene = build.prepared_scene.as_mut().expect("prepared scene");
            let declaration = scene
                .nodes
                .iter_mut()
                .find(|n| n.id == "effect.center")
                .unwrap();
            declaration.params.insert("x".into(), json!(center[0] * scale[0]));
            declaration.params.insert("y".into(), json!(center[1] * scale[1]));
            let updated = graph_uniforms::apply_graph_uniform_updates(
                &mut build.pass_bindings,
                &mut build.shader_space,
                scene,
            )
            .expect("upload center to all three stages");
            assert_eq!(updated, 3);
        }
        let actual = pixels(&integrated);
        let delta = max_difference(&pixels(&reference), &actual);
        assert!(delta <= 0.002, "center {center:?} pixel delta: {delta}");
        assert!(
            max_difference(&initial, &actual) > 0.05,
            "center must change the effect"
        );
    }
}

fn small_scene(w: u32, h: u32) -> (SceneDSL, AssetStore, Vec<f32>) {
    let mut rgba = Vec::new();
    let mut expected = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let pixel = [
                (x * 255 / w) as u8,
                (y * 255 / h) as u8,
                ((x + y) * 127 / (w + h)) as u8,
                if x < 3 { 0 } else { 128 },
            ];
            let alpha = pixel[3] as f32 / 255.0;
            expected.extend([
                pixel[0] as f32 / 255.0 * alpha,
                pixel[1] as f32 / 255.0 * alpha,
                pixel[2] as f32 / 255.0 * alpha,
                alpha,
            ]);
            rgba.extend(pixel);
        }
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&rgba, w, h, image::ExtendedColorType::Rgba8)
        .unwrap();
    let assets = AssetStore::new();
    assets.insert(
        "pixels",
        AssetData {
            bytes: png,
            mime_type: "image/png".into(),
            original_name: "pixels.png".into(),
        },
    );
    let scene = serde_json::from_value(json!({
        "version": "6.1", "workingColorSpace": "linear-srgb", "dpi": 3.0, "metadata": {"name": "chromatic-contract"},
        "nodes": [
            {"id":"image", "type":"ImageTexture", "params": {"assetId":"pixels", "encoderSpace":"linear", "alphaMode":"straight", "aspectCorrection":"off"}},
            {"id":"blur", "type":"ChromaticBlur", "params": {
                "radialEnabled":false, "rgbEnabled":false, "gaussianEnabled":false,
                "center":[-1,-1], "rgbSamples":1, "rgbSpread":0,
                "rgbOffset":4, "rgbFixedDirection":true, "rgbAngle":90
            }},
            {"id":"radial.mask", "type":"MathClosure", "params":{"source":"output = 1.0;", "outputType":"float"}, "outputs":[{"id":"output","name":"Output","type":"float"}]},
            {"id":"rgb.mask", "type":"MathClosure", "params":{"source":"output = 1.0;", "outputType":"float"}, "outputs":[{"id":"output","name":"Output","type":"float"}]},
            {"id":"gaussian.mask", "type":"MathClosure", "params":{"source":"output = 1.0;", "outputType":"float"}, "outputs":[{"id":"output","name":"Output","type":"float"}]},
            {"id":"target", "type":"RenderTexture", "params":{"width":w,"height":h,"format":"rgba16float"}},
            {"id":"draw", "type":"ImagePass", "params":{"loadOp":"none"}},
            {"id":"composite", "type":"Composite"},
            {"id":"screen", "type":"Screen", "params":{ "outputColorSpace": "srgb","width":w,"height":h}}
        ],
        "connections": [
            {"id":"radial.mask", "from":{"nodeId":"radial.mask","portId":"output"}, "to":{"nodeId":"blur","portId":"radialMask"}},
            {"id":"rgb.mask", "from":{"nodeId":"rgb.mask","portId":"output"}, "to":{"nodeId":"blur","portId":"rgbMask"}},
            {"id":"gaussian.mask", "from":{"nodeId":"gaussian.mask","portId":"output"}, "to":{"nodeId":"blur","portId":"gaussianMask"}},
            {"id":"source", "from":{"nodeId":"image","portId":"texture"}, "to":{"nodeId":"blur","portId":"texture"}},
            {"id":"result", "from":{"nodeId":"blur","portId":"texture"}, "to":{"nodeId":"draw","portId":"image"}},
            {"id":"draw", "from":{"nodeId":"draw","portId":"pass"}, "to":{"nodeId":"composite","portId":"pass"}},
            {"id":"target", "from":{"nodeId":"target","portId":"texture"}, "to":{"nodeId":"composite","portId":"target"}},
            {"id":"screen", "from":{"nodeId":"composite","portId":"texture"}, "to":{"nodeId":"screen","portId":"texture"}}
        ]
    })).unwrap();
    (scene, assets, expected)
}

#[test]
fn chromatic_uses_local_pixels_y_up_and_preserves_disabled_premultiplied_input() {
    let Some(gpu) = gpu() else { return };
    for (w, h) in [(40, 24), (73, 51)] {
        let (mut scene, assets, expected) = small_scene(w, h);
        let disabled_build = build(&gpu, &scene, assets.clone());
        let disabled = pixels(&disabled_build);
        let delta = max_difference(&expected, &disabled);
        assert!(
            delta < 0.001,
            "disabled effects must pass through premultiplied pixels: {delta}"
        );
        scene
            .nodes
            .iter_mut()
            .find(|n| n.id == "blur")
            .unwrap()
            .params
            .insert("rgbEnabled".into(), json!(true));
        let shifted = pixels(&build(&gpu, &scene, assets.clone()));
        // Readback rows are top-down. Positive LocalUV Y samples four rows upward.
        for y in 5..h - 5 {
            for x in 5..w - 5 {
                let i = (y * w + x) as usize * 4;
                let red = ((y - 4) * w + x) as usize * 4;
                let blue = ((y + 4) * w + x) as usize * 4;
                for (channel, value) in [
                    expected[red],
                    expected[i + 1],
                    expected[blue + 2],
                    expected[i + 3],
                ]
                .iter()
                .enumerate()
                {
                    assert!(
                        (shifted[i + channel] - value).abs() < 0.001,
                        "{w}x{h} pixel ({x},{y}) channel {channel}: {} != {value}",
                        shifted[i + channel]
                    );
                }
            }
        }
        // A custom camera is consumed by the source pass once; filters stay local/fullscreen.
        let custom = renderer::camera::legacy_projection_camera_matrix([w as f32, h as f32]);
        let mut custom = custom;
        custom[0] *= 0.9;
        let row_major: [f32; 16] = std::array::from_fn(|i| custom[(i % 4) * 4 + i / 4]);
        scene
            .nodes
            .iter_mut()
            .find(|n| n.id == "blur")
            .unwrap()
            .params
            .insert("camera".into(), json!(row_major));
        let camera_build = build(&gpu, &scene, assets);
        let source = camera_build
            .pass_bindings
            .iter()
            .find(|p| p.pass_id == "sys.gb.blur.src.pass")
            .expect("custom camera source pass");
        assert_eq!(source.base_params.camera, custom);
        for pass in camera_build
            .pass_bindings
            .iter()
            .filter(|p| p.pass_id.starts_with("sys.cb.blur."))
        {
            assert_eq!(
                pass.base_params.camera,
                renderer::camera::legacy_projection_camera_matrix(pass.base_params.target_size)
            );
        }
    }
}

#[test]
fn chromatic_mirror_sampling_preserves_white_at_all_edges() {
    let Some(gpu) = gpu() else { return };
    let (w, h) = (40, 24);
    for alpha in [255u8, 128] {
        let (mut scene, _, _) = small_scene(w, h);
        let assets = AssetStore::new();
        let rgba = [255, 255, 255, alpha].repeat((w * h) as usize);
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&rgba, w, h, image::ExtendedColorType::Rgba8)
            .unwrap();
        assets.insert(
            "pixels",
            AssetData {
                bytes: png,
                mime_type: "image/png".into(),
                original_name: "white.png".into(),
            },
        );
        scene.nodes.push(serde_json::from_value(json!({
            "id":"mirror", "type":"TextureSampler", "params":{
                "addressModeU":"mirror-repeat", "addressModeV":"mirror-repeat",
                "magFilter":"linear", "minFilter":"linear"
            }
        })).unwrap());
        scene.connections.iter_mut().find(|c| c.id == "source").unwrap()
            .from.node_id = "mirror".into();
        scene.connections.push(serde_json::from_value(json!({
            "id":"mirror.source", "from":{"nodeId":"image","portId":"texture"},
            "to":{"nodeId":"mirror","portId":"texture"}
        })).unwrap());

        for (radial, rgb, gaussian) in [(true, false, false), (false, true, false), (true, true, true)] {
            let blur = scene.nodes.iter_mut().find(|n| n.id == "blur").unwrap();
            blur.params.insert("radialEnabled".into(), json!(radial));
            blur.params.insert("rgbEnabled".into(), json!(rgb));
            blur.params.insert("gaussianEnabled".into(), json!(gaussian));
            blur.params.insert("center".into(), json!([w as f32 * 0.5, 0.0]));
            blur.params.insert("radialStrength".into(), json!(0.5));
            blur.params.insert("rgbFixedDirection".into(), json!(false));
            blur.params.insert("rgbOffset".into(), json!(16));
            blur.params.insert("rgbChannels".into(), json!([1, 0, -1]));
            blur.params.insert("gaussianSigma".into(), json!(5));
            let actual = pixels(&build(&gpu, &scene, assets.clone()));
            // Even out-of-bounds taps must return the same premultiplied white.
            // Clearing R alone creates cyan; clearing radial taps also loses alpha.
            let expected = alpha as f32 / 255.0;
            for (i, value) in actual.iter().enumerate() {
                assert!(
                    (value - expected).abs() < 0.002,
                    "radial={radial} rgb={rgb} gaussian={gaussian} alpha={alpha} pixel ({}, {}) channel {}: {value} != {expected}",
                    i / 4 % w as usize, i / 4 / w as usize, i % 4
                );
            }
        }
    }
}

#[test]
fn chromatic_radial_rgb_direction_uses_source_local_pixel_center() {
    let Some(gpu) = gpu() else { return };
    for (w, h) in [(40, 24), (73, 51)] {
        let (mut scene, assets, expected) = small_scene(w, h);
        let center = [w as f32 * 0.2, h as f32 * 0.6];
        let blur = scene.nodes.iter_mut().find(|n| n.id == "blur").unwrap();
        blur.params.insert("rgbEnabled".into(), json!(true));
        blur.params.insert("rgbFixedDirection".into(), json!(false));
        blur.params.insert("center".into(), json!(center));
        let actual = pixels(&build(&gpu, &scene, assets));
        for y in 8..h - 8 {
            for x in 8..w - 8 {
                let delta = [x as f32 + 0.5 - center[0], h as f32 - y as f32 - 0.5 - center[1]];
                let length = delta[0].hypot(delta[1]);
                let direction = delta.map(|v| v / length.max(0.0001));
                for (channel, offset) in [(0, 4.0), (1, 0.0), (2, -4.0)] {
                    // Readback is top-down; positive LocalPx Y samples upward.
                    let sx = x as f32 + direction[0] * offset;
                    let sy = y as f32 - direction[1] * offset;
                    let (ix, iy) = (sx.floor() as u32, sy.floor() as u32);
                    let (fx, fy) = (sx.fract(), sy.fract());
                    let at = |dx, dy| expected[(((iy + dy) * w + ix + dx) * 4) as usize + channel];
                    let top = at(0, 0) * (1.0 - fx) + at(1, 0) * fx;
                    let bottom = at(0, 1) * (1.0 - fx) + at(1, 1) * fx;
                    let target = top * (1.0 - fy) + bottom * fy;
                    let value = actual[((y * w + x) * 4) as usize + channel];
                    assert!((value - target).abs() < 0.001, "{w}×{h} ({x}, {y}) channel {channel}: {value} != {target}");
                }
            }
        }
    }
}

#[test]
fn chromatic_default_and_connected_controls_generate_valid_wgsl() {
    let (mut scene, assets, _) = small_scene(73, 51);
    scene
        .nodes
        .iter_mut()
        .find(|n| n.id == "blur")
        .unwrap()
        .params
        .clear();
    let definition = node_forge_render_server::schema::load_default_scheme()
        .unwrap()
        .nodes
        .remove("ChromaticBlur")
        .unwrap();
    for (port, value) in &definition.input_defaults {
        if port == "camera" {
            continue;
        }
        let (node_type, output, params) = match port.as_str() {
            "center" => ("Vector2Input", "vector", json!({"x":18.25,"y":38.25})),
            "rgbChannels" => ("Vector3Input", "vector", json!({"x":1,"y":0,"z":-1})),
            _ if value.is_boolean() => ("BoolInput", "value", json!({"value":value})),
            "radialSamples" | "rgbSamples" => ("IntInput", "value", json!({"value":value})),
            _ => ("FloatInput", "value", json!({"value":value})),
        };
        scene.nodes.push(
            serde_json::from_value(json!({"id":port,"type":node_type,"params":params})).unwrap(),
        );
        scene.connections.push(serde_json::from_value(json!({"id":format!("control.{port}"),"from":{"nodeId":port,"portId":output},"to":{"nodeId":"blur","portId":port}})).unwrap());
    }
    // A real ShaderMaterial returns vec4f; its red channel supplies coverage.
    let material = scene.nodes.iter_mut().find(|n| n.id == "rgb.mask").unwrap();
    material.node_type = "ShaderMaterial".into();
    material.params.clear();
    material.outputs.clear();
    scene.connections.iter_mut().find(|c| c.id == "rgb.mask").unwrap().from.port_id = "material".into();
    dsl::normalize_scene_defaults(&mut scene).unwrap();
    for bundle in effect_bundles(&scene, "blur", [73, 51]) {
        let module = renderer::validation::validate_wgsl(&bundle.module).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
    if let Some(gpu) = gpu() {
        assert!(
            pixels(&build(&gpu, &scene, assets))
                .iter()
                .all(|v| v.is_finite())
        );
    }
}

#[test]
fn chromatic_material_masks_control_each_stage_in_local_y_up_space() {
    let Some(gpu) = gpu() else { return };
    let (w, h) = (73, 51);
    for stage in ["radial", "rgb", "gaussian"] {
        let (mut scene, assets, expected) = small_scene(w, h);
        let blur = scene.nodes.iter_mut().find(|n| n.id == "blur").unwrap();
        blur.params.insert(format!("{stage}Enabled"), json!(true));
        blur.params.insert("center".into(), json!([w as f32 * 0.5, h as f32 * 0.5]));
        blur.params.insert("gaussianSigma".into(), json!(12));
        // Exercise the red gradient without relying on out-of-bounds taps turning black.
        blur.params.insert("rgbAngle".into(), json!(0));
        let full = pixels(&build(&gpu, &scene, assets.clone()));
        assert!(max_difference(&full, &expected) > 0.01, "{stage} must affect the source");

        let mask = scene.nodes.iter_mut().find(|n| n.id == format!("{stage}.mask")).unwrap();
        // Color masks use R, not alpha or luminance. Values outside [0,1] clamp.
        mask.params.insert("outputType".into(), json!("color"));
        mask.outputs[0].port_type = Some("color".into());
        mask.params.insert("source".into(), json!("output = vec4(uv.y > 0.5 ? 2.0 : -1.0, 0.75, 0.75, 1.0);"));
        let masked = pixels(&build(&gpu, &scene, assets));
        for y in 0..h {
            if y == h / 2 { continue; }
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                // Readback is top-down; the upper half has positive LocalUV Y.
                let target = if y < h / 2 { &full } else { &expected };
                let delta = max_difference(&masked[i..i + 4], &target[i..i + 4]);
                assert!(delta < 0.001, "{stage} ({x}, {y}): {delta}");
            }
        }
    }
}

#[test]
fn chromatic_material_masks_can_sample_an_upstream_pass() {
    let Some(gpu) = gpu() else { return };
    let (mut scene, assets, _) = small_scene(40, 24);
    let baseline = pixels(&build(&gpu, &scene, assets.clone()));
    for stage in ["radial", "rgb", "gaussian"] {
        scene.nodes.iter_mut().find(|n| n.id == "blur").unwrap()
            .params.insert(format!("{stage}Enabled"), json!(true));
    }
    let full_coverage = pixels(&build(&gpu, &scene, assets.clone()));
    assert!(max_difference(&baseline, &full_coverage) > 0.01);
    scene.nodes.extend(serde_json::from_value::<Vec<dsl::Node>>(json!([
        {"id":"mask.material", "type":"MathClosure", "params":{"source":"output = vec4(0.0, 1.0, 1.0, 1.0);", "outputType":"color"}, "outputs":[{"id":"output","name":"Output","type":"color"}]},
        {"id":"mask.geometry", "type":"Rect2DGeometry", "params":{"size":[40,24],"position":[20,12]}},
        {"id":"mask.pass", "type":"RenderPass"}
    ])).unwrap());
    scene.connections.push(serde_json::from_value(json!({
        "id":"mask.draw", "from":{"nodeId":"mask.material","portId":"output"},
        "to":{"nodeId":"mask.pass","portId":"material"}
    })).unwrap());
    scene.connections.push(serde_json::from_value(json!({
        "id":"mask.geometry", "from":{"nodeId":"mask.geometry","portId":"geometry"},
        "to":{"nodeId":"mask.pass","portId":"geometry"}
    })).unwrap());
    for stage in ["radial", "rgb", "gaussian"] {
        let mask = scene.nodes.iter_mut().find(|n| n.id == format!("{stage}.mask")).unwrap();
        mask.params.insert("source".into(), json!("output = sample_image(uv).r;"));
        mask.inputs = serde_json::from_value(json!([{"id":"image","name":"image","type":"texture"}])).unwrap();
        scene.connections.push(serde_json::from_value(json!({
            "id":format!("mask.sample.{stage}"), "from":{"nodeId":"mask.pass","portId":"texture"},
            "to":{"nodeId":format!("{stage}.mask"),"portId":"image"}
        })).unwrap());
    }
    let actual = pixels(&build(&gpu, &scene, assets.clone()));
    assert!(max_difference(&baseline, &actual) < 0.001);
    // A nonzero mask proves the sampled pass was rendered before its consumers.
    scene.nodes.iter_mut().find(|n| n.id == "mask.material").unwrap()
        .params.insert("source".into(), json!("output = vec4(1.0, 0.0, 0.0, 1.0);"));
    let actual = pixels(&build(&gpu, &scene, assets));
    assert!(max_difference(&full_coverage, &actual) < 0.001);
}

fn effect_bundles(scene: &SceneDSL, id: &str, size: [u32; 2]) -> Vec<renderer::WgslShaderBundle> {
    use renderer::wgsl_chromatic_blur::{ChromaticStage, build_chromatic_stage_bundle};
    use renderer::wgsl_gradient_blur::{
        GB_MIP_LEVELS, build_gradient_blur_composite_wgsl_bundle, gradient_blur_padded_size,
    };
    let nodes = scene
        .nodes
        .iter()
        .cloned()
        .map(|node| (node.id.clone(), node))
        .collect();
    let radial = build_chromatic_stage_bundle(
        scene,
        &nodes,
        id,
        &format!("sys.cb.{id}.input"),
        ChromaticStage::Radial,
        None,
    )
    .unwrap();
    let rgb = build_chromatic_stage_bundle(
        scene,
        &nodes,
        id,
        &format!("sys.cb.{id}.radial"),
        ChromaticStage::Rgb,
        None,
    )
    .unwrap();
    let padded = gradient_blur_padded_size(size[0], size[1]).map(|v| v as f32);
    let offset = [
        (padded[0] - size[0] as f32) * 0.5,
        (padded[1] - size[1] as f32) * 0.5,
    ];
    let mip_ids = (0..GB_MIP_LEVELS)
        .map(|i| {
            if i == 0 {
                format!("sys.gb.{id}.pad")
            } else {
                format!("sys.gb.{id}.mip{i}")
            }
        })
        .collect::<Vec<_>>();
    let gaussian =
        build_gradient_blur_composite_wgsl_bundle(scene, &nodes, id, &mip_ids, padded, offset)
            .unwrap();
    vec![radial, rgb, gaussian]
}

#[test]
fn chromatic_effect_shader_goldens() {
    let _registry = support::function_registry_lock();
    let scene = support::load_render_case_scene("blur-chromatic");
    let directory = support::render_case_dir("blur-chromatic").join("expected/chromatic");
    let update = std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v != "0");
    for (name, bundle) in ["radial", "rgb", "gaussian"]
        .into_iter()
        .zip(effect_bundles(&scene, "chromatic.blur", [1080, 1080]))
    {
        let path = directory.join(format!("{name}.wgsl"));
        if update {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(&path, &bundle.module).unwrap();
        }
        assert_eq!(
            bundle.module,
            std::fs::read_to_string(&path).expect("chromatic shader golden"),
            "{}",
            path.display()
        );
    }
}
