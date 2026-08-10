use std::{collections::BTreeMap, io::Cursor};

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use node_forge_render_server::{
    asset_store::AssetData,
    renderer::{self, types::Kernel2D},
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};

mod support;

const SOURCE_ASSET_ID: &str = "08cc563ac177";
const FIRST_DOWNSAMPLE_TEXTURE: &str = "sys.downsample.Downsample_e2192d99_15.out";
const SOURCE_IMAGE_NODE_ID: &str = "ImageTexture_e2192d99_25";
const FIRST_DOWNSAMPLE_NODE_ID: &str = "Downsample_e2192d99_15";
const KERNEL_NODE_ID: &str = "Kernel_e2192d99_16";
const MULTIPASS_TARGET_NODE_ID: &str = "RenderTexture_e2192d99_17";

type SparseKernel = BTreeMap<(i32, i32), f64>;

fn centered_marker_png() -> Vec<u8> {
    let mut image = RgbaImage::from_pixel(512, 512, Rgba([255, 255, 255, 255]));
    for y in 240..272 {
        for x in 240..272 {
            image.put_pixel(x, y, Rgba([0, 0, 0, 255]));
        }
    }

    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode centered downsample marker");
    bytes.into_inner()
}

fn deterministic_probe_png() -> (Vec<u8>, Vec<[f64; 3]>) {
    fn channel(x: u32, y: u32, salt: u32) -> u8 {
        let mut value = x.wrapping_mul(0x9e37_79b9)
            ^ y.wrapping_mul(0x85eb_ca6b)
            ^ salt.wrapping_mul(0xc2b2_ae35);
        value ^= value >> 16;
        value = value.wrapping_mul(0x7feb_352d);
        value ^= value >> 15;
        value = value.wrapping_mul(0x846c_a68b);
        value ^= value >> 16;
        value as u8
    }

    let mut image = RgbaImage::new(512, 512);
    let mut linear = Vec::with_capacity(512 * 512);
    for y in 0..512 {
        for x in 0..512 {
            let rgba = [channel(x, y, 1), channel(x, y, 2), channel(x, y, 3)];
            image.put_pixel(x, y, Rgba([rgba[0], rgba[1], rgba[2], 255]));
            linear.push([
                half::f16::from_f32(rgba[0] as f32 / 255.0).to_f32() as f64,
                half::f16::from_f32(rgba[1] as f32 / 255.0).to_f32() as f64,
                half::f16::from_f32(rgba[2] as f32 / 255.0).to_f32() as f64,
            ]);
        }
    }

    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode deterministic kernel probe");
    (bytes.into_inner(), linear)
}

fn add_sparse_weight(kernel: &mut SparseKernel, position: (i32, i32), weight: f64) {
    if weight.abs() > 1.0e-15 {
        *kernel.entry(position).or_default() += weight;
    }
}

/// Resolve one shader pass into its discrete source-pixel kernel. The shader supplies normalized
/// coordinates at `0.5 + tap - kernel_center` in texel-index space; odd kernels therefore split
/// each tap across the adjacent 2x2 texels while even kernels land on exact texel centers.
fn single_pass_effective_kernel(kernel: &Kernel2D) -> SparseKernel {
    let center_x = (kernel.width as f64 - 1.0) * 0.5;
    let center_y = (kernel.height as f64 - 1.0) * 0.5;
    let mut effective = SparseKernel::new();

    for tap_y in 0..kernel.height {
        for tap_x in 0..kernel.width {
            let tap_weight = kernel.values[(tap_y * kernel.width + tap_x) as usize] as f64;
            let source_x = 0.5 + tap_x as f64 - center_x;
            let source_y = 0.5 + tap_y as f64 - center_y;
            let x0 = source_x.floor() as i32;
            let y0 = source_y.floor() as i32;
            let fx = source_x - x0 as f64;
            let fy = source_y - y0 as f64;

            for (x, x_weight) in [(x0, 1.0 - fx), (x0 + 1, fx)] {
                for (y, y_weight) in [(y0, 1.0 - fy), (y0 + 1, fy)] {
                    add_sparse_weight(&mut effective, (x, y), tap_weight * x_weight * y_weight);
                }
            }
        }
    }

    effective
}

/// For `out[j] = sum(H[n] * src[2*j + n])`, three passes are equivalent to
/// `H * dilate_2(H) * dilate_4(H)` in source-pixel space.
fn three_pass_equivalent_kernel(kernel: &Kernel2D) -> SparseKernel {
    let single = single_pass_effective_kernel(kernel);
    let mut equivalent = SparseKernel::from([((0, 0), 1.0)]);

    for _ in 0..3 {
        let mut expanded = SparseKernel::new();
        for (&(output_x, output_y), &output_weight) in &equivalent {
            for (&(tap_x, tap_y), &tap_weight) in &single {
                add_sparse_weight(
                    &mut expanded,
                    (2 * output_x + tap_x, 2 * output_y + tap_y),
                    output_weight * tap_weight,
                );
            }
        }
        equivalent = expanded;
    }

    equivalent
}

fn kernel_statistics(kernel: &SparseKernel) -> ((i32, i32, i32, i32), f64, (f64, f64)) {
    let min_x = kernel.keys().map(|(x, _)| *x).min().expect("kernel X min");
    let max_x = kernel.keys().map(|(x, _)| *x).max().expect("kernel X max");
    let min_y = kernel.keys().map(|(_, y)| *y).min().expect("kernel Y min");
    let max_y = kernel.keys().map(|(_, y)| *y).max().expect("kernel Y max");
    let mass: f64 = kernel.values().sum();
    let centroid_x = kernel
        .iter()
        .map(|(&(x, _), &weight)| x as f64 * weight)
        .sum::<f64>()
        / mass;
    let centroid_y = kernel
        .iter()
        .map(|(&(_, y), &weight)| y as f64 * weight)
        .sum::<f64>()
        / mass;
    ((min_x, max_x, min_y, max_y), mass, (centroid_x, centroid_y))
}

fn kernel_source(kernel: &Kernel2D) -> String {
    let values = kernel
        .values
        .iter()
        .map(|value| format!("{value:.9}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "return {{ width: {}, height: {}, value: [{values}] }};",
        kernel.width, kernel.height
    )
}

fn fixed_kernel_cases() -> Vec<(&'static str, Kernel2D)> {
    vec![
        (
            "box-2x2",
            Kernel2D {
                width: 2,
                height: 2,
                values: vec![0.25; 4],
            },
        ),
        (
            "cross-box-3x3",
            Kernel2D {
                width: 3,
                height: 3,
                values: vec![0.0, 0.25, 0.0, 0.25, 0.0, 0.25, 0.0, 0.25, 0.0],
            },
        ),
        (
            "gaussian-3x3",
            Kernel2D {
                width: 3,
                height: 3,
                values: vec![
                    0.0625, 0.125, 0.0625, 0.125, 0.25, 0.125, 0.0625, 0.125, 0.0625,
                ],
            },
        ),
        (
            "phase-probe-4x4",
            Kernel2D {
                width: 4,
                height: 4,
                values: vec![
                    0.5,
                    0.0,
                    1.0 / 64.0,
                    0.0,
                    0.0,
                    0.25,
                    0.0,
                    1.0 / 32.0,
                    1.0 / 16.0,
                    0.0,
                    0.125,
                    0.0,
                    0.0,
                    1.0 / 128.0,
                    0.0,
                    1.0 / 128.0,
                ],
            },
        ),
    ]
}

fn centroid(width: u32, height: u32, rgba: &[f32]) -> (f64, f64) {
    let mut weight_sum = 0.0_f64;
    let mut weighted_x = 0.0_f64;
    let mut weighted_y = 0.0_f64;

    for (index, pixel) in rgba.chunks_exact(4).enumerate() {
        let luminance = (pixel[0] + pixel[1] + pixel[2]) / 3.0;
        let weight = (pixel[3] - luminance).max(0.0) as f64;
        let x = (index as u32 % width) as f64;
        let y = (index as u32 / width) as f64;
        weight_sum += weight;
        weighted_x += x * weight;
        weighted_y += y * weight;
    }

    assert_eq!(rgba.len(), (width * height * 4) as usize);
    assert!(weight_sum > 0.0, "downsample marker must remain visible");
    (weighted_x / weight_sum, weighted_y / weight_sum)
}

fn rgba8_as_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .iter()
        .map(|channel| *channel as f32 / 255.0)
        .collect()
}

fn assert_centroid(actual: (f64, f64), expected: (f64, f64), label: &str) {
    const TOLERANCE: f64 = 0.05;
    assert!(
        (actual.0 - expected.0).abs() <= TOLERANCE,
        "{label} X centroid drifted: expected {:.3}, got {:.3}",
        expected.0,
        actual.0
    );
    assert!(
        (actual.1 - expected.1).abs() <= TOLERANCE,
        "{label} Y centroid drifted: expected {:.3}, got {:.3}",
        expected.1,
        actual.1
    );
}

#[test]
fn box2x2_downsample_preserves_phase_across_multiple_passes() {
    let _registry_guard = support::function_registry_lock();
    let (mut scene, assets) = support::load_render_case("kernel-research");
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
        .expect("raw image source connection to first downsample")
        .id
        .clone();
    let source_materialization_texture =
        format!("sys.pass.sys.auto.fullscreen.pass.{source_connection_id}.out");

    assets.insert_or_replace(
        SOURCE_ASSET_ID,
        AssetData {
            bytes: centered_marker_png(),
            mime_type: "image/png".to_string(),
            original_name: "centered-downsample-marker.png".to_string(),
        },
    );

    let headless = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("No adapter available for downsample phase test: {error:?}");
            return;
        }
    };
    if headless.adapter.get_info().backend == rust_wgpu_fiber::eframe::wgpu::Backend::Noop {
        eprintln!("Native GPU unavailable; skipping downsample phase integration test");
        return;
    }

    let build = renderer::ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
        .with_adapter(headless.adapter.clone())
        .with_asset_store(assets)
        .build(&scene)
        .expect("build kernel-research shader space");
    build.shader_space.render();

    let source = build
        .shader_space
        .texture_info(&source_materialization_texture)
        .expect("inspect full-resolution image materialization output");
    assert_eq!((source.size.width, source.size.height), (512, 512));
    assert_eq!(
        source.format,
        rust_wgpu_fiber::eframe::wgpu::TextureFormat::Rgba16Float
    );

    let first = build
        .shader_space
        .read_texture_rgba8(FIRST_DOWNSAMPLE_TEXTURE)
        .expect("read first 512-to-256 downsample output");
    assert_eq!((first.width, first.height), (256, 256));
    let first_channels = rgba8_as_f32(&first.bytes);
    assert_centroid(
        centroid(first.width, first.height, &first_channels),
        (127.5, 127.5),
        "single-pass 512-to-256",
    );

    let multipass = build
        .shader_space
        .read_texture_rgba8(build.scene_output_texture.as_str())
        .expect("read 512-to-256-to-128-to-64 output");
    assert_eq!((multipass.width, multipass.height), (64, 64));
    let multipass_channels = rgba8_as_f32(&multipass.bytes);
    assert_centroid(
        centroid(multipass.width, multipass.height, &multipass_channels),
        (31.5, 31.5),
        "three-pass 512-to-64",
    );
}

#[test]
fn theoretical_three_pass_equivalent_kernels_are_phase_correct() {
    for (case_name, kernel) in fixed_kernel_cases() {
        let equivalent = three_pass_equivalent_kernel(&kernel);
        let ((min_x, max_x, min_y, max_y), mass, equivalent_centroid) =
            kernel_statistics(&equivalent);
        assert!((mass - 1.0).abs() <= 1.0e-6, "{case_name} kernel mass");

        if case_name == "box-2x2" {
            assert_eq!((min_x, max_x, min_y, max_y), (0, 7, 0, 7));
            assert_eq!(equivalent.len(), 64);
            assert!(
                equivalent
                    .values()
                    .all(|weight| (*weight - 1.0 / 64.0).abs() <= 1.0e-12)
            );
        }
        if case_name != "phase-probe-4x4" {
            assert!((equivalent_centroid.0 - 3.5).abs() <= 1.0e-6);
            assert!((equivalent_centroid.1 - 3.5).abs() <= 1.0e-6);
        }
        eprintln!(
            "{case_name}: support=({min_x}..{max_x}, {min_y}..{max_y}) taps={} mass={mass:.9} centroid=({:.6}, {:.6})",
            equivalent.len(),
            equivalent_centroid.0,
            equivalent_centroid.1,
        );
    }
}

#[test]
fn three_pass_gpu_matches_theoretical_equivalent_kernels() {
    let _registry_guard = support::function_registry_lock();
    let headless = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("No adapter available for equivalent-kernel test: {error:?}");
            return;
        }
    };
    if headless.adapter.get_info().backend == rust_wgpu_fiber::eframe::wgpu::Backend::Noop {
        eprintln!("Native GPU unavailable; skipping equivalent-kernel integration test");
        return;
    }

    let (probe_png, probe_linear) = deterministic_probe_png();

    for (case_name, kernel) in fixed_kernel_cases() {
        let equivalent = three_pass_equivalent_kernel(&kernel);
        let ((min_x, max_x, min_y, max_y), mass, equivalent_centroid) =
            kernel_statistics(&equivalent);

        let (mut scene, assets) = support::load_render_case("kernel-research");
        scene.connections.retain(|connection| {
            connection.to.node_id != SOURCE_IMAGE_NODE_ID || connection.to.port_id != "uv"
        });
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
        kernel_node
            .params
            .insert("source".to_string(), kernel_source(&kernel).into());
        let target_node = scene
            .nodes
            .iter_mut()
            .find(|node| node.id == MULTIPASS_TARGET_NODE_ID)
            .expect("kernel-research multipass target");
        target_node
            .params
            .insert("format".to_string(), "rgba16float".into());

        assets.insert_or_replace(
            SOURCE_ASSET_ID,
            AssetData {
                bytes: probe_png.clone(),
                mime_type: "image/png".to_string(),
                original_name: "downsample-equivalent-kernel-probe.png".to_string(),
            },
        );

        let build =
            renderer::ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
                .with_adapter(headless.adapter.clone())
                .with_asset_store(assets)
                .build(&scene)
                .unwrap_or_else(|error| panic!("build {case_name} shader space: {error:#}"));
        build.shader_space.render();
        let output = build
            .shader_space
            .read_texture_rgba16f(build.scene_output_texture.as_str())
            .unwrap_or_else(|error| panic!("read {case_name} output: {error:#}"));
        assert_eq!((output.width, output.height), (64, 64));

        let mut max_error = 0.0_f64;
        let mut squared_error = 0.0_f64;
        let mut compared_channels = 0_usize;
        for output_y in 0..output.height as i32 {
            let base_y = output_y * 8;
            if base_y + min_y < 0 || base_y + max_y >= 512 {
                continue;
            }
            for output_x in 0..output.width as i32 {
                let base_x = output_x * 8;
                if base_x + min_x < 0 || base_x + max_x >= 512 {
                    continue;
                }

                for channel in 0..3 {
                    let expected = equivalent
                        .iter()
                        .map(|(&(kernel_x, kernel_y), &weight)| {
                            let source_x = (base_x + kernel_x) as usize;
                            let source_y = (base_y + kernel_y) as usize;
                            probe_linear[source_y * 512 + source_x][channel] * weight
                        })
                        .sum::<f64>();
                    let output_index =
                        ((output_y as u32 * output.width + output_x as u32) * 4) as usize + channel;
                    let actual = output.channels[output_index] as f64;
                    let error = (actual - expected).abs();
                    max_error = max_error.max(error);
                    squared_error += error * error;
                    compared_channels += 1;
                }
            }
        }
        let rms_error = (squared_error / compared_channels as f64).sqrt();
        eprintln!(
            "{case_name}: support=({min_x}..{max_x}, {min_y}..{max_y}) taps={} mass={mass:.9} centroid=({:.6}, {:.6}) max_error={max_error:.6} rms_error={rms_error:.6}",
            equivalent.len(),
            equivalent_centroid.0,
            equivalent_centroid.1,
        );
        assert!(
            max_error <= 0.004,
            "{case_name} differs from its theoretical three-pass kernel: max error {max_error}"
        );
        assert!(
            rms_error <= 0.001,
            "{case_name} differs from its theoretical three-pass kernel: RMS error {rms_error}"
        );
    }
}
