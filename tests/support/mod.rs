#![allow(dead_code)]

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, OnceLock},
};

use node_forge_render_server::{
    asset_store::{self, AssetStore},
    dsl::SceneDSL,
};

pub fn render_case_dir(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("render");
    for group in ["editor-examples", "renderer-only"] {
        let candidate = root.join(group).join(name);
        if candidate.is_dir() {
            return candidate;
        }
    }
    panic!(
        "render case {name:?} was not found under {}",
        root.display()
    );
}

pub fn render_case_archive(name: &str) -> PathBuf {
    render_case_dir(name).join("scene.nforge")
}

pub fn load_render_case(name: &str) -> (SceneDSL, AssetStore) {
    let archive = render_case_archive(name);
    asset_store::load_from_nforge(&archive)
        .unwrap_or_else(|error| panic!("failed to load {}: {error:#}", archive.display()))
}

pub fn load_render_case_scene(name: &str) -> SceneDSL {
    load_render_case(name).0
}

/// Archive loading installs the document's Graph Function resources in a
/// process-global runtime registry. Integration tests in one binary must keep
/// load + compile/evaluation atomic with respect to one another.
pub fn function_registry_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn expected_path(case_dir: &Path, relative: impl AsRef<Path>) -> PathBuf {
    case_dir.join("expected").join(relative)
}
