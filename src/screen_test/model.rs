use crate::pixel_perfecto::calibration::Calibration;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn new_id(prefix: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "{prefix}-{}-{}-{}",
        now_ms(),
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Workflow {
    Geometry,
    #[default]
    Reference,
    Inspection,
}
impl Workflow {
    pub fn label(self) -> &'static str {
        match self {
            Self::Geometry => "Geometry calibration",
            Self::Reference => "Color calibration",
            Self::Inspection => "Color inspection",
        }
    }
    pub fn action(self) -> &'static str {
        match self {
            Self::Geometry => "Calibrate geometry",
            Self::Reference => "Capture reference",
            Self::Inspection => "Capture test",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayMode {
    #[default]
    Srgb,
    DisplayP3,
    LinearHdr,
}
impl DisplayMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::DisplayP3 => "Display P3",
            Self::LinearHdr => "Linear HDR",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    Android,
    Ios,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
    #[serde(other)]
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrightnessGuard {
    pub requested_brightness: Option<f32>,
    pub holding: bool,
    pub idle_timer_disabled: bool,
    pub settled: bool,
    pub revision: u64,
    pub thermal_state: DisplayThermalState,
    pub low_power_mode: bool,
    pub issue: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayDevice {
    pub id: String,
    pub platform: Platform,
    pub name: String,
    pub size: [u32; 2],
    pub drawable: [u32; 2],
    /// Left, bottom, right, top in physical pixels. Public coordinates are Y-up.
    pub insets: [u32; 4],
    pub modes: Vec<DisplayMode>,
    pub brightness: f32,
    pub headroom: f32,
    pub active: bool,
    pub generation: String,
    /// Android and previously saved captures have no iOS brightness guard snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness_guard: Option<BrightnessGuard>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Patch {
    pub id: u32,
    pub rect: [u32; 4],
    pub rgb: [f32; 3],
    pub display_rgb: [f32; 3],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Marker {
    pub id: i32,
    pub rect: [u32; 4],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatternManifest {
    pub version: String,
    pub generation: String,
    pub size: [u32; 2],
    pub insets: [u32; 4],
    pub mode: DisplayMode,
    pub peak: f32,
    pub patches: Vec<Patch>,
    pub markers: Vec<Marker>,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeometryProfile {
    pub id: String,
    pub revision: u64,
    pub created_ms: u64,
    pub source_capture: String,
    pub camera_key: String,
    pub raw_size: [u32; 2],
    /// Maps normalized screen UV to active RAW sensor pixels, both Y-up.
    pub mapping: Calibration,
    pub holdout_rms: f64,
    pub marker_count: usize,
}
/// Display-only metadata. Kept in memory for this App session, never loaded from the library.
#[derive(Clone, Debug)]
pub struct DisplayGeometry {
    pub profile: GeometryProfile,
    pub screen_size: [u32; 2],
    /// Camera JPEG image area in active RAW pixel coordinates, bottom-left/Y-up.
    pub sensor_crop: [f64; 4],
}
#[derive(Default)]
pub struct SessionGeometryStore {
    pub active: Option<GeometryProfile>,
    revision: u64,
    displays: HashMap<String, Arc<DisplayGeometry>>,
}
impl SessionGeometryStore {
    pub fn publish(&mut self, mut profile: GeometryProfile) -> anyhow::Result<GeometryProfile> {
        anyhow::ensure!(
            profile.holdout_rms.is_finite() && profile.holdout_rms <= 1.,
            "Geometry validation exceeds 1 camera pixel"
        );
        profile.mapping.validate(profile.raw_size)?;
        profile.revision = self.revision + 1;
        self.revision = profile.revision;
        self.active = Some(profile.clone());
        Ok(profile)
    }
    pub fn publish_display(
        &mut self,
        profile: GeometryProfile,
        screen_size: [u32; 2],
        sensor_crop: [f64; 4],
    ) -> anyhow::Result<GeometryProfile> {
        anyhow::ensure!(
            screen_size.iter().all(|v| *v > 1),
            "Invalid screen dimensions"
        );
        let [x, y, w, h] = sensor_crop;
        anyhow::ensure!(
            sensor_crop.iter().all(|v| v.is_finite())
                && x >= 0.
                && y >= 0.
                && w > 1.
                && h > 1.
                && x + w <= profile.raw_size[0] as f64
                && y + h <= profile.raw_size[1] as f64,
            "Invalid camera display crop"
        );
        let profile = self.publish(profile)?;
        self.displays.insert(
            profile.id.clone(),
            Arc::new(DisplayGeometry {
                profile: profile.clone(),
                screen_size,
                sensor_crop,
            }),
        );
        Ok(profile)
    }
    pub fn display(&self, id: &str) -> Option<Arc<DisplayGeometry>> {
        self.displays.get(id).cloned()
    }
    pub fn active_display(&self) -> Option<Arc<DisplayGeometry>> {
        self.display(&self.active.as_ref()?.id)
    }
}
/// App-process lifetime, not window lifetime. Never hydrated from the reference database.
pub fn session_geometry() -> Arc<Mutex<SessionGeometryStore>> {
    static STORE: OnceLock<Arc<Mutex<SessionGeometryStore>>> = OnceLock::new();
    STORE
        .get_or_init(|| Arc::new(Mutex::new(SessionGeometryStore::default())))
        .clone()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaptureConditions {
    pub device: DisplayDevice,
    pub brightness: f32,
    pub batch: String,
    pub notes: String,
    pub manifest: PatternManifest,
    pub camera_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaptureTicket {
    pub id: String,
    pub workflow: Workflow,
    pub conditions: CaptureConditions,
    pub geometry: Option<GeometryProfile>,
    pub reference: Option<String>,
    pub thresholds: Option<Thresholds>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawMetadata {
    pub width: u32,
    pub height: u32,
    pub cfa: [u8; 4],
    pub black: [f64; 4],
    pub white: f64,
    pub exposure: f64,
    pub iso: f64,
    pub aperture: f64,
    pub focal_length: f64,
    #[serde(default)]
    pub make: String,
    pub camera: String,
    pub lens: String,
    pub orientation: i32,
    /// XYZ D65 to camera RGB, as required by DNG ColorMatrix1.
    #[serde(default = "identity_color_matrix")]
    pub color_matrix: [f64; 9],
    #[serde(default = "unit_as_shot_neutral")]
    pub as_shot_neutral: [f64; 3],
}
fn identity_color_matrix() -> [f64; 9] {
    [1., 0., 0., 0., 1., 0., 0., 0., 1.]
}
fn unit_as_shot_neutral() -> [f64; 3] {
    [1.; 3]
}
impl RawMetadata {
    pub fn geometry_key(&self) -> String {
        format!("{}|{}|{}", self.camera, self.lens, self.focal_length)
    }
    pub fn camera_key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.camera, self.lens, self.iso, self.aperture, self.focal_length
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatchMeasurement {
    pub id: u32,
    pub rgb: [f64; 3],
    pub cfa_mean: [f64; 4],
    pub stddev: [f64; 4],
    pub count: [u64; 4],
    pub rejected_fraction: f64,
    pub valid: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Measurements {
    pub raw: RawMetadata,
    pub patches: Vec<PatchMeasurement>,
    pub issues: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ColorTransform {
    pub matrix: [[f64; 3]; 3],
    pub validation_rms: f64,
    pub reference_white: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeometryEvidence {
    pub profile_id: String,
    pub pattern_generation: String,
    pub raw_sha256: String,
    pub max_residual_px: f64,
    pub normalized_path: PathBuf,
    pub normalized_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredCapture {
    pub id: String,
    pub created_ms: u64,
    pub ticket: CaptureTicket,
    pub camera_record: String,
    pub raw_path: PathBuf,
    pub raw_sha256: String,
    pub measurements: Option<Measurements>,
    pub display_issue: Option<String>,
    pub analysis_issue: Option<String>,
    pub artifacts: Vec<PathBuf>,
    /// Old/unprocessed records have no evidence and are ineligible for color calibration.
    #[serde(default)]
    pub geometry_verified: Option<GeometryEvidence>,
}
impl StoredCapture {
    pub fn ensure_geometry_verified(&self) -> anyhow::Result<()> {
        use anyhow::{Context, ensure};
        let proof = self.geometry_verified.as_ref().context(
            "Color calibration requires a geometrically corrected RAW; reanalyze this capture",
        )?;
        let profile = self
            .ticket
            .geometry
            .as_ref()
            .context("Missing geometry profile")?;
        ensure!(
            proof.profile_id == profile.id
                && proof.pattern_generation == self.ticket.conditions.manifest.generation
                && proof.raw_sha256 == self.raw_sha256
                && proof.max_residual_px.is_finite()
                && (0.0..=3.0).contains(&proof.max_residual_px)
                && profile.holdout_rms.is_finite()
                && (0.0..=1.0).contains(&profile.holdout_rms)
                && self.artifacts.contains(&proof.normalized_path),
            "Geometry evidence does not match this RAW/presentation"
        );
        ensure!(
            self.analysis_issue.is_none() && self.display_issue.is_none(),
            "Capture failed geometry/display validation"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferenceSetVersion {
    pub id: String,
    pub name: String,
    pub created_ms: u64,
    pub members: Vec<String>,
    pub conditions: CaptureConditions,
    pub measurements: Measurements,
    pub transform: Option<ColorTransform>,
    pub dispersion: Vec<[f64; 3]>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Thresholds {
    pub rgb_fraction_of_white: f64,
    pub white_relative: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatchDifference {
    pub id: u32,
    pub rect: [u32; 4],
    pub delta: [f64; 3],
    pub max_fraction_of_white: f64,
    pub flagged: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InspectionReport {
    pub id: String,
    pub capture: String,
    pub reference: String,
    pub created_ms: u64,
    pub source_captures: Vec<String>,
    pub test_conditions: CaptureConditions,
    pub reference_conditions: CaptureConditions,
    pub reference_members: Vec<String>,
    pub geometry: Option<GeometryProfile>,
    pub thresholds: Option<Thresholds>,
    pub differences: Vec<PatchDifference>,
    pub white_relative: Option<f64>,
    pub issues: Vec<String>,
    pub conclusion: String,
}
