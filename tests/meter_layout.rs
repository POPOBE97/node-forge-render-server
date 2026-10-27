use node_forge_render_server::{asset_store, dsl, renderer};
use renderer::types::BakedValue;
use serde_json::json;

#[test]
fn meter_layout_keeps_pixel_extent_and_dp_offsets() {
    for example in ["practice-gaudge-meter", "practice-bauhaus-meter"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/render/editor-examples")
            .join(example)
            .join("scene.nforge");
        let (original, assets) = asset_store::load_from_nforge(&path).unwrap();
        for dpi in [1.0, 2.5, 3.0, 4.0] {
            for extent in [720.0, 1080.0] {
                let mut scene = original.clone();
                scene.dpi = dpi;
                scene.nodes.iter_mut().find(|node| node.id == "gauge.layout")
                    .unwrap().params.insert("extent".into(), json!(extent));
                let prepared = renderer::prepare_scene(&scene).unwrap();
                assert_eq!(prepared.resolution, [extent as u32; 2]);
                for target in prepared.scene.nodes.iter().filter(|node| node.node_type == "RenderTexture") {
                    assert_eq!(
                        dsl::render_texture_size(&prepared.scene, &prepared.nodes_by_id, target).unwrap(),
                        [extent as u32; 2],
                        "{example}: {} at dpi={dpi}", target.id,
                    );
                }
                let half = extent as f32 * 0.5;
                let scale = dpi as f32;
                let expected = [
                    ("extent", vec![extent as f32]),
                    ("size", vec![extent as f32; 2]),
                    ("center", vec![half; 2]),
                    ("label50", vec![half - 99.0 * scale, half + 53.0 * scale, 0.0]),
                    ("label10", vec![half + 80.0 * scale, half - 49.5 * scale, 0.0]),
                    ("dialCenter", vec![half + 346.0 * scale, half - 205.5 * scale]),
                ];
                for (port, expected) in expected {
                    let values = &prepared.baked_data_parse[
                        &("__global".into(), "gauge.layout".into(), port.into())
                    ];
                    assert_eq!(values.len(), 1);
                    let actual = match &values[0] {
                        BakedValue::F32(value) => vec![*value],
                        BakedValue::Vec2(value) => value.to_vec(),
                        BakedValue::Vec3(value) => value.to_vec(),
                        value => panic!("Unexpected layout type: {value:?}"),
                    };
                    assert_eq!(actual, expected, "{example}.{port}: dpi={dpi}, extent={extent}");
                }
            }
            // Exercise both the shared CPU outputs and direct Index -> MathClosure coercion.
            if dpi == 1.0 || dpi == 3.0 {
                let mut scene = original.clone();
                scene.dpi = dpi;
                let bundles = renderer::build_all_pass_wgsl_bundles_from_scene_with_assets(
                    &scene, Some(&assets),
                ).unwrap();
                assert!(!bundles.is_empty());
                for (pass, bundle) in bundles {
                    renderer::validation::validate_wgsl_with_context(
                        &bundle.module, &format!("{example}:{pass}:dpi={dpi}"),
                    ).unwrap();
                }
            }
        }
    }
}
