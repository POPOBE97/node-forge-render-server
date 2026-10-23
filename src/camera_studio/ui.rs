use super::{
    model::*,
    service::{CameraService, default_directory},
};
use crate::screen_test::model::{DisplayGeometry, session_geometry};
use crate::ui::{
    button::{ButtonOptions, ButtonSize, ButtonVariant, button, button_with_width},
    components::{
        radio_button_group::{RadioButtonOption, radio_button_group},
        two_column_section as section,
    },
    design_tokens::{self as tokens, TextRole},
    tool_window,
};
use image::{ImageDecoder, metadata::Orientation};
use rust_wgpu_fiber::eframe::egui::{self, Color32};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

mod controls;
mod fixture;
mod gallery;
mod geometry;
mod media;
mod results;
#[cfg(test)]
mod tests;
mod view;
use media::{PhotoLoader, Thumbnails};
use view::{ImageTransform, ScaleMode};

const PANEL: Color32 = Color32::from_gray(23);
const CANVAS: Color32 = Color32::from_gray(16);
const MUTED: Color32 = Color32::from_gray(150);
const ERROR: Color32 = Color32::from_rgb(244, 135, 125);
const RED: Color32 = Color32::from_rgb(240, 90, 90);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ShootingMode {
    #[default]
    Photo,
    Video,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum CanvasSource {
    #[default]
    Live,
    Photo(PathBuf),
    Record(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum IssueTarget {
    Device,
    Preview,
    Exposure,
    Focus,
    Output,
    Shooting,
    Record(String),
}
struct Issue {
    target: IssueTarget,
    summary: String,
    detail: String,
}
struct Transfer {
    name: String,
    fraction: Option<f32>,
}
struct ImageView {
    texture: egui::TextureHandle,
    size: [u32; 2],
    label: String,
    sequence: u64,
    received: Instant,
    orientation: Orientation,
}
type PhotoResult = Result<(PathBuf, [u32; 2], Vec<u8>, Orientation), String>;

pub(super) struct Workspace {
    service: Option<CameraService>,
    screen_test: Option<crate::screen_test::ui::Panel>,
    screen_test_open: bool,
    screen_preview_source: Option<(crate::screen_test::model::Workflow, CanvasSource)>,
    pending_screen_capture: Option<crate::screen_test::controller::PreparedCapture>,
    screen_camera_connect_requested: bool,
    calibration_running: bool,
    calibration_result: Option<Result<String, String>>,
    display_geometry: Option<std::sync::Arc<DisplayGeometry>>,
    geometry_enabled: bool,
    results: results::Results,
    status: CameraStatus,
    records: Vec<CaptureRecord>,
    directory: PathBuf,
    release_occupied: bool,
    pattern_id: String,
    issues: Vec<Issue>,
    diagnostics: VecDeque<String>,
    diagnostics_open: bool,
    error_detail: Option<String>,
    transfers: HashMap<String, Transfer>,
    live: Option<ImageView>,
    photo: Option<ImageView>,
    photos: PhotoLoader,
    thumbnails: Thumbnails,
    history_job: Option<mpsc::Receiver<Result<Vec<CaptureRecord>, String>>>,
    source: CanvasSource,
    shooting_mode: ShootingMode,
    live_view: ImageTransform,
    photo_view: ImageTransform,
    selected_record: Option<String>,
    detail_record: Option<String>,
    strip_open: bool,
    follow_latest: bool,
    scroll_latest: bool,
    focus_step: u8,
    focus_feedback: Option<(String, Instant)>,
    close_dialog: bool,
    close_after_work: bool,
    shutdown_sent: bool,
    parent_to_close: Option<egui::ViewportId>,
    pub closed: bool,
    fixture: Option<String>,
    fixture_image: Option<PathBuf>,
    fixture_capture_countdown: Option<u8>,
    fixture_capture_batch: VecDeque<(String, egui::Vec2)>,
}

fn action(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    button(
        ui,
        ButtonOptions {
            enabled,
            variant: ButtonVariant::Ghost,
            ..ButtonOptions::new(label)
        },
    )
    .clicked()
}
fn wide_action(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    large: bool,
    recording: bool,
) -> bool {
    button_with_width(
        ui,
        ButtonOptions {
            enabled,
            size: if large {
                ButtonSize::Large
            } else {
                ButtonSize::Default
            },
            variant: if recording {
                ButtonVariant::Destructive
            } else {
                ButtonVariant::Secondary
            },
            ..ButtonOptions::new(label)
        },
        ui.available_width(),
    )
    .clicked()
}
fn muted(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(tokens::rich_text(text, TextRole::AttributeTitle));
}
fn divider(ui: &mut egui::Ui) {
    ui.add_space(8.);
    ui.separator();
    ui.add_space(4.);
}
fn display_value(value: &ParameterValue) -> String {
    match value {
        ParameterValue::Choice(s) | ParameterValue::Text(s) => s.clone(),
        ParameterValue::Toggle(v) => if *v { "On" } else { "Off" }.into(),
        ParameterValue::Number(v) => format!("{v}"),
    }
}
fn reveal(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("explorer");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    command.arg(path).spawn().map(|_| ())
}

impl Workspace {
    pub fn new() -> Self {
        let mut workspace = Self {
            service: None,
            screen_test: None,
            screen_test_open: false,
            screen_preview_source: None,
            pending_screen_capture: None,
            screen_camera_connect_requested: false,
            calibration_running: false,
            calibration_result: None,
            display_geometry: None,
            geometry_enabled: true,
            results: results::Results::default(),
            status: CameraStatus::default(),
            records: vec![],
            directory: default_directory(),
            release_occupied: cfg!(target_os = "macos"),
            pattern_id: String::new(),
            issues: vec![],
            diagnostics: VecDeque::new(),
            diagnostics_open: false,
            error_detail: None,
            transfers: HashMap::new(),
            live: None,
            photo: None,
            photos: PhotoLoader::default(),
            thumbnails: Thumbnails::default(),
            history_job: None,
            source: CanvasSource::Live,
            shooting_mode: ShootingMode::Photo,
            live_view: ImageTransform::default(),
            photo_view: ImageTransform::default(),
            selected_record: None,
            detail_record: None,
            strip_open: true,
            follow_latest: true,
            scroll_latest: false,
            focus_step: 1,
            focus_feedback: None,
            close_dialog: false,
            close_after_work: false,
            shutdown_sent: false,
            parent_to_close: None,
            closed: false,
            fixture: None,
            fixture_image: None,
            fixture_capture_countdown: None,
            fixture_capture_batch: VecDeque::new(),
        };
        workspace.load_history();
        workspace
    }
    fn log(&mut self, message: String) {
        if self.diagnostics.len() == 100 {
            self.diagnostics.pop_front();
        }
        self.diagnostics.push_back(message);
    }
    fn issue(
        &mut self,
        target: IssueTarget,
        summary: impl Into<String>,
        detail: impl Into<String>,
    ) {
        let detail = detail.into();
        self.log(detail.clone());
        self.issues.retain(|issue| issue.target != target);
        self.issues.push(Issue {
            target,
            summary: summary.into(),
            detail,
        });
    }
    fn target(operation: &CameraOperation, record: Option<&String>) -> IssueTarget {
        match operation {
            CameraOperation::StartingPreview | CameraOperation::StoppingPreview => {
                IssueTarget::Preview
            }
            CameraOperation::SettingParameter(id) => match id.as_str() {
                "iso" | "aperture" | "shutterspeed" | "whitebalance" | "exposurecompensation" => {
                    IssueTarget::Exposure
                }
                "continuousaf" | "afmethod" | "focusmode" | "movieservoaf" => IssueTarget::Focus,
                _ => IssueTarget::Output,
            },
            CameraOperation::Focusing => IssueTarget::Focus,
            CameraOperation::Capturing
            | CameraOperation::StartingRecording
            | CameraOperation::StoppingRecording
            | CameraOperation::WaitingFiles
            | CameraOperation::Downloading
            | CameraOperation::Recovering => record
                .map(|id| IssueTarget::Record(id.clone()))
                .unwrap_or(IssueTarget::Shooting),
            _ => IssueTarget::Device,
        }
    }
    fn report(&mut self, error: CameraError, operation: CameraOperation, record: Option<String>) {
        if operation == CameraOperation::Connecting && self.pending_screen_capture.is_some() {
            self.pending_screen_capture = None;
            if let Some(panel) = &mut self.screen_test {
                panel.controller.camera_error(error.message.clone());
            }
        }
        let target = Self::target(&operation, record.as_ref());
        let summary = match &target {
            IssueTarget::Device => match error.kind {
                CameraErrorKind::NotFound => {
                    "Canon T4i / 650D not found. Check its USB connection and power."
                }
                CameraErrorKind::Occupied => {
                    "Camera USB is still occupied. Close the other camera client and reconnect."
                }
                CameraErrorKind::Disconnected => {
                    "The camera disconnected. Check its USB connection and power."
                }
                CameraErrorKind::Busy | CameraErrorKind::Invalid => &error.message,
                CameraErrorKind::Unavailable => {
                    "Camera driver is unavailable. Open Details for setup information."
                }
                _ => "Camera connection failed. Open Details for the cause.",
            },
            IssueTarget::Preview => "Preview is unavailable. Try again when the camera is ready.",
            IssueTarget::Exposure | IssueTarget::Output => {
                "The camera could not apply this setting."
            }
            IssueTarget::Focus => &error.message,
            IssueTarget::Record(_) if operation == CameraOperation::Downloading => {
                "Download failed — originals remain on the card"
            }
            IssueTarget::Record(_) | IssueTarget::Shooting => {
                "Capture needs attention — do not repeat the shutter"
            }
        };
        self.issue(
            target,
            summary,
            format!("{:?}: {}", error.kind, error.message),
        );
        self.close_after_work = false;
    }
    fn send(&mut self, command: CameraCommand) {
        if matches!(command, CameraCommand::Focus { .. }) {
            self.focus_feedback = None;
            self.return_live();
        }
        if self.fixture.is_some() {
            self.fixture_command(command);
            return;
        }
        let operation = command.operation();
        let record_id = match &command {
            CameraCommand::RetryDownloads { record_id } => Some(record_id.clone()),
            _ => self.status.active_record_id.clone(),
        };
        if self.service.is_none() && matches!(command, CameraCommand::Connect { .. }) {
            match CameraService::spawn() {
                Ok(service) => self.service = Some(service),
                Err(e) => {
                    self.report(e, operation, None);
                    return;
                }
            }
        }
        if let Some(service) = &self.service {
            match service.send(command) {
                Ok(()) => {
                    let target = Self::target(&operation, record_id.as_ref());
                    self.issues.retain(|issue| issue.target != target);
                    self.status.operation = operation;
                    self.status.active_record_id = record_id;
                }
                Err(e) => self.report(e, operation, record_id),
            }
        }
    }
    fn context(&self) -> BTreeMap<String, String> {
        let mut context = BTreeMap::new();
        if !self.pattern_id.trim().is_empty() {
            context.insert("pattern_id".into(), self.pattern_id.trim().into());
        }
        if let Some(geometry) = &self.display_geometry {
            context.insert("display_geometry_id".into(), geometry.profile.id.clone());
        }
        context
    }
    fn busy_capture(&self) -> bool {
        self.results.busy()
            || self.calibration_running
            || self.status.photo_format_restore_pending
            || self.screen_test.as_ref().is_some_and(|p| p.controller.busy)
            || self.status.recording
            || self.status.operation.is_busy()
            || self.status.pending_downloads > 0
    }
    fn screen_capture_issue(&self, geometry: bool) -> Option<String> {
        if self.status.operation.is_busy()
            || self.status.photo_format_restore_pending
            || self.status.recording
            || self.status.pending_downloads > 0
            || self.shutdown_sent
            || self.close_after_work
        {
            return Some("Wait for the current camera task to finish.".into());
        }
        let Some(info) = &self.status.info else {
            return (!geometry).then(|| "Connect the Canon camera to capture.".into());
        };
        if let Err(error) = info.raw_capture_format() {
            return Some(error.message);
        }
        if !geometry {
            if info
                .parameters
                .iter()
                .any(|p| p.id == "iso" && display_value(&p.value).to_lowercase().contains("auto"))
            {
                return Some("Set a fixed ISO for measurement.".into());
            }
            if !info.parameters.iter().any(|p| {
                p.id == "autoexposuremode"
                    && matches!(display_value(&p.value).as_str(), "Manual" | "M")
            }) {
                return Some("Set the camera to manual exposure for measurement.".into());
            }
        }
        None
    }
    fn start_calibration(&mut self) {
        if self.screen_capture_issue(true).is_some() || self.busy_capture() {
            return;
        }
        self.calibration_running = true;
        self.calibration_result = None;
        self.screen_test_open = false;
        self.return_live();
        self.screen_test
            .get_or_insert_with(Default::default)
            .controller
            .send(crate::screen_test::controller::Command::Prepare(
                crate::screen_test::controller::Prepare::geometry(),
            ));
    }
    fn finish_calibration(&mut self) {
        if !self.calibration_running
            || self.status.photo_format_restore_pending
            || self.status.operation.is_busy()
        {
            return;
        }
        let Some(panel) = &self.screen_test else {
            return;
        };
        if panel.controller.busy {
            return;
        }
        self.calibration_result = Some(match &panel.controller.error {
            Some(error) => Err(error.clone()),
            None => Ok(panel
                .controller
                .geometry
                .lock()
                .ok()
                .and_then(|s| {
                    s.active
                        .as_ref()
                        .map(|g| format!("Calibrated · {:.2} px · ready to shoot", g.holdout_rms))
                })
                .unwrap_or_else(|| panel.controller.message.clone())),
        });
        self.calibration_running = false;
    }
    fn controls_idle(&self) -> bool {
        !self.status.operation.is_busy()
            && !self.status.photo_format_restore_pending
            && !self.calibration_running
            && !self.shutdown_sent
            && !self.close_after_work
            && !self.screen_test.as_ref().is_some_and(|p| p.controller.busy)
    }
    fn can_capture(&self) -> bool {
        self.status.info.is_some() && !self.busy_capture() && self.controls_idle()
    }
    fn shoot(&mut self) {
        if self.can_capture() && self.shooting_mode == ShootingMode::Photo {
            self.send(CameraCommand::Capture {
                directory: self.directory.clone(),
                context: self.context(),
            });
        }
    }
    fn shutdown(&mut self) {
        if self.service.is_some() {
            self.send(CameraCommand::Shutdown);
            self.shutdown_sent = true;
        } else {
            self.closed = true;
        }
    }
    pub fn request_close(&mut self, parent: Option<egui::ViewportId>) {
        if self.close_after_work || self.shutdown_sent {
            return;
        }
        self.parent_to_close = parent;
        if self.busy_capture() {
            self.close_dialog = true;
        } else {
            self.close_after_work = true;
        }
    }
    fn accept_event(&mut self, event: CameraEvent) {
        match event {
            CameraEvent::FocusAcknowledged { near, step } => {
                let text = format!(
                    "{} {step} · command accepted",
                    if near { "Near" } else { "Far" }
                );
                self.log(format!("Focus: {text}"));
                self.focus_feedback = Some((text, Instant::now()));
                self.issues
                    .retain(|issue| issue.target != IssueTarget::Focus);
            }
            CameraEvent::Status(status) => {
                if self.status.photo_format_restore_pending
                    && !status.photo_format_restore_pending
                    && status.info.is_some()
                {
                    self.issues
                        .retain(|issue| issue.target != IssueTarget::Output);
                }
                if status.recording {
                    self.shooting_mode = ShootingMode::Video;
                }
                self.status = status;
            }
            CameraEvent::Record(record) => {
                if let Some(panel) = &mut self.screen_test {
                    panel.controller.accept_record(&record);
                }
                if record
                    .context
                    .get("screen_test_ticket")
                    .and_then(|value| {
                        serde_json::from_str::<crate::screen_test::model::CaptureTicket>(value).ok()
                    })
                    .is_some_and(|ticket| {
                        ticket.workflow == crate::screen_test::model::Workflow::Geometry
                    })
                {
                    return;
                }
                if let Some(previous) = self.records.iter_mut().find(|r| r.id == record.id) {
                    *previous = record;
                } else {
                    self.records.push(record);
                    self.records.sort_by_key(|r| r.started_unix_ms);
                    self.scroll_latest = self.follow_latest;
                }
            }
            CameraEvent::Transfer {
                record_id,
                name,
                fraction,
            } => {
                self.transfers
                    .insert(record_id.clone(), Transfer { name, fraction });
                if fraction.is_none() || fraction == Some(1.0) {
                    self.issues
                        .retain(|issue| issue.target != IssueTarget::Record(record_id.clone()));
                }
            }
            CameraEvent::Error {
                error,
                operation,
                record_id,
            } => {
                if matches!(
                    operation,
                    CameraOperation::Capturing
                        | CameraOperation::WaitingFiles
                        | CameraOperation::Downloading
                ) {
                    if let Some(panel) = &mut self.screen_test {
                        panel.controller.camera_error(error.message.clone());
                    }
                }
                self.report(error, operation, record_id);
            }
            CameraEvent::Notice(text) => self.log(text),
            CameraEvent::Stopped => {
                self.service = None;
                self.status = CameraStatus::default();
                if self.shutdown_sent {
                    self.closed = true;
                }
            }
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(panel) = &mut self.screen_test {
            panel.controller.set_camera_ready(
                !self.status.operation.is_busy()
                    && !self.status.photo_format_restore_pending
                    && self.status.pending_downloads == 0,
            );
        }
        if let Some(panel) = &self.screen_test {
            if panel.controller.busy && self.screen_preview_source.is_none() {
                self.screen_preview_source = Some((panel.workflow, self.source.clone()));
            }
        }
        if let Some(prepared) = self.screen_test.as_mut().and_then(|p| p.controller.poll()) {
            self.pending_screen_capture = Some(prepared);
            self.screen_camera_connect_requested = false;
        }
        if self.pending_screen_capture.is_some() {
            if !self
                .screen_test
                .as_ref()
                .is_some_and(|p| p.controller.awaiting_camera())
            {
                self.pending_screen_capture = None;
            } else if self.status.info.is_none() && !self.screen_camera_connect_requested {
                self.screen_camera_connect_requested = true;
                if let Some(panel) = &mut self.screen_test {
                    panel.controller.message = "Connecting camera…".into();
                }
                self.send(CameraCommand::Connect {
                    directory: self.directory.clone(),
                    release_occupied: self.release_occupied,
                });
            } else if self.status.info.is_some()
                && !self.status.operation.is_busy()
                && !self.status.recording
                && self.status.pending_downloads == 0
                && !self.shutdown_sent
            {
                let prepared = self.pending_screen_capture.take().unwrap();
                let mut context = self.context();
                match serde_json::to_string(&prepared.ticket) {
                    Ok(value) => {
                        context.insert("screen_test_ticket".into(), value);
                        self.send(CameraCommand::CaptureRaw {
                            directory: prepared.directory.unwrap_or_else(|| self.directory.clone()),
                            context,
                        });
                    }
                    Err(e) => {
                        if let Some(p) = &mut self.screen_test {
                            p.controller.camera_error(e.to_string());
                        }
                    }
                }
            }
        }
        if let Some(path) = self
            .screen_test
            .as_mut()
            .and_then(|p| p.controller.preview.take())
        {
            let current = self
                .screen_preview_source
                .as_ref()
                .is_none_or(|(workflow, source)| {
                    *source == self.source
                        && self
                            .screen_test
                            .as_ref()
                            .is_some_and(|p| p.workflow == *workflow)
                });
            if !self.calibration_running && self.screen_test_open && current {
                self.open_photo(path);
                self.photo_view.turns = 0; // Screen outputs already have canonical orientation.
            }
        }
        if !self.screen_test.as_ref().is_some_and(|p| p.controller.busy) {
            self.screen_preview_source = None;
        }
        self.finish_calibration();
        self.sync_display_geometry();

        if let Some(result) = self.history_job.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.history_job = None;
            match result {
                Ok(records) => {
                    for record in records {
                        if !self.records.iter().any(|r| r.id == record.id) {
                            self.records.push(record);
                        }
                    }
                    self.records.sort_by_key(|r| r.started_unix_ms);
                }
                Err(e) => self.issue(IssueTarget::Output, "Could not load capture history", e),
            }
        }
        let events = self
            .service
            .as_ref()
            .map(|s| s.events.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            self.accept_event(event);
        }
        self.poll_results();
        if let Some(frame) = self.service.as_ref().and_then(CameraService::take_preview) {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.size[0] as usize, frame.size[1] as usize],
                &frame.rgba,
            );
            if let Some(view) = &mut self.live {
                view.texture.set(image, egui::TextureOptions::LINEAR);
                view.size = frame.size;
                view.sequence = frame.sequence;
                view.received = frame.received;
            } else {
                self.live = Some(ImageView {
                    texture: ctx.load_texture(
                        "sys.camera.live",
                        image,
                        egui::TextureOptions::LINEAR,
                    ),
                    size: frame.size,
                    label: "Live".into(),
                    sequence: frame.sequence,
                    received: frame.received,
                    orientation: Orientation::NoTransforms,
                });
            }
        }
        if let Some(result) = self.photos.poll() {
            match result {
                Ok((path, size, rgba, orientation))
                    if self.source == CanvasSource::Photo(path.clone()) =>
                {
                    self.photo = Some(ImageView {
                        texture: ctx.load_texture(
                            "sys.camera.photo",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [size[0] as usize, size[1] as usize],
                                &rgba,
                            ),
                            egui::TextureOptions::LINEAR,
                        ),
                        size,
                        label: path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into(),
                        sequence: 0,
                        received: Instant::now(),
                        orientation,
                    });
                }
                Ok(_) => {}
                Err(e) => self.issue(IssueTarget::Preview, "Could not open photograph", e),
            }
        }
        self.thumbnails.poll(ctx);
        if self.fixture.is_some() {
            self.fixture_screenshots(ctx);
        }
        if self.close_after_work && !self.busy_capture() && !self.shutdown_sent {
            self.shutdown();
        }
        if self.closed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            if let Some(parent) = self.parent_to_close {
                ctx.send_viewport_cmd_to(parent, egui::ViewportCommand::Close);
            }
        }
    }
    pub fn open_photo(&mut self, path: PathBuf) {
        self.source = CanvasSource::Photo(path.clone());
        self.photo = None;
        self.photo_view.fit();
        self.photo_view.turns = self.live_view.turns;
        self.photos.request(path);
    }
    fn return_live(&mut self) {
        self.source = CanvasSource::Live;
        self.photos.cancel();
        self.follow_latest = true;
    }
    fn load_history(&mut self) {
        let directory = self.directory.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(super::service::load_records(&directory).map_err(|e| e.to_string()));
        });
        self.history_job = Some(rx);
    }
    fn error_panel(&mut self, ui: &mut egui::Ui, target: IssueTarget) {
        if let Some(issue) = self.issues.iter().find(|issue| issue.target == target) {
            ui.add_space(4.);
            ui.label(egui::RichText::new(&issue.summary).size(11.).color(ERROR));
            if action(ui, "Details", true) {
                self.error_detail = Some(issue.detail.clone());
            }
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        if self.closed {
            return;
        }
        tool_window::apply(ui);
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_close(None);
        }
        self.shortcuts(&ctx);
        egui::Panel::left("camera.controls")
            .default_size(340.)
            .size_range(280.0..=420.0)
            .resizable(true)
            .frame(tool_window::panel_frame())
            .show_inside(ui, |ui| {
                self.device(ui);
                ui.add_space(8.);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!self.calibration_running, |ui| {
                        ui.selectable_value(&mut self.screen_test_open, false, "Capture");
                        ui.selectable_value(&mut self.screen_test_open, true, "Screen test");
                    });
                });
                if !self.screen_test_open {
                    self.shooting(ui);
                }
                divider(ui);
                egui::ScrollArea::vertical()
                    .id_salt("camera.parameters")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.screen_test_open {
                            let issue = self.screen_capture_issue(false);
                            self.screen_test
                                .get_or_insert_with(Default::default)
                                .show(ui, issue.as_deref());
                            self.error_panel(ui, IssueTarget::Output);
                            ui.collapsing("Camera settings", |ui| self.settings(ui));
                        } else {
                            self.settings(ui);
                        }
                    });
            });
        {
            egui::Panel::bottom("camera.filmstrip")
                .exact_size(if self.strip_open { 112. } else { 32. })
                .frame(
                    egui::Frame::new()
                        .fill(PANEL)
                        .inner_margin(egui::Margin::symmetric(12, 4)),
                )
                .show_inside(ui, |ui| self.gallery(ui));
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(CANVAS)
                    .inner_margin(egui::Margin::same(0)),
            )
            .show_inside(ui, |ui| self.canvas(ui));
        self.dialogs(&ctx);
        ctx.request_repaint_after(Duration::from_millis(if self.status.preview {
            16
        } else {
            100
        }));
    }
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.focused)
            || ctx.egui_wants_keyboard_input()
            || egui::Popup::is_any_open(ctx)
            || self.close_dialog
            || self.diagnostics_open
            || self.detail_record.is_some()
            || self.error_detail.is_some()
        {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
            && !self.screen_test_open
        {
            self.shoot();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.return_live();
        }
        let view = if self.source == CanvasSource::Live {
            &mut self.live_view
        } else {
            &mut self.photo_view
        };
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F)) {
            view.fit();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Num1)) {
            view.original();
        }
    }
}

fn decode_photo(path: &std::path::Path) -> anyhow::Result<([u32; 2], Vec<u8>, Orientation)> {
    // Reuse the existing ICC-aware original-image decoder. Display proxies never replace originals.
    let is_raw = path
        .extension()
        .is_some_and(|v| v.eq_ignore_ascii_case("cr2"));
    let bytes = if is_raw {
        crate::screen_test::raw::native(&[std::ffi::OsStr::new("preview"), path.as_os_str()])?
    } else {
        std::fs::read(path)?
    };
    let mut decoder = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()?
        .into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut decoded = crate::pixel_perfecto::io::decode(&bytes)?;
    // Some embedded JPEGs omit EXIF orientation; use the RAW's orientation for display only.
    let raw_orientation = if is_raw && orientation == Orientation::NoTransforms {
        match crate::screen_test::raw::display_info(path)?.flip {
            3 => Orientation::Rotate180,
            5 => Orientation::Rotate270,
            6 => Orientation::Rotate90,
            _ => Orientation::NoTransforms,
        }
    } else {
        Orientation::NoTransforms
    };
    let pixels = match decoded.pixels {
        crate::pixel_perfecto::io::Pixels::Srgb8(bytes) => bytes,
        crate::pixel_perfecto::io::Pixels::Linear(values) => values
            .chunks_exact(4)
            .flat_map(|p| {
                let alpha = p[3];
                let encoded = |v: f32| {
                    let v = if alpha > 0.0 { v / alpha } else { 0.0 };
                    let s = if v <= 0.0031308 {
                        12.92 * v
                    } else {
                        1.055 * v.powf(1.0 / 2.4) - 0.055
                    };
                    (s.clamp(0.0, 1.0) * 255.0).round() as u8
                };
                [
                    encoded(p[0]),
                    encoded(p[1]),
                    encoded(p[2]),
                    (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
                ]
            })
            .collect(),
    };
    let pixels = if raw_orientation != Orientation::NoTransforms {
        let mut image = image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(decoded.size[0], decoded.size[1], pixels)
                .ok_or_else(|| anyhow::anyhow!("Invalid RAW preview size"))?,
        );
        image.apply_orientation(raw_orientation);
        decoded.size = [image.width(), image.height()];
        image.into_rgba8().into_raw()
    } else {
        pixels
    };
    Ok((
        decoded.size,
        pixels,
        if raw_orientation != Orientation::NoTransforms {
            raw_orientation
        } else {
            orientation
        },
    ))
}
