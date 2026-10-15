//! One USB session, 33 explicit conditions, with the same RAW/geometry gates as the UI.
use super::{
    controller, device::DeviceConnection, geometry, measurement, model::*, pattern::Pattern,
    raw::RawFrame, repository::Repository,
};
use crate::camera_studio::{model::*, service::CameraService};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CellState {
    Pending,
    Running,
    Saved,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SweepCell {
    pub mode: DisplayMode,
    pub brightness: f32,
    pub state: CellState,
    pub captures: Vec<String>,
    pub reference: Option<String>,
    pub issue: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SweepRun {
    pub id: String,
    pub created_ms: u64,
    pub finished_ms: Option<u64>,
    pub device: Option<DisplayDevice>,
    pub hdr_peak: f32,
    #[serde(default)]
    pub fixed_exposure: Option<BTreeMap<String, ParameterValue>>,
    pub cells: Vec<SweepCell>,
    pub issue: Option<String>,
}
impl SweepRun {
    pub fn planned() -> Self {
        Self {
            id: new_id("sweep"),
            created_ms: now_ms(),
            finished_ms: None,
            device: None,
            hdr_peak: 2.,
            fixed_exposure: None,
            issue: None,
            cells: [
                DisplayMode::Srgb,
                DisplayMode::DisplayP3,
                DisplayMode::LinearHdr,
            ]
            .into_iter()
            .flat_map(|mode| {
                (0..=10).map(move |i| SweepCell {
                    mode,
                    brightness: i as f32 / 10.,
                    state: CellState::Pending,
                    captures: vec![],
                    reference: None,
                    issue: None,
                })
            })
            .collect(),
        }
    }
    pub fn saved(&self) -> usize {
        self.cells
            .iter()
            .filter(|c| c.state == CellState::Saved)
            .count()
    }
}

/// Executes physical shutter operations. Failures and partial results stay queryable in SQLite.
pub fn run(root: &Path, iphone_id: &str) -> Result<SweepRun> {
    let mut repo = Repository::open(root)?;
    let mut run = SweepRun::planned();
    repo.save_sweep(&run)?;
    println!(
        "Batch {}: sRGB / P3 / HDR-linear, 0.0–1.0 step 0.1 (33 conditions)",
        run.id
    );
    let result = acquire(&mut repo, &mut run, iphone_id);
    if let Err(error) = result {
        run.issue = Some(format!("{error:#}"));
        for cell in run
            .cells
            .iter_mut()
            .filter(|c| c.state == CellState::Running)
        {
            cell.state = CellState::Failed;
            cell.issue.get_or_insert_with(|| format!("{error:#}"));
        }
    }
    run.finished_ms = Some(now_ms());
    repo.save_sweep(&run)?;
    println!(
        "Saved {}/33 reference conditions; database {}",
        run.saved(),
        repo.root.join("screen-tests.sqlite3").display()
    );
    Ok(run)
}
fn acquire(repo: &mut Repository, run: &mut SweepRun, iphone_id: &str) -> Result<()> {
    let capture_dir = repo.root.join("acquisitions").join(&run.id);
    // Open the camera first: an absent camera must not leave the phone in a test chart.
    let mut camera = CameraSession::connect(&capture_dir)?;
    run.fixed_exposure = Some(camera.fixed_exposure.clone());
    repo.save_sweep(run)?;
    println!(
        "Fixed camera settings for the entire sweep: {}",
        serde_json::to_string(&camera.fixed_exposure)?
    );
    let result = (|| -> Result<()> {
        let info = camera
            .status
            .info
            .as_ref()
            .context("Camera not connected")?;
        ensure!(
            info.model.contains("650D") || info.model.contains("T4i"),
            "This sweep requires the Canon 650D/T4i"
        );
        let exposure_mode = info.parameters.iter().find(|p| p.id == "autoexposuremode");
        ensure!(
            exposure_mode.is_some_and(
                |p| matches!(&p.value, ParameterValue::Choice(v) if v == "Manual" || v == "M")
            ),
            "Set the Canon mode dial to M; fixed aperture and ISO are required for comparable references"
        );
        ensure!(info.parameters.iter().find(|p| p.id == "iso").is_some_and(|p|
            matches!(&p.value, ParameterValue::Choice(v) if v.parse::<u32>().is_ok_and(|v| v > 0))),
            "Use a fixed ISO, not Auto ISO, for this reference sweep");
        let mut phone = DeviceConnection::connect_updated_ios(iphone_id)?;
        run.device = Some(phone.device.clone());
        repo.save_sweep(run)?;
        let result = acquire_conditions(repo, run, &mut camera, &mut phone, &capture_dir);
        let release = phone.restore_after_calibration();
        combine_cleanup(result, release, "Phone brightness restoration")
    })();
    let restore = camera.restore_and_close();
    combine_cleanup(result, restore, "Camera setting restoration")
}
fn combine_cleanup(result: Result<()>, cleanup: Result<()>, label: &str) -> Result<()> {
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(e), Ok(())) => Err(e),
        (Ok(()), Err(e)) => Err(e).with_context(|| label.to_string()),
        (Err(e), Err(c)) => anyhow::bail!("{e:#}; {label}: {c:#}"),
    }
}
fn ticket(
    pattern: &Pattern,
    actual: DisplayDevice,
    brightness: f32,
    run: &SweepRun,
    profile: Option<GeometryProfile>,
) -> CaptureTicket {
    CaptureTicket { id: new_id("sweep-capture"), workflow: if profile.is_some() { Workflow::Reference } else { Workflow::Geometry },
        conditions: CaptureConditions { device: actual, brightness, batch: run.id.clone(), notes: "One full reference sweep; original CFA measurements after verified geometry correction".into(), manifest: pattern.manifest.clone(), camera_key: String::new() },
        geometry: profile, reference: None, thresholds: None }
}
fn acquire_conditions(
    repo: &mut Repository,
    run: &mut SweepRun,
    camera: &mut CameraSession,
    phone: &mut DeviceConnection,
    directory: &Path,
) -> Result<()> {
    let mut temporary = TemporaryGeometry(std::env::temp_dir().join(new_id("sweep-geometry")), false);
    std::fs::create_dir_all(&temporary.0)?;
    println!("Geometry: presenting chart, capturing RAW, fitting and validating current placement");
    let chart = Pattern::generate(&phone.status()?, DisplayMode::Srgb, 1.)?;
    let actual = phone.present(&chart, 1.0)?;
    let geometry_ticket = ticket(&chart, actual, 1.0, run, None);
    let record = camera.capture(&temporary.0, &geometry_ticket)?;
    phone.validate_capture(&geometry_ticket)?;
    let raw = RawFrame::decode(&raw_path(&record)?)?;
    let markers = geometry::detect(&raw, &temporary.0)?;
    let profile = geometry::fit(&raw, &chart.manifest, &markers, &geometry_ticket.id).map_err(|error| {
        temporary.1 = true;
        let _ = chart.save(&temporary.0.join("pattern"));
        anyhow::anyhow!("{error:#}. Temporary diagnostic RAW: {}", temporary.0.display())
    })?;
    // The mapping is session-only. Each color capture stores its own validation evidence.
    let profile = session_geometry()
        .lock()
        .map_err(|_| anyhow::anyhow!("Session geometry lock failed"))?
        .publish(profile)?;
    let camera_key = raw.metadata.camera_key();
    let fixed_shutter = raw.metadata.exposure;
    println!(
        "Geometry confirmed: {:.3} px holdout; every subsequent RAW is revalidated",
        profile.holdout_rms
    );
    drop(raw);
    for i in 0..run.cells.len() {
        let mode = run.cells[i].mode;
        let brightness = run.cells[i].brightness;
        run.cells[i].state = CellState::Running;
        repo.save_sweep(run)?;
        println!(
            "[{}/33] {} brightness {:.1}",
            i + 1,
            mode.label(),
            brightness
        );
        let chart = Pattern::generate(&phone.status()?, mode, run.hdr_peak)?;
        chart.save(&repo.root.join("patterns").join(&chart.manifest.generation))?;
        // An unsupported/unstable display condition is recorded without triggering the shutter.
        let actual = match phone.present(&chart, brightness) {
            Ok(actual) => actual,
            Err(error) => {
                let detail = phone
                    .status()
                    .ok()
                    .map(|s| {
                        format!(
                            "actual brightness {:.3}, EDR headroom {:.3}",
                            s.brightness, s.headroom
                        )
                    })
                    .unwrap_or_default();
                run.cells[i].state = CellState::Failed;
                run.cells[i].issue = Some(format!("{error:#}; {detail}"));
                repo.save_sweep(run)?;
                println!("  Not captured: {}", run.cells[i].issue.as_deref().unwrap());
                continue;
            }
        };
        let capture_ticket = ticket(&chart, actual, brightness, run, Some(profile.clone()));
        phone.validate_capture(&capture_ticket)?;
        println!("  One RAW at fixed exposure; no automatic adjustment or bracketing");
        let record = camera.capture(directory, &capture_ticket)?;
        let display_issue = phone
            .validate_capture(&capture_ticket)
            .err()
            .map(|e| format!("{e:#}"));
        let capture = controller::analyze(
            repo,
            capture_ticket,
            raw_path(&record)?,
            record.id,
            display_issue.clone(),
        )?;
        run.cells[i].captures.push(capture.id.clone());
        repo.save_sweep(run)?;
        ensure!(
            display_issue.is_none(),
            "{}",
            display_issue.as_deref().unwrap_or_default()
        );
        if let Some(issue) = &capture.analysis_issue {
            run.cells[i].issue = Some(issue.clone());
        } else {
            capture.ensure_geometry_verified()?;
            ensure!(
                capture
                    .measurements
                    .as_ref()
                    .is_some_and(|m| m.raw.camera_key() == camera_key
                        && (m.raw.exposure - fixed_shutter).abs() < 1e-8),
                "Camera exposure or lens changed during the batch; reference not published"
            );
            let valid = capture
                .measurements
                .as_ref()
                .unwrap()
                .patches
                .iter()
                .filter(|p| p.valid)
                .count();
            println!("  Geometry verified; {valid}/60 valid patches");
            let name = format!(
                "{} · {} · brightness {:.1}",
                run.id,
                mode.label(),
                brightness
            );
            match measurement::reference(name, &[capture]) {
                Ok(reference) => {
                    repo.insert_reference(&reference)?;
                    run.cells[i].reference = Some(reference.id);
                    run.cells[i].state = CellState::Saved;
                    run.cells[i].issue = None;
                    println!("  Saved reference (fixed exposure)");
                }
                Err(error) => {
                    run.cells[i].issue = Some(format!(
                        "Quality check failed at fixed exposure: {error:#}. RAW retained; exposure was not changed."
                    ))
                }
            }
        }
        if run.cells[i].state != CellState::Saved {
            run.cells[i].state = CellState::Failed;
            println!(
                "  Not published: {}",
                run.cells[i]
                    .issue
                    .as_deref()
                    .unwrap_or("No valid reference")
            );
        }
        repo.save_sweep(run)?;
    }
    Ok(())
}
fn raw_path(record: &CaptureRecord) -> Result<PathBuf> {
    record
        .files
        .iter()
        .find(|f| {
            f.path
                .extension()
                .is_some_and(|v| v.eq_ignore_ascii_case("cr2"))
        })
        .map(|f| f.path.clone())
        .context("Capture ended without a CR2; shutter was not repeated")
}
fn shutter_seconds(value: &str) -> Option<f64> {
    let value = value.trim().trim_end_matches('"');
    let seconds = if let Some((a, b)) = value.split_once('/') {
        a.parse::<f64>().ok()? / b.parse::<f64>().ok()?
    } else {
        value.parse().ok()?
    };
    (seconds.is_finite() && seconds > 0.).then_some(seconds)
}
struct TemporaryGeometry(PathBuf, bool);
impl Drop for TemporaryGeometry {
    fn drop(&mut self) {
        if !self.1 { let _ = std::fs::remove_dir_all(&self.0); }
    }
}
struct CameraSession {
    service: CameraService,
    status: CameraStatus,
    fixed_exposure: BTreeMap<String, ParameterValue>,
}
impl CameraSession {
    fn connect(directory: &Path) -> Result<Self> {
        let mut result = Self {
            service: CameraService::spawn()?,
            status: CameraStatus::default(),
            fixed_exposure: BTreeMap::new(),
        };
        result.command(CameraCommand::Connect {
            directory: directory.to_path_buf(),
            release_occupied: true,
        })?;
        ensure!(
            !result.status.recording && result.status.pending_downloads == 0,
            "Camera has active recording/downloads"
        );
        let info = result
            .status
            .info
            .as_ref()
            .context("Camera not connected")?;
        for id in ["shutterspeed", "aperture", "iso", "autoexposuremode"] {
            let value = info
                .parameters
                .iter()
                .find(|p| p.id == id)
                .with_context(|| format!("Camera setting unavailable: {id}"))?
                .value
                .clone();
            result.fixed_exposure.insert(id.into(), value);
        }
        ensure!(
            matches!(result.fixed_exposure.get("shutterspeed"), Some(ParameterValue::Choice(s)) if shutter_seconds(s).is_some()),
            "Choose a finite manual shutter time before the sweep"
        );
        result.command(CameraCommand::Preview(false))?;
        Ok(result)
    }
    fn command(&mut self, command: CameraCommand) -> Result<()> {
        self.service.events.try_iter().for_each(drop);
        let expected_operation = command.operation();
        let disconnect = matches!(command, CameraCommand::Disconnect);
        let mut started = false;
        self.service.send(command)?;
        let until = Instant::now() + Duration::from_secs(120);
        loop {
            let event = self
                .service
                .events
                .recv_timeout(until.saturating_duration_since(Instant::now()))
                .context("Camera command timed out; no automatic shutter retry")?;
            match event {
                CameraEvent::Status(status) => {
                    started |= status.operation == expected_operation;
                    self.status = status;
                    if (started || (disconnect && self.status.info.is_none()))
                        && self.status.operation == CameraOperation::Idle
                        && !self.status.photo_format_restore_pending
                    {
                        return Ok(());
                    }
                }
                CameraEvent::Error { error, .. } => return Err(error.into()),
                CameraEvent::Stopped => anyhow::bail!("Camera session ended"),
                _ => {}
            }
        }
    }
    fn capture(&mut self, directory: &Path, ticket: &CaptureTicket) -> Result<CaptureRecord> {
        self.command(CameraCommand::Refresh)?;
        validate_fixed_exposure(
            self.status.info.as_ref().context("Camera disconnected")?,
            &self.fixed_exposure,
        )?;

        self.service.events.try_iter().for_each(drop);
        self.service.send(CameraCommand::CaptureRaw {
            directory: directory.to_path_buf(),
            context: BTreeMap::from([(
                "screen_test_ticket".into(),
                serde_json::to_string(ticket)?,
            )]),
        })?;
        let until = Instant::now() + Duration::from_secs(150);
        let mut record = None;
        loop {
            let event = self
                .service
                .events
                .recv_timeout(until.saturating_duration_since(Instant::now()))
                .context("RAW capture timed out; no automatic shutter retry")?;
            match event {
                CameraEvent::Record(value) => {
                    if value
                        .context
                        .get("screen_test_ticket")
                        .and_then(|s| serde_json::from_str::<CaptureTicket>(s).ok())
                        .is_some_and(|t| t.id == ticket.id)
                    {
                        record = Some(value);
                    }
                }
                CameraEvent::Status(value) => {
                    self.status = value;
                    if capture_finished(&self.status, record.as_ref()) {
                        return Ok(record.unwrap());
                    }
                }
                CameraEvent::Error { error, .. } => return Err(error.into()),
                CameraEvent::Stopped => anyhow::bail!("Camera session ended during capture"),
                _ => {}
            }
        }
    }
    fn restore_and_close(&mut self) -> Result<()> {
        // Wait for any outstanding RAW transaction, even after a reported shutter/transfer error.
        // A failed download may remain recoverable; it must not prevent restoring settings.
        self.command(CameraCommand::Refresh)?;
        validate_fixed_exposure(
            self.status.info.as_ref().context("Camera disconnected")?,
            &self.fixed_exposure,
        )?;
        self.command(CameraCommand::Disconnect)
    }
}
fn validate_fixed_exposure(
    info: &CameraInfo,
    expected: &BTreeMap<String, ParameterValue>,
) -> Result<()> {
    for (id, value) in expected {
        ensure!(
            info.parameters
                .iter()
                .find(|p| &p.id == id)
                .is_some_and(|p| &p.value == value),
            "Camera setting {id} changed; fixed-exposure sweep stopped before another shutter"
        );
    }
    Ok(())
}
fn capture_finished(status: &CameraStatus, record: Option<&CaptureRecord>) -> bool {
    status.operation == CameraOperation::Idle
        && !status.photo_format_restore_pending
        && status.pending_downloads == 0
        && record.is_some_and(|r| !r.files.is_empty() && r.pending_files.is_empty())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changing_any_locked_exposure_parameter_rejects_the_next_capture() {
        let parameters: Vec<_> = [("shutterspeed", "1/200"), ("aperture", "5"), ("iso", "400"), ("autoexposuremode", "Manual")]
            .into_iter().map(|(id, value)| CameraParameter {
                id: id.into(), label: id.into(), value: ParameterValue::Choice(value.into()),
                kind: ParameterKind::Choice(vec![value.into()]), read_only: false,
            }).collect();
        let expected = parameters.iter().map(|p| (p.id.clone(), p.value.clone())).collect();
        let info = CameraInfo { parameters, ..Default::default() };
        validate_fixed_exposure(&info, &expected).unwrap();
        for i in 0..info.parameters.len() {
            let mut changed = info.clone();
            changed.parameters[i].value = ParameterValue::Choice("changed".into());
            assert!(validate_fixed_exposure(&changed, &expected).is_err());
        }
    }
    #[test]
    fn sweep_is_exactly_three_modes_and_eleven_inclusive_steps() {
        let run = SweepRun::planned();
        assert_eq!(run.cells.len(), 33);
        for (index, mode) in [
            DisplayMode::Srgb,
            DisplayMode::DisplayP3,
            DisplayMode::LinearHdr,
        ]
        .into_iter()
        .enumerate()
        {
            for step in 0..=10 {
                let cell = &run.cells[index * 11 + step];
                assert_eq!(cell.mode, mode);
                assert_eq!(cell.brightness, step as f32 / 10.);
                assert_eq!(cell.state, CellState::Pending);
            }
        }
        assert_eq!(run.saved(), 0);
    }
    #[test]
    fn finite_shutters_exclude_bulb_and_invalid_values() {
        assert_eq!(shutter_seconds("1/200"), Some(0.005));
        assert_eq!(shutter_seconds("0.3"), Some(0.3));
        assert_eq!(shutter_seconds("30\""), Some(30.));
        for s in ["bulb", "0", "-1", "NaN", "1/0"] {
            assert!(shutter_seconds(s).is_none());
        }
    }
    #[test]
    fn idle_is_not_capture_completion_without_files_and_restoration() {
        let mut status = CameraStatus::default();
        assert!(!capture_finished(&status, None));
        let mut record = CaptureRecord {
            id: "test".into(),
            kind: CaptureKind::Photo,
            started_unix_ms: 0,
            camera: CameraInfo::default(),
            context: BTreeMap::new(),
            files: vec![],
            pending_files: vec![],
            directory: PathBuf::new(),
        };
        assert!(!capture_finished(&status, Some(&record)));
        record.files.push(CapturedFile {
            remote: RemoteFile {
                folder: "/".into(),
                name: "photo.CR2".into(),
            },
            path: "photo.CR2".into(),
            bytes: 1,
            sha256: "hash".into(),
        });
        status.photo_format_restore_pending = true;
        assert!(!capture_finished(&status, Some(&record)));
        status.photo_format_restore_pending = false;
        assert!(capture_finished(&status, Some(&record)));
    }
}
