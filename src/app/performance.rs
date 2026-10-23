//! Live scene profiling. CPU submission, GPU execution and redraw cadence have
//! different scopes and are deliberately kept as separate sample series.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rust_wgpu_fiber::{
    eframe::{egui, wgpu},
    pool::texture_pool::TextureDesc,
    shader_space::{PassProfilePipelineKind, RenderProfile, ShaderSpace},
};

use super::types::App;
use crate::ui::tool_window;

#[cfg(target_os = "macos")]
use objc2_metal::MTLDevice;

const HISTORY: usize = 240;
const REFRESH: Duration = Duration::from_millis(250);
const OPEN_ID: &str = "render-performance-open";

#[cfg(test)]
#[path = "performance_tests.rs"]
mod ui_tests;

#[derive(Clone, Default)]
struct Series(VecDeque<f64>);

impl Series {
    fn push(&mut self, value: f64) {
        if !value.is_finite() || value < 0.0 {
            return;
        }
        if self.0.len() == HISTORY {
            self.0.pop_front();
        }
        self.0.push_back(value);
    }

    fn percentile(&self, quantile: f64) -> Option<f64> {
        let mut values: Vec<_> = self.0.iter().copied().collect();
        if values.is_empty() {
            return None;
        }
        values.sort_by(f64::total_cmp);
        let index = ((values.len() as f64 * quantile).ceil() as usize).saturating_sub(1);
        values.get(index.min(values.len() - 1)).copied()
    }

    fn over_budget(&self, budget: f64) -> usize {
        self.0.iter().filter(|&&ms| ms > budget).count()
    }
}

#[derive(Clone, Default)]
struct Snapshot {
    cpu: Series,
    gpu: Series,
    cadence: Series,
    phases: [Series; 6],
    latest_cpu: Option<RenderProfile>,
    latest_gpu: Option<RenderProfile>,
    age: Option<f64>,
    memory: Vec<String>,
    pending: usize,
    skipped: u64,
    resolution: [u32; 2],
    rebuilds: u64,
    uniform_updates: u64,
}

struct WindowState {
    open: bool,
    frozen: bool,
    reset: bool,
    fps: u32,
    snapshot: Snapshot,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            open: false,
            frozen: false,
            reset: false,
            fps: 60,
            snapshot: Snapshot::default(),
        }
    }
}

#[derive(Default)]
pub(super) struct Performance {
    data: Snapshot,
    last_cadence: Option<f64>,
    last_gpu: Option<Instant>,
    refreshed: Option<Instant>,
    refresh_pending: bool,
    summary: [Option<f64>; 2],
    window: Arc<Mutex<WindowState>>,
}

impl Performance {
    pub fn record_submission(&mut self, profile: &RenderProfile) {
        if profile.passes.is_empty() {
            return;
        }
        self.data
            .cpu
            .push(profile.frame_cpu_encode_ms + profile.submit_cpu_ms);
        self.data.latest_cpu = Some(profile.clone());
        self.refresh_pending = true;
    }

    pub fn record_phases(&mut self, phases: [f64; 6]) {
        for (series, value) in self.data.phases.iter_mut().zip(phases) {
            series.push(value);
        }
    }

    pub fn record_cadence(&mut self, now: f64, continuous: bool) {
        if continuous {
            if let Some(previous) = self.last_cadence {
                self.data.cadence.push((now - previous) * 1000.0);
            }
            self.last_cadence = Some(now);
        } else {
            self.last_cadence = None;
        }
    }

    pub fn reset_scene(&mut self) {
        self.data = Snapshot::default();
        self.last_cadence = None;
        self.last_gpu = None;
        self.refreshed = None;
        self.refresh_pending = true;
        self.summary = [None, None];
    }

    fn handle_window_requests(&mut self, ctx: &egui::Context) {
        let reset = {
            let mut window = self.window.lock().unwrap();
            if ctx
                .data_mut(|data| data.remove_temp::<bool>(egui::Id::new(OPEN_ID)))
                .unwrap_or(false)
            {
                window.open = true;
            }
            std::mem::take(&mut window.reset)
        };
        if reset {
            self.reset_scene();
        }
    }

    fn publish_snapshot(&mut self) {
        self.summary = [self.data.cpu.percentile(0.5), self.data.gpu.percentile(0.5)];
        self.data.age = self.last_gpu.map(|last| last.elapsed().as_secs_f64());
        let mut window = self.window.lock().unwrap();
        if !window.frozen {
            window.snapshot = self.data.clone();
        }
        self.refreshed = Some(Instant::now());
        self.refresh_pending = false;
    }
}

/// Run even on idle UI updates so the last submitted scene gets its GPU results.
pub(super) fn poll(app: &mut App, ctx: &egui::Context) {
    let completed = app.core.shader_space.take_completed_render_profiles();
    let perf = &mut app.runtime.performance;
    for profile in completed {
        if let Some(ms) = profile.frame_gpu_duration_ms {
            perf.data.gpu.push(ms);
            perf.last_gpu = Some(Instant::now());
        }
        perf.data.latest_gpu = Some(profile);
        perf.refresh_pending = true;
    }
    let (pending, skipped) = app.core.shader_space.live_profile_status();
    perf.data.pending = pending;
    perf.data.skipped = skipped;
    if pending > 0 {
        ctx.request_repaint_after(Duration::from_millis(16));
    }
    perf.handle_window_requests(ctx);
    if perf.refreshed.is_none_or(|last| last.elapsed() >= REFRESH) {
        perf.data.memory = resource_memory(&app.core.shader_space);
        perf.data.resolution = app.core.resolution;
        perf.data.rebuilds = app.runtime.pipeline_rebuild_count;
        perf.data.uniform_updates = app.runtime.uniform_only_update_count;
        perf.publish_snapshot();
    }
}

pub(super) fn show_window(app: &App, ctx: &egui::Context) {
    show_performance_window(
        &app.runtime.performance,
        ctx,
        app.core.shader_space.live_profile_status().0,
    );
}

fn show_performance_window(perf: &Performance, ctx: &egui::Context, pending: usize) {
    // A static scene may have submitted its only frame after this update's poll.
    // Keep pumping callbacks and publish the debounced summary even with no window.
    if pending > 0 {
        ctx.request_repaint_after(Duration::from_millis(16));
    } else if perf.refresh_pending {
        ctx.request_repaint_after(REFRESH);
    }
    let state = Arc::clone(&perf.window);
    if !state.lock().unwrap().open {
        return;
    }
    ctx.request_repaint_after(REFRESH);
    let viewport = egui::ViewportId::from_hash_of("render-performance-window");
    ctx.show_viewport_deferred(
        viewport,
        egui::ViewportBuilder::default()
            .with_title("Render Performance")
            .with_inner_size([900.0, 720.0])
            .with_min_inner_size([600.0, 420.0]),
        move |ui, class| {
            let mut state = state.lock().unwrap();
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                state.open = false;
                return;
            }
            if class == egui::ViewportClass::EmbeddedWindow {
                let mut open = true;
                egui::Window::new("Render Performance")
                    .open(&mut open)
                    .resizable(true)
                    .default_size([900.0, 720.0])
                    .show(ui.ctx(), |ui| contents(ui, &mut state));
                state.open = open;
            } else {
                contents(ui, &mut state);
            }
            ui.ctx().request_repaint_after(REFRESH);
        },
    );
    ctx.request_repaint_of(viewport);
}

/// Small canvas summary; clicking opens a separate native performance window.
pub(super) fn draw_summary(app: &App, ui: &egui::Ui, position: egui::Pos2) {
    draw_performance_summary(&app.runtime.performance, ui, position);
}

fn draw_performance_summary(perf: &Performance, ui: &egui::Ui, position: egui::Pos2) {
    let window = perf.window.lock().unwrap();
    let [cpu, gpu] = perf.summary;
    let age = perf.last_gpu.map(|last| last.elapsed().as_secs_f64());
    let stale = age.is_some_and(|age| age > 1.0);
    let budget = 1000.0 / f64::from(window.fps);
    let text = format!(
        "Scene · CPU {}  ·  GPU {}{}  |  {} Hz / {:.2} ms  ↗",
        ms(cpu),
        ms(gpu),
        if stale { " (last)" } else { "" },
        window.fps,
        budget
    );
    let color = if gpu.is_some_and(|gpu| gpu > budget) {
        egui::Color32::from_rgb(255, 184, 100)
    } else {
        egui::Color32::from_rgb(210, 224, 255)
    };
    let galley = ui
        .painter()
        .layout_no_wrap(text, egui::FontId::monospace(11.0), color);
    let rect = egui::Rect::from_min_size(position, galley.size() + egui::vec2(14.0, 10.0));
    ui.painter()
        .rect_filled(rect, 5.0, egui::Color32::from_black_alpha(185));
    ui.painter()
        .galley(position + egui::vec2(7.0, 5.0), galley, color);
    if ui.interact(rect, egui::Id::new(OPEN_ID), egui::Sense::click())
        .on_hover_text("Open Render Performance · rolling median scene CPU encode + submit / GPU pass span. Excludes display presentation.")
        .on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(OPEN_ID), true));
        ui.ctx().request_repaint();
    }
}

fn ms(value: Option<f64>) -> String {
    value
        .map(|v| format!("{v:.2} ms"))
        .unwrap_or_else(|| "—".into())
}

fn contents(ui: &mut egui::Ui, state: &mut WindowState) {
    tool_window::apply(ui);
    tool_window::panel_frame().show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        contents_inner(ui, state);
    });
}

fn contents_inner(ui: &mut egui::Ui, state: &mut WindowState) {
    // Keep vertical wheel events on the page when the pointer is over a wide
    // table. Horizontal trackpad/shift-wheel input still scrolls that table.
    ui.style_mut().always_scroll_the_only_direction = false;
    ui.horizontal(|ui| {
        tool_window::heading(ui, "Render Performance");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            tool_window::badge(
                ui,
                if state.frozen {
                    "Frozen"
                } else {
                    "Live · 4 Hz"
                },
            );
        });
    });
    tool_window::caption(
        ui,
        "Scene render · last 240 samples · CPU and GPU run concurrently",
    );
    ui.add_space(tool_window::GAP);
    ui.horizontal_wrapped(|ui| {
        ui.label("Budget");
        egui::ComboBox::from_id_salt("performance-budget")
            .selected_text(format!("{} Hz", state.fps))
            .show_ui(ui, |ui| {
                for fps in [30, 60, 90, 120, 144, 240] {
                    ui.selectable_value(&mut state.fps, fps, format!("{fps} Hz"));
                }
            });
        ui.checkbox(&mut state.frozen, "Freeze view");
        if tool_window::action(ui, "Reset samples", true).clicked() {
            state.reset = true;
            state.frozen = false;
        }
    });
    ui.add_space(tool_window::GAP);
    ui.separator();
    let data = &state.snapshot;
    let budget = 1000.0 / f64::from(state.fps);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        tool_window::heading(ui, "Frame cost & pacing");
        ui.add_space(4.0);
        egui::ScrollArea::horizontal().id_salt("frame-cost-scroll").show(ui, |ui| {
        egui::Grid::new("frame-cost").striped(true).min_row_height(tool_window::ROW_HEIGHT).spacing([12.0, 0.0]).min_col_width(44.0).show(ui, |ui| {
            for label in ["Scope", "P50", "P95", "P99", "Max", "Over budget", "Samples"] { ui.strong(label); }
            ui.end_row();
            for (label, series) in [("CPU encode + submit", &data.cpu), ("GPU scene span", &data.gpu), ("Redraw interval", &data.cadence)] {
                ui.label(label);
                for q in [0.5, 0.95, 0.99, 1.0] { ui.monospace(ms(series.percentile(q))); }
                ui.monospace(format!("{} / {}", series.over_budget(budget), series.0.len()));
                ui.monospace(series.0.len().to_string());
                ui.end_row();
            }
        });
        });
        if let Some(p95) = data.gpu.percentile(0.95) {
            ui.add(egui::ProgressBar::new((p95 / budget) as f32).text(format!(
                "GPU P95: {:.0}% of {:.2} ms budget (not GPU utilization)", p95 / budget * 100.0, budget)));
        }
        tool_window::caption(ui, "Redraw interval measures scene submissions during continuous playback; it is not display FPS or input latency. Idle gaps are excluded.");
        plot(ui, &data.gpu, budget);
        tool_window::section(ui, "CPU update breakdown");
        egui::Grid::new("cpu-phases").striped(true).min_row_height(tool_window::ROW_HEIGHT).spacing([20.0, 0.0]).show(ui, |ui| {
            for label in ["UI update scope", "P50", "P95"] { ui.strong(label); }
            ui.end_row();
            for (label, series) in ["Total app update", "Scene ingest / rebuild", "Animation + derivation", "Render + analysis scopes", "Canvas UI", "Finalize"].into_iter().zip(&data.phases) {
                ui.label(label);
                ui.monospace(ms(series.percentile(0.5)));
                ui.monospace(ms(series.percentile(0.95)));
                ui.end_row();
            }
        });
        if let Some(profile) = &data.latest_cpu {
            ui.label(format!("Latest scene: encode {:.3} ms · submit {:.3} ms · CPU call {:.3} ms",
                profile.frame_cpu_encode_ms, profile.submit_cpu_ms, profile.frame_wall_ms));
        }
        tool_window::caption(ui, "App update excludes eframe painting, swapchain acquire/present and GPU completion. Phase percentiles are independent and do not add up.");
        tool_window::section(ui, "GPU passes · slowest first");
        ui.label(format!("Completed sample age: {} · readbacks {}/3 · skipped samples {}",
            data.age.map(|v| format!("{v:.2} s")).unwrap_or_else(|| "waiting".into()), data.pending, data.skipped));
        let profile = data.latest_gpu.as_ref().or(data.latest_cpu.as_ref());
        if let Some(profile) = profile {
            if !profile.timestamp_query_supported { ui.label("GPU timestamps unavailable on this device; CPU timings remain available."); }
            if let Some(error) = &profile.timestamp_error { ui.colored_label(egui::Color32::LIGHT_RED, error); }
            let mut passes: Vec<_> = profile.passes.iter().collect();
            passes.sort_by(|a, b| b.gpu_duration_ms.unwrap_or(-1.0).total_cmp(&a.gpu_duration_ms.unwrap_or(-1.0))
                .then_with(|| b.cpu_encode_ms.total_cmp(&a.cpu_encode_ms)));
            let draws = passes.iter().filter(|p| p.pipeline_kind == PassProfilePipelineKind::Render).count();
            let vertices: u64 = passes.iter().map(|p| u64::from(p.vertex_count) * u64::from(p.instance_count)).sum();
            let groups: u64 = passes.iter().map(|p| u64::from(p.workgroup_count)).sum();
            ui.label(format!("{} passes · {draws} draws · {} dispatches · {vertices} vertex/index elements × instances · {groups} workgroups",
                passes.len(), passes.len() - draws));
            egui::ScrollArea::horizontal().id_salt("gpu-passes-scroll").show(ui, |ui| {
            egui::Grid::new("performance-passes").striped(true).min_row_height(tool_window::ROW_HEIGHT).spacing([16.0, 0.0]).show(ui, |ui| {
                for label in ["Pass", "Kind", "GPU", "CPU encode", "Budget"] { ui.strong(label); }
                ui.end_row();
                for pass in passes {
                    ui.label(&pass.pass_name);
                    ui.label(pass.pipeline_kind.as_str());
                    ui.monospace(ms(pass.gpu_duration_ms));
                    ui.monospace(format!("{:.3} ms", pass.cpu_encode_ms));
                    ui.monospace(pass.gpu_duration_ms.map(|v| format!("{:.1}%", v / budget * 100.0)).unwrap_or_else(|| "—".into()));
                    ui.end_row();
                }
            });
            });
        }
        tool_window::caption(ui, "Each row uses CPU and GPU data from the same submission. Passes can overlap: their sum is not the scene span. Preview capture adds overhead; analysis scopes, matrix cells and egui are outside these queries.");
        tool_window::section(ui, "Resources & Metal");
        ui.label(format!("Scene {} × {} · pipeline rebuilds {} · uniform updates {} (session totals)",
            data.resolution[0], data.resolution[1], data.rebuilds, data.uniform_updates));
        for line in &data.memory { ui.label(line); }
        ui.separator();
        egui::CollapsingHeader::new("Metric definitions & deeper GPU analysis").show(ui, |ui| {
            ui.label("GPU span is the envelope of valid scene-pass timestamps, not busy time. Budget percentage is not hardware utilization. Shader invocation counts are only available in the existing synchronous profiler on supported Vulkan/DX12 devices; wgpu does not expose them on Metal.");
            ui.label("Occupancy, ALU/texture limiters, cache misses, overdraw and actual memory bandwidth are not collected here. Use Xcode GPU capture or Instruments Metal System Trace to distinguish shader, fill-rate and bandwidth bottlenecks.");
            ui.hyperlink_to("Apple: live GPU profiling with Metal counters", "https://developer.apple.com/videos/play/tech-talks/10001/");
            ui.hyperlink_to("Apple: interpreting GPU counters", "https://developer.apple.com/videos/play/wwdc2020/10603/");
            ui.hyperlink_to("Unity: rendering profiler metrics", "https://docs.unity3d.com/Manual/ProfilerRendering.html");
        });
    });
}

fn plot(ui: &mut egui::Ui, series: &Series, budget: f64) {
    ui.add_space(tool_window::GAP);
    tool_window::caption(
        ui,
        "GPU scene span history · one point per completed sample",
    );
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(300.0), 100.0),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, 4.0, tool_window::CANVAS);
    let maximum = series.percentile(1.0).unwrap_or(0.0).max(budget) * 1.15;
    let y = |value: f64| rect.bottom() - (value / maximum) as f32 * rect.height();
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), y(budget)),
            egui::pos2(rect.right(), y(budget)),
        ],
        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(180, 120, 60)),
    );
    let points: Vec<_> = series
        .0
        .iter()
        .enumerate()
        .map(|(i, value)| {
            egui::pos2(
                rect.left() + i as f32 / (HISTORY - 1) as f32 * rect.width(),
                y(*value),
            )
        })
        .collect();
    if points.len() > 1 {
        ui.painter().add(egui::Shape::line(
            points,
            egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(100, 190, 255)),
        ));
    }
    ui.painter().text(
        rect.left_top() + egui::vec2(5.0, 3.0),
        egui::Align2::LEFT_TOP,
        format!("0–{maximum:.1} ms · budget {budget:.2} ms"),
        egui::FontId::monospace(10.0),
        egui::Color32::GRAY,
    );
}

fn texture_bytes(desc: &TextureDesc) -> Option<u64> {
    let bytes = u64::from(desc.format.block_copy_size(None)?);
    let (block_w, block_h) = desc.format.block_dimensions();
    let mut total = 0;
    for mip in 0..desc.mip_level_count {
        let w = (desc.size.width >> mip).max(1).div_ceil(block_w);
        let h = (desc.size.height >> mip).max(1).div_ceil(block_h);
        let depth = if desc.dimension == wgpu::TextureDimension::D3 {
            (desc.size.depth_or_array_layers >> mip).max(1)
        } else {
            desc.size.depth_or_array_layers
        };
        total +=
            u64::from(w) * u64::from(h) * u64::from(depth) * bytes * u64::from(desc.sample_count);
    }
    Some(total)
}

fn resource_memory(space: &ShaderSpace) -> Vec<String> {
    let mut texture_total = 0;
    let mut texture_count = 0;
    let mut unknown = 0;
    for texture in space
        .textures
        .values()
        .filter(|t| t.source_texture.is_none() && t.wgpu_texture.is_some())
    {
        texture_count += 1;
        if let Some(bytes) = texture_bytes(&texture.wgpu_texture_desc) {
            texture_total += bytes;
        } else {
            unknown += 1;
        }
    }
    let buffers = space.buffers.lock().unwrap();
    let buffer_count = buffers.values().filter(|b| b.wgpu_buffer.is_some()).count();
    let buffer_total: u64 = buffers
        .values()
        .filter_map(|b| b.wgpu_buffer.as_ref())
        .map(wgpu::Buffer::size)
        .sum();
    let mut lines = vec![format!("Scene resources: {texture_count} textures ≈ {:.2} MiB · {buffer_count} buffers {:.2} MiB · {unknown} textures with unknown byte size",
        texture_total as f64 / 1048576.0, buffer_total as f64 / 1048576.0),
        "Texture estimate includes mips/MSAA; excludes views, driver padding and compression. Not resident VRAM or bandwidth.".into()];
    #[cfg(target_os = "macos")]
    {
        // SAFETY: Read-only queries while the HAL guard retains the device. No
        // resources, command queues or backend synchronization are changed.
        if let Some(device) = unsafe { space.device.as_hal::<wgpu::hal::api::Metal>() } {
            let raw = device.raw_device();
            let allocated = raw.currentAllocatedSize() as f64;
            let recommended = raw.recommendedMaxWorkingSetSize() as f64;
            lines.push(format!(
                "Metal device allocations: {:.2} MiB · recommended working set {:.0} MiB{}",
                allocated / 1048576.0,
                recommended / 1048576.0,
                if recommended > 0.0 {
                    format!(" ({:.1}%)", allocated / recommended * 100.0)
                } else {
                    " (unavailable)".into()
                }
            ));
            lines.push("Metal allocation covers this device's resources, including UI/analysis; recommended working set is not free VRAM or a hard limit.".into());
        } else {
            lines.push("Metal device memory: unavailable on this backend".into());
        }
    }
    #[cfg(not(target_os = "macos"))]
    lines.push("Metal device memory: unavailable on this platform".into());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn performance_percentiles_are_bounded_and_exclude_invalid_samples() {
        let mut series = Series::default();
        assert_eq!(series.percentile(0.95), None);
        for value in 1..=300 {
            series.push(value as f64);
        }
        series.push(f64::NAN);
        series.push(-1.0);
        assert_eq!(series.0.len(), 240);
        assert_eq!(series.percentile(0.5), Some(180.0));
        assert_eq!(series.percentile(0.95), Some(288.0));
        assert_eq!(series.percentile(0.99), Some(298.0));
        assert_eq!(series.over_budget(290.0), 10);
    }

    #[test]
    fn performance_cadence_excludes_idle_and_resets_on_rebuild() {
        let mut perf = Performance::default();
        perf.record_cadence(1.0, true);
        perf.record_cadence(1.01, true);
        perf.record_cadence(2.0, false);
        perf.record_cadence(5.0, true);
        assert_eq!(perf.data.cadence.0.len(), 1);
        assert!((perf.data.cadence.0[0] - 10.0).abs() < 1e-6);
        perf.reset_scene();
        assert!(perf.data.cadence.0.is_empty());
        assert!(perf.last_cadence.is_none());
    }

    #[test]
    fn performance_texture_estimate_accounts_for_blocks_mips_layers_and_samples() {
        let mut desc = TextureDesc {
            size: wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 2,
            },
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            mip_level_count: 4,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            view_formats: vec![],
        };
        assert_eq!(texture_bytes(&desc), Some((64 + 16 + 4 + 1) * 4 * 2));
        desc.mip_level_count = 1;
        desc.sample_count = 4;
        assert_eq!(texture_bytes(&desc), Some(64 * 4 * 2 * 4));
        desc.sample_count = 1;
        desc.format = wgpu::TextureFormat::Bc1RgbaUnorm;
        desc.size.width = 5;
        assert_eq!(texture_bytes(&desc), Some(2 * 2 * 8 * 2));
    }
}
