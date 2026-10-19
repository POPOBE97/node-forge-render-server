use node_forge_render_server::{
    asset_store::{AssetData, AssetStore},
    dsl::SceneDSL,
    renderer::ShaderSpaceBuilder,
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
use serde_json::json;

fn assets() -> AssetStore {
    let store = AssetStore::new();
    for (id, bytes) in [
        (
            "latin",
            include_bytes!("../assets/fonts/GeistMono/GeistMono-Regular.ttf").as_slice(),
        ),
        (
            "cjk",
            include_bytes!("../assets/fonts/MiSans/MiSans-Regular.ttf").as_slice(),
        ),
    ] {
        store.insert(
            id,
            AssetData {
                bytes: bytes.to_vec(),
                mime_type: "font/ttf".into(),
                original_name: format!("{id}.ttf"),
            },
        );
    }
    store
}
fn scene(width: u32, height: u32) -> SceneDSL {
    serde_json::from_value(json!({"version":"6.0", "workingColorSpace": "linear-srgb", "metadata":{"name":"text-pass-test"},
        "nodes":[
            {"id":"font","type":"FontFile","params":{"assetId":"latin"}},
            {"id":"fallback","type":"FontFile","params":{"assetId":"cjk"}},
            {"id":"text","type":"TextPass","params":{
                "text":"AV 中文 fi gyp", "fontSize":19.3333, "size":[230,50], "position":[130,60],
                "color":[0.8,0.4,0.2,0.5], "alpha":0.5, "blend_preset":"premul_alpha"
            }},
            {"id":"target","type":"RenderTexture","params":{"width":width,"height":height,"format":"rgba16float"}},
            {"id":"composite","type":"Composite","params":{"blend_preset":"premul_alpha"}},
            {"id":"screen","type":"Screen","params":{ "outputColorSpace": "srgb","width":width,"height":height}}
        ],"connections":[
            {"id":"font","from":{"nodeId":"font","portId":"font"},"to":{"nodeId":"text","portId":"font"}},
            {"id":"fallback","from":{"nodeId":"fallback","portId":"font"},"to":{"nodeId":"text","portId":"fallbackFont"}},
            {"id":"text","from":{"nodeId":"text","portId":"pass"},"to":{"nodeId":"composite","portId":"pass"}},
            {"id":"target","from":{"nodeId":"target","portId":"texture"},"to":{"nodeId":"composite","portId":"target"}},
            {"id":"screen","from":{"nodeId":"composite","portId":"texture"},"to":{"nodeId":"screen","portId":"texture"}}
        ],"outputs":null})).unwrap()
}

#[test]
fn native_text_preserves_pixels_premultiplication_and_dual_output_domains() {
    let renderer =
        HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("native GPU required");
    let build = |scene: &SceneDSL| {
        ShaderSpaceBuilder::new(renderer.device.clone(), renderer.queue.clone())
            .with_adapter(renderer.adapter.clone())
            .with_asset_store(assets())
            .build(scene)
            .unwrap()
    };
    let small = build(&scene(300, 120));
    small.shader_space.render();
    let pixels = small
        .shader_space
        .read_texture_rgba16f("target")
        .unwrap()
        .channels;
    let visible: Vec<_> = pixels.chunks_exact(4).filter(|p| p[3] > 0.01).collect();
    assert!(visible.len() > 300, "text must render visible glyphs");
    for p in &visible {
        assert!(p[3] <= 0.251, "coverage × color alpha × pass alpha");
        assert!(
            (p[0] - p[3] * 0.8).abs() < 0.001,
            "premultiply exactly once"
        );
        assert!((p[1] - p[3] * 0.4).abs() < 0.001);
    }
    let large = build(&scene(600, 240));
    large.shader_space.render();
    let large_pixels = large
        .shader_space
        .read_texture_rgba16f("target")
        .unwrap()
        .channels;
    // Readback rows are top-down; compare the same bottom-left physical pixels.
    for y in 0..120 {
        for x in 0..300 {
            for c in 0..4 {
                let a = pixels[((119 - y) * 300 + x) * 4 + c];
                let b = large_pixels[((239 - y) * 600 + x) * 4 + c];
                assert!(
                    (a - b).abs() < 0.002,
                    "resolution changed physical text at {x},{y}: {a} != {b}"
                );
            }
        }
    }
    let mut dual = scene(300, 120);
    dual.nodes
        .iter_mut()
        .find(|n| n.id == "composite")
        .unwrap()
        .inputs = serde_json::from_value(json!([
        {"id":"pass","type":"pass"},{"id":"dynamic_1","type":"pass"}
    ]))
    .unwrap();
    dual.nodes.push(serde_json::from_value(json!({"id":"localDraw","type":"ImagePass"})).unwrap());
    dual.connections.push(serde_json::from_value(json!({"id":"localText",
        "from":{"nodeId":"text","portId":"texture"},"to":{"nodeId":"localDraw","portId":"image"}})).unwrap());
    dual.connections.push(serde_json::from_value(json!({"id":"localLayer",
        "from":{"nodeId":"localDraw","portId":"pass"},"to":{"nodeId":"composite","portId":"dynamic_1"}})).unwrap());
    let dual_build = build(&dual);
    dual_build.shader_space.render();
    let local = dual_build
        .shader_space
        .read_texture_rgba16f("sys.text.sys.route.texture.text.out")
        .unwrap();
    assert_eq!([local.width, local.height], [230, 50]);
    assert!(
        local
            .channels
            .chunks_exact(4)
            .filter(|p| p[3] > 0.01)
            .count()
            > 300
    );

    let mut formatted = scene(300, 120);
    formatted.nodes.push(
        serde_json::from_value(json!({"id":"format","type":"StringFormat",
        "params":{"format":"Width {0}","value0":300}}))
        .unwrap(),
    );
    formatted.connections.push(
        serde_json::from_value(json!({"id":"formatted",
        "from":{"nodeId":"format","portId":"text"},"to":{"nodeId":"text","portId":"text"}}))
        .unwrap(),
    );
    let formatted_build = build(&formatted);
    formatted_build.shader_space.render();
    let formatted_pixels = formatted_build
        .shader_space
        .read_texture_rgba16f("target")
        .unwrap()
        .channels;
    let mut literal = scene(300, 120);
    literal
        .nodes
        .iter_mut()
        .find(|n| n.id == "text")
        .unwrap()
        .params
        .insert("text".into(), json!("Width 300"));
    let literal_build = build(&literal);
    literal_build.shader_space.render();
    assert_eq!(
        formatted_pixels,
        literal_build
            .shader_space
            .read_texture_rgba16f("target")
            .unwrap()
            .channels,
        "connected CPU string must override the inline text"
    );
    formatted
        .nodes
        .iter_mut()
        .find(|n| n.id == "format")
        .unwrap()
        .params
        .insert("value0".into(), json!(600));
    let changed = build(&formatted);
    changed.shader_space.render();
    assert_ne!(
        formatted_pixels,
        changed
            .shader_space
            .read_texture_rgba16f("target")
            .unwrap()
            .channels,
        "numeric edits must regenerate the formatted text"
    );

    let mut shared = scene(300, 120);
    shared
        .nodes
        .iter_mut()
        .find(|n| n.id == "composite")
        .unwrap()
        .inputs = serde_json::from_value(
        json!([{"id":"pass","type":"pass"},{"id":"dynamic_1","type":"pass"}]),
    )
    .unwrap();
    shared.nodes.push(
        serde_json::from_value(json!({"id":"sample","type":"MathClosure",
        "params":{"source":"output = samplePass(source, uv) * 0.0;"},
        "inputs":[{"id":"source","name":"source","type":"texture"}],
        "outputs":[{"id":"output","type":"color"}]}))
        .unwrap(),
    );
    for (id, from, port, to, input) in [
        ("sampleText", "text", "texture", "sample", "source"),
        ("sampleLayer", "sample", "output", "composite", "dynamic_1"),
    ] {
        shared.connections.push(
            serde_json::from_value(json!({"id":id,"from":{"nodeId":from,"portId":port},
        "to":{"nodeId":to,"portId":input}}))
            .unwrap(),
        );
    }
    let shared_build = build(&shared);
    shared_build.shader_space.render();
    let shared_pixels = shared_build.shader_space.read_texture_rgba16f("target").unwrap().channels;
    let delta = pixels.iter().zip(&shared_pixels).map(|(a,b)|(a-b).abs()).fold(0.0_f32,f32::max);
    let visible = shared_pixels.chunks_exact(4).filter(|p|p[3]>0.01).count();
    assert!(delta < 0.002, "sampling the explicit texture must preserve the independent draw: max delta {delta}, visible {visible}");
}

#[test]
fn data_parse_layout_matches_inline_text_after_dimension_edits() {
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default()).unwrap();
    let build = |scene: &SceneDSL| {
        ShaderSpaceBuilder::new(renderer.device.clone(), renderer.queue.clone())
            .with_adapter(renderer.adapter.clone())
            .with_asset_store(assets())
            .build(scene)
    };
    let mut connected = scene(300, 120);
    for node in [
        json!({"id":"width","type":"IntInput","params":{"value":300}}),
        json!({"id":"height","type":"IntInput","params":{"value":120}}),
        json!({"id":"layout","type":"DataParse",
            "params":{"source":"return { position: [width * 0.5, height * 0.5] as vec2, size: [width - 40, 50] as vec2 };"},
            "inputs":[{"id":"w","type":"float"},{"id":"h","type":"float"}],
            "inputBindings":[{"portId":"w","variableName":"width","type":"float"},
                {"portId":"h","variableName":"height","type":"float"}],
            "outputs":[{"id":"position","type":"vector2"},{"id":"size","type":"vector2"}]})
    ] {
        connected.nodes.push(serde_json::from_value(node).unwrap());
    }
    for (id, from, output, to, input) in [
        ("w", "width", "value", "layout", "w"),
        ("h", "height", "value", "layout", "h"),
        ("position", "layout", "position", "text", "position"),
        ("size", "layout", "size", "text", "size"),
    ] {
        connected.connections.push(serde_json::from_value(json!({"id":id,
            "from":{"nodeId":from,"portId":output},"to":{"nodeId":to,"portId":input}})).unwrap());
    }
    for (width, height) in [(300, 120), (600, 240), (317, 131)] {
        for (id, value) in [("width", width), ("height", height)] {
            connected.nodes.iter_mut().find(|node| node.id == id).unwrap()
                .params.insert("value".into(), json!(value));
        }
        for id in ["target", "screen"] {
            let node = connected.nodes.iter_mut().find(|node| node.id == id).unwrap();
            node.params.insert("width".into(), json!(width));
            node.params.insert("height".into(), json!(height));
        }
        let mut literal = scene(width, height);
        let text = literal.nodes.iter_mut().find(|node| node.id == "text").unwrap();
        text.params.insert("position".into(), json!([width as f64 * 0.5, height as f64 * 0.5]));
        text.params.insert("size".into(), json!([width - 40, 50]));
        let actual = build(&connected).unwrap();
        actual.shader_space.render();
        let expected = build(&literal).unwrap();
        expected.shader_space.render();
        assert_eq!(actual.shader_space.read_texture_rgba16f("target").unwrap().channels,
            expected.shader_space.read_texture_rgba16f("target").unwrap().channels,
            "DataParse layout must match inline physical pixels at {width}x{height}");
    }
    connected.nodes.iter_mut().find(|node| node.id == "layout").unwrap().params
        .insert("source".into(), json!("return { position: [1, 2, 3], size: [200, 50] };"));
    let error = match build(&connected) { Ok(_) => panic!("invalid layout accepted"), Err(error) => error };
    assert!(format!("{error:#}").contains("exactly two components"));
}
