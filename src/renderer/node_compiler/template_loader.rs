use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use node_forge_resource_store::{material_id_from_reference, ResourceIdentity};

static CACHE: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
static OVERRIDE_CACHE: OnceLock<RwLock<HashMap<PathBuf, String>>> = OnceLock::new();
static MATERIALS_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
static CACHE_GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Archive-local material sources belong to the document currently loaded
    /// by this render thread. Keeping them thread-local prevents concurrent
    /// archive compilation (notably the test runner) from replacing another
    /// document's shader sources midway through planning.
    static DOCUMENT_OVERRIDES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    /// Document overrides are thread-local, so their invalidation generation
    /// must be thread-local as well. A process-global counter lets one render
    /// thread spuriously invalidate another thread's otherwise identical plan.
    static DOCUMENT_OVERRIDE_GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// Returns the root directory for per-node WGSL material override files,
/// resolved once from `NODE_FORGE_MATERIALS_DIR` at first call.
///
/// Set by the editor (via env var when spawning the renderer) to the
/// `<autosave-root>/resources/materials/` path. Returns None if unset, in which case
/// `wgslOverride` fields silently fall back to bundled templates.
pub fn materials_root() -> Option<&'static Path> {
    MATERIALS_ROOT
        .get_or_init(|| std::env::var_os("NODE_FORGE_MATERIALS_DIR").map(PathBuf::from))
        .as_deref()
}

/// Resolve a logical document reference through the same mapping as the editor.
/// IDs never become path components, including when the ID contains separators.
pub fn resolve_override_path(rel: &str) -> Option<PathBuf> {
    resolve_override_path_in(materials_root(), rel)
}

fn resolve_override_path_in(root: Option<&Path>, reference: &str) -> Option<PathBuf> {
    if let Some(id) = material_id_from_reference(reference) {
        let relative = ResourceIdentity::material(id).ok()?.relative_path();
        return Some(match root {
            Some(root) => root.join(relative.file_name()?),
            None => relative,
        });
    }
    // Explicit file overrides are also used by the standalone renderer.
    Some(PathBuf::from(reference))
}

/// Explicit document ownership for hosts scheduling several scenes on one thread.
#[derive(Default)]
pub struct DocumentMaterials {
    overrides: HashMap<String, String>,
    generation: u64,
    _thread_owned: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl DocumentMaterials {
    pub fn with<R>(&mut self, operation: impl FnOnce() -> R) -> R {
        struct Restore<'a>(&'a mut DocumentMaterials);
        impl Drop for Restore<'_> {
            fn drop(&mut self) { self.0.swap_active(); }
        }
        self.swap_active();
        let _restore = Restore(self);
        operation()
    }

    fn swap_active(&mut self) {
        DOCUMENT_OVERRIDES.with(|active| std::mem::swap(&mut *active.borrow_mut(), &mut self.overrides));
        DOCUMENT_OVERRIDE_GENERATION.with(|active| {
            let previous = active.replace(self.generation);
            self.generation = previous;
        });
    }
}

pub fn install_document_overrides(overrides: impl IntoIterator<Item = (String, String)>) {
    DOCUMENT_OVERRIDES.with(|current| {
        let mut current = current.borrow_mut();
        current.clear();
        current.extend(overrides.into_iter().filter_map(|(id, source)| {
            let path = ResourceIdentity::material(&id).ok()?.relative_path();
            Some((path.file_name()?.to_str()?.to_owned(), source))
        }));
    });
    DOCUMENT_OVERRIDE_GENERATION.with(|generation| {
        generation.set(generation.get().wrapping_add(1));
    });
}

/// Remote playback cannot resolve working-copy files on the Editor's computer.
pub fn validate_document_overrides<'a>(scene: &crate::dsl::SceneDSL, ids: impl IntoIterator<Item = &'a str>) -> anyhow::Result<()> {
    let ids: std::collections::HashSet<_> = ids.into_iter().collect();
    for node in scene.nodes.iter().chain(scene.groups.iter().flat_map(|group| &group.nodes)) {
        if let Some(reference) = &node.wgsl_override {
            let id = material_id_from_reference(reference)
                .ok_or_else(|| anyhow::anyhow!("Node '{}' uses a non-document material reference '{reference}'", node.id))?;
            if !ids.contains(id) { anyhow::bail!("Material '{id}' required by node '{}' was not transferred", node.id); }
        }
    }
    Ok(())
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

    #[cfg(any(feature = "web-core", feature = "android-core", feature = "embedded-templates"))]
    let content = embedded_template(name)
        .unwrap_or_else(|| panic!("unknown embedded WGSL template '{name}'"))
        .to_string();
    #[cfg(all(feature = "native", not(any(feature = "android-core", feature = "embedded-templates"))))]
    let content = {
        let path = templates_dir().join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read template {}: {e}", path.display()))
    };

    let mut c = cache().write().unwrap();
    c.insert(name.to_owned(), content.clone());
    content
}

/// OKLab's matrices operate on linear sRGB. Specialize both boundaries for the
/// scene's working primaries; never clamp the intermediate sRGB values, since
/// Display P3 colors can lie outside that gamut. The caller's final clamp stays
/// in working space. An sRGB scene retains the original shader expressions.
pub(super) fn specialize_oklab_template(
    template: &str,
    working: crate::color::WorkingColorSpace,
) -> String {
    use crate::color::WorkingColorSpace::Srgb;
    template
        .replace(
            "{{oklab_input_rgb}}",
            &working.wgsl_transform(Srgb, "color.rgb / color.a"),
        )
        .replace(
            "{{oklab_output_rgb}}",
            &Srgb.wgsl_transform(working, "mapped_rgb"),
        )
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
        if let Some(resource_file) = path.file_name().and_then(|value| value.to_str())
            && let Some(content) =
                DOCUMENT_OVERRIDES.with(|overrides| overrides.borrow().get(resource_file).cloned())
        {
            return content;
        }
        #[cfg(any(feature = "web-core", feature = "android-core", feature = "embedded-templates"))]
        return load_template(template_name);
        #[cfg(all(feature = "native", not(any(feature = "android-core", feature = "embedded-templates"))))]
        {
            let c = override_cache().read().unwrap();
            if let Some(content) = c.get(path) {
                return content.clone();
            }
        }

        #[cfg(all(feature = "native", not(any(feature = "android-core", feature = "embedded-templates"))))]
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

#[cfg(any(feature = "web-core", feature = "android-core", feature = "embedded-templates"))]
fn embedded_template(name: &str) -> Option<&'static str> {
    Some(match name {
        "glass_material_fragment.wgsl" => include_str!("templates/glass_material_fragment.wgsl"),
        "hyperos_glass_material_fragment.wgsl" => {
            include_str!("templates/hyperos_glass_material_fragment.wgsl")
        }
        "intelligent_light.wgsl" => include_str!("templates/intelligent_light.wgsl"),
        "luminance_curve_lab.wgsl" => include_str!("templates/luminance_curve_lab.wgsl"),
        "luminance_curve_rgb.wgsl" => include_str!("templates/luminance_curve_rgb.wgsl"),
        "mesh_gradient_fragment.wgsl" => include_str!("templates/mesh_gradient_fragment.wgsl"),
        "mesh_gradient_vertex.wgsl" => include_str!("templates/mesh_gradient_vertex.wgsl"),
        "sdf2d.wgsl" => include_str!("templates/sdf2d.wgsl"),
        "sdf2d_bevel.wgsl" => include_str!("templates/sdf2d_bevel.wgsl"),
        "shader_material_default.wgsl" => include_str!("templates/shader_material_default.wgsl"),
        _ => return None,
    })
}

pub fn invalidate_cache() {
    if let Some(c) = CACHE.get() {
        c.write().unwrap().clear();
    }
    if let Some(c) = OVERRIDE_CACHE.get() {
        c.write().unwrap().clear();
    }
    CACHE_GENERATION.fetch_add(1, Ordering::Relaxed);
}

pub fn generation() -> [u64; 2] {
    [
        CACHE_GENERATION.load(Ordering::Relaxed),
        DOCUMENT_OVERRIDE_GENERATION.with(Cell::get),
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    #[test]
    fn editor_working_sources_and_archive_sources_use_the_same_identity() {
        use node_forge_resource_store::ResourceStore;
        let root = std::env::temp_dir().join(format!("nforge-render-resource-test-{}", std::process::id()));
        let store = ResourceStore::new(&root);
        let ids = ["pointer.localize", "Material / 中文", "A", "a", " a ", "../outside", "C:\\CON"];
        install_document_overrides([]);
        for id in ids {
            let identity = ResourceIdentity::material(id).unwrap();
            let written = store.write(&identity, &format!("disk:{id}")).unwrap();
            let resolved = resolve_override_path_in(Some(&root.join("materials")), &format!("materials/{id}.wgsl")).unwrap();
            assert_eq!(resolved, written);
            assert_eq!(load_template_with_override(Some(&resolved), "shader_material_default.wgsl"), format!("disk:{id}"));
        }
        install_document_overrides(ids.map(|id| (id.to_string(), format!("archive:{id}"))));
        for id in ids {
            let resolved = resolve_override_path_in(None, &format!("materials/{id}.wgsl")).unwrap();
            assert_eq!(load_template_with_override(Some(&resolved), "shader_material_default.wgsl"), format!("archive:{id}"));
        }
        install_document_overrides([]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn document_overrides_are_isolated_between_render_threads() {
        let barrier = Arc::new(Barrier::new(2));
        let handles = ["thread-a", "thread-b"].map(|source| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                install_document_overrides([("SharedNode".to_string(), source.to_string())]);
                barrier.wait();
                load_template_with_override(
                    resolve_override_path("materials/SharedNode.wgsl").as_deref(),
                    "shader_material_default.wgsl",
                )
            })
        });

        let values = handles.map(|handle| handle.join().expect("render thread should finish"));
        assert_eq!(values, ["thread-a", "thread-b"]);
    }

    #[test]
    fn document_override_generation_is_isolated_between_render_threads() {
        let main_thread_generation = generation()[1];
        let child_generation = std::thread::spawn(|| {
            let before = generation()[1];
            install_document_overrides([("SharedNode".to_string(), "child".to_string())]);
            (before, generation()[1])
        })
        .join()
        .expect("render thread should finish");

        assert_eq!(child_generation.1, child_generation.0.wrapping_add(1));
        assert_eq!(generation()[1], main_thread_generation);
    }
}
