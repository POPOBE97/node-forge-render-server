use image::ImageEncoder;
use node_forge_render_server::{
    asset_store::{AssetData, AssetStore},
    dsl::SceneDSL,
    color::WorkingColorSpace,
    renderer::ShaderSpaceBuilder,
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig, eframe::wgpu};
use serde_json::json;

const PIXELS: [[u8; 4]; 8] = [
    [255, 0, 0, 255],
    [0, 255, 0, 255],
    [0, 0, 255, 255],
    [128, 128, 128, 255],
    [188, 106, 68, 128],
    [255, 0, 0, 128],
    [0, 0, 0, 0],
    [255, 255, 255, 255],
];

fn scene(encoding: &str, alpha: &str, output: &str) -> SceneDSL {
    let mut value = json!({
        "version": "6.0", "workingColorSpace": "linear-srgb",
        "metadata": { "name": "image-texture-color-space" },
        "nodes": [
            { "id": "image", "type": "ImageTexture", "params": {
                "assetId": "pixels", "encoderSpace": encoding, "alphaMode": alpha,
                "aspectCorrection": "off", "magFilter": "nearest", "minFilter": "nearest"
            } },
            { "id": "target", "type": "RenderTexture", "params": {
                "width": 4, "height": 2, "format": "rgba16float"
            } },
            { "id": "draw", "type": "ImagePass", "params": { "loadOp": "none" } },
            { "id": "composite", "type": "Composite" },
            { "id": "screen", "type": "Screen", "params": { "outputColorSpace": "srgb", "width": 4, "height": 2 } }
        ],
        "connections": [
            { "id": "image-pass", "from": { "nodeId": "image", "portId": output },
                "to": { "nodeId": if output == "texture" { "draw" } else { "composite" },
                    "portId": if output == "texture" { "image" } else { "pass" } } },
            { "id": "draw", "from": { "nodeId": "draw", "portId": "pass" },
                "to": { "nodeId": "composite", "portId": "pass" } },
            { "id": "target", "from": { "nodeId": "target", "portId": "texture" },
                "to": { "nodeId": "composite", "portId": "target" } },
            { "id": "screen", "from": { "nodeId": "composite", "portId": "texture" },
                "to": { "nodeId": "screen", "portId": "texture" } }
        ],
        "outputs": null
    });
    if output != "texture" {
        value["connections"].as_array_mut().unwrap().retain(|edge| edge["id"] != "draw");
    }
    serde_json::from_value(value)
    .expect("image texture scene")
}

fn decode(encoded: u8) -> f32 {
    let value = encoded as f32 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[test]
fn image_texture_gpu_decodes_color_spaces_and_alpha_without_clipping_p3() {
    let headless = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("No adapter available for image texture color test: {error:?}");
            return;
        }
    };
    if headless.adapter.get_info().backend == wgpu::Backend::Noop {
        eprintln!("Native GPU unavailable; skipping image texture pixel comparison");
        return;
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(PIXELS.as_flattened(), 4, 2, image::ExtendedColorType::Rgba8)
        .expect("encode source pixels");
    let assets = AssetStore::new();
    assets.insert(
        "pixels",
        AssetData {
            bytes: png,
            mime_type: "image/png".into(),
            original_name: "pixels.png".into(),
        },
    );

    for working in [WorkingColorSpace::Srgb, WorkingColorSpace::DisplayP3] {
    for encoding in ["srgb", "linear", "display-p3"] {
        for alpha_mode in ["straight", "premultiplied"] {
            for output in ["color", "texture"] {
                let mut scene = scene(encoding, alpha_mode, output);
                scene.working_color_space = working;
                let build =
                    ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
                        .with_adapter(headless.adapter.clone())
                        .with_asset_store(assets.clone())
                        .build(&scene)
                        .expect("build image texture color space");
                build.shader_space.render();
                let image = build
                    .shader_space
                    .read_texture_rgba16f(build.scene_output_texture.as_str())
                    .expect("read linear render output");
                assert_eq!((image.width, image.height), (4, 2));
                for (index, source) in PIXELS.iter().enumerate() {
                    let alpha = source[3] as f32 / 255.0;
                    let mut rgb = [source[0], source[1], source[2]].map(|value| {
                        if encoding == "linear" {
                            value as f32 / 255.0
                        } else {
                            decode(value)
                        }
                    });
                    let source_space = if encoding == "display-p3" { WorkingColorSpace::DisplayP3 } else { WorkingColorSpace::Srgb };
                    rgb = source_space.convert(working, rgb);
                    if alpha_mode == "straight" {
                        rgb = rgb.map(|value| value * alpha);
                    }
                    let expected = [rgb[0], rgb[1], rgb[2], alpha];
                    for channel in 0..4 {
                        let actual = image.channels[index * 4 + channel];
                        assert!(
                            (actual - expected[channel]).abs() < 0.004,
                            "{encoding}/{alpha_mode}/{output} pixel {index} channel {channel}: {actual} != {}",
                            expected[channel]
                        );
                    }
                }
            }
        }
    }
}
}
