use super::*;

impl Workspace {
    pub(super) fn record_state(&self, record: &CaptureRecord) -> String {
        if let Some(issue) = self
            .issues
            .iter()
            .find(|issue| issue.target == IssueTarget::Record(record.id.clone()))
        {
            return issue.summary.clone();
        }
        if self.status.active_record_id.as_ref() == Some(&record.id) {
            match self.status.operation {
                CameraOperation::Capturing => return "Taking photo…".into(),
                CameraOperation::WaitingFiles => return "Waiting for files…".into(),
                CameraOperation::StartingRecording => return "Starting recording…".into(),
                CameraOperation::Recording => return "Recording…".into(),
                CameraOperation::StoppingRecording => return "Stopping recording…".into(),
                CameraOperation::Recovering => return "Retrying download…".into(),
                CameraOperation::Downloading => {
                    return self
                        .transfers
                        .get(&record.id)
                        .and_then(|t| t.fraction)
                        .map(|v| format!("Downloading {:.0}%", v * 100.))
                        .unwrap_or("Downloading…".into());
                }
                _ => {}
            }
        }
        if !record.pending_files.is_empty() {
            "Transfer needed".into()
        } else if record.files.is_empty() {
            "No files received".into()
        } else {
            match self.results.state(&record.id) {
                Some(results::ResultState::Pending) => "Correcting…",
                Some(results::ResultState::Saved(_)) => "Saved · corrected",
                Some(results::ResultState::Failed(_)) => "Correction failed",
                Some(results::ResultState::Expired) => "Original · calibration expired",
                None => "Saved",
            }
            .into()
        }
    }
    pub(super) fn gallery(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(tokens::rich_text(
                format!("Captures · {}", self.records.len()),
                TextRole::SectionTitle,
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if action(ui, if self.strip_open { "Hide" } else { "Show" }, true) {
                    self.strip_open = !self.strip_open;
                }
                if action(ui, "Import…", true) {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Photographs", &["jpg", "jpeg", "png", "cr2"])
                        .pick_file()
                    {
                        self.open_photo(path);
                    }
                }
                if action(ui, "Latest", !self.records.is_empty()) {
                    self.follow_latest = true;
                    self.scroll_latest = true;
                }
            });
        });
        if !self.strip_open {
            return;
        }
        if self.records.is_empty() {
            muted(
                ui,
                "Your captures will appear here. Original files are saved to your output folder.",
            );
            return;
        }
        let mut selected = None;
        let mut detail = None;
        let mut retry = None;
        let mut folder = None;
        let pending_scroll = self.scroll_latest;
        let output = egui::ScrollArea::horizontal()
            .id_salt("camera.thumbnails")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 8.;
                ui.horizontal(|ui| {
                    for (index, record) in self.records.iter().enumerate() {
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(132., 68.), egui::Sense::click());
                        if pending_scroll && index + 1 == self.records.len() {
                            response.scroll_to_me(Some(egui::Align::RIGHT));
                        }
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let active = self.selected_record.as_ref() == Some(&record.id);
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::Button,
                                true,
                                active,
                                record
                                    .files
                                    .first()
                                    .map(|f| f.remote.name.clone())
                                    .unwrap_or_else(|| "Capture without files".into()),
                            )
                        });
                        let painter = ui.painter();
                        painter.rect_filled(
                            rect,
                            4,
                            if active {
                                Color32::from_gray(48)
                            } else {
                                Color32::from_gray(30)
                            },
                        );
                        painter.rect_stroke(
                            rect,
                            4,
                            egui::Stroke::new(
                                1.0_f32,
                                if active {
                                    Color32::from_gray(160)
                                } else {
                                    Color32::from_gray(48)
                                },
                            ),
                            egui::StrokeKind::Inside,
                        );
                        let photo = record
                            .files
                            .iter()
                            .find(|f| {
                                matches!(f.remote.extension().as_str(), "jpg" | "jpeg" | "png")
                            })
                            .or_else(|| {
                                record.files.iter().find(|f| f.remote.extension() == "cr2")
                            });
                        let image_rect = egui::Rect::from_min_max(
                            rect.min + egui::vec2(3., 3.),
                            egui::pos2(rect.max.x - 3., rect.min.y + 41.),
                        );
                        let saved = self
                            .results
                            .saved(&record.id)
                            .filter(|_| self.geometry_enabled);
                        let preview = saved.map(|s| &s.path).or_else(|| photo.map(|f| &f.path));
                        if let Some((texture, orientation, original_size)) =
                            preview.and_then(|path| self.thumbnails.get(path))
                        {
                            let correction = photo
                                .filter(|_| saved.is_none())
                                .and_then(|f| self.correction_for_path(&f.path))
                                .filter(|_| self.geometry_enabled);
                            let size = correction
                                .as_ref()
                                .map(|g| {
                                    egui::vec2(g.screen_size[0] as f32, g.screen_size[1] as f32)
                                })
                                .unwrap_or_else(|| texture.size_vec2());
                            let scale =
                                (image_rect.width() / size.x).min(image_rect.height() / size.y);
                            let rect =
                                egui::Rect::from_center_size(image_rect.center(), size * scale);
                            if let Some(g) = correction {
                                let mesh = geometry::unit_mesh(&g, original_size, orientation, 24);
                                painter.add(egui::Shape::mesh(geometry::place_mesh(
                                    mesh,
                                    texture.id(),
                                    rect,
                                    0,
                                )));
                            } else {
                                painter.image(
                                    texture.id(),
                                    rect,
                                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                                    Color32::WHITE,
                                );
                            }
                        } else {
                            painter.text(
                                image_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                if record.kind == CaptureKind::Movie {
                                    "MOV"
                                } else if record.files.is_empty() {
                                    "···"
                                } else if photo.is_some() {
                                    "JPG"
                                } else {
                                    "RAW"
                                },
                                tokens::font_id(13., tokens::FontWeight::Medium),
                                MUTED,
                            );
                        }
                        let name = record
                            .files
                            .first()
                            .map(|file| file.remote.name.as_str())
                            .unwrap_or(if record.kind == CaptureKind::Movie {
                                "Video capture"
                            } else {
                                "Photo capture"
                            });
                        let label_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.min.x + 5., rect.min.y + 42.),
                            egui::pos2(rect.max.x - 5., rect.min.y + 55.),
                        );
                        let text = painter.layout(
                            name.into(),
                            tokens::font_id(11., tokens::FontWeight::Normal),
                            Color32::from_gray(215),
                            label_rect.width(),
                        );
                        painter.with_clip_rect(label_rect).galley(
                            label_rect.min,
                            text,
                            Color32::WHITE,
                        );
                        let state = self.record_state(record);
                        let formats = record
                            .files
                            .iter()
                            .map(|f| f.remote.extension().to_uppercase())
                            .collect::<std::collections::BTreeSet<_>>()
                            .into_iter()
                            .collect::<Vec<_>>()
                            .join("+");
                        let caption = if self
                            .issues
                            .iter()
                            .any(|issue| issue.target == IssueTarget::Record(record.id.clone()))
                        {
                            if record.pending_files.is_empty() {
                                "Capture issue".into()
                            } else {
                                "Transfer failed".into()
                            }
                        } else if state == "Saved" {
                            if record.kind == CaptureKind::Movie {
                                format!("MOV · {} segments", record.files.len())
                            } else {
                                formats
                            }
                        } else {
                            state.clone()
                        };
                        let caption_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.min.x + 5., rect.min.y + 55.),
                            rect.max - egui::vec2(4., 1.),
                        );
                        painter.with_clip_rect(caption_rect).text(
                            caption_rect.min,
                            egui::Align2::LEFT_TOP,
                            caption,
                            tokens::font_id(9., tokens::FontWeight::Normal),
                            if self
                                .issues
                                .iter()
                                .any(|i| i.target == IssueTarget::Record(record.id.clone()))
                            {
                                ERROR
                            } else {
                                MUTED
                            },
                        );
                        if self.status.active_record_id.as_ref() == Some(&record.id)
                            && self.status.operation == CameraOperation::Downloading
                        {
                            if let Some(fraction) =
                                self.transfers.get(&record.id).and_then(|t| t.fraction)
                            {
                                painter.rect_filled(
                                    egui::Rect::from_min_size(
                                        egui::pos2(rect.min.x, rect.max.y - 2.),
                                        egui::vec2(rect.width() * fraction, 2.),
                                    ),
                                    0,
                                    Color32::from_gray(210),
                                );
                            }
                        }
                        let response = response.on_hover_text(format!(
                            "{name}\n{state}\nClick to review · right-click for files and details"
                        ));
                        if response.clicked() {
                            selected = Some(record.id.clone());
                        }
                        if response.double_clicked() {
                            detail = Some(record.id.clone());
                        }
                        response.context_menu(|ui| {
                            if action(ui, "Capture details…", true) {
                                detail = Some(record.id.clone());
                                ui.close();
                            }
                            if action(ui, "Open folder", true) {
                                folder = Some(record.directory.clone());
                                ui.close();
                            }
                            if !record.pending_files.is_empty()
                                && action(
                                    ui,
                                    "Retry download",
                                    self.controls_idle()
                                        && !self.status.recording
                                        && self.status.info.is_some(),
                                )
                            {
                                retry = Some(record.id.clone());
                                ui.close();
                            }
                        });
                    }
                });
            });
        self.scroll_latest = false;
        if ui.rect_contains_pointer(output.inner_rect)
            && ui.input(|i| i.smooth_scroll_delta != egui::Vec2::ZERO || i.pointer.any_down())
        {
            self.follow_latest = false;
        }
        if let Some(id) = selected {
            self.selected_record = Some(id.clone());
            self.follow_latest = false;
            if let Some(record) = self.records.iter().find(|r| r.id == id) {
                if let Some(saved) = self
                    .results
                    .saved(&record.id)
                    .filter(|_| self.geometry_enabled)
                {
                    self.open_photo(saved.path.clone());
                } else if let Some(file) = record
                    .files
                    .iter()
                    .find(|f| matches!(f.remote.extension().as_str(), "jpg" | "jpeg" | "png"))
                    .or_else(|| record.files.iter().find(|f| f.remote.extension() == "cr2"))
                {
                    self.open_photo(file.path.clone());
                } else {
                    self.photos.cancel();
                    self.source = CanvasSource::Record(id);
                }
            }
        }
        if let Some(id) = detail {
            self.detail_record = Some(id);
        }
        if let Some(record_id) = retry {
            self.send(CameraCommand::RetryDownloads { record_id });
        }
        if let Some(path) = folder {
            if let Err(e) = reveal(&path) {
                self.issue(IssueTarget::Output, "Could not open folder", e.to_string());
            }
        }
    }
    pub(super) fn canvas(&mut self, ui: &mut egui::Ui) {
        let available_geometry = self.canvas_geometry();
        let has_saved_result = matches!(&self.source, CanvasSource::Photo(path) if self.results.for_path(path).is_some());
        egui::Frame::new()
            .fill(PANEL)
            .inner_margin(egui::Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.source == CanvasSource::Live {
                        let text = if self.status.info.is_none() {
                            "Camera disconnected"
                        } else if !self.status.preview {
                            "Preview paused"
                        } else if self.fixture.is_none()
                            && self
                                .live
                                .as_ref()
                                .is_some_and(|v| v.received.elapsed() > Duration::from_secs(2))
                        {
                            "Waiting for frames…"
                        } else {
                            "Live"
                        };
                        ui.label(tokens::rich_text(text, TextRole::SectionTitle));
                    } else if action(ui, "← Live", true) {
                        self.return_live();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if action(
                            ui,
                            if self.calibration_running {
                                "Calibrating…"
                            } else {
                                "Calibrate"
                            },
                            !self.busy_capture() && self.screen_capture_issue(true).is_none(),
                        ) {
                            self.start_calibration();
                        }
                        if (available_geometry.is_some() || has_saved_result)
                            && action(
                                ui,
                                if self.geometry_enabled {
                                    "Corrected"
                                } else {
                                    "Original"
                                },
                                true,
                            )
                        {
                            self.toggle_correction();
                        }
                        if self.source == CanvasSource::Live {
                            if action(
                                ui,
                                if self.status.preview {
                                    "Pause"
                                } else {
                                    "Start preview"
                                },
                                self.status.info.is_some()
                                    && self.controls_idle()
                                    && !self.status.recording,
                            ) {
                                self.send(CameraCommand::Preview(!self.status.preview));
                            }
                        } else if action(ui, "Details", self.selected_record.is_some()) {
                            self.detail_record = self.selected_record.clone();
                        }
                        let view = if self.source == CanvasSource::Live {
                            &mut self.live_view
                        } else {
                            &mut self.photo_view
                        };
                        if action(ui, "Rotate", true) {
                            view.rotate();
                        }
                        if action(ui, "100%", true) {
                            view.original();
                        }
                        if action(ui, "Fit", true) {
                            view.fit();
                        }
                        if let ScaleMode::Physical(scale) = view.scale {
                            muted(ui, format!("{:.0}%", scale * 100.));
                        }
                    });
                });
            });
        if self.calibration_running {
            ui.horizontal(|ui| {
                ui.spinner();
                let message = self
                    .screen_test
                    .as_ref()
                    .map(|p| p.controller.message.as_str())
                    .unwrap_or("Preparing calibration…");
                let message = match self.status.operation {
                    CameraOperation::Capturing => "Taking calibration photo…",
                    CameraOperation::WaitingFiles => "Waiting for RAW…",
                    CameraOperation::Downloading => "Downloading RAW…",
                    _ if self.status.photo_format_restore_pending => "Restoring camera settings…",
                    _ => message,
                };
                muted(ui, message);
            });
        } else if let Some(result) = &self.calibration_result {
            match result {
                Ok(message) => {
                    muted(ui, message);
                }
                Err(error) => {
                    ui.colored_label(
                        Color32::from_rgb(244, 135, 125),
                        "Calibration failed · click Calibrate to retry",
                    )
                    .on_hover_text(error);
                }
            }
        }
        self.error_panel(ui, IssueTarget::Preview);
        if self.source == CanvasSource::Live
            && self.issues.iter().any(|i| i.target == IssueTarget::Preview)
            && action(
                ui,
                "Retry preview",
                self.status.info.is_some() && self.controls_idle(),
            )
        {
            self.send(CameraCommand::Preview(true));
        }
        match self.source.clone() {
            CanvasSource::Live => {
                if let Some(view) = &self.live {
                    self.live_view.draw(
                        ui,
                        view,
                        available_geometry
                            .as_deref()
                            .filter(|_| self.geometry_enabled),
                    );
                } else {
                    self.canvas_empty(
                        ui,
                        if self.status.info.is_none() {
                            "Connect your camera"
                        } else if self.status.operation == CameraOperation::StartingPreview {
                            "Starting Live View…"
                        } else {
                            "Live View is paused"
                        },
                        "Exposure, focus and capture controls are on the left.",
                    );
                }
            }
            CanvasSource::Photo(path) => {
                if let Some(view) = &self.photo {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(16, 0))
                        .show(ui, |ui| {
                            muted(
                                ui,
                                format!("{} · {} × {}", view.label, view.size[0], view.size[1]),
                            );
                        });
                    self.photo_view.draw(
                        ui,
                        view,
                        available_geometry
                            .as_deref()
                            .filter(|_| self.geometry_enabled),
                    );
                } else {
                    self.canvas_empty(
                        ui,
                        if self.photos.requested.is_some() {
                            "Loading photograph…"
                        } else {
                            "Photograph unavailable"
                        },
                        &path.file_name().unwrap_or_default().to_string_lossy(),
                    );
                }
            }
            CanvasSource::Record(id) => {
                if let Some(record) = self.records.iter().find(|r| r.id == id).cloned() {
                    self.canvas_empty(
                        ui,
                        if record.kind == CaptureKind::Movie {
                            "Video capture"
                        } else {
                            "Original capture"
                        },
                        &self.record_state(&record),
                    );
                    ui.add_space(12.);
                    ui.vertical_centered(|ui| {
                        if action(ui, "Files & capture details", true) {
                            self.detail_record = Some(id.clone());
                        }
                        if record.kind == CaptureKind::Movie {
                            if let Some(file) = record.files.first() {
                                if action(ui, "Open video", true) {
                                    if let Err(e) = reveal(&file.path) {
                                        self.issue(
                                            IssueTarget::Preview,
                                            "Could not open video",
                                            e.to_string(),
                                        );
                                    }
                                }
                            }
                        }
                        if !record.pending_files.is_empty()
                            && action(
                                ui,
                                "Retry download",
                                self.status.info.is_some()
                                    && self.controls_idle()
                                    && !self.status.recording,
                            )
                        {
                            self.send(CameraCommand::RetryDownloads {
                                record_id: record.id,
                            });
                        }
                    });
                }
            }
        }
    }
    fn canvas_empty(&self, ui: &mut egui::Ui, title: &str, message: &str) {
        ui.add_space((ui.available_height() * 0.38).max(0.));
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(title)
                    .size(18.)
                    .color(Color32::from_gray(205)),
            );
            ui.add_space(4.);
            muted(ui, message);
        });
    }
    pub(super) fn capture_details(&mut self, ctx: &egui::Context) {
        let Some(record) = self
            .detail_record
            .as_ref()
            .and_then(|id| self.records.iter().find(|r| &r.id == id))
            .cloned()
        else {
            return;
        };
        let mut open = true;
        egui::Window::new("Capture details")
            .open(&mut open)
            .default_width(480.)
            .show(ctx, |ui| {
                ui.label(tokens::rich_text(
                    format!(
                        "{} · {}",
                        if record.kind == CaptureKind::Movie {
                            "Video"
                        } else {
                            "Photo"
                        },
                        self.record_state(&record)
                    ),
                    TextRole::SectionTitle,
                ));
                if let Some(transfer) = self.transfers.get(&record.id) {
                    if record.pending_files.len() > 0 {
                        muted(ui, &transfer.name);
                    }
                }
                self.error_panel(ui, IssueTarget::Record(record.id.clone()));
                match self.results.state(&record.id).cloned() {
                    Some(results::ResultState::Saved(saved)) => {
                        ui.horizontal(|ui| {
                            ui.label(format!("Corrected · {} × {}", saved.size[0], saved.size[1]));
                            if action(ui, "Open preview", true) {
                                if let Err(e) = reveal(&saved.path) {
                                    self.issue(IssueTarget::Record(record.id.clone()), "Could not open preview", e.to_string());
                                }
                            }
                            if let Some(raw_path) = &saved.raw_path {
                                if action(ui, "Open corrected RAW", true) {
                                    if let Err(e) = reveal(raw_path) {
                                        self.issue(IssueTarget::Record(record.id.clone()), "Could not open corrected RAW", e.to_string());
                                    }
                                }
                            }
                        });
                        if saved.raw_path.is_some() {
                            muted(ui, "Saved results/rectified.dng (16-bit Bayer RAW) and a PNG preview. The original CR2 is unchanged.");
                        } else {
                            muted(ui, "Saved separately in results/rectified.png.");
                        }
                    }
                    Some(results::ResultState::Pending) => { muted(ui, "Saving corrected result…"); }
                    Some(results::ResultState::Failed(error)) => {
                        ui.colored_label(ERROR, format!("Could not save corrected result: {error}"));
                        if action(ui, "Retry correction", true) { self.results.retry(&record.id); }
                    }
                    Some(results::ResultState::Expired) => {
                        muted(ui, "No corrected file was saved in this capture's calibration session. Only the original is available.");
                    }
                    None => {}
                }
                for file in &record.files {
                    ui.horizontal(|ui| {
                        ui.label(&file.remote.name);
                        muted(ui, format!("{:.1} MB", file.bytes as f64 / 1_000_000.));
                        if action(ui, "Open original", true) {
                            if let Err(e) = reveal(&file.path) {
                                self.issue(
                                    IssueTarget::Record(record.id.clone()),
                                    "Could not open file",
                                    e.to_string(),
                                );
                            }
                        }
                    });
                }
                for pending in &record.pending_files {
                    muted(ui, format!("Pending · {}", pending.name));
                }
                if !record.pending_files.is_empty()
                    && action(
                        ui,
                        "Retry download",
                        self.status.info.is_some()
                            && self.controls_idle()
                            && !self.status.recording,
                    )
                {
                    self.send(CameraCommand::RetryDownloads {
                        record_id: record.id.clone(),
                    });
                }
                ui.separator();
                for id in [
                    "iso",
                    "shutterspeed",
                    "aperture",
                    "whitebalance",
                    "lensname",
                ] {
                    if let Some(p) = record.camera.parameters.iter().find(|p| p.id == id) {
                        ui.horizontal(|ui| {
                            muted(ui, &p.label);
                            ui.label(display_value(&p.value));
                        });
                    }
                }
                muted(ui, record.directory.display().to_string());
                if action(ui, "Open folder", true) {
                    if let Err(e) = reveal(&record.directory) {
                        self.issue(IssueTarget::Output, "Could not open folder", e.to_string());
                    }
                }
            });
        if !open {
            self.detail_record = None;
        }
    }
}
