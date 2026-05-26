use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

static CACHE: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn cache() -> &'static RwLock<HashMap<String, String>> {
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
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

pub fn invalidate_cache() {
    if let Some(c) = CACHE.get() {
        c.write().unwrap().clear();
    }
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}
