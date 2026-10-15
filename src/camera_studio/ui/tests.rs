use super::*;

#[test]
fn connection_errors_show_the_cause_without_opening_technical_details() {
    for (kind, message, expected) in [
        (
            CameraErrorKind::Occupied,
            "claim failed",
            "Camera USB is still occupied",
        ),
        (
            CameraErrorKind::NotFound,
            "No target camera",
            "Canon T4i / 650D not found",
        ),
        (
            CameraErrorKind::Invalid,
            "More than one Canon T4i / 650D is connected.",
            "More than one Canon",
        ),
    ] {
        let mut w = Workspace::new();
        w.accept_event(CameraEvent::Error {
            error: CameraError::new(kind, message),
            operation: CameraOperation::Connecting,
            record_id: None,
        });
        assert_eq!(w.issues[0].target, IssueTarget::Device);
        assert!(w.issues[0].summary.contains(expected));
        assert!(w.issues[0].detail.contains(message));
    }
}

#[test]
fn focus_returns_review_to_live_and_reports_acknowledgement_or_actual_error() {
    let mut w = Workspace::new();
    w.source = CanvasSource::Photo("old.jpg".into());
    w.send(CameraCommand::Focus {
        near: true,
        step: 2,
    });
    assert_eq!(w.source, CanvasSource::Live);
    assert!(w.focus_feedback.is_none());
    w.accept_event(CameraEvent::FocusAcknowledged {
        near: true,
        step: 2,
    });
    assert!(w.focus_feedback.as_ref().unwrap().0.contains("Near 2"));
    w.send(CameraCommand::Focus {
        near: false,
        step: 1,
    });
    assert!(w.focus_feedback.is_none());
    w.accept_event(CameraEvent::Error {
        error: CameraError::new(CameraErrorKind::Unsupported, "Set the lens switch to AF"),
        operation: CameraOperation::Focusing,
        record_id: None,
    });
    assert_eq!(w.issues[0].target, IssueTarget::Focus);
    assert_eq!(w.issues[0].summary, "Set the lens switch to AF");
}

#[test]
fn reopening_tool_window_reuses_offline_workspace_and_does_not_connect() {
    let mut window = None;
    super::super::open_tool_window(&mut window);
    let first = window.as_ref().unwrap().workspace.clone();
    super::super::open_tool_window(&mut window);
    assert!(std::sync::Arc::ptr_eq(
        &first,
        &window.as_ref().unwrap().workspace
    ));
    let ctx = egui::Context::default();
    crate::ui::fonts::configure_egui_fonts(&ctx);
    ctx.set_embed_viewports(true);
    let _ = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        },
        |ui| super::super::show_tool_window(ui.ctx(), &mut window),
    );
    let workspace = first.lock().unwrap();
    assert!(workspace.service.is_none());
    assert!(workspace.status.info.is_none());
    assert!(!workspace.closed);
    assert_eq!(workspace.release_occupied, cfg!(target_os = "macos"));
}

#[test]
fn close_waits_for_recording_and_pending_files_but_releases_an_idle_window() {
    for status in [
        CameraStatus {
            recording: true,
            ..Default::default()
        },
        CameraStatus {
            pending_downloads: 1,
            ..Default::default()
        },
        CameraStatus {
            operation: CameraOperation::Downloading,
            ..Default::default()
        },
    ] {
        let mut workspace = Workspace::new();
        workspace.status = status;
        workspace.request_close(Some(egui::ViewportId::ROOT));
        assert!(workspace.close_dialog);
        assert!(!workspace.closed);
        assert!(!workspace.shutdown_sent);
        assert!(!workspace.close_after_work);
    }
    let mut workspace = Workspace::new();
    workspace.request_close(None);
    workspace.poll(&egui::Context::default());
    assert!(workspace.closed);
}

#[test]
fn viewport_centers_fit_and_preserves_physical_pixels_on_retina() {
    let rect = egui::Rect::from_min_size(egui::pos2(340., 40.), egui::vec2(1060., 748.));
    let mut view = ImageTransform::default();
    let fit = view.image_rect([960, 640], rect, 2.);
    assert_eq!(fit.center(), rect.center());
    assert!(rect.contains_rect(fit));
    view.original();
    assert_eq!(
        view.image_rect([960, 640], rect, 2.).size(),
        egui::vec2(480., 320.)
    );
    view.rotate();
    assert_eq!(
        view.image_rect([960, 640], rect, 2.).size(),
        egui::vec2(320., 480.)
    );
    assert_eq!(
        view.image_rect([960, 640], rect, 2.).center(),
        rect.center()
    );
}

#[test]
fn zoom_keeps_the_image_point_under_the_pointer() {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900., 650.));
    let pointer = egui::pos2(300., 180.);
    let mut view = ImageTransform::default();
    let before = view.image_rect([960, 640], rect, 2.);
    let uv = (pointer - before.min) / before.size();
    view.zoom_at(2., pointer, [960, 640], rect, 2.);
    let after = view.image_rect([960, 640], rect, 2.);
    assert!(((pointer - after.min) / after.size() - uv).length() < 0.0001);
}

#[test]
fn capture_events_never_steal_live_or_existing_review_selection() {
    let mut w = Workspace::new();
    let record = CaptureRecord {
        id: "ui-test".into(),
        kind: CaptureKind::Photo,
        started_unix_ms: 0,
        camera: CameraInfo::default(),
        context: BTreeMap::new(),
        files: vec![],
        pending_files: vec![],
        directory: PathBuf::new(),
    };
    w.accept_event(CameraEvent::Record(record.clone()));
    assert_eq!(w.source, CanvasSource::Live);
    w.source = CanvasSource::Photo("old.jpg".into());
    w.accept_event(CameraEvent::Record(record.clone()));
    assert_eq!(w.source, CanvasSource::Photo("old.jpg".into()));
    assert_eq!(w.record_state(&record), "No files received");
    w.status.active_record_id = Some(record.id.clone());
    w.status.operation = CameraOperation::WaitingFiles;
    assert_eq!(w.record_state(&record), "Waiting for files…");
}

#[test]
fn local_view_controls_and_mode_selection_do_not_create_a_camera_session() {
    let mut w = Workspace::new();
    w.shooting_mode = ShootingMode::Video;
    w.live_view.original();
    w.live_view.rotate();
    w.photo_view.fit();
    w.return_live();
    assert!(w.service.is_none());
    assert_eq!(w.live_view.scale, ScaleMode::Physical(1.));
    assert_eq!(w.photo_view.scale, ScaleMode::Fit);
    assert!(!w.status.recording);
}

#[test]
fn transfer_and_error_events_are_scoped_to_their_record() {
    let mut w = Workspace::new();
    w.accept_event(CameraEvent::Transfer {
        record_id: "first".into(),
        name: "a.jpg".into(),
        fraction: Some(0.4),
    });
    w.accept_event(CameraEvent::Error {
        error: CameraError::new(CameraErrorKind::Storage, "disk write failed"),
        operation: CameraOperation::Downloading,
        record_id: Some("second".into()),
    });
    assert_eq!(w.transfers["first"].fraction, Some(0.4));
    assert!(
        w.issues
            .iter()
            .any(|i| i.target == IssueTarget::Record("second".into()))
    );
    assert!(!w.issues.iter().any(|i| i.target == IssueTarget::Device));
}

#[test]
fn one_click_geometry_allows_disconnected_camera_and_jpeg_with_auto_exposure() {
    let mut workspace = Workspace::new();
    assert!(workspace.screen_capture_issue(true).is_none());
    assert!(workspace.screen_capture_issue(false).is_some());
    workspace.status.info = Some(CameraInfo {
        parameters: vec![CameraParameter {
            id: "imageformat".into(),
            label: "Photo format".into(),
            value: ParameterValue::Choice("L".into()),
            kind: ParameterKind::Choice(vec!["L".into(), "RAW".into()]),
            read_only: false,
        }],
        ..Default::default()
    });
    assert!(workspace.screen_capture_issue(true).is_none());
    assert!(
        workspace
            .screen_capture_issue(false)
            .unwrap()
            .contains("manual exposure")
    );
    workspace.status.recording = true;
    assert!(workspace.screen_capture_issue(true).is_some());
    workspace.status.recording = false;
    workspace.status.info.as_mut().unwrap().parameters[0].read_only = true;
    assert!(
        workspace
            .screen_capture_issue(true)
            .unwrap()
            .contains("RAW")
    );
}

#[test]
fn normalized_geometry_preview_does_not_inherit_live_view_rotation() {
    let mut workspace = Workspace::new();
    workspace.screen_test_open = true;
    workspace.live_view.turns = 1;
    let mut panel =
        crate::screen_test::ui::Panel::preview(crate::screen_test::model::Workflow::Geometry);
    panel.controller.preview = Some(PathBuf::from("/nonexistent/geometry-preview.png"));
    workspace.screen_test = Some(panel);
    workspace.poll(&egui::Context::default());
    assert_eq!(workspace.photo_view.turns, 0);
    assert_eq!(
        workspace.source,
        CanvasSource::Photo("/nonexistent/geometry-preview.png".into())
    );
}

#[test]
fn canvas_calibration_waits_for_restore_and_keeps_live_view() {
    let mut workspace = Workspace::new();
    workspace.calibration_running = true;
    workspace.status.photo_format_restore_pending = true;
    let mut panel =
        crate::screen_test::ui::Panel::preview(crate::screen_test::model::Workflow::Reference);
    panel.controller.message = "Calibration complete".into();
    panel.controller.preview = Some("/nonexistent/correction.png".into());
    workspace.screen_test = Some(panel);
    workspace.finish_calibration();
    assert!(workspace.calibration_running);
    assert!(workspace.calibration_result.is_none());
    assert!(!workspace.controls_idle());
    workspace.status.photo_format_restore_pending = false;
    workspace.poll(&egui::Context::default());
    assert!(!workspace.calibration_running);
    assert!(workspace.calibration_result.as_ref().unwrap().is_ok());
    assert_eq!(workspace.source, CanvasSource::Live);
    assert!(!workspace.screen_test_open);
    assert!(workspace.controls_idle());
}

#[test]
fn failed_calibration_does_not_report_prior_geometry_as_success() {
    let mut workspace = Workspace::new();
    workspace.calibration_running = true;
    let mut panel =
        crate::screen_test::ui::Panel::preview(crate::screen_test::model::Workflow::Reference);
    panel.controller.error = Some("Could not restore phone".into());
    workspace.screen_test = Some(panel);
    workspace.finish_calibration();
    assert_eq!(
        workspace.calibration_result,
        Some(Err("Could not restore phone".into()))
    );
}
