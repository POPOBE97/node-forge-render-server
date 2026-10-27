use node_forge_render_server::{dsl::SceneDSL, renderer::ShaderSpaceBuilder};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig, wgpu};
use serde_json::json;

#[test]
fn voice_derivation_keeps_dp_vectors_in_the_light_consumer_space() {
    use node_forge_render_server::{animation::AnimationSession, nforge};
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/render/editor-examples/doubao-voice-interaction/scene.nforge");
    let loaded = nforge::load(&path).unwrap();
    let key = node_forge_render_server::state_machine::OverrideKey::new(
        "Vector2ArrayInput_IntelligentLightPositions",
        "value",
    );
    for dpi in [1.0, 2.5, 4.0] {
        let mut scene = loaded.scene.clone();
        scene.dpi = dpi;
        let mut session = AnimationSession::from_scene(&scene).unwrap().unwrap();
        for _ in 0..60 {
            let step = session.step(1.0 / 60.0);
            assert!(step.diagnostics.is_empty(), "{:?}", step.diagnostics);
            let positions = step.active_overrides[&key]
                .as_array()
                .expect("packed vector2 positions");
            assert_eq!(positions.len(), 11);
            for position in positions {
                let xy = position.as_array().unwrap();
                assert_eq!(xy.len(), 2);
                for value in xy {
                    let value = value.as_f64().unwrap();
                    // A missing vector-component edge used to produce a zero canvas,
                    // then divide by epsilon and send billion-pixel coordinates to IL.
                    assert!(
                        value.is_finite() && value.abs() < 1000.0,
                        "dpi={dpi}: {position}"
                    );
                }
            }
        }
    }
}

#[test]
fn dp_content_and_explicit_target_units_render_at_the_expected_physical_size() {
    let gpu = HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("GPU adapter");
    if gpu.adapter.get_info().backend == wgpu::Backend::Noop {
        eprintln!("DP pixel matrix requires a rendering GPU backend");
        return;
    }
    for dpi in [1.0, 2.0, 3.0, 4.0] {
        for target_dp in [false, true] {
            let extent = if target_dp {
                (360.0 * dpi) as u32
            } else {
                1080
            };
            let target_value = if target_dp { 360 } else { 1080 };
            let scene: SceneDSL = serde_json::from_value(json!({
                "version":"6.1", "dpi":dpi, "workingColorSpace":"linear-srgb",
                "metadata":{"name":"DP physical pixel matrix"},
                "nodes":[
                    {"id":"target","type":"RenderTexture","params":{"width":999,"height":target_value,"inputUnits":{"height":if target_dp {"dp"} else {"px"}},"format":"rgba8unorm"}},
                    {"id":"width","type":"FloatInput","params":{"value":target_value,"inputUnits":{"value":if target_dp {"dp"} else {"px"}}}},
                    {"id":"length","type":"FloatInput","params":{"value":100,"inputUnits":{"value":"dp"}}},
                    {"id":"size","type":"Vector2Input","params":{"x":999,"y":100,"inputUnits":{"x":"dp","y":"dp"}}},
                    {"id":"geometry","type":"Rect2DGeometry","params":{"size":[1,1],"position":[extent/2,extent/2]}},
                    {"id":"draw","type":"RenderPass"},
                    {"id":"color","type":"ColorInput","params":{"value":[1,0,0,1]}},
                    {"id":"composite","type":"Composite"},
                    {"id":"screen","type":"Screen","params":{"outputColorSpace":"srgb"}}
                ],
                "connections":[
                    {"id":"width","from":{"nodeId":"width","portId":"value"},"to":{"nodeId":"target","portId":"width"}},
                    {"id":"length","from":{"nodeId":"length","portId":"value"},"to":{"nodeId":"size","portId":"x"}},
                    {"id":"size","from":{"nodeId":"size","portId":"vector"},"to":{"nodeId":"geometry","portId":"size"}},
                    {"id":"geometry","from":{"nodeId":"geometry","portId":"geometry"},"to":{"nodeId":"draw","portId":"geometry"}},
                    {"id":"color","from":{"nodeId":"color","portId":"color"},"to":{"nodeId":"draw","portId":"material"}},
                    {"id":"draw","from":{"nodeId":"draw","portId":"pass"},"to":{"nodeId":"composite","portId":"pass"}},
                    {"id":"target","from":{"nodeId":"target","portId":"texture"},"to":{"nodeId":"composite","portId":"target"}},
                    {"id":"screen","from":{"nodeId":"composite","portId":"texture"},"to":{"nodeId":"screen","portId":"texture"}}
                ]
            })).unwrap();
            let build = ShaderSpaceBuilder::new(gpu.device.clone(), gpu.queue.clone())
                .with_adapter(gpu.adapter.clone())
                .build(&scene)
                .unwrap();
            assert_eq!(build.resolution, [extent, extent]);
            build.shader_space.render();
            let pixels = build
                .shader_space
                .read_texture_rgba8(build.scene_output_texture.as_str())
                .unwrap();
            let length = (100.0 * dpi) as u32;
            let start = (extent - length) / 2;
            let end = start + length;
            for (index, pixel) in pixels.bytes.chunks_exact(4).enumerate() {
                let x = index as u32 % extent;
                let y = index as u32 / extent;
                let inside = (start..end).contains(&x) && (start..end).contains(&y);
                assert_eq!(
                    pixel[0] > 250,
                    inside,
                    "dpi={dpi}, target_dp={target_dp}, ({x},{y}): {pixel:?}"
                );
                assert_eq!(pixel[1], 0);
                assert_eq!(pixel[2], 0);
            }
        }
    }
}

#[test]
fn canonical_examples_prepare_inline_units_and_validate_wgsl_without_gpu() {
    use node_forge_render_server::{asset_store, renderer};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/render/editor-examples");
    let mut archives: Vec<_> = std::fs::read_dir(root).unwrap()
        .map(|entry| entry.unwrap().path().join("scene.nforge"))
        .filter(|path| path.exists()).collect();
    archives.sort();
    assert_eq!(archives.len(), 47);
    for path in archives {
        let (scene, assets) = asset_store::load_from_nforge(&path).unwrap();
        let bundles = renderer::build_all_pass_wgsl_bundles_from_scene_with_assets(&scene, Some(&assets))
            .unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
        for (pass, bundle) in bundles {
            renderer::validation::validate_wgsl_with_context(&bundle.module, &format!("{}:{pass}", path.display()))
                .unwrap_or_else(|error| panic!("{}:{pass}: {error:#}", path.display()));
        }
    }
}
