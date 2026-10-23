use crate::ui::tool_window;
use super::{
    controller::{Command, Controller, Prepare},
    model::*,
};
use rust_wgpu_fiber::eframe::egui;
use std::collections::BTreeSet;
pub struct Panel {
    pub controller: Controller,
    pub workflow: Workflow,
    platform: Platform,
    device_id: String,
    mode: DisplayMode,
    brightness: f32,
    peak: f32,
    batch: String,
    notes: String,
    baseline: Option<String>,
    selected: BTreeSet<String>,
    baseline_name: String,
    thresholds_enabled: bool,
    thresholds: Thresholds,
    report: Option<InspectionReport>,
    library_filter: String,
}
impl Default for Panel {
    fn default() -> Self {
        Self::with_controller(Controller::new())
    }
}
impl Panel {
    fn with_controller(controller: Controller) -> Self {
        Self {
            controller,
            workflow: Workflow::Reference,
            platform: Platform::Ios,
            device_id: String::new(),
            mode: DisplayMode::Srgb,
            brightness: 0.5,
            peak: 2.,
            batch: String::new(),
            notes: String::new(),
            baseline: None,
            selected: BTreeSet::new(),
            baseline_name: "iPhone reference".into(),
            thresholds_enabled: false,
            thresholds: Thresholds {
                rgb_fraction_of_white: 0.03,
                white_relative: 0.05,
            },
            report: None,
            library_filter: String::new(),
        }
    }
    pub(crate) fn preview(workflow: Workflow) -> Self {
        let mut panel = Self::with_controller(Controller::preview());
        panel.workflow = workflow;
        panel.platform = if workflow == Workflow::Inspection {
            Platform::Android
        } else {
            Platform::Ios
        };
        panel.controller.device = Some(DisplayDevice {
            id: "ui-fixture".into(),
            platform: panel.platform,
            name: if panel.platform == Platform::Ios {
                "iPhone 16"
            } else {
                "Android display"
            }
            .into(),
            size: [1179, 2556],
            drawable: [1179, 2556],
            insets: [0, 34, 0, 62],
            modes: vec![
                DisplayMode::Srgb,
                DisplayMode::DisplayP3,
                DisplayMode::LinearHdr,
            ],
            brightness: 0.5,
            headroom: 8.,
            active: true,
            generation: "ui-fixture".into(),
            brightness_guard: None,
        });
        panel
    }
}
fn action(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    tool_window::action(ui, label, enabled).clicked()
}

impl Panel {
    pub fn show(&mut self, ui: &mut egui::Ui, capture_issue: Option<&str>) {
        let previous = self.workflow;
        egui::ComboBox::from_id_salt("screen.workflow")
            .selected_text(self.workflow.label())
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for w in [Workflow::Reference, Workflow::Inspection] {
                    ui.selectable_value(&mut self.workflow, w, w.label());
                }
            });
        if self.workflow != previous {
            self.selected.clear();
            self.report = None;
            if self.controller.device.is_none() {
                self.platform = if self.workflow == Workflow::Inspection {
                    Platform::Android
                } else {
                    Platform::Ios
                };
            }
        }
        if self.workflow != Workflow::Geometry {
            self.controller.load_library_if_needed();
        }
        let busy = self.controller.busy;
        ui.add_space(4.);
        if let Ok(store) = self.controller.geometry.lock() {
            if let Some(g) = &store.active {
                ui.small(format!(
                    "Session geometry · v{} · {:.2} px",
                    g.revision, g.holdout_rms
                ));
            } else {
                ui.small("Session geometry · not calibrated");
            }
        }
        tool_window::section(ui, "Display");
        ui.add_enabled_ui(!busy, |ui| {
            if let Some(device) = &self.controller.device {
                ui.label(&device.name);
                ui.small(format!(
                    "{} × {} · USB · {:.1}× headroom",
                    device.size[0], device.size[1], device.headroom
                ));
                if action(ui, "Disconnect display", true) {
                    self.controller.send(Command::Disconnect);
                }
            } else {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.platform, Platform::Ios, "iPhone");
                    ui.selectable_value(&mut self.platform, Platform::Android, "Android");
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.device_id)
                        .hint_text(if self.platform == Platform::Ios {
                            "USB UDID (blank: single iPhone)"
                        } else {
                            "USB serial (blank: single device)"
                        })
                        .desired_width(f32::INFINITY),
                );
                if action(ui, "Connect display", true) {
                    self.controller
                        .send(Command::Connect(self.platform, self.device_id.clone()));
                }
                if self.platform == Platform::Android && action(ui, "Install player APK…", true) {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Android player", &["apk"])
                        .pick_file()
                    {
                        self.controller
                            .send(Command::InstallAndroid(path, self.device_id.clone()));
                    }
                }
                if self.platform == Platform::Ios {
                    ui.small("Install with Xcode; open Screen Test on the iPhone. USB only.");
                }
            }
            tool_window::section(ui, "Capture conditions");
            egui::ComboBox::from_id_salt("screen.color_mode")
                .selected_text(self.mode.label())
                .show_ui(ui, |ui| {
                    for mode in [
                        DisplayMode::Srgb,
                        DisplayMode::DisplayP3,
                        DisplayMode::LinearHdr,
                    ] {
                        ui.selectable_value(&mut self.mode, mode, mode.label());
                    }
                });
            ui.add(egui::Slider::new(&mut self.brightness, 0.0..=1.).text("Brightness"));
            if self.platform == Platform::Ios {
                ui.small(
                    "Brightness is held and Auto-Lock disabled while the test chart is shown.",
                );
                if let Some(guard) = self
                    .controller
                    .device
                    .as_ref()
                    .and_then(|device| device.brightness_guard.as_ref())
                {
                    if let Some(issue) = &guard.issue {
                        ui.small(issue);
                    } else if guard.holding && guard.settled {
                        ui.small(format!(
                            "Last confirmed brightness · {:.0}%",
                            guard.requested_brightness.unwrap_or_default() * 100.
                        ));
                    }
                }
            }
            if self.mode == DisplayMode::LinearHdr {
                ui.add(egui::Slider::new(&mut self.peak, 1.1..=8.).text("HDR peak / SDR white"));
            }
            if self.workflow != Workflow::Geometry {
                ui.add(
                    egui::TextEdit::singleline(&mut self.batch)
                        .hint_text("Batch / condition label")
                        .desired_width(f32::INFINITY),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.notes)
                        .hint_text("Conditions / notes")
                        .desired_width(f32::INFINITY),
                );
            }
            if self.workflow == Workflow::Inspection {
                egui::ComboBox::from_id_salt("screen.reference")
                    .selected_text(
                        self.controller
                            .references
                            .iter()
                            .find(|r| Some(&r.id) == self.baseline.as_ref())
                            .map(|r| r.name.as_str())
                            .unwrap_or("Select iPhone baseline"),
                    )
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for reference in &self.controller.references {
                            ui.selectable_value(
                                &mut self.baseline,
                                Some(reference.id.clone()),
                                format!(
                                    "{} · {} · {:.0}% · {}",
                                    reference.name,
                                    reference.conditions.manifest.mode.label(),
                                    reference.conditions.brightness * 100.,
                                    timestamp(reference.created_ms)
                                ),
                            );
                        }
                    });
                ui.checkbox(&mut self.thresholds_enabled, "Apply engineering thresholds");
                if self.thresholds_enabled {
                    ui.add(
                        egui::DragValue::new(&mut self.thresholds.rgb_fraction_of_white)
                            .speed(0.001)
                            .range(0.001..=1.)
                            .prefix("RGB / reference white "),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.thresholds.white_relative)
                            .speed(0.001)
                            .range(0.001..=1.)
                            .prefix("White response "),
                    );
                }
            }
            ui.small(
                "Captures use RAW. The camera's original photo format is restored after transfer.",
            );
            let connected = self.controller.device.is_some();
            ui.add_space(tool_window::GAP);
            ui.horizontal_wrapped(|ui| {
                if action(ui, "Show pattern", connected) {
                    self.prepare(false);
                }
                if action(
                    ui,
                    self.workflow.action(),
                    connected && capture_issue.is_none(),
                ) {
                    self.prepare(true);
                }
            });
        });
        if let Some(reason) = capture_issue {
            ui.label(
                egui::RichText::new(reason)
                    .size(11.)
                    .color(egui::Color32::from_rgb(230, 186, 112)),
            );
        }
        ui.small(&self.controller.message);
        if let Some(error) = &self.controller.error {
            ui.colored_label(egui::Color32::from_rgb(244, 135, 125), error);
        }
        if busy {
            ui.spinner();
        }
        if self.workflow == Workflow::Reference {
            tool_window::section(ui, "Reference library");
            ui.add(
                egui::TextEdit::singleline(&mut self.library_filter)
                    .hint_text("Filter date / brightness / batch / mode")
                    .desired_width(f32::INFINITY),
            );
            ui.small("RAW originals persist across App restarts. Select compatible captures to publish a baseline.");
            for capture in self
                .controller
                .captures
                .iter()
                .filter(|c| c.ticket.workflow == Workflow::Reference)
                .filter(|c| matches_filter(c, &self.library_filter))
                .take(100)
            {
                let mut selected = self.selected.contains(&capture.id);
                let text = format!(
                    "{} · {:.0}% · {}{}",
                    capture.ticket.conditions.manifest.mode.label(),
                    capture.ticket.conditions.brightness * 100.,
                    timestamp(capture.created_ms),
                    if capture.analysis_issue.is_some() {
                        " · needs analysis"
                    } else {
                        ""
                    }
                );
                if ui.checkbox(&mut selected, text).changed() {
                    if selected {
                        self.selected.insert(capture.id.clone());
                    } else {
                        self.selected.remove(&capture.id);
                    }
                }
                if let Some(issue) = &capture.analysis_issue {
                    ui.small(issue);
                }
            }
            if self.selected.len() == 1 {
                if let Some(capture) = self
                    .controller
                    .captures
                    .iter()
                    .find(|c| self.selected.contains(&c.id))
                {
                    if let Some(path) = capture
                        .artifacts
                        .iter()
                        .find(|p| p.ends_with("preview.png"))
                    {
                        if action(ui, "View normalized capture", true) {
                            self.controller.preview = Some(path.clone());
                        }
                    }
                }
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.baseline_name)
                    .hint_text("Baseline name")
                    .desired_width(f32::INFINITY),
            );
            if action(ui, "Publish baseline", !busy && !self.selected.is_empty()) {
                self.controller.send(Command::Publish(
                    self.baseline_name.clone(),
                    self.selected.iter().cloned().collect(),
                ));
            }
            if action(
                ui,
                "Analyze selected with session geometry",
                !busy && self.selected.len() == 1,
            ) {
                self.controller.send(Command::Reanalyze(
                    self.selected.first().unwrap().clone(),
                    None,
                    None,
                ));
            }
        } else if self.workflow == Workflow::Inspection {
            tool_window::section(ui, "Test exposures");
            if self.baseline.is_none() {
                ui.small("You can save RAW now and choose a baseline for comparison later.");
            }
            for capture in self
                .controller
                .captures
                .iter()
                .filter(|c| c.ticket.workflow == Workflow::Inspection && c.measurements.is_some())
                .take(100)
            {
                let mut selected = self.selected.contains(&capture.id);
                if ui
                    .checkbox(
                        &mut selected,
                        format!(
                            "{} · {}",
                            timestamp(capture.created_ms),
                            capture.ticket.conditions.batch
                        ),
                    )
                    .changed()
                {
                    if selected {
                        self.selected.insert(capture.id.clone());
                    } else {
                        self.selected.remove(&capture.id);
                    }
                }
            }
            if action(
                ui,
                "Merge exposures and inspect",
                !busy && !self.selected.is_empty(),
            ) {
                self.controller.send(Command::InspectExposures(
                    self.selected.iter().cloned().collect(),
                    self.thresholds_enabled.then(|| self.thresholds.clone()),
                ));
            }
            if action(
                ui,
                "Compare selected with chosen baseline",
                !busy && self.selected.len() == 1 && self.baseline.is_some(),
            ) {
                self.controller.send(Command::Reanalyze(
                    self.selected.first().unwrap().clone(),
                    self.baseline.clone(),
                    self.thresholds_enabled.then(|| self.thresholds.clone()),
                ));
            }
            ui.small("For clipped/shadow patches, adjust shutter only and add an exposure. Keep ISO, aperture, phone brightness and placement fixed.");
            ui.label("Inspection reports");
            for report in &self.controller.reports {
                if ui
                    .small_button(format!(
                        "{} · {}",
                        timestamp(report.created_ms),
                        report.conclusion
                    ))
                    .clicked()
                {
                    self.report = Some(report.clone());
                }
            }
            for capture in self
                .controller
                .captures
                .iter()
                .filter(|c| c.ticket.workflow == Workflow::Inspection && c.analysis_issue.is_some())
                .take(30)
            {
                ui.small(format!(
                    "{} · {}",
                    timestamp(capture.created_ms),
                    capture.analysis_issue.as_deref().unwrap_or_default()
                ));
                let id = capture.id.clone();
                if ui
                    .add_enabled(!busy, egui::Button::new("Reanalyze with session geometry"))
                    .clicked()
                {
                    self.controller.send(Command::Reanalyze(
                        id,
                        self.baseline.clone(),
                        self.thresholds_enabled.then(|| self.thresholds.clone()),
                    ));
                    break;
                }
            }
        }
        if self.workflow != Workflow::Geometry {
            ui.collapsing("Saved images", |ui| {
                for capture in self
                    .controller
                    .captures
                    .iter()
                    .filter(|c| c.ticket.workflow == self.workflow)
                    .take(30)
                {
                    ui.push_id(&capture.id, |ui| {
                        ui.small(format!(
                            "{} · {}",
                            timestamp(capture.created_ms),
                            capture.ticket.conditions.batch
                        ));
                        ui.horizontal(|ui| {
                            for (label, name) in [
                                ("Sensor", "sensor-preview.png"),
                                ("Normalized", "preview.png"),
                                ("Geometry", "geometry-overlay.png"),
                            ] {
                                if let Some(path) =
                                    capture.artifacts.iter().find(|p| p.ends_with(name))
                                {
                                    if action(ui, label, true) {
                                        self.controller.preview = Some(path.clone());
                                    }
                                }
                            }
                        });
                    });
                }
            });
            if action(ui, "Refresh library", !busy) {
                self.controller.send(Command::Refresh);
            }
        }
        if let Some(report) = &self.report {
            let mut open = true;
            egui::Window::new("Screen inspection report")
                .open(&mut open)
                .default_width(650.)
                .max_height(720.)
                .vscroll(true)
                .frame(tool_window::dialog_frame(ui.ctx()))
                .show(ui.ctx(), |ui| {
                    tool_window::apply(ui);
                    ui.heading(&report.conclusion);
                    ui.small(format!("Baseline: {}", report.reference));
                    ui.small(format!("Capture: {}", report.capture));
                    ui.label(format!(
                        "{} → {} · {}",
                        report.test_conditions.device.name,
                        report.reference_conditions.device.name,
                        report.test_conditions.manifest.mode.label()
                    ));
                    ui.small(format!(
                        "{} source exposure(s) · {} reference capture(s)",
                        report.source_captures.len(),
                        report.reference_members.len()
                    ));
                    if let Some(g) = &report.geometry {
                        ui.small(format!(
                            "Geometry v{} · holdout {:.2} px",
                            g.revision, g.holdout_rms
                        ));
                    }
                    if let Some(white) = report.white_relative {
                        ui.label(format!("White response vs iPhone: {:+.2}%", white * 100.));
                    }
                    for issue in &report.issues {
                        ui.colored_label(egui::Color32::LIGHT_RED, issue);
                    }
                    if action(ui, "Copy report JSON", true) {
                        if let Ok(text) = serde_json::to_string_pretty(report) {
                            ui.ctx().copy_text(text);
                        }
                    }
                    let normalized = super::repository::default_root()
                        .join("results")
                        .join(&report.id)
                        .join("preview.png");
                    if normalized.exists() && action(ui, "View merged normalized image", true) {
                        self.controller.preview = Some(normalized);
                    }
                    ui.horizontal(|ui| {
                        for (label,id,name) in [("Reference", &report.reference, "preview.png"),("Test", &report.capture, "preview.png"),("Difference map", &report.id, "difference-map.png")] {
                            let path=super::repository::default_root().join("results").join(id).join(name);
                            if action(ui,label,path.exists()) { self.controller.preview=Some(path); }
                        }
                    });
                    ui.small("Difference map: fixed 0–10% of reference white; amber means unmeasured. This shows patch differences, not per-pixel image registration.");
                    ui.collapsing(
                        "Patch locations · red: outside threshold, amber: unmeasured",
                        |ui| {
                            patch_map(ui, report);
                        },
                    );
                    egui::ScrollArea::vertical()
                        .max_height(420.)
                        .show(ui, |ui| {
                            egui::Grid::new("screen.patch_report")
                                .striped(true)
                                .min_row_height(tool_window::ROW_HEIGHT)
                                .spacing([12.0, 0.0])
                                .show(ui, |ui| {
                                    ui.label("Patch");
                                    ui.label("Max RGB / reference white");
                                    ui.label("Flag");
                                    ui.end_row();
                                    for p in &report.differences {
                                        ui.label(p.id.to_string());
                                        ui.label(format!("{:.2}%", p.max_fraction_of_white * 100.));
                                        ui.label(if p.flagged {
                                            "Outside threshold"
                                        } else {
                                            "—"
                                        });
                                        ui.end_row();
                                    }
                                });
                        });
                });
            if !open {
                self.report = None;
            }
        }
    }
    fn prepare(&mut self, capture: bool) {
        self.controller.send(Command::Prepare(Prepare {
            workflow: self.workflow,
            mode: self.mode,
            peak: self.peak,
            brightness: self.brightness,
            batch: self.batch.clone(),
            notes: self.notes.clone(),
            reference: if self.workflow == Workflow::Inspection {
                self.baseline.clone()
            } else {
                None
            },
            thresholds: (self.workflow == Workflow::Inspection && self.thresholds_enabled)
                .then(|| self.thresholds.clone()),
            capture,
        }));
    }
}

fn matches_filter(capture: &StoredCapture, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    let c = &capture.ticket.conditions;
    format!(
        "{} {} {} {} {:.0}% {}",
        timestamp(capture.created_ms),
        c.device.name,
        c.batch,
        c.manifest.mode.label(),
        c.brightness * 100.,
        c.notes
    )
    .to_lowercase()
    .contains(&query.to_lowercase())
}

fn timestamp(ms: u64) -> String {
    #[cfg(unix)]
    {
        let seconds = (ms / 1000) as libc::time_t;
        let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
        // localtime_r writes to caller-owned storage and reports an out-of-range timestamp.
        if !unsafe { libc::localtime_r(&seconds, local.as_mut_ptr()) }.is_null() {
            let date = unsafe { local.assume_init() };
            return format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                date.tm_year + 1900,
                date.tm_mon + 1,
                date.tm_mday,
                date.tm_hour,
                date.tm_min,
                date.tm_sec
            );
        }
    }
    format!("Unix {} s", ms / 1000)
}

fn patch_map(ui: &mut egui::Ui, report: &InspectionReport) {
    let manifest = &report.test_conditions.manifest;
    let scale =
        (320. / manifest.size[1] as f32).min(ui.available_width() / manifest.size[0] as f32);
    let (canvas, _) = ui.allocate_exact_size(
        egui::vec2(
            manifest.size[0] as f32 * scale,
            manifest.size[1] as f32 * scale,
        ),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(canvas, 2., egui::Color32::from_gray(25));
    for patch in &manifest.patches {
        let [x, y, w, h] = patch.rect;
        let rect = egui::Rect::from_min_size(
            canvas.min + egui::vec2(x as f32, (manifest.size[1] - y - h) as f32) * scale,
            egui::vec2(w as f32, h as f32) * scale,
        );
        let difference = report.differences.iter().find(|p| p.id == patch.id);
        let color = match difference {
            Some(p) if p.flagged => egui::Color32::from_rgb(210, 65, 55),
            Some(_) => egui::Color32::from_gray(85),
            None => egui::Color32::from_rgb(180, 130, 35),
        };
        ui.painter().rect_filled(rect, 0., color);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            patch.id.to_string(),
            egui::FontId::proportional(9.),
            egui::Color32::WHITE,
        );
        ui.interact(
            rect,
            ui.id().with(("patch", patch.id)),
            egui::Sense::hover(),
        )
        .on_hover_text(
            difference
                .map(|p| {
                    format!(
                        "Patch {} · RGB / reference white {:.2}%",
                        p.id,
                        p.max_fraction_of_white * 100.
                    )
                })
                .unwrap_or_else(|| format!("Patch {} · no valid measurement", patch.id)),
        );
    }
}
