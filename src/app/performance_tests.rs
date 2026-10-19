//! Exercise the actual egui widgets and deferred viewport callback without a
//! window server. Optional Metal rendering writes the same paint output to PNGs.
use super::*;
use egui::{Event, PointerButton, Pos2, Rect, Vec2, ViewportId};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig, eframe::egui_wgpu};

struct Harness {
    ctx: egui::Context,
    size: Vec2,
    time: f64,
    textures: egui::TexturesDelta,
}

impl Harness {
    fn new(width: f32, height: f32) -> Self {
        let ctx = egui::Context::default();
        crate::ui::fonts::configure_egui_fonts(&ctx);
        ctx.set_visuals(egui::Visuals::dark());
        ctx.global_style_mut(|style| style.animation_time = 0.0);
        Self {
            ctx,
            size: egui::vec2(width, height),
            time: 0.0,
            textures: Default::default(),
        }
    }

    fn frame(
        &mut self,
        events: Vec<Event>,
        mut draw: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        self.time += 1.0 / 60.0;
        let output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| draw(ui),
        );
        self.textures.append(output.textures_delta.clone());
        output
    }

    fn detail(&mut self, perf: &Performance, events: Vec<Event>) -> egui::FullOutput {
        self.frame(events, |ui| contents(ui, &mut perf.window.lock().unwrap()))
    }

    fn click(&mut self, perf: &Performance, position: Pos2) -> egui::FullOutput {
        self.detail(perf, pointer(position, true));
        self.detail(perf, pointer(position, false));
        // Popups need their first sizing pass before egui paints their contents.
        self.detail(perf, vec![])
    }
}

fn pointer(pos: Pos2, pressed: bool) -> Vec<Event> {
    vec![
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        },
    ]
}

fn labels(output: &egui::FullOutput) -> Vec<(String, Rect, Rect)> {
    fn visit(shape: &egui::Shape, clip: Rect, result: &mut Vec<(String, Rect, Rect)>) {
        match shape {
            egui::Shape::Text(text) => result.push((
                text.galley.job.text.clone(),
                shape.visual_bounding_rect(),
                clip,
            )),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    visit(shape, clip, result);
                }
            }
            _ => {}
        }
    }
    let mut result = Vec::new();
    for shape in &output.shapes {
        visit(&shape.shape, shape.clip_rect, &mut result);
    }
    result
}

fn label(output: &egui::FullOutput, text: &str) -> Rect {
    labels(output)
        .into_iter()
        .find(|(label, _, _)| label == text)
        .unwrap_or_else(|| panic!("Missing label {text:?}; labels: {:?}", labels(output)))
        .1
}

fn sample() -> RenderProfile {
    use rust_wgpu_fiber::shader_space::PassProfileSample;
    RenderProfile {
        frame_cpu_encode_ms: 0.35,
        submit_cpu_ms: 0.12,
        frame_wall_ms: 0.47,
        frame_gpu_duration_ms: Some(3.9),
        timestamp_query_supported: true,
        passes: [
            "sys.scene.output.present.sdr.srgb.pass",
            "Blur.horizontal.pass",
            "Blur.vertical.pass",
        ]
        .into_iter()
        .enumerate()
        .map(|(i, name)| PassProfileSample {
            pass_name: name.into(),
            order_index: i,
            pipeline_kind: PassProfilePipelineKind::Render,
            cpu_encode_ms: 0.03 * (i + 1) as f64,
            gpu_duration_ms: Some(0.4 + i as f64),
            vertex_count: 6,
            instance_count: 1,
            workgroup_count: 0,
            vertex_shader_invocations: None,
            clipper_invocations: None,
            clipper_primitives_out: None,
            fragment_shader_invocations: None,
            compute_shader_invocations: None,
            gpu_occupancy_total_percent: None,
            gpu_limiter_alu_percent: None,
            gpu_memory_bandwidth_bytes_per_second: None,
        })
        .collect(),
        ..Default::default()
    }
}

fn populated() -> Performance {
    let mut perf = Performance::default();
    for index in 0..240 {
        let wave = (index as f64 / 10.0).sin();
        perf.data.cpu.push(0.47 + wave * 0.1);
        perf.data
            .gpu
            .push(if index % 57 == 0 { 19.0 } else { 3.9 + wave });
        perf.data.cadence.push(16.67 + wave * 0.03);
        perf.record_phases([1.1, 0.01, 0.2, 0.5, 0.3, 0.01]);
    }
    perf.data.latest_cpu = Some(sample());
    perf.data.latest_gpu = Some(sample());
    perf.data.memory = vec![
        "Scene resources: 8 textures ≈ 31.50 MiB · 12 buffers 0.02 MiB · 0 unknown".into(),
        "Metal device allocations: 68.25 MiB · recommended working set 16384 MiB (0.4%)".into(),
    ];
    perf.data.resolution = [1920, 1080];
    perf.last_gpu = Some(Instant::now());
    perf.publish_snapshot();
    perf
}

#[test]
fn performance_ui_budget_freeze_reset_use_real_pointer_events() {
    let mut harness = Harness::new(900.0, 720.0);
    let mut perf = populated();
    for _ in 0..3 {
        harness.detail(&perf, vec![]);
    }
    let output = harness.detail(&perf, vec![]);
    let popup = harness.click(&perf, label(&output, "60 Hz").center());
    harness.click(&perf, label(&popup, "120 Hz").center());
    assert_eq!(perf.window.lock().unwrap().fps, 120);
    let output = harness.detail(&perf, vec![]);
    assert!(
        labels(&output)
            .iter()
            .any(|(text, _, _)| text.contains("8.33 ms budget"))
    );
    harness.click(&perf, label(&output, "Freeze view").center());
    assert!(perf.window.lock().unwrap().frozen);
    let frozen_median = perf.window.lock().unwrap().snapshot.gpu.percentile(0.5);
    perf.data.gpu = Series::default();
    perf.data.gpu.push(7.0);
    perf.publish_snapshot();
    assert_eq!(
        perf.window.lock().unwrap().snapshot.gpu.percentile(0.5),
        frozen_median
    );
    assert_eq!(
        perf.summary[1],
        Some(7.0),
        "summary keeps updating while detail is frozen"
    );
    let output = harness.detail(&perf, vec![]);
    harness.click(&perf, label(&output, "Reset samples").center());
    assert!(perf.window.lock().unwrap().reset);
    assert!(!perf.window.lock().unwrap().frozen);
    perf.handle_window_requests(&harness.ctx);
    perf.publish_snapshot();
    assert!(perf.window.lock().unwrap().snapshot.gpu.0.is_empty());
    assert_eq!(perf.summary, [None, None]);
}

#[test]
fn performance_summary_opens_native_viewport_and_close_can_reopen() {
    let mut harness = Harness::new(900.0, 720.0);
    harness.ctx.set_embed_viewports(false);
    let mut perf = populated();
    let mut draw = |ui: &mut egui::Ui| {
        draw_performance_summary(&perf, ui, egui::pos2(8.0, 8.0));
        show_performance_window(&perf, ui.ctx(), 0);
    };
    let output = harness.frame(vec![], &mut draw);
    let summary = labels(&output)
        .into_iter()
        .find(|(text, _, _)| text.starts_with("Scene ·"))
        .unwrap()
        .1;
    assert!(!perf.window.lock().unwrap().open);
    harness.frame(pointer(summary.center(), true), &mut draw);
    harness.frame(pointer(summary.center(), false), &mut draw);
    perf.handle_window_requests(&harness.ctx);
    assert!(perf.window.lock().unwrap().open);
    let output = harness.frame(vec![], |ui| show_performance_window(&perf, ui.ctx(), 0));
    let id = ViewportId::from_hash_of("render-performance-window");
    let viewport = output
        .viewport_output
        .get(&id)
        .expect("independent native viewport requested");
    assert!(viewport.class == egui::ViewportClass::Deferred);
    assert_eq!(
        viewport.builder.title.as_deref(),
        Some("Render Performance")
    );
    assert_eq!(viewport.builder.inner_size, Some(egui::vec2(900.0, 720.0)));
    let callback = viewport.viewport_ui_cb.as_ref().unwrap().clone();
    let mut input = egui::RawInput {
        viewport_id: id,
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, harness.size)),
        ..Default::default()
    };
    input.viewports.insert(
        id,
        egui::ViewportInfo {
            parent: Some(ViewportId::ROOT),
            ..Default::default()
        },
    );
    let child = harness.ctx.run_ui(input.clone(), |ui| callback(ui));
    label(&child, "Frame cost & pacing");
    input
        .viewports
        .get_mut(&id)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let _ = harness.ctx.run_ui(input, |ui| callback(ui));
    assert!(!perf.window.lock().unwrap().open);
    harness.frame(vec![], |ui| {
        draw_performance_summary(&perf, ui, egui::pos2(8.0, 8.0))
    });
    harness.frame(pointer(summary.center(), true), |ui| {
        draw_performance_summary(&perf, ui, egui::pos2(8.0, 8.0))
    });
    harness.frame(pointer(summary.center(), false), |ui| {
        draw_performance_summary(&perf, ui, egui::pos2(8.0, 8.0))
    });
    perf.handle_window_requests(&harness.ctx);
    assert!(perf.window.lock().unwrap().open);
}

#[test]
fn performance_closed_window_schedules_static_readback_and_summary_refresh() {
    let mut harness = Harness::new(900.0, 720.0);
    let mut perf = Performance::default();
    for _ in 0..4 {
        harness.frame(vec![], |ui| show_performance_window(&perf, ui.ctx(), 0));
    }
    let idle = harness.frame(vec![], |ui| show_performance_window(&perf, ui.ctx(), 0));
    assert_eq!(
        idle.viewport_output[&ViewportId::ROOT].repaint_delay,
        Duration::MAX
    );
    perf.record_submission(&sample());
    let pending = harness.frame(vec![], |ui| show_performance_window(&perf, ui.ctx(), 1));
    assert!(pending.viewport_output[&ViewportId::ROOT].repaint_delay <= Duration::from_millis(16));
    let publish = harness.frame(vec![], |ui| show_performance_window(&perf, ui.ctx(), 0));
    assert!(publish.viewport_output[&ViewportId::ROOT].repaint_delay <= REFRESH);
    perf.publish_snapshot();
    for _ in 0..4 {
        harness.frame(vec![], |ui| show_performance_window(&perf, ui.ctx(), 0));
    }
    let idle = harness.frame(vec![], |ui| show_performance_window(&perf, ui.ctx(), 0));
    assert_eq!(
        idle.viewport_output[&ViewportId::ROOT].repaint_delay,
        Duration::MAX
    );
}

#[test]
fn performance_window_controls_fit_and_passes_sort_by_gpu_cost() {
    for size in [[900.0, 720.0], [600.0, 420.0]] {
        let mut harness = Harness::new(size[0], size[1]);
        let perf = populated();
        for _ in 0..3 {
            harness.detail(&perf, vec![]);
        }
        let output = harness.detail(&perf, vec![]);
        for text in [
            "Render Performance",
            "60 Hz",
            "Freeze view",
            "Reset samples",
            "Samples",
        ] {
            let rect = label(&output, text);
            assert!(
                rect.max.x <= size[0] && rect.min.x >= 0.0,
                "{text} clipped at {size:?}: {rect:?}"
            );
        }
        let mut trends = 0;
        for clipped in &output.shapes {
            if let egui::Shape::Path(path) = &clipped.shape
                && path.points.len() == HISTORY
            {
                trends += 1;
                let bounds = clipped.shape.visual_bounding_rect();
                assert!(
                    bounds.max.x <= size[0] && bounds.min.x >= 0.0,
                    "trend must fit the viewport even when tables scroll: {bounds:?}"
                );
            }
        }
        assert_eq!(trends, 1, "the full GPU trend must be painted");
        // Scroll down using the same wheel input as the native window.
        let mut scrolled = output;
        for _ in 0..20 {
            scrolled = harness.detail(
                &perf,
                vec![
                    Event::PointerMoved(egui::pos2(350.0, 300.0)),
                    Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        phase: egui::TouchPhase::Move,
                        delta: egui::vec2(0.0, -70.0),
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        // Let egui consume the remaining smoothed wheel delta.
        for _ in 0..30 {
            scrolled = harness.detail(&perf, vec![]);
        }
        let resources = label(&scrolled, "Resources & Metal");
        assert!(
            resources.min.y >= 0.0 && resources.max.y <= size[1],
            "resources must be reachable at {size:?}: {resources:?}"
        );
        let first = label(&scrolled, "Blur.vertical.pass");
        let second = label(&scrolled, "Blur.horizontal.pass");
        assert!(
            first.min.x >= 0.0,
            "vertical scrolling must not scroll pass names sideways"
        );
        assert!(
            first.min.y < second.min.y,
            "highest GPU cost should appear first"
        );
    }
}

#[test]
#[ignore = "requires a native GPU; renders offscreen PNGs without a desktop"]
fn performance_offscreen_metal_snapshots() {
    let gpu = HeadlessRenderer::new(HeadlessRendererConfig {
        backends: wgpu::Backends::all() - wgpu::Backends::NOOP,
        ..Default::default()
    })
    .expect("native headless GPU");
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp/performance-ui");
    std::fs::create_dir_all(&out).unwrap();
    for (name, size, scroll) in [
        ("overview", [900.0, 720.0], false),
        ("resources", [900.0, 720.0], true),
        ("compact", [600.0, 420.0], false),
        ("compact-resources", [600.0, 420.0], true),
    ] {
        let mut harness = Harness::new(size[0], size[1]);
        let perf = populated();
        let mut output = harness.detail(&perf, vec![]);
        for _ in 0..5 {
            output = harness.detail(&perf, vec![]);
        }
        if scroll {
            for _ in 0..20 {
                output = harness.detail(
                    &perf,
                    vec![
                        Event::PointerMoved(egui::pos2(400.0, 400.0)),
                        Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            phase: egui::TouchPhase::Move,
                            delta: egui::vec2(0.0, -100.0),
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
            for _ in 0..30 {
                output = harness.detail(&perf, vec![]);
            }
        }
        render_png(&gpu, &harness, output, &out.join(format!("{name}.png")));
    }
    eprintln!(
        "Offscreen UI snapshots (deterministic test data): {}",
        out.display()
    );
}

fn render_png(
    gpu: &HeadlessRenderer,
    harness: &Harness,
    output: egui::FullOutput,
    path: &std::path::Path,
) {
    let device = &gpu.device;
    let size = wgpu::Extent3d {
        width: harness.size.x as u32,
        height: harness.size.y as u32,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("performance-ui.offscreen"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut renderer =
        egui_wgpu::Renderer::new(device, wgpu::TextureFormat::Rgba8Unorm, Default::default());
    for (id, delta) in &harness.textures.set {
        renderer.update_texture(device, &gpu.queue, *id, delta);
    }
    let jobs = harness
        .ctx
        .tessellate(output.shapes, output.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [size.width, size.height],
        pixels_per_point: output.pixels_per_point,
    };
    let row_bytes = (size.width * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row_bytes) * u64::from(size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    let commands = renderer.update_buffers(device, &gpu.queue, &mut encoder, &jobs, &screen);
    {
        let view = target.create_view(&Default::default());
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
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
            })
            .forget_lifetime();
        renderer.render(&mut pass, &jobs, &screen);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
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
        .map_async(wgpu::MapMode::Read, move |status| {
            let _ = tx.send(status);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(20)),
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    let mapped = readback.slice(..).get_mapped_range().unwrap();
    let bytes: Vec<u8> = mapped
        .chunks_exact(row_bytes as usize)
        .flat_map(|row| row[..size.width as usize * 4].iter().copied())
        .collect();
    assert!(
        bytes.chunks_exact(4).any(|pixel| pixel[0] > 100),
        "UI must contain rendered text"
    );
    image::save_buffer(
        path,
        &bytes,
        size.width,
        size.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
    drop(mapped);
    readback.unmap();
}
