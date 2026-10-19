use node_forge_render_server::{
    color::WorkingColorSpace,
    dsl::SceneDSL,
    renderer::{ShaderSpaceBuilder, prepare_scene},
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
use serde_json::json;

mod support;

fn scene(width: u32, height: u32, working: WorkingColorSpace) -> SceneDSL {
    serde_json::from_value(json!({
        "version": "6.0", "workingColorSpace": working,
        "metadata": { "name": "procedural-test-texture" },
        "nodes": [
            { "id": "chart", "type": "ProceduralTestTexture", "params": { "width": width, "height": height } },
            { "id": "image", "type": "ImagePass" },
            { "id": "target", "type": "RenderTexture", "params": { "width": width, "height": height, "format": "rgba16float" } },
            { "id": "composite", "type": "Composite" },
            { "id": "screen", "type": "Screen", "params": { "width": width, "height": height, "outputColorSpace": "display-p3" } }
        ],
        "connections": [
            { "id": "image", "from": { "nodeId": "chart", "portId": "texture" }, "to": { "nodeId": "image", "portId": "image" } },
            { "id": "layer", "from": { "nodeId": "image", "portId": "pass" }, "to": { "nodeId": "composite", "portId": "pass" } },
            { "id": "target", "from": { "nodeId": "target", "portId": "texture" }, "to": { "nodeId": "composite", "portId": "target" } },
            { "id": "screen", "from": { "nodeId": "composite", "portId": "texture" }, "to": { "nodeId": "screen", "portId": "texture" } }
        ]
    })).unwrap()
}

#[test]
fn procedural_test_texture_has_an_independent_size_and_accepts_connected_dimensions() {
    let mut input = scene(1260, 2800, WorkingColorSpace::DisplayP3);
    input.nodes.push(
        serde_json::from_value(json!({"id":"width", "type":"IntInput", "params":{"value":1080}}))
            .unwrap(),
    );
    input.connections.push(serde_json::from_value(json!({"id":"width", "from":{"nodeId":"width","portId":"value"}, "to":{"nodeId":"chart","portId":"width"}})).unwrap());
    let prepared = prepare_scene(&input).unwrap();
    assert_eq!(
        prepared.resolution,
        [1260, 2800],
        "source size must not resize its consumer"
    );
    let source_target = &prepared.nodes_by_id["sys.procedural-test.chart.chart.target"];
    let width = node_forge_render_server::renderer::utils::cpu_num_u32_min_1(
        &prepared.scene,
        &prepared.nodes_by_id,
        source_target,
        "width",
        1,
    )
    .unwrap();
    assert_eq!(width, 1080);
    assert_eq!(
        prepared
            .scene
            .nodes
            .iter()
            .filter(|node| node.node_type == "TextPass")
            .count(),
        18
    );
    assert!(
        prepared
            .scene
            .nodes
            .iter()
            .all(|node| node.node_type != "ProceduralTestTexture")
    );
    assert_eq!(
        input.nodes[0].node_type, "ProceduralTestTexture",
        "expansion must not alter the document"
    );
    for width in [0.0, -1.0, 1.5, 8193.0] {
        let mut invalid = scene(1260, 2800, WorkingColorSpace::DisplayP3);
        invalid.nodes[0].params.insert("width".into(), json!(width));
        assert!(
            prepare_scene(&invalid)
                .unwrap_err()
                .to_string()
                .contains("integer in [1, 8192]")
        );
    }
}

#[test]
fn procedural_test_texture_matches_the_chart_in_each_working_color_space() {
    let gpu =
        HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("native GPU required");
    let (reference, assets) = support::load_render_case("procedural-rendering-test");
    for [width, height] in [[1260, 2800], [1279, 2797]] {
        let mut reference = reference.clone();
        for (id, value) in [("chart.width", width), ("chart.height", height)] {
            reference
                .nodes
                .iter_mut()
                .find(|node| node.id == id)
                .unwrap()
                .params
                .insert("value".into(), json!(value));
        }
        for working in [WorkingColorSpace::DisplayP3, WorkingColorSpace::Srgb] {
            // Working-space changes reinterpret the chart's numeric RGB, just as
            // they do for the authored graph. They must not preserve P3 appearance.
            reference.working_color_space = working;
            let reference_build = ShaderSpaceBuilder::new(gpu.device.clone(), gpu.queue.clone())
                .with_adapter(gpu.adapter.clone())
                .with_asset_store(assets.clone())
                .build(&reference)
                .unwrap();
            reference_build.shader_space.render();
            let expected = reference_build
                .shader_space
                .read_texture_rgba16f("chart.target")
                .unwrap();
            // No document assets or shader overrides are needed for the new node.
            let build = ShaderSpaceBuilder::new(gpu.device.clone(), gpu.queue.clone())
                .with_adapter(gpu.adapter.clone())
                .build(&scene(width, height, working))
                .unwrap();
            build.shader_space.render();
            let actual = build.shader_space.read_texture_rgba16f("target").unwrap();
            assert_eq!([actual.width, actual.height], [width, height]);
            assert!(
                actual.channels.iter().any(|value| *value > 1.4),
                "HDR test patches must survive"
            );
            let mut maximum_error = 0.0_f32;
            for (a, b) in actual
                .channels
                .chunks_exact(4)
                .zip(expected.channels.chunks_exact(4))
            {
                for channel in 0..3 {
                    maximum_error = maximum_error.max((a[channel] - b[channel]).abs());
                }
                assert!((a[3] - b[3]).abs() < 0.001);
            }
            assert!(
                maximum_error < 0.003,
                "{width}x{height} {working:?}: maximum pixel error {maximum_error}"
            );
        }
    }
}
