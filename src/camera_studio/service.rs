use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use sha2::{Digest, Sha256};

use super::{gphoto::GPhoto, model::*};

/// Control/status events are reliable. Only the live preview mailbox may overwrite old data.
pub struct CameraService {
    commands: Sender<CameraCommand>,
    pub events: Receiver<CameraEvent>,
    latest: Arc<Mutex<Option<PreviewFrame>>>,
}

impl CameraService {
    pub fn spawn() -> CameraResult<Self> {
        Self::with_backend(|| Ok(Box::new(GPhoto::load()?) as Box<dyn CameraBackend>))
    }

    pub fn with_backend(
        factory: impl FnOnce() -> CameraResult<Box<dyn CameraBackend>> + Send + 'static,
    ) -> CameraResult<Self> {
        let (tx, commands) = bounded(32);
        let (events, rx) = unbounded();
        let latest = Arc::new(Mutex::new(None));
        let mailbox = latest.clone();
        thread::Builder::new()
            .name("camera-studio".into())
            .spawn(move || {
                match factory() {
                    Ok(backend) => Worker::new(backend, commands, events.clone(), mailbox).run(),
                    Err(error) => {
                        let _ = events.send(CameraEvent::Error {
                            error,
                            operation: CameraOperation::Connecting,
                            record_id: None,
                        });
                    }
                }
                let _ = events.send(CameraEvent::Stopped);
            })
            .map_err(|e| {
                CameraError::new(
                    CameraErrorKind::Unavailable,
                    format!("Start camera worker: {e}"),
                )
            })?;
        Ok(Self {
            commands: tx,
            events: rx,
            latest,
        })
    }

    pub fn send(&self, command: CameraCommand) -> CameraResult<()> {
        self.commands.try_send(command).map_err(|e| {
            CameraError::new(
                CameraErrorKind::Busy,
                format!("Camera command was not queued: {e}"),
            )
        })
    }

    pub fn take_preview(&self) -> Option<PreviewFrame> {
        self.latest.lock().ok()?.take()
    }
}

impl Drop for CameraService {
    fn drop(&mut self) {
        // Never join a USB call on the UI thread. Channel disconnection also requests cleanup.
        let _ = self.commands.try_send(CameraCommand::Shutdown);
    }
}

struct PendingDownload {
    file: RemoteFile,
    record: usize,
    failed: bool,
}
struct Settling {
    record: usize,
    until: Instant,
}
struct PhotoFormatRestore {
    original: ParameterValue,
    record: Option<usize>,
    last_attempt: Option<Instant>,
}

struct Worker {
    backend: Box<dyn CameraBackend>,
    commands: Receiver<CameraCommand>,
    events: Sender<CameraEvent>,
    latest: Arc<Mutex<Option<PreviewFrame>>>,
    status: CameraStatus,
    known: HashSet<RemoteFile>,
    groups: HashMap<String, usize>,
    pending: VecDeque<PendingDownload>,
    records: Vec<CaptureRecord>,
    active_movie: Option<usize>,
    movie_start_pending: bool,
    stop_requested: bool,
    settling: Option<Settling>,
    photo_format_restore: Option<PhotoFormatRestore>,
    default_directory: PathBuf,
    last_refresh: Instant,
    last_status: Instant,
    last_error: Option<(String, Instant)>,
    preview_failures: u32,
    event_failures: u32,
    fps_start: Instant,
    fps_frames: u64,
    camera_identity: Option<String>,
}

impl Worker {
    fn new(
        backend: Box<dyn CameraBackend>,
        commands: Receiver<CameraCommand>,
        events: Sender<CameraEvent>,
        latest: Arc<Mutex<Option<PreviewFrame>>>,
    ) -> Self {
        Self {
            backend,
            commands,
            events,
            latest,
            status: CameraStatus::default(),
            known: HashSet::new(),
            groups: HashMap::new(),
            pending: VecDeque::new(),
            records: Vec::new(),
            active_movie: None,
            movie_start_pending: false,
            stop_requested: false,
            settling: None,
            photo_format_restore: None,
            default_directory: default_directory(),
            last_refresh: Instant::now(),
            last_status: Instant::now(),
            last_error: None,
            preview_failures: 0,
            event_failures: 0,
            fps_start: Instant::now(),
            fps_frames: 0,
            camera_identity: None,
        }
    }

    fn emit(&self, event: CameraEvent) {
        let _ = self.events.send(event);
    }
    fn snapshot(&mut self) {
        self.status.pending_downloads = self.pending.len();
        self.status.photo_format_restore_pending = self.photo_format_restore.is_some();
        self.emit(CameraEvent::Status(self.status.clone()));
        self.last_status = Instant::now();
    }
    fn error(&mut self, error: CameraError) {
        if self.last_error.as_ref().is_none_or(|(text, when)| {
            *text != error.message || when.elapsed() > Duration::from_secs(5)
        }) {
            self.last_error = Some((error.message.clone(), Instant::now()));
            self.emit(CameraEvent::Error {
                error,
                operation: self.status.operation.clone(),
                record_id: self.status.active_record_id.clone(),
            });
        }
    }
    fn resting_operation(&mut self) {
        let (operation, record) = if self.status.info.is_none() {
            (CameraOperation::Idle, None)
        } else if self.status.recording {
            (
                if self.stop_requested {
                    CameraOperation::StoppingRecording
                } else {
                    CameraOperation::Recording
                },
                self.active_movie,
            )
        } else if self.movie_start_pending {
            (CameraOperation::StartingRecording, self.active_movie)
        } else if let Some(settling) = &self.settling {
            (CameraOperation::WaitingFiles, Some(settling.record))
        } else if let Some(restore) = &self.photo_format_restore {
            (
                CameraOperation::SettingParameter("imageformat".into()),
                restore.record,
            )
        } else {
            (CameraOperation::Idle, None)
        };
        self.status.operation = operation;
        self.status.active_record_id = record.map(|index| self.records[index].id.clone());
    }

    fn busy(&mut self, label: &str) {
        self.status.busy = Some(label.into());
        self.snapshot();
    }

    fn run(mut self) {
        loop {
            let timeout = if self.status.info.is_some() {
                Duration::ZERO
            } else {
                Duration::from_millis(100)
            };
            match self.commands.recv_timeout(timeout) {
                Ok(CameraCommand::Shutdown)
                | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                Ok(command) => {
                    if let Err(e) = self.command(command) {
                        self.error(e);
                    }
                    self.status.busy = self
                        .settling
                        .as_ref()
                        .map(|_| "Waiting for camera files".into());
                    self.resting_operation();
                    self.snapshot();
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            }
            if self.status.info.is_none() {
                continue;
            }
            // Drain bursts (RAW+JPEG and property changes) without starving controls or preview.
            for _ in 0..8 {
                match self
                    .backend
                    .poll_event(if self.status.preview { 1 } else { 30 })
                {
                    Ok(BackendEvent::Timeout) => {
                        self.event_failures = 0;
                        break;
                    }
                    Ok(BackendEvent::FileAdded(file)) => {
                        self.event_failures = 0;
                        self.queue(file, None);
                    }
                    Ok(BackendEvent::CaptureComplete | BackendEvent::Changed) => {
                        self.event_failures = 0;
                    }
                    Err(e) => {
                        self.event_failures += 1;
                        self.error(e);
                        if self.event_failures >= 3 {
                            self.lost_connection();
                        }
                        thread::sleep(Duration::from_millis(150));
                        break;
                    }
                }
            }
            if self.status.info.is_none() {
                continue;
            }
            if self.last_refresh.elapsed() > Duration::from_secs(2) {
                if let Err(e) = self.refresh() {
                    self.error(e);
                }
                self.resting_operation();
                self.snapshot();
                self.last_refresh = Instant::now();
            }
            if self
                .settling
                .as_ref()
                .is_some_and(|s| Instant::now() >= s.until)
            {
                let record = self.settling.as_ref().map(|s| s.record);
                if let Err(e) = self.reconcile(record) {
                    self.error(e);
                }
                self.settling = None;
                self.status.busy = None;
                self.resting_operation();
                self.snapshot();
            }
            if !self.status.recording {
                self.download_next();
            }
            self.finish_raw_capture();
            if self.status.preview {
                match self.backend.preview_frame().and_then(decode_preview) {
                    Ok((size, rgba)) => {
                        self.preview_failures = 0;
                        self.status.frame_count += 1;
                        self.fps_frames += 1;
                        self.status.preview_size = Some(size);
                        if let Ok(mut slot) = self.latest.lock() {
                            *slot = Some(PreviewFrame {
                                sequence: self.status.frame_count,
                                received: Instant::now(),
                                size,
                                rgba,
                            });
                        }
                        let seconds = self.fps_start.elapsed().as_secs_f32();
                        if seconds >= 1.0 {
                            self.status.preview_fps = self.fps_frames as f32 / seconds;
                            self.fps_frames = 0;
                            self.fps_start = Instant::now();
                        }
                    }
                    Err(e) => {
                        self.preview_failures += 1;
                        if self.preview_failures > 5 {
                            self.status.preview = false;
                            self.status.preview_fps = 0.0;
                            self.emit(CameraEvent::Error {
                                error: e,
                                operation: CameraOperation::StartingPreview,
                                record_id: None,
                            });
                            self.emit(CameraEvent::Notice("Live preview paused after repeated errors. Camera recording, if active, is still monitored.".into()));
                            self.snapshot();
                        } else {
                            thread::sleep(Duration::from_millis(80));
                        }
                    }
                }
            }
            if self.last_status.elapsed() >= Duration::from_secs(1) {
                self.snapshot();
            }
        }
        // Graceful shutdown after a command has completed; never interrupt a file mid-write.
        if self.status.recording {
            if let Err(e) = self.stop_recording() {
                self.error(e);
            }
        }
        if self.status.info.is_some() {
            if !self.status.recording {
                if self.settling.is_some() {
                    let deadline = Instant::now() + Duration::from_secs(3);
                    while Instant::now() < deadline {
                        match self.backend.poll_event(100) {
                            Ok(BackendEvent::FileAdded(file)) => self.queue(file, None),
                            Ok(_) => {}
                            Err(e) => {
                                self.error(e);
                                break;
                            }
                        }
                    }
                }
                let record = self.settling.take().map(|s| s.record).or(self.active_movie);
                if let Err(e) = self.reconcile(record) {
                    self.error(e);
                }
                while self.pending.iter().any(|p| !p.failed) {
                    self.download_next();
                }
                if let Err(error) = self.restore_photo_format() {
                    self.error(error);
                }
            }
            if self.status.preview {
                if let Err(e) = self.backend.preview(false) {
                    self.error(e);
                }
            }
            if let Err(e) = self.backend.disconnect() {
                self.error(e);
            }
        }
    }

    fn command(&mut self, command: CameraCommand) -> CameraResult<()> {
        self.status.operation = command.operation();
        self.status.active_record_id = if matches!(
            command,
            CameraCommand::StopRecording | CameraCommand::RecoverDownloads
        ) {
            self.active_movie
                .map(|index| self.records[index].id.clone())
        } else {
            None
        };
        if !matches!(command, CameraCommand::Connect { .. }) && self.status.info.is_none() {
            return Err(CameraError::new(
                CameraErrorKind::Disconnected,
                "Connect the camera first",
            ));
        }
        match command {
            CameraCommand::Connect {
                directory,
                release_occupied,
            } => {
                if self.status.info.is_some() {
                    return Ok(());
                }
                self.busy("Connecting");
                let events = self.events.clone();
                let info = super::access::connect_with_recovery(
                    &mut *self.backend,
                    release_occupied,
                    super::access::release_camera_clients,
                    |notice| {
                        let _ = events.send(CameraEvent::Notice(notice));
                    },
                )?;
                let identity = camera_identity(&info);
                if self
                    .camera_identity
                    .as_ref()
                    .is_some_and(|previous| *previous != identity)
                    && (!self.pending.is_empty() || self.photo_format_restore.is_some())
                {
                    let _ = self.backend.disconnect();
                    return Err(CameraError::new(
                        CameraErrorKind::Invalid,
                        "Reconnect the original camera to restore its photo format and recover pending downloads",
                    ));
                }
                if self.camera_identity.as_ref() != Some(&identity) {
                    match self.backend.list_files() {
                        Ok(files) => self.known = files.into_iter().collect(),
                        Err(e) => {
                            let _ = self.backend.disconnect();
                            return Err(e);
                        }
                    }
                    self.groups.clear();
                    self.active_movie = None;
                    self.movie_start_pending = false;
                }
                self.camera_identity = Some(identity);
                self.status.info = Some(info);
                self.default_directory = directory.clone();
                for record in load_records(&directory).map_err(storage_error)? {
                    if self.records.iter().any(|r| r.id == record.id) {
                        continue;
                    }
                    let index = self.records.len();
                    if Some(camera_identity(&record.camera)) == self.camera_identity {
                        for file in &record.pending_files {
                            self.known.insert(file.clone());
                            self.groups.insert(file.group_key(), index);
                            self.pending.push_back(PendingDownload {
                                file: file.clone(),
                                record: index,
                                failed: true,
                            });
                        }
                    }
                    self.emit(CameraEvent::Record(record.clone()));
                    self.records.push(record);
                }
                self.event_failures = 0;
                self.refresh()?;
                self.restore_photo_format()?;
                if !self.status.recording {
                    self.status.operation = CameraOperation::StartingPreview;
                    self.busy("Starting live view");
                    // Only an explicit successful connection starts preview. Refresh/review never do.
                    self.backend.preview(true)?;
                    self.status.preview = true;
                    self.preview_failures = 0;
                    self.fps_start = Instant::now();
                    self.fps_frames = 0;
                }
            }
            CameraCommand::Disconnect => {
                if self.status.recording
                    || self.movie_start_pending
                    || self.settling.is_some()
                    || !self.pending.is_empty()
                {
                    return Err(CameraError::new(
                        CameraErrorKind::Busy,
                        "Finish recording and recover pending downloads before disconnecting",
                    ));
                }
                self.restore_photo_format()?;
                if self.status.preview {
                    self.backend.preview(false)?;
                }
                self.backend.disconnect()?;
                self.status = CameraStatus::default();
                if let Ok(mut slot) = self.latest.lock() {
                    *slot = None;
                }
            }
            CameraCommand::Refresh => {
                self.busy("Refreshing settings");
                self.refresh()?;
                if self.settling.is_none() && !self.status.recording {
                    self.restore_photo_format()?;
                }
            }
            CameraCommand::Preview(enabled) => {
                if self.status.recording && !enabled {
                    return Err(CameraError::new(
                        CameraErrorKind::Busy,
                        "Keep the viewfinder enabled while recording",
                    ));
                }
                self.busy(if enabled {
                    "Starting live view"
                } else {
                    "Stopping live view"
                });
                self.backend.preview(enabled)?;
                self.status.preview = enabled;
                self.preview_failures = 0;
                self.fps_start = Instant::now();
                self.fps_frames = 0;
            }
            CameraCommand::SetParameter { id, value } => {
                if self.status.recording
                    || self.settling.is_some()
                    || self.photo_format_restore.is_some()
                {
                    return Err(CameraError::new(
                        CameraErrorKind::Busy,
                        "Wait until the capture has finished before changing settings",
                    ));
                }
                self.busy("Changing camera setting");
                self.backend.set_parameter(&id, &value)?;
                self.refresh()?;
            }
            CameraCommand::Focus { near, step } => {
                if !self.status.preview {
                    return Err(CameraError::new(
                        CameraErrorKind::Unsupported,
                        "Start Live View before using Near / Far.",
                    ));
                }
                if self.settling.is_some() || self.movie_start_pending || self.stop_requested {
                    return Err(CameraError::new(
                        CameraErrorKind::Busy,
                        "Wait for the current capture operation before adjusting focus.",
                    ));
                }
                self.busy("Preparing manual focus");
                // Read fresh settings: the lens switch or AF mode may have changed on the body.
                self.refresh()?;
                let changes = self
                    .status
                    .info
                    .as_ref()
                    .unwrap()
                    .focus_preparation(near, step)?;
                for (id, value) in &changes {
                    if let Err(error) = self.backend.set_parameter(id, value) {
                        let _ = self.refresh();
                        return Err(CameraError::new(
                            error.kind,
                            format!("Could not disable {id} before focus: {}", error.message),
                        ));
                    }
                }
                if !changes.is_empty() {
                    self.refresh()?;
                    if !self
                        .status
                        .info
                        .as_ref()
                        .unwrap()
                        .focus_preparation(near, step)?
                        .is_empty()
                    {
                        return Err(CameraError::new(
                            CameraErrorKind::Busy,
                            "The camera has not confirmed continuous AF is off. No focus step was sent; try again when it is ready.",
                        ));
                    }
                }
                self.busy("Driving focus");
                self.backend.focus(near, step)?;
                self.emit(CameraEvent::FocusAcknowledged { near, step });
            }
            CameraCommand::Capture { directory, context } => {
                self.capture_photo(directory, context, false)?
            }
            CameraCommand::CaptureRaw { directory, context } => {
                self.capture_photo(directory, context, true)?
            }
            CameraCommand::StartRecording { directory, context } => {
                self.refresh()?;
                self.ensure_idle_capture()?;
                if !self
                    .status
                    .info
                    .as_ref()
                    .is_some_and(CameraInfo::can_start_recording)
                {
                    return Err(CameraError::new(
                        CameraErrorKind::Unsupported,
                        "Recording to the card is unavailable in the current camera mode. Set the T4i's physical power switch to movie mode and insert an SD card.",
                    ));
                }
                self.busy("Starting camera recording");
                let record = self.create_record(directory, CaptureKind::Movie, context)?;
                // Keep this association if the start command times out but the camera starts.
                self.active_movie = Some(record);
                self.movie_start_pending = true;
                self.status.active_record_id = Some(self.records[record].id.clone());
                self.snapshot();
                self.backend.record(true)?;
                self.movie_start_pending = false;
                self.status.recording = true;
                self.status.recording_started = Some(Instant::now());
                self.status.preview = true;
            }
            CameraCommand::StopRecording => {
                self.busy("Stopping recording");
                self.stop_recording()?;
            }
            CameraCommand::RetryDownloads { record_id } => {
                if self.status.recording || self.settling.is_some() || self.movie_start_pending {
                    return Err(CameraError::new(
                        CameraErrorKind::Busy,
                        "Finish the current capture before retrying downloads",
                    ));
                }
                self.status.active_record_id = Some(record_id.clone());
                self.busy("Retrying capture downloads");
                for pending in &mut self.pending {
                    if self.records[pending.record].id == record_id {
                        pending.failed = false;
                    }
                }
            }
            CameraCommand::RecoverDownloads => {
                if self.status.recording {
                    return Err(CameraError::new(
                        CameraErrorKind::Busy,
                        "Stop recording before downloading",
                    ));
                }
                self.busy("Reconciling camera files");
                self.reconcile(self.active_movie)?;
                for pending in &mut self.pending {
                    pending.failed = false;
                }
            }
            CameraCommand::Shutdown => unreachable!("handled by event loop"),
        }
        Ok(())
    }

    fn ensure_idle_capture(&self) -> CameraResult<()> {
        if self.status.recording
            || self.movie_start_pending
            || self.settling.is_some()
            || !self.pending.is_empty()
            || self.photo_format_restore.is_some()
        {
            Err(CameraError::new(
                CameraErrorKind::Busy,
                "Wait for the current capture and its downloads",
            ))
        } else {
            Ok(())
        }
    }

    fn capture_photo(
        &mut self,
        directory: PathBuf,
        mut context: BTreeMap<String, String>,
        require_raw: bool,
    ) -> CameraResult<()> {
        self.ensure_idle_capture()?;
        let result = (|| {
            if require_raw {
                self.busy("Preparing RAW capture");
                self.refresh()?;
                self.ensure_idle_capture()?;
                let info = self.status.info.as_ref().unwrap();
                if let Some(raw) = info.raw_capture_format()? {
                    let original = info
                        .parameters
                        .iter()
                        .find(|p| p.id == "imageformat")
                        .unwrap()
                        .value
                        .clone();
                    context.insert(
                        "original_photo_format".into(),
                        serde_json::to_string(&original).map_err(storage_error)?,
                    );
                    // Install cleanup before writing: even a timed-out write may have applied.
                    self.photo_format_restore = Some(PhotoFormatRestore {
                        original,
                        record: None,
                        last_attempt: None,
                    });
                    self.backend.set_parameter("imageformat", &raw)?;
                    self.refresh()?;
                    if !self.status.info.as_ref().is_some_and(|info| {
                        info.parameters
                            .iter()
                            .any(|p| p.id == "imageformat" && p.value == raw)
                    }) {
                        return Err(CameraError::new(
                            CameraErrorKind::Busy,
                            "Camera did not confirm RAW. No shutter was sent.",
                        ));
                    }
                }
            }
            self.busy("Taking photograph");
            let default_directory = self.default_directory.clone();
            let record = self.create_record(directory, CaptureKind::Photo, context);
            // Temporary geometry captures must not redirect later camera-initiated downloads.
            if require_raw {
                self.default_directory = default_directory;
            }
            let record = record?;
            if let Some(restore) = &mut self.photo_format_restore {
                restore.record = Some(record);
            }
            self.status.active_record_id = Some(self.records[record].id.clone());
            self.snapshot();
            self.settling = Some(Settling {
                record,
                until: Instant::now() + Duration::from_secs(3),
            });
            // Retain the record even on an ambiguous shutter timeout. Never retry the shutter.
            let result = self.backend.capture();
            if let Ok(Some(file)) = &result {
                self.queue(file.clone(), Some(record));
            }
            self.settling.as_mut().unwrap().until = Instant::now() + Duration::from_secs(3);
            result.map(|_| ())
        })();
        // Once a shutter was attempted, allow file events/downloads to finish before restoring.
        if let Err(error) = result {
            let restore = if self.settling.is_none() {
                self.restore_photo_format()
            } else {
                Ok(())
            };
            self.status.operation = CameraOperation::Capturing;
            return Err(match restore {
                Ok(()) => error,
                Err(cleanup) => CameraError::new(
                    error.kind,
                    format!("{}; {}", error.message, cleanup.message),
                ),
            });
        }
        Ok(())
    }

    fn finish_raw_capture(&mut self) {
        if self.status.info.is_none()
            || self.status.recording
            || self.settling.is_some()
            || self.pending.iter().any(|p| !p.failed)
            || self.photo_format_restore.as_ref().is_none_or(|restore| {
                restore
                    .last_attempt
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
            })
        {
            return;
        }
        if let Err(error) = self.restore_photo_format() {
            self.error(error);
        }
        self.status.busy = None;
        self.resting_operation();
        self.snapshot();
    }

    fn restore_photo_format(&mut self) -> CameraResult<()> {
        let Some(restore) = &mut self.photo_format_restore else {
            return Ok(());
        };
        restore.last_attempt = Some(Instant::now());
        let original = restore.original.clone();
        self.status.active_record_id = restore.record.map(|r| self.records[r].id.clone());
        self.status.operation = CameraOperation::SettingParameter("imageformat".into());
        self.busy("Restoring photo format");
        let result = (|| {
            let mut info = self.backend.info()?;
            if !info
                .parameters
                .iter()
                .any(|p| p.id == "imageformat" && p.value == original)
            {
                self.backend.set_parameter("imageformat", &original)?;
                info = self.backend.info()?;
            }
            if !info
                .parameters
                .iter()
                .any(|p| p.id == "imageformat" && p.value == original)
            {
                return Err(CameraError::new(
                    CameraErrorKind::Busy,
                    "Camera has not confirmed the original format",
                ));
            }
            self.status.info = Some(info);
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.photo_format_restore = None;
                self.emit(CameraEvent::Notice(
                    "Original photo format restored.".into(),
                ));
                Ok(())
            }
            Err(error) => Err(CameraError::new(
                error.kind,
                format!(
                    "Could not restore the original photo format: {}. Restoration will retry; reconnect the same camera if disconnected.",
                    error.message
                ),
            )),
        }
    }

    fn refresh(&mut self) -> CameraResult<()> {
        let info = self.backend.info()?;
        let recording = info.recording_to_card();
        self.status.info = Some(info);
        if recording == Some(false) && self.status.recording {
            self.status.recording = false;
            self.status.recording_started = None;
            self.stop_requested = false;
            if let Some(record) = self.active_movie {
                self.settling = Some(Settling {
                    record,
                    until: Instant::now() + Duration::from_secs(3),
                });
                self.status.busy = Some("Camera stopped; waiting for video files".into());
            }
            self.emit(CameraEvent::Notice(
                "Camera recording stopped. Collecting all video segments.".into(),
            ));
        } else if recording == Some(true) && !self.status.recording {
            self.status.recording = true;
            self.status.recording_started = Some(Instant::now());
            if !self.movie_start_pending || self.active_movie.is_none() {
                self.active_movie = Some(self.create_record(
                    self.default_directory.clone(),
                    CaptureKind::Movie,
                    BTreeMap::new(),
                )?);
            }
        }
        if recording.is_some() {
            self.movie_start_pending = false;
        }
        self.snapshot();
        Ok(())
    }

    fn stop_recording(&mut self) -> CameraResult<()> {
        // A refresh may reveal a natural stop, or the SDRAM Live View state. Neither needs
        // an EVFRecordStatus write, and some still-photo modes do not allow None at all.
        self.refresh()?;
        if !self.status.recording && !self.movie_start_pending {
            return Ok(());
        }
        self.backend.record(false)?;
        self.stop_requested = true;
        self.movie_start_pending = false;
        self.refresh()?;
        if self.status.recording {
            self.emit(CameraEvent::Notice(
                "Stop command sent. Waiting for the camera to confirm recording has stopped."
                    .into(),
            ));
        }
        Ok(())
    }

    fn lost_connection(&mut self) {
        let _ = self.backend.disconnect();
        self.status.info = None;
        self.status.preview = false;
        self.status.recording = false;
        self.status.recording_started = None;
        self.status.busy = None;
        self.status.operation = CameraOperation::Idle;
        self.status.active_record_id = None;
        self.stop_requested = false;
        self.settling = None;
        self.emit(CameraEvent::Notice("Connection lost. Files already saved remain available. Reconnect the same camera and use Recover downloads; the shutter will not be retried.".into()));
        self.snapshot();
    }

    fn create_record(
        &mut self,
        directory: PathBuf,
        kind: CaptureKind,
        context: BTreeMap<String, String>,
    ) -> CameraResult<usize> {
        let info = self.status.info.clone().ok_or_else(|| {
            CameraError::new(CameraErrorKind::Disconnected, "Camera is disconnected")
        })?;
        let record = new_record(&directory, kind, info, context).map_err(storage_error)?;
        self.default_directory = directory;
        save_record(&record).map_err(storage_error)?;
        let index = self.records.len();
        self.emit(CameraEvent::Record(record.clone()));
        self.records.push(record);
        Ok(index)
    }

    fn queue(&mut self, file: RemoteFile, preferred: Option<usize>) {
        if !file.is_media() || self.known.contains(&file) {
            return;
        }
        let key = file.group_key();
        let record = self
            .groups
            .get(&key)
            .copied()
            .or_else(|| {
                preferred.filter(|&index| {
                    self.records[index].kind != CaptureKind::Movie || file.is_movie()
                })
            })
            .or_else(|| {
                if file.is_movie() {
                    self.active_movie
                } else {
                    None
                }
            })
            .or_else(|| {
                self.settling.as_ref().map(|s| s.record).filter(|&index| {
                    self.records[index].kind != CaptureKind::Movie || file.is_movie()
                })
            });
        let record = match record {
            Some(index) => index,
            None => match self.create_record(
                self.default_directory.clone(),
                CaptureKind::Recovered,
                BTreeMap::new(),
            ) {
                Ok(index) => index,
                Err(e) => {
                    self.error(e);
                    return;
                }
            },
        };
        self.records[record].pending_files.push(file.clone());
        let persisted = save_record(&self.records[record]);
        self.known.insert(file.clone());
        self.groups.insert(key, record);
        let failed = persisted.is_err();
        self.pending.push_back(PendingDownload {
            file,
            record,
            failed,
        });
        if let Err(e) = persisted {
            self.emit(CameraEvent::Error {
                error: storage_error(e),
                operation: CameraOperation::Downloading,
                record_id: Some(self.records[record].id.clone()),
            });
        }
        self.emit(CameraEvent::Record(self.records[record].clone()));
        self.snapshot();
    }

    fn reconcile(&mut self, record: Option<usize>) -> CameraResult<()> {
        for file in self.backend.list_files()? {
            self.queue(file, record);
        }
        Ok(())
    }

    fn download_next(&mut self) {
        if self.status.recording {
            return;
        }
        let Some(index) = self
            .pending
            .iter()
            .position(|p| !p.failed && !(p.file.is_movie() && self.settling.is_some()))
        else {
            return;
        };
        let Some(mut pending) = self.pending.remove(index) else {
            return;
        };
        self.status.operation = CameraOperation::Downloading;
        let record_id = self.records[pending.record].id.clone();
        self.status.active_record_id = Some(record_id.clone());
        self.busy(&format!("Downloading {}", pending.file.name));
        let events = self.events.clone();
        let name = pending.file.name.clone();
        self.emit(CameraEvent::Transfer {
            record_id: record_id.clone(),
            name: name.clone(),
            fraction: None,
        });
        let mut last = Instant::now() - Duration::from_secs(1);
        let result = download_atomic(
            &mut *self.backend,
            &pending.file,
            &self.records[pending.record].directory,
            &mut |fraction| {
                if fraction >= 1.0 || last.elapsed() >= Duration::from_millis(100) {
                    let _ = events.send(CameraEvent::Transfer {
                        record_id: record_id.clone(),
                        name: name.clone(),
                        fraction: Some(fraction),
                    });
                    last = Instant::now();
                }
            },
        );
        match result {
            Ok(file) => {
                let mut record = self.records[pending.record].clone();
                record.pending_files.retain(|f| *f != pending.file);
                record.files.retain(|f| f.remote != pending.file);
                record.files.push(file);
                match save_record(&record) {
                    Ok(()) => {
                        self.records[pending.record] = record.clone();
                        self.emit(CameraEvent::Record(record));
                        self.emit(CameraEvent::Transfer {
                            record_id: record_id.clone(),
                            name,
                            fraction: Some(1.0),
                        });
                    }
                    Err(e) => {
                        // Keep the durable pending entry until metadata publication succeeds.
                        // A retry verifies an already published file against a fresh download.
                        pending.failed = true;
                        self.pending.push_back(pending);
                        self.error(storage_error(e));
                    }
                }
            }
            Err(e) => {
                pending.failed = true;
                self.pending.push_back(pending);
                self.error(e);
            }
        }
        self.status.busy = self
            .settling
            .as_ref()
            .map(|_| "Waiting for camera files".into());
        self.resting_operation();
        self.snapshot();
    }
}

pub fn default_directory() -> PathBuf {
    std::env::var_os("NODE_FORGE_CAPTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("Pictures/Node Forge Captures")
        })
}

fn storage_error(e: impl std::fmt::Display) -> CameraError {
    CameraError::new(CameraErrorKind::Storage, e.to_string())
}
static NEXT_RECORD: AtomicU64 = AtomicU64::new(1);
fn new_record(
    root: &Path,
    kind: CaptureKind,
    camera: CameraInfo,
    context: BTreeMap<String, String>,
) -> std::io::Result<CaptureRecord> {
    fs::create_dir_all(root)?;
    let started_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    for _ in 0..100 {
        let id = format!(
            "capture-{started_unix_ms}-{}",
            NEXT_RECORD.fetch_add(1, Ordering::Relaxed)
        );
        let directory = root.join(&id);
        match fs::create_dir(&directory) {
            Ok(()) => {
                return Ok(CaptureRecord {
                    id,
                    kind,
                    started_unix_ms,
                    camera,
                    context,
                    files: Vec::new(),
                    pending_files: Vec::new(),
                    directory,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other(
        "Unable to allocate capture directory",
    ))
}

fn save_record(record: &CaptureRecord) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(record).map_err(std::io::Error::other)?;
    let path = record.directory.join("capture.json");
    let temp = record.directory.join("capture.json.tmp");
    let mut output = File::create(&temp)?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    fs::rename(temp, path)
}

fn camera_identity(info: &CameraInfo) -> String {
    format!(
        "{}:{}",
        info.model,
        info.parameters
            .iter()
            .find(|p| p.id == "serialnumber")
            .map(|p| format!("{:?}", p.value))
            .unwrap_or_default()
    )
}

pub fn load_records(directory: &Path) -> std::io::Result<Vec<CaptureRecord>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir()
            || !entry.file_name().to_string_lossy().starts_with("capture-")
        {
            continue;
        }
        let path = entry.path().join("capture.json");
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path)?;
        let mut record: CaptureRecord = serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::other(format!("Read {}: {e}", path.display())))?;
        // A copied capture folder remains usable; manifests never redirect future writes.
        for file in &mut record.files {
            if let Ok(relative) = file.path.strip_prefix(&record.directory) {
                file.path = entry.path().join(relative);
            }
        }
        record.directory = entry.path();
        records.push(record);
    }
    records.sort_by_key(|r| r.started_unix_ms);
    Ok(records)
}

fn download_atomic(
    backend: &mut dyn CameraBackend,
    remote: &RemoteFile,
    directory: &Path,
    progress: &mut dyn FnMut(f32),
) -> CameraResult<CapturedFile> {
    if remote.name.is_empty()
        || remote.name == "."
        || remote.name == ".."
        || remote.name.contains(['/', '\\', '\0'])
    {
        return Err(CameraError::new(
            CameraErrorKind::Invalid,
            "Camera returned an unsafe filename",
        ));
    }
    let folder_hash = format!("{:x}", Sha256::digest(remote.folder.as_bytes()));
    let target_dir = directory.join("files").join(&folder_hash[..12]);
    fs::create_dir_all(&target_dir).map_err(storage_error)?;
    let path = target_dir.join(&remote.name);
    let partial = target_dir.join(format!("{}.partial", remote.name));
    let mut created = false;
    let result = (|| {
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
            .map_err(storage_error)?;
        created = true;
        backend.download(remote, &output, progress)?;
        output.sync_all().map_err(storage_error)?;
        let bytes = output.metadata().map_err(storage_error)?.len();
        if bytes == 0 {
            return Err(CameraError::new(
                CameraErrorKind::Storage,
                "Camera downloaded an empty file",
            ));
        }
        drop(output);
        let sha256 = hash_file(&partial).map_err(storage_error)?;
        // Atomic no-clobber publication on the same filesystem. Never overwrite a capture.
        match fs::hard_link(&partial, &path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // A previous publication may have succeeded just before a manifest write failed.
                // Verify the complete bytes again; a name alone never establishes completeness.
                if fs::symlink_metadata(&path)
                    .map_err(storage_error)?
                    .file_type()
                    .is_symlink()
                    || hash_file(&path).map_err(storage_error)? != sha256
                {
                    return Err(CameraError::new(
                        CameraErrorKind::Storage,
                        format!(
                            "Existing file differs from camera original: {}",
                            path.display()
                        ),
                    ));
                }
            }
            Err(e) => return Err(storage_error(e)),
        }
        fs::remove_file(&partial).map_err(storage_error)?;
        Ok(CapturedFile {
            remote: remote.clone(),
            path,
            bytes,
            sha256,
        })
    })();
    if result.is_err() && created && partial.exists() {
        let _ = fs::remove_file(&partial);
    }
    result
}

fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut input = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn decode_preview(bytes: Vec<u8>) -> CameraResult<([u32; 2], Vec<u8>)> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| CameraError::new(CameraErrorKind::Invalid, e.to_string()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| {
            CameraError::new(
                CameraErrorKind::Invalid,
                format!("Decode camera preview: {e}"),
            )
        })?
        .to_rgba8();
    Ok(([image.width(), image.height()], image.into_raw()))
}

#[cfg(test)]
mod tests;
