use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::dsl::DebugArtifactItem;
use crate::ui::pass_debug::artifacts::{
    REFERENCE_WORKSPACE_VERSION, ReferencePatchesPayload, ReferenceWorkspaceManifest,
    ReferenceWorkspaceManifestFile, ShortwireNodePatch, debug_artifact_content_hash,
    pass_reference_file_artifact_id, pass_reference_file_artifact_item,
    reference_patches_artifact_item, reference_workspace_artifact_item,
};
use crate::ui::pass_debug::file_io::ReferenceFileRead;
use crate::ui::pass_debug::patch::{ShortwireHunk, apply_hunks, compute_hunks};
use crate::ui::pass_debug::shader_document::hash_source;

pub(crate) struct ReferenceArtifactText<'a> {
    pub(crate) artifact_id: &'a str,
    pub(crate) name: &'a str,
    pub(crate) text: &'a str,
}

pub(crate) enum ReferenceArtifactRestorePlan {
    None,
    Loaded {
        state: ReferenceWorkspaceState,
        migrated_legacy: bool,
    },
    ReadManifestLocalFiles(ReferenceManifestLocalReadRequest),
}

pub(crate) struct ReferenceManifestLocalReadRequest {
    pub(crate) manifest: ReferenceWorkspaceManifest,
    pub(crate) root_path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReferenceReloadPlan {
    pub(crate) root: PathBuf,
    pub(crate) root_label: String,
    pub(crate) selected_file: Option<String>,
    pub(crate) single_file: bool,
    pub(crate) now_secs: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct ReferenceSyncPlan {
    pub(crate) root_path: Option<String>,
    pub(crate) root_label: String,
    pub(crate) selected_file: Option<String>,
    pub(crate) skipped_files: usize,
    pub(crate) manifest_dirty: bool,
    pub(crate) files: Vec<ReferenceSyncFileSnapshot>,
}

#[derive(Clone, Debug)]
pub(crate) struct ReferenceSyncedFile {
    pub(crate) relative_path: String,
    pub(crate) source: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ReferenceSyncCompletion {
    pub(crate) plan: ReferenceSyncPlan,
    pub(crate) artifacts: Vec<(DebugArtifactItem, String)>,
    pub(crate) synced_files: Vec<ReferenceSyncedFile>,
    pub(crate) write_errors: Vec<String>,
    pub(crate) emitted_manifest: bool,
}

type ReferenceWorkspacePatchesPayload = ReferencePatchesPayload<ShortwireNodePatch>;

pub(crate) fn reference_workspace_artifact_from_sync_plan(
    pass_name: &str,
    plan: &ReferenceSyncPlan,
) -> Option<(DebugArtifactItem, String)> {
    let manifest = ReferenceWorkspaceManifest {
        version: REFERENCE_WORKSPACE_VERSION,
        root_path: plan.root_path.clone(),
        root_label: plan.root_label.clone(),
        selected_file: plan.selected_file.clone(),
        files: plan
            .files
            .iter()
            .map(|file| ReferenceWorkspaceManifestFile {
                relative_path: file.relative_path.clone(),
                artifact_id: file.artifact_id.clone(),
                content_hash: file.content_hash.clone(),
                size: file.size,
            })
            .collect(),
        skipped_files: plan.skipped_files,
    };
    let content_text = serde_json::to_string(&manifest).ok()?;
    let item = reference_workspace_artifact_item(pass_name, &content_text);
    Some((item, content_text))
}

pub(crate) fn reference_file_artifact_from_sync_file(
    pass_name: &str,
    file: &ReferenceSyncFileSnapshot,
) -> (DebugArtifactItem, String) {
    let item = pass_reference_file_artifact_item(
        pass_name,
        &file.relative_path,
        &file.artifact_id,
        file.size,
        file.content_hash.clone(),
    );
    (item, file.source.clone())
}

#[derive(Clone, Debug)]
pub(crate) struct ReferenceSyncFileSnapshot {
    pub(crate) relative_path: String,
    pub(crate) artifact_id: String,
    pub(crate) source: String,
    pub(crate) loaded_source: String,
    pub(crate) size: u64,
    pub(crate) content_hash: String,
}

impl ReferenceSyncFileSnapshot {
    pub(crate) fn is_dirty(&self) -> bool {
        self.source != self.loaded_source
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ReferenceWorkspaceFile {
    pub(crate) relative_path: String,
    pub(crate) artifact_id: String,
    pub(crate) source: String,
    pub(crate) loaded_source: String,
}

impl From<ReferenceFileRead> for ReferenceWorkspaceFile {
    fn from(file: ReferenceFileRead) -> Self {
        Self {
            relative_path: file.relative_path,
            artifact_id: file.artifact_id,
            source: file.source,
            loaded_source: file.loaded_source,
        }
    }
}

impl ReferenceWorkspaceFile {
    pub(crate) fn size(&self) -> u64 {
        self.source.len() as u64
    }

    pub(crate) fn content_hash(&self) -> String {
        debug_artifact_content_hash(self.source.as_bytes())
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.source != self.loaded_source
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ReferenceShortwireLocalFile {
    pub(crate) path: PathBuf,
    pub(crate) restore_source: String,
    pub(crate) wrote_patch: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReferenceShortwireLocalReadRequest {
    pub(crate) path: PathBuf,
    pub(crate) write_after_read: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReferenceShortwireFileWrite {
    pub(crate) path: PathBuf,
    pub(crate) content: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReferenceShortwireFileRestore {
    pub(crate) path: PathBuf,
    pub(crate) content: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReferenceShortwireEnterResult {
    pub(crate) local_read: Option<ReferenceShortwireLocalReadRequest>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReferenceShortwireRestoreResult {
    pub(crate) restore_file: Option<ReferenceShortwireFileRestore>,
    pub(crate) editor_restored: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ReferenceWorkspaceState {
    pub(crate) root_path: Option<String>,
    pub(crate) root_label: String,
    pub(crate) selected_file: Option<String>,
    pub(crate) files: Vec<ReferenceWorkspaceFile>,
    pub(crate) editor_source: String,
    pub(crate) sync_due_secs: Option<f64>,
    pub(crate) last_status: Option<String>,
    pub(crate) skipped_files: usize,
    pub(crate) manifest_dirty: bool,
    pub(crate) pre_shortwire_source: Option<String>,
    pub(crate) shortwire_active_key: Option<String>,
    pub(crate) shortwire_base_source: Option<String>,
    pub(crate) pending_shortwire_patch: Option<(String, Vec<ShortwireHunk>)>,
    pub(crate) local_shortwire_file: Option<ReferenceShortwireLocalFile>,
    pub(crate) reference_patches: HashMap<String, ShortwireNodePatch>,
    pub(crate) reference_patches_dirty: bool,
}

impl Default for ReferenceWorkspaceState {
    fn default() -> Self {
        Self {
            root_path: None,
            root_label: "Reference".to_string(),
            selected_file: None,
            files: Vec::new(),
            editor_source: String::new(),
            sync_due_secs: None,
            last_status: None,
            skipped_files: 0,
            manifest_dirty: false,
            pre_shortwire_source: None,
            shortwire_active_key: None,
            shortwire_base_source: None,
            pending_shortwire_patch: None,
            local_shortwire_file: None,
            reference_patches: HashMap::new(),
            reference_patches_dirty: false,
        }
    }
}

impl ReferenceWorkspaceState {
    fn selected_file_index(&self) -> Option<usize> {
        let selected = self.selected_file.as_deref()?;
        self.files
            .iter()
            .position(|file| file.relative_path == selected)
    }

    pub(crate) fn selected_file(&self) -> Option<&ReferenceWorkspaceFile> {
        self.selected_file_index()
            .and_then(|index| self.files.get(index))
    }

    pub(crate) fn selected_file_mut(&mut self) -> Option<&mut ReferenceWorkspaceFile> {
        let index = self.selected_file_index()?;
        self.files.get_mut(index)
    }

    pub(crate) fn selected_local_path(&self) -> Option<PathBuf> {
        let root_path = self.root_path.as_deref()?;
        let selected_file = self.selected_file.as_deref()?;
        Some(PathBuf::from(root_path).join(selected_file))
    }

    pub(crate) fn selected_file_dirty(&self) -> bool {
        self.selected_file()
            .is_some_and(ReferenceWorkspaceFile::is_dirty)
    }

    pub(crate) fn has_dirty_files(&self) -> bool {
        self.files.iter().any(ReferenceWorkspaceFile::is_dirty)
    }

    pub(crate) fn has_content(&self) -> bool {
        !self.files.is_empty()
    }

    pub(crate) fn commit_editor_to_selected(&mut self) {
        let editor_source = self.editor_source.clone();
        if let Some(file) = self.selected_file_mut() {
            file.source = editor_source;
        }
    }

    pub(crate) fn select_file(&mut self, relative_path: &str) -> bool {
        if self.shortwire_active_key.is_some() {
            return false;
        }
        self.commit_editor_to_selected();
        let Some(file) = self
            .files
            .iter()
            .find(|file| file.relative_path == relative_path)
        else {
            return false;
        };
        self.selected_file = Some(file.relative_path.clone());
        self.editor_source = file.source.clone();
        self.manifest_dirty = true;
        true
    }

    pub(crate) fn replace_files(
        &mut self,
        root_path: Option<String>,
        root_label: String,
        files: Vec<ReferenceWorkspaceFile>,
        selected_file: Option<String>,
        skipped_files: usize,
        mark_dirty: bool,
    ) {
        self.root_path = root_path;
        self.root_label = root_label;
        self.files = files;
        self.files
            .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        self.skipped_files = skipped_files;
        self.selected_file = selected_file
            .filter(|selected| {
                self.files
                    .iter()
                    .any(|file| &file.relative_path == selected)
            })
            .or_else(|| self.files.first().map(|file| file.relative_path.clone()));
        self.editor_source = self
            .selected_file()
            .map(|file| file.source.clone())
            .unwrap_or_default();
        self.sync_due_secs = None;
        self.last_status = None;
        self.manifest_dirty = mark_dirty;
        self.pre_shortwire_source = None;
        self.shortwire_active_key = None;
        self.shortwire_base_source = None;
        self.pending_shortwire_patch = None;
    }

    pub(crate) fn build_sync_plan(&self) -> ReferenceSyncPlan {
        ReferenceSyncPlan {
            root_path: self.root_path.clone(),
            root_label: self.root_label.clone(),
            selected_file: self.selected_file.clone(),
            skipped_files: self.skipped_files,
            manifest_dirty: self.manifest_dirty,
            files: self
                .files
                .iter()
                .map(|file| ReferenceSyncFileSnapshot {
                    relative_path: file.relative_path.clone(),
                    artifact_id: file.artifact_id.clone(),
                    source: file.source.clone(),
                    loaded_source: file.loaded_source.clone(),
                    size: file.size(),
                    content_hash: file.content_hash(),
                })
                .collect(),
        }
    }

    pub(crate) fn prepare_reload(&mut self, now_secs: f64) -> Option<ReferenceReloadPlan> {
        if self.shortwire_active_key.is_some() {
            self.last_status = Some("Close shortwire before reloading".to_string());
            return None;
        }
        let Some(root_path) = self.root_path.clone() else {
            self.last_status = Some("No local path to reload".to_string());
            return None;
        };
        let single_file = self.files.len() <= 1;
        let selected_file = self.selected_file.clone();
        if single_file && selected_file.is_none() {
            self.last_status = Some("No selected file to reload".to_string());
            return None;
        }
        self.last_status = Some("Reloading...".to_string());
        Some(ReferenceReloadPlan {
            root: PathBuf::from(root_path),
            root_label: self.root_label.clone(),
            selected_file,
            single_file,
            now_secs,
        })
    }

    pub(crate) fn apply_folder_import_result(
        &mut self,
        path: &Path,
        now_secs: f64,
        result: Result<(Vec<ReferenceFileRead>, usize), String>,
    ) -> bool {
        if self.shortwire_active_key.is_some() {
            self.last_status = Some("Close shortwire before opening a folder".to_string());
            return false;
        }
        match result {
            Ok((files, skipped_files)) => {
                if files.is_empty() {
                    self.last_status = Some("No UTF-8 text files found".to_string());
                    return false;
                }
                self.replace_files(
                    Some(path.to_string_lossy().to_string()),
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Reference folder")
                        .to_string(),
                    files.into_iter().map(Into::into).collect(),
                    None,
                    skipped_files,
                    true,
                );
                self.sync_due_secs = Some(now_secs);
                self.last_status = Some(format!("Folder imported ({skipped_files} skipped)"));
                true
            }
            Err(error) => {
                self.last_status = Some(error);
                false
            }
        }
    }

    pub(crate) fn mark_reload_missing_path(&mut self) {
        self.last_status = Some("Local path missing — keeping archive snapshot".to_string());
    }

    pub(crate) fn restore_reference_patches_from_text(&mut self, text: Option<&str>) {
        if self.reference_patches_dirty {
            return;
        }
        let Some(text) = text else {
            return;
        };
        let Ok(payload) = serde_json::from_str::<ReferenceWorkspacePatchesPayload>(text) else {
            return;
        };
        if payload.version != REFERENCE_WORKSPACE_VERSION {
            return;
        }
        self.reference_patches = payload.patches;
    }

    pub(crate) fn reference_patches_artifact(
        &self,
        pass_name: &str,
    ) -> Option<(DebugArtifactItem, String)> {
        let payload = ReferenceWorkspacePatchesPayload {
            version: REFERENCE_WORKSPACE_VERSION,
            patches: self.reference_patches.clone(),
        };
        let content_text = serde_json::to_string(&payload).ok()?;
        let item = reference_patches_artifact_item(pass_name, &content_text);
        Some((item, content_text))
    }

    pub(crate) fn take_reference_patches_dirty_artifact(
        &mut self,
        pass_name: &str,
    ) -> Option<(DebugArtifactItem, String)> {
        if !self.reference_patches_dirty {
            return None;
        }
        self.reference_patches_dirty = false;
        self.reference_patches_artifact(pass_name)
    }

    pub(crate) fn enter_shortwire(
        &mut self,
        row_patch_key: &str,
    ) -> Option<ReferenceShortwireEnterResult> {
        if self.shortwire_active_key.is_some() {
            return None;
        }
        self.commit_editor_to_selected();
        let file = self.selected_file()?;
        let relative_path = file.relative_path.clone();
        let base_source = file.source.clone();
        let patch_key = reference_shortwire_patch_key(&relative_path, row_patch_key);
        let mut draft = base_source.clone();
        let mut applied_stored_patch = false;

        if let Some(patch) = self.reference_patches.get(&patch_key).cloned() {
            match apply_hunks(&base_source, &patch.hunks) {
                Ok(patched) => {
                    draft = patched;
                    applied_stored_patch = true;
                }
                Err(_) => {
                    self.reference_patches.remove(&patch_key);
                    self.reference_patches_dirty = true;
                    self.last_status = Some("Reference patch outdated — removed".to_string());
                }
            }
        }

        let local_path = self.selected_local_path();
        self.pre_shortwire_source = Some(base_source.clone());
        self.shortwire_active_key = Some(patch_key);
        self.shortwire_base_source = Some(base_source);
        self.pending_shortwire_patch = None;
        self.local_shortwire_file = None;
        self.editor_source = draft;
        Some(ReferenceShortwireEnterResult {
            local_read: local_path.map(|path| ReferenceShortwireLocalReadRequest {
                path,
                write_after_read: applied_stored_patch,
            }),
        })
    }

    pub(crate) fn apply_shortwire_local_snapshot(
        &mut self,
        path: PathBuf,
        result: Result<String, String>,
        write_after_read: bool,
    ) -> Option<ReferenceShortwireFileWrite> {
        if self.shortwire_active_key.is_none() {
            return None;
        }
        match result {
            Ok(restore_source) => {
                self.local_shortwire_file = Some(ReferenceShortwireLocalFile {
                    path,
                    restore_source,
                    wrote_patch: false,
                });
                if write_after_read {
                    self.shortwire_write_request()
                } else {
                    None
                }
            }
            Err(error) => {
                self.last_status = Some(error);
                None
            }
        }
    }

    pub(crate) fn prepare_shortwire_save(&mut self) {
        let Some(patch_key) = self.shortwire_active_key.clone() else {
            return;
        };
        let Some(base_source) = self.shortwire_base_source.clone() else {
            return;
        };
        let hunks = compute_hunks(&base_source, &self.editor_source);
        self.pending_shortwire_patch = Some((patch_key, hunks));
    }

    pub(crate) fn commit_pending_shortwire_after_left_apply(&mut self) -> bool {
        let Some((patch_key, hunks)) = self.pending_shortwire_patch.take() else {
            return false;
        };
        let should_write_local_file = !hunks.is_empty()
            || self
                .local_shortwire_file
                .as_ref()
                .is_some_and(|local| local.wrote_patch);
        if hunks.is_empty() {
            self.reference_patches.remove(&patch_key);
        } else {
            let base_hash = self
                .shortwire_base_source
                .as_deref()
                .map(hash_source)
                .unwrap_or_else(|| hash_source(""));
            self.reference_patches.insert(
                patch_key,
                ShortwireNodePatch {
                    hunks,
                    base_source_hash: base_hash,
                    reference_image: None,
                    diff_result: None,
                },
            );
        }
        self.reference_patches_dirty = true;
        should_write_local_file
    }

    pub(crate) fn commit_pending_shortwire_without_left_apply(&mut self) {
        let Some((patch_key, hunks)) = self.pending_shortwire_patch.take() else {
            return;
        };
        if hunks.is_empty() {
            return;
        }
        let base_hash = self
            .shortwire_base_source
            .as_deref()
            .map(hash_source)
            .unwrap_or_else(|| hash_source(""));
        self.reference_patches.insert(
            patch_key,
            ShortwireNodePatch {
                hunks,
                base_source_hash: base_hash,
                reference_image: None,
                diff_result: None,
            },
        );
        self.reference_patches_dirty = true;
    }

    pub(crate) fn shortwire_write_request(&self) -> Option<ReferenceShortwireFileWrite> {
        let path = self.local_shortwire_file.as_ref()?.path.clone();
        Some(ReferenceShortwireFileWrite {
            path,
            content: self.editor_source.clone(),
        })
    }

    pub(crate) fn apply_shortwire_local_write_result(
        &mut self,
        path: &Path,
        result: Result<(), String>,
    ) {
        match result {
            Ok(()) => {
                if let Some(local) = self.local_shortwire_file.as_mut()
                    && local.path == path
                {
                    local.wrote_patch = true;
                }
                self.last_status = Some(format!("Wrote {}", path.display()));
            }
            Err(error) => {
                self.last_status = Some(error);
            }
        }
    }

    pub(crate) fn save_and_exit_shortwire_without_left_apply(
        &mut self,
    ) -> ReferenceShortwireRestoreResult {
        self.prepare_shortwire_save();
        self.commit_pending_shortwire_without_left_apply();
        self.restore_after_shortwire()
    }

    #[cfg(test)]
    pub(crate) fn cancel_shortwire_without_save(&mut self) -> ReferenceShortwireRestoreResult {
        self.pending_shortwire_patch = None;
        self.restore_after_shortwire()
    }

    pub(crate) fn restore_after_shortwire(&mut self) -> ReferenceShortwireRestoreResult {
        let restore_file = self.take_shortwire_restore_file_request();
        let Some(pre_shortwire_source) = self.pre_shortwire_source.take() else {
            return ReferenceShortwireRestoreResult {
                restore_file,
                editor_restored: false,
            };
        };
        self.editor_source = pre_shortwire_source.clone();
        if let Some(file) = self.selected_file_mut() {
            file.source = pre_shortwire_source;
        }
        self.shortwire_active_key = None;
        self.shortwire_base_source = None;
        self.pending_shortwire_patch = None;
        ReferenceShortwireRestoreResult {
            restore_file,
            editor_restored: true,
        }
    }

    pub(crate) fn apply_shortwire_local_restore_result(
        &mut self,
        path: &Path,
        result: Result<(), String>,
    ) -> Option<String> {
        if let Err(error) = result {
            let message = format!(
                "Reference file restore failed for {}: {error}",
                path.display()
            );
            self.last_status = Some(message.clone());
            return Some(message);
        }
        None
    }

    fn take_shortwire_restore_file_request(&mut self) -> Option<ReferenceShortwireFileRestore> {
        let local = self.local_shortwire_file.take()?;
        if !local.wrote_patch {
            return None;
        }
        Some(ReferenceShortwireFileRestore {
            path: local.path,
            content: local.restore_source,
        })
    }

    pub(crate) fn apply_file_reload_result(
        &mut self,
        root: &Path,
        root_label: String,
        relative_path: String,
        now_secs: f64,
        result: Result<ReferenceFileRead, String>,
    ) -> bool {
        match result {
            Ok(file) => {
                self.replace_files(
                    Some(root.to_string_lossy().to_string()),
                    root_label,
                    vec![file.into()],
                    Some(relative_path),
                    0,
                    true,
                );
                self.sync_due_secs = Some(now_secs);
                self.last_status = Some("File reloaded".to_string());
                true
            }
            Err(error) => {
                self.last_status = Some(format!("Reload failed — {error}"));
                false
            }
        }
    }

    pub(crate) fn apply_folder_reload_result(
        &mut self,
        root: &Path,
        root_label: String,
        selected_file: Option<String>,
        now_secs: f64,
        result: Result<(Vec<ReferenceFileRead>, usize), String>,
    ) -> bool {
        match result {
            Ok((files, skipped_files)) => {
                if files.is_empty() {
                    self.last_status = Some("Reload found no UTF-8 text files".to_string());
                    return false;
                }
                self.replace_files(
                    Some(root.to_string_lossy().to_string()),
                    root_label,
                    files.into_iter().map(Into::into).collect(),
                    selected_file,
                    skipped_files,
                    true,
                );
                self.sync_due_secs = Some(now_secs);
                self.last_status = Some(format!("Folder reloaded ({skipped_files} skipped)"));
                true
            }
            Err(error) => {
                self.last_status = Some(format!("Reload failed — {error}"));
                false
            }
        }
    }

    pub(crate) fn apply_sync_completion(
        &mut self,
        completion: ReferenceSyncCompletion,
    ) -> Vec<(DebugArtifactItem, String)> {
        let had_write_error = !completion.write_errors.is_empty();
        for synced_file in &completion.synced_files {
            if let Some(current_file) = self
                .files
                .iter_mut()
                .find(|file| file.relative_path == synced_file.relative_path)
                && current_file.source == synced_file.source
            {
                current_file.loaded_source = synced_file.source.clone();
            }
        }

        if completion.emitted_manifest && self.matches_sync_plan(&completion.plan) {
            self.manifest_dirty = had_write_error;
        }

        if let Some(error) = completion.write_errors.into_iter().next() {
            self.last_status = Some(error);
        }

        let artifacts = completion.artifacts;
        if !artifacts.is_empty()
            && !self.has_dirty_files()
            && !self.manifest_dirty
            && self.last_status.as_deref() != Some("Syncing...")
        {
            self.last_status = Some("Syncing...".to_string());
        }
        artifacts
    }

    pub(crate) fn mark_loading_local_reference(&mut self) {
        self.last_status = Some("Loading local reference".to_string());
    }

    pub(crate) fn apply_empty_artifact_restore(&mut self) -> bool {
        if !self.has_dirty_files()
            && !self.manifest_dirty
            && self.shortwire_active_key.is_none()
            && self.has_content()
        {
            self.replace_preserving_patch_store(ReferenceWorkspaceState::default());
            return true;
        }
        false
    }

    pub(crate) fn apply_artifact_restore(
        &mut self,
        incoming: ReferenceWorkspaceState,
        migrated_legacy: bool,
    ) -> bool {
        if self.shortwire_active_key.is_some() || self.has_dirty_files() || self.manifest_dirty {
            self.acknowledge_sync(&incoming);
            return false;
        }

        if !reference_workspace_loaded_matches(self, &incoming) {
            self.replace_preserving_patch_store(incoming);
            if migrated_legacy {
                self.manifest_dirty = true;
                if let Some(file) = self.selected_file_mut() {
                    file.loaded_source.clear();
                }
                self.sync_due_secs = Some(0.0);
                self.last_status = Some("Migrating legacy reference".to_string());
            }
            return true;
        }
        false
    }

    pub(crate) fn mark_edited(&mut self, now_secs: f64, debounce_secs: f64) {
        if self.shortwire_active_key.is_some() {
            self.last_status = Some("Reference patch draft".to_string());
            return;
        }

        self.commit_editor_to_selected();
        if self.selected_file().is_some() {
            self.sync_due_secs = Some(now_secs + debounce_secs);
            self.last_status = Some("Sync pending".to_string());
        }
    }

    pub(crate) fn take_due_sync_plan(&mut self, now_secs: f64) -> Option<ReferenceSyncPlan> {
        if self.shortwire_active_key.is_some() {
            return None;
        }
        let due_secs = self.sync_due_secs?;
        if now_secs < due_secs {
            return None;
        }

        self.sync_due_secs = None;
        self.commit_editor_to_selected();
        let plan = self.build_sync_plan();
        let has_dirty_files = plan.files.iter().any(|file| file.is_dirty());
        if !plan.manifest_dirty && !has_dirty_files {
            if !self.has_dirty_files() && !self.manifest_dirty {
                self.last_status = None;
            }
            return None;
        }
        self.last_status = Some("Syncing...".to_string());
        Some(plan)
    }

    fn replace_preserving_patch_store(&mut self, mut incoming: ReferenceWorkspaceState) {
        incoming.reference_patches = std::mem::take(&mut self.reference_patches);
        incoming.reference_patches_dirty = self.reference_patches_dirty;
        *self = incoming;
    }

    fn acknowledge_sync(&mut self, incoming: &ReferenceWorkspaceState) {
        let mut any_ack = false;
        for incoming_file in &incoming.files {
            if let Some(current_file) = self
                .files
                .iter_mut()
                .find(|file| file.relative_path == incoming_file.relative_path)
                && current_file.source == incoming_file.source
            {
                current_file.loaded_source = incoming_file.source.clone();
                any_ack = true;
            }
        }

        if self.root_path == incoming.root_path
            && self.root_label == incoming.root_label
            && self.selected_file == incoming.selected_file
            && self.files.len() == incoming.files.len()
        {
            self.manifest_dirty = false;
        }

        if any_ack && !self.has_dirty_files() {
            self.sync_due_secs = None;
            self.last_status = Some("Synced".to_string());
        }
    }

    fn matches_sync_plan(&self, plan: &ReferenceSyncPlan) -> bool {
        self.root_path == plan.root_path
            && self.root_label == plan.root_label
            && self.selected_file == plan.selected_file
            && self.skipped_files == plan.skipped_files
            && self.files.len() == plan.files.len()
            && self
                .files
                .iter()
                .zip(plan.files.iter())
                .all(|(current, planned)| {
                    current.relative_path == planned.relative_path
                        && current.artifact_id == planned.artifact_id
                })
    }
}

pub(crate) fn reference_workspace_loaded_matches(
    current: &ReferenceWorkspaceState,
    incoming: &ReferenceWorkspaceState,
) -> bool {
    current.root_path == incoming.root_path
        && current.root_label == incoming.root_label
        && current.selected_file == incoming.selected_file
        && current.skipped_files == incoming.skipped_files
        && current.files.len() == incoming.files.len()
        && current
            .files
            .iter()
            .zip(incoming.files.iter())
            .all(|(a, b)| {
                a.relative_path == b.relative_path
                    && a.artifact_id == b.artifact_id
                    && a.source == b.source
                    && a.loaded_source == b.loaded_source
            })
}

pub(crate) fn reference_shortwire_patch_key(relative_path: &str, row_patch_key: &str) -> String {
    format!("{relative_path}::{row_patch_key}")
}

pub(crate) fn plan_reference_workspace_artifact_restore(
    pass_name: &str,
    workspace_text: Option<&str>,
    reference_files: &[ReferenceArtifactText<'_>],
    legacy_reference_source: Option<&str>,
) -> ReferenceArtifactRestorePlan {
    let file_text_by_id = reference_files
        .iter()
        .map(|snapshot| (snapshot.artifact_id, snapshot.text))
        .collect::<std::collections::HashMap<_, _>>();

    if let Some(workspace_text) = workspace_text
        && let Ok(manifest) = serde_json::from_str::<ReferenceWorkspaceManifest>(workspace_text)
        && manifest.version == REFERENCE_WORKSPACE_VERSION
    {
        if let Some(root_path) = manifest.root_path.clone() {
            return ReferenceArtifactRestorePlan::ReadManifestLocalFiles(
                ReferenceManifestLocalReadRequest {
                    manifest,
                    root_path,
                },
            );
        }

        let mut files = Vec::new();
        let mut archive_fallback_count = 0usize;
        let mut missing_count = 0usize;
        for manifest_file in manifest.files {
            let Some(text) = file_text_by_id.get(manifest_file.artifact_id.as_str()) else {
                missing_count += 1;
                continue;
            };
            archive_fallback_count += 1;
            files.push(ReferenceWorkspaceFile {
                relative_path: manifest_file.relative_path,
                artifact_id: manifest_file.artifact_id,
                source: (*text).to_string(),
                loaded_source: (*text).to_string(),
            });
        }

        if !files.is_empty() {
            let mut state = ReferenceWorkspaceState::default();
            state.replace_files(
                None,
                manifest.root_label,
                files,
                manifest.selected_file,
                manifest.skipped_files + missing_count,
                false,
            );
            if archive_fallback_count > 0 {
                state.last_status = Some("Loaded archived reference".to_string());
            }
            return ReferenceArtifactRestorePlan::Loaded {
                state,
                migrated_legacy: false,
            };
        }
    }

    if !reference_files.is_empty() {
        let mut files = reference_files
            .iter()
            .map(|snapshot| {
                let relative_path = snapshot
                    .name
                    .strip_prefix("Reference: ")
                    .unwrap_or(snapshot.name)
                    .to_string();
                ReferenceWorkspaceFile {
                    relative_path,
                    artifact_id: snapshot.artifact_id.to_string(),
                    source: snapshot.text.to_string(),
                    loaded_source: snapshot.text.to_string(),
                }
            })
            .collect::<Vec<_>>();
        files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(None, "Reference archive".to_string(), files, None, 0, false);
        return ReferenceArtifactRestorePlan::Loaded {
            state,
            migrated_legacy: false,
        };
    }

    let legacy_reference_source = legacy_reference_source.unwrap_or_default();
    if legacy_reference_source.is_empty() {
        return ReferenceArtifactRestorePlan::None;
    }

    let relative_path = "reference.txt".to_string();
    let file = ReferenceWorkspaceFile {
        artifact_id: pass_reference_file_artifact_id(pass_name, &relative_path),
        relative_path: relative_path.clone(),
        source: legacy_reference_source.to_string(),
        loaded_source: String::new(),
    };
    let mut state = ReferenceWorkspaceState::default();
    state.replace_files(
        None,
        "Legacy reference".to_string(),
        vec![file],
        Some(relative_path),
        0,
        true,
    );
    ReferenceArtifactRestorePlan::Loaded {
        state,
        migrated_legacy: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(relative_path: &str, source: &str, loaded_source: &str) -> ReferenceWorkspaceFile {
        ReferenceWorkspaceFile {
            relative_path: relative_path.to_string(),
            artifact_id: format!("artifact:{relative_path}"),
            source: source.to_string(),
            loaded_source: loaded_source.to_string(),
        }
    }

    fn read_file(relative_path: &str, source: &str) -> ReferenceFileRead {
        ReferenceFileRead {
            relative_path: relative_path.to_string(),
            artifact_id: format!("artifact:{relative_path}"),
            source: source.to_string(),
            loaded_source: source.to_string(),
        }
    }

    fn reference_patch() -> ShortwireNodePatch {
        ShortwireNodePatch {
            hunks: vec![ShortwireHunk {
                old_start: 0,
                old_lines: vec!["old".to_string()],
                new_lines: vec!["new".to_string()],
                context_before: vec![],
                context_after: vec![],
            }],
            base_source_hash: 42,
            reference_image: None,
            diff_result: None,
        }
    }

    #[test]
    fn sync_completion_marks_matching_files_clean() {
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(
            None,
            "Reference".to_string(),
            vec![file("a.wgsl", "edited", "old")],
            Some("a.wgsl".to_string()),
            0,
            true,
        );
        let plan = state.build_sync_plan();

        let artifacts = state.apply_sync_completion(ReferenceSyncCompletion {
            plan,
            artifacts: vec![],
            synced_files: vec![ReferenceSyncedFile {
                relative_path: "a.wgsl".to_string(),
                source: "edited".to_string(),
            }],
            write_errors: vec![],
            emitted_manifest: true,
        });

        assert!(artifacts.is_empty());
        assert!(!state.has_dirty_files());
        assert!(!state.manifest_dirty);
    }

    #[test]
    fn stale_sync_completion_does_not_clear_manifest_dirty() {
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(
            None,
            "Reference".to_string(),
            vec![file("a.wgsl", "edited", "old")],
            Some("a.wgsl".to_string()),
            0,
            true,
        );
        let mut stale_plan = state.build_sync_plan();
        stale_plan.selected_file = Some("other.wgsl".to_string());

        state.apply_sync_completion(ReferenceSyncCompletion {
            plan: stale_plan,
            artifacts: vec![],
            synced_files: vec![ReferenceSyncedFile {
                relative_path: "a.wgsl".to_string(),
                source: "edited".to_string(),
            }],
            write_errors: vec![],
            emitted_manifest: true,
        });

        assert!(state.manifest_dirty);
    }

    #[test]
    fn empty_artifact_restore_clears_clean_workspace_and_preserves_patch_store() {
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(
            None,
            "Reference".to_string(),
            vec![file("a.wgsl", "source", "source")],
            Some("a.wgsl".to_string()),
            0,
            false,
        );
        state
            .reference_patches
            .insert("a.wgsl::row".to_string(), reference_patch());
        state.reference_patches_dirty = true;

        assert!(state.apply_empty_artifact_restore());

        assert!(state.files.is_empty());
        assert!(state.reference_patches.contains_key("a.wgsl::row"));
        assert!(state.reference_patches_dirty);
    }

    #[test]
    fn artifact_restore_migrates_legacy_state() {
        let mut state = ReferenceWorkspaceState::default();
        let mut incoming = ReferenceWorkspaceState::default();
        incoming.replace_files(
            None,
            "Legacy reference".to_string(),
            vec![file("reference.txt", "legacy", "legacy")],
            Some("reference.txt".to_string()),
            0,
            false,
        );

        assert!(state.apply_artifact_restore(incoming, true));

        assert!(state.manifest_dirty);
        assert_eq!(state.sync_due_secs, Some(0.0));
        assert_eq!(
            state.last_status.as_deref(),
            Some("Migrating legacy reference")
        );
        assert_eq!(
            state
                .selected_file()
                .map(|file| file.loaded_source.as_str()),
            Some("")
        );
    }

    #[test]
    fn artifact_restore_acknowledges_dirty_matching_workspace() {
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(
            Some("/tmp/reference".to_string()),
            "Reference".to_string(),
            vec![file("a.wgsl", "edited", "old")],
            Some("a.wgsl".to_string()),
            0,
            true,
        );
        let mut incoming = ReferenceWorkspaceState::default();
        incoming.replace_files(
            Some("/tmp/reference".to_string()),
            "Reference".to_string(),
            vec![file("a.wgsl", "edited", "edited")],
            Some("a.wgsl".to_string()),
            0,
            false,
        );

        assert!(!state.apply_artifact_restore(incoming, false));

        assert!(!state.manifest_dirty);
        assert_eq!(state.sync_due_secs, None);
        assert_eq!(state.last_status.as_deref(), Some("Synced"));
        assert_eq!(
            state
                .selected_file()
                .map(|file| file.loaded_source.as_str()),
            Some("edited")
        );
    }

    #[test]
    fn mark_edited_debounces_due_sync_plan() {
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(
            None,
            "Reference".to_string(),
            vec![file("a.wgsl", "old", "old")],
            Some("a.wgsl".to_string()),
            0,
            false,
        );
        state.editor_source = "edited".to_string();

        state.mark_edited(1.0, 0.25);

        assert_eq!(state.sync_due_secs, Some(1.25));
        assert_eq!(state.last_status.as_deref(), Some("Sync pending"));
        assert!(state.take_due_sync_plan(1.24).is_none());
        let plan = state
            .take_due_sync_plan(1.25)
            .expect("due dirty reference should produce sync plan");
        assert_eq!(state.last_status.as_deref(), Some("Syncing..."));
        assert_eq!(plan.files[0].source, "edited");
        assert!(plan.files[0].is_dirty());
    }

    #[test]
    fn mark_edited_during_shortwire_keeps_patch_draft_local() {
        let mut state = ReferenceWorkspaceState::default();
        state.shortwire_active_key = Some("patch".to_string());

        state.mark_edited(1.0, 0.25);

        assert_eq!(state.last_status.as_deref(), Some("Reference patch draft"));
        assert!(state.sync_due_secs.is_none());
    }

    #[test]
    fn prepare_reload_blocks_without_local_root() {
        let mut state = ReferenceWorkspaceState::default();

        let plan = state.prepare_reload(1.0);

        assert!(plan.is_none());
        assert_eq!(
            state.last_status.as_deref(),
            Some("No local path to reload")
        );
    }

    #[test]
    fn prepare_reload_blocks_during_shortwire() {
        let mut state = ReferenceWorkspaceState::default();
        state.root_path = Some("/tmp/reference".to_string());
        state.shortwire_active_key = Some("patch".to_string());

        let plan = state.prepare_reload(1.0);

        assert!(plan.is_none());
        assert_eq!(
            state.last_status.as_deref(),
            Some("Close shortwire before reloading")
        );
    }

    #[test]
    fn prepare_reload_returns_effect_plan() {
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(
            Some("/tmp/reference".to_string()),
            "reference".to_string(),
            vec![file("a.wgsl", "source", "source")],
            Some("a.wgsl".to_string()),
            0,
            false,
        );

        let plan = state
            .prepare_reload(2.5)
            .expect("reload should produce a plan");

        assert_eq!(plan.root, PathBuf::from("/tmp/reference"));
        assert_eq!(plan.root_label, "reference");
        assert_eq!(plan.selected_file.as_deref(), Some("a.wgsl"));
        assert!(plan.single_file);
        assert_eq!(plan.now_secs, 2.5);
        assert_eq!(state.last_status.as_deref(), Some("Reloading..."));
    }

    #[test]
    fn folder_import_result_replaces_files_and_schedules_sync() {
        let mut state = ReferenceWorkspaceState::default();
        let root = PathBuf::from("/tmp/reference");

        let changed = state.apply_folder_import_result(
            &root,
            3.0,
            Ok((
                vec![
                    read_file("b.wgsl", "fn b() {}\n"),
                    read_file("a.wgsl", "fn a() {}\n"),
                ],
                2,
            )),
        );

        assert!(changed);
        assert_eq!(state.root_path.as_deref(), Some("/tmp/reference"));
        assert_eq!(state.root_label, "reference");
        assert_eq!(state.selected_file.as_deref(), Some("a.wgsl"));
        assert_eq!(state.editor_source, "fn a() {}\n");
        assert_eq!(state.sync_due_secs, Some(3.0));
        assert_eq!(
            state.last_status.as_deref(),
            Some("Folder imported (2 skipped)")
        );
        assert_eq!(state.skipped_files, 2);
        assert!(state.manifest_dirty);
    }

    #[test]
    fn folder_import_result_blocks_during_shortwire() {
        let mut state = ReferenceWorkspaceState::default();
        state.shortwire_active_key = Some("patch".to_string());

        let changed = state.apply_folder_import_result(
            &PathBuf::from("/tmp/reference"),
            3.0,
            Ok((vec![read_file("a.wgsl", "fn a() {}\n")], 0)),
        );

        assert!(!changed);
        assert!(state.files.is_empty());
        assert_eq!(
            state.last_status.as_deref(),
            Some("Close shortwire before opening a folder")
        );
    }

    #[test]
    fn folder_import_result_keeps_existing_files_when_empty() {
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(
            None,
            "Reference".to_string(),
            vec![file("existing.wgsl", "old", "old")],
            Some("existing.wgsl".to_string()),
            0,
            false,
        );

        let changed = state.apply_folder_import_result(
            &PathBuf::from("/tmp/reference"),
            3.0,
            Ok((vec![], 0)),
        );

        assert!(!changed);
        assert_eq!(state.selected_file.as_deref(), Some("existing.wgsl"));
        assert_eq!(
            state.last_status.as_deref(),
            Some("No UTF-8 text files found")
        );
    }

    #[test]
    fn folder_import_result_reports_error() {
        let mut state = ReferenceWorkspaceState::default();

        let changed = state.apply_folder_import_result(
            &PathBuf::from("/tmp/reference"),
            3.0,
            Err("read failed".to_string()),
        );

        assert!(!changed);
        assert_eq!(state.last_status.as_deref(), Some("read failed"));
    }

    #[test]
    fn file_reload_result_replaces_selected_file_and_schedules_sync() {
        let mut state = ReferenceWorkspaceState::default();
        let root = PathBuf::from("/tmp/reference");

        let changed = state.apply_file_reload_result(
            &root,
            "Reference Root".to_string(),
            "main.wgsl".to_string(),
            4.0,
            Ok(read_file("main.wgsl", "fn main() {}\n")),
        );

        assert!(changed);
        assert_eq!(state.root_path.as_deref(), Some("/tmp/reference"));
        assert_eq!(state.root_label, "Reference Root");
        assert_eq!(state.selected_file.as_deref(), Some("main.wgsl"));
        assert_eq!(state.editor_source, "fn main() {}\n");
        assert_eq!(state.sync_due_secs, Some(4.0));
        assert_eq!(state.last_status.as_deref(), Some("File reloaded"));
        assert!(state.manifest_dirty);
    }

    #[test]
    fn file_reload_result_reports_error() {
        let mut state = ReferenceWorkspaceState::default();

        let changed = state.apply_file_reload_result(
            &PathBuf::from("/tmp/reference"),
            "Reference Root".to_string(),
            "main.wgsl".to_string(),
            4.0,
            Err("read failed".to_string()),
        );

        assert!(!changed);
        assert_eq!(
            state.last_status.as_deref(),
            Some("Reload failed — read failed")
        );
    }

    #[test]
    fn folder_reload_result_replaces_files_and_preserves_selected_when_present() {
        let mut state = ReferenceWorkspaceState::default();

        let changed = state.apply_folder_reload_result(
            &PathBuf::from("/tmp/reference"),
            "Reference Root".to_string(),
            Some("b.wgsl".to_string()),
            5.0,
            Ok((
                vec![
                    read_file("a.wgsl", "fn a() {}\n"),
                    read_file("b.wgsl", "fn b() {}\n"),
                ],
                1,
            )),
        );

        assert!(changed);
        assert_eq!(state.root_path.as_deref(), Some("/tmp/reference"));
        assert_eq!(state.root_label, "Reference Root");
        assert_eq!(state.selected_file.as_deref(), Some("b.wgsl"));
        assert_eq!(state.editor_source, "fn b() {}\n");
        assert_eq!(state.sync_due_secs, Some(5.0));
        assert_eq!(
            state.last_status.as_deref(),
            Some("Folder reloaded (1 skipped)")
        );
        assert_eq!(state.skipped_files, 1);
        assert!(state.manifest_dirty);
    }

    #[test]
    fn folder_reload_result_keeps_existing_files_when_empty() {
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(
            None,
            "Reference".to_string(),
            vec![file("existing.wgsl", "old", "old")],
            Some("existing.wgsl".to_string()),
            0,
            false,
        );

        let changed = state.apply_folder_reload_result(
            &PathBuf::from("/tmp/reference"),
            "Reference Root".to_string(),
            None,
            5.0,
            Ok((vec![], 0)),
        );

        assert!(!changed);
        assert_eq!(state.selected_file.as_deref(), Some("existing.wgsl"));
        assert_eq!(
            state.last_status.as_deref(),
            Some("Reload found no UTF-8 text files")
        );
    }

    #[test]
    fn folder_reload_result_reports_error() {
        let mut state = ReferenceWorkspaceState::default();

        let changed = state.apply_folder_reload_result(
            &PathBuf::from("/tmp/reference"),
            "Reference Root".to_string(),
            None,
            5.0,
            Err("read failed".to_string()),
        );

        assert!(!changed);
        assert_eq!(
            state.last_status.as_deref(),
            Some("Reload failed — read failed")
        );
    }

    #[test]
    fn mark_reload_missing_path_updates_status() {
        let mut state = ReferenceWorkspaceState::default();

        state.mark_reload_missing_path();

        assert_eq!(
            state.last_status.as_deref(),
            Some("Local path missing — keeping archive snapshot")
        );
    }

    #[test]
    fn restore_reference_patches_from_text_loads_payload_when_clean() {
        let mut state = ReferenceWorkspaceState::default();
        let payload = ReferenceWorkspacePatchesPayload {
            version: REFERENCE_WORKSPACE_VERSION,
            patches: HashMap::from([("file.wgsl::row".to_string(), reference_patch())]),
        };
        let text = serde_json::to_string(&payload).unwrap();

        state.restore_reference_patches_from_text(Some(&text));

        assert!(state.reference_patches.contains_key("file.wgsl::row"));
    }

    #[test]
    fn restore_reference_patches_from_text_keeps_dirty_state() {
        let mut state = ReferenceWorkspaceState::default();
        state
            .reference_patches
            .insert("local::row".to_string(), reference_patch());
        state.reference_patches_dirty = true;
        let payload = ReferenceWorkspacePatchesPayload {
            version: REFERENCE_WORKSPACE_VERSION,
            patches: HashMap::from([("incoming::row".to_string(), reference_patch())]),
        };
        let text = serde_json::to_string(&payload).unwrap();

        state.restore_reference_patches_from_text(Some(&text));

        assert!(state.reference_patches.contains_key("local::row"));
        assert!(!state.reference_patches.contains_key("incoming::row"));
    }

    #[test]
    fn take_reference_patches_dirty_artifact_clears_dirty_flag() {
        let mut state = ReferenceWorkspaceState::default();
        state
            .reference_patches
            .insert("file.wgsl::row".to_string(), reference_patch());
        state.reference_patches_dirty = true;

        let artifact = state
            .take_reference_patches_dirty_artifact("pass")
            .expect("dirty reference patches should emit artifact");

        assert!(!state.reference_patches_dirty);
        assert_eq!(artifact.0.name, "Reference shortwire patches");
        assert!(artifact.1.contains("file.wgsl::row"));
        assert!(
            state
                .take_reference_patches_dirty_artifact("pass")
                .is_none()
        );
    }

    #[test]
    fn enter_shortwire_applies_stored_patch_and_requests_local_read() {
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(
            Some("/tmp/reference".to_string()),
            "Reference".to_string(),
            vec![file("main.wgsl", "fn main() {}\n", "fn main() {}\n")],
            Some("main.wgsl".to_string()),
            0,
            false,
        );
        let patch_key = reference_shortwire_patch_key("main.wgsl", "row");
        state.reference_patches.insert(
            patch_key.clone(),
            ShortwireNodePatch {
                hunks: compute_hunks("fn main() {}\n", "fn patched() {}\n"),
                base_source_hash: hash_source("fn main() {}\n"),
                reference_image: None,
                diff_result: None,
            },
        );

        let result = state
            .enter_shortwire("row")
            .expect("selected file should enter reference shortwire");

        assert_eq!(
            state.shortwire_active_key.as_deref(),
            Some(patch_key.as_str())
        );
        assert_eq!(state.editor_source, "fn patched() {}\n");
        assert_eq!(
            result.local_read,
            Some(ReferenceShortwireLocalReadRequest {
                path: PathBuf::from("/tmp/reference/main.wgsl"),
                write_after_read: true,
            })
        );
    }

    #[test]
    fn commit_pending_shortwire_after_left_apply_tracks_patch_and_local_restore() {
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(
            Some("/tmp/reference".to_string()),
            "Reference".to_string(),
            vec![file("main.wgsl", "fn main() {}\n", "fn main() {}\n")],
            Some("main.wgsl".to_string()),
            0,
            false,
        );
        state.enter_shortwire("row").unwrap();
        let path = PathBuf::from("/tmp/reference/main.wgsl");
        assert!(
            state
                .apply_shortwire_local_snapshot(
                    path.clone(),
                    Ok("local original".to_string()),
                    false
                )
                .is_none()
        );
        state.editor_source = "fn patched() {}\n".to_string();
        state.prepare_shortwire_save();

        assert!(state.commit_pending_shortwire_after_left_apply());
        let write = state
            .shortwire_write_request()
            .expect("local shortwire file should be writeable");
        assert_eq!(write.path, path);
        assert_eq!(write.content, "fn patched() {}\n");
        state.apply_shortwire_local_write_result(&write.path, Ok(()));
        let restore = state.restore_after_shortwire();

        assert!(state.reference_patches_dirty);
        assert_eq!(state.reference_patches.len(), 1);
        assert_eq!(
            restore.restore_file,
            Some(ReferenceShortwireFileRestore {
                path: PathBuf::from("/tmp/reference/main.wgsl"),
                content: "local original".to_string(),
            })
        );
        assert!(restore.editor_restored);
        assert_eq!(state.editor_source, "fn main() {}\n");
        assert_eq!(
            state.selected_file().map(|file| file.source.as_str()),
            Some("fn main() {}\n")
        );
        assert!(state.shortwire_active_key.is_none());
    }

    #[test]
    fn save_and_exit_shortwire_without_left_apply_stores_patch_and_restores_editor() {
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(
            None,
            "Reference".to_string(),
            vec![file("main.wgsl", "fn main() {}\n", "fn main() {}\n")],
            Some("main.wgsl".to_string()),
            0,
            false,
        );
        state.enter_shortwire("row").unwrap();
        state.editor_source = "fn patched() {}\n".to_string();

        let restore = state.save_and_exit_shortwire_without_left_apply();

        assert!(state.reference_patches_dirty);
        assert_eq!(state.reference_patches.len(), 1);
        assert_eq!(restore.restore_file, None);
        assert!(restore.editor_restored);
        assert_eq!(state.editor_source, "fn main() {}\n");
        assert!(state.shortwire_active_key.is_none());
    }
}
