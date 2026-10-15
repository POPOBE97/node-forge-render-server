use super::*;
use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
struct MockState {
    files: Vec<RemoteFile>,
    capture_result: Option<RemoteFile>,
    shutter_calls: usize,
    shutter_error: bool,
    download_error: bool,
    downloads: usize,
    recording: bool,
    record_start_error: bool,
    connect_failures: usize,
    connect_error: Option<CameraErrorKind>,
    connect_calls: usize,
    connect_errors: VecDeque<CameraErrorKind>,
    record_target: Option<String>,
    record_choices: Option<Vec<String>>,
    movie_switch: Option<String>,
    record_commands: Vec<bool>,
    preview_commands: Vec<bool>,
    preview_error: bool,
    focus_parameters: Vec<CameraParameter>,
    focus_calls: Vec<(bool, u8)>,
    focus_trace: Vec<String>,
    parameter_error: bool,
    ignore_parameter_write: bool,
    focus_error: bool,
    shutter_formats: Vec<ParameterValue>,
    parameter_writes: Vec<(String, ParameterValue)>,
}
struct Mock(Rc<RefCell<MockState>>);
impl CameraBackend for Mock {
    fn connect(&mut self) -> CameraResult<CameraInfo> {
        {
            let mut state = self.0.borrow_mut();
            state.connect_calls += 1;
            if let Some(kind) = state.connect_errors.pop_front() {
                return Err(CameraError::new(kind, "Mock USB recovery"));
            }
            if state.connect_failures > 0 {
                state.connect_failures -= 1;
                return Err(CameraError::new(
                    state.connect_error.unwrap_or(CameraErrorKind::Occupied),
                    "Mock connection failure",
                ));
            }
        }
        self.info()
    }
    fn disconnect(&mut self) -> CameraResult<()> {
        Ok(())
    }
    fn info(&mut self) -> CameraResult<CameraInfo> {
        let state = self.0.borrow();
        let mut parameters = vec![CameraParameter {
            id: "movierecordtarget".into(),
            label: "Recording Destination".into(),
            read_only: false,
            kind: ParameterKind::Choice(
                state
                    .record_choices
                    .clone()
                    .unwrap_or_else(|| vec!["Card".into(), "None".into()]),
            ),
            value: ParameterValue::Choice(
                state
                    .record_target
                    .clone()
                    .unwrap_or_else(|| if state.recording { "Card" } else { "None" }.into()),
            ),
        }];
        if let Some(value) = &state.movie_switch {
            parameters.push(CameraParameter {
                id: "eosmovieswitch".into(),
                label: "Movie Switch".into(),
                read_only: true,
                kind: ParameterKind::Text,
                value: ParameterValue::Text(value.clone()),
            });
        }
        parameters.extend(state.focus_parameters.clone());
        Ok(CameraInfo {
            model: "Canon T4i test".into(),
            port: "test".into(),
            summary: String::new(),
            parameters,
        })
    }
    fn set_parameter(&mut self, id: &str, value: &ParameterValue) -> CameraResult<()> {
        let mut state = self.0.borrow_mut();
        state.focus_trace.push(format!("set {id}"));
        state.parameter_writes.push((id.into(), value.clone()));
        if state.parameter_error {
            return Err(CameraError::new(CameraErrorKind::Busy, "Setting rejected"));
        }
        if !state.ignore_parameter_write {
            if let Some(p) = state.focus_parameters.iter_mut().find(|p| p.id == id) {
                p.value = value.clone();
            }
        }
        Ok(())
    }
    fn preview(&mut self, enabled: bool) -> CameraResult<()> {
        self.0.borrow_mut().preview_commands.push(enabled);
        if self.0.borrow().preview_error {
            return Err(CameraError::new(CameraErrorKind::Busy, "Preview not ready"));
        }
        Ok(())
    }
    fn preview_frame(&mut self) -> CameraResult<Vec<u8>> {
        Err(CameraError::new(
            CameraErrorKind::Unsupported,
            "Mock preview",
        ))
    }
    fn focus(&mut self, near: bool, step: u8) -> CameraResult<()> {
        let mut state = self.0.borrow_mut();
        state.focus_trace.push("drive".into());
        state.focus_calls.push((near, step));
        if state.focus_error {
            return Err(CameraError::new(
                CameraErrorKind::Busy,
                "Lens drive rejected",
            ));
        }
        Ok(())
    }
    fn capture(&mut self) -> CameraResult<Option<RemoteFile>> {
        let mut state = self.0.borrow_mut();
        state.shutter_calls += 1;
        if let Some(value) = state
            .focus_parameters
            .iter()
            .find(|p| p.id == "imageformat")
            .map(|p| p.value.clone())
        {
            state.shutter_formats.push(value);
        }
        if state.shutter_error {
            Err(CameraError::new(
                CameraErrorKind::Busy,
                "Shutter response timed out",
            ))
        } else {
            Ok(state.capture_result.clone())
        }
    }
    fn record(&mut self, enabled: bool) -> CameraResult<()> {
        self.0.borrow_mut().record_commands.push(enabled);
        self.0.borrow_mut().recording = enabled;
        if enabled && self.0.borrow().record_start_error {
            Err(CameraError::new(
                CameraErrorKind::Busy,
                "Recording start response timed out",
            ))
        } else {
            Ok(())
        }
    }
    fn poll_event(&mut self, _: i32) -> CameraResult<BackendEvent> {
        Ok(BackendEvent::Timeout)
    }
    fn list_files(&mut self) -> CameraResult<Vec<RemoteFile>> {
        Ok(self.0.borrow().files.clone())
    }
    fn download(
        &mut self,
        _: &RemoteFile,
        mut output: &File,
        progress: &mut dyn FnMut(f32),
    ) -> CameraResult<()> {
        self.0.borrow_mut().downloads += 1;
        output
            .write_all(b"original camera bytes including metadata")
            .map_err(storage_error)?;
        progress(0.5);
        if self.0.borrow().download_error {
            return Err(CameraError::new(
                CameraErrorKind::Storage,
                "Simulated disk or USB write failure",
            ));
        }
        progress(1.0);
        Ok(())
    }
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "node-forge-camera-test-{}-{}",
            std::process::id(),
            NEXT_RECORD.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn remote(name: &str) -> RemoteFile {
    RemoteFile {
        folder: "/store/DCIM/100CANON".into(),
        name: name.into(),
    }
}
fn worker(root: &Path) -> (Worker, Rc<RefCell<MockState>>) {
    let state = Rc::new(RefCell::new(MockState::default()));
    let (_, commands) = bounded(8);
    let (events, _) = unbounded();
    let mut worker = Worker::new(
        Box::new(Mock(state.clone())),
        commands,
        events,
        Arc::new(Mutex::new(None)),
    );
    worker
        .command(CameraCommand::Connect {
            directory: root.into(),
            release_occupied: false,
        })
        .unwrap();
    worker.status.busy = None;
    worker.resting_operation();
    state.borrow_mut().preview_commands.clear();
    (worker, state)
}
fn capture(worker: &mut Worker, root: &Path) -> CameraResult<()> {
    worker.command(CameraCommand::Capture {
        directory: root.into(),
        context: BTreeMap::from([("pattern_id".into(), "gray-128".into())]),
    })
}

fn focus_parameters() -> Vec<CameraParameter> {
    [
        (
            "manualfocusdrive",
            "None",
            vec![
                "Near 1", "Near 2", "Near 3", "None", "Far 1", "Far 2", "Far 3",
            ],
        ),
        ("continuousaf", "On", vec!["Off", "On"]),
        ("movieservoaf", "On", vec!["Off", "On"]),
        ("focusmode", "One Shot", vec!["One Shot", "Manual"]),
    ]
    .into_iter()
    .map(|(id, value, choices)| CameraParameter {
        id: id.into(),
        label: id.into(),
        value: ParameterValue::Choice(value.into()),
        kind: ParameterKind::Choice(choices.into_iter().map(Into::into).collect()),
        read_only: false,
    })
    .collect()
}

#[test]
fn focus_disables_af_before_drive_and_acknowledges_each_step_without_capture() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    let (tx, rx) = unbounded();
    worker.events = tx;
    // Cached connection info has no focus parameters. The command must read fresh capabilities.
    state.borrow_mut().focus_parameters = focus_parameters();
    for near in [true, false] {
        for step in 1..=3 {
            worker.command(CameraCommand::Focus { near, step }).unwrap();
        }
    }
    let state = state.borrow();
    assert_eq!(
        state.focus_calls,
        [
            (true, 1),
            (true, 2),
            (true, 3),
            (false, 1),
            (false, 2),
            (false, 3)
        ]
    );
    assert_eq!(
        state.focus_trace,
        [
            "set continuousaf",
            "set movieservoaf",
            "drive",
            "drive",
            "drive",
            "drive",
            "drive",
            "drive"
        ]
    );
    assert_eq!(state.shutter_calls, 0);
    assert!(state.record_commands.is_empty());
    let acknowledged = rx
        .try_iter()
        .filter_map(|event| match event {
            CameraEvent::FocusAcknowledged { near, step } => Some((near, step)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(acknowledged, state.focus_calls);
}

#[test]
fn focus_never_drives_when_preconditions_or_af_readback_fail() {
    for failure in [
        "preview",
        "lens_mf",
        "missing_drive",
        "readonly_af",
        "write_error",
        "ignored_write",
        "invalid_step",
    ] {
        let temp = Temp::new();
        let (mut worker, state) = worker(&temp.0);
        let (tx, rx) = unbounded();
        worker.events = tx;
        state.borrow_mut().focus_parameters = focus_parameters();
        match failure {
            "preview" => worker.status.preview = false,
            "lens_mf" => {
                state.borrow_mut().focus_parameters[3].value =
                    ParameterValue::Choice("Manual".into())
            }
            "missing_drive" => {
                state.borrow_mut().focus_parameters.remove(0);
            }
            "readonly_af" => state.borrow_mut().focus_parameters[1].read_only = true,
            "write_error" => state.borrow_mut().parameter_error = true,
            "ignored_write" => state.borrow_mut().ignore_parameter_write = true,
            _ => {}
        }
        assert!(
            worker
                .command(CameraCommand::Focus {
                    near: true,
                    step: if failure == "invalid_step" { 0 } else { 3 }
                })
                .is_err(),
            "{failure}"
        );
        assert!(state.borrow().focus_calls.is_empty(), "{failure}");
        assert!(
            !rx.try_iter()
                .any(|e| matches!(e, CameraEvent::FocusAcknowledged { .. })),
            "{failure}"
        );
    }
}

#[test]
fn a_failed_lens_command_does_not_emit_success() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    state.borrow_mut().focus_parameters = focus_parameters();
    state.borrow_mut().focus_error = true;
    let (tx, rx) = unbounded();
    worker.events = tx;
    assert!(
        worker
            .command(CameraCommand::Focus {
                near: false,
                step: 3
            })
            .is_err()
    );
    assert!(
        !rx.try_iter()
            .any(|e| matches!(e, CameraEvent::FocusAcknowledged { .. }))
    );
}

#[test]
fn raw_jpeg_pair_deduplicates_events_preserves_bytes_and_manifest() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    let raw = remote("IMG_1001.CR2");
    let jpeg = remote("IMG_1001.JPG");
    state.borrow_mut().capture_result = Some(raw.clone());
    capture(&mut worker, &temp.0).unwrap();
    worker.queue(raw.clone(), None);
    worker.queue(jpeg.clone(), None);
    worker.queue(jpeg, None);
    assert_eq!(worker.pending.len(), 2);
    worker.download_next();
    worker.download_next();
    assert_eq!(state.borrow().downloads, 2);
    assert_eq!(worker.records.len(), 1);
    let record = &worker.records[0];
    assert_eq!(record.files.len(), 2);
    assert!(record.pending_files.is_empty());
    assert_eq!(record.context["pattern_id"], "gray-128");
    for file in &record.files {
        assert_eq!(
            fs::read(&file.path).unwrap(),
            b"original camera bytes including metadata"
        );
        assert_eq!(
            file.sha256,
            format!("{:x}", Sha256::digest(fs::read(&file.path).unwrap()))
        );
    }
    assert_eq!(load_records(&temp.0).unwrap()[0].files.len(), 2);
}

#[test]
fn partial_failure_is_retryable_without_publishing_or_repeating_shutter() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    state.borrow_mut().capture_result = Some(remote("IMG_1002.JPG"));
    state.borrow_mut().download_error = true;
    capture(&mut worker, &temp.0).unwrap();
    worker.download_next();
    assert_eq!(worker.pending.len(), 1);
    assert!(worker.pending[0].failed);
    assert!(worker.records[0].files.is_empty());
    assert_eq!(load_records(&temp.0).unwrap()[0].pending_files.len(), 1);
    let files_dir = worker.records[0].directory.join("files");
    for folder in fs::read_dir(files_dir).unwrap() {
        assert_eq!(fs::read_dir(folder.unwrap().path()).unwrap().count(), 0);
    }
    state.borrow_mut().download_error = false;
    worker.command(CameraCommand::RecoverDownloads).unwrap();
    worker.download_next();
    assert!(worker.pending.is_empty());
    assert_eq!(state.borrow().shutter_calls, 1);
    assert_eq!(worker.records[0].files.len(), 1);
}

#[test]
fn ambiguous_shutter_timeout_reconciles_without_recapture() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    state.borrow_mut().shutter_error = true;
    assert!(capture(&mut worker, &temp.0).is_err());
    state.borrow_mut().files = vec![remote("IMG_1003.CR2"), remote("IMG_1003.JPG")];
    worker.reconcile(Some(0)).unwrap();
    worker.download_next();
    worker.download_next();
    worker.reconcile(Some(0)).unwrap();
    assert_eq!(state.borrow().shutter_calls, 1);
    assert_eq!(worker.records[0].files.len(), 2);
    assert!(worker.pending.is_empty());
}

#[test]
fn movie_segments_wait_for_stop_and_keep_one_record_including_late_events() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    worker
        .command(CameraCommand::StartRecording {
            directory: temp.0.clone(),
            context: BTreeMap::new(),
        })
        .unwrap();
    worker.queue(remote("MVI_1004.MOV"), None);
    worker.download_next();
    assert_eq!(state.borrow().downloads, 0);
    // Natural stop by duration, card or camera button is observed by status refresh.
    state.borrow_mut().recording = false;
    worker.refresh().unwrap();
    assert!(!worker.status.recording);
    worker.queue(remote("MVI_1005.MOV"), None);
    worker.download_next();
    assert_eq!(state.borrow().downloads, 0);
    worker.settling = None;
    worker.download_next();
    worker.download_next();
    worker.queue(remote("MVI_1006.MOV"), None);
    worker.download_next();
    assert_eq!(worker.records.len(), 1);
    assert_eq!(worker.records[0].files.len(), 3);
    assert_eq!(state.borrow().downloads, 3);
}

#[test]
fn reconnect_recovers_persisted_pending_files_without_importing_old_card_files() {
    let temp = Temp::new();
    {
        let (mut worker, state) = worker(&temp.0);
        state.borrow_mut().capture_result = Some(remote("IMG_1007.JPG"));
        state.borrow_mut().download_error = true;
        capture(&mut worker, &temp.0).unwrap();
        worker.download_next();
    }
    let (mut worker, state) = worker(&temp.0);
    assert_eq!(worker.pending.len(), 1);
    assert!(worker.pending[0].failed);
    worker.command(CameraCommand::RecoverDownloads).unwrap();
    worker.download_next();
    assert_eq!(state.borrow().shutter_calls, 0);
    assert_eq!(worker.records[0].files.len(), 1);
}

#[test]
fn unsafe_camera_names_and_existing_files_are_never_overwritten() {
    let temp = Temp::new();
    let state = Rc::new(RefCell::new(MockState::default()));
    let mut backend = Mock(state);
    assert!(download_atomic(&mut backend, &remote("../escape.JPG"), &temp.0, &mut |_| {}).is_err());
    let file = remote("IMG_1008.JPG");
    let saved = download_atomic(&mut backend, &file, &temp.0, &mut |_| {}).unwrap();
    fs::write(&saved.path, b"must survive").unwrap();
    assert!(download_atomic(&mut backend, &file, &temp.0, &mut |_| {}).is_err());
    assert_eq!(fs::read(&saved.path).unwrap(), b"must survive");
    let partial = saved.path.with_file_name("IMG_1008.JPG.partial");
    fs::write(&partial, b"existing interrupted download").unwrap();
    assert!(download_atomic(&mut backend, &file, &temp.0, &mut |_| {}).is_err());
    assert_eq!(fs::read(partial).unwrap(), b"existing interrupted download");
}

#[test]
fn pending_capture_blocks_disconnect_and_overlapping_shutter() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    capture(&mut worker, &temp.0).unwrap();
    assert!(worker.command(CameraCommand::Disconnect).is_err());
    assert!(capture(&mut worker, &temp.0).is_err());
    assert_eq!(state.borrow().shutter_calls, 1);
}

#[test]
fn original_card_inventory_is_not_automatically_downloaded() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    worker.command(CameraCommand::Disconnect).unwrap();
    worker.camera_identity = None;
    state.borrow_mut().files = vec![remote("IMG_0001.JPG")];
    worker
        .command(CameraCommand::Connect {
            directory: temp.0.clone(),
            release_occupied: false,
        })
        .unwrap();
    worker.command(CameraCommand::RecoverDownloads).unwrap();
    assert!(worker.pending.is_empty());
    assert_eq!(state.borrow().downloads, 0);
}

#[test]
fn a_new_camera_initiated_movie_gets_a_new_record() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    for number in 1..=2 {
        state.borrow_mut().recording = true;
        worker.refresh().unwrap();
        worker.queue(remote(&format!("MVI_{number:04}.MOV")), None);
        state.borrow_mut().recording = false;
        worker.refresh().unwrap();
        worker.settling = None;
        worker.download_next();
    }
    assert_eq!(worker.records.len(), 2);
    assert!(worker.records.iter().all(|r| r.files.len() == 1));
}

#[test]
fn ambiguous_movie_start_blocks_new_capture_and_preserves_association() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    state.borrow_mut().record_start_error = true;
    assert!(
        worker
            .command(CameraCommand::StartRecording {
                directory: temp.0.clone(),
                context: BTreeMap::new()
            })
            .is_err()
    );
    assert!(capture(&mut worker, &temp.0).is_err());
    assert!(worker.command(CameraCommand::Disconnect).is_err());
    worker.refresh().unwrap();
    assert!(worker.status.recording);
    assert_eq!(worker.records.len(), 1);
    worker.queue(remote("MVI_1009.MOV"), None);
    worker.command(CameraCommand::StopRecording).unwrap();
    worker.settling = None;
    worker.download_next();
    assert_eq!(worker.records[0].files.len(), 1);
    assert_eq!(state.borrow().shutter_calls, 0);
}

#[test]
fn manifest_failure_after_publication_can_be_recovered_after_restart() {
    let temp = Temp::new();
    {
        let (mut worker, state) = worker(&temp.0);
        state.borrow_mut().capture_result = Some(remote("IMG_1010.JPG"));
        capture(&mut worker, &temp.0).unwrap();
        // Make only the manifest write fail; the original file can still be published.
        let blocker = worker.records[0].directory.join("capture.json.tmp");
        fs::create_dir(&blocker).unwrap();
        worker.download_next();
        assert_eq!(worker.pending.len(), 1);
        assert!(worker.pending[0].failed);
        assert_eq!(load_records(&temp.0).unwrap()[0].pending_files.len(), 1);
        fs::remove_dir(blocker).unwrap();
    }
    let (mut worker, state) = worker(&temp.0);
    worker.command(CameraCommand::RecoverDownloads).unwrap();
    worker.download_next();
    assert!(worker.pending.is_empty());
    assert_eq!(worker.records[0].files.len(), 1);
    assert_eq!(state.borrow().shutter_calls, 0);
    assert!(load_records(&temp.0).unwrap()[0].pending_files.is_empty());
}

#[test]
fn late_raw_companion_remains_in_its_original_capture_during_next_capture() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    state.borrow_mut().capture_result = Some(remote("IMG_1011.JPG"));
    capture(&mut worker, &temp.0).unwrap();
    worker.download_next();
    worker.settling = None;
    state.borrow_mut().capture_result = Some(remote("IMG_1012.JPG"));
    capture(&mut worker, &temp.0).unwrap();
    state.borrow_mut().files = vec![remote("IMG_1011.CR2"), remote("IMG_1012.CR2")];
    worker.reconcile(Some(1)).unwrap();
    while !worker.pending.is_empty() {
        worker.download_next();
    }
    assert!(
        worker.records[0]
            .files
            .iter()
            .all(|f| f.remote.name.starts_with("IMG_1011"))
    );
    assert!(
        worker.records[1]
            .files
            .iter()
            .all(|f| f.remote.name.starts_with("IMG_1012"))
    );
    assert_eq!(worker.records[0].files.len(), 2);
    assert_eq!(worker.records[1].files.len(), 2);
}

#[test]
fn occupied_connection_releases_once_and_stops_if_usb_is_reclaimed() {
    for failures in [1, 2] {
        let state = Rc::new(RefCell::new(MockState {
            connect_failures: failures,
            ..Default::default()
        }));
        let releases = std::cell::Cell::new(0);
        let mut notices = Vec::new();
        let result = super::super::access::connect_with_recovery(
            &mut Mock(state.clone()),
            true,
            || {
                releases.set(releases.get() + 1);
                Ok(vec!["ptpcamerad (PID 123)".into()])
            },
            |notice| notices.push(notice),
        );
        assert_eq!(result.is_ok(), failures == 1);
        assert_eq!(state.borrow().connect_calls, 2);
        assert_eq!(releases.get(), 1);
        assert!(notices.iter().any(|notice| notice.contains("ptpcamerad")));
    }
}

#[test]
fn client_release_is_never_attempted_for_opt_out_or_an_active_studio_session() {
    for (enabled, kind) in [
        (false, CameraErrorKind::Occupied),
        (true, CameraErrorKind::Busy),
    ] {
        let state = Rc::new(RefCell::new(MockState {
            connect_failures: 1,
            connect_error: Some(kind),
            ..Default::default()
        }));
        let result = super::super::access::connect_with_recovery(
            &mut Mock(state.clone()),
            enabled,
            || panic!("must not stop other clients"),
            |_| {},
        );
        assert!(result.is_err());
        assert_eq!(state.borrow().connect_calls, 1);
    }
}

#[test]
fn usb_reenumeration_after_release_is_retried_without_repeating_termination() {
    let state = Rc::new(RefCell::new(MockState {
        connect_errors: VecDeque::from([
            CameraErrorKind::Occupied,
            CameraErrorKind::NotFound,
            CameraErrorKind::Disconnected,
            CameraErrorKind::NotFound,
        ]),
        ..Default::default()
    }));
    let releases = std::cell::Cell::new(0);
    let mut waits = Vec::new();
    let result = super::super::access::connect_with_recovery_wait(
        &mut Mock(state.clone()),
        true,
        || {
            releases.set(releases.get() + 1);
            Ok(vec!["ptpcamerad (PID 123)".into()])
        },
        |_| {},
        |delay| waits.push(delay),
    );
    assert!(result.is_ok());
    assert_eq!(state.borrow().connect_calls, 5);
    assert_eq!(state.borrow().shutter_calls, 0);
    assert_eq!(releases.get(), 1);
    assert_eq!(waits.len(), 4);
    assert!(waits.iter().all(|delay| *delay > Duration::ZERO));
}

#[test]
fn usb_recovery_stops_after_a_bounded_number_of_attempts() {
    let state = Rc::new(RefCell::new(MockState {
        connect_errors: std::iter::once(CameraErrorKind::Occupied)
            .chain(std::iter::repeat_n(CameraErrorKind::NotFound, 10))
            .collect(),
        ..Default::default()
    }));
    let result = super::super::access::connect_with_recovery_wait(
        &mut Mock(state.clone()),
        true,
        || Ok(vec!["ptpcamerad".into()]),
        |_| {},
        |_| {},
    )
    .unwrap_err();
    assert_eq!(result.kind, CameraErrorKind::NotFound);
    assert!(result.message.contains("did not reappear"));
    assert_eq!(state.borrow().connect_calls, 8);
    assert_eq!(state.borrow().shutter_calls, 0);
}

#[test]
fn usb_recovery_does_not_retry_multiple_cameras_or_active_studio_sessions() {
    for kind in [CameraErrorKind::Invalid, CameraErrorKind::Busy] {
        let state = Rc::new(RefCell::new(MockState {
            connect_errors: VecDeque::from([CameraErrorKind::Occupied, kind]),
            ..Default::default()
        }));
        assert!(
            super::super::access::connect_with_recovery_wait(
                &mut Mock(state.clone()),
                true,
                || Ok(vec!["ptpcamerad".into()]),
                |_| {},
                |_| {},
            )
            .is_err()
        );
        assert_eq!(state.borrow().connect_calls, 2);
    }
}

#[test]
fn live_view_sdram_never_becomes_a_movie_and_can_be_stopped_and_disconnected() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    // Values copied from the T4i's actual still-photo Live View capture.json.
    state.borrow_mut().record_target = Some("SDRAM".into());
    state.borrow_mut().record_choices = Some(vec!["SDRAM".into()]);
    state.borrow_mut().movie_switch = Some("0".into());
    worker.command(CameraCommand::Preview(true)).unwrap();
    for _ in 0..4 {
        worker.refresh().unwrap();
    }
    assert!(worker.status.preview);
    assert!(!worker.status.recording);
    assert!(worker.status.recording_started.is_none());
    assert!(worker.records.is_empty());
    assert!(state.borrow().record_commands.is_empty());
    worker.command(CameraCommand::Preview(false)).unwrap();
    worker.command(CameraCommand::Disconnect).unwrap();
    assert_eq!(state.borrow().preview_commands, [true, false]);
    assert!(state.borrow().record_commands.is_empty());
}

#[test]
fn unavailable_movie_mode_rejects_record_without_creating_a_capture_or_sending_commands() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    for (switch, choices) in [("0", vec!["Card", "None"]), ("1", vec!["SDRAM"])] {
        state.borrow_mut().movie_switch = Some(switch.into());
        state.borrow_mut().record_choices = Some(choices.into_iter().map(Into::into).collect());
        assert!(
            worker
                .command(CameraCommand::StartRecording {
                    directory: temp.0.clone(),
                    context: BTreeMap::new()
                })
                .is_err()
        );
    }
    assert!(worker.records.is_empty());
    assert!(state.borrow().record_commands.is_empty());
    assert!(state.borrow().preview_commands.is_empty());
}

#[test]
fn stopping_an_already_idle_or_sdram_camera_never_writes_an_invalid_none_value() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    for target in ["SDRAM", "None"] {
        state.borrow_mut().record_target = Some(target.into());
        state.borrow_mut().record_choices = Some(vec![target.into()]);
        // Covers a stale REC display or a camera-initiated stop just before the click.
        worker.status.recording = true;
        worker.status.recording_started = Some(Instant::now());
        worker.command(CameraCommand::StopRecording).unwrap();
        assert!(!worker.status.recording);
        assert!(worker.status.recording_started.is_none());
    }
    assert!(state.borrow().record_commands.is_empty());
}

#[test]
fn stopping_a_movie_waits_for_camera_confirmation_before_downloading() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    worker
        .command(CameraCommand::StartRecording {
            directory: temp.0.clone(),
            context: BTreeMap::new(),
        })
        .unwrap();
    worker.queue(remote("MVI_1011.MOV"), None);
    // The stop command succeeds, but the camera continues to report Card while finishing.
    state.borrow_mut().record_target = Some("Card".into());
    worker.command(CameraCommand::StopRecording).unwrap();
    assert_eq!(state.borrow().record_commands, [true, false]);
    assert!(worker.status.recording);
    assert!(worker.status.recording_started.is_some());
    assert!(worker.settling.is_none());
    worker.download_next();
    assert_eq!(state.borrow().downloads, 0);

    state.borrow_mut().record_target = Some("None".into());
    worker.refresh().unwrap();
    assert!(!worker.status.recording);
    assert!(worker.status.recording_started.is_none());
    assert!(worker.settling.is_some());
    worker.settling = None;
    worker.download_next();
    assert_eq!(state.borrow().downloads, 1);
    assert_eq!(worker.records.len(), 1);
}

#[test]
fn unknown_record_target_is_not_positive_evidence_of_recording() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    for value in ["Unknown value 0007", "", "0"] {
        state.borrow_mut().record_target = Some(value.into());
        worker.refresh().unwrap();
        assert!(!worker.status.recording);
        assert!(worker.records.is_empty());
    }
}

#[test]
fn connect_starts_preview_once_and_refresh_never_takes_a_picture_or_movie() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    worker.command(CameraCommand::Disconnect).unwrap();
    state.borrow_mut().preview_commands.clear();
    worker
        .command(CameraCommand::Connect {
            directory: temp.0.clone(),
            release_occupied: false,
        })
        .unwrap();
    worker.command(CameraCommand::Refresh).unwrap();
    worker
        .command(CameraCommand::Connect {
            directory: temp.0.clone(),
            release_occupied: false,
        })
        .unwrap();
    assert_eq!(state.borrow().preview_commands, [true]);
    assert_eq!(state.borrow().shutter_calls, 0);
    assert!(state.borrow().record_commands.is_empty());
    assert!(worker.status.preview);
}

#[test]
fn retry_is_scoped_to_selected_capture_without_repeating_shutter() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    state.borrow_mut().download_error = true;
    capture(&mut worker, &temp.0).unwrap();
    worker.queue(remote("IMG_6001.JPG"), Some(0));
    worker.settling = None;
    worker.download_next();
    let other = worker
        .create_record(temp.0.clone(), CaptureKind::Recovered, BTreeMap::new())
        .unwrap();
    worker.queue(remote("IMG_6002.JPG"), Some(other));
    worker.download_next();
    assert!(worker.pending.iter().all(|p| p.failed));
    worker
        .command(CameraCommand::RetryDownloads {
            record_id: worker.records[0].id.clone(),
        })
        .unwrap();
    assert!(
        !worker
            .pending
            .iter()
            .find(|p| p.record == 0)
            .unwrap()
            .failed
    );
    assert!(
        worker
            .pending
            .iter()
            .find(|p| p.record == other)
            .unwrap()
            .failed
    );
    assert_eq!(state.borrow().shutter_calls, 1);
}

#[test]
fn automatic_preview_failure_keeps_connection_and_can_be_retried() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    worker.command(CameraCommand::Disconnect).unwrap();
    state.borrow_mut().preview_error = true;
    assert!(
        worker
            .command(CameraCommand::Connect {
                directory: temp.0.clone(),
                release_occupied: false
            })
            .is_err()
    );
    assert!(worker.status.info.is_some());
    assert!(!worker.status.preview);
    assert_eq!(worker.status.operation, CameraOperation::StartingPreview);
    state.borrow_mut().preview_error = false;
    worker.command(CameraCommand::Preview(true)).unwrap();
    assert!(worker.status.preview);
    assert_eq!(state.borrow().shutter_calls, 0);
    assert!(state.borrow().record_commands.is_empty());
}

fn photo_formats(state: &Rc<RefCell<MockState>>, current: &str) {
    state.borrow_mut().focus_parameters = ["imageformat", "imageformatsd"]
        .into_iter()
        .map(|id| CameraParameter {
            id: id.into(),
            label: id.into(),
            value: ParameterValue::Choice(current.into()),
            kind: ParameterKind::Choice(vec!["L".into(), "RAW".into(), "RAW + L".into()]),
            read_only: false,
        })
        .collect();
}
fn capture_raw(worker: &mut Worker, root: &Path) -> CameraResult<()> {
    worker.command(CameraCommand::CaptureRaw {
        directory: root.into(),
        context: BTreeMap::new(),
    })
}
fn end_raw_capture(worker: &mut Worker) {
    let record = worker.settling.take().map(|s| s.record);
    worker.reconcile(record).unwrap();
    while worker.pending.iter().any(|p| !p.failed) {
        worker.download_next();
    }
    worker.finish_raw_capture();
}
#[test]
fn temporary_raw_switches_before_shutter_and_restores_after_transfer_without_touching_sd() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    photo_formats(&state, "L");
    state.borrow_mut().capture_result = Some(remote("IMG_9001.CR2"));
    capture_raw(&mut worker, &temp.0).unwrap();
    assert_eq!(
        state.borrow().shutter_formats,
        [ParameterValue::Choice("RAW".into())]
    );
    assert_eq!(
        worker.records[0]
            .camera
            .parameters
            .iter()
            .find(|p| p.id == "imageformat")
            .unwrap()
            .value,
        ParameterValue::Choice("RAW".into())
    );
    worker.finish_raw_capture();
    assert!(worker.photo_format_restore.is_some());
    end_raw_capture(&mut worker);
    assert!(worker.photo_format_restore.is_none());
    assert_eq!(state.borrow().downloads, 1);
    assert_eq!(
        state.borrow().parameter_writes,
        [
            ("imageformat".into(), ParameterValue::Choice("RAW".into())),
            ("imageformat".into(), ParameterValue::Choice("L".into()))
        ]
    );
    assert_eq!(
        state.borrow().focus_parameters[1].value,
        ParameterValue::Choice("L".into())
    );
    assert_eq!(state.borrow().shutter_calls, 1);
}
#[test]
fn temporary_raw_preserves_existing_raw_and_ordinary_capture_format() {
    for (format, raw) in [("RAW", true), ("RAW + L", true), ("L", false)] {
        let temp = Temp::new();
        let (mut worker, state) = worker(&temp.0);
        photo_formats(&state, format);
        if raw {
            capture_raw(&mut worker, &temp.0).unwrap();
        } else {
            capture(&mut worker, &temp.0).unwrap();
        }
        end_raw_capture(&mut worker);
        assert!(state.borrow().parameter_writes.is_empty());
        assert_eq!(
            state.borrow().shutter_formats,
            [ParameterValue::Choice(format.into())]
        );
    }
}
#[test]
fn temporary_raw_requires_confirmed_write_before_any_shutter() {
    for failure in ["write_error", "ignored_write", "read_only", "no_raw"] {
        let temp = Temp::new();
        let (mut worker, state) = worker(&temp.0);
        photo_formats(&state, "L");
        match failure {
            "write_error" => state.borrow_mut().parameter_error = true,
            "ignored_write" => state.borrow_mut().ignore_parameter_write = true,
            "read_only" => state.borrow_mut().focus_parameters[0].read_only = true,
            _ => {
                state.borrow_mut().focus_parameters[0].kind =
                    ParameterKind::Choice(vec!["L".into()])
            }
        }
        assert!(capture_raw(&mut worker, &temp.0).is_err(), "{failure}");
        assert_eq!(state.borrow().shutter_calls, 0);
        assert!(worker.photo_format_restore.is_none());
    }
}
#[test]
fn temporary_raw_restores_after_shutter_timeout_download_failure_or_storage_failure() {
    for failure in ["shutter", "download", "storage"] {
        let temp = Temp::new();
        let (mut worker, state) = worker(&temp.0);
        photo_formats(&state, "L");
        let target = if failure == "storage" {
            let path = temp.0.join("not-a-folder");
            fs::write(&path, b"existing file").unwrap();
            path
        } else {
            temp.0.clone()
        };
        state.borrow_mut().shutter_error = failure == "shutter";
        state.borrow_mut().download_error = failure == "download";
        state.borrow_mut().capture_result = Some(remote("IMG_9002.CR2"));
        let result = capture_raw(&mut worker, &target);
        assert_eq!(result.is_err(), failure != "download");
        end_raw_capture(&mut worker);
        assert!(worker.photo_format_restore.is_none(), "{failure}");
        assert_eq!(
            state.borrow().focus_parameters[0].value,
            ParameterValue::Choice("L".into())
        );
        assert_eq!(
            state.borrow().shutter_calls,
            usize::from(failure != "storage")
        );
        if failure == "download" {
            state.borrow_mut().download_error = false;
            worker.command(CameraCommand::RecoverDownloads).unwrap();
            worker.download_next();
            assert_eq!(state.borrow().shutter_calls, 1);
            assert_eq!(state.borrow().parameter_writes.len(), 2);
        }
    }
}
#[test]
fn temporary_raw_failed_restore_blocks_next_capture_and_retries_original_value() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    photo_formats(&state, "L");
    capture_raw(&mut worker, &temp.0).unwrap();
    state.borrow_mut().parameter_error = true;
    end_raw_capture(&mut worker);
    assert!(worker.photo_format_restore.is_some());
    assert!(capture_raw(&mut worker, &temp.0).is_err());
    state.borrow_mut().parameter_error = false;
    worker.restore_photo_format().unwrap();
    assert!(worker.photo_format_restore.is_none());
    assert_eq!(
        state.borrow().focus_parameters[0].value,
        ParameterValue::Choice("L".into())
    );
    assert_eq!(state.borrow().shutter_calls, 1);
}
#[test]
fn temporary_raw_survives_disconnect_and_restores_when_same_camera_reconnects() {
    let temp = Temp::new();
    let (mut worker, state) = worker(&temp.0);
    photo_formats(&state, "L");
    capture_raw(&mut worker, &temp.0).unwrap();
    worker.lost_connection();
    assert!(worker.photo_format_restore.is_some());
    worker
        .command(CameraCommand::Connect {
            directory: temp.0.clone(),
            release_occupied: false,
        })
        .unwrap();
    assert!(worker.photo_format_restore.is_none());
    assert_eq!(
        state.borrow().focus_parameters[0].value,
        ParameterValue::Choice("L".into())
    );
    assert_eq!(state.borrow().shutter_calls, 1);
}
