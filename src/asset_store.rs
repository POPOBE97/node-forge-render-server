use std::collections::HashMap;
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result};

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
    // A non-.nforge scene must not inherit document-local material overrides
    // from a previously loaded SQLite document in the same process.
    crate::renderer::node_compiler::template_loader::install_document_overrides(std::iter::empty());
    crate::state_machine::mutation_function::clear_document_functions();
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

/// Open a SQLite `.nforge` document and return its SceneDSL projection and assets.
pub fn load_from_nforge(nforge_path: &Path) -> Result<(SceneDSL, AssetStore)> {
    let loaded = load_from_nforge_with_debug_artifacts(nforge_path)?;
    Ok((loaded.scene, loaded.asset_store))
}

/// Open a SQLite `.nforge` document and also hydrate debug artifacts.
pub fn load_from_nforge_with_debug_artifacts(nforge_path: &Path) -> Result<LoadedNforge> {
    crate::nforge::load(nforge_path)
}

/// Transactionally replace debug artifacts in a SQLite `.nforge` document.
pub fn save_debug_artifacts_to_nforge(
    nforge_path: &Path,
    _scene: &SceneDSL,
    debug_artifacts: &DebugArtifactStore,
) -> Result<()> {
    crate::nforge::save_debug_artifacts(nforge_path, debug_artifacts)
}

#[cfg(test)]
mod tests {
    use std::{
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
        _artifact_path: &str,
        artifact_text: &str,
    ) {
        let scene: crate::dsl::SceneDSL = serde_json::from_str(scene_json).unwrap();
        crate::nforge::initialize_test_document(path, &scene).unwrap();
        let mut artifacts = crate::debug_artifacts::DebugArtifactStore::default();
        artifacts.sync_manifest(scene.debug_artifacts.clone());
        let item = scene
            .debug_artifacts
            .as_ref()
            .and_then(|manifest| manifest.items.values().next())
            .unwrap()
            .clone();
        artifacts.upsert(item, Some(artifact_text.to_string()));
        crate::nforge::save_debug_artifacts(path, &artifacts).unwrap();
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

    #[test]
    fn legacy_zip_nforge_is_rejected_with_a_clear_error() {
        let path = temp_nforge_path("legacy-zip");
        std::fs::write(&path, b"PK\x03\x04legacy").unwrap();
        let error = super::load_from_nforge(&path).unwrap_err();
        assert!(
            error.to_string().contains("legacy ZIP .nforge unsupported"),
            "unexpected error: {error:#}"
        );
        let _ = std::fs::remove_file(path);
    }
}
