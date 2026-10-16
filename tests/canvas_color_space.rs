use node_forge_render_server::color::WorkingColorSpace;
use rust_wgpu_fiber::{
    HeadlessRenderer, HeadlessRendererConfig,
    eframe::{egui, egui_wgpu, wgpu},
};

#[test]
fn canvas_and_ui_match_the_selected_surface_with_encoding_after_linear_blending() {
    for extended in [false, true] {
        check_canvas_output(extended);
    }
}

fn check_canvas_output(extended: bool) {
    let gpu = HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("GPU adapter");
    if gpu.adapter.get_info().backend == wgpu::Backend::Noop {
        return;
    }
    let device = &gpu.device;
    let size = wgpu::Extent3d {
        width: 1,
        height: 1,
        depth_or_array_layers: 1,
    };
    let source = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("canvas-color.source"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let target_format = if extended {
        wgpu::TextureFormat::Rgba16Float
    } else {
        wgpu::TextureFormat::Bgra8Unorm
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("canvas-color.target"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: target_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("canvas-color.readback"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut renderer = egui_wgpu::Renderer::new(
        device,
        wgpu::TextureFormat::Rgba16Float,
        egui_wgpu::RendererOptions {
            dithering: false,
            ..Default::default()
        },
    );
    let id = renderer.register_native_texture(
        device,
        &source.create_view(&Default::default()),
        wgpu::FilterMode::Nearest,
    );
    let ui_id = renderer.register_native_texture(
        device,
        &source.create_view(&Default::default()),
        wgpu::FilterMode::Nearest,
    );
    let presenter = egui_wgpu::EncodedSurfacePresenter::new(
        device, 1, 1, target_format,
    );
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1.0, 1.0));
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [1, 1],
        pixels_per_point: 1.0,
    };
    for (p3, working) in [
        (false, WorkingColorSpace::Srgb),
        (true, WorkingColorSpace::DisplayP3),
    ] {
        renderer.set_native_texture_linear_color_space(id, p3);
        for output in [WorkingColorSpace::Srgb, WorkingColorSpace::DisplayP3] {
            let needs_encoding = !extended || output == WorkingColorSpace::DisplayP3;
            renderer.set_compositing_display_p3(output == WorkingColorSpace::DisplayP3);
            for ordinary_ui in [false, true] {
                if ordinary_ui && p3 {
                    continue;
                }
                for pixel in [
                    [0.5_f32, 0.0, 0.0, 0.5],
                    [-0.25, 2.0, 0.125, 1.0],
                    [0.0, 1.0, 0.0, 1.0],
                ] {
                    for opacity in [255, 128] {
                        if ordinary_ui && opacity != 255 {
                            continue;
                        }
                        for background in [[0.0; 4], [0.05, 0.1, 0.2, 1.0]] {
                            let encoded = pixel.map(half::f16::from_f32).map(half::f16::to_bits);
                            gpu.queue.write_texture(
                                source.as_image_copy(),
                                bytemuck::cast_slice(&encoded),
                                wgpu::TexelCopyBufferLayout {
                                    offset: 0,
                                    bytes_per_row: Some(8),
                                    rows_per_image: None,
                                },
                                size,
                            );
                            let mut mesh =
                                egui::Mesh::with_texture(if ordinary_ui { ui_id } else { id });
                            mesh.add_rect_with_uv(
                                rect,
                                rect,
                                egui::Color32::from_white_alpha(opacity),
                            );
                            let jobs = [egui::ClippedPrimitive {
                                clip_rect: rect,
                                primitive: egui::epaint::Primitive::Mesh(mesh),
                            }];
                            let mut encoder = device.create_command_encoder(&Default::default());
                            let commands = renderer.update_buffers(
                                device,
                                &gpu.queue,
                                &mut encoder,
                                &jobs,
                                &screen,
                            );
                            {
                                let view = target.create_view(&Default::default());
                                let mut pass = encoder
                                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                                        label: None,
                                        color_attachments: &[Some(
                                            wgpu::RenderPassColorAttachment {
                                                view: if needs_encoding {
                                                    presenter.linear_view()
                                                } else {
                                                    &view
                                                },
                                                resolve_target: None,
                                                depth_slice: None,
                                                ops: wgpu::Operations {
                                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                                        r: background[0] as f64,
                                                        g: background[1] as f64,
                                                        b: background[2] as f64,
                                                        a: background[3] as f64,
                                                    }),
                                                    store: wgpu::StoreOp::Store,
                                                },
                                            },
                                        )],
                                        ..Default::default()
                                    })
                                    .forget_lifetime();
                                renderer.render(&mut pass, &jobs, &screen);
                            }
                            if needs_encoding {
                                presenter
                                    .encode(&mut encoder, &target.create_view(&Default::default()));
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
                            gpu.queue
                                .submit(commands.into_iter().chain([encoder.finish()]));
                            let (tx, rx) = std::sync::mpsc::channel();
                            readback
                                .slice(..)
                                .map_async(wgpu::MapMode::Read, move |result| {
                                    tx.send(result).unwrap()
                                });
                            device
                                .poll(wgpu::PollType::Wait {
                                    submission_index: None,
                                    timeout: Some(std::time::Duration::from_secs(30)),
                                })
                                .unwrap();
                            rx.recv().unwrap().unwrap();
                            let mapped = readback.slice(..).get_mapped_range().unwrap();
                            let rgb = working.convert(output, [pixel[0], pixel[1], pixel[2]]);
                            let alpha = pixel[3] * opacity as f32 / 255.0;
                            let final_alpha = alpha + background[3] * (1.0 - alpha);
                            let mut expected = [0.0; 4];
                            for channel in 0..3 {
                                let linear = rgb[channel] * opacity as f32 / 255.0
                                    + background[channel] * (1.0 - alpha);
                                expected[channel] = if needs_encoding {
                                    let divisor = if final_alpha > 0.0 { final_alpha } else { 1.0 };
                                    let straight = linear / divisor;
                                    let magnitude = straight.abs();
                                    let gamma = if magnitude <= 0.0031308 {
                                        12.92 * magnitude
                                    } else {
                                        1.055 * magnitude.powf(1.0 / 2.4) - 0.055
                                    };
                                    straight.signum() * gamma * divisor
                                } else {
                                    linear
                                };
                            }
                            expected[3] = final_alpha;
                            for (channel, expected) in expected.into_iter().enumerate() {
                                let (actual, expected) = if extended {
                                    (half::f16::from_le_bytes(
                                        mapped[channel * 2..channel * 2 + 2].try_into().unwrap(),
                                    ).to_f32(), expected)
                                } else {
                                    (mapped[[2, 1, 0, 3][channel]] as f32 / 255.0, expected.clamp(0.0, 1.0))
                                };
                                assert!(
                                    (actual - expected).abs() < 0.005,
                                    "extended={extended} {working:?}/{output:?} ui={ordinary_ui} {pixel:?} opacity={opacity} channel={channel} actual={actual} expected={expected}"
                                );
                            }
                            drop(mapped);
                            readback.unmap();
                        }
                    }
                }
            }
        }
    }
}
