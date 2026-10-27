use image::ImageEncoder;
use node_forge_render_server::{
    asset_store::{AssetData, AssetStore},
    dsl::SceneDSL,
    renderer::{ShaderSpaceBuildOptions, ShaderSpaceBuilder, ShaderSpacePresentationMode},
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig, wgpu};
use serde_json::json;

fn texture_scene(producer: &str, alias: bool) -> SceneDSL {
    let mut value = json!({
        "version": "6.1", "workingColorSpace": "linear-srgb", "dpi": 3.0,
        "metadata": { "name": "texture-owned output" },
        "nodes": [
            { "id": "color", "type": "ColorInput", "params": { "value": [0.5, 0.25, 0.125, 1] } },
            { "id": "geometry", "type": "Rect2DGeometry", "params": { "size": [20, 12], "position": [100, 200] } },
            { "id": "draw", "type": "RenderPass" },
            { "id": "target", "type": "RenderTexture", "params": { "width": 30, "height": 24, "format": "rgba16float" } },
            { "id": "composite", "type": "Composite" },
            { "id": "blur", "type": "GuassianBlurPass", "params": { "radius": 0 } },
            { "id": "sampler", "type": "TextureSampler" },
            { "id": "image", "type": "ImageTexture", "params": { "assetId": "pixels", "encoderSpace": "linear", "alphaMode": "premultiplied" } },
            { "id": "screen", "type": "Screen", "params": { "width": 777, "height": 999, "outputColorSpace": "srgb" } }
        ],
        "connections": [
            { "id": "material", "from": { "nodeId": "color", "portId": "color" }, "to": { "nodeId": "draw", "portId": "material" } },
            { "id": "geometry", "from": { "nodeId": "geometry", "portId": "geometry" }, "to": { "nodeId": "draw", "portId": "geometry" } }
        ]
    });
    let edges = value["connections"].as_array_mut().unwrap();
    let mut connect = |id: &str, source: &str, port: &str, dest: &str, input: &str| {
        edges.push(json!({ "id": id, "from": { "nodeId": source, "portId": port }, "to": { "nodeId": dest, "portId": input } }));
    };
    if producer == "composite" {
        connect("layer", "draw", "pass", "composite", "pass");
        connect("target", "target", "texture", "composite", "target");
    }
    if producer == "blur" {
        connect("blur", "draw", "texture", "blur", "texture");
    }
    if alias {
        connect("alias", producer, "texture", "sampler", "texture");
    }
    connect("output", if alias { "sampler" } else { producer }, "texture", "screen", "texture");
    serde_json::from_value(value).unwrap()
}

fn image_assets() -> AssetStore {
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&[64, 128, 192, 255].repeat(8), 4, 2, image::ExtendedColorType::Rgba8)
        .unwrap();
    let assets = AssetStore::new();
    assets.insert("pixels", AssetData {
        bytes: png, mime_type: "image/png".into(), original_name: "pixels.png".into(),
    });
    assets
}

#[test]
fn direct_texture_outputs_and_aliases_use_producer_dimensions() {
    let gpu = HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("GPU adapter");
    if gpu.adapter.get_info().backend == wgpu::Backend::Noop { return; }
    for producer in ["draw", "blur", "composite", "image"] {
        for alias in [false, true] {
            let scene = texture_scene(producer, alias);
            let build = ShaderSpaceBuilder::new(gpu.device.clone(), gpu.queue.clone())
                .with_adapter(gpu.adapter.clone()).with_asset_store(image_assets())
                .build(&scene).unwrap();
            let expected = match producer { "image" => [4, 2], "composite" => [30, 24], _ => [20, 12] };
            assert_eq!(build.resolution, expected, "{producer}, alias={alias}");
            build.shader_space.render();
            if producer == "image" {
                let pixels = build.shader_space.read_texture_rgba8(build.scene_output_texture.as_str()).unwrap();
                assert_eq!(pixels.bytes, [64, 128, 192, 255].repeat(8));
            } else if producer != "composite" {
                let pixels = build.shader_space.read_texture_rgba16f(build.scene_output_texture.as_str()).unwrap();
                for pixel in pixels.channels.chunks_exact(4) {
                    for (actual, expected) in pixel.iter().zip([0.5, 0.25, 0.125, 1.0]) {
                        assert!((actual - expected).abs() < 0.001, "{producer}: {pixel:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn direct_image_output_prepares_alpha_and_color_before_presentation() {
    let gpu = HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("GPU adapter");
    if gpu.adapter.get_info().backend == wgpu::Backend::Noop { return; }
    for mode in [ShaderSpacePresentationMode::SceneLinear, ShaderSpacePresentationMode::UiSdrDisplayEncode, ShaderSpacePresentationMode::UiHdrNative] {
        let mut scene = texture_scene("image", true);
        let image = scene.nodes.iter_mut().find(|node| node.id == "image").unwrap();
        image.params.insert("encoderSpace".into(), json!("srgb"));
        image.params.insert("alphaMode".into(), json!("straight"));
        let build = ShaderSpaceBuilder::new(gpu.device.clone(), gpu.queue.clone())
            .with_adapter(gpu.adapter.clone()).with_asset_store(image_assets())
            .with_options(ShaderSpaceBuildOptions { presentation_mode: mode, ..Default::default() })
            .build(&scene).unwrap();
        assert_eq!(build.resolution, [4, 2]);
        build.shader_space.render();
        let pixels = build.shader_space.read_texture_rgba32f(build.scene_output_texture.as_str()).unwrap();
        for pixel in pixels.channels.chunks_exact(4) {
            for (value, code) in pixel[..3].iter().zip([64.0_f32, 128.0, 192.0]) {
                let expected = ((code / 255.0 + 0.055) / 1.055).powf(2.4);
                assert!((value - expected).abs() < 0.001);
            }
            assert_eq!(pixel[3], 1.0);
        }
    }
}

#[test]
fn file_accepts_texture_directly_and_rejects_draw_output() {
    let mut scene = texture_scene("draw", false);
    scene.nodes.iter_mut().find(|node| node.id == "screen").unwrap().node_type = "File".into();
    node_forge_render_server::renderer::scene_prep::prepare_scene(&scene).unwrap();
    let output = scene.connections.iter_mut().find(|edge| edge.id == "output").unwrap();
    output.from.port_id = "pass".into();
    let error = node_forge_render_server::renderer::scene_prep::prepare_scene(&scene).unwrap_err();
    assert!(error.to_string().contains("type mismatch"));
}

#[test]
fn file_exports_a_prepared_image_texture_without_an_authored_target() {
    let mut scene = texture_scene("image", true);
    scene.nodes.iter_mut().find(|node| node.id == "screen").unwrap().node_type = "File".into();
    scene.nodes.iter_mut().find(|node| node.id == "image").unwrap()
        .params.insert("alphaMode".into(), json!("straight"));
    let path = std::env::temp_dir().join(format!("nforge-texture-output-{}.exr", std::process::id()));
    node_forge_render_server::renderer::render_scene_to_file_headless(&scene, &path, Some(&image_assets())).unwrap();
    let image = image::open(&path).unwrap();
    assert_eq!([image.width(), image.height()], [4, 2]);
    assert_eq!(image.color(), image::ColorType::Rgba32F);
    std::fs::remove_file(path).unwrap();
}
