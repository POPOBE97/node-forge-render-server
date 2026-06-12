use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result, anyhow};

use crate::{debug_artifacts::DebugArtifactStore, dsl::SceneDSL};

/// Metadata + raw bytes for a single asset.
#[derive(Debug, Clone)]
pub struct AssetData {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub original_name: String,
}

/// Thread-safe, clone-friendly in-memory asset cache keyed by `assetId`.
#[derive(Debug, Clone)]
pub struct AssetStore {
    inner: Arc<Mutex<HashMap<String, AssetData>>>,
    revision: Arc<AtomicU64>,
}

#[derive(Debug)]
pub struct LoadedNforge {
    pub scene: SceneDSL,
    pub asset_store: AssetStore,
    pub debug_artifacts: DebugArtifactStore,
}

impl Default for AssetStore {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            revision: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl AssetStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an asset. If the `asset_id` already exists, this is a no-op
    /// (content-addressed dedup).
    pub fn insert(&self, asset_id: impl Into<String>, data: AssetData) {
        let asset_id = asset_id.into();
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        if let std::collections::hash_map::Entry::Vacant(entry) = map.entry(asset_id) {
            entry.insert(data);
            self.revision.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Insert or replace an asset unconditionally.
    pub fn insert_or_replace(&self, asset_id: impl Into<String>, data: AssetData) {
        let asset_id = asset_id.into();
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        map.insert(asset_id, data);
        self.revision.fetch_add(1, Ordering::Relaxed);
    }

    /// Retrieve a clone of the asset data for the given id.
    pub fn get(&self, asset_id: &str) -> Option<AssetData> {
        let map = self.inner.lock().ok()?;
        map.get(asset_id).cloned()
    }

    /// Check if an asset exists without cloning its bytes.
    pub fn contains(&self, asset_id: &str) -> bool {
        self.inner
            .lock()
            .ok()
            .is_some_and(|map| map.contains_key(asset_id))
    }

    /// Remove an asset by id.
    pub fn remove(&self, asset_id: &str) -> Option<AssetData> {
        let removed = self.inner.lock().ok()?.remove(asset_id);
        if removed.is_some() {
            self.revision.fetch_add(1, Ordering::Relaxed);
        }
        removed
    }

    /// Return the subset of `ids` that are missing from the store.
    pub fn missing_ids(&self, ids: &[String]) -> Vec<String> {
        let Ok(map) = self.inner.lock() else {
            return ids.to_vec();
        };
        ids.iter()
            .filter(|id| !map.contains_key(id.as_str()))
            .cloned()
            .collect()
    }

    /// Clear all assets.
    pub fn clear(&self) {
        if let Ok(mut map) = self.inner.lock() {
            map.clear();
            self.revision.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// Load a `DynamicImage` from an asset id. Returns `None` if the asset is
    /// missing, or an error if the bytes cannot be decoded.
    pub fn load_image(&self, asset_id: &str) -> Result<Option<image::DynamicImage>> {
        let Some(data) = self.get(asset_id) else {
            return Ok(None);
        };
        let img = image::load_from_memory(&data.bytes)
            .with_context(|| format!("failed to decode image for asset '{asset_id}'"))?;
        Ok(Some(img))
    }
}

// ---------------------------------------------------------------------------
// Convenience loaders
// ---------------------------------------------------------------------------

/// Populate an `AssetStore` from a `SceneDSL`'s `assets` manifest, resolving
/// each `entry.path` relative to `base_dir`.
pub fn load_from_scene_dir(scene: &SceneDSL, base_dir: &Path) -> Result<AssetStore> {
    let store = AssetStore::new();
    for (asset_id, entry) in &scene.assets {
        let file_path = base_dir.join(&entry.path);
        let bytes = std::fs::read(&file_path).with_context(|| {
            format!(
                "failed to read asset '{}' at {}",
                asset_id,
                file_path.display()
            )
        })?;
        store.insert(
            asset_id.clone(),
            AssetData {
                bytes,
                mime_type: entry.mime_type.clone(),
                original_name: entry.original_name.clone(),
            },
        );
    }
    Ok(store)
}

/// Open a `.nforge` zip archive, extract `scene.json` and all `assets/*`,
/// return the parsed `SceneDSL` and a populated `AssetStore`.
pub fn load_from_nforge(nforge_path: &Path) -> Result<(SceneDSL, AssetStore)> {
    let loaded = load_from_nforge_with_debug_artifacts(nforge_path)?;
    Ok((loaded.scene, loaded.asset_store))
}

/// Open a `.nforge` archive and also hydrate debug artifacts.
pub fn load_from_nforge_with_debug_artifacts(nforge_path: &Path) -> Result<LoadedNforge> {
    let file = std::fs::File::open(nforge_path)
        .with_context(|| format!("failed to open .nforge at {}", nforge_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("failed to read zip archive {}", nforge_path.display()))?;

    // 1) Extract scene.json
    let scene_json = {
        let mut entry = archive
            .by_name("scene.json")
            .with_context(|| "missing scene.json in .nforge archive")?;
        let mut buf = String::new();
        entry
            .read_to_string(&mut buf)
            .context("failed to read scene.json from archive")?;
        buf
    };

    let mut scene: SceneDSL =
        serde_json::from_str(&scene_json).context("failed to parse scene.json from archive")?;
    crate::dsl::normalize_scene_defaults(&mut scene)?;

    // 2) Populate asset store from scene.assets manifest + archive entries
    let store = AssetStore::new();
    for (asset_id, entry) in &scene.assets {
        let zip_path = &entry.path;
        let mut zip_entry = archive.by_name(zip_path).with_context(|| {
            format!("missing asset '{zip_path}' in .nforge archive for asset id '{asset_id}'")
        })?;
        let mut bytes = Vec::with_capacity(zip_entry.size() as usize);
        zip_entry
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to read asset '{zip_path}' from archive"))?;
        store.insert(
            asset_id.clone(),
            AssetData {
                bytes,
                mime_type: entry.mime_type.clone(),
                original_name: entry.original_name.clone(),
            },
        );
    }

    // 3) Populate debug artifacts from the archive. The manifest lives in
    // scene.json while the payloads live under debug-artifacts/*.
    let mut debug_artifacts = DebugArtifactStore::default();
    let missing_debug_artifact_ids = debug_artifacts.sync_manifest(scene.debug_artifacts.clone());
    for artifact_id in missing_debug_artifact_ids {
        let Some(item) = scene
            .debug_artifacts
            .as_ref()
            .and_then(|manifest| manifest.items.get(artifact_id.as_str()))
            .cloned()
        else {
            continue;
        };
        let Ok(mut zip_entry) = archive.by_name(item.path.as_str()) else {
            continue;
        };
        if item.mime_type.starts_with("text/") {
            let mut text = String::new();
            if zip_entry.read_to_string(&mut text).is_ok() {
                debug_artifacts.upsert(item, Some(text));
            }
        } else {
            let mut bytes = Vec::with_capacity(zip_entry.size() as usize);
            if zip_entry.read_to_end(&mut bytes).is_ok() {
                debug_artifacts.upsert_bytes(item, bytes);
            }
        }
    }

    Ok(LoadedNforge {
        scene,
        asset_store: store,
        debug_artifacts,
    })
}

/// Update `scene.json` and `debug-artifacts/*` entries in an existing `.nforge`.
pub fn save_debug_artifacts_to_nforge(
    nforge_path: &Path,
    scene: &SceneDSL,
    debug_artifacts: &DebugArtifactStore,
) -> Result<()> {
    let file = std::fs::File::open(nforge_path)
        .with_context(|| format!("failed to open .nforge at {}", nforge_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("failed to read zip archive {}", nforge_path.display()))?;

    let mut files = BTreeMap::<String, Vec<u8>>::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("failed to read zip entry #{index}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to read zip entry {name}"))?;
        files.insert(name, bytes);
    }

    let mut next_scene = scene.clone();
    next_scene.debug_artifacts = debug_artifacts.export_manifest();
    files.insert(
        "scene.json".to_string(),
        serde_json::to_vec(&next_scene).context("failed to serialize scene.json")?,
    );

    if let Some(manifest) = debug_artifacts.export_manifest() {
        for item in manifest.items.values() {
            if let Some(bytes) = debug_artifacts.bytes(item.id.as_str()) {
                files.insert(item.path.clone(), bytes.to_vec());
            }
        }
    }

    let file_name = nforge_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("invalid .nforge file name: {}", nforge_path.display()))?;
    let tmp_path = nforge_path.with_file_name(format!(".{file_name}.tmp"));
    let tmp_file = std::fs::File::create(&tmp_path)
        .with_context(|| format!("failed to create temp archive {}", tmp_path.display()))?;
    let mut writer = zip::ZipWriter::new(tmp_file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in files {
        writer
            .start_file(name.as_str(), options)
            .with_context(|| format!("failed to start zip entry {name}"))?;
        writer
            .write_all(bytes.as_slice())
            .with_context(|| format!("failed to write zip entry {name}"))?;
    }
    writer
        .finish()
        .context("failed to finish updated .nforge archive")?;
    std::fs::rename(&tmp_path, nforge_path).with_context(|| {
        format!(
            "failed to replace {} with {}",
            nforge_path.display(),
            tmp_path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{AssetData, AssetStore};

    fn sample_asset(name: &str) -> AssetData {
        AssetData {
            bytes: vec![1, 2, 3],
            mime_type: "image/png".to_string(),
            original_name: name.to_string(),
        }
    }

    fn temp_nforge_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "node-forge-{name}-{}-{unique}.nforge",
            std::process::id()
        ))
    }

    fn write_test_nforge(
        path: &PathBuf,
        scene_json: &str,
        artifact_path: &str,
        artifact_text: &str,
    ) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("scene.json", options).unwrap();
        writer.write_all(scene_json.as_bytes()).unwrap();
        writer.start_file(artifact_path, options).unwrap();
        writer.write_all(artifact_text.as_bytes()).unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn revision_increments_on_mutations_only() {
        let store = AssetStore::new();
        assert_eq!(store.revision(), 0);

        store.insert("a", sample_asset("a.png"));
        assert_eq!(store.revision(), 1);

        store.insert("a", sample_asset("a-duplicate.png"));
        assert_eq!(store.revision(), 1);

        store.insert_or_replace("a", sample_asset("a-replaced.png"));
        assert_eq!(store.revision(), 2);

        let _ = store.remove("missing");
        assert_eq!(store.revision(), 2);

        let _ = store.remove("a");
        assert_eq!(store.revision(), 3);

        store.insert("b", sample_asset("b.png"));
        assert_eq!(store.revision(), 4);

        store.clear();
        assert_eq!(store.revision(), 5);
    }

    #[test]
    fn nforge_debug_artifacts_load_and_save_round_trip() {
        let path = temp_nforge_path("debug-artifacts");
        let artifact_id = "pass__Main__patch__default";
        let artifact_path = "debug-artifacts/pass__Main__patch__default/Main.patches.json";
        let initial_patch_text = r#"{"version":1,"patches":{"old":{}}}"#;
        let scene_json = serde_json::json!({
            "version": "1.0",
            "metadata": {
                "name": "debug artifact scene",
                "created": null,
                "modified": null
            },
            "nodes": [],
            "connections": [],
            "outputs": null,
            "groups": [],
            "assets": {},
            "debugArtifacts": {
                "version": 1,
                "items": {
                    artifact_id: {
                        "id": artifact_id,
                        "anchor": { "kind": "pass", "passName": "Main" },
                        "role": "patch",
                        "name": "Shortwire patches",
                        "mimeType": "text/plain",
                        "path": artifact_path,
                        "slotKey": "default"
                    }
                }
            }
        })
        .to_string();

        write_test_nforge(&path, &scene_json, artifact_path, initial_patch_text);

        let mut loaded = super::load_from_nforge_with_debug_artifacts(&path).unwrap();
        assert_eq!(
            loaded.debug_artifacts.pass_patches_text("Main"),
            Some(initial_patch_text)
        );

        let item = loaded
            .scene
            .debug_artifacts
            .as_ref()
            .unwrap()
            .items
            .get(artifact_id)
            .unwrap()
            .clone();
        let updated_patch_text = r#"{"version":1,"patches":{"new":{}}}"#;
        loaded
            .debug_artifacts
            .upsert(item, Some(updated_patch_text.to_string()));
        super::save_debug_artifacts_to_nforge(&path, &loaded.scene, &loaded.debug_artifacts)
            .unwrap();

        let reloaded = super::load_from_nforge_with_debug_artifacts(&path).unwrap();
        assert_eq!(
            reloaded.debug_artifacts.pass_patches_text("Main"),
            Some(updated_patch_text)
        );

        let _ = std::fs::remove_file(path);
    }
}
