//! Persist display results, not session calibration or RAW measurement data.
use super::*;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct SavedResult {
    pub source: PathBuf,
    pub source_sha256: String,
    pub geometry_id: String,
    pub size: [u32; 2],
    pub png_sha256: String,
    pub display_source: String,
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub(super) enum ResultState {
    Pending,
    Saved(SavedResult),
    Failed(String),
    Expired,
}
struct Entry {
    key: (PathBuf, String, String),
    state: ResultState,
}
struct Job {
    record: CaptureRecord,
    source: CapturedFile,
    geometry: Option<Arc<DisplayGeometry>>,
}
#[derive(Default)]
pub(super) struct Results {
    entries: HashMap<String, Entry>,
    queue: VecDeque<Job>,
    job: Option<(String, mpsc::Receiver<Result<Option<SavedResult>, String>>)>,
}

pub(super) fn photo_file(record: &CaptureRecord) -> Option<&CapturedFile> {
    record
        .files
        .iter()
        .find(|f| matches!(f.remote.extension().as_str(), "jpg" | "jpeg" | "png"))
        .or_else(|| record.files.iter().find(|f| f.remote.extension() == "cr2"))
}

impl Results {
    pub fn observe(&mut self, record: &CaptureRecord, geometry: Option<Arc<DisplayGeometry>>) {
        let Some(id) = record.context.get("display_geometry_id") else {
            return;
        };
        let Some(source) = photo_file(record) else {
            return;
        };
        // Wait for a RAW+JPEG download group to finish before choosing its display source.
        if !record.pending_files.is_empty() {
            return;
        }
        let key = (source.path.clone(), source.sha256.clone(), id.clone());
        if self.entries.get(&record.id).is_some_and(|e| e.key == key) {
            return;
        }
        if self.job.as_ref().is_some_and(|(id, _)| id == &record.id) {
            return;
        }
        self.queue.retain(|j| j.record.id != record.id);
        self.entries.insert(
            record.id.clone(),
            Entry {
                key,
                state: ResultState::Pending,
            },
        );
        self.queue.push_back(Job {
            record: record.clone(),
            source: source.clone(),
            geometry,
        });
    }
    pub fn poll(&mut self) -> Option<String> {
        let completed = self.job.as_ref().and_then(|(_, rx)| match rx.try_recv() {
            Ok(value) => Some(value),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("Correction worker stopped".into())),
            Err(mpsc::TryRecvError::Empty) => None,
        });
        let completed_id = completed.map(|result| {
            let (id, _) = self.job.take().unwrap();
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.state = match result {
                    Ok(Some(saved)) => ResultState::Saved(saved),
                    Ok(None) => ResultState::Expired,
                    Err(error) => ResultState::Failed(error),
                };
            }
            id
        });
        if self.job.is_none() {
            if let Some(job) = self.queue.pop_front() {
                let id = job.record.id.clone();
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(load_or_save(&job).map_err(|e| format!("{e:#}")));
                });
                self.job = Some((id, rx));
            }
        }
        completed_id
    }
    pub fn state(&self, id: &str) -> Option<&ResultState> {
        self.entries.get(id).map(|e| &e.state)
    }
    pub fn saved(&self, id: &str) -> Option<&SavedResult> {
        match self.state(id)? {
            ResultState::Saved(result) => Some(result),
            _ => None,
        }
    }
    pub fn for_path(&self, path: &Path) -> Option<&SavedResult> {
        self.entries.values().find_map(|e| match &e.state {
            ResultState::Saved(result) if result.path == path || result.source == path => {
                Some(result)
            }
            _ => None,
        })
    }
    pub fn busy(&self) -> bool {
        self.job.is_some() || !self.queue.is_empty()
    }
    pub fn retry(&mut self, id: &str) {
        if matches!(self.state(id), Some(ResultState::Failed(_))) {
            self.entries.remove(id);
        }
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn load_or_save(job: &Job) -> Result<Option<SavedResult>> {
    let folder = job.record.directory.join("results");
    let path = folder.join("rectified.png");
    let metadata = folder.join("rectified.json");
    let id = job
        .record
        .context
        .get("display_geometry_id")
        .context("Missing calibration association")?;
    if let Ok(bytes) = std::fs::read(&metadata) {
        if let Ok(mut saved) = serde_json::from_slice::<SavedResult>(&bytes) {
            if saved.geometry_id == *id
                && saved.source == job.source.path
                && saved.source_sha256 == job.source.sha256
                && std::fs::read(&path).is_ok_and(|bytes| digest(&bytes) == saved.png_sha256)
            {
                saved.path = path;
                return Ok(Some(saved));
            }
        }
    }
    let Some(geometry) = &job.geometry else {
        return Ok(None);
    };
    ensure!(geometry.profile.id == *id, "Capture calibration changed");
    let bytes = std::fs::read(&job.source.path)?;
    ensure!(
        digest(&bytes) == job.source.sha256,
        "Original file has changed; correction not saved"
    );
    let (size, rgba, orientation) = decode_photo(&job.source.path)?;
    let pixels = rectify(geometry, size, orientation, rgba)?;
    std::fs::create_dir_all(&folder)?;
    let temp = folder.join("rectified.png.part");
    let result = (|| -> Result<SavedResult> {
        pixels.save_with_format(&temp, image::ImageFormat::Png)?;
        std::fs::File::open(&temp)?.sync_all()?;
        let png_sha256 = digest(&std::fs::read(&temp)?);
        std::fs::rename(&temp, &path)?;
        let saved = SavedResult {
            source: job.source.path.clone(),
            source_sha256: job.source.sha256.clone(),
            geometry_id: id.clone(),
            size: geometry.screen_size,
            png_sha256,
            path,
            display_source: if job.source.remote.extension() == "cr2" {
                "camera-embedded-jpeg"
            } else {
                "camera-photo"
            }
            .into(),
        };
        let meta_temp = folder.join("rectified.json.part");
        std::fs::write(&meta_temp, serde_json::to_vec_pretty(&saved)?)?;
        std::fs::File::open(&meta_temp)?.sync_all()?;
        std::fs::rename(meta_temp, metadata)?;
        Ok(saved)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result.map(Some)
}

fn rectify(
    g: &DisplayGeometry,
    size: [u32; 2],
    orientation: Orientation,
    rgba: Vec<u8>,
) -> Result<image::RgbaImage> {
    let source =
        image::RgbaImage::from_raw(size[0], size[1], rgba).context("Invalid photo pixels")?;
    let [width, height] = g.screen_size;
    ensure!(
        width > 1 && height > 1 && u64::from(width) * u64::from(height) <= 40_000_000,
        "Invalid corrected image dimensions"
    );
    // The exact same inverse sampling map as Live View. This is a display image, never CFA input.
    Ok(image::RgbaImage::from_fn(width, height, |x, y| {
        let Some(uv) = geometry::sampling_uv(
            g,
            size,
            orientation,
            [
                (x as f64 + 0.5) / width as f64,
                1. - (y as f64 + 0.5) / height as f64,
            ],
        ) else {
            return image::Rgba([0; 4]);
        };
        if !(0.0..=1.0).contains(&uv.x) || !(0.0..=1.0).contains(&uv.y) {
            return image::Rgba([0; 4]);
        }
        let x = (uv.x * size[0] as f32 - 0.5).clamp(0., (size[0] - 1) as f32);
        let y = (uv.y * size[1] as f32 - 0.5).clamp(0., (size[1] - 1) as f32);
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(size[0] - 1), (y0 + 1).min(size[1] - 1));
        let (fx, fy) = (x.fract(), y.fract());
        image::Rgba(std::array::from_fn(|c| {
            let top = source[(x0, y0)][c] as f32 * (1. - fx) + source[(x1, y0)][c] as f32 * fx;
            let bottom = source[(x0, y1)][c] as f32 * (1. - fx) + source[(x1, y1)][c] as f32 * fx;
            (top * (1. - fy) + bottom * fy).round() as u8
        }))
    }))
}

impl Workspace {
    pub(super) fn poll_results(&mut self) {
        if self.fixture.is_some() {
            return;
        }
        // Never write capture.json here: the camera worker owns it and may append a late file.
        for record in &self.records {
            if self.status.active_record_id.as_ref() == Some(&record.id)
                && self.status.operation.is_busy()
            {
                continue;
            }
            let geometry = record
                .context
                .get("display_geometry_id")
                .and_then(|id| self.geometry_store().lock().ok()?.display(id));
            self.results.observe(record, geometry);
        }
        if let Some(id) = self.results.poll() {
            if let Some(saved) = self.results.saved(&id) {
                self.thumbnails.invalidate(&saved.path);
            }
            if self.geometry_enabled {
                if let Some(saved) = self.results.saved(&id) {
                    if matches!(&self.source, CanvasSource::Photo(p) if *p == saved.source || *p == saved.path)
                    {
                        self.open_photo(saved.path.clone());
                    }
                }
            }
        }
    }
    pub(super) fn toggle_correction(&mut self) {
        self.geometry_enabled = !self.geometry_enabled;
        if let CanvasSource::Photo(path) = &self.source {
            if let Some(saved) = self.results.for_path(path) {
                let path = if self.geometry_enabled {
                    saved.path.clone()
                } else {
                    saved.source.clone()
                };
                self.open_photo(path);
            }
        }
        self.live_view.fit();
        self.photo_view.fit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        pixel_perfecto::calibration::Calibration,
        screen_test::model::{GeometryProfile, Workflow},
    };

    struct Fixture {
        folder: PathBuf,
        record: CaptureRecord,
        geometry: Arc<DisplayGeometry>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.folder);
        }
    }
    fn fixture() -> Fixture {
        let folder =
            std::env::temp_dir().join(crate::screen_test::model::new_id("corrected-result-test"));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("original.png");
        image::RgbaImage::from_fn(4, 4, |x, y| {
            image::Rgba([x as u8 * 40, y as u8 * 60, 0, 255])
        })
        .save(&path)
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let record = CaptureRecord {
            id: "capture".into(),
            kind: CaptureKind::Photo,
            started_unix_ms: 1,
            camera: CameraInfo::default(),
            context: BTreeMap::from([("display_geometry_id".into(), "session-a".into())]),
            files: vec![CapturedFile {
                remote: RemoteFile {
                    folder: "/".into(),
                    name: "original.png".into(),
                },
                path,
                bytes: bytes.len() as u64,
                sha256: digest(&bytes),
            }],
            pending_files: vec![],
            directory: folder.clone(),
        };
        std::fs::write(
            folder.join("capture.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let geometry = Arc::new(DisplayGeometry {
            profile: GeometryProfile {
                id: "session-a".into(),
                revision: 1,
                created_ms: 0,
                source_capture: "chart".into(),
                camera_key: "650D".into(),
                raw_size: [4, 4],
                mapping: Calibration {
                    h: [1., 0., 0.25, 0., 1., 0.25, 0., 0., 1.],
                    lens: [0.; 4],
                    center: [0.; 2],
                },
                holdout_rms: 0.1,
                marker_count: 39,
            },
            screen_size: [2, 2],
            sensor_crop: [0., 0., 4., 4.],
        });
        Fixture {
            folder,
            record,
            geometry,
        }
    }
    fn wait(results: &mut Results) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while results.busy() {
            results.poll();
            assert!(Instant::now() < deadline, "Result worker did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn camera_download_creates_real_rectified_pixels_without_modifying_raw_or_manifest() {
        let f = fixture();
        let original = std::fs::read(&f.record.files[0].path).unwrap();
        let manifest = std::fs::read(f.folder.join("capture.json")).unwrap();
        let mut w = Workspace::new();
        w.history_job = None;
        w.screen_test = Some(crate::screen_test::ui::Panel::preview(Workflow::Geometry));
        w.geometry_store()
            .lock()
            .unwrap()
            .publish_display(
                f.geometry.profile.clone(),
                f.geometry.screen_size,
                f.geometry.sensor_crop,
            )
            .unwrap();
        w.sync_display_geometry();
        w.accept_event(CameraEvent::Record(f.record.clone()));
        w.poll_results();
        assert!(w.results.busy());
        w.request_close(None);
        assert!(w.close_dialog);
        wait(&mut w.results);
        let saved = w.results.saved("capture").unwrap();
        let image = image::open(&saved.path).unwrap().into_rgba8();
        assert_eq!(image.dimensions(), (2, 2));
        assert_eq!(image[(0, 0)].0, [40, 60, 0, 255]);
        assert_eq!(image[(1, 1)].0, [80, 120, 0, 255]);
        assert_eq!(std::fs::read(&f.record.files[0].path).unwrap(), original);
        assert_eq!(
            std::fs::read(f.folder.join("capture.json")).unwrap(),
            manifest
        );
        assert_eq!(w.source, CanvasSource::Live); // Background completion never steals Live View.
        w.source = CanvasSource::Photo(saved.path.clone());
        assert!(w.canvas_geometry().is_none()); // A saved image is never warped again.
    }
    #[test]
    fn saved_result_survives_new_app_session_without_restoring_calibration() {
        let f = fixture();
        let mut results = Results::default();
        results.observe(&f.record, Some(f.geometry.clone()));
        wait(&mut results);
        let path = results.saved("capture").unwrap().path.clone();
        let mut w = Workspace::new();
        w.history_job = None;
        w.screen_test = Some(crate::screen_test::ui::Panel::preview(Workflow::Geometry));
        w.records.push(f.record.clone());
        w.poll_results();
        wait(&mut w.results);
        assert!(
            w.geometry_store()
                .lock()
                .unwrap()
                .active_display()
                .is_none()
        );
        assert_eq!(w.results.saved("capture").unwrap().path, path);
        w.source = CanvasSource::Photo(path.clone());
        w.toggle_correction();
        assert_eq!(
            w.source,
            CanvasSource::Photo(f.record.files[0].path.clone())
        );
        w.toggle_correction();
        assert_eq!(w.source, CanvasSource::Photo(path));
        assert!(w.canvas_geometry().is_none());
    }
    #[test]
    fn failed_save_retries_only_image_processing_and_expired_geometry_is_explicit() {
        let f = fixture();
        std::fs::write(f.folder.join("results"), b"blocked directory").unwrap();
        let mut results = Results::default();
        results.observe(&f.record, Some(f.geometry.clone()));
        wait(&mut results);
        assert!(matches!(
            results.state("capture"),
            Some(ResultState::Failed(_))
        ));
        std::fs::remove_file(f.folder.join("results")).unwrap();
        results.retry("capture");
        results.observe(&f.record, Some(f.geometry.clone()));
        wait(&mut results);
        assert!(results.saved("capture").is_some());
        std::fs::remove_file(f.folder.join("results/rectified.png")).unwrap();
        let mut reopened = Results::default();
        reopened.observe(&f.record, None);
        wait(&mut reopened);
        assert!(matches!(
            reopened.state("capture"),
            Some(ResultState::Expired)
        ));
        assert!(!f.folder.join("results/rectified.png").exists());
    }
    #[test]
    fn changed_source_cannot_publish_a_result() {
        let f = fixture();
        let job = Job {
            record: f.record.clone(),
            source: f.record.files[0].clone(),
            geometry: Some(f.geometry.clone()),
        };
        std::fs::write(&job.source.path, b"changed").unwrap();
        assert!(
            load_or_save(&job)
                .unwrap_err()
                .to_string()
                .contains("Original file has changed")
        );
        assert!(!f.folder.join("results/rectified.png").exists());
    }

    #[test]
    #[ignore = "requires explicit RAW, geometry fixture and output directory; no USB access"]
    fn real_raw_result_export() {
        #[derive(Deserialize)]
        struct Input {
            profile: GeometryProfile,
            screen_size: [u32; 2],
            sensor_crop: [f64; 4],
        }
        let source = PathBuf::from(std::env::var("NODE_FORGE_CAMERA_RESULT_RAW").unwrap());
        let input: Input = serde_json::from_slice(
            &std::fs::read(std::env::var("NODE_FORGE_CAMERA_FIXTURE_GEOMETRY").unwrap()).unwrap(),
        )
        .unwrap();
        let directory = PathBuf::from(std::env::var("NODE_FORGE_CAMERA_RESULT_DIR").unwrap());
        let bytes = std::fs::read(&source).unwrap();
        let file = CapturedFile {
            remote: RemoteFile {
                folder: "/".into(),
                name: source.file_name().unwrap().to_string_lossy().into(),
            },
            path: source.clone(),
            bytes: bytes.len() as u64,
            sha256: digest(&bytes),
        };
        let record = CaptureRecord {
            id: "offline-result-validation".into(),
            kind: CaptureKind::Photo,
            started_unix_ms: 0,
            camera: CameraInfo::default(),
            context: BTreeMap::from([("display_geometry_id".into(), input.profile.id.clone())]),
            files: vec![file.clone()],
            pending_files: vec![],
            directory,
        };
        let job = Job {
            record,
            source: file,
            geometry: Some(Arc::new(DisplayGeometry {
                profile: input.profile,
                screen_size: input.screen_size,
                sensor_crop: input.sensor_crop,
            })),
        };
        let saved = load_or_save(&job).unwrap().unwrap();
        assert_eq!(
            image::image_dimensions(&saved.path).unwrap(),
            (input.screen_size[0], input.screen_size[1])
        );
        assert_eq!(digest(&std::fs::read(source).unwrap()), saved.source_sha256);
        println!("Saved corrected result: {}", saved.path.display());
    }
}
