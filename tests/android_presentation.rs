#[path = "../crates/android-player/src/presentation.rs"]
mod presentation;

use node_forge_render_server::{
    asset_store::AssetStore, color::WorkingColorSpace, dsl::SceneDSL, playback::PlaybackRuntime,
    renderer::ShaderSpacePresentationMode,
};
use presentation::Output;
use rust_wgpu_fiber::{
    HeadlessRenderer, HeadlessRendererConfig,
    wgpu::{self, util::DeviceExt},
};
use serde_json::json;

#[test]
fn android_maps_float_targets_to_native_hdr_and_unorm_to_window_primaries() {
    use wgpu::{SurfaceColorSpaces as S, TextureFormat as F};
    let caps = wgpu::SurfaceCapabilities {
        format_capabilities: vec![
            wgpu::SurfaceFormatCapabilities {
                format: F::Rgba8UnormSrgb,
                color_spaces: S::SRGB | S::DISPLAY_P3,
            },
            // Vulkan exposes ordinary P3 even on float surfaces. That is NOT extended P3.
            wgpu::SurfaceFormatCapabilities {
                format: F::Rgba16Float,
                color_spaces: S::EXTENDED_SRGB_LINEAR | S::DISPLAY_P3,
            },
        ],
        ..Default::default()
    };
    for source in [
        F::Rgba8Unorm,
        F::Rgba8UnormSrgb,
        F::Bgra8Unorm,
        F::Bgra8UnormSrgb,
    ] {
        for (space, name, mode) in [
            (WorkingColorSpace::Srgb, "srgb", 0),
            (WorkingColorSpace::DisplayP3, "display-p3", 1),
        ] {
            let output = Output::select(&caps, source, space).unwrap();
            assert_eq!(output.format_name(), "rgba8unorm-srgb");
            assert_eq!(output.color_space_name(), name);
            assert_eq!(output.android_color_mode(), mode);
            assert!(!output.hdr());
            assert_eq!(output.options(false, false, [4, 4])[4], 0.0); // hardware encoding
        }
    }
    let hdr = Output::select(&caps, F::Rgba32Float, WorkingColorSpace::Srgb).unwrap();
    assert!(hdr.hdr());
    assert_eq!(hdr.android_color_mode(), 2);
    assert_eq!(hdr.options(false, false, [4, 4])[4], 1.0);
    for source in [F::Rgba16Float, F::Rgba32Float] {
        for window in [WorkingColorSpace::Srgb, WorkingColorSpace::DisplayP3] {
            // Native AHB transport does not depend on WSI exposing extended P3.
            let hdr =
                Output::select(&wgpu::SurfaceCapabilities::default(), source, window).unwrap();
            assert_eq!(hdr.format_name(), "rgb10a2unorm");
            assert_eq!(hdr.color_space_name(), "extended-display-p3");
            assert_eq!(hdr.buffer_ratio(), 4.0);
            assert_eq!(
                hdr.options(false, false, [4, 4])[6],
                if window == WorkingColorSpace::Srgb {
                    1.0
                } else {
                    0.0
                }
            );
        }
    }
    assert!(
        Output::select(
            &wgpu::SurfaceCapabilities::default(),
            F::Rgba8Unorm,
            WorkingColorSpace::Srgb
        )
        .is_err()
    );
}

fn scene(working: WorkingColorSpace, output: &str, format: &str) -> SceneDSL {
    serde_json::from_value(json!({
        "version":"6.0", "workingColorSpace":working,
        "metadata":{"name":"android-presentation"},
        "nodes":[
            {"id":"color","type":"ColorInput","params":{"value":if format == "rgba16float" { [-0.25,2.0,0.125,1.0] } else { [0.125,1.0,0.5,1.0] }}},
            {"id":"geometry","type":"Rect2DGeometry","params":{"size":[4,4],"position":[2,2],"radius":0}},
            {"id":"pass","type":"RenderPass","params":{"blend_preset":"replace"}},
            {"id":"target","type":"RenderTexture","params":{"width":4,"height":4,"format":format}},
            {"id":"composite","type":"Composite"},
            {"id":"screen","type":"Screen","params":{"outputColorSpace":output,"width":4,"height":4}}
        ],
        "connections":[
            {"id":"color","from":{"nodeId":"color","portId":"color"},"to":{"nodeId":"pass","portId":"material"}},
            {"id":"geo","from":{"nodeId":"geometry","portId":"geometry"},"to":{"nodeId":"pass","portId":"geometry"}},
            {"id":"pass","from":{"nodeId":"pass","portId":"pass"},"to":{"nodeId":"composite","portId":"pass"}},
            {"id":"target","from":{"nodeId":"target","portId":"texture"},"to":{"nodeId":"composite","portId":"target"}},
            {"id":"screen","from":{"nodeId":"composite","portId":"pass"},"to":{"nodeId":"screen","portId":"pass"}}
        ]
    })).unwrap()
}

#[test]
fn android_gpu_encodes_screen_once_for_scaled_native_and_probe_paths() {
    use wgpu::{SurfaceColorSpace as S, TextureFormat as F};
    let gpu = HeadlessRenderer::new(HeadlessRendererConfig::default()).unwrap();
    assert_ne!(gpu.adapter.get_info().backend, wgpu::Backend::Noop);
    for source in ["rgba8unorm", "rgba8unorm-srgb", "rgba16float"] {
        for working in [WorkingColorSpace::Srgb, WorkingColorSpace::DisplayP3] {
            for (window, window_name) in [
                (WorkingColorSpace::Srgb, "srgb"),
                (WorkingColorSpace::DisplayP3, "display-p3"),
            ] {
                let mut runtime = PlaybackRuntime::new_with_presentation(
                    gpu.device.clone(),
                    gpu.queue.clone(),
                    gpu.adapter.clone(),
                    scene(working, window_name, source),
                    AssetStore::new(),
                    ShaderSpacePresentationMode::UiHdrNative,
                )
                .unwrap();
                runtime.advance(0.0).unwrap();
                assert_eq!(runtime.working_color_space(), working);
                let (_, _, raw) = runtime.capture_linear_rgba().unwrap();
                let expected_linear = working.convert(window, raw[20..23].try_into().unwrap());
                let formats: &[wgpu::TextureFormat] = if source == "rgba16float" {
                    &[F::Rgb10a2Unorm]
                } else {
                    &[F::Rgba8Unorm, F::Rgba8UnormSrgb, F::Bgra8Unorm]
                };
                for &format in formats {
                    let output = Output {
                        format,
                        color_space: if source == "rgba16float" {
                            S::ExtendedDisplayP3
                        } else {
                            window.window_color_space(F::Rgba8Unorm)
                        },
                        window_space: window,
                    };
                    for (native, probe) in [(false, false), (true, false), (false, true)] {
                        let bytes = render(&gpu, &runtime, output, native, probe);
                        let xs: &[usize] = if probe { &[0, 1, 2, 3] } else { &[1] };
                        for &x in xs {
                            let pixel_size = if format == F::Rgba16Float { 8 } else { 4 };
                            let bytes = &bytes[x * pixel_size..(x + 1) * pixel_size];
                            let rgb = if probe {
                                [[0.25; 3], [1.0; 3], [2.0; 3], [4.0; 3]][x]
                            } else {
                                expected_linear
                            };
                            let transport = if output.hdr() {
                                window.convert(WorkingColorSpace::DisplayP3, rgb)
                            } else {
                                rgb
                            };
                            for channel in 0..3 {
                                let expected = encode(transport[channel] / output.buffer_ratio());
                                let (actual, expected) = if format == F::Rgb10a2Unorm {
                                    let packed = u32::from_le_bytes(bytes.try_into().unwrap());
                                    let encoded =
                                        ((packed >> (channel * 10)) & 1023) as f32 / 1023.0;
                                    let reconstructed = decode(encoded) * output.buffer_ratio();
                                    assert!(
                                        (reconstructed - transport[channel].clamp(0.0, 4.0)).abs()
                                            < 0.01,
                                        "HDR reconstruction: {reconstructed} != {}",
                                        transport[channel]
                                    );
                                    (encoded, expected.clamp(0.0, 1.0))
                                } else {
                                    let index = if format == F::Bgra8Unorm {
                                        [2, 1, 0][channel]
                                    } else {
                                        channel
                                    };
                                    (bytes[index] as f32 / 255.0, expected.clamp(0.0, 1.0))
                                };
                                assert!(
                                    (actual - expected).abs() < 0.004,
                                    "{source} {working:?}/{window:?} {format:?} native={native} probe={probe} channel={channel}: {actual} != {expected}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

fn encode(value: f32) -> f32 {
    let x = value.abs();
    value.signum()
        * if x <= 0.0031308 {
            12.92 * x
        } else {
            1.055 * x.powf(1.0 / 2.4) - 0.055
        }
}

fn decode(x: f32) -> f32 {
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

fn render(
    gpu: &HeadlessRenderer,
    runtime: &PlaybackRuntime,
    output: Output,
    native: bool,
    probe: bool,
) -> Vec<u8> {
    let device = &gpu.device;
    let size = wgpu::Extent3d {
        width: 4,
        height: 4,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: output.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let pipeline = presentation::create_pipeline(device, output.format);
    let options = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&output.options(probe, native, [4, 4])),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let sampler = device.create_sampler(&Default::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    runtime.present_texture_view().unwrap(),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: options.as_entire_binding(),
            },
        ],
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 1024,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
        },
        size,
    );
    gpu.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = readback.slice(..).get_mapped_range().unwrap();
    let bytes_per_pixel = if output.format == wgpu::TextureFormat::Rgba16Float {
        8
    } else {
        4
    };
    let result = mapped[256..256 + 4 * bytes_per_pixel].to_vec();
    drop(mapped);
    readback.unmap();
    result
}
