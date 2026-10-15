//! Opt-in UI acceptance fixtures. Never instantiate CameraService or access USB.
use super::*;

fn info() -> CameraInfo {
    let choices = [
        (
            "shutterspeed",
            "Shutter",
            "1/200",
            vec!["1/60", "1/125", "1/200", "1/250", "1/500"],
        ),
        (
            "aperture",
            "Aperture",
            "5.0",
            vec!["3.5", "4.0", "5.0", "5.6", "8.0"],
        ),
        (
            "iso",
            "ISO",
            "200",
            vec!["100", "200", "400", "800", "1600"],
        ),
        (
            "exposurecompensation",
            "Compensation",
            "0",
            vec!["-2", "-1", "0", "+1", "+2"],
        ),
        (
            "whitebalance",
            "White balance",
            "Manual",
            vec!["Auto", "Daylight", "Manual"],
        ),
        (
            "imageformat",
            "Photo format",
            "RAW + L",
            vec!["RAW", "RAW + L", "L"],
        ),
        (
            "imageformatsd",
            "SD image format",
            "RAW + L",
            vec!["RAW", "RAW + L", "L"],
        ),
        ("autoexposuremode", "Mode", "Manual", vec!["Manual"]),
        (
            "movierecordtarget",
            "Recording destination",
            "None",
            vec!["None", "Card"],
        ),
        ("continuousaf", "Continuous AF", "Off", vec!["Off", "On"]),
        (
            "manualfocusdrive",
            "Focus drive",
            "None",
            vec![
                "Near 1", "Near 2", "Near 3", "None", "Far 1", "Far 2", "Far 3",
            ],
        ),
    ];
    let mut parameters = choices
        .into_iter()
        .map(|(id, label, value, choices)| CameraParameter {
            id: id.into(),
            label: label.into(),
            value: ParameterValue::Choice(value.into()),
            kind: ParameterKind::Choice(choices.into_iter().map(Into::into).collect()),
            read_only: false,
        })
        .collect::<Vec<_>>();
    parameters.push(CameraParameter {
        id: "batterylevel".into(),
        label: "Battery".into(),
        value: ParameterValue::Text("84".into()),
        kind: ParameterKind::Text,
        read_only: true,
    });
    CameraInfo {
        model: "Canon EOS 650D".into(),
        port: "UI fixture · no USB session".into(),
        summary:
            "Simulated data for layout and interaction acceptance. No camera session is opened."
                .into(),
        parameters,
    }
}
impl Workspace {
    pub(in crate::camera_studio) fn preview_fixture(
        ctx: &egui::Context,
        path: PathBuf,
        state: &str,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            [
                "live",
                "review",
                "corrected-live",
                "corrected-review",
                "corrected-live-small",
                "recording",
                "stopping",
                "downloading",
                "error",
                "disconnected",
                "paused",
                "screen-geometry",
                "screen-geometry-jpeg",
                "screen-reference",
                "screen-inspection",
                "screen-geometry-small",
                "calibration-busy",
                "calibration-ready",
                "calibration-error-small",
                "screen-reference-small",
                "screen-inspection-small"
            ]
            .contains(&state),
            "Unknown UI state: {state}"
        );
        let mut w = Self::new();
        w.history_job = None;
        w.records.clear();
        w.fixture = Some(state.into());
        w.fixture_image = Some(path.clone());
        let (size, pixels, orientation) = decode_photo(&path)?;
        let texture = ctx.load_texture(
            "sys.camera.fixture",
            egui::ColorImage::from_rgba_unmultiplied([size[0] as usize, size[1] as usize], &pixels),
            egui::TextureOptions::LINEAR,
        );
        w.live = Some(ImageView {
            texture: texture.clone(),
            size,
            label: "Live".into(),
            sequence: 128,
            received: Instant::now(),
            orientation,
        });
        w.status.info = Some(info());
        w.status.preview = true;
        w.status.preview_fps = 7.2;
        for index in 0..6 {
            w.records.push(CaptureRecord {
                id: format!("fixture-{index}"),
                kind: CaptureKind::Photo,
                started_unix_ms: index,
                camera: info(),
                context: BTreeMap::new(),
                files: vec![CapturedFile {
                    remote: RemoteFile {
                        folder: "/DCIM/100CANON".into(),
                        name: format!("IMG_{:04}.JPG", 4101 + index),
                    },
                    path: path.clone(),
                    bytes: 3_500_000,
                    sha256: String::new(),
                }],
                pending_files: vec![],
                directory: path.parent().unwrap_or(Path::new("/")).to_owned(),
            });
        }
        if matches!(state, "review" | "corrected-review") {
            w.source = CanvasSource::Photo(path);
            w.selected_record = Some("fixture-5".into());
            w.follow_latest = false;
            w.photo = Some(ImageView {
                texture,
                size,
                label: "IMG_4106.JPG".into(),
                sequence: 0,
                received: Instant::now(),
                orientation,
            });
        }
        if matches!(state, "recording" | "stopping" | "downloading" | "error") {
            let mut record = w.records[5].clone();
            record.id = "fixture-6".into();
            record.files.clear();
            record.kind = CaptureKind::Movie;
            record.started_unix_ms = 6;
            w.records.push(record);
            w.status.active_record_id = Some("fixture-6".into());
        }
        if matches!(state, "recording" | "stopping") {
            w.status.recording = true;
            w.status.recording_started = Some(Instant::now() - Duration::from_secs(83));
            w.shooting_mode = ShootingMode::Video;
            w.status.operation = if state == "stopping" {
                CameraOperation::StoppingRecording
            } else {
                CameraOperation::Recording
            };
        }
        if matches!(state, "downloading" | "error") {
            w.records
                .last_mut()
                .unwrap()
                .pending_files
                .push(RemoteFile {
                    folder: "/DCIM/100CANON".into(),
                    name: "MVI_4107.MOV".into(),
                });
            w.status.pending_downloads = 1;
            if state == "downloading" {
                w.status.operation = CameraOperation::Downloading;
                w.transfers.insert(
                    "fixture-6".into(),
                    Transfer {
                        name: "MVI_4107.MOV".into(),
                        fraction: Some(0.46),
                    },
                );
            } else {
                w.issue(IssueTarget::Record("fixture-6".into()),"Download failed — originals remain on the card","Simulated transfer failure for UI acceptance. No device operation was performed.");
                w.selected_record = Some("fixture-6".into());
                w.source = CanvasSource::Record("fixture-6".into());
            }
        }
        if state == "disconnected" {
            w.status.info = None;
            w.status.preview = false;
            w.issue(
                IssueTarget::Device,
                "Camera connection needs attention",
                "Simulated USB disconnect. Reconnect your camera to resume.",
            );
        }
        if state == "paused" {
            w.status.preview = false;
        }
        if state.starts_with("screen-") {
            use crate::screen_test::{model::Workflow, ui::Panel};
            let workflow = if state.starts_with("screen-geometry") {
                Workflow::Geometry
            } else if state.starts_with("screen-inspection") {
                Workflow::Inspection
            } else {
                Workflow::Reference
            };
            w.screen_test = Some(Panel::preview(workflow));
            w.screen_test_open = !state.starts_with("screen-geometry");
            w.fixture_capture_countdown = Some(10);
            if state == "screen-geometry-jpeg" {
                w.status
                    .info
                    .as_mut()
                    .unwrap()
                    .parameters
                    .iter_mut()
                    .find(|p| p.id == "imageformat")
                    .unwrap()
                    .value = ParameterValue::Choice("L".into());
            }
            if state.ends_with("-small") {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1000., 680.)));
            }
        }
        if state.starts_with("calibration-") {
            w.screen_test = Some(crate::screen_test::ui::Panel::preview(
                crate::screen_test::model::Workflow::Reference,
            ));
            w.fixture_capture_countdown = Some(10);
            match state {
                "calibration-busy" => {
                    w.calibration_running = true;
                    let controller = &mut w.screen_test.as_mut().unwrap().controller;
                    controller.busy = true;
                    controller.message = "Restoring phone…".into();
                }
                "calibration-ready" => {
                    w.calibration_result = Some(Ok("Calibrated · 0.79 px · ready to shoot".into()))
                }
                _ => {
                    w.calibration_result = Some(Err(
                        "Simulated phone installation failure · no USB access".into(),
                    ));
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                        1000., 680.,
                    )));
                }
            }
        }
        if state.starts_with("corrected-") {
            // Explicit offline acceptance input; production sessions never hydrate geometry.
            #[derive(serde::Deserialize)]
            struct FixtureGeometry {
                profile: crate::screen_test::model::GeometryProfile,
                screen_size: [u32; 2],
                sensor_crop: [f64; 4],
            }
            let input = std::env::var("NODE_FORGE_CAMERA_FIXTURE_GEOMETRY")?;
            let g: FixtureGeometry = serde_json::from_slice(&std::fs::read(input)?)?;
            w.screen_test = Some(crate::screen_test::ui::Panel::preview(
                crate::screen_test::model::Workflow::Geometry,
            ));
            w.screen_test
                .as_ref()
                .unwrap()
                .controller
                .geometry
                .lock()
                .unwrap()
                .publish_display(g.profile.clone(), g.screen_size, g.sensor_crop)?;
            w.sync_display_geometry();
            for record in &mut w.records {
                record
                    .context
                    .insert("display_geometry_id".into(), g.profile.id.clone());
            }
            w.calibration_result = Some(Ok(format!(
                "Calibrated · {:.2} px · ready to shoot",
                g.profile.holdout_rms
            )));
            w.fixture_capture_countdown = Some(20);
            if state.ends_with("-small") {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1000., 680.)));
            }
        }
        Ok(w)
    }
    pub(super) fn fixture_command(&mut self, command: CameraCommand) {
        match command {
            CameraCommand::Connect { .. } => {
                self.status.info = Some(info());
                self.status.preview = true;
                self.issues.clear();
            }
            CameraCommand::Disconnect => {
                self.status = CameraStatus::default();
            }
            CameraCommand::Preview(enabled) => {
                self.status.preview = enabled;
            }
            CameraCommand::SetParameter { id, value } => {
                if let Some(p) = self
                    .status
                    .info
                    .as_mut()
                    .and_then(|i| i.parameters.iter_mut().find(|p| p.id == id))
                {
                    p.value = value;
                }
            }
            CameraCommand::StartRecording { .. } => {
                self.status.recording = true;
                self.status.recording_started = Some(Instant::now());
                self.status.operation = CameraOperation::Recording;
            }
            CameraCommand::StopRecording => {
                self.status.recording = false;
                self.status.recording_started = None;
                self.status.operation = CameraOperation::Idle;
            }
            CameraCommand::Shutdown => self.closed = true,
            _ => {}
        }
    }
}

impl Workspace {
    pub(super) fn fixture_menu(&mut self, ui: &mut egui::Ui) {
        let mut change = None;
        let mut capture_all = false;
        ui.menu_button("UI PREVIEW · simulated camera", |ui| {
            for state in [
                "live",
                "review",
                "recording",
                "stopping",
                "downloading",
                "error",
                "disconnected",
                "paused",
                "screen-geometry",
                "screen-reference",
                "screen-inspection",
            ] {
                if ui
                    .selectable_label(self.fixture.as_deref() == Some(state), state)
                    .clicked()
                {
                    change = Some(state);
                    ui.close();
                }
            }
            ui.separator();
            for (label, size) in [
                ("1000 × 680", egui::vec2(1000., 680.)),
                ("1400 × 900", egui::vec2(1400., 900.)),
            ] {
                if action(ui, label, true) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                    ui.close();
                }
            }
            if action(ui, "Save all acceptance screenshots", true) {
                capture_all = true;
                ui.close();
            }
            if action(ui, "Save UI screenshot", true) {
                self.fixture_capture_countdown = Some(3);
                ui.close();
            }
        });
        if capture_all {
            self.fixture_capture_batch = [
                "live",
                "review",
                "recording",
                "stopping",
                "downloading",
                "error",
                "disconnected",
                "paused",
            ]
            .into_iter()
            .map(|state| (state.to_owned(), egui::vec2(1400., 900.)))
            .chain([("live".into(), egui::vec2(1000., 680.))])
            .collect();
            self.next_fixture_capture(ui.ctx());
        }
        if let Some(state) = change {
            if let Some(path) = self.fixture_image.clone() {
                match Self::preview_fixture(ui.ctx(), path, state) {
                    Ok(next) => *self = next,
                    Err(error) => self.issue(
                        IssueTarget::Device,
                        "Could not load UI fixture",
                        error.to_string(),
                    ),
                }
            }
        }
    }
    fn next_fixture_capture(&mut self, ctx: &egui::Context) {
        if let Some((state, size)) = self.fixture_capture_batch.pop_front() {
            let remaining = std::mem::take(&mut self.fixture_capture_batch);
            if let Some(path) = self.fixture_image.clone() {
                match Self::preview_fixture(ctx, path, &state) {
                    Ok(mut next) => {
                        next.fixture_capture_batch = remaining;
                        next.fixture_capture_countdown = Some(8);
                        *self = next;
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                    }
                    Err(error) => self.issue(
                        IssueTarget::Device,
                        "Could not capture UI fixture",
                        error.to_string(),
                    ),
                }
            }
        }
    }
    pub(super) fn fixture_screenshots(&mut self, ctx: &egui::Context) {
        if let Some(count) = self.fixture_capture_countdown {
            if self.thumbnails.is_loading() {
                // Capture the settled native UI, including its asynchronously decoded filmstrip.
                ctx.request_repaint();
            } else if count == 0 {
                self.fixture_capture_countdown = None;
                let size = ctx.content_rect().size();
                let path = std::env::current_dir()
                    .unwrap_or_default()
                    .join("tmp/camera-studio-ui")
                    .join(format!(
                        "{}-{}x{}.png",
                        self.fixture.as_deref().unwrap_or("ui"),
                        size.x.round(),
                        size.y.round()
                    ));
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(path)));
            } else {
                self.fixture_capture_countdown = Some(count - 1);
            }
        }
        let images = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|event| {
                    if let egui::Event::Screenshot {
                        user_data, image, ..
                    } = event
                    {
                        user_data
                            .data
                            .as_ref()?
                            .downcast_ref::<PathBuf>()
                            .map(|path| (path.clone(), image.clone()))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        });
        let completed = !images.is_empty();
        for (path, image) in images {
            std::thread::spawn(move || {
                let result = (|| -> anyhow::Result<()> {
                    std::fs::create_dir_all(path.parent().unwrap())?;
                    let rgba = image
                        .pixels
                        .iter()
                        .flat_map(|p| p.to_array())
                        .collect::<Vec<_>>();
                    image::save_buffer(
                        &path,
                        &rgba,
                        image.size[0] as u32,
                        image.size[1] as u32,
                        image::ColorType::Rgba8,
                    )?;
                    Ok(())
                })();
                match result {
                    Ok(()) => eprintln!("UI screenshot: {}", path.display()),
                    Err(error) => eprintln!("UI screenshot failed: {error}"),
                }
            });
        }
        if completed {
            self.next_fixture_capture(ctx);
        }
    }
}
