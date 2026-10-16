use std::io::Cursor;

use image::{DynamicImage, ImageBuffer, ImageFormat};
use node_forge_render_server::{
    asset_store::{AssetData, AssetStore},
    dsl::SceneDSL,
    reference_image::linear_p3_to_linear_srgb,
    renderer::ShaderSpaceBuilder,
};
use rust_wgpu_fiber::{
    HeadlessRenderer, HeadlessRendererConfig, eframe::wgpu, shader_space::ShaderSpace,
};
use serde_json::json;

fn scene(width: u32, height: u32, encoding: &str, alpha: &str, output: &str) -> SceneDSL {
    serde_json::from_value(json!({
        "version": "6.0", "workingColorSpace": "linear-srgb",
        "metadata": { "name": "image-texture-precision" },
        "nodes": [
            { "id": "image", "type": "ImageTexture", "params": {
                "assetId": "pixels", "encoderSpace": encoding, "alphaMode": alpha,
                "aspectCorrection": "off", "magFilter": "nearest", "minFilter": "nearest"
            } },
            { "id": "target", "type": "RenderTexture", "params": {
                "width": width, "height": height, "format": "rgba16float"
            } },
            { "id": "composite", "type": "Composite" },
            { "id": "screen", "type": "Screen", "params": { "outputColorSpace": "srgb", "width": width, "height": height } }
        ],
        "connections": [
            { "id": "image", "from": { "nodeId": "image", "portId": output },
                "to": { "nodeId": "composite", "portId": "pass" } },
            { "id": "target", "from": { "nodeId": "target", "portId": "texture" },
                "to": { "nodeId": "composite", "portId": "target" } },
            { "id": "screen", "from": { "nodeId": "composite", "portId": "pass" },
                "to": { "nodeId": "screen", "portId": "pass" } }
        ],
        "outputs": null
    }))
    .unwrap()
}

// Inspect the actual uploaded/prepared texture, before the authored FP16 render
// target can round it. This catches both CPU RGBA8 truncation and FP16 prepasses.
fn read_source_f32(space: &ShaderSpace) -> Vec<f32> {
    let resource = &space.textures["image"];
    assert_eq!(
        resource.wgpu_texture_desc.format,
        wgpu::TextureFormat::Rgba32Float
    );
    space.read_texture_rgba32f("image").unwrap().channels
}

fn assets(image: &DynamicImage, format: ImageFormat) -> AssetStore {
    let mut data = Cursor::new(Vec::new());
    image.write_to(&mut data, format).unwrap();
    let assets = AssetStore::new();
    assets.insert(
        "pixels",
        AssetData {
            bytes: data.into_inner(),
            mime_type: "application/octet-stream".into(),
            original_name: "pixels".into(),
        },
    );
    assets
}

fn decode(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[test]
fn image_texture_gpu_preserves_u16_gradients_float_range_and_alpha_precision() {
    let headless = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("No native adapter for precision test: {error}");
            return;
        }
    };
    if headless.adapter.get_info().backend == wgpu::Backend::Noop {
        eprintln!("Noop adapter cannot validate image precision");
        return;
    }
    let source = ImageBuffer::from_fn(256, 1, |x, _| {
        image::Rgba([32768 + x as u16, 1000 + x as u16, 60000, 54321])
    });
    let images = assets(&DynamicImage::ImageRgba16(source.clone()), ImageFormat::Png);
    for encoding in ["linear", "srgb", "display-p3"] {
        for alpha in ["straight", "premultiplied"] {
            for output in ["color", "texture"] {
                let build =
                    ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
                        .with_adapter(headless.adapter.clone())
                        .with_asset_store(images.clone())
                        .build(&scene(256, 1, encoding, alpha, output))
                        .unwrap();
                build.shader_space.render();
                let actual = read_source_f32(&build.shader_space);
                for (index, pixel) in source.pixels().enumerate() {
                    let a = pixel[3] as f32 / 65535.0;
                    let mut rgb = [pixel[0], pixel[1], pixel[2]].map(|v| v as f32 / 65535.0);
                    if encoding != "linear" {
                        rgb = rgb.map(decode);
                    }
                    if encoding == "display-p3" {
                        rgb = linear_p3_to_linear_srgb(rgb);
                    }
                    if alpha == "straight" {
                        rgb = rgb.map(|v| v * a);
                    }
                    for (channel, expected) in [rgb[0], rgb[1], rgb[2], a].into_iter().enumerate() {
                        assert!(
                            (actual[index * 4 + channel] - expected).abs() < 2e-7,
                            "{encoding}/{alpha}/{output} pixel {index} channel {channel}"
                        );
                    }
                }
                let distinct: std::collections::HashSet<_> =
                    actual.chunks_exact(4).map(|p| p[0].to_bits()).collect();
                assert_eq!(distinct.len(), 256, "adjacent u16 levels must not collapse");
            }
        }
    }

    let values = vec![
        -0.25,
        0.500_123_4,
        100_000.0,
        1.5,
        0.123_456_78,
        8.5,
        -2.0,
        0.5,
    ];
    let float = DynamicImage::ImageRgba32F(ImageBuffer::from_raw(2, 1, values.clone()).unwrap());
    let build = ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
        .with_adapter(headless.adapter.clone())
        .with_asset_store(assets(&float, ImageFormat::OpenExr))
        .build(&scene(2, 1, "linear", "premultiplied", "texture"))
        .unwrap();
    build.shader_space.render();
    assert_eq!(read_source_f32(&build.shader_space), values);
    let export = std::env::temp_dir().join(format!("image-precision-{}.exr", std::process::id()));
    build
        .shader_space
        .save_texture_exr("image", &export)
        .unwrap();
    assert_eq!(image::open(&export).unwrap().to_rgba32f().as_raw(), &values);
    std::fs::remove_file(export).unwrap();

    let low_alpha =
        DynamicImage::ImageRgba8(ImageBuffer::from_pixel(1, 1, image::Rgba([1, 17, 129, 1])));
    let build = ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
        .with_adapter(headless.adapter.clone())
        .with_asset_store(assets(&low_alpha, ImageFormat::Png))
        .build(&scene(1, 1, "linear", "straight", "color"))
        .unwrap();
    build.shader_space.render();
    let actual = read_source_f32(&build.shader_space);
    for (channel, value) in [1.0, 17.0, 129.0].into_iter().enumerate() {
        assert!((actual[channel] - value / (255.0 * 255.0)).abs() < 1e-8);
    }
}
