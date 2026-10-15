use super::model::*;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn default_root() -> PathBuf {
    std::env::var_os("NODE_FORGE_SCREEN_TEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join("Library/Application Support/Node Forge/Screen Tests")
        })
}
pub struct Repository {
    db: Connection,
    pub root: PathBuf,
}
impl Repository {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("files"))?;
        let db = Connection::open(root.join("screen-tests.sqlite3"))?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS captures(id TEXT PRIMARY KEY, created INTEGER NOT NULL, kind TEXT NOT NULL, payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS reference_versions(id TEXT PRIMARY KEY, created INTEGER NOT NULL, payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS reference_members(reference_id TEXT NOT NULL REFERENCES reference_versions(id), capture_id TEXT NOT NULL REFERENCES captures(id), PRIMARY KEY(reference_id,capture_id));
            CREATE TABLE IF NOT EXISTS reports(id TEXT PRIMARY KEY, capture_id TEXT NOT NULL REFERENCES captures(id), reference_id TEXT NOT NULL REFERENCES reference_versions(id), created INTEGER NOT NULL, payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS calibration_runs(id TEXT PRIMARY KEY, created INTEGER NOT NULL, payload TEXT NOT NULL);")?;
        Ok(Self { db, root })
    }
    pub fn import_raw(&self, source: &Path) -> Result<(PathBuf, String)> {
        ensure!(
            source
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("cr2")),
            "Color measurements require an original CR2"
        );
        let bytes = fs::read(source).with_context(|| format!("Read {}", source.display()))?;
        ensure!(
            bytes.len() > 16 && (&bytes[..2] == b"II" || &bytes[..2] == b"MM"),
            "Not a TIFF-based Canon RAW file"
        );
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let relative = PathBuf::from("files").join(format!("{hash}.CR2"));
        let destination = self.root.join(&relative);
        if !destination.exists() {
            let tmp = self.root.join("files").join(new_id("incoming"));
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            use std::io::Write;
            f.write_all(&bytes)?;
            f.sync_all()?;
            drop(f);
            fs::rename(tmp, destination)?;
        }
        Ok((relative, hash))
    }
    pub fn verify_raw(&self, capture: &StoredCapture) -> Result<PathBuf> {
        ensure!(
            !capture.raw_path.is_absolute()
                && capture
                    .raw_path
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
            "Invalid raw path"
        );
        let path = self.root.join(&capture.raw_path);
        ensure!(
            format!("{:x}", Sha256::digest(fs::read(&path)?)) == capture.raw_sha256,
            "RAW checksum mismatch"
        );
        Ok(path)
    }
    pub fn insert_capture(&self, capture: &StoredCapture) -> Result<()> {
        ensure!(
            capture.ticket.workflow != Workflow::Geometry,
            "Geometry calibration belongs to the current session, not the capture library"
        );
        self.verify_raw(capture)?;
        if capture.measurements.is_some() {
            self.verify_rectified(capture)?;
        }
        self.db.execute(
            "INSERT INTO captures VALUES (?1,?2,?3,?4)",
            params![
                capture.id,
                capture.created_ms,
                format!("{:?}", capture.ticket.workflow),
                serde_json::to_string(capture)?
            ],
        )?;
        Ok(())
    }
    pub fn capture(&self, id: &str) -> Result<StoredCapture> {
        self.get("captures", id)
    }
    pub fn reference(&self, id: &str) -> Result<ReferenceSetVersion> {
        let reference: ReferenceSetVersion = self.get("reference_versions", id)?;
        for member in &reference.members {
            let capture = self.capture(member)?;
            self.verify_raw(&capture)?;
            self.verify_rectified(&capture)?;
        }
        Ok(reference)
    }
    fn get<T: DeserializeOwned>(&self, table: &str, id: &str) -> Result<T> {
        let payload: String = self.db.query_row(
            &format!("SELECT payload FROM {table} WHERE id=?1"),
            [id],
            |r| r.get(0),
        )?;
        Ok(serde_json::from_str(&payload)?)
    }
    pub fn captures(&self) -> Result<Vec<StoredCapture>> {
        self.list("captures")
    }
    pub fn save_sweep(&self, run: &super::batch::SweepRun) -> Result<()> {
        for cell in run
            .cells
            .iter()
            .filter(|c| c.state == super::batch::CellState::Saved)
        {
            let id = cell
                .reference
                .as_ref()
                .context("Saved condition has no reference")?;
            let reference: ReferenceSetVersion = self.get("reference_versions", id)?;
            ensure!(
                reference.conditions.batch == run.id
                    && reference.conditions.manifest.mode == cell.mode
                    && (reference.conditions.brightness - cell.brightness).abs() < 0.001
                    && reference.members == cell.captures,
                "Saved reference does not belong to this batch condition"
            );
        }
        self.db.execute("INSERT INTO calibration_runs VALUES (?1,?2,?3) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",
            params![run.id, run.created_ms, serde_json::to_string(run)?])?;
        self.write_json(&PathBuf::from("runs").join(format!("{}.json", run.id)), run)?;
        Ok(())
    }
    pub fn sweeps(&self) -> Result<Vec<super::batch::SweepRun>> {
        self.list("calibration_runs")
    }
    pub fn references(&self) -> Result<Vec<ReferenceSetVersion>> {
        self.list("reference_versions")
    }
    pub fn reports(&self) -> Result<Vec<InspectionReport>> {
        self.list("reports")
    }
    fn list<T: DeserializeOwned>(&self, table: &str) -> Result<Vec<T>> {
        let mut stmt = self.db.prepare(&format!(
            "SELECT payload FROM {table} ORDER BY created DESC,id DESC"
        ))?;
        let strings = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        strings
            .into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }
    pub fn insert_reference(&mut self, reference: &ReferenceSetVersion) -> Result<()> {
        ensure!(
            !reference.members.is_empty(),
            "A baseline needs measured reference captures"
        );
        for id in &reference.members {
            let capture = self.capture(id)?;
            self.verify_raw(&capture)?;
            self.verify_rectified(&capture)?;
            ensure!(
                capture.ticket.workflow == Workflow::Reference
                    && capture.measurements.is_some()
                    && capture.analysis_issue.is_none(),
                "Reference member is not analyzed"
            );
            super::measurement::compatible(
                &reference.conditions,
                &capture.ticket.conditions,
                true,
            )?;
        }
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO reference_versions VALUES (?1,?2,?3)",
            params![
                reference.id,
                reference.created_ms,
                serde_json::to_string(reference)?
            ],
        )?;
        for id in &reference.members {
            tx.execute(
                "INSERT INTO reference_members VALUES (?1,?2)",
                params![reference.id, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn insert_report(&self, report: &InspectionReport) -> Result<()> {
        for id in &report.source_captures {
            let capture = self.capture(id)?;
            self.verify_raw(&capture)?;
            self.verify_rectified(&capture)?;
            ensure!(
                capture.analysis_issue.is_none() && capture.display_issue.is_none(),
                "Report source failed quality checks"
            );
        }
        let relative = PathBuf::from("reports").join(format!("{}.json", report.id));
        ensure!(!self.root.join(&relative).exists(), "Report already exists");
        self.write_json(&relative, report)?;
        let map_folder = self.root.join("results").join(&report.id);
        fs::create_dir_all(&map_folder)?;
        let manifest = &report.test_conditions.manifest;
        let mut map = image::RgbImage::from_pixel(
            manifest.size[0],
            manifest.size[1],
            image::Rgb([22, 22, 22]),
        );
        for patch in &manifest.patches {
            let diff = report.differences.iter().find(|p| p.id == patch.id);
            let color = diff
                .map(|p| {
                    let intensity = (p.max_fraction_of_white / 0.1).clamp(0., 1.);
                    image::Rgb([
                        (40. + 215. * intensity) as u8,
                        (90. * (1. - intensity)) as u8,
                        45,
                    ])
                })
                .unwrap_or(image::Rgb([170, 120, 30]));
            let [x, y, w, h] = patch.rect;
            for yy in y..y + h {
                for xx in x..x + w {
                    map.put_pixel(xx, manifest.size[1] - 1 - yy, color);
                }
            }
        }
        map.save(map_folder.join("difference-map.png"))?;
        self.db.execute(
            "INSERT INTO reports VALUES (?1,?2,?3,?4,?5)",
            params![
                report.id,
                report.capture,
                report.reference,
                report.created_ms,
                serde_json::to_string(report)?
            ],
        )?;
        Ok(())
    }
    pub fn write_json(&self, relative: &Path, value: &impl Serialize) -> Result<PathBuf> {
        let path = self.root.join(relative);
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        let tmp = path.with_extension("tmp");
        use std::io::Write;
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        drop(file);
        fs::rename(tmp, &path)?;
        Ok(path)
    }
    pub fn verify_rectified(&self, capture: &StoredCapture) -> Result<()> {
        capture.ensure_geometry_verified()?;
        let proof = capture.geometry_verified.as_ref().unwrap();
        ensure!(
            format!("{:x}", Sha256::digest(fs::read(&proof.normalized_path)?))
                == proof.normalized_sha256,
            "Normalized image checksum mismatch; repeat geometry analysis"
        );
        Ok(())
    }
}
