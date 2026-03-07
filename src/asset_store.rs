use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result};

use crate::dsl::SceneDSL;

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

#[cfg(test)]
mod tests {
    use super::{AssetData, AssetStore};

    fn sample_asset(name: &str) -> AssetData {
        AssetData {
            bytes: vec![1, 2, 3],
            mime_type: "image/png".to_string(),
            original_name: name.to_string(),
        }
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
}
