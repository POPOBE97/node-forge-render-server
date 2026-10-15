use node_forge_render_server::{
    app::{ClippingMode, ClippingSettings},
    reference_image::linear_p3_to_linear_srgb,
    ui::clipping_map::ClippingMapRenderer,
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig, eframe::wgpu};

const CLEAR: [u8; 4] = [0, 0, 0, 0];
const HIGH: [u8; 4] = [255, 89, 0, 199];
const LOW: [u8; 4] = [0, 115, 255, 199];
const BOTH: [u8; 4] = [255, 0, 255, 217];

fn read_overlay(gpu: &HeadlessRenderer, renderer: &ClippingMapRenderer) -> Vec<[u8; 4]> {
    let [width, height] = renderer.output_size();
    let row_bytes = (width * 4).div_ceil(256) * 256;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("clipping-test.readback"),
        size: u64::from(row_bytes * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        renderer.output_texture().as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    gpu.device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .expect("wait for clipping readback");
    rx.recv().unwrap().expect("map clipping pixels");
    let mapped = slice.get_mapped_range().unwrap();
    mapped
        .chunks(row_bytes as usize)
        .flat_map(|row| row[..width as usize * 4].chunks_exact(4))
        .map(|pixel| pixel.try_into().unwrap())
        .collect()
}

#[test]
fn clipping_gpu_checks_linear_rgb_bounds_and_preserves_threshold_mode() {
    let gpu = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(gpu) => gpu,
        Err(error) => {
            eprintln!("No GPU available for clipping pixel comparison: {error:?}");
            return;
        }
    };
    if gpu.adapter.get_info().backend == wgpu::Backend::Noop {
        eprintln!("Native GPU unavailable; skipping clipping pixel comparison");
        return;
    }
    let mut cases = vec![
        ([0.0, 0.0, 0.0, 1.0], CLEAR),
        ([1.0, 1.0, 1.0, 1.0], CLEAR),
        ([0.0, 0.5, 1.0, 1.0], CLEAR),
        ([0.01, 0.5, 0.99, 1.0], CLEAR),
        ([0.5, 0.5, 0.5, -0.5], CLEAR),
        ([0.5, 0.5, 0.5, 2.0], CLEAR),
        ([1.25, -0.1, 0.5, 1.0], BOTH),
    ];
    for channel in 0..3 {
        let mut high = [0.5; 4];
        high[channel] = 1.0 + f32::EPSILON;
        cases.push((high, HIGH));
        let mut low = [0.5; 4];
        low[channel] = -f32::EPSILON;
        cases.push((low, LOW));
    }
    let [r, g, b] = linear_p3_to_linear_srgb([1.0, 0.0, 0.0]);
    cases.push(([r, g, b, 1.0], BOTH));

    let pixels: Vec<[f32; 4]> = cases.iter().map(|(pixel, _)| *pixel).collect();
    // Two rows exercise dispatch bounds and readback row alignment.
    let size = [7, 2];
    let source = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("clipping-test.linear-source"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        source.as_image_copy(),
        bytemuck::cast_slice(&pixels),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size[0] * 16),
            rows_per_image: None,
        },
        source.size(),
    );
    let view = source.create_view(&Default::default());
    let mut renderer = ClippingMapRenderer::new(&gpu.device, [1, 1]);
    let linear = ClippingSettings {
        mode: ClippingMode::LinearSrgb,
        // Threshold sliders must have no effect on the strict mode.
        shadow_threshold: 0.25,
        highlight_threshold: 0.75,
    };
    for settings in [linear, ClippingSettings::default(), linear] {
        renderer.update(&gpu.device, &gpu.queue, &view, size, settings);
        let output = read_overlay(&gpu, &renderer);
        assert_eq!(output.len(), cases.len());
        for (index, ((_, expected), actual)) in cases.iter().zip(&output).enumerate() {
            let expected = if settings.mode == ClippingMode::LinearSrgb {
                *expected
            } else {
                match index {
                    0 => LOW,
                    1 => HIGH,
                    2 | 3 => BOTH,
                    _ => *expected,
                }
            };
            for channel in 0..4 {
                assert!(
                    actual[channel].abs_diff(expected[channel]) <= 1,
                    "{:?} pixel {index}: {actual:?} != {expected:?}",
                    settings.mode
                );
            }
        }
    }
}
