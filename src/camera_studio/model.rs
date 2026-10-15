use std::{collections::BTreeMap, path::PathBuf, time::Instant};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParameterValue {
    Choice(String),
    Toggle(bool),
    Number(f32),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParameterKind {
    Choice(Vec<String>),
    Toggle,
    Range { min: f32, max: f32, step: f32 },
    Text,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraParameter {
    pub id: String,
    pub label: String,
    pub value: ParameterValue,
    pub kind: ParameterKind,
    pub read_only: bool,
}

impl CameraParameter {
    pub fn allows_choice(&self, value: &str) -> bool {
        !self.read_only
            && matches!(&self.kind, ParameterKind::Choice(choices) if choices.iter().any(|choice| choice == value))
    }

    /// libgphoto2 Canon EOS EVFRecordStatus: None = 0, SDRAM = 3, Card = 4.
    /// SDRAM is also reported during still-photo Live View. Only Card confirms a MOV recording.
    pub fn recording_to_card(&self) -> Option<bool> {
        if self.id != "movierecordtarget" {
            return None;
        }
        match &self.value {
            ParameterValue::Choice(value) if value == "Card" => Some(true),
            ParameterValue::Choice(value) if value == "None" || value == "SDRAM" => Some(false),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CameraInfo {
    pub model: String,
    pub port: String,
    pub summary: String,
    pub parameters: Vec<CameraParameter>,
}

impl CameraInfo {
    /// A screen measurement may temporarily enable RAW. Preserve an existing RAW setting,
    /// and change only imageformat (imageformatsd is a separate camera property).
    pub fn raw_capture_format(&self) -> CameraResult<Option<ParameterValue>> {
        let parameter = self
            .parameters
            .iter()
            .find(|p| p.id == "imageformat")
            .ok_or_else(|| {
                CameraError::new(
                    CameraErrorKind::Unsupported,
                    "Photo format unavailable. Refresh camera settings in the device menu.",
                )
            })?;
        if matches!(&parameter.value, ParameterValue::Choice(value) if value.to_ascii_uppercase().contains("RAW"))
        {
            return Ok(None);
        }
        if let ParameterKind::Choice(choices) = &parameter.kind {
            let raw = choices
                .iter()
                .find(|v| v.eq_ignore_ascii_case("RAW"))
                .or_else(|| {
                    choices
                        .iter()
                        .find(|v| v.to_ascii_uppercase().contains("RAW"))
                });
            if let Some(raw) = raw.filter(|_| !parameter.read_only) {
                return Ok(Some(ParameterValue::Choice(raw.clone())));
            }
        }
        Err(CameraError::new(
            CameraErrorKind::Unsupported,
            "RAW cannot be selected remotely in this mode. Set RAW on the camera, then refresh settings.",
        ))
    }

    /// Canon's remote lens drive needs the lens in AF, but continuous AF must be off.
    /// Leave AF off after a step so the camera cannot immediately undo the adjustment.
    pub fn focus_preparation(
        &self,
        near: bool,
        step: u8,
    ) -> CameraResult<Vec<(String, ParameterValue)>> {
        if !(1..=3).contains(&step) {
            return Err(CameraError::new(
                CameraErrorKind::Invalid,
                "Focus step must be 1, 2 or 3",
            ));
        }
        if self.parameters.iter().any(|p| {
            p.id == "focusmode"
                && matches!(&p.value, ParameterValue::Choice(value) if value == "Manual")
        }) {
            return Err(CameraError::new(
                CameraErrorKind::Unsupported,
                "Set the lens switch to AF to use remote Near / Far controls.",
            ));
        }
        let drive = format!("{} {step}", if near { "Near" } else { "Far" });
        if !self
            .parameters
            .iter()
            .any(|p| p.id == "manualfocusdrive" && p.allows_choice(&drive))
        {
            return Err(CameraError::new(
                CameraErrorKind::Unsupported,
                "The camera does not offer this focus step in the current mode.",
            ));
        }
        let mut changes = Vec::new();
        for id in ["continuousaf", "movieservoaf"] {
            let Some(p) = self.parameters.iter().find(|p| p.id == id) else {
                continue;
            };
            let off = match (&p.kind, &p.value) {
                (_, ParameterValue::Choice(value)) if value == "Off" => continue,
                (ParameterKind::Toggle, ParameterValue::Toggle(false)) => continue,
                (_, ParameterValue::Choice(value)) if value == "On" && p.allows_choice("Off") => {
                    ParameterValue::Choice("Off".into())
                }
                (ParameterKind::Toggle, ParameterValue::Toggle(true)) if !p.read_only => {
                    ParameterValue::Toggle(false)
                }
                _ => {
                    return Err(CameraError::new(
                        CameraErrorKind::Unsupported,
                        format!("Disable {} on the camera before using Near / Far.", p.label),
                    ));
                }
            };
            changes.push((id.into(), off));
        }
        Ok(changes)
    }

    pub fn recording_to_card(&self) -> Option<bool> {
        self.parameters
            .iter()
            .find(|p| p.id == "movierecordtarget")?
            .recording_to_card()
    }

    pub fn can_start_recording(&self) -> bool {
        // eosmoviemode is an action widget returning an "always changed" sentinel, not status.
        // eosmovieswitch is the read-only physical switch state on the T4i.
        let photo_mode = self.parameters.iter().any(|p| p.id == "eosmovieswitch"
            && matches!(&p.value, ParameterValue::Text(value) | ParameterValue::Choice(value) if value == "0"));
        !photo_mode
            && self
                .parameters
                .iter()
                .any(|p| p.id == "movierecordtarget" && p.allows_choice("Card"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemoteFile {
    pub folder: String,
    pub name: String,
}

impl RemoteFile {
    pub fn is_media(&self) -> bool {
        matches!(
            self.extension().as_str(),
            "jpg" | "jpeg" | "cr2" | "cr3" | "mov" | "mp4"
        )
    }

    pub fn is_movie(&self) -> bool {
        matches!(self.extension().as_str(), "mov" | "mp4")
    }

    pub fn extension(&self) -> String {
        std::path::Path::new(&self.name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    }

    /// Canon RAW+JPEG companions share folder and stem, never just an event timestamp.
    pub fn group_key(&self) -> String {
        format!(
            "{}/{}",
            self.folder,
            std::path::Path::new(&self.name)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapturedFile {
    pub remote: RemoteFile,
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub id: String,
    pub kind: CaptureKind,
    pub started_unix_ms: u64,
    pub camera: CameraInfo,
    /// Reserved user-supplied context, e.g. phone pattern ID or brightness.
    pub context: BTreeMap<String, String>,
    pub files: Vec<CapturedFile>,
    pub pending_files: Vec<RemoteFile>,
    pub directory: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureKind {
    Photo,
    Movie,
    Recovered,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CameraOperation {
    #[default]
    Idle,
    Connecting,
    Disconnecting,
    Refreshing,
    StartingPreview,
    StoppingPreview,
    SettingParameter(String),
    Focusing,
    Capturing,
    WaitingFiles,
    StartingRecording,
    Recording,
    StoppingRecording,
    Downloading,
    Recovering,
    ShuttingDown,
}

impl CameraOperation {
    pub fn is_busy(&self) -> bool {
        !matches!(self, Self::Idle | Self::Recording)
    }
}

#[derive(Clone, Debug, Default)]
pub struct CameraStatus {
    pub info: Option<CameraInfo>,
    pub preview: bool,
    pub recording: bool,
    pub recording_started: Option<Instant>,
    pub busy: Option<String>,
    pub operation: CameraOperation,
    pub active_record_id: Option<String>,
    pub pending_downloads: usize,
    pub photo_format_restore_pending: bool,
    pub frame_count: u64,
    pub preview_size: Option<[u32; 2]>,
    pub preview_fps: f32,
}

pub struct PreviewFrame {
    pub sequence: u64,
    pub received: Instant,
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum CameraCommand {
    Connect {
        directory: PathBuf,
        release_occupied: bool,
    },
    Disconnect,
    Refresh,
    Preview(bool),
    SetParameter {
        id: String,
        value: ParameterValue,
    },
    Focus {
        near: bool,
        step: u8,
    },
    Capture {
        directory: PathBuf,
        context: BTreeMap<String, String>,
    },
    CaptureRaw {
        directory: PathBuf,
        context: BTreeMap<String, String>,
    },
    StartRecording {
        directory: PathBuf,
        context: BTreeMap<String, String>,
    },
    StopRecording,
    RecoverDownloads,
    RetryDownloads {
        record_id: String,
    },
    Shutdown,
}

impl CameraCommand {
    pub fn operation(&self) -> CameraOperation {
        match self {
            Self::Connect { .. } => CameraOperation::Connecting,
            Self::Disconnect => CameraOperation::Disconnecting,
            Self::Refresh => CameraOperation::Refreshing,
            Self::Preview(true) => CameraOperation::StartingPreview,
            Self::Preview(false) => CameraOperation::StoppingPreview,
            Self::SetParameter { id, .. } => CameraOperation::SettingParameter(id.clone()),
            Self::Focus { .. } => CameraOperation::Focusing,
            Self::Capture { .. } | Self::CaptureRaw { .. } => CameraOperation::Capturing,
            Self::StartRecording { .. } => CameraOperation::StartingRecording,
            Self::StopRecording => CameraOperation::StoppingRecording,
            Self::RecoverDownloads | Self::RetryDownloads { .. } => CameraOperation::Recovering,
            Self::Shutdown => CameraOperation::ShuttingDown,
        }
    }
}

#[derive(Clone, Debug)]
pub enum CameraEvent {
    Status(CameraStatus),
    /// The camera accepted the drive command; this does not prove physical lens movement.
    FocusAcknowledged {
        near: bool,
        step: u8,
    },
    Record(CaptureRecord),
    Transfer {
        record_id: String,
        name: String,
        fraction: Option<f32>,
    },
    Error {
        error: CameraError,
        operation: CameraOperation,
        record_id: Option<String>,
    },
    Notice(String),
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraErrorKind {
    Unavailable,
    NotFound,
    Occupied,
    Disconnected,
    Busy,
    Unsupported,
    Storage,
    Invalid,
    Other,
}

#[derive(Clone, Debug)]
pub struct CameraError {
    pub kind: CameraErrorKind,
    pub message: String,
}

impl CameraError {
    pub fn new(kind: CameraErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for CameraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CameraError {}
pub type CameraResult<T> = Result<T, CameraError>;

#[derive(Clone, Debug)]
pub enum BackendEvent {
    Timeout,
    Changed,
    FileAdded(RemoteFile),
    CaptureComplete,
}

/// All methods are called on the owning camera thread. Implementations need not be Send.
pub trait CameraBackend {
    fn connect(&mut self) -> CameraResult<CameraInfo>;
    fn disconnect(&mut self) -> CameraResult<()>;
    fn info(&mut self) -> CameraResult<CameraInfo>;
    fn set_parameter(&mut self, id: &str, value: &ParameterValue) -> CameraResult<()>;
    fn preview(&mut self, enabled: bool) -> CameraResult<()>;
    fn preview_frame(&mut self) -> CameraResult<Vec<u8>>;
    fn focus(&mut self, near: bool, step: u8) -> CameraResult<()>;
    fn capture(&mut self) -> CameraResult<Option<RemoteFile>>;
    fn record(&mut self, enabled: bool) -> CameraResult<()>;
    fn poll_event(&mut self, timeout_ms: i32) -> CameraResult<BackendEvent>;
    fn list_files(&mut self) -> CameraResult<Vec<RemoteFile>>;
    fn download(
        &mut self,
        file: &RemoteFile,
        output: &std::fs::File,
        progress: &mut dyn FnMut(f32),
    ) -> CameraResult<()>;
}
