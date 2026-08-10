use std::io::Cursor;

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use node_forge_render_server::{asset_store::AssetData, renderer};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};

mod support;

const SOURCE_ASSET_ID: &str = "08cc563ac177";
const SOURCE_IMAGE_NODE_ID: &str = "ImageTexture_e2192d99_25";
const FIRST_DOWNSAMPLE_NODE_ID: &str = "Downsample_e2192d99_15";
const KERNEL_NODE_ID: &str = "Kernel_e2192d99_16";
const MULTIPASS_TARGET_NODE_ID: &str = "RenderTexture_e2192d99_17";

#[derive(Clone)]
struct FloatImage {
    width: u32,
    height: u32,
    pixels: Vec<[f32; 3]>,
}

fn quantize_f16(value: f32) -> f32 {
    half::f16::from_f32(value).to_f32()
}

fn lanczos_probe_png() -> (Vec<u8>, FloatImage) {
    fn random_channel(x: u32, y: u32) -> u8 {
        let mut value = x.wrapping_mul(0x9e37_79b9) ^ y.wrapping_mul(0x85eb_ca6b) ^ 0xc2b2_ae35;
        value ^= value >> 16;
        value = value.wrapping_mul(0x7feb_352d);
        value ^= value >> 15;
        value = value.wrapping_mul(0x846c_a68b);
        value ^= value >> 16;
        value as u8
    }

    let mut image = RgbaImage::new(512, 512);
    let mut pixels = Vec::with_capacity(512 * 512);
    for y in 0..512 {
        for x in 0..512 {
            let rgba = [
                random_channel(x, y),
                if (x + y) % 2 == 0 { 0 } else { 255 },
                if x % 2 == 0 { 0 } else { 255 },
            ];
            image.put_pixel(x, y, Rgba([rgba[0], rgba[1], rgba[2], 255]));
            pixels.push([
                quantize_f16(rgba[0] as f32 / 255.0),
                quantize_f16(rgba[1] as f32 / 255.0),
                quantize_f16(rgba[2] as f32 / 255.0),
            ]);
        }
    }

    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode Lanczos probe image");
    (
        bytes.into_inner(),
        FloatImage {
            width: 512,
            height: 512,
            pixels,
        },
    )
}

fn mirror_index(index: i32, length: u32) -> usize {
    let period = (length * 2) as i32;
    let mirrored = index.rem_euclid(period);
    if mirrored < length as i32 {
        mirrored as usize
    } else {
        (period - 1 - mirrored) as usize
    }
}

fn sinc(value: f32) -> f32 {
    if value.abs() <= 0.000_001 {
        1.0
    } else {
        let pi_value = std::f32::consts::PI * value;
        pi_value.sin() / pi_value
    }
}

fn lanczos_weight(sample_index: i32, source_center: f32, scale: f32, lobes: u32) -> f32 {
    let x = (sample_index as f32 - source_center) / scale;
    sinc(x) * sinc(x / lobes as f32)
}

fn resample_horizontal(source: &FloatImage, target_width: u32, lobes: u32) -> FloatImage {
    let scale = (source.width as f32 / target_width as f32).max(1.0);
    let support = lobes as f32 * scale;
    let mut pixels = vec![[0.0; 3]; (target_width * source.height) as usize];

    for y in 0..source.height {
        for target_x in 0..target_width {
            let source_center =
                (target_x as f32 + 0.5) * source.width as f32 / target_width as f32 - 0.5;
            let first_sample = (source_center - support).ceil() as i32;
            let last_sample = (source_center + support).floor() as i32;
            let mut sum = [0.0_f32; 3];
            let mut weight_sum = 0.0_f32;
            for sample_index in first_sample..=last_sample {
                let weight = lanczos_weight(sample_index, source_center, scale, lobes);
                let source_x = mirror_index(sample_index, source.width);
                let sample = source.pixels[y as usize * source.width as usize + source_x];
                for channel in 0..3 {
                    sum[channel] += sample[channel] * weight;
                }
                weight_sum += weight;
            }
            let output_index = (y * target_width + target_x) as usize;
            for channel in 0..3 {
                pixels[output_index][channel] = quantize_f16(sum[channel] / weight_sum);
            }
        }
    }

    FloatImage {
        width: target_width,
        height: source.height,
        pixels,
    }
}

fn resample_vertical(source: &FloatImage, target_height: u32, lobes: u32) -> FloatImage {
    let scale = (source.height as f32 / target_height as f32).max(1.0);
    let support = lobes as f32 * scale;
    let mut pixels = vec![[0.0; 3]; (source.width * target_height) as usize];

    for target_y in 0..target_height {
        let source_center =
            (target_y as f32 + 0.5) * source.height as f32 / target_height as f32 - 0.5;
        let first_sample = (source_center - support).ceil() as i32;
        let last_sample = (source_center + support).floor() as i32;
        for x in 0..source.width {
            let mut sum = [0.0_f32; 3];
            let mut weight_sum = 0.0_f32;
            for sample_index in first_sample..=last_sample {
                let weight = lanczos_weight(sample_index, source_center, scale, lobes);
                let source_y = mirror_index(sample_index, source.height);
                let sample = source.pixels[source_y * source.width as usize + x as usize];
                for channel in 0..3 {
                    sum[channel] += sample[channel] * weight;
                }
                weight_sum += weight;
            }
            let output_index = (target_y * source.width + x) as usize;
            for channel in 0..3 {
                pixels[output_index][channel] = quantize_f16(sum[channel] / weight_sum);
            }
        }
    }

    FloatImage {
        width: source.width,
        height: target_height,
        pixels,
    }
}

fn three_pass_lanczos(source: &FloatImage, lobes: u32) -> FloatImage {
    let mut current = source.clone();
    for target_size in [256, 128, 64] {
        current = resample_horizontal(&current, target_size, lobes);
        current = resample_vertical(&current, target_size, lobes);
    }
    current
}

fn configure_lanczos_scene(
    scene: &mut node_forge_render_server::dsl::SceneDSL,
    lobes: u32,
) -> String {
    scene.connections.retain(|connection| {
        connection.to.node_id != SOURCE_IMAGE_NODE_ID || connection.to.port_id != "uv"
    });
    let source_connection_id = scene
        .connections
        .iter()
        .find(|connection| {
            connection.from.node_id == SOURCE_IMAGE_NODE_ID
                && connection.to.node_id == FIRST_DOWNSAMPLE_NODE_ID
                && connection.to.port_id == "source"
        })
        .expect("raw image source connection to first Lanczos pass")
        .id
        .clone();
    let image_node = scene
        .nodes
        .iter_mut()
        .find(|node| node.id == SOURCE_IMAGE_NODE_ID)
        .expect("kernel-research source image");
    image_node
        .params
        .insert("encoderSpace".to_string(), "linear".into());
    image_node
        .params
        .insert("alphaMode".to_string(), "premultiplied".into());
    let kernel_node = scene
        .nodes
        .iter_mut()
        .find(|node| node.id == KERNEL_NODE_ID)
        .expect("kernel-research shared kernel");
    kernel_node.params.insert(
        "source".to_string(),
        format!("return {{ kind: 'lanczos', lobes: {lobes} }};").into(),
    );
    let target_node = scene
        .nodes
        .iter_mut()
        .find(|node| node.id == MULTIPASS_TARGET_NODE_ID)
        .expect("kernel-research multipass target");
    target_node
        .params
        .insert("format".to_string(), "rgba16float".into());

    format!("sys.pass.sys.auto.fullscreen.pass.{source_connection_id}.out")
}

#[test]
fn scale_aware_lanczos_rejects_nyquist_frequency() {
    for lobes in [3, 5] {
        let source_center = 0.5_f32;
        let scale = 2.0_f32;
        let support = lobes as f32 * scale;
        let first_sample = (source_center - support).ceil() as i32;
        let last_sample = (source_center + support).floor() as i32;
        let mut dc = 0.0_f32;
        let mut nyquist = 0.0_f32;
        for sample_index in first_sample..=last_sample {
            let weight = lanczos_weight(sample_index, source_center, scale, lobes);
            dc += weight;
            nyquist += weight * if sample_index % 2 == 0 { 1.0 } else { -1.0 };
        }
        let gain = (nyquist / dc).abs();
        eprintln!("Lanczos{lobes}: Nyquist gain={gain:.9}");
        assert!(gain <= 1.0e-6, "Lanczos{lobes} must reject Nyquist");
    }
}

#[test]
fn three_pass_gpu_lanczos_matches_cpu_and_suppresses_aliasing() {
    let _registry_guard = support::function_registry_lock();
    let headless = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("No adapter available for Lanczos test: {error:?}");
            return;
        }
    };
    if headless.adapter.get_info().backend == rust_wgpu_fiber::eframe::wgpu::Backend::Noop {
        eprintln!("Native GPU unavailable; skipping Lanczos integration test");
        return;
    }

    let (probe_png, probe_linear) = lanczos_probe_png();
    for lobes in [3, 5] {
        let expected = three_pass_lanczos(&probe_linear, lobes);
        let (mut scene, assets) = support::load_render_case("kernel-research");
        let source_materialization_texture = configure_lanczos_scene(&mut scene, lobes);
        assets.insert_or_replace(
            SOURCE_ASSET_ID,
            AssetData {
                bytes: probe_png.clone(),
                mime_type: "image/png".to_string(),
                original_name: "downsample-lanczos-probe.png".to_string(),
            },
        );

        let build =
            renderer::ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
                .with_adapter(headless.adapter.clone())
                .with_asset_store(assets)
                .build(&scene)
                .unwrap_or_else(|error| panic!("build Lanczos{lobes} shader space: {error:#}"));
        build.shader_space.render();
        let source_info = build
            .shader_space
            .texture_info(&source_materialization_texture)
            .expect("inspect native Lanczos source texture");
        assert_eq!(
            (source_info.size.width, source_info.size.height),
            (512, 512)
        );
        assert_eq!(
            source_info.format,
            rust_wgpu_fiber::eframe::wgpu::TextureFormat::Rgba16Float
        );
        let actual = build
            .shader_space
            .read_texture_rgba16f(build.scene_output_texture.as_str())
            .unwrap_or_else(|error| panic!("read Lanczos{lobes} output: {error:#}"));
        assert_eq!((actual.width, actual.height), (64, 64));

        let mut max_error = 0.0_f64;
        let mut squared_error = 0.0_f64;
        let mut compared_channels = 0_usize;
        let mut max_alias_deviation = 0.0_f32;
        let alias_margin = lobes as usize * 2;
        for (pixel_index, expected_pixel) in expected.pixels.iter().enumerate() {
            for channel in 0..3 {
                let actual_value = actual.channels[pixel_index * 4 + channel];
                let error = (actual_value - expected_pixel[channel]).abs() as f64;
                max_error = max_error.max(error);
                squared_error += error * error;
                compared_channels += 1;
            }
            let x = pixel_index % actual.width as usize;
            let y = pixel_index / actual.width as usize;
            if x >= alias_margin
                && x < actual.width as usize - alias_margin
                && y >= alias_margin
                && y < actual.height as usize - alias_margin
            {
                for channel in [1, 2] {
                    max_alias_deviation = max_alias_deviation
                        .max((actual.channels[pixel_index * 4 + channel] - 0.5).abs());
                }
            }
        }
        let rms_error = (squared_error / compared_channels as f64).sqrt();
        eprintln!(
            "Lanczos{lobes}: max_error={max_error:.6} rms_error={rms_error:.6} max_nyquist_deviation={max_alias_deviation:.6}"
        );
        assert!(max_error <= 0.004, "Lanczos{lobes} CPU/GPU max error");
        assert!(rms_error <= 0.001, "Lanczos{lobes} CPU/GPU RMS error");
        assert!(
            max_alias_deviation <= 0.002,
            "Lanczos{lobes} should suppress checkerboard and stripe Nyquist inputs"
        );
    }
}
