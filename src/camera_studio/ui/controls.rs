use super::*;

impl Workspace {
    pub(super) fn device(&mut self, ui: &mut egui::Ui) {
        let name = self
            .status
            .info
            .as_ref()
            .map(|info| info.model.as_str())
            .unwrap_or("Camera Studio")
            .to_owned();
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("•••", |ui| {
                    ui.set_min_width(200.);
                    if action(ui, "Diagnostics…", true) {
                        self.diagnostics_open = true;
                        ui.close();
                    }
                    if action(
                        ui,
                        "Refresh camera settings",
                        self.status.info.is_some() && self.controls_idle(),
                    ) {
                        self.send(CameraCommand::Refresh);
                        ui.close();
                    }
                    ui.separator();
                    if action(
                        ui,
                        "Disconnect camera",
                        self.status.info.is_some() && !self.busy_capture(),
                    ) {
                        self.send(CameraCommand::Disconnect);
                        ui.close();
                    }
                });
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(tokens::rich_text(name, TextRole::SectionTitle))
                            .truncate(),
                    );
                });
            });
        });
        if self.fixture.is_some() {
            self.fixture_menu(ui);
        }
        let label = if self.status.operation == CameraOperation::Connecting {
            "Connecting…"
        } else if self.status.info.is_some() {
            "Connected"
        } else {
            "Not connected"
        };
        ui.horizontal(|ui| {
            ui.colored_label(
                if self.status.info.is_some() {
                    Color32::from_rgb(130, 180, 147)
                } else {
                    MUTED
                },
                "●",
            );
            muted(ui, label);
            if let Some(value) = self.parameter_value("batterylevel") {
                muted(ui, format!("· {value}%"));
            }
            if let Some(mode) = self.parameter_value("autoexposuremode") {
                muted(ui, format!("· {mode}"));
            }
        });
        if self.status.info.is_none()
            && wide_action(
                ui,
                if self.status.operation == CameraOperation::Connecting {
                    "Connecting…"
                } else {
                    "Connect camera"
                },
                self.controls_idle(),
                false,
                false,
            )
        {
            self.send(CameraCommand::Connect {
                directory: self.directory.clone(),
                release_occupied: self.release_occupied,
            });
        }
        self.error_panel(ui, IssueTarget::Device);
    }
    fn parameter_value(&self, id: &str) -> Option<String> {
        self.status
            .info
            .as_ref()?
            .parameters
            .iter()
            .find(|p| p.id == id)
            .map(|p| display_value(&p.value))
    }
    pub(super) fn shooting(&mut self, ui: &mut egui::Ui) {
        let recording = self.status.recording;
        let mode_width = (ui.available_width() - 134.).max(110.);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(mode_width, 28.),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.add_enabled_ui(!self.busy_capture(), |ui| {
                        radio_button_group(
                            ui,
                            "camera.mode",
                            &mut self.shooting_mode,
                            &[
                                RadioButtonOption {
                                    value: ShootingMode::Photo,
                                    label: "Photo",
                                },
                                RadioButtonOption {
                                    value: ShootingMode::Video,
                                    label: "Video",
                                },
                            ],
                        );
                    });
                },
            );
            let label = if recording {
                if self.status.operation == CameraOperation::StoppingRecording {
                    "Stopping…"
                } else {
                    "Stop recording"
                }
            } else {
                match self.status.operation {
                    CameraOperation::Capturing => "Taking photo…",
                    CameraOperation::StartingRecording => "Starting…",
                    CameraOperation::WaitingFiles => "Waiting files…",
                    CameraOperation::Downloading => "Downloading…",
                    _ => {
                        if self.shooting_mode == ShootingMode::Photo {
                            "Take photo"
                        } else {
                            "Record video"
                        }
                    }
                }
            };
            let enabled = if recording {
                self.controls_idle()
            } else {
                self.can_capture()
                    && (self.shooting_mode == ShootingMode::Photo
                        || self
                            .status
                            .info
                            .as_ref()
                            .is_some_and(CameraInfo::can_start_recording))
            };
            if button_with_width(
                ui,
                ButtonOptions {
                    enabled,
                    variant: if recording {
                        ButtonVariant::Destructive
                    } else {
                        ButtonVariant::Secondary
                    },
                    ..ButtonOptions::new(label)
                },
                126.,
            )
            .clicked()
            {
                if recording {
                    self.send(CameraCommand::StopRecording);
                } else if self.shooting_mode == ShootingMode::Photo {
                    self.shoot();
                } else {
                    self.send(CameraCommand::StartRecording {
                        directory: self.directory.clone(),
                        context: self.context(),
                    });
                }
            }
        });
        if recording {
            let seconds = self
                .status
                .recording_started
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(0);
            ui.horizontal(|ui| {
                ui.colored_label(
                    RED,
                    format!("● REC  {:02}:{:02}", seconds / 60, seconds % 60),
                );
                if self.status.operation == CameraOperation::StoppingRecording {
                    muted(ui, "Waiting for camera");
                }
            });
        }
        if !recording && self.shooting_mode == ShootingMode::Video {
            if !self
                .status
                .info
                .as_ref()
                .is_some_and(CameraInfo::can_start_recording)
            {
                muted(ui, "Set the camera switch to Movie and insert an SD card.");
            } else {
                muted(ui, "Records MOV to card · downloads after stop");
            }
        }
        if self.status.pending_downloads > 0
            && !self.status.recording
            && !self.status.operation.is_busy()
        {
            muted(
                ui,
                format!("{} files need transfer", self.status.pending_downloads),
            );
            if action(ui, "Retry pending downloads", self.status.info.is_some()) {
                self.send(CameraCommand::RecoverDownloads);
            }
        }
        self.error_panel(ui, IssueTarget::Shooting);
    }
    pub(super) fn settings(&mut self, ui: &mut egui::Ui) {
        section::section(ui, "Exposure", |ui| {
            self.parameter_pair(ui, ("shutterspeed", "Shutter"), ("aperture", "Aperture"));
            self.parameter_pair(ui, ("iso", "ISO"), ("exposurecompensation", "Compensation"));
            self.parameter(ui, "whitebalance", "White balance");
            self.error_panel(ui, IssueTarget::Exposure);
        });
        divider(ui);
        section::section(ui, "Focus", |ui| {
            let enabled = self.status.preview && self.controls_idle() && self.status.info.is_some();
            ui.add_enabled_ui(enabled, |ui| {
                radio_button_group(
                    ui,
                    "camera.focus.step",
                    &mut self.focus_step,
                    &[
                        RadioButtonOption {
                            value: 1,
                            label: "Fine",
                        },
                        RadioButtonOption {
                            value: 2,
                            label: "Medium",
                        },
                        RadioButtonOption {
                            value: 3,
                            label: "Coarse",
                        },
                    ],
                );
                let width = (ui.available_width() - 8.) * 0.5;
                ui.horizontal(|ui| {
                    for (near, label) in [(true, "←  Near"), (false, "Far  →")] {
                        let available = self.status.info.as_ref().is_some_and(|info| {
                            info.focus_preparation(near, self.focus_step).is_ok()
                        });
                        if button_with_width(
                            ui,
                            ButtonOptions {
                                enabled: enabled && available,
                                variant: ButtonVariant::Secondary,
                                ..ButtonOptions::new(label)
                            },
                            width,
                        )
                        .clicked()
                        {
                            self.send(CameraCommand::Focus {
                                near,
                                step: self.focus_step,
                            });
                        }
                    }
                });
            });
            if !self.status.preview {
                muted(ui, "Start Live View to adjust focus.");
            } else if let Some(info) = &self.status.info {
                match info.focus_preparation(true, self.focus_step) {
                    Err(error) => {
                        muted(ui, error.message);
                    }
                    Ok(changes) if !changes.is_empty() => {
                        muted(ui, "Continuous AF will be turned off before the next step.");
                    }
                    Ok(_) => {
                        muted(ui, "Lens switch: AF · one click = one step");
                    }
                }
            }
            if self.status.operation == CameraOperation::Focusing {
                muted(ui, "Adjusting focus…");
            } else if let Some((message, when)) = &self.focus_feedback {
                if when.elapsed() < Duration::from_secs(5) {
                    muted(ui, message);
                }
            }
            self.error_panel(ui, IssueTarget::Focus);
        });
        divider(ui);
        section::section(ui, "Output", |ui| {
            self.parameter(ui, "imageformat", "Photo format");
            let directory = self.directory.clone();
            section::cell(ui, "Save to", |ui| {
                ui.menu_button(
                    directory
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    |ui| {
                        if action(ui, "Open folder", true) {
                            if let Err(e) = reveal(&directory) {
                                self.issue(
                                    IssueTarget::Output,
                                    "Could not open folder",
                                    e.to_string(),
                                );
                            }
                            ui.close();
                        }
                        if action(ui, "Choose folder…", !self.busy_capture()) {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_directory(&directory)
                                .pick_folder()
                            {
                                self.directory = path;
                                self.records.clear();
                                self.selected_record = None;
                                self.detail_record = None;
                                self.load_history();
                            }
                            ui.close();
                        }
                    },
                )
                .response
                .on_hover_text(directory.display().to_string());
            });
            self.error_panel(ui, IssueTarget::Output);
        });
        divider(ui);
        ui.collapsing(
            tokens::rich_text("Advanced", TextRole::SectionTitle),
            |ui| {
                ui.collapsing("Image", |ui| {
                    for (id, label) in [
                        ("imageformatsd", "SD image format"),
                        ("picturestyle", "Picture style"),
                        ("colorspace", "Color space"),
                        ("aspectratio", "Aspect ratio"),
                        ("autoexposuremode", "Exposure mode"),
                    ] {
                        self.parameter(ui, id, label);
                    }
                });
                ui.collapsing("Autofocus", |ui| {
                    for (id, label) in [
                        ("continuousaf", "Continuous AF"),
                        ("afmethod", "AF method"),
                        ("focusmode", "Focus mode"),
                        ("movieservoaf", "Movie servo AF"),
                    ] {
                        self.parameter(ui, id, label);
                    }
                });
                ui.collapsing("Capture context", |ui| {
                    muted(ui, "Test pattern ID");
                    ui.text_edit_singleline(&mut self.pattern_id);
                    muted(
                        ui,
                        "Stored with capture metadata; originals remain unchanged.",
                    );
                });
            },
        );
    }
    fn parameter(&mut self, ui: &mut egui::Ui, id: &str, label: &str) {
        let parameter = self
            .status
            .info
            .as_ref()
            .and_then(|info| info.parameters.iter().find(|p| p.id == id))
            .cloned();
        section::cell(ui, label, |ui| {
            let Some(p) = parameter else {
                ui.add_enabled(
                    false,
                    egui::Button::new("—").min_size(egui::vec2(ui.available_width(), 28.)),
                );
                return;
            };
            let mut value = p.value.clone();
            let enabled = !p.read_only && self.status.info.is_some() && !self.busy_capture();
            ui.push_id(id, |ui| {
                ui.add_enabled_ui(enabled, |ui| match (&p.kind, &mut value) {
                    (ParameterKind::Choice(choices), ParameterValue::Choice(current)) => {
                        egui::ComboBox::from_id_salt("choice")
                            .selected_text(current.as_str())
                            .width(ui.available_width())
                            .height(240.)
                            .show_ui(ui, |ui| {
                                for choice in choices {
                                    ui.selectable_value(current, choice.clone(), choice);
                                }
                            });
                    }
                    (ParameterKind::Toggle, ParameterValue::Toggle(current)) => {
                        radio_button_group(
                            ui,
                            "toggle",
                            current,
                            &[
                                RadioButtonOption {
                                    value: false,
                                    label: "Off",
                                },
                                RadioButtonOption {
                                    value: true,
                                    label: "On",
                                },
                            ],
                        );
                    }
                    (ParameterKind::Range { min, max, step }, ParameterValue::Number(current)) => {
                        ui.add(
                            egui::Slider::new(current, *min..=*max)
                                .step_by(f64::from(*step).max(0.001)),
                        );
                    }
                    _ => {
                        ui.label(display_value(&value));
                    }
                })
                .response
                .on_hover_text(if p.read_only {
                    "Read-only in the current camera mode"
                } else {
                    &p.label
                });
            });
            if value != p.value {
                self.send(CameraCommand::SetParameter { id: p.id, value });
            }
        });
    }
    fn parameter_pair(&mut self, ui: &mut egui::Ui, left: (&str, &str), right: (&str, &str)) {
        let width = (ui.available_width() - 8.) * 0.5;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(width, 0.),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.parameter(ui, left.0, left.1),
            );
            ui.allocate_ui_with_layout(
                egui::vec2(width, 0.),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.parameter(ui, right.0, right.1),
            );
        });
    }
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        if self.diagnostics_open {
            let mut open = true;
            egui::Window::new("Camera diagnostics").open(&mut open).default_width(520.).frame(tool_window::dialog_frame(ctx)).show(ctx, |ui| {
                tool_window::apply(ui);
                ui.add_enabled_ui(!self.busy_capture(), |ui| { ui.checkbox(&mut self.release_occupied, "Release other camera apps if USB is occupied"); });
                muted(ui, "Known camera clients are stopped only after an occupied connection attempt.");
                if let Some(info) = &self.status.info { ui.label(format!("{} · {}", info.model, info.port)); ui.collapsing("Camera details", |ui| { ui.label(&info.summary); }); }
                if let Some(view) = &self.live { ui.label(format!("{} × {} · {:.1} fps · frame {} · {:.0} ms since reception", view.size[0], view.size[1], self.status.preview_fps, view.sequence, view.received.elapsed().as_secs_f64()*1000.)); }
                ui.separator();
                egui::ScrollArea::vertical().max_height(280.).show(ui, |ui| { for message in &self.diagnostics { ui.label(message); ui.separator(); } });
                if action(ui,"Copy diagnostics",true) { ctx.copy_text(self.diagnostics.iter().cloned().collect::<Vec<_>>().join("\n")); }
            });
            self.diagnostics_open = open;
        }
        if let Some(detail) = self.error_detail.clone() {
            let mut open = true;
            egui::Window::new("Error details")
                .open(&mut open)
                .default_width(480.)
                .max_height(560.)
                .vscroll(true)
                .frame(tool_window::dialog_frame(ctx)).show(ctx, |ui| {
                tool_window::apply(ui);
                    ui.label(&detail);
                    if action(ui, "Copy details", true) {
                        ctx.copy_text(detail.clone());
                    }
                });
            if !open {
                self.error_detail = None;
            }
        }
        self.capture_details(ctx);
        if self.close_dialog {
            egui::Window::new("Finish camera work before closing")
                .default_width(520.)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .frame(tool_window::dialog_frame(ctx)).show(ctx, |ui| {
                tool_window::apply(ui);
                    ui.label(
                        "Recording, downloads and corrected image saves must finish before closing.",
                    );
                    ui.horizontal_wrapped(|ui| {
                        if action(ui, "Stay open", true) {
                            self.close_dialog = false;
                            self.parent_to_close = None;
                        }
                        if action(ui, "Finish & close", true) {
                            self.close_dialog = false;
                            self.close_after_work = true;
                            if self.status.recording {
                                self.send(CameraCommand::StopRecording);
                            } else if self.status.pending_downloads > 0 {
                                self.send(CameraCommand::RecoverDownloads);
                            }
                        }
                        if action(
                            ui,
                            "Close; keep pending on card",
                            !self.status.recording
                                && !self.status.operation.is_busy()
                                && !self.calibration_running
                                && !self.results.busy()
                                && !self.status.photo_format_restore_pending,
                        ) {
                            self.close_dialog = false;
                            self.shutdown();
                        }
                    });
                });
        }
    }
}
