use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use rust_wgpu_fiber::eframe::{egui, egui_wgpu};

use super::{
    calibration::Calibration,
    detection::{self, Assistance},
    gpu::GpuCanvas,
    io::{self, Asset, Assets},
    measure::{self, RgbMeasure},
    model::*,
    rulers,
};
use crate::ui::button::{ButtonOptions, button};

fn action(ui: &mut egui::Ui, label: &str) -> bool {
    button(ui, ButtonOptions::new(label)).clicked()
}
fn screen_point(p: egui::Pos2, rect: egui::Rect) -> Point {
    [(p.x - rect.left()) as f64, (p.y - rect.top()) as f64]
}
fn rect_size(rect: egui::Rect) -> Point {
    [rect.width() as f64, rect.height() as f64]
}
fn to_screen(view: &Viewport, p: Point, rect: egui::Rect) -> egui::Pos2 {
    let p = view.world_to_screen(p, rect_size(rect));
    egui::pos2(rect.left() + p[0] as f32, rect.top() + p[1] as f32)
}

#[derive(Clone)]
enum Pending {
    Close,
    CloseParent,
    Open(PathBuf),
    New,
}
type ImportedImage = (String, String, Arc<Asset>, Result<Assistance, String>);
enum Work {
    Imported(Vec<ImportedImage>, Vec<String>),
    Opened(PathBuf, PixelPerfectoDocument, Assets),
    Saved(PathBuf, PixelPerfectoDocument, Option<Pending>),
    Assisted {
        expected: ImageLayer,
        result: Assistance,
        epoch: u64,
        initial: bool,
    },
    Measured(MeasurementReport),
    Exported(PathBuf),
}
struct MeasurementReport {
    segment: Measurement,
    layer: ImageLayer,
    result: Result<RgbMeasure, String>,
}
struct Job {
    receiver: mpsc::Receiver<anyhow::Result<Work>>,
    progress: Arc<AtomicU64>,
    label: &'static str,
}
#[derive(Clone)]
enum Drag {
    Layer {
        id: u64,
        start: Point,
        offset: [i32; 2],
    },
    Anchor {
        id: u64,
        index: usize,
        /// Cursor position in the shared, corrected grid view.
        raw: Point,
        /// Keep the sampling map stable throughout a gesture, including its cell offset.
        mapping: Calibration,
        offset: Point,
        snapped: bool,
    },
    Guide {
        id: u64,
    },
    Measure {
        endpoint: usize,
    },
    Wipe,
    Roi {
        start: Point,
    },
}

pub struct Workspace {
    pub closed: bool,
    doc: PixelPerfectoDocument,
    assets: Assets,
    history: History,
    saved: PixelPerfectoDocument,
    path: Option<PathBuf>,
    state: egui_wgpu::RenderState,
    canvas: GpuCanvas,
    magnifier: GpuCanvas,
    job: Option<Job>,
    status: String,
    error: bool,
    pending: Option<Pending>,
    calibration: bool,
    fit_lens: bool,
    kind: Subpixel,
    snap_centers: bool,
    auto_points: bool,
    assistance: HashMap<u64, Assistance>,
    refit_requested: Option<u64>,
    analysis_epoch: u64,
    landmark: Option<usize>,
    selected_guide: Option<u64>,
    drag: Option<Drag>,
    edit_before: Option<PixelPerfectoDocument>,
    fine_shift: Point,
    fine_scale: Point,
    export_open: bool,
    select_roi: bool,
    measuring: bool,
    measure_preview: Option<Measurement>,
    measure_snap: bool,
    measure_report: Option<MeasurementReport>,
    last_canvas: Point,
    history_action: bool,
}
impl Workspace {
    pub fn new(state: egui_wgpu::RenderState) -> Self {
        let canvas = GpuCanvas::new(&state.device);
        let magnifier = GpuCanvas::new(&state.device);
        Self {
            closed: false,
            doc: PixelPerfectoDocument::default(),
            assets: Assets::new(),
            history: History::default(),
            saved: PixelPerfectoDocument::default(),
            path: None,
            state,
            canvas,
            magnifier,
            job: None,
            status: "Drop PNG or JPEG images to begin".into(),
            error: false,
            pending: None,
            calibration: false,
            fit_lens: true,
            kind: Subpixel::Red,
            snap_centers: true,
            auto_points: true,
            assistance: HashMap::new(),
            refit_requested: None,
            analysis_epoch: 0,
            landmark: None,
            selected_guide: None,
            drag: None,
            edit_before: None,
            fine_shift: [0., 0.],
            fine_scale: [1., 1.],
            export_open: false,
            select_roi: false,
            measuring: false,
            measure_preview: None,
            measure_snap: false,
            measure_report: None,
            last_canvas: [800., 600.],
            history_action: false,
        }
    }
    fn dirty(&self) -> bool {
        self.doc != self.saved
    }
    pub fn needs_close_confirmation(&self) -> bool {
        self.dirty() || self.job.is_some()
    }
    pub fn request_parent_close(&mut self) {
        self.pending = Some(Pending::CloseParent);
    }
    pub fn request_close(&mut self) {
        self.pending = Some(Pending::Close);
    }
    fn message(&mut self, message: impl Into<String>, error: bool) {
        self.status = message.into();
        self.error = error;
    }
    fn start(
        &mut self,
        ctx: egui::Context,
        label: &'static str,
        task: impl FnOnce(Arc<AtomicU64>) -> anyhow::Result<Work> + Send + 'static,
    ) {
        if self.job.is_some() {
            return;
        }
        let (tx, receiver) = mpsc::channel();
        let progress = Arc::new(AtomicU64::new(0));
        let worker_progress = progress.clone();
        std::thread::spawn(move || {
            let result = task(worker_progress);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.job = Some(Job {
            receiver,
            progress,
            label,
        });
        self.message(label, false);
    }
    pub fn open_paths(&mut self, paths: Vec<PathBuf>, ctx: egui::Context) {
        if paths.is_empty() || self.job.is_some() {
            return;
        }
        if paths.len() == 1 && paths[0].extension().is_some_and(|e| e == "pixelperfecto") {
            self.request(Pending::Open(paths[0].clone()), ctx);
            return;
        }
        self.start(ctx, "Importing images…", move |progress| {
            let total = paths.len();
            let mut ok = vec![];
            let mut errors = vec![];
            for (i, path) in paths.into_iter().enumerate() {
                match io::import(&path) {
                    Ok((name, key, asset)) => {
                        let alignment =
                            detection::auto_align(&asset.image).map_err(|e| format!("{e:#}"));
                        ok.push((name, key, asset, alignment));
                    }
                    Err(e) => errors.push(format!("{e:#}")),
                };
                progress.store(((i + 1) * 100 / total) as u64, Ordering::Relaxed);
            }
            Ok(Work::Imported(ok, errors))
        });
    }
    fn request(&mut self, pending: Pending, ctx: egui::Context) {
        if self.needs_close_confirmation() {
            self.pending = Some(pending);
        } else {
            self.perform(pending, ctx);
        }
    }
    fn reset_gpu_cache(&mut self) {
        let mut renderer = self.state.renderer.write();
        self.canvas.release(&mut renderer);
        self.magnifier.release(&mut renderer);
    }

    fn perform(&mut self, pending: Pending, ctx: egui::Context) {
        match pending {
            Pending::Close => {
                self.closed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Pending::CloseParent => {
                self.closed = true;
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Pending::New => {
                self.reset_gpu_cache();
                self.assistance.clear();
                self.measure_preview = None;
                self.measure_report = None;
                self.selected_guide = None;
                self.analysis_epoch += 1;
                self.doc = PixelPerfectoDocument::default();
                self.saved = self.doc.clone();
                self.assets.clear();
                self.path = None;
                self.history = History::default();
                self.refit_requested = None;
                self.landmark = None;
                self.message("New project", false);
            }
            Pending::Open(path) => self.start(ctx, "Opening project…", move |_| {
                let (doc, assets) = io::open(&path)?;
                Ok(Work::Opened(path, doc, assets))
            }),
        }
    }
    fn save(&mut self, ctx: egui::Context, save_as: bool, after: Option<Pending>) {
        let path = if save_as || self.path.is_none() {
            rfd::FileDialog::new()
                .add_filter("Pixel Perfecto", &["pixelperfecto"])
                .set_file_name("Untitled.pixelperfecto")
                .save_file()
        } else {
            self.path.clone()
        };
        let Some(mut path) = path else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("pixelperfecto");
        }
        let doc = self.doc.clone();
        let assets = self.assets.clone();
        self.start(ctx, "Saving project…", move |_| {
            io::save(&path, &doc, &assets)?;
            Ok(Work::Saved(path, doc, after))
        });
    }
    fn poll(&mut self, ctx: &egui::Context) {
        let result = self
            .job
            .as_ref()
            .and_then(|job| match job.receiver.try_recv() {
                Ok(r) => Some(r),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!(
                    "Background operation stopped unexpectedly"
                ))),
            });
        let Some(result) = result else {
            return;
        };
        self.job = None;
        match result {
            Err(e) => self.message(format!("{e:#}"), true),
            Ok(Work::Imported(imported, errors)) => {
                let before = self.doc.clone();
                let first = self.doc.layers.is_empty();
                let count = imported.len();
                let mut aligned = 0;
                let mut alignment_notes = Vec::new();
                for (name, key, asset, alignment) in imported {
                    let id = self.doc.layers.iter().map(|l| l.id).max().unwrap_or(0) + 1;
                    let size = asset.image.size;
                    let bit_depth = asset.image.bit_depth;
                    let calibration = match alignment {
                        Ok(result) => {
                            aligned += 1;
                            let c = result.calibration.clone();
                            self.assistance.insert(id, result);
                            c
                        }
                        Err(reason) => {
                            alignment_notes.push(format!("{name}: {reason}"));
                            Calibration::initial(size)
                        }
                    };
                    self.assets.insert(key.clone(), asset);
                    self.doc.layers.push(ImageLayer {
                        id,
                        asset: key,
                        name,
                        size,
                        bit_depth,
                        visible: true,
                        locked: false,
                        opacity: 1.,
                        offset: [0, 0],
                        calibration,
                        landmarks: vec![],
                    });
                    self.doc.selected = Some(id);
                }
                self.doc.repair_selection();
                if first {
                    self.fit_view();
                }
                self.history.record(before, &self.doc);
                self.message(if errors.is_empty(){format!("Imported {count} image(s); auto-aligned {aligned}. Verify the cell origin in Calibrate. {}",alignment_notes.join(" · "))}else{errors.join("\n")},!errors.is_empty());
            }
            Ok(Work::Opened(path, doc, assets)) => {
                self.reset_gpu_cache();
                self.assistance.clear();
                self.measure_preview = None;
                self.measure_report = None;
                self.selected_guide = None;
                self.analysis_epoch += 1;
                self.doc = doc;
                self.saved = self.doc.clone();
                self.assets = assets;
                self.path = Some(path);
                self.history = History::default();
                self.landmark = None;
                self.refit_requested = None;
                self.message("Project opened with embedded originals", false);
            }
            Ok(Work::Saved(path, doc, after)) => {
                self.saved = doc;
                self.path = Some(path);
                self.message("Project saved", false);
                if let Some(after) = after {
                    if !self.dirty() {
                        self.pending = None;
                        self.perform(after, ctx.clone());
                    } else {
                        self.message(
                            "Saved snapshot. There are newer edits; save again before closing.",
                            true,
                        );
                    }
                }
            }
            Ok(Work::Assisted {
                expected,
                result,
                epoch,
                initial,
            }) => {
                let before = self.doc.clone();
                let accepted = epoch == self.analysis_epoch
                    && self
                        .doc
                        .layers
                        .iter()
                        .find(|l| l.id == expected.id)
                        .is_some_and(|l| {
                            !l.locked
                                && l.asset == expected.asset
                                && l.calibration == expected.calibration
                                && l.landmarks == expected.landmarks
                        });
                if accepted {
                    self.doc
                        .layers
                        .iter_mut()
                        .find(|l| l.id == expected.id)
                        .unwrap()
                        .calibration = result.calibration.clone();
                    if initial {
                        self.history.record(before, &self.doc);
                    } else {
                        self.history.record_followup(before, &self.doc);
                    }
                    let validation = result
                        .validation_rms
                        .map(|v| format!(" · held-out {:.3} px", v))
                        .unwrap_or_default();
                    self.message(
                        format!(
                            "{} · {} automatic points · fit RMS {:.3} px{validation} · {}",
                            result.model.label(),
                            result.automatic.len(),
                            result.fitted_rms,
                            result.note
                        ),
                        false,
                    );
                    self.assistance.insert(expected.id, result);
                    if initial {
                        self.fit_view();
                    }
                } else {
                    self.message(
                        "Calibration changed during analysis; discarded the stale proposal.",
                        false,
                    );
                }
            }
            Ok(Work::Measured(report)) => {
                if self.doc.measurement == Some(report.segment)
                    && self.doc.layers.iter().any(|l| *l == report.layer)
                {
                    match &report.result {
                        Ok(counts) => self.message(
                            format!(
                                "Image {} pixels · Grid {} pixels",
                                counts.centers.len(),
                                counts.grid_centers.len()
                            ),
                            false,
                        ),
                        Err(error) => self.message(error.clone(), true),
                    }
                    self.measure_report = Some(report);
                }
            }
            Ok(Work::Exported(path)) => self.message(format!("Exported {}", path.display()), false),
        }
    }
    fn fit_view(&mut self) {
        let Some(layer) = self.doc.selected_layer() else {
            return;
        };
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for y in 0..=8 {
            for x in 0..=8 {
                if let Some(p) = layer.source_to_world([
                    layer.size[0] as f64 * x as f64 / 8.,
                    layer.size[1] as f64 * y as f64 / 8.,
                ]) {
                    for a in 0..2 {
                        lo[a] = lo[a].min(p[a]);
                        hi[a] = hi[a].max(p[a]);
                    }
                }
            }
        }
        if lo.iter().chain(hi.iter()).all(|v| v.is_finite()) {
            self.doc.viewport.center = mul(add(lo, hi), 0.5);
            self.doc.viewport.zoom = (self.last_canvas[0] / (hi[0] - lo[0]).max(0.1))
                .min(self.last_canvas[1] / (hi[1] - lo[1]).max(0.1))
                * 0.88;
            self.doc.viewport.zoom = self.doc.viewport.zoom.clamp(0.02, 4096.);
        }
    }
    fn import_dialog(&mut self, ctx: egui::Context) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg"])
            .pick_files()
        {
            self.open_paths(paths, ctx);
        }
    }
    fn open_dialog(&mut self, ctx: egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Pixel Perfecto", &["pixelperfecto"])
            .pick_file()
        {
            self.request(Pending::Open(path), ctx);
        }
    }
    fn prepare_export(&mut self) {
        self.doc.export.bounds = self.doc.viewport.bounds(self.last_canvas);
        self.doc.export.pixels_per_unit = self
            .doc
            .selected_layer()
            .map(|l| {
                l.calibration
                    .density_at(sub(self.doc.viewport.center, l.offset_world()), l.size)
            })
            .unwrap_or(32.);
        self.doc.export.bit_depth = self
            .doc
            .layers
            .iter()
            .map(|l| l.bit_depth)
            .max()
            .unwrap_or(8);
        self.export_open = true;
    }
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if self.closed {
            return;
        }
        let ctx = ui.ctx().clone();
        if self.drag.is_none() && !ctx.input(|i| i.pointer.any_down()) {
            self.poll(&ctx);
        }
        if self
            .selected_guide
            .is_some_and(|id| !self.doc.rulers().guides.iter().any(|g| g.id == id))
        {
            self.selected_guide = None;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.needs_close_confirmation() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.request_close();
            } else {
                self.closed = true;
                return;
            }
        }
        let before = self.doc.clone();
        self.history_action = false;
        self.shortcuts(&ctx);
        if self.job.is_none() {
            let files = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .filter_map(|f| f.path.clone())
                    .collect::<Vec<_>>()
            });
            if !files.is_empty() {
                self.open_paths(files, ctx.clone());
            }
        }
        egui::Panel::top("pp.toolbar").show_inside(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("pp.status").show_inside(ui,|ui|{
            ui.horizontal_wrapped(|ui|{
                if let Some(job)=&self.job{ui.spinner();ui.label(format!("{} {}%",job.label,job.progress.load(Ordering::Relaxed)));}
                else{ui.colored_label(if self.error{egui::Color32::from_rgb(255,125,115)}else{egui::Color32::from_gray(170)},&self.status);}
            });
            ui.small(if self.measuring {"Measure: click A then B, or drag · Drag endpoints to adjust · Shift: horizontal/vertical · M: toggle · Esc: cancel"} else if self.calibration {"Drag a grid point onto its subpixel in the corrected image · Release: update alignment · Alt: bypass center snap · Middle: pan · Wheel: zoom · Esc: cancel"} else {"Middle drag: pan · Wheel: zoom · Left drag: snap image · Esc: cancel · Cmd/Ctrl+Z: undo"});
        });
        egui::Panel::left("pp.layers")
            .default_size(215.)
            .size_range(180.0..=320.)
            .resizable(true)
            .show_inside(ui, |ui| self.layers_panel(ui));
        egui::Panel::right("pp.inspector")
            .default_size(290.)
            .size_range(250.0..=410.)
            .resizable(true)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.inspector(ui));
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| self.canvases(ui));
        self.export_dialog(&ctx);
        self.pending_dialog(&ctx);
        if !self.history_action && before != self.doc && self.edit_before.is_none() {
            self.edit_before = Some(before);
        }
        if !ctx.input(|i| i.pointer.any_down()) && self.drag.is_none() {
            if let Some(before) = self.edit_before.take() {
                self.history.record(before, &self.doc);
            }
        }
        if self.job.is_none() && self.drag.is_none() && !ctx.input(|i| i.pointer.any_down()) {
            if let Some(id) = self.refit_requested.take() {
                self.start_assistance(ctx.clone(), id, false);
            }
        }
        if self.doc.measurement.is_some()
            && self.measure_preview.is_none()
            && self.drag.is_none()
            && self.job.is_none()
            && !ctx.input(|i| i.pointer.any_down())
        {
            self.start_measurement(ctx.clone());
        }
        if self.job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.measuring
                && self.measure_preview.is_none()
                && !matches!(self.drag, Some(Drag::Measure { .. }))
            {
                self.measuring = false;
            }
            self.measure_preview = None;
            if let Some(before) = self.edit_before.take() {
                self.doc = before;
                self.history_action = true;
            }
            self.drag = None;
            self.refit_requested = None;
            self.analysis_epoch += 1;
            self.select_roi = false;
        }
        if ctx.text_edit_focused() {
            return;
        }
        let (command, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        if !command && ctx.input(|i| i.key_pressed(egui::Key::M)) && self.drag.is_none() {
            self.measuring = !self.measuring;
            self.measure_preview = None;
            if self.measuring {
                self.calibration = false;
                self.select_roi = false;
                self.selected_guide = None;
            }
        }
        if shift && !command && ctx.input(|i| i.key_pressed(egui::Key::R)) {
            let visible = self.doc.rulers().visible;
            self.doc.rulers_mut().visible = !visible;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
            if self.measuring {
                self.doc.measurement = None;
                self.measure_preview = None;
            } else {
                self.delete_selected_guide();
            }
        }
        if let Some(id) = self.selected_guide.filter(|_| {
            self.doc.rulers().visible
                && self.doc.rulers().guides_visible
                && !self.doc.rulers().locked
        }) {
            if let Some(guide) = self.doc.rulers_mut().guides.iter_mut().find(|g| g.id == id) {
                let (negative, positive) = if guide.axis == GuideAxis::Vertical {
                    (egui::Key::ArrowLeft, egui::Key::ArrowRight)
                } else {
                    (egui::Key::ArrowDown, egui::Key::ArrowUp)
                };
                let step = if shift {
                    10.
                } else if ctx.input(|i| i.modifiers.alt) {
                    0.1
                } else {
                    1.
                };
                if ctx.input(|i| i.key_pressed(negative)) {
                    guide.position -= step;
                }
                if ctx.input(|i| i.key_pressed(positive)) {
                    guide.position += step;
                }
                guide.position = guide.position.clamp(-1e9, 1e9);
            }
        }
        if command && ctx.input(|i| i.key_pressed(egui::Key::Z)) {
            self.edit_before = None;
            self.drag = None;
            self.measure_preview = None;
            if shift {
                self.history.redo(&mut self.doc);
            } else {
                self.history.undo(&mut self.doc);
            }
            self.history_action = true;
            self.analysis_epoch += 1;
            self.refit_requested = None;
        }
        if self.job.is_none() {
            if command && ctx.input(|i| i.key_pressed(egui::Key::S)) {
                self.save(ctx.clone(), shift, None);
            }
            if command && ctx.input(|i| i.key_pressed(egui::Key::O)) {
                self.open_dialog(ctx.clone());
            }
            if command && ctx.input(|i| i.key_pressed(egui::Key::I)) {
                self.import_dialog(ctx.clone());
            }
        }
        if self.doc.comparison.mode == CompareMode::Ab
            && ctx.input(|i| i.key_pressed(egui::Key::Tab))
        {
            self.doc.comparison.show_b = !self.doc.comparison.show_b;
        }
    }
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong("PIXEL PERFECTO");
            if self.dirty() {
                ui.label("•");
            }
            ui.separator();
            ui.add_enabled_ui(self.job.is_none(), |ui| {
                if action(ui, "New") {
                    self.request(Pending::New, ui.ctx().clone());
                }
                if action(ui, "Open") {
                    self.open_dialog(ui.ctx().clone());
                }
                if action(ui, "Import") {
                    self.import_dialog(ui.ctx().clone());
                }
                if action(ui, "Save") {
                    self.save(ui.ctx().clone(), false, None);
                }
                if action(ui, "Save as") {
                    self.save(ui.ctx().clone(), true, None);
                }
                if action(ui, "Export") {
                    self.prepare_export();
                }
            });
            ui.separator();
            if action(ui, "Undo") {
                self.edit_before = None;
                self.drag = None;
                self.measure_preview = None;
                self.history.undo(&mut self.doc);
                self.analysis_epoch += 1;
                self.refit_requested = None;
                self.history_action = true;
            }
            if action(ui, "Redo") {
                self.edit_before = None;
                self.drag = None;
                self.measure_preview = None;
                self.history.redo(&mut self.doc);
                self.analysis_epoch += 1;
                self.refit_requested = None;
                self.history_action = true;
            }
            ui.separator();
            if ui
                .toggle_value(&mut self.calibration, "Calibrate")
                .changed()
            {
                self.measuring = false;
                self.measure_preview = None;
            }
            if ui
                .toggle_value(&mut self.measuring, "Measure")
                .on_hover_text("Two-point measurement · M")
                .changed()
            {
                self.measure_preview = None;
                self.selected_guide = None;
                if self.measuring {
                    self.calibration = false;
                    self.select_roi = false;
                }
            }
            if action(ui, "Fit view") {
                self.fit_view();
            }
            ui.checkbox(&mut self.doc.nearest, "Nearest");
            let mut rulers = self.doc.rulers().visible;
            if ui
                .checkbox(&mut rulers, "Rulers")
                .on_hover_text("Show rulers and guides · Shift+R")
                .changed()
            {
                self.doc.rulers_mut().visible = rulers;
            }
        });
        ui.horizontal_wrapped(|ui| {
            for mode in CompareMode::ALL {
                ui.selectable_value(&mut self.doc.comparison.mode, mode, mode.label());
            }
            if self.doc.comparison.mode != CompareMode::Overlay {
                for which in [false, true] {
                    let value = if which {
                        &mut self.doc.comparison.b
                    } else {
                        &mut self.doc.comparison.a
                    };
                    let label = if which { "B" } else { "A" };
                    let selected = self
                        .doc
                        .layers
                        .iter()
                        .find(|l| Some(l.id) == *value)
                        .map(|l| l.name.as_str())
                        .unwrap_or("Choose image");
                    egui::ComboBox::from_id_salt(("pp.pair", which))
                        .width(130.)
                        .selected_text(format!("{label}: {selected}"))
                        .show_ui(ui, |ui| {
                            for l in &self.doc.layers {
                                ui.selectable_value(value, Some(l.id), &l.name);
                            }
                        });
                }
            }
            match self.doc.comparison.mode {
                CompareMode::Ab => {
                    ui.toggle_value(&mut self.doc.comparison.show_b, "Show B (Tab)");
                }
                CompareMode::Difference => {
                    ui.label("Gain");
                    ui.add(
                        egui::DragValue::new(&mut self.doc.comparison.gain)
                            .speed(0.1)
                            .range(0.1..=100.),
                    );
                }
                CompareMode::Wipe => {
                    ui.checkbox(&mut self.doc.comparison.horizontal, "Horizontal");
                }
                _ => {}
            }
        });
    }
    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Layers");
        ui.small("Top row is the topmost image");
        ui.separator();
        let mut remove = None;
        let mut reorder = None;
        let layer_count = self.doc.layers.len();
        egui::ScrollArea::vertical()
            .id_salt("pp.layer-scroll")
            .show(ui, |ui| {
                for index in (0..self.doc.layers.len()).rev() {
                    let layer = &mut self.doc.layers[index];
                    ui.push_id(layer.id, |ui| {
                        if ui
                            .selectable_label(self.doc.selected == Some(layer.id), &layer.name)
                            .clicked()
                        {
                            self.doc.selected = Some(layer.id);
                            self.landmark = None;
                            self.selected_guide = None;
                            self.refit_requested = None;
                        }
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut layer.visible, "Visible");
                            ui.checkbox(&mut layer.locked, "Lock");
                        });
                        ui.add(egui::Slider::new(&mut layer.opacity, 0.0..=1.0).text("Opacity"));
                        ui.horizontal(|ui| {
                            if ui.small_button("Up").clicked() && index + 1 < layer_count {
                                reorder = Some((index, index + 1));
                            }
                            if ui.small_button("Down").clicked() && index > 0 {
                                reorder = Some((index, index - 1));
                            }
                            if ui.small_button("Remove").clicked() {
                                remove = Some(index);
                            }
                        });
                        ui.separator();
                    });
                }
            });
        if let Some((a, b)) = reorder {
            self.doc.layers.swap(a, b);
        }
        if let Some(index) = remove {
            self.doc.layers.remove(index);
            self.doc.repair_selection();
            self.landmark = None;
            self.refit_requested = None;
        }
    }
    fn inspector(&mut self, ui: &mut egui::Ui) {
        if self.measuring {
            self.measurement_panel(ui);
            ui.separator();
        }
        ui.collapsing("Rulers & guides",|ui|{
            ui.small("Drag from a ruler. Alt-drag a guide to duplicate. Delete or drag back outside to remove. Shift: whole grid units.");
            ui.small("Shared grid units · X right · Y up");
            let mut settings=self.doc.rulers().clone();let before=settings.clone();
            ui.checkbox(&mut settings.guides_visible,"Show guides");
            ui.checkbox(&mut settings.locked,"Lock guides");
            ui.add_enabled_ui(!settings.locked,|ui|{
                if let Some(guide)=settings.guides.iter_mut().find(|g|Some(g.id)==self.selected_guide) {
                    ui.horizontal(|ui|{ui.label(format!("Guide {}",guide.axis.label()));ui.add(egui::DragValue::new(&mut guide.position).speed(0.01).max_decimals(4).range(-1e9..=1e9));});
                }
                ui.horizontal(|ui|{
                    if action(ui,"Delete selected") {settings.guides.retain(|g|Some(g.id)!=self.selected_guide);self.selected_guide=None;}
                    if action(ui,"Clear guides") {settings.guides.clear();self.selected_guide=None;}
                });
            });
            if settings!=before {self.doc.rulers=Some(settings);}
        });
        ui.separator();
        ui.heading("Diamond grid");
        ui.checkbox(&mut self.doc.grid.visible, "Show grid");
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.doc.grid.lines, "Lines");
            ui.checkbox(&mut self.doc.grid.centers, "Centers");
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.doc.grid.outlines, "Outlines");
            ui.checkbox(&mut self.doc.grid.labels, "Indices");
        });
        ui.add(egui::Slider::new(&mut self.doc.grid.opacity, 0.0..=1.0).text("Opacity"));
        ui.add(egui::Slider::new(&mut self.doc.grid.min_spacing, 4.0..=128.0).text("Min spacing"));
        ui.small("R · B · G1 / · G2 \\   —   u ↗   v ↖");
        ui.separator();
        let Some(mut layer) = self.doc.selected_layer().cloned() else {
            ui.label("Import images to start aligning.");
            return;
        };
        ui.heading("Selected image");
        ui.text_edit_singleline(&mut layer.name);
        ui.small(format!(
            "{} × {} · {} bit",
            layer.size[0], layer.size[1], layer.bit_depth
        ));
        ui.add_enabled_ui(!layer.locked, |ui| {
            ui.horizontal(|ui| {
                ui.label("Cell u / v");
                ui.add(egui::DragValue::new(&mut layer.offset[0]).speed(0.1));
                ui.add(egui::DragValue::new(&mut layer.offset[1]).speed(0.1));
            });
        });
        let mut fit_requested = false;
        let mut initial_requested = false;
        let mut reset_requested = false;
        let points_before = layer.landmarks.clone();
        let mut error = None;
        ui.add_enabled_ui(!layer.locked && self.job.is_none(),|ui|{
            ui.label("Image rotation (°)");
            let current=layer.calibration.image_rotation_degrees(layer.size).unwrap_or(0.);
            let mut angle=current;
            let mut delta=0.;
            ui.horizontal(|ui|{
                if ui.add(egui::DragValue::new(&mut angle).speed(0.1).max_decimals(2).suffix("°"))
                    .on_hover_text("Positive: counterclockwise. Rotates this image around the view center; existing anchors stay fixed as calibration targets.").changed() {delta=(angle-current+180.).rem_euclid(360.)-180.;}
                if action(ui,"↶ 90°"){delta=90.;}
                if action(ui,"↷ 90°"){delta=-90.;}
            });
            if delta!=0. {
                match layer.rotate_image(delta,self.doc.viewport.center) {
                    Ok(())=>{self.refit_requested=None;},Err(e)=>error=Some(format!("{e:#}"))
                }
            }
            if self.calibration {
                ui.separator();ui.heading("Align in place");
                ui.label("Drag a grid point onto its matching center in this corrected image. Release to update the accumulated result here.");
                ui.small("The grid stays fixed. Each correction builds on all existing anchors; inspect the image and residual arrows to find the next area to refine.");
                ui.horizontal(|ui|{for kind in Subpixel::ALL{ui.selectable_value(&mut self.kind,kind,kind.label());}});
                ui.checkbox(&mut self.snap_centers,"Snap to subpixel center (Alt bypass)");
                fit_requested |= ui.checkbox(&mut self.auto_points,"Automatically add reliable points").changed();
                fit_requested |= ui.checkbox(&mut self.fit_lens,"Fit lens when coverage is sufficient").changed();
                ui.horizontal(|ui|{
                    if ui.add_enabled(layer.landmarks.is_empty(),egui::Button::new("Auto initial alignment")).clicked(){initial_requested=true;}
                    if action(ui,"Refine"){fit_requested=true;}
                });
                if let Some(result)=self.assistance.get(&layer.id).filter(|a|a.calibration==layer.calibration && a.anchors==layer.landmarks) {
                    ui.small(format!("{} auto · {} held out · {}",result.automatic.len(),result.held_out.len(),result.model.label()));
                    if let Some(rms)=result.validation_rms {ui.colored_label(if rms>2.5 {egui::Color32::LIGHT_RED}else{egui::Color32::LIGHT_GREEN},format!("Held-out error: {rms:.3} source px"));}
                    let mut coverage=layer.landmarks.clone();coverage.extend_from_slice(&result.automatic);
                    ui.small(detection::coverage_hint(&coverage,layer.size));
                } else {ui.small(detection::coverage_hint(&layer.landmarks,layer.size));}
                ui.small("A periodic pattern cannot identify the same absolute cell across photos. Verify a common anchor.");
                ui.small("Cyan: automatic · Purple: held out · Red: check alignment. Arrows point from image centers to grid targets.");
                ui.label("Manual anchors (drag to correct)");
                if let Some(hint)=detection::anchor_geometry_hint(&layer.landmarks) {ui.colored_label(egui::Color32::YELLOW,hint);}
                let residuals=layer.calibration.residuals(&layer.landmarks,layer.size);let count=residuals.iter().flatten().count();
                if count>0 {let rms=(residuals.iter().flatten().map(|r|length(*r).powi(2)).sum::<f64>()/count as f64).sqrt();ui.label(format!("{} active points · RMS {:.3} px",count,rms));}
                let mut remove=None;
                egui::ScrollArea::vertical().id_salt("pp.points").max_height(200.).show(ui,|ui|{
                    for (i,p) in layer.landmarks.iter_mut().enumerate() {
                        ui.horizontal(|ui|{
                            ui.checkbox(&mut p.enabled,"");
                            let residual=residuals[i].map(|r|format!("{:.2}px",length(r))).unwrap_or_else(||"off".into());
                            if ui.selectable_label(self.landmark==Some(i),format!("{} {} ({},{}) {residual}",i+1,p.target.kind.label(),p.target.cell[0],p.target.cell[1])).clicked(){self.landmark=Some(i);}
                            if ui.small_button("×").clicked(){remove=Some(i);}
                        });
                    }
                });
                if let Some(i)=remove{layer.landmarks.remove(i);self.landmark=None;}
                if let Some(p)=self.landmark.and_then(|i|layer.landmarks.get_mut(i)) {
                    ui.label("Selected correspondence");
                    ui.horizontal(|ui|{ui.label("Source x/y");ui.add(egui::DragValue::new(&mut p.source[0]).speed(0.05).range(0.0..=layer.size[0] as f64));ui.add(egui::DragValue::new(&mut p.source[1]).speed(0.05).range(0.0..=layer.size[1] as f64));});
                    ui.horizontal(|ui|{ui.label("Grid u/v");ui.add(egui::DragValue::new(&mut p.target.cell[0]));ui.add(egui::DragValue::new(&mut p.target.cell[1]));});
                    egui::ComboBox::from_id_salt("pp.point-kind").selected_text(p.target.kind.label()).show_ui(ui,|ui|{for kind in Subpixel::ALL{ui.selectable_value(&mut p.target.kind,kind,kind.label());}});
                }
            }
            ui.separator();
            ui.collapsing("Fine alignment",|ui|{
                ui.label("Continuous adjustments; image dragging stays snapped.");
                if action(ui,"Reset alignment and anchors") {
                    layer.calibration=Calibration::initial(layer.size);layer.landmarks.clear();
                    self.assistance.remove(&layer.id);self.landmark=None;self.refit_requested=None;reset_requested=true;
                }
                ui.horizontal(|ui|{ui.label("Move x/y");ui.add(egui::DragValue::new(&mut self.fine_shift[0]).speed(0.01));ui.add(egui::DragValue::new(&mut self.fine_shift[1]).speed(0.01));});
                ui.horizontal(|ui|{ui.label("Scale x/y");ui.add(egui::DragValue::new(&mut self.fine_scale[0]).speed(0.001).range(0.01..=100.));ui.add(egui::DragValue::new(&mut self.fine_scale[1]).speed(0.001).range(0.01..=100.));});
                if action(ui,"Apply adjustment") {
                    let mut c=layer.calibration.clone();c.adjust(self.fine_shift,0.,self.fine_scale);
                    match c.validate(layer.size){Ok(())=>{layer.calibration=c;self.fine_shift=[0.,0.];self.fine_scale=[1.,1.];},Err(e)=>error=Some(format!("{e:#}"))}
                }
            });
            ui.collapsing("Lens & perspective parameters",|ui|{
                let mut c=layer.calibration.clone();let mut changed=false;
                for (index,label) in ["Radial k1","Radial k2","Tangential p1","Tangential p2"].into_iter().enumerate(){ui.horizontal(|ui|{ui.label(label);changed|=ui.add(egui::DragValue::new(&mut c.lens[index]).speed(0.0001).max_decimals(6)).changed();});}
                ui.horizontal(|ui|{ui.label("Center x/y");for v in &mut c.center{changed|=ui.add(egui::DragValue::new(v).speed(0.25)).changed();}});
                ui.label("Perspective (homography)");
                egui::Grid::new("pp.homography").num_columns(3).show(ui,|ui|{for i in 0..9{changed|=ui.add_enabled(i!=8,egui::DragValue::new(&mut c.h[i]).speed(0.00001).max_decimals(7)).changed();if i%3==2{ui.end_row();}}});
                if changed {match c.validate(layer.size){Ok(())=>layer.calibration=c,Err(e)=>error=Some(format!("{e:#}"))}}
            });
        });
        if let Some(actual) = self.doc.layers.iter_mut().find(|l| l.id == layer.id) {
            *actual = layer.clone();
        }
        if let Some(error) = error {
            self.message(error, true);
        }
        if initial_requested {
            self.start_assistance(ui.ctx().clone(), layer.id, true);
        } else if !reset_requested && (fit_requested || points_before != layer.landmarks) {
            self.refit_requested = Some(layer.id);
        }
    }
    fn start_assistance(&mut self, ctx: egui::Context, id: u64, initial: bool) {
        if self.job.is_some() {
            return;
        }
        let Some(layer) = self
            .doc
            .layers
            .iter()
            .find(|l| l.id == id && !l.locked)
            .cloned()
        else {
            return;
        };
        let Some(asset) = self.assets.get(&layer.asset).cloned() else {
            return;
        };
        let epoch = self.analysis_epoch;
        let lens = self.fit_lens;
        let complete = self.auto_points;
        self.start(ctx, "Aligning subpixels…", move |_| {
            let result = if initial && layer.landmarks.is_empty() {
                detection::auto_align(&asset.image)?
            } else {
                detection::refine(
                    &asset.image,
                    &layer.landmarks,
                    &layer.calibration,
                    lens,
                    complete,
                )?
            };
            Ok(Work::Assisted {
                expected: layer,
                result,
                epoch,
                initial,
            })
        });
    }
    fn canvases(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_rect_before_wrap();
        if available.width() < 8. || available.height() < 8. {
            return;
        }
        if self.doc.layers.is_empty() {
            self.canvas(ui, available);
            ui.painter().text(
                available.center() - egui::vec2(0., 24.),
                egui::Align2::CENTER_CENTER,
                "Pixel Perfecto",
                egui::FontId::proportional(32.),
                egui::Color32::WHITE,
            );
            ui.painter().text(
                available.center() + egui::vec2(0., 22.),
                egui::Align2::CENTER_CENTER,
                "Drop microscope photos here, or choose Import",
                egui::FontId::proportional(16.),
                egui::Color32::from_gray(150),
            );
            return;
        }
        self.canvas(ui, available);
    }
    fn canvas(&mut self, ui: &mut egui::Ui, outer: egui::Rect) {
        let body = egui::Rect::from_min_max(outer.min + egui::vec2(0., 26.), outer.max);
        let layout = rulers::Layout::new(body, self.doc.rulers().visible);
        let rect = layout.canvas;
        if rect.width() < 1. || rect.height() < 1. {
            return;
        }
        ui.painter()
            .rect_filled(outer, 0., egui::Color32::from_gray(17));
        let title = if self.measuring {
            "MEASURE · common grid / selected image"
        } else if self.calibration {
            "ALIGNMENT · accumulated correction + fixed grid"
        } else {
            "SHARED GRID"
        };
        ui.painter().text(
            outer.min + egui::vec2(10., 13.),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(11.),
            egui::Color32::from_gray(165),
        );
        self.last_canvas = rect_size(rect);
        self.ruler_input(ui, layout);
        let response = ui.interact(
            rect,
            ui.id().with("pp.canvas"),
            egui::Sense::click_and_drag(),
        );
        self.canvas_input(ui, &response, rect);
        self.update_guide_drag(ui.ctx(), rect);
        if response.secondary_clicked() {
            self.selected_guide = response
                .interact_pointer_pos()
                .and_then(|p| rulers::hit(self.doc.rulers(), &self.doc.viewport, rect, p));
        }
        if self.selected_guide.is_some() && !self.doc.rulers().locked {
            response.context_menu(|ui| {
                if action(ui, "Remove guide") {
                    self.delete_selected_guide();
                    ui.close();
                }
            });
        }
        let mut preview = self.doc.clone();
        let view = self.doc.viewport.clone();
        if self.calibration {
            preview.layers = self.doc.selected_layer().cloned().into_iter().collect();
            preview.comparison.mode = CompareMode::Overlay;
            for l in &mut preview.layers {
                l.visible = true;
                l.opacity = 1.;
            }
        }
        preview.viewport = view.clone();
        let result = self
            .canvas
            .show(&self.state, &preview, &self.assets, &view, rect, ui);
        if let Err(error) = result {
            self.message(format!("{error:#}"), true);
        }
        let painter = ui.painter().with_clip_rect(rect);
        if self.doc.grid.visible && self.doc.grid.labels {
            self.grid_labels(&painter, rect, &view);
        }
        rulers::paint_guides(
            &painter,
            rect,
            &view,
            self.doc.rulers(),
            self.selected_guide,
        );
        if self.calibration && !self.measuring {
            self.measurements_overlay(&painter, rect, &view);
            self.landmarks_overlay(&painter, rect, &view);
        } else if self.doc.comparison.mode == CompareMode::Wipe {
            let c = &self.doc.comparison;
            let line = if c.horizontal {
                let y = rect.top() + rect.height() * c.wipe;
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)]
            } else {
                let x = rect.left() + rect.width() * c.wipe;
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())]
            };
            painter.line_segment(line, egui::Stroke::new(1.5_f32, egui::Color32::WHITE));
            painter.circle_filled(line[0].lerp(line[1], 0.5), 5., egui::Color32::WHITE);
        }
        if self.select_roi || matches!(self.drag, Some(Drag::Roi { .. })) {
            let b = self.doc.export.bounds;
            let a = to_screen(&view, [b[0], b[1]], rect);
            let b = to_screen(&view, [b[2], b[3]], rect);
            painter.rect_stroke(
                egui::Rect::from_two_pos(a, b),
                0.,
                egui::Stroke::new(2.0_f32, egui::Color32::LIGHT_BLUE),
                egui::StrokeKind::Inside,
            );
        }
        if self.calibration {
            if let Some(Drag::Anchor {
                index,
                raw,
                snapped,
                ..
            }) = self.drag.clone()
            {
                if let Some(point) = self
                    .doc
                    .selected_layer()
                    .and_then(|l| l.landmarks.get(index))
                    .cloned()
                {
                    let size = 148_f32.min(rect.width() * 0.4).min(rect.height() * 0.4);
                    let loupe = egui::Rect::from_min_size(
                        rect.right_top() + egui::vec2(-size - 12., 12.),
                        egui::vec2(size, size),
                    );
                    let layer = self.doc.selected_layer().unwrap();
                    let density = layer
                        .sample_position(raw)
                        .zip(layer.sample_position(add(raw, [1., 0.])))
                        .map(|(a, b)| length(sub(a, b)))
                        .unwrap_or(32.);
                    let measured = layer.source_to_world(point.source);
                    let magnified = Viewport {
                        center: raw,
                        zoom: (density * 6.).max(view.zoom * 2.).clamp(0.02, 4096.),
                    };
                    preview.nearest = true;
                    if let Err(e) = self.magnifier.show(
                        &self.state,
                        &preview,
                        &self.assets,
                        &magnified,
                        loupe,
                        ui,
                    ) {
                        self.message(format!("{e:#}"), true);
                    }
                    let overlay = ui.painter().with_clip_rect(loupe);
                    overlay.rect_stroke(
                        loupe,
                        0.,
                        egui::Stroke::new(2.0_f32, egui::Color32::GRAY),
                        egui::StrokeKind::Inside,
                    );
                    let center = to_screen(&magnified, measured.unwrap_or(raw), loupe);
                    let color = if snapped {
                        egui::Color32::LIGHT_GREEN
                    } else {
                        egui::Color32::YELLOW
                    };
                    overlay.line_segment(
                        [center - egui::vec2(8., 0.), center + egui::vec2(8., 0.)],
                        egui::Stroke::new(1.0_f32, color),
                    );
                    overlay.line_segment(
                        [center - egui::vec2(0., 8.), center + egui::vec2(0., 8.)],
                        egui::Stroke::new(1.0_f32, color),
                    );
                    overlay.text(
                        loupe.left_bottom() + egui::vec2(5., -5.),
                        egui::Align2::LEFT_BOTTOM,
                        format!(
                            "{} ({},{}) {}",
                            point.target.kind.label(),
                            point.target.cell[0],
                            point.target.cell[1],
                            if snapped { "snapped" } else { "manual" }
                        ),
                        egui::FontId::monospace(11.),
                        color,
                    );
                }
            }
        }
        if let Some(p) = response.hover_pos() {
            let w = view.screen_to_world(screen_point(p, rect), rect_size(rect));
            let offset = self
                .doc
                .selected_layer()
                .map(|l| l.offset_world())
                .unwrap_or([0., 0.]);
            let site = GridSite::nearest(sub(w, offset), self.kind);
            let text = format!(
                "{} ({}, {}) · {:.1}×",
                site.kind.label(),
                site.cell[0],
                site.cell[1],
                view.zoom
            );
            if self.calibration && self.doc.grid.visible && self.drag.is_none() {
                if let Some(layer) = self.doc.selected_layer().filter(|l| !l.locked) {
                    let target = add(site.world(), layer.offset_world());
                    let pos = to_screen(&view, target, rect);
                    if pos.distance(p) < 12. && layer.contains(target) {
                        painter.circle_stroke(
                            pos,
                            6.,
                            egui::Stroke::new(1.5_f32, egui::Color32::YELLOW),
                        );
                    }
                }
            }
            painter.text(
                rect.left_bottom() + egui::vec2(8., -8.),
                egui::Align2::LEFT_BOTTOM,
                text,
                egui::FontId::monospace(11.),
                egui::Color32::LIGHT_GRAY,
            );
        }
        if self.measuring || self.doc.measurement.is_some() {
            self.paint_measurement(&painter, rect, &view);
        }
        rulers::paint_rulers(
            ui.painter(),
            layout,
            &view,
            ui.ctx().input(|i| i.pointer.hover_pos()),
        );
    }
    fn measurement_point(&self, p: Point, opposite: Option<Point>, ctx: &egui::Context) -> Point {
        let mut p = if self.measure_snap && !ctx.input(|i| i.modifiers.alt) {
            cell_to_world(world_to_cell(p).map(|v| (v * 2.).round() * 0.5))
        } else {
            p
        };
        if let Some(a) = opposite.filter(|_| ctx.input(|i| i.modifiers.shift)) {
            if (p[0] - a[0]).abs() >= (p[1] - a[1]).abs() {
                p[1] = a[1];
            } else {
                p[0] = a[0];
            }
        }
        p.map(|v| v.clamp(-1e9, 1e9))
    }
    fn measurement_input(&mut self, ui: &egui::Ui, response: &egui::Response, rect: egui::Rect) {
        let ctx = ui.ctx();
        if response.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if self
            .drag
            .as_ref()
            .is_some_and(|d| !matches!(d, Drag::Measure { .. }))
        {
            return;
        }
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos());
        let world = pointer.map(|p| rulers::world_at(&self.doc.viewport, rect, p));
        if let (Some(preview), Some(p)) = (self.measure_preview, world) {
            self.measure_preview = Some(Measurement {
                b: self.measurement_point(p, Some(preview.a), ctx),
                ..preview
            });
        }
        if response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(press) = ctx.input(|i| i.pointer.press_origin()) {
                self.edit_before.get_or_insert_with(|| self.doc.clone());
                let start = self.measurement_point(
                    rulers::world_at(&self.doc.viewport, rect, press),
                    None,
                    ctx,
                );
                let endpoint = if let Some(preview) = self.measure_preview.take() {
                    self.doc.measurement = Some(preview);
                    1
                } else if let Some((index, _)) = self.doc.measurement.and_then(|m| {
                    [m.a, m.b]
                        .into_iter()
                        .enumerate()
                        .map(|(i, p)| (i, to_screen(&self.doc.viewport, p, rect).distance(press)))
                        .filter(|(_, d)| *d < 10.)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                }) {
                    index
                } else {
                    let mut segment = Measurement::new(start, start);
                    if let Some(previous) = self.doc.measurement {
                        segment.threshold = previous.threshold;
                    }
                    self.doc.measurement = Some(segment);
                    1
                };
                self.drag = Some(Drag::Measure { endpoint });
            }
        }
        if let (Some(Drag::Measure { endpoint }), Some(p), Some(mut segment)) =
            (self.drag.clone(), world, self.doc.measurement)
        {
            let opposite = if endpoint == 0 { segment.b } else { segment.a };
            let p = self.measurement_point(p, Some(opposite), ctx);
            if endpoint == 0 {
                segment.a = p;
            } else {
                segment.b = p;
            }
            self.doc.measurement = Some(segment);
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            self.drag = None;
        }
        if response.clicked_by(egui::PointerButton::Primary) {
            if let Some(p) = world {
                if let Some(preview) = self.measure_preview.take() {
                    self.doc.measurement = Some(Measurement {
                        b: self.measurement_point(p, Some(preview.a), ctx),
                        ..preview
                    });
                } else {
                    let p = self.measurement_point(p, None, ctx);
                    let mut preview = Measurement::new(p, p);
                    if let Some(previous) = self.doc.measurement {
                        preview.threshold = previous.threshold;
                    }
                    self.measure_preview = Some(preview);
                }
            }
        }
    }
    fn start_measurement(&mut self, ctx: egui::Context) {
        let Some(segment) = self.doc.measurement else {
            return;
        };
        let Some(layer) = self.doc.selected_layer().cloned() else {
            return;
        };
        if self
            .measure_report
            .as_ref()
            .is_some_and(|r| r.segment == segment && r.layer == layer)
        {
            return;
        }
        let Some(asset) = self.assets.get(&layer.asset).cloned() else {
            return;
        };
        self.start(ctx, "Counting pixels along the line…", move |_| {
            let result =
                measure::count_rgb(segment, &layer, &asset.image).map_err(|e| e.to_string());
            Ok(Work::Measured(MeasurementReport {
                segment,
                layer,
                result,
            }))
        });
    }
    fn measurement_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Measure");
        ui.small("Click A then B, or drag. Drag either endpoint to adjust. Shift: horizontal / vertical.");
        ui.checkbox(
            &mut self.measure_snap,
            "Snap endpoints to grid centers (Alt bypass)",
        );
        if action(ui, "Clear measurement") {
            self.doc.measurement = None;
            self.measure_preview = None;
        }
        if let Some(m) = self
            .doc
            .measurement
            .as_mut()
            .filter(|_| self.measure_preview.is_none())
        {
            ui.horizontal(|ui| {
                ui.label("A x/y");
                for v in &mut m.a {
                    ui.add(egui::DragValue::new(v).speed(0.01).range(-1e9..=1e9));
                }
            });
            ui.horizontal(|ui| {
                ui.label("B x/y");
                for v in &mut m.b {
                    ui.add(egui::DragValue::new(v).speed(0.01).range(-1e9..=1e9));
                }
            });
            ui.add(egui::Slider::new(&mut m.threshold, 0.01..=0.8).text("Image threshold"));
        }
        let Some(m) = self.measure_preview.or(self.doc.measurement) else {
            ui.small("Draw a line across the pixels you want to count.");
            return;
        };
        let Some(layer) = self.doc.selected_layer() else {
            ui.small("Select an image to compare its pixels against the grid.");
            return;
        };
        ui.label(format!("Image: {}", layer.name));
        if let Some(report) = self
            .measure_report
            .as_ref()
            .filter(|r| r.segment == m && r.layer == *layer && self.measure_preview.is_none())
        {
            match &report.result {
                Ok(rgb) => {
                    let actual = rgb.centers.len();
                    let expected = rgb.grid_centers.len();
                    ui.heading(format!("Image pixels: {actual}"));
                    ui.monospace(format!(
                        "R {}   G {}   B {}",
                        rgb.detected[0], rgb.detected[1], rgb.detected[2]
                    ));
                    ui.heading(format!("Grid pixels: {expected}"));
                    ui.monospace(format!(
                        "R {}   G {}   B {}",
                        rgb.expected[0],
                        rgb.expected[2] + rgb.expected[3],
                        rgb.expected[1]
                    ));
                    ui.strong(format!(
                        "Image − grid: {:+}",
                        actual as isize - expected as isize
                    ));
                    ui.small("I1, I2… mark image pixels. G1, G2… mark grid pixels. Each crossed subpixel counts once, including pixels at A/B.");
                    self.measure_count_track(ui, m, &rgb.centers, "Image");
                    self.measure_count_track(ui, m, &rgb.grid_centers, "Grid");
                    if rgb.outside {
                        ui.colored_label(egui::Color32::YELLOW,"Part of the line is outside this image; image count covers only the visible part.");
                    }
                }
                Err(e) => {
                    ui.colored_label(egui::Color32::YELLOW, e);
                }
            }
        } else {
            ui.small(if self.measure_preview.is_some() {
                "Set B to count image and grid pixels."
            } else {
                "Counting pixels…"
            });
        }
        ui.separator();
        ui.small("Image counting scans the RGB signal along the line, independently of the grid. Lower the threshold for dim pixels; raise it to reject noise. Check the numbered marks for missed or merged pixels.");
        ui.small("Grid counting uses the RGB subpixel shapes, regardless of grid visibility/zoom. It does not count guide-line intersections or photograph px distances.");
    }
    fn measure_count_track(
        &self,
        ui: &mut egui::Ui,
        m: Measurement,
        hits: &[(Point, Subpixel)],
        label: &str,
    ) {
        ui.small(label);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let a = rect.left_center() + egui::vec2(5., 0.);
        let b = rect.right_center() - egui::vec2(5., 0.);
        painter.line_segment([a, b], egui::Stroke::new(1_f32, egui::Color32::GRAY));
        let d = sub(m.b, m.a);
        let len2 = length(d).powi(2).max(1e-20);
        for (index, (p, kind)) in hits.iter().enumerate() {
            let q = sub(*p, m.a);
            let t = ((q[0] * d[0] + q[1] * d[1]) / len2).clamp(0., 1.);
            let at = a.lerp(b, t as f32);
            let rgb = kind.color();
            let color = egui::Color32::from_rgb(
                (rgb[0] * 255.) as u8,
                (rgb[1] * 255.) as u8,
                (rgb[2] * 255.) as u8,
            );
            painter.line_segment(
                [at - egui::vec2(0., 5.), at + egui::vec2(0., 5.)],
                egui::Stroke::new(2_f32, color),
            );
            if hits.len() <= 16 || index % (hits.len() / 12).max(1) == 0 {
                painter.text(
                    at - egui::vec2(0., 5.),
                    egui::Align2::CENTER_BOTTOM,
                    (index + 1).to_string(),
                    egui::FontId::monospace(9.),
                    color,
                );
            }
        }
    }
    fn paint_measurement(&self, painter: &egui::Painter, rect: egui::Rect, view: &Viewport) {
        let Some(m) = self.measure_preview.or(self.doc.measurement) else {
            return;
        };
        let a = to_screen(view, m.a, rect);
        let b = to_screen(view, m.b, rect);
        let direction = b - a;
        let normal = if direction.length() > 0.01 {
            egui::vec2(-direction.y, direction.x).normalized()
        } else {
            egui::vec2(0., 1.)
        };
        let color = egui::Color32::from_rgb(80, 215, 255);
        painter.line_segment([a, b], egui::Stroke::new(1.5_f32, color));
        let mut text = "Set B to count pixels".to_string();
        if let Some(layer) = self.doc.selected_layer() {
            if let Some(report) = self
                .measure_report
                .as_ref()
                .filter(|r| r.segment == m && r.layer == *layer)
            {
                if let Ok(rgb) = &report.result {
                    text = format!(
                        "Image {} · Grid {} · Δ {:+}",
                        rgb.centers.len(),
                        rgb.grid_centers.len(),
                        rgb.centers.len() as isize - rgb.grid_centers.len() as isize
                    );
                    for (hits, prefix, side) in
                        [(&rgb.centers, "I", -1.), (&rgb.grid_centers, "G", 1.)]
                    {
                        for (index, (p, kind)) in hits.iter().enumerate() {
                            let at = to_screen(view, *p, rect);
                            let [r, g, b] = kind.color();
                            let c = egui::Color32::from_rgb(
                                (r * 255.) as u8,
                                (g * 255.) as u8,
                                (b * 255.) as u8,
                            );
                            if side < 0. {
                                painter.circle_filled(at, 3., c);
                            } else {
                                painter.rect_stroke(
                                    egui::Rect::from_center_size(at, egui::vec2(8., 8.)),
                                    0.,
                                    egui::Stroke::new(1.5_f32, c),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            if hits.len() <= 40 || index % (hits.len() / 30).max(1) == 0 {
                                let label = painter.layout_no_wrap(
                                    format!("{prefix}{}", index + 1),
                                    egui::FontId::monospace(10.),
                                    egui::Color32::WHITE,
                                );
                                let box_rect = egui::Rect::from_center_size(
                                    at + normal * (side * 21.),
                                    label.size() + egui::vec2(4., 2.),
                                );
                                painter.rect_filled(
                                    box_rect,
                                    2.,
                                    egui::Color32::from_black_alpha(205),
                                );
                                painter.galley(
                                    box_rect.min + egui::vec2(2., 1.),
                                    label,
                                    egui::Color32::WHITE,
                                );
                            }
                        }
                    }
                }
            } else if self.measure_preview.is_none() {
                text = "Counting image / grid pixels…".into();
            }
        }
        for (p, label) in [(a, "A"), (b, "B")] {
            painter.circle_filled(p, 5., egui::Color32::from_gray(20));
            painter.circle_stroke(p, 5., egui::Stroke::new(2_f32, color));
            painter.text(
                p + egui::vec2(9., -9.),
                egui::Align2::LEFT_BOTTOM,
                label,
                egui::FontId::monospace(13.),
                color,
            );
        }
        let galley =
            painter.layout_no_wrap(text, egui::FontId::monospace(12.), egui::Color32::WHITE);
        let size = galley.size() + egui::vec2(12., 8.);
        let pos = (a.lerp(b, 0.5) + egui::vec2(8., 28.)).clamp(
            rect.min,
            egui::pos2(
                (rect.right() - size.x).max(rect.left()),
                (rect.bottom() - size.y).max(rect.top()),
            ),
        );
        painter.rect_filled(
            egui::Rect::from_min_size(pos, size),
            3.,
            egui::Color32::from_rgba_unmultiplied(10, 25, 35, 235),
        );
        painter.galley(pos + egui::vec2(6., 4.), galley, egui::Color32::WHITE);
    }
    fn delete_selected_guide(&mut self) {
        if self.doc.rulers().locked
            || !self.doc.rulers().visible
            || !self.doc.rulers().guides_visible
        {
            return;
        }
        if let Some(id) = self.selected_guide.take() {
            self.doc.rulers_mut().guides.retain(|g| g.id != id);
            if matches!(self.drag, Some(Drag::Guide { id: dragged }) if dragged == id) {
                self.drag = None;
            }
        }
    }
    fn begin_guide(
        &mut self,
        axis: GuideAxis,
        position: f64,
        existing: Option<u64>,
        duplicate: bool,
    ) {
        if self.doc.rulers().locked {
            return;
        }
        if (existing.is_none() || duplicate) && self.doc.rulers().guides.len() >= 1000 {
            self.message("Limit of 1,000 guides reached", true);
            return;
        }
        self.edit_before.get_or_insert_with(|| self.doc.clone());
        let settings = self.doc.rulers_mut();
        settings.guides_visible = true;
        let id = if let Some(id) = existing.filter(|_| !duplicate) {
            id
        } else {
            // The guide limit guarantees a free ID, even when a loaded ID is near u64::MAX.
            let id = (1..=1001)
                .find(|id| settings.guides.iter().all(|g| g.id != *id))
                .unwrap();
            settings.guides.push(Guide { id, axis, position });
            id
        };
        self.selected_guide = Some(id);
        self.landmark = None;
        self.drag = Some(Drag::Guide { id });
    }
    fn ruler_input(&mut self, ui: &egui::Ui, layout: rulers::Layout) {
        if !layout.visible {
            return;
        }
        for (axis, rect) in [
            (GuideAxis::Horizontal, layout.top),
            (GuideAxis::Vertical, layout.left),
        ] {
            let response = ui.interact(
                rect,
                ui.id().with(("pp.ruler", axis.coordinate())),
                egui::Sense::click_and_drag(),
            );
            if response.hovered() && !self.doc.rulers().locked {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if response.drag_started_by(egui::PointerButton::Primary) && self.drag.is_none() {
                if let Some(p) = response.interact_pointer_pos() {
                    let position =
                        rulers::world_at(&self.doc.viewport, layout.canvas, p)[axis.coordinate()];
                    self.begin_guide(axis, position, None, false);
                }
            }
            response.context_menu(|ui| {
                if ui
                    .add_enabled(
                        !self.doc.rulers().locked,
                        egui::Button::new("Remove guides on this axis"),
                    )
                    .clicked()
                {
                    self.doc.rulers_mut().guides.retain(|g| g.axis != axis);
                    self.selected_guide = None;
                    ui.close();
                }
                let mut locked = self.doc.rulers().locked;
                if ui.checkbox(&mut locked, "Lock guides").changed() {
                    self.doc.rulers_mut().locked = locked;
                }
            });
        }
    }
    fn update_guide_drag(&mut self, ctx: &egui::Context, rect: egui::Rect) {
        let Some(Drag::Guide { id }) = self.drag else {
            return;
        };
        let pointer = ctx.input(|i| i.pointer.interact_pos());
        if let Some(p) = pointer {
            let world = rulers::world_at(&self.doc.viewport, rect, p);
            if let Some(g) = self.doc.rulers_mut().guides.iter_mut().find(|g| g.id == id) {
                let position = world[g.axis.coordinate()];
                g.position = if ctx.input(|i| i.modifiers.shift) {
                    position.round()
                } else {
                    position
                }
                .clamp(-1e9, 1e9);
            }
        }
        if ctx.input(|i| i.pointer.button_released(egui::PointerButton::Primary)) {
            if !pointer.is_some_and(|p| rect.contains(p)) {
                self.doc.rulers_mut().guides.retain(|g| g.id != id);
                self.selected_guide = None;
            }
            self.drag = None;
        }
    }
    fn anchor_hit(
        &self,
        view: &Viewport,
        rect: egui::Rect,
        press: egui::Pos2,
    ) -> Option<(Option<usize>, GridSite)> {
        if !self.calibration || self.job.is_some() {
            return None;
        }
        let layer = self.doc.selected_layer().filter(|l| !l.locked)?;
        if let Some((i, p)) = layer.landmarks.iter().enumerate().find(|(_, p)| {
            to_screen(view, add(p.target.world(), layer.offset_world()), rect).distance(press) < 10.
                || layer
                    .source_to_world(p.source)
                    .is_some_and(|q| to_screen(view, q, rect).distance(press) < 10.)
        }) {
            return Some((Some(i), p.target));
        }
        let world = rulers::world_at(view, rect, press);
        let site = GridSite::nearest(sub(world, layer.offset_world()), self.kind);
        let target = add(site.world(), layer.offset_world());
        (self.doc.grid.visible
            && layer.contains(target)
            && to_screen(view, target, rect).distance(press) < 12.)
            .then_some((None, site))
    }
    fn canvas_input(&mut self, ui: &egui::Ui, response: &egui::Response, rect: egui::Rect) {
        let ctx = ui.ctx();
        if response.hovered() {
            let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0. {
                if let Some(p) = response.hover_pos() {
                    let view = &mut self.doc.viewport;
                    view.zoom_at(
                        (scroll as f64 * 0.003).exp(),
                        screen_point(p, rect),
                        rect_size(rect),
                    );
                }
            }
        }
        if response.dragged_by(egui::PointerButton::Middle) {
            let delta = ctx.input(|i| i.pointer.delta());
            let view = &mut self.doc.viewport;
            view.center[0] -= delta.x as f64 / view.zoom;
            view.center[1] += delta.y as f64 / view.zoom;
        }
        let view = self.doc.viewport.clone();
        if self.measuring {
            self.measurement_input(ui, response, rect);
            return;
        }
        if let Some(p) = response
            .hover_pos()
            .filter(|p| self.anchor_hit(&view, rect, *p).is_none())
        {
            if let Some(id) = rulers::hit(self.doc.rulers(), &view, rect, p) {
                let axis = self
                    .doc
                    .rulers()
                    .guides
                    .iter()
                    .find(|g| g.id == id)
                    .unwrap()
                    .axis;
                ctx.set_cursor_icon(if axis == GuideAxis::Vertical {
                    egui::CursorIcon::ResizeHorizontal
                } else {
                    egui::CursorIcon::ResizeVertical
                });
            }
        }
        let pointer = response.interact_pointer_pos();
        if response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(press) = ctx.input(|i| i.pointer.press_origin()) {
                let start = view.screen_to_world(screen_point(press, rect), rect_size(rect));
                if self.select_roi {
                    self.drag = Some(Drag::Roi { start });
                } else if !self.calibration && self.doc.comparison.mode == CompareMode::Wipe && {
                    let c = &self.doc.comparison;
                    if c.horizontal {
                        (press.y - (rect.top() + rect.height() * c.wipe)).abs() < 10.
                    } else {
                        (press.x - (rect.left() + rect.width() * c.wipe)).abs() < 10.
                    }
                } {
                    self.drag = Some(Drag::Wipe);
                } else if let Some(id) = rulers::hit(self.doc.rulers(), &view, rect, press)
                    .filter(|_| self.anchor_hit(&view, rect, press).is_none())
                {
                    let guide = self
                        .doc
                        .rulers()
                        .guides
                        .iter()
                        .find(|g| g.id == id)
                        .unwrap()
                        .clone();
                    self.begin_guide(
                        guide.axis,
                        guide.position,
                        Some(id),
                        ctx.input(|i| i.modifiers.alt),
                    );
                } else if let Some((hit, site)) = self.anchor_hit(&view, rect, press) {
                    self.edit_before.get_or_insert_with(|| self.doc.clone());
                    let layer = self.doc.selected_layer_mut().unwrap();
                    let index = hit
                        .or_else(|| layer.landmarks.iter().position(|p| p.target == site))
                        .unwrap_or_else(|| {
                            let position = layer.calibration.map(site.world(), layer.size).unwrap();
                            layer.landmarks.push(Landmark {
                                source: position,
                                target: site,
                                enabled: true,
                            });
                            layer.landmarks.len() - 1
                        });
                    layer.landmarks[index].enabled = true;
                    self.selected_guide = None;
                    self.landmark = Some(index);
                    self.drag = Some(Drag::Anchor {
                        id: layer.id,
                        index,
                        raw: start,
                        mapping: layer.calibration.clone(),
                        offset: layer.offset_world(),
                        snapped: false,
                    });
                } else if !self.calibration {
                    let selected = self
                        .doc
                        .selected_layer()
                        .filter(|l| l.visible && l.contains(start))
                        .or_else(|| {
                            self.doc
                                .layers
                                .iter()
                                .rev()
                                .find(|l| l.visible && l.contains(start))
                        });
                    if let Some(layer) = selected {
                        let (id, offset, locked) = (layer.id, layer.offset, layer.locked);
                        self.doc.selected = Some(id);
                        self.selected_guide = None;
                        if !locked {
                            self.drag = Some(Drag::Layer { id, start, offset });
                        }
                    }
                }
            }
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            if let (Some(p), Some(drag)) = (pointer, self.drag.clone()) {
                let world = view.screen_to_world(screen_point(p, rect), rect_size(rect));
                match drag {
                    Drag::Layer { id, start, offset } => {
                        if let Some(l) = self.doc.layers.iter_mut().find(|l| l.id == id) {
                            let d = snapped_delta(sub(world, start));
                            l.offset = [
                                offset[0].saturating_add(d[0]),
                                offset[1].saturating_add(d[1]),
                            ];
                        }
                    }
                    Drag::Anchor {
                        id,
                        index,
                        mapping,
                        offset,
                        ..
                    } => {
                        if let Some(layer) =
                            self.doc.layers.iter().find(|l| l.id == id && !l.locked)
                        {
                            if let (Some(point), Some(raw_source)) = (
                                layer.landmarks.get(index),
                                mapping.map(sub(world, offset), layer.size),
                            ) {
                                if raw_source[0] >= 0.
                                    && raw_source[1] >= 0.
                                    && raw_source[0] < layer.size[0] as f64
                                    && raw_source[1] < layer.size[1] as f64
                                {
                                    let target = point.target;
                                    // The pointer is on the already-corrected image. Resolve it back to
                                    // the ORIGINAL once with the gesture's map; never store warped samples.
                                    let center =
                                        if self.snap_centers && !ctx.input(|i| i.modifiers.alt) {
                                            self.assets.get(&layer.asset).and_then(|a| {
                                                detection::snap_center(
                                                    &a.image,
                                                    raw_source,
                                                    target.kind,
                                                    detection::search_radius(
                                                        &mapping, layer.size, target,
                                                    ),
                                                )
                                            })
                                        } else {
                                            None
                                        };
                                    let position =
                                        center.as_ref().map(|c| c.position).unwrap_or(raw_source);
                                    self.doc
                                        .layers
                                        .iter_mut()
                                        .find(|l| l.id == id)
                                        .unwrap()
                                        .landmarks[index]
                                        .source = position;
                                    self.drag = Some(Drag::Anchor {
                                        id,
                                        index,
                                        raw: world,
                                        mapping,
                                        offset,
                                        snapped: center.is_some(),
                                    });
                                }
                            }
                        }
                    }
                    Drag::Guide { .. } | Drag::Measure { .. } => {}
                    Drag::Wipe => {
                        let c = &mut self.doc.comparison;
                        c.wipe = if c.horizontal {
                            ((p.y - rect.top()) / rect.height()).clamp(0., 1.)
                        } else {
                            ((p.x - rect.left()) / rect.width()).clamp(0., 1.)
                        };
                    }
                    Drag::Roi { start } => {
                        self.doc.export.bounds = [
                            start[0].min(world[0]),
                            start[1].min(world[1]),
                            start[0].max(world[0]),
                            start[1].max(world[1]),
                        ];
                    }
                }
            }
        }
        if response.drag_stopped_by(egui::PointerButton::Primary)
            && !matches!(self.drag, Some(Drag::Guide { .. }))
        {
            if let Some(Drag::Anchor { id, .. }) = self.drag {
                self.refit_requested = Some(id);
            }
            if matches!(self.drag, Some(Drag::Roi { .. })) {
                self.select_roi = false;
                self.export_open = true;
            }
            self.drag = None;
        }
        if response.clicked_by(egui::PointerButton::Primary) {
            if let Some(p) = pointer {
                let world = view.screen_to_world(screen_point(p, rect), rect_size(rect));
                let anchor = self.anchor_hit(&view, rect, p).and_then(|(i, _)| i);
                let guide =
                    rulers::hit(self.doc.rulers(), &view, rect, p).filter(|_| anchor.is_none());
                self.selected_guide = guide;
                if guide.is_some() {
                    self.landmark = None;
                } else if self.calibration {
                    self.landmark = anchor;
                } else {
                    if let Some(layer) = self
                        .doc
                        .layers
                        .iter()
                        .rev()
                        .find(|l| l.visible && l.contains(world))
                    {
                        self.doc.selected = Some(layer.id);
                    }
                }
            }
        }
    }
    fn grid_labels(&self, painter: &egui::Painter, rect: egui::Rect, view: &Viewport) {
        let stride = super::grid::line_step(&self.doc.grid, view.zoom)
            .max((90. / (view.zoom * 2_f64.sqrt())).ceil())
            .max(1.) as i32;
        let corners = [
            rect.left_top(),
            rect.right_top(),
            rect.left_bottom(),
            rect.right_bottom(),
        ];
        let cells = corners
            .map(|p| world_to_cell(view.screen_to_world(screen_point(p, rect), rect_size(rect))));
        let min = [
            cells
                .iter()
                .map(|p| p[0])
                .fold(f64::INFINITY, f64::min)
                .floor() as i32,
            cells
                .iter()
                .map(|p| p[1])
                .fold(f64::INFINITY, f64::min)
                .floor() as i32,
        ];
        let max = [
            cells
                .iter()
                .map(|p| p[0])
                .fold(f64::NEG_INFINITY, f64::max)
                .ceil() as i32,
            cells
                .iter()
                .map(|p| p[1])
                .fold(f64::NEG_INFINITY, f64::max)
                .ceil() as i32,
        ];
        let mut count = 0;
        for i in (min[0].div_euclid(stride) * stride..=max[0]).step_by(stride as usize) {
            for j in (min[1].div_euclid(stride) * stride..=max[1]).step_by(stride as usize) {
                let p = to_screen(view, cell_to_world([i as f64, j as f64]), rect);
                if rect.contains(p) {
                    painter.text(
                        p + egui::vec2(5., 5.),
                        egui::Align2::LEFT_TOP,
                        format!("{i},{j}"),
                        egui::FontId::monospace(9.),
                        egui::Color32::from_white_alpha((self.doc.grid.opacity * 170.) as u8),
                    );
                    count += 1;
                    if count > 800 {
                        return;
                    }
                }
            }
        }
    }
    /// Draw measured centers and residuals in the SAME corrected coordinates as the photo.
    fn measurements_overlay(&self, painter: &egui::Painter, rect: egui::Rect, view: &Viewport) {
        let Some(layer) = self.doc.selected_layer() else {
            return;
        };
        if let Some(result) = self
            .assistance
            .get(&layer.id)
            .filter(|r| r.calibration == layer.calibration && r.anchors == layer.landmarks)
        {
            for (points, color) in [
                (&result.automatic, egui::Color32::from_rgb(80, 190, 230)),
                (&result.held_out, egui::Color32::from_rgb(180, 130, 245)),
            ] {
                for point in points {
                    let Some(measured) = layer.source_to_world(point.source) else {
                        continue;
                    };
                    let pos = to_screen(view, measured, rect);
                    if !rect.contains(pos) {
                        continue;
                    }
                    let target =
                        to_screen(view, add(point.target.world(), layer.offset_world()), rect);
                    let error = layer
                        .calibration
                        .map(point.target.world(), layer.size)
                        .map(|p| length(sub(p, point.source)))
                        .unwrap_or(f64::INFINITY);
                    let color = if error > 2.5 {
                        egui::Color32::LIGHT_RED
                    } else {
                        color
                    };
                    painter.circle_filled(pos, 2., color);
                    painter.arrow(pos, target - pos, egui::Stroke::new(1.0_f32, color));
                }
            }
        }
    }
    fn landmarks_overlay(&self, painter: &egui::Painter, rect: egui::Rect, view: &Viewport) {
        let Some(layer) = self.doc.selected_layer() else {
            return;
        };
        for (i, p) in layer.landmarks.iter().enumerate() {
            let expected = add(p.target.world(), layer.offset_world());
            let position = Some(expected);
            let target = layer.source_to_world(p.source);
            let color = if !p.enabled {
                egui::Color32::GRAY
            } else if layer
                .calibration
                .map(p.target.world(), layer.size)
                .is_some_and(|v| length(sub(v, p.source)) > 2.5)
            {
                egui::Color32::LIGHT_RED
            } else if self.landmark == Some(i) {
                egui::Color32::YELLOW
            } else {
                egui::Color32::LIGHT_GREEN
            };
            if let Some(pos) = position {
                let center = to_screen(view, pos, rect);
                painter.circle_stroke(center, 6., egui::Stroke::new(1.5_f32, color));
                painter.text(
                    center + egui::vec2(8., -8.),
                    egui::Align2::LEFT_BOTTOM,
                    format!(
                        "{} {} ({},{})",
                        i + 1,
                        p.target.kind.label(),
                        p.target.cell[0],
                        p.target.cell[1]
                    ),
                    egui::FontId::monospace(11.),
                    color,
                );
                if let Some(target) = target {
                    let target = to_screen(view, target, rect);
                    painter.arrow(
                        target,
                        center - target,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(255, 135, 70)),
                    );
                    painter.line_segment(
                        [target - egui::vec2(3., 0.), target + egui::vec2(3., 0.)],
                        egui::Stroke::new(1.0_f32, color),
                    );
                    painter.line_segment(
                        [target - egui::vec2(0., 3.), target + egui::vec2(0., 3.)],
                        egui::Stroke::new(1.0_f32, color),
                    );
                }
            }
        }
    }
    fn export_dialog(&mut self, ctx: &egui::Context) {
        if !self.export_open {
            return;
        }
        let mut open = true;
        let mut export = false;
        let mut roi = false;
        egui::Window::new("Export corrected image")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.checkbox(&mut self.doc.export.selected_only, "Selected image only");
                ui.checkbox(
                    &mut self.doc.export.grid,
                    "Include grid (without editor labels)",
                );
                ui.label("Grid-space crop: left / bottom / right / top");
                ui.horizontal(|ui| {
                    for value in &mut self.doc.export.bounds {
                        ui.add(egui::DragValue::new(value).speed(0.1));
                    }
                });
                if action(ui, "Draw crop on canvas") {
                    roi = true;
                }
                ui.horizontal(|ui| {
                    ui.label("Output pixels per grid unit");
                    ui.add(
                        egui::DragValue::new(&mut self.doc.export.pixels_per_unit)
                            .speed(0.1)
                            .range(0.1..=10000.),
                    );
                });
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.doc.export.bit_depth, 8, "8-bit PNG");
                    ui.selectable_value(&mut self.doc.export.bit_depth, 16, "16-bit PNG");
                });
                match self.doc.export.dimensions() {
                    Ok([w, h]) => {
                        ui.label(format!("{w} × {h} pixels · transparent outside images"));
                        ui.add_enabled_ui(self.job.is_none(), |ui| {
                            export = action(ui, "Export PNG");
                        });
                    }
                    Err(e) => {
                        ui.colored_label(egui::Color32::LIGHT_RED, e.to_string());
                    }
                }
            });
        self.export_open = open;
        if roi {
            self.export_open = false;
            self.select_roi = true;
            self.calibration = false;
            self.measuring = false;
            self.measure_preview = None;
            self.message("Drag a crop rectangle on the shared grid", false);
        }
        if export {
            if let Some(mut path) = rfd::FileDialog::new()
                .add_filter("PNG", &["png"])
                .set_file_name("pixel-perfecto.png")
                .save_file()
            {
                if path.extension().is_none() {
                    path.set_extension("png");
                }
                let doc = self.doc.clone();
                let assets = self.assets.clone();
                self.start(ctx.clone(), "Exporting PNG…", move |progress| {
                    io::export_png(&path, &doc, &assets, &progress)?;
                    Ok(Work::Exported(path))
                });
                self.export_open = false;
            }
        }
    }
    fn pending_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if !self.dirty() && self.job.is_none() {
            self.pending = None;
            self.perform(pending, ctx.clone());
            return;
        }
        let mut save = false;
        let mut discard = false;
        let mut cancel = false;
        egui::Window::new("Unsaved Pixel Perfecto project")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(if self.job.is_some() {
                    "Wait for the current operation to finish, or cancel closing."
                } else {
                    "Save the embedded images and alignment before continuing?"
                });
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(self.job.is_none(), |ui| {
                        save = action(ui, "Save");
                        discard = action(ui, "Discard");
                    });
                    cancel = action(ui, "Cancel");
                });
            });
        if cancel {
            self.pending = None;
        } else if discard {
            self.pending = None;
            self.perform(pending, ctx.clone());
        } else if save {
            self.save(ctx.clone(), false, Some(pending));
        }
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let mut renderer = self.state.renderer.write();
        self.canvas.release(&mut renderer);
        self.magnifier.release(&mut renderer);
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig, eframe::wgpu};
    fn workspace() -> Workspace {
        let gpu = HeadlessRenderer::new(HeadlessRendererConfig::default()).unwrap();
        let renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            wgpu::TextureFormat::Rgba16Float,
            Default::default(),
        );
        let state = egui_wgpu::RenderState {
            adapter: gpu.adapter,
            available_adapters: vec![],
            device: (*gpu.device).clone(),
            queue: (*gpu.queue).clone(),
            target_format: wgpu::TextureFormat::Rgba16Float,
            renderer: Arc::new(egui::mutex::RwLock::new(renderer)),
        };
        let mut w = Workspace::new(state);
        w.doc.viewport.zoom = 10.;
        for id in [1, 2] {
            w.doc.layers.push(ImageLayer {
                id,
                asset: format!("{id}"),
                name: format!("Layer {id}"),
                size: [1000, 1000],
                bit_depth: 8,
                visible: true,
                locked: false,
                opacity: 1.,
                offset: [0, 0],
                calibration: Calibration::initial([1000, 1000]),
                landmarks: vec![],
            });
        }
        w.doc.selected = Some(1);
        w
    }
    fn frame(ctx: &egui::Context, w: &mut Workspace, time: &mut f64, events: Vec<egui::Event>) {
        *time += 0.02;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 400.));
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(rect),
                time: Some(*time),
                events,
                ..Default::default()
            },
            |ui| {
                let response = ui.interact(
                    rect,
                    egui::Id::new("gesture-test"),
                    egui::Sense::click_and_drag(),
                );
                w.canvas_input(ui, &response, rect);
            },
        );
    }
    fn press(button: egui::PointerButton, pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Default::default(),
        }
    }
    // Run real egui pointer capture across the ruler/canvas boundary, including gesture history.
    fn guide_frame(
        ctx: &egui::Context,
        w: &mut Workspace,
        time: &mut f64,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) {
        *time += 0.02;
        let body = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(424., 424.));
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(body),
                time: Some(*time),
                events,
                modifiers,
                ..Default::default()
            },
            |ui| {
                let before = w.doc.clone();
                w.history_action = false;
                w.shortcuts(ctx);
                let layout = rulers::Layout::new(body, w.doc.rulers().visible);
                w.ruler_input(ui, layout);
                let response = ui.interact(
                    layout.canvas,
                    egui::Id::new("guide-canvas-test"),
                    egui::Sense::click_and_drag(),
                );
                w.canvas_input(ui, &response, layout.canvas);
                w.update_guide_drag(ctx, layout.canvas);
                if !w.history_action && before != w.doc && w.edit_before.is_none() {
                    w.edit_before = Some(before);
                }
                if !ctx.input(|i| i.pointer.any_down()) && w.drag.is_none() {
                    if let Some(before) = w.edit_before.take() {
                        w.history.record(before, &w.doc);
                    }
                }
            },
        );
    }
    fn guide_gesture(
        ctx: &egui::Context,
        w: &mut Workspace,
        time: &mut f64,
        start: egui::Pos2,
        end: egui::Pos2,
        modifiers: egui::Modifiers,
    ) {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers,
        };
        guide_frame(
            ctx,
            w,
            time,
            vec![egui::Event::PointerMoved(start), button(start, true)],
            modifiers,
        );
        guide_frame(
            ctx,
            w,
            time,
            vec![egui::Event::PointerMoved(end)],
            modifiers,
        );
        guide_frame(ctx, w, time, vec![button(end, false)], modifiers);
    }
    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }
    #[test]
    fn measure_clicks_handles_cancel_undo_and_pan_preserve_layers() {
        let mut w = workspace();
        w.calibration = true;
        let layers = w.doc.layers.clone();
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(2.);
        let mut time = 0.;
        let none = egui::Modifiers::default();
        guide_frame(&ctx, &mut w, &mut time, vec![key(egui::Key::M, none)], none);
        assert!(w.measuring && !w.calibration);
        let a = egui::pos2(204., 224.);
        let b = egui::pos2(264., 194.);
        guide_gesture(&ctx, &mut w, &mut time, a, a, none);
        assert!(w.measure_preview.is_some());
        assert!(
            w.doc.measurement.is_none(),
            "A alone must not dirty the project"
        );
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![egui::Event::PointerMoved(b)],
            none,
        );
        assert_eq!(w.measure_preview.unwrap().b, [4., 3.]);
        guide_gesture(&ctx, &mut w, &mut time, b, b, none);
        let initial = Measurement::new([-2., 0.], [4., 3.]);
        assert_eq!(w.doc.measurement, Some(initial));
        assert!(w.measure_preview.is_none());
        assert_eq!(w.doc.layers, layers);
        let command = egui::Modifiers {
            command: true,
            ..none
        };
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![key(egui::Key::Z, command)],
            command,
        );
        assert!(w.doc.measurement.is_none());
        let redo = egui::Modifiers {
            shift: true,
            ..command
        };
        guide_frame(&ctx, &mut w, &mut time, vec![key(egui::Key::Z, redo)], redo);
        assert_eq!(w.doc.measurement, Some(initial));

        // Shift constrains the dragged endpoint against the other endpoint, not screen origin.
        let shift = egui::Modifiers {
            shift: true,
            ..none
        };
        guide_gesture(&ctx, &mut w, &mut time, b, egui::pos2(304., 200.), shift);
        assert_eq!(w.doc.measurement.unwrap().b, [8., 0.]);
        let completed = w.doc.measurement;
        let start = egui::pos2(304., 224.);
        let end = egui::pos2(300., 180.);
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![
                egui::Event::PointerMoved(start),
                press(egui::PointerButton::Primary, start, true),
            ],
            none,
        );
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![egui::Event::PointerMoved(end)],
            none,
        );
        assert_ne!(w.doc.measurement, completed);
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![key(egui::Key::Escape, none)],
            none,
        );
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![press(egui::PointerButton::Primary, end, false)],
            none,
        );
        assert_eq!(w.doc.measurement, completed);
        assert!(w.measuring);

        // View changes do not alter the world endpoints or touch either image calibration.
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![
                egui::Event::PointerMoved(a),
                press(egui::PointerButton::Middle, a, true),
            ],
            none,
        );
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![egui::Event::PointerMoved(b)],
            none,
        );
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![press(egui::PointerButton::Middle, b, false)],
            none,
        );
        assert_ne!(w.doc.viewport.center, [0., 0.]);
        assert_eq!(w.doc.measurement, completed);
        assert_eq!(w.doc.layers, layers);
        // Endpoint snapping works for negative coordinates and Alt bypass is exact.
        w.measure_snap = true;
        w.doc.viewport = Viewport {
            center: [0., 0.],
            zoom: 10.,
        };
        guide_gesture(&ctx, &mut w, &mut time, a, egui::pos2(211., 205.), none);
        assert_eq!(w.doc.measurement.unwrap().a, [-1., 2.]);
        let alt = egui::Modifiers { alt: true, ..none };
        guide_gesture(
            &ctx,
            &mut w,
            &mut time,
            egui::pos2(214., 204.),
            egui::pos2(201., 185.),
            alt,
        );
        assert!(length(sub(w.doc.measurement.unwrap().a, [-2.3, 3.9])) < 1e-9);
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![key(egui::Key::Delete, none)],
            none,
        );
        assert!(w.doc.measurement.is_none());
        assert_eq!(w.doc.layers, layers);
    }
    #[test]
    fn rgb_measurement_worker_discards_obsolete_segments_and_refreshes_selected_image() {
        let mut w = workspace();
        let ctx = egui::Context::default();
        for id in [1, 2] {
            w.assets.insert(
                id.to_string(),
                Arc::new(Asset {
                    bytes: Arc::new(vec![]),
                    image: Arc::new(io::DecodedImage {
                        size: [1000, 1000],
                        bit_depth: 8,
                        pixels: io::Pixels::Srgb8(vec![0; 4_000_000]),
                    }),
                }),
            );
        }
        w.doc.measurement = Some(Measurement::new([-2., 0.], [2., 0.]));
        w.start_measurement(ctx.clone());
        w.doc.measurement.as_mut().unwrap().b = [4., 0.];
        finish_job(&mut w, &ctx);
        assert!(w.measure_report.is_none());
        w.start_measurement(ctx.clone());
        finish_job(&mut w, &ctx);
        let report = w.measure_report.as_ref().unwrap();
        assert_eq!(report.layer.id, 1);
        assert_eq!(report.result.as_ref().unwrap().expected, [4, 3, 0, 0]);
        assert_eq!(report.result.as_ref().unwrap().detected, [0; 3]);
        w.start_measurement(ctx.clone());
        assert!(
            w.job.is_none(),
            "Unchanged measurements must use the cached report"
        );
        w.doc.selected = Some(2);
        w.start_measurement(ctx.clone());
        finish_job(&mut w, &ctx);
        assert_eq!(w.measure_report.as_ref().unwrap().layer.id, 2);
        w.doc.layers[1].rotate_image(12., [0., 0.]).unwrap();
        w.start_measurement(ctx.clone());
        w.doc.layers[1].offset = [-2, 3];
        finish_job(&mut w, &ctx);
        assert_ne!(w.measure_report.as_ref().unwrap().layer, w.doc.layers[1]);
        w.start_measurement(ctx.clone());
        finish_job(&mut w, &ctx);
        assert_eq!(w.measure_report.as_ref().unwrap().layer, w.doc.layers[1]);
    }
    #[test]
    fn ruler_guides_drag_duplicate_delete_cancel_and_undo_across_canvas_boundary() {
        let mut w = workspace();
        let layers = w.doc.layers.clone();
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(2.);
        let mut time = 0.;
        let none = egui::Modifiers::default();
        let alt = egui::Modifiers { alt: true, ..none };
        let shift = egui::Modifiers {
            shift: true,
            ..none
        };
        guide_frame(&ctx, &mut w, &mut time, vec![], none);
        // The top ruler produces a horizontal line at Y=5.2; the left ruler produces X=-4.3.
        guide_gesture(
            &ctx,
            &mut w,
            &mut time,
            egui::pos2(224., 12.),
            egui::pos2(224., 172.),
            none,
        );
        assert_eq!(
            w.doc.rulers().guides,
            vec![Guide {
                id: 1,
                axis: GuideAxis::Horizontal,
                position: 5.2
            }]
        );
        guide_gesture(
            &ctx,
            &mut w,
            &mut time,
            egui::pos2(12., 224.),
            egui::pos2(181., 224.),
            none,
        );
        assert_eq!(
            w.doc.rulers().guides[1],
            Guide {
                id: 2,
                axis: GuideAxis::Vertical,
                position: -4.3
            }
        );
        let two = w.doc.clone();
        // Alt-drag preserves the source; Shift-drag snaps to whole shared-grid units.
        guide_gesture(
            &ctx,
            &mut w,
            &mut time,
            egui::pos2(181., 260.),
            egui::pos2(281., 260.),
            alt,
        );
        assert_eq!(w.doc.rulers().guides[1].position, -4.3);
        assert_eq!(w.doc.rulers().guides[2].position, 5.7);
        guide_gesture(
            &ctx,
            &mut w,
            &mut time,
            egui::pos2(281., 260.),
            egui::pos2(297., 260.),
            shift,
        );
        assert_eq!(w.doc.rulers().guides[2].position, 7.);
        let three = w.doc.clone();
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![key(egui::Key::Delete, none)],
            none,
        );
        assert_eq!(w.doc, two);
        w.history.undo(&mut w.doc);
        assert_eq!(w.doc, three);
        w.history.redo(&mut w.doc);
        assert_eq!(w.doc, two);
        // Dragging back onto a ruler deletes. One undo restores the exact guide coordinate.
        guide_gesture(
            &ctx,
            &mut w,
            &mut time,
            egui::pos2(181., 260.),
            egui::pos2(12., 260.),
            none,
        );
        assert_eq!(w.doc.rulers().guides.len(), 1);
        w.history.undo(&mut w.doc);
        assert_eq!(w.doc, two);
        // Escape restores the guide under the pointer, even while the mouse stays down.
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![
                egui::Event::PointerMoved(egui::pos2(181., 260.)),
                press(egui::PointerButton::Primary, egui::pos2(181., 260.), true),
            ],
            none,
        );
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![egui::Event::PointerMoved(egui::pos2(241., 260.))],
            none,
        );
        assert_ne!(w.doc, two);
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![key(egui::Key::Escape, none)],
            none,
        );
        assert_eq!(w.doc, two);
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![press(
                egui::PointerButton::Primary,
                egui::pos2(241., 260.),
                false,
            )],
            none,
        );
        assert_eq!(w.doc.layers, layers, "Guides never change image alignment");
        assert!(w.drag.is_none());
        assert_eq!(w.refit_requested, None);
    }
    #[test]
    fn calibration_and_wipe_handles_take_priority_over_guides_and_locked_guides_allow_image_drag() {
        let mut w = workspace();
        let ctx = egui::Context::default();
        let mut time = 0.;
        let none = egui::Modifiers::default();
        w.doc.rulers_mut().guides.push(Guide {
            id: 1,
            axis: GuideAxis::Vertical,
            position: 0.,
        });
        w.calibration = true;
        guide_frame(&ctx, &mut w, &mut time, vec![], none);
        guide_gesture(
            &ctx,
            &mut w,
            &mut time,
            egui::pos2(224., 224.),
            egui::pos2(244., 214.),
            none,
        );
        assert_eq!(w.doc.layers[0].landmarks.len(), 1);
        assert_eq!(w.refit_requested, Some(1));
        assert_eq!(w.doc.rulers().guides[0].position, 0.);
        w.calibration = false;
        w.doc.comparison.mode = CompareMode::Wipe;
        guide_gesture(
            &ctx,
            &mut w,
            &mut time,
            egui::pos2(224., 290.),
            egui::pos2(264., 290.),
            none,
        );
        assert!((w.doc.comparison.wipe - 0.6).abs() < 1e-6);
        assert_eq!(w.doc.rulers().guides[0].position, 0.);
        w.doc.comparison.mode = CompareMode::Overlay;
        w.doc.rulers_mut().locked = true;
        guide_gesture(
            &ctx,
            &mut w,
            &mut time,
            egui::pos2(224., 280.),
            egui::pos2(244., 280.),
            none,
        );
        assert_eq!(w.doc.layers[0].offset, [1, -1]);
        assert_eq!(w.doc.rulers().guides[0].position, 0.);
        w.selected_guide = Some(1);
        guide_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![key(egui::Key::Delete, none)],
            none,
        );
        assert_eq!(w.doc.rulers().guides.len(), 1);
    }
    #[test]
    fn actual_pointer_gestures_snap_only_selected_image_and_prioritize_wipe() {
        let mut w = workspace();
        let ctx = egui::Context::default();
        let mut time = 0.;
        let start = egui::pos2(200., 200.);
        let moved = egui::pos2(260., 160.);
        frame(&ctx, &mut w, &mut time, vec![]);
        let before = w.doc.layers.clone();
        frame(
            &ctx,
            &mut w,
            &mut time,
            vec![
                egui::Event::PointerMoved(start),
                press(egui::PointerButton::Primary, start, true),
            ],
        );
        frame(
            &ctx,
            &mut w,
            &mut time,
            vec![egui::Event::PointerMoved(moved)],
        );
        assert_eq!(w.doc.layers[0].offset, [5, -1]);
        assert_eq!(w.doc.layers[0].calibration, before[0].calibration);
        assert_eq!(w.doc.layers[1], before[1]);
        frame(
            &ctx,
            &mut w,
            &mut time,
            vec![press(egui::PointerButton::Primary, moved, false)],
        );
        frame(
            &ctx,
            &mut w,
            &mut time,
            vec![
                egui::Event::PointerMoved(start),
                press(egui::PointerButton::Middle, start, true),
            ],
        );
        let p = egui::pos2(263., 181.);
        frame(&ctx, &mut w, &mut time, vec![egui::Event::PointerMoved(p)]);
        assert_eq!(w.doc.viewport.center, [-6.3, -1.9]);
        assert_eq!(w.doc.layers[0].offset, [5, -1]);
        frame(
            &ctx,
            &mut w,
            &mut time,
            vec![press(egui::PointerButton::Middle, p, false)],
        );
        w.doc.comparison.mode = CompareMode::Wipe;
        frame(
            &ctx,
            &mut w,
            &mut time,
            vec![
                egui::Event::PointerMoved(start),
                press(egui::PointerButton::Primary, start, true),
            ],
        );
        let p = egui::pos2(300., 205.);
        frame(&ctx, &mut w, &mut time, vec![egui::Event::PointerMoved(p)]);
        assert_eq!(w.doc.comparison.wipe, 0.75);
        assert_eq!(w.doc.layers[0].offset, [5, -1]);
        assert_eq!(w.doc.layers[1].offset, [0, 0]);
    }
    fn alignment_frame(
        ctx: &egui::Context,
        w: &mut Workspace,
        time: &mut f64,
        events: Vec<egui::Event>,
    ) {
        *time += 0.02;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 400.));
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(rect),
                time: Some(*time),
                events,
                ..Default::default()
            },
            |ui| {
                w.shortcuts(ui.ctx());
                let response = ui.interact(
                    rect,
                    egui::Id::new("alignment-gesture-test"),
                    egui::Sense::click_and_drag(),
                );
                w.canvas_input(ui, &response, rect);
            },
        );
    }
    fn finish_job(w: &mut Workspace, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while w.job.is_some() && std::time::Instant::now() < deadline {
            w.poll(ctx);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(w.job.is_none(), "Analysis timed out");
    }
    #[test]
    fn collinear_third_and_fourth_anchors_apply_through_the_background_worker() {
        let mut w = workspace();
        w.auto_points = false;
        w.doc.layers[0].calibration.h[0] = 0.024;
        w.doc.layers[0].calibration.h[4] = 0.024;
        let truth = w.doc.layers[0].calibration.clone();
        let other = w.doc.layers[1].clone();
        w.assets.insert(
            "1".into(),
            Arc::new(Asset {
                bytes: Arc::new(vec![]),
                image: Arc::new(io::DecodedImage {
                    size: [1000, 1000],
                    bit_depth: 8,
                    pixels: io::Pixels::Srgb8(vec![0; 4_000_000]),
                }),
            }),
        );
        let ctx = egui::Context::default();
        for (index, cell) in [[-26, -3], [-27, -4], [-17, 6], [-12, 11]]
            .into_iter()
            .enumerate()
        {
            let target = GridSite {
                cell,
                kind: Subpixel::Red,
            };
            let mut source = truth.map(target.world(), [1000, 1000]).unwrap();
            if index == 1 {
                source = add(source, [1.8, 0.5]);
            }
            w.doc.layers[0].landmarks.push(Landmark {
                source,
                target,
                enabled: true,
            });
            w.start_assistance(ctx.clone(), 1, false);
            finish_job(&mut w, &ctx);
            assert!(!w.error, "anchor {}: {}", index + 1, w.status);
            let result = w.assistance.get(&1).unwrap();
            assert_eq!(result.anchors, w.doc.layers[0].landmarks);
            assert_eq!(result.calibration, w.doc.layers[0].calibration);
            if index >= 2 {
                assert_eq!(result.model, detection::FitModel::Similarity);
                assert!(result.fitted_rms < 1.5);
                assert!(w.status.contains("same grid column"));
            }
            assert_eq!(w.doc.layers[1], other);
        }
    }
    #[test]
    fn corrected_canvas_drag_keeps_identity_fits_on_release_and_undoes_as_one_edit() {
        let mut w = workspace();
        w.calibration = true;
        w.auto_points = false;
        w.doc.viewport = Viewport {
            center: [0., 0.],
            zoom: 64.,
        };
        w.assets.insert(
            "1".into(),
            Arc::new(Asset {
                bytes: Arc::new(vec![]),
                image: Arc::new(io::DecodedImage {
                    size: [1000, 1000],
                    bit_depth: 8,
                    pixels: io::Pixels::Srgb8(vec![0; 4_000_000]),
                }),
            }),
        );
        let original = w.doc.clone();
        let ctx = egui::Context::default();
        let mut time = 0.;
        let start = egui::pos2(200., 200.);
        let moved = egui::pos2(236., 174.);
        alignment_frame(&ctx, &mut w, &mut time, vec![]);
        alignment_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![
                egui::Event::PointerMoved(start),
                press(egui::PointerButton::Primary, start, true),
            ],
        );
        alignment_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![egui::Event::PointerMoved(moved)],
        );
        let site = GridSite {
            cell: [0, 0],
            kind: Subpixel::Red,
        };
        assert_eq!(w.doc.layers[0].landmarks[0].target, site);
        assert_eq!(w.doc.layers[0].landmarks[0].source, [518., 513.]);
        assert_eq!(
            w.doc.layers[0].calibration, original.layers[0].calibration,
            "The last accumulated correction remains stable while choosing a source point"
        );
        assert_eq!(w.doc.layers[0].offset, [0, 0]);
        assert_eq!(w.doc.layers[1], original.layers[1]);
        assert!(w.refit_requested.is_none());
        alignment_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![press(egui::PointerButton::Primary, moved, false)],
        );
        assert_eq!(w.refit_requested, Some(1));
        let before = w.edit_before.take().unwrap();
        w.history.record(before, &w.doc);
        w.start_assistance(ctx.clone(), 1, false);
        finish_job(&mut w, &ctx);
        assert!(!w.error, "{}", w.status);
        assert!(
            length(sub(
                w.doc.layers[0]
                    .calibration
                    .map(site.world(), [1000, 1000])
                    .unwrap(),
                [518., 513.]
            )) < 1e-6
        );
        assert_eq!(w.doc.layers[1], original.layers[1]);
        let after = w.doc.clone();
        w.history.undo(&mut w.doc);
        assert_eq!(w.doc, original);
        w.history.redo(&mut w.doc);
        assert_eq!(w.doc, after);
        // A completed worker must not reapply a result after an undo/cancel epoch change.
        w.start_assistance(ctx.clone(), 1, false);
        w.analysis_epoch += 1;
        finish_job(&mut w, &ctx);
        assert!(w.status.contains("stale"));
        assert_eq!(w.doc, after);
    }
    #[test]
    fn successive_picks_use_accumulated_rotation_lens_and_offset_in_the_same_view() {
        let mut w = workspace();
        w.calibration = true;
        w.auto_points = false;
        w.snap_centers = false;
        w.doc.layers[0].offset = [-2, 3];
        let layer = &mut w.doc.layers[0];
        layer.calibration.lens = [0.04, -0.008, 0.001, -0.001];
        layer.calibration.h[6] = 0.001;
        layer.calibration.h[7] = -0.0006;
        layer.rotate_image(31., layer.offset_world()).unwrap();
        let lens = layer.calibration.lens;
        w.doc.viewport = Viewport {
            center: layer.offset_world(),
            zoom: 64.,
        };
        w.assets.insert(
            "1".into(),
            Arc::new(Asset {
                bytes: Arc::new(vec![]),
                image: Arc::new(io::DecodedImage {
                    size: [1000, 1000],
                    bit_depth: 8,
                    pixels: io::Pixels::Srgb8(vec![0; 4_000_000]),
                }),
            }),
        );
        let other = w.doc.layers[1].clone();
        let ctx = egui::Context::default();
        let mut time = 0.;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 400.));
        alignment_frame(&ctx, &mut w, &mut time, vec![]);
        let mut measured = Vec::new();
        for (cell, delta) in [
            ([0, 0], [0.25, -0.125]),
            ([1, 1], [0.25, 0.25]),
            ([-1, 1], [-0.25, 0.125]),
        ] {
            let site = GridSite {
                cell,
                kind: Subpixel::Red,
            };
            let l = &w.doc.layers[0];
            let target = add(site.world(), l.offset_world());
            let observed = add(target, delta);
            let expected = l.sample_position(observed).unwrap();
            let start = to_screen(&w.doc.viewport, target, rect);
            let end = to_screen(&w.doc.viewport, observed, rect);
            let view = w.doc.viewport.clone();
            alignment_frame(
                &ctx,
                &mut w,
                &mut time,
                vec![
                    egui::Event::PointerMoved(start),
                    press(egui::PointerButton::Primary, start, true),
                ],
            );
            alignment_frame(
                &ctx,
                &mut w,
                &mut time,
                vec![egui::Event::PointerMoved(end)],
            );
            let point = w.doc.layers[0].landmarks.last().unwrap();
            assert_eq!(point.target, site);
            assert!(
                length(sub(point.source, expected)) < 1e-8,
                "A corrected-space click must be mapped to original pixels through the latest transform"
            );
            measured.push(expected);
            alignment_frame(
                &ctx,
                &mut w,
                &mut time,
                vec![press(egui::PointerButton::Primary, end, false)],
            );
            let before = w.edit_before.take().unwrap();
            w.history.record(before, &w.doc);
            w.refit_requested = None;
            w.start_assistance(ctx.clone(), 1, false);
            finish_job(&mut w, &ctx);
            assert!(!w.error, "{}", w.status);
            let l = &w.doc.layers[0];
            for (point, expected) in l.landmarks.iter().zip(&measured) {
                assert!(
                    length(sub(point.source, *expected)) < 1e-8,
                    "Earlier measurements must never be resampled or overwritten"
                );
                assert!(
                    length(sub(
                        l.source_to_world(point.source).unwrap(),
                        add(point.target.world(), l.offset_world())
                    )) < 1e-6
                );
            }
            assert_eq!(l.calibration.lens, lens);
            assert_eq!(l.offset, [-2, 3]);
            assert_eq!(w.doc.layers[1], other);
            assert_eq!(w.doc.viewport, view);
        }
    }
    #[test]
    fn escape_restores_anchor_drag_and_locked_layer_cannot_be_calibrated() {
        let mut w = workspace();
        w.calibration = true;
        w.doc.viewport = Viewport {
            center: [0., 0.],
            zoom: 64.,
        };
        let original = w.doc.clone();
        let ctx = egui::Context::default();
        let mut time = 0.;
        let start = egui::pos2(200., 200.);
        let moved = egui::pos2(232., 180.);
        alignment_frame(&ctx, &mut w, &mut time, vec![]);
        alignment_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![
                egui::Event::PointerMoved(start),
                press(egui::PointerButton::Primary, start, true),
            ],
        );
        alignment_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![egui::Event::PointerMoved(moved)],
        );
        assert_eq!(w.doc.layers[0].landmarks.len(), 1);
        alignment_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(w.doc, original);
        assert!(w.drag.is_none());
        assert!(w.refit_requested.is_none());
        alignment_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![press(egui::PointerButton::Primary, moved, false)],
        );
        w.doc.layers[0].locked = true;
        alignment_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![
                egui::Event::PointerMoved(start),
                press(egui::PointerButton::Primary, start, true),
            ],
        );
        alignment_frame(
            &ctx,
            &mut w,
            &mut time,
            vec![egui::Event::PointerMoved(moved)],
        );
        assert!(w.doc.layers[0].landmarks.is_empty());
        assert!(w.drag.is_none());
    }
    #[test]
    fn idle_layout_does_not_dirty_document_and_empty_tool_window_needs_no_scene() {
        let mut w = workspace();
        let ctx = egui::Context::default();
        ctx.set_theme(egui::Theme::Dark);
        crate::ui::fonts::configure_egui_fonts(&ctx);
        w.saved = w.doc.clone();
        let expected = w.doc.clone();
        for tick in 0..3 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1440., 900.),
                    )),
                    time: Some(tick as f64 * 0.1),
                    ..Default::default()
                },
                |ui| w.show(ui),
            );
            assert_eq!(
                w.doc, expected,
                "An idle UI must not change persisted values"
            );
        }
        let size = w.last_canvas;
        w.calibration = true;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440., 900.),
                )),
                time: Some(1.),
                ..Default::default()
            },
            |ui| w.show(ui),
        );
        assert_eq!(
            w.last_canvas, size,
            "Calibration must use the full shared canvas, not one half of a split view"
        );
        assert_eq!(
            w.doc, expected,
            "Entering calibration must retain the accumulated correction and view"
        );
        let mut window = None;
        super::super::open_tool_window(&mut window, &w.state);
        let first = window.as_ref().unwrap().workspace.clone();
        super::super::open_tool_window(&mut window, &w.state);
        assert!(Arc::ptr_eq(&first, &window.as_ref().unwrap().workspace));
        ctx.set_embed_viewports(true);
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440., 900.),
                )),
                ..Default::default()
            },
            |ui| super::super::show_tool_window(ui.ctx(), &mut window),
        );
        let child = first.lock().unwrap();
        assert!(child.doc.layers.is_empty());
        assert!(!child.closed);
    }
}
