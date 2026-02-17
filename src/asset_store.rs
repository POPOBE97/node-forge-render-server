use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};

use crate::dsl::SceneDSL;

/// Metadata + raw bytes for a single asset.
#[derive(Debug, Clone)]
pub struct AssetData {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub original_name: String,
}

/// Thread-safe, clone-friendly in-memory asset cache keyed by `assetId`.
#[derive(Debug, Clone, Default)]
pub struct AssetStore {
    inner: Arc<Mutex<HashMap<String, AssetData>>>,
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
        map.entry(asset_id).or_insert(data);
    }

    /// Insert or replace an asset unconditionally.
    pub fn insert_or_replace(&self, asset_id: impl Into<String>, data: AssetData) {
        let asset_id = asset_id.into();
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        map.insert(asset_id, data);
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
        self.inner.lock().ok()?.remove(asset_id)
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
        }
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

    Ok((scene, store))
}
