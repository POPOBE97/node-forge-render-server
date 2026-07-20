use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

static CACHE: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
static OVERRIDE_CACHE: OnceLock<RwLock<HashMap<PathBuf, String>>> = OnceLock::new();
static DOCUMENT_OVERRIDES: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
static MATERIALS_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Returns the root directory for per-node WGSL material override files,
/// resolved once from `NODE_FORGE_MATERIALS_DIR` at first call.
///
/// Set by the editor (via env var when spawning the renderer) to the
/// `<autosave-root>/materials/` path. Returns None if unset, in which case
/// `wgslOverride` fields silently fall back to bundled templates.
pub fn materials_root() -> Option<&'static Path> {
    MATERIALS_ROOT
        .get_or_init(|| std::env::var_os("NODE_FORGE_MATERIALS_DIR").map(PathBuf::from))
        .as_deref()
}

/// Resolve a `wgslOverride` relative path (e.g. "materials/<id>.wgsl") to
/// an absolute path under the materials root, if one is configured. The
/// returned path may not yet exist on disk.
pub fn resolve_override_path(rel: &str) -> Option<PathBuf> {
    let Some(root) = materials_root() else {
        return Some(PathBuf::from(rel));
    };
    // The DSL stores `materials/<id>.wgsl`; strip the leading prefix so we
    // don't end up at `<materials_root>/materials/<id>.wgsl`.
    let stripped = rel.strip_prefix("materials/").unwrap_or(rel);
    Some(root.join(stripped))
}

fn document_overrides() -> &'static RwLock<HashMap<String, String>> {
    DOCUMENT_OVERRIDES.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn install_document_overrides(overrides: impl IntoIterator<Item = (String, String)>) {
    let Ok(mut current) = document_overrides().write() else {
        return;
    };
    current.clear();
    current.extend(overrides);
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

fn cache() -> &'static RwLock<HashMap<String, String>> {
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn override_cache() -> &'static RwLock<HashMap<PathBuf, String>> {
    OVERRIDE_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn templates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("renderer")
        .join("node_compiler")
        .join("templates")
}

pub fn load_template(name: &str) -> String {
    {
        let c = cache().read().unwrap();
        if let Some(content) = c.get(name) {
            return content.clone();
        }
    }

    let path = templates_dir().join(name);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read template {}: {e}", path.display()));

    let mut c = cache().write().unwrap();
    c.insert(name.to_owned(), content.clone());
    content
}

/// Load a node-specific WGSL override file if `override_abs_path` points to
/// a readable file; otherwise fall back to the bundled `template_name`.
///
/// Override content is cached separately, keyed by absolute path. Cache is
/// invalidated globally by [`invalidate_cache`] (the existing HMR watcher
/// hook), so editing the override on disk and saving will trigger a rebuild.
pub fn load_template_with_override(
    override_abs_path: Option<&Path>,
    template_name: &str,
) -> String {
    if let Some(path) = override_abs_path {
        if let Some(node_id) = path.file_stem().and_then(|value| value.to_str())
            && let Ok(overrides) = document_overrides().read()
            && let Some(content) = overrides.get(node_id)
        {
            return content.clone();
        }
        {
            let c = override_cache().read().unwrap();
            if let Some(content) = c.get(path) {
                return content.clone();
            }
        }

        match std::fs::read_to_string(path) {
            Ok(content) => {
                let mut c = override_cache().write().unwrap();
                c.insert(path.to_path_buf(), content.clone());
                return content;
            }
            Err(e) => {
                eprintln!(
                    "[material-override] failed to read {}: {e}; falling back to bundled template '{template_name}'",
                    path.display()
                );
            }
        }
    }

    load_template(template_name)
}

pub fn invalidate_cache() {
    if let Some(c) = CACHE.get() {
        c.write().unwrap().clear();
    }
    if let Some(c) = OVERRIDE_CACHE.get() {
        c.write().unwrap().clear();
    }
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}
