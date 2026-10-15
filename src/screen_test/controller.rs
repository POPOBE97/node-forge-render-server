use super::{
    device::DeviceConnection, geometry, measurement, model::*, pattern::Pattern, raw::RawFrame,
    repository::Repository,
};
use crate::camera_studio::model::CaptureRecord;
use anyhow::{Context, Result, ensure};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct Prepare {
    pub workflow: Workflow,
    pub mode: DisplayMode,
    pub peak: f32,
    pub brightness: f32,
    pub batch: String,
    pub notes: String,
    pub reference: Option<String>,
    pub thresholds: Option<Thresholds>,
    pub capture: bool,
}
impl Prepare {
    pub fn geometry() -> Self {
        Self {
            workflow: Workflow::Geometry,
            mode: DisplayMode::Srgb,
            peak: 1.,
            brightness: 0.7,
            batch: String::new(),
            notes: String::new(),
            reference: None,
            thresholds: None,
            capture: true,
        }
    }
}
pub enum Command {
    Shutdown,
    Connect(Platform, String),
    Disconnect,
    InstallAndroid(PathBuf, String),
    Prepare(Prepare),
    Analyze(CaptureTicket, PathBuf, String),
    AbortGeometry(String),
    Reanalyze(String, Option<String>, Option<Thresholds>),
    InspectExposures(Vec<String>, Option<Thresholds>),
    Publish(String, Vec<String>),
    Refresh,
}
pub enum Event {
    Progress(String),
    Device(Option<DisplayDevice>),
    DisplayStatus(DisplayDevice),
    DisplayReleased,
    Ready(PreparedCapture),
    GeometryPreview(Vec<PathBuf>),
    Library(
        Vec<StoredCapture>,
        Vec<ReferenceSetVersion>,
        Vec<InspectionReport>,
    ),
    Finished(String, Option<PathBuf>),
    Error(String),
}
pub struct PreparedCapture {
    pub ticket: CaptureTicket,
    /// Geometry captures live only in the worker's temporary session directory.
    pub directory: Option<PathBuf>,
}
#[derive(Default)]
struct SessionFiles(Option<PathBuf>);
impl SessionFiles {
    fn root(&mut self) -> Result<PathBuf> {
        if let Some(root) = &self.0 {
            return Ok(root.clone());
        }
        let root = std::env::temp_dir().join(new_id("node-forge-geometry"));
        std::fs::create_dir(&root)?;
        self.0 = Some(root.clone());
        Ok(root)
    }
}
impl Drop for SessionFiles {
    fn drop(&mut self) {
        if let Some(root) = &self.0 {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
pub struct Controller {
    tx: mpsc::Sender<Command>,
    worker: Option<std::thread::JoinHandle<()>>,
    rx: mpsc::Receiver<Event>,
    pub device: Option<DisplayDevice>,
    pub captures: Vec<StoredCapture>,
    pub references: Vec<ReferenceSetVersion>,
    pub reports: Vec<InspectionReport>,
    pub busy: bool,
    pub message: String,
    pub error: Option<String>,
    pub preview: Option<PathBuf>,
    pub geometry_images: Vec<PathBuf>,
    pub geometry: Arc<Mutex<SessionGeometryStore>>,
    pending: Option<(CaptureTicket, Instant)>,
    seen: HashSet<String>,
    recoverable: HashMap<String, CaptureTicket>,
    downloaded: VecDeque<(CaptureTicket, PathBuf, String)>,
    library_requested: bool,
    camera_ready: bool,
}
impl Controller {
    /// UI acceptance only: no worker, repository or USB connection exists.
    pub(crate) fn preview() -> Self {
        let (tx, _) = mpsc::channel();
        let (_, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            worker: None,
            device: None,
            captures: vec![],
            references: vec![],
            reports: vec![],
            busy: false,
            message: "UI preview · simulated display · no USB access".into(),
            error: None,
            preview: None,
            geometry_images: vec![],
            geometry: Arc::new(Mutex::new(SessionGeometryStore::default())),
            pending: None,
            seen: HashSet::new(),
            recoverable: HashMap::new(),
            downloaded: VecDeque::new(),
            library_requested: true,
            camera_ready: true,
        }
    }
    pub fn new() -> Self {
        Self::at(super::repository::default_root(), session_geometry())
    }
    pub fn at(root: PathBuf, geometry: Arc<Mutex<SessionGeometryStore>>) -> Self {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let store = geometry.clone();
        let worker = std::thread::spawn(move || worker(root, store, commands, events));
        let result = Self {
            tx,
            worker: Some(worker),
            rx,
            device: None,
            captures: vec![],
            references: vec![],
            reports: vec![],
            busy: false,
            message: "Connect a display to begin".into(),
            error: None,
            preview: None,
            geometry_images: vec![],
            geometry,
            pending: None,
            seen: HashSet::new(),
            recoverable: HashMap::new(),
            downloaded: VecDeque::new(),
            library_requested: false,
            camera_ready: true,
        };
        result
    }
    pub fn load_library_if_needed(&mut self) {
        if !self.library_requested && !self.busy {
            self.send(Command::Refresh);
        }
    }
    pub fn send(&mut self, command: Command) {
        if self.busy {
            return;
        }
        self.error = None;
        if matches!(command, Command::Refresh) {
            self.library_requested = true;
        }
        self.busy = true;
        self.message = "Working…".into();
        if self.tx.send(command).is_err() {
            self.busy = false;
            self.error = Some("Screen Test worker stopped".into());
        }
    }
    pub fn poll(&mut self) -> Option<PreparedCapture> {
        let mut ready = None;
        for event in self.rx.try_iter() {
            match event {
                Event::Progress(message) => self.message = message,
                Event::Device(device) => {
                    self.device = device;
                    self.busy = false;
                    self.message = "Display connected".into();
                }
                Event::Ready(prepared) => {
                    self.camera_ready = false;
                    let ticket = &prepared.ticket;
                    self.device = Some(ticket.conditions.device.clone());
                    self.message = "Waiting for camera RAW…".into();
                    self.recoverable.insert(ticket.id.clone(), ticket.clone());
                    self.pending = Some((ticket.clone(), Instant::now()));
                    ready = Some(prepared);
                }
                Event::GeometryPreview(images) => self.geometry_images = images,
                Event::DisplayStatus(device) => self.device = Some(device),
                Event::DisplayReleased => self.device = None,
                Event::Library(c, r, reports) => {
                    self.captures = c;
                    self.references = r;
                    self.reports = reports;
                }
                Event::Finished(message, preview) => {
                    self.message = message;
                    self.preview = preview;
                    self.busy = false;
                }
                Event::Error(message) => {
                    self.error = Some(message);
                    self.busy = false;
                    self.pending = None;
                }
            }
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(90))
        {
            self.camera_error("RAW did not arrive. Keep the original capture; recover downloads before retrying analysis. Shutter was not repeated.".into());
        }
        self.start_downloaded();
        ready
    }
    pub fn camera_error(&mut self, message: String) {
        if let Some((ticket, _)) = self.pending.take() {
            if ticket.workflow == Workflow::Geometry {
                // Failed geometry must not analyze a late download after restoring the phone.
                self.recoverable.remove(&ticket.id);
                self.downloaded
                    .retain(|(queued, _, _)| queued.id != ticket.id);
                self.message = "Restoring phone…".into();
                if self
                    .tx
                    .send(Command::AbortGeometry(message.clone()))
                    .is_ok()
                {
                    return;
                }
            }
            self.busy = false;
            self.error = Some(message);
        }
    }
    pub fn set_camera_ready(&mut self, ready: bool) {
        self.camera_ready = ready;
        if !ready && !self.downloaded.is_empty() {
            self.message = "Restoring camera settings…".into();
        }
    }
    pub fn awaiting_camera(&self) -> bool {
        self.pending.is_some()
    }
    pub fn accept_record(&mut self, record: &CaptureRecord) {
        let Some(ticket) = record
            .context
            .get("screen_test_ticket")
            .and_then(|s| serde_json::from_str::<CaptureTicket>(s).ok())
            .and_then(|t| self.recoverable.get(&t.id).cloned())
        else {
            return;
        };
        let Some(raw) = record.files.iter().find(|f| f.remote.extension() == "cr2") else {
            return;
        };
        if !self.seen.insert(record.id.clone()) {
            return;
        }
        self.recoverable.remove(&ticket.id);
        self.downloaded
            .push_back((ticket, raw.path.clone(), record.id.clone()));
    }
    fn start_downloaded(&mut self) {
        if !self.camera_ready {
            return;
        }
        let index = self.downloaded.iter().position(|(ticket, _, _)| {
            !self.busy
                || self
                    .pending
                    .as_ref()
                    .is_some_and(|(pending, _)| pending.id == ticket.id)
        });
        let Some(index) = index else {
            return;
        };
        let (ticket, path, record) = self.downloaded.remove(index).unwrap();
        if self
            .pending
            .as_ref()
            .is_some_and(|(pending, _)| pending.id == ticket.id)
        {
            self.pending = None;
        }
        self.message = "Analyzing RAW…".into();
        self.busy = true;
        if self
            .tx
            .send(Command::Analyze(ticket, path, record))
            .is_err()
        {
            self.error = Some("Analysis worker stopped".into());
            self.busy = false;
        }
    }
}
fn library(repo: &Repository, events: &mpsc::Sender<Event>) -> Result<()> {
    let _ = events.send(Event::Library(
        repo.captures()?,
        repo.references()?,
        repo.reports()?,
    ));
    Ok(())
}
fn worker(
    root: PathBuf,
    geometry: Arc<Mutex<SessionGeometryStore>>,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
) {
    let mut connection: Option<DeviceConnection> = None;
    let mut session_files = SessionFiles::default();
    let mut geometry_active = false;
    for command in commands {
        if matches!(command, Command::Shutdown) {
            break;
        }
        let result = (|| -> Result<()> {
            match command {
                Command::Shutdown => {}
                Command::AbortGeometry(message) => {
                    anyhow::bail!(message);
                }
                Command::Connect(platform, id) => {
                    connection = Some(DeviceConnection::connect(platform, &id)?);
                    let _ =
                        events.send(Event::Device(connection.as_ref().map(|c| c.device.clone())));
                }
                Command::Disconnect => {
                    connection = None;
                    let _ = events.send(Event::Device(None));
                }
                Command::InstallAndroid(path, id) => {
                    DeviceConnection::install_android(&path, &id)?;
                    let _ = events.send(Event::Finished("Android player installed".into(), None));
                }
                Command::Refresh => {
                    let repo = Repository::open(&root)?;
                    library(&repo, &events)?;
                    let _ = events.send(Event::Finished("Reference library loaded".into(), None));
                }
                Command::Prepare(mut request) => {
                    if request.capture && request.workflow != Workflow::Geometry {
                        ensure!(
                            geometry
                                .lock()
                                .map_err(|_| anyhow::anyhow!("Session geometry lock failed"))?
                                .active
                                .is_some(),
                            "Calibrate geometry on the canvas before capturing color references or tests"
                        );
                    }
                    geometry_active = request.workflow == Workflow::Geometry;
                    if request.workflow == Workflow::Geometry && connection.is_none() {
                        let _ =
                            events.send(Event::Progress("Preparing phone app over USB…".into()));
                        connection = Some(DeviceConnection::connect_available()?);
                    }
                    let connection = connection.as_mut().context("Connect a display first")?;
                    ensure!(
                        request.workflow != Workflow::Reference
                            || connection.device.platform == Platform::Ios,
                        "Reference calibration requires the iPhone"
                    );
                    ensure!(
                        request.workflow != Workflow::Inspection
                            || connection.device.platform == Platform::Android,
                        "Color inspection requires Android"
                    );
                    let status = connection.status()?;
                    if request.workflow == Workflow::Geometry {
                        // Geometry uses neutral markers; color-test preferences do not apply.
                        request.mode = if status.modes.contains(&DisplayMode::Srgb) {
                            DisplayMode::Srgb
                        } else {
                            DisplayMode::DisplayP3
                        };
                        request.brightness = Prepare::geometry().brightness;
                        request.peak = 1.;
                    }
                    ensure!(
                        status.modes.contains(&request.mode),
                        "Requested display mode is unavailable"
                    );
                    let pattern = Pattern::generate(&status, request.mode, request.peak)?;
                    let _ = events.send(Event::Progress("Showing calibration chart…".into()));
                    let temporary = (request.workflow == Workflow::Geometry)
                        .then(|| session_files.root())
                        .transpose()?;
                    let folder = temporary
                        .as_ref()
                        .unwrap_or(&root)
                        .join("patterns")
                        .join(&pattern.manifest.generation);
                    pattern.save(&folder)?;
                    let actual = connection.present(&pattern, request.brightness)?;
                    let _ = events.send(Event::DisplayStatus(actual.clone()));
                    let ticket = CaptureTicket {
                        id: new_id("screen-capture"),
                        workflow: request.workflow,
                        conditions: CaptureConditions {
                            device: actual,
                            brightness: request.brightness,
                            batch: request.batch,
                            notes: request.notes,
                            manifest: pattern.manifest,
                            camera_key: String::new(),
                        },
                        geometry: geometry
                            .lock()
                            .map_err(|_| anyhow::anyhow!("Session geometry lock failed"))?
                            .active
                            .clone(),
                        reference: request.reference,
                        thresholds: request.thresholds,
                    };
                    if request.capture {
                        let _ = events.send(Event::Ready(PreparedCapture {
                            ticket,
                            directory: temporary.map(|root| root.join("captures")),
                        }));
                    } else {
                        let _ = events.send(Event::Finished(
                            "Pattern displayed; no camera operation sent".into(),
                            Some(folder.join("preview.png")),
                        ));
                    }
                }
                Command::Analyze(ticket, path, record) => {
                    let display_issue = connection
                        .as_ref()
                        .context("Display disconnected during capture")
                        .and_then(|c| c.validate_capture(&ticket))
                        .err()
                        .map(|e| e.to_string());
                    if ticket.workflow == Workflow::Geometry {
                        if let Some(issue) = display_issue {
                            anyhow::bail!(issue);
                        }
                        let folder = session_files.root()?.join(new_id("correction"));
                        let images = calibrate_geometry(&geometry, &ticket, &path, &folder)?;
                        let _ = events.send(Event::Progress("Restoring phone…".into()));
                        release_geometry_display(&mut connection, &events)?;
                        geometry_active = false;
                        let _ = events.send(Event::GeometryPreview(images));
                        let _ = events.send(Event::Finished(
                            "Calibration complete · ready to shoot".into(),
                            None,
                        ));
                    } else {
                        let repo = Repository::open(&root)?;
                        let capture = analyze(&repo, ticket, path, record, display_issue)?;
                        finish_capture(&repo, capture, &events)?;
                        library(&repo, &events)?;
                    }
                }
                Command::Reanalyze(id, reference, thresholds) => {
                    let repo = Repository::open(&root)?;
                    let old = repo.capture(&id)?;
                    let path = repo.verify_raw(&old)?;
                    let mut ticket = old.ticket.clone();
                    ticket.id = new_id("reanalyze");
                    if ticket.workflow == Workflow::Inspection {
                        ticket.reference = reference.or(ticket.reference);
                        ticket.thresholds = thresholds;
                    }
                    ticket.geometry = geometry
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Session lock failed"))?
                        .active
                        .clone();
                    let capture =
                        analyze(&repo, ticket, path, old.camera_record, old.display_issue)?;
                    finish_capture(&repo, capture, &events)?;
                    library(&repo, &events)?;
                }
                Command::InspectExposures(ids, thresholds) => {
                    let repo = Repository::open(&root)?;
                    let captures = ids
                        .iter()
                        .map(|id| repo.capture(id))
                        .collect::<Result<Vec<_>>>()?;
                    for c in &captures {
                        repo.verify_raw(c)?;
                    }
                    let id = captures
                        .first()
                        .and_then(|c| c.ticket.reference.as_ref())
                        .context("Test captures need a selected baseline")?;
                    let reference = repo.reference(id)?;
                    let report = measurement::inspect_exposures(&captures, &reference, thresholds)?;
                    let folder = repo.root.join("results").join(&report.id);
                    export_capture_set(
                        &repo,
                        &captures,
                        reference.transform.as_ref().map(|t| t.matrix),
                        &folder,
                    )?;
                    repo.insert_report(&report)?;
                    library(&repo, &events)?;
                    let _ = events.send(Event::Finished(
                        format!("Exposure report: {}", report.conclusion),
                        Some(folder.join("preview.png")),
                    ));
                }
                Command::Publish(name, ids) => {
                    let mut repo = Repository::open(&root)?;
                    let captures = ids
                        .iter()
                        .map(|id| repo.capture(id))
                        .collect::<Result<Vec<_>>>()?;
                    let baseline = measurement::reference(name, &captures)?;
                    let folder = repo.root.join("results").join(&baseline.id);
                    export_capture_set(
                        &repo,
                        &captures,
                        baseline.transform.as_ref().map(|t| t.matrix),
                        &folder,
                    )?;
                    repo.insert_reference(&baseline)?;
                    library(&repo, &events)?;
                    let _ = events.send(Event::Finished(
                        format!(
                            "Baseline saved: {} ({} captures)",
                            baseline.name,
                            baseline.members.len()
                        ),
                        Some(folder.join("preview.png")),
                    ));
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            let cleanup = if geometry_active {
                geometry_active = false;
                release_geometry_display(&mut connection, &events).err()
            } else {
                None
            };
            let message = match cleanup {
                Some(cleanup) => format!("{error:#}. Phone restore failed: {cleanup:#}"),
                None => format!("{error:#}"),
            };
            let _ = events.send(Event::Error(message));
        }
    }
}
fn release_geometry_display(
    connection: &mut Option<DeviceConnection>,
    events: &mpsc::Sender<Event>,
) -> Result<()> {
    let result = connection
        .as_mut()
        .map(DeviceConnection::restore_after_calibration)
        .transpose();
    *connection = None;
    let _ = events.send(Event::DisplayReleased);
    result.map(|_| ())
}
fn export_capture_set(
    repo: &Repository,
    captures: &[StoredCapture],
    matrix: Option<[[f64; 3]; 3]>,
    folder: &std::path::Path,
) -> Result<()> {
    // Repeated captures all contribute CFA statistics. The image export needs one representative
    // per shutter time; choose the one with the most valid patches, without loading every RAW.
    let mut exposures = std::collections::BTreeMap::<u64, &StoredCapture>::new();
    for capture in captures {
        repo.verify_rectified(capture)?;
        let measured = capture
            .measurements
            .as_ref()
            .context("Unanalyzed exposure")?;
        let key = measured.raw.exposure.to_bits();
        let valid = measured.patches.iter().filter(|p| p.valid).count();
        if exposures.get(&key).is_none_or(|old| {
            old.measurements
                .as_ref()
                .unwrap()
                .patches
                .iter()
                .filter(|p| p.valid)
                .count()
                < valid
        }) {
            exposures.insert(key, capture);
        }
    }
    ensure!(
        exposures.len() <= 16,
        "Select at most 16 distinct shutter times for one normalized image"
    );
    let chosen: Vec<_> = exposures.into_values().collect();
    let raw = chosen
        .iter()
        .map(|c| RawFrame::decode(&repo.verify_raw(c)?))
        .collect::<Result<Vec<_>>>()?;
    let sources = raw
        .iter()
        .zip(&chosen)
        .map(|(r, c)| {
            Ok((
                r,
                c.ticket
                    .geometry
                    .as_ref()
                    .context("Exposure has no geometry")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    std::fs::create_dir_all(folder)?;
    measurement::export_exposures(
        &sources,
        &captures[0].ticket.conditions.manifest,
        matrix,
        folder,
    )?;
    repo.write_json(&folder.strip_prefix(&repo.root)?.join("members.json"), &serde_json::json!({"all_statistics_members":captures.iter().map(|c| &c.id).collect::<Vec<_>>(),"image_exposure_members":chosen.iter().map(|c| &c.id).collect::<Vec<_>>()}))?;
    Ok(())
}
fn calibrate_geometry(
    session: &Arc<Mutex<SessionGeometryStore>>,
    ticket: &CaptureTicket,
    path: &std::path::Path,
    folder: &std::path::Path,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(folder)?;
    let raw = RawFrame::decode(path)?;
    let detected = geometry::detect(&raw, folder)?;
    let manifest = &ticket.conditions.manifest;
    let profile = geometry::fit(&raw, manifest, &detected, &ticket.id)?;
    measurement::export(&raw, &profile, manifest, None, folder)?;
    geometry::export_overlay(&raw, manifest, &detected, &profile, folder)?;
    let display = super::raw::display_info(path)?;
    // Publish only once the fit and previews succeed. No repository or RAW import is involved.
    session
        .lock()
        .map_err(|_| anyhow::anyhow!("Session lock failed"))?
        .publish_display(profile, manifest.size, display.sensor_crop)?;
    Ok(
        ["sensor-preview.png", "preview.png", "geometry-overlay.png"]
            .into_iter()
            .map(|name| folder.join(name))
            .collect(),
    )
}

pub(crate) fn analyze(
    repo: &Repository,
    mut ticket: CaptureTicket,
    path: PathBuf,
    camera_record: String,
    display_issue: Option<String>,
) -> Result<StoredCapture> {
    ensure!(
        ticket.workflow != Workflow::Geometry,
        "Geometry is session-only; recalibrate instead of loading it from the library"
    );
    let (relative, hash) = repo.import_raw(&path)?;
    let mut capture = StoredCapture {
        id: new_id("analysis"),
        created_ms: now_ms(),
        ticket: ticket.clone(),
        camera_record,
        raw_path: relative,
        raw_sha256: hash,
        measurements: None,
        display_issue: display_issue.clone(),
        analysis_issue: display_issue,
        artifacts: vec![],
        geometry_verified: None,
    };
    let result = (|| -> Result<()> {
        ensure!(
            capture.analysis_issue.is_none(),
            "{}",
            capture.analysis_issue.as_deref().unwrap_or_default()
        );
        let raw = RawFrame::decode(&repo.root.join(&capture.raw_path))?;
        ticket.conditions.camera_key = raw.metadata.camera_key();
        let folder = repo.root.join("results").join(&capture.id);
        capture.ticket = ticket.clone();
        let profile = ticket
            .geometry
            .as_ref()
            .context("RAW retained. Calibrate geometry before color analysis.")?;
        let corrected =
            geometry::rectify_for_color(&raw, profile, &ticket.conditions.manifest, &folder)?;
        capture.artifacts = [
            "normalized.exr",
            "preview.png",
            "valid-mask.png",
            "sensor-preview.png",
            "geometry-overlay.png",
        ]
        .into_iter()
        .map(|name| folder.join(name))
        .collect();
        let normalized_path = folder.join("normalized.exr");
        use sha2::{Digest, Sha256};
        capture.geometry_verified = Some(GeometryEvidence {
            profile_id: profile.id.clone(),
            pattern_generation: ticket.conditions.manifest.generation.clone(),
            raw_sha256: capture.raw_sha256.clone(),
            max_residual_px: corrected.max_residual_px,
            normalized_sha256: format!("{:x}", Sha256::digest(std::fs::read(&normalized_path)?)),
            normalized_path,
        });
        capture.measurements = Some(measurement::measure(&corrected)?);
        Ok(())
    })();
    if let Err(error) = result {
        capture.analysis_issue = Some(format!("{error:#}"));
        capture.measurements = None;
        capture.geometry_verified = None;
    }
    repo.insert_capture(&capture)?;
    Ok(capture)
}
fn finish_capture(
    repo: &Repository,
    capture: StoredCapture,
    events: &mpsc::Sender<Event>,
) -> Result<()> {
    let mut message =
        capture
            .analysis_issue
            .clone()
            .unwrap_or_else(|| match capture.ticket.workflow {
                Workflow::Geometry => "Geometry applied to this App session".into(),
                Workflow::Reference => {
                    "Reference RAW saved. Select captures and publish a baseline.".into()
                }
                Workflow::Inspection => {
                    "Test RAW saved; choose an iPhone baseline to compare".into()
                }
            });
    if capture.ticket.workflow == Workflow::Inspection && capture.analysis_issue.is_none() {
        if let Some(id) = &capture.ticket.reference {
            let reference = repo.reference(id)?;
            match measurement::inspect(&capture, &reference, capture.ticket.thresholds.clone()) {
                Ok(report) => {
                    message = report.conclusion.clone();
                    repo.insert_report(&report)?;
                }
                Err(e) => message = format!("RAW saved; report unavailable: {e:#}"),
            }
        }
    }
    let _ = events.send(Event::Finished(
        message,
        capture
            .artifacts
            .iter()
            .find(|p| p.ends_with("preview.png"))
            .cloned(),
    ));
    Ok(())
}

impl Drop for Controller {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_analysis_waits_for_photo_format_restoration() {
        let mut controller = Controller::preview();
        let (tx, commands) = mpsc::channel();
        controller.tx = tx;
        let mut ticket = crate::screen_test::tests::capture().ticket;
        ticket.workflow = Workflow::Geometry;
        controller.busy = true;
        controller.pending = Some((ticket.clone(), Instant::now()));
        controller
            .downloaded
            .push_back((ticket.clone(), "geometry.CR2".into(), "camera".into()));
        controller.set_camera_ready(false);
        controller.poll();
        assert!(commands.try_recv().is_err());
        assert!(controller.busy);
        controller.set_camera_ready(true);
        controller.poll();
        assert!(
            matches!(commands.try_recv().unwrap(), Command::Analyze(t, _, _) if t.id == ticket.id)
        );
        assert!(commands.try_recv().is_err());
    }
    #[test]
    fn geometry_capture_failure_restores_display_before_finishing_and_drops_late_raw() {
        let mut controller = Controller::preview();
        let (tx, commands) = mpsc::channel();
        controller.tx = tx;
        let mut ticket = crate::screen_test::tests::capture().ticket;
        ticket.workflow = Workflow::Geometry;
        controller
            .recoverable
            .insert(ticket.id.clone(), ticket.clone());
        controller.pending = Some((ticket.clone(), Instant::now()));
        controller
            .downloaded
            .push_back((ticket.clone(), "geometry.CR2".into(), "camera".into()));
        controller.busy = true;
        controller.camera_error("Capture failed".into());
        assert!(controller.busy);
        assert!(controller.pending.is_none());
        assert!(controller.recoverable.is_empty());
        assert!(controller.downloaded.is_empty());
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::AbortGeometry(_)
        ));
        assert!(commands.try_recv().is_err());
    }
    #[test]
    fn recovered_download_waits_for_current_capture_without_repeating_shutter() {
        let mut controller = Controller::preview();
        let (tx, commands) = mpsc::channel();
        controller.tx = tx;
        let late = crate::screen_test::tests::capture().ticket;
        let active = crate::screen_test::tests::capture().ticket;
        controller.pending = Some((active.clone(), Instant::now()));
        controller.busy = true;
        controller
            .downloaded
            .push_back((late.clone(), "old.CR2".into(), "old".into()));
        controller.start_downloaded();
        assert!(commands.try_recv().is_err());
        controller
            .downloaded
            .push_back((active.clone(), "new.CR2".into(), "new".into()));
        controller.start_downloaded();
        assert!(
            matches!(commands.try_recv().unwrap(), Command::Analyze(t,_,_) if t.id == active.id)
        );
        controller.busy = false;
        controller.start_downloaded();
        assert!(matches!(commands.try_recv().unwrap(), Command::Analyze(t,_,_) if t.id == late.id));
        assert!(commands.try_recv().is_err());
    }
}

#[cfg(test)]
mod geometry_session_tests {
    use super::*;
    #[test]
    fn geometry_controller_starts_without_a_database_and_cleans_temporary_files() {
        let root = std::env::temp_dir().join(new_id("unused-geometry-database"));
        let shared = Arc::new(Mutex::new(SessionGeometryStore::default()));
        let controller = Controller::at(root.clone(), shared);
        assert!(!controller.busy);
        drop(controller);
        assert!(!root.exists());
        let folder;
        {
            let mut files = SessionFiles::default();
            folder = files.root().unwrap();
            std::fs::write(folder.join("temporary.CR2"), b"temporary capture").unwrap();
            assert!(!folder.join("screen-tests.sqlite3").exists());
        }
        assert!(!folder.exists());
    }
}
