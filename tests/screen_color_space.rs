use node_forge_render_server::{
    color::WorkingColorSpace,
    dsl::SceneDSL,
    renderer::{ShaderSpaceBuildOptions, ShaderSpaceBuilder, ShaderSpacePresentationMode},
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig, wgpu};
use serde_json::json;

fn scene(working: WorkingColorSpace, output: &str, rgba: [f32; 4]) -> SceneDSL {
    serde_json::from_value(json!({
        "version": "6.1", "workingColorSpace": working, "dpi": 3.0,
        "metadata": { "name": "screen-color-space" },
        "nodes": [
            { "id": "color", "type": "ColorInput", "params": { "value": rgba } },
            { "id": "geometry", "type": "Rect2DGeometry", "params": {
                "size": [4, 4], "position": [2, 2], "radius": 0
            } },
            { "id": "pass", "type": "RenderPass", "params": {"blend_preset": "replace"} },
            { "id": "target", "type": "RenderTexture", "params": {
                "width": 4, "height": 4, "format": "rgba16float"
            } },
            { "id": "composite", "type": "Composite" },
            { "id": "screen", "type": "Screen", "params": {
                "outputColorSpace": output, "width": 4, "height": 4
            } }
        ],
        "connections": [
            { "id": "color", "from": {"nodeId":"color", "portId":"color"}, "to":{"nodeId":"pass", "portId":"material"} },
            { "id": "geometry", "from": {"nodeId":"geometry", "portId":"geometry"}, "to":{"nodeId":"pass", "portId":"geometry"} },
            { "id": "pass", "from": {"nodeId":"pass", "portId":"pass"}, "to":{"nodeId":"composite", "portId":"pass"} },
            { "id": "target", "from": {"nodeId":"target", "portId":"texture"}, "to":{"nodeId":"composite", "portId":"target"} },
            { "id": "screen", "from": {"nodeId":"composite", "portId":"texture"}, "to":{"nodeId":"screen", "portId":"texture"} }
        ]
    })).unwrap()
}

#[test]
fn unorm_screen_source_stays_sdr_when_primary_conversion_uses_float() {
    let gpu = HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("GPU adapter");
    if gpu.adapter.get_info().backend == wgpu::Backend::Noop {
        return;
    }
    for format in [
        "rgba8unorm", "rgba8unorm-srgb", "bgra8unorm", "bgra8unorm-srgb",
    ] {
        for (working, output, output_space, expected) in [
            (
                WorkingColorSpace::DisplayP3, "srgb", WorkingColorSpace::Srgb,
                wgpu::SurfaceColorSpace::Srgb,
            ),
            (
                WorkingColorSpace::Srgb, "display-p3", WorkingColorSpace::DisplayP3,
                wgpu::SurfaceColorSpace::DisplayP3,
            ),
        ] {
            // Fractional channels catch missing/double sRGB transfer conversion;
            // the green component still produces out-of-gamut P3 -> sRGB values.
            let rgb = [0.125, 1.0, 0.5];
            let mut scene = scene(working, output, [rgb[0], rgb[1], rgb[2], 1.0]);
            scene.nodes.iter_mut().find(|node| node.id == "target").unwrap()
                .params.insert("format".into(), json!(format));
            let build = ShaderSpaceBuilder::new(gpu.device.clone(), gpu.queue.clone())
                .with_adapter(gpu.adapter.clone())
                .with_options(ShaderSpaceBuildOptions {
                    presentation_mode: ShaderSpacePresentationMode::UiHdrNative,
                    ..Default::default()
                })
                .build(&scene)
                .expect("UNORM Screen scene");
            let source = &build.shader_space.textures[build.scene_output_texture.as_str()];
            let presented = &build.shader_space.textures[build.present_output_texture.as_str()];
            assert_eq!(presented.wgpu_texture_desc.format, wgpu::TextureFormat::Rgba16Float);
            assert_eq!(output_space.window_color_space(source.wgpu_texture_desc.format), expected);
            build.shader_space.render();
            let pixels = build.shader_space
                .read_texture_rgba16f(build.present_output_texture.as_str()).unwrap();
            let expected_rgb = working.convert(output_space, rgb);
            for channel in 0..3 {
                assert!((pixels.channels[(1 * 4 + 1) * 4 + channel] - expected_rgb[channel]).abs() < 0.005);
            }
        }
    }
}

#[test]
fn screen_gpu_converts_all_four_combinations_preserving_alpha_and_extended_rgb() {
    let gpu = HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("GPU adapter");
    if gpu.adapter.get_info().backend == wgpu::Backend::Noop {
        return;
    }
    for working in [WorkingColorSpace::Srgb, WorkingColorSpace::DisplayP3] {
        for (output, output_space) in [
            ("srgb", WorkingColorSpace::Srgb),
            ("display-p3", WorkingColorSpace::DisplayP3),
        ] {
            assert_eq!(output_space.window_color_space(wgpu::TextureFormat::Rgba16Float),
                if output_space == WorkingColorSpace::Srgb {
                    wgpu::SurfaceColorSpace::ExtendedSrgbLinear
                } else {
                    wgpu::SurfaceColorSpace::ExtendedDisplayP3
                });
            for rgba in [
                [1.0, 0.0, 0.0, 1.0],
                [-0.25, 2.0, 0.5, 0.5],
                [1.0, 1.0, 1.0, 1.0],
            ] {
                let build = ShaderSpaceBuilder::new(gpu.device.clone(), gpu.queue.clone())
                    .with_adapter(gpu.adapter.clone())
                    .with_options(ShaderSpaceBuildOptions {
                        presentation_mode: ShaderSpacePresentationMode::UiHdrNative,
                        ..Default::default()
                    })
                    .build(&scene(working, output, rgba))
                    .expect("build Screen output");
                build.shader_space.render();
                let raw = build
                    .shader_space
                    .read_texture_rgba16f(build.scene_output_texture.as_str())
                    .unwrap();
                let shown = build
                    .shader_space
                    .read_texture_rgba16f(build.present_output_texture.as_str())
                    .unwrap();
                let expected_raw = [
                    rgba[0] * rgba[3],
                    rgba[1] * rgba[3],
                    rgba[2] * rgba[3],
                    rgba[3],
                ];
                let rgb = working.convert(
                    output_space,
                    [expected_raw[0], expected_raw[1], expected_raw[2]],
                );
                let expected_output = [rgb[0], rgb[1], rgb[2], rgba[3]];
                for (pixels, expected) in [
                    (&raw.channels, expected_raw),
                    (&shown.channels, expected_output),
                ] {
                    // Interior pixel avoids geometry edge coverage.
                    for channel in 0..4 {
                        let actual = pixels[(1 * 4 + 1) * 4 + channel];
                        assert!(
                            (actual - expected[channel]).abs() < 0.003,
                            "{working:?}/{output} {rgba:?} channel {channel}: {actual} != {}",
                            expected[channel]
                        );
                    }
                }
                assert_eq!(
                    build.scene_output_texture == build.present_output_texture,
                    working == output_space
                );
            }
        }
    }
}

#[test]
fn color_settings_rebuild_the_pipeline_and_survive_scene_cache() {
    use node_forge_render_server::{
        renderer::graph_uniforms::compute_pipeline_signature,
        ws::{SceneCache, apply_scene_update, materialize_scene_dsl},
    };
    let srgb = scene(WorkingColorSpace::Srgb, "srgb", [1.0; 4]);
    let mut p3 = srgb.clone();
    p3.working_color_space = WorkingColorSpace::DisplayP3;
    assert_ne!(
        compute_pipeline_signature(&srgb),
        compute_pipeline_signature(&p3)
    );
    assert_eq!(
        materialize_scene_dsl(&SceneCache::from_scene_update(&p3)).working_color_space,
        WorkingColorSpace::DisplayP3
    );
    let mut cache = SceneCache::from_scene_update(&srgb);
    apply_scene_update(&mut cache, &p3);
    assert_eq!(materialize_scene_dsl(&cache).working_color_space, WorkingColorSpace::DisplayP3);
    let signature = compute_pipeline_signature(&p3);
    p3.nodes
        .iter_mut()
        .find(|n| n.node_type == "Screen")
        .unwrap()
        .params
        .insert("outputColorSpace".into(), json!("display-p3"));
    assert_ne!(signature, compute_pipeline_signature(&p3));
}

#[test]
fn source_tagged_hex_converts_once_while_numeric_colors_remain_in_working_space() {
    use node_forge_render_server::dsl::normalize_scene_defaults;

    let mut authored = scene(WorkingColorSpace::DisplayP3, "display-p3", [1.0; 4]);
    authored.nodes[0]
        .params
        .insert("value".into(), json!("#ff000080"));
    normalize_scene_defaults(&mut authored).unwrap();
    let converted = authored.nodes[0].params["value"].clone();
    let rgba = converted.as_array().unwrap();
    for (actual, expected) in rgba
        .iter()
        .zip([0.82246196, 0.0331942, 0.01708263, 128.0 / 255.0])
    {
        assert!((actual.as_f64().unwrap() - expected).abs() < 1e-6);
    }
    normalize_scene_defaults(&mut authored).unwrap();
    assert_eq!(authored.nodes[0].params["value"], converted);

    let numeric = json!([-0.25, 2.0, 0.5, 0.5]);
    authored.nodes[0]
        .params
        .insert("value".into(), numeric.clone());
    normalize_scene_defaults(&mut authored).unwrap();
    assert_eq!(authored.nodes[0].params["value"], numeric);
}
