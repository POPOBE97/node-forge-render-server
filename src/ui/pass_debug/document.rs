use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rust_wgpu_fiber::eframe::egui;

use crate::app::ShortwirePastedReferenceImage;
use crate::dsl::DebugArtifactItem;
use crate::renderer::{PassDebugSource, PassDebugSourceRange};
#[cfg(test)]
use crate::ui::pass_debug::artifacts;
#[cfg(test)]
use crate::ui::pass_debug::artifacts::ShortwireNodePatch;
use crate::ui::pass_debug::artifacts::shortwire_reference_image_artifact;
#[cfg(test)]
use crate::ui::pass_debug::artifacts::{
    DEBUG_ARTIFACT_DEFAULT_SLOT, DEBUG_ARTIFACT_REFERENCE_PATCHES_SLOT,
    DEBUG_ARTIFACT_REFERENCE_WORKSPACE_SLOT, REFERENCE_WORKSPACE_VERSION,
    ReferenceWorkspaceManifest, ReferenceWorkspaceManifestFile, pass_reference_file_artifact_id,
    pass_reference_file_artifact_item, pass_reference_file_slot_key,
    reference_workspace_artifact_item,
};
use crate::ui::pass_debug::dependency_tree::{
    DependencyTreeStateChange, PassDebugDependencyRow, PassDebugTreeClick,
};
#[cfg(test)]
use crate::ui::pass_debug::dependency_tree::{
    byte_index_to_char_index, dependency_path_for_row_key, flatten_dependency_tree,
};
use crate::ui::pass_debug::event::{PassDebugEffect, PassDebugEvent, push_window_action};
use crate::ui::pass_debug::file_io::ReferenceFileRead;
#[cfg(test)]
use crate::ui::pass_debug::file_io::{read_reference_file, write_reference_workspace_file};
use crate::ui::pass_debug::merge::{MergeCanonicalChangeResult, MergePatchRequest};
#[cfg(test)]
use crate::ui::pass_debug::patch::ShortwireDiffRowKind;
#[cfg(test)]
use crate::ui::pass_debug::patch::{ShortwireHunk, build_shortwire_diff_view};
use crate::ui::pass_debug::patch::{apply_hunks, compute_hunks};
#[cfg(test)]
use crate::ui::pass_debug::reference_workspace::reference_shortwire_patch_key;
use crate::ui::pass_debug::reference_workspace::{
    ReferenceArtifactRestorePlan, ReferenceArtifactText, ReferenceShortwireFileWrite,
    ReferenceShortwireRestoreResult, ReferenceSyncCompletion, ReferenceWorkspaceState,
    plan_reference_workspace_artifact_restore,
};
#[cfg(test)]
use crate::ui::pass_debug::reference_workspace::{
    ReferenceSyncPlan, ReferenceSyncedFile, ReferenceWorkspaceFile,
};
#[cfg(test)]
use crate::ui::pass_debug::registry::{
    PassDebugWindowMap, PassDebugWindowState, mark_patch_applied,
    request_active_shortwire_diff_capture,
};
use crate::ui::pass_debug::render::editor::LineGalleyCache;
use crate::ui::pass_debug::render::tree_paint::source_jump_button_size;
use crate::ui::pass_debug::shader_document::ShaderDocumentState;
#[cfg(test)]
use crate::ui::pass_debug::shader_document::hash_source;
#[cfg(test)]
use crate::ui::pass_debug::shortwire::ShortwireDotStatus;
#[cfg(test)]
use crate::ui::pass_debug::shortwire::ShortwirePatchesPayload;
#[cfg(test)]
use crate::ui::pass_debug::shortwire::shortwire_dot_info_for_patch;
use crate::ui::pass_debug::shortwire::{
    SHORTWIRE_DIFF_PASS_MAX_AE, ShortwirePhase, ShortwireRowIdentity,
    shortwire_click_matches_active_row, shortwire_diff_result_summary, shortwire_diff_status,
    shortwire_patch_key, shortwire_patch_summary,
};
use crate::ui::pass_debug::store::PassDebugStore;

pub use crate::ui::pass_debug::artifacts::ShortwireDiffResult;
pub use crate::ui::pass_debug::event::{PassDebugPatchApplyResult, PassDebugWindowAction};
pub use crate::ui::pass_debug::shortwire::ShortwireDiffCaptureRequest;

const TREE_ROW_INDENT_WIDTH: f32 = 14.0;
const TREE_ROW_TRAILING_PADDING: f32 = 24.0;
const TREE_ROW_SOURCE_JUMP_GAP: f32 = 8.0;
const PASS_DEBUG_REFERENCE_SYNC_DEBOUNCE_SECS: f64 = 0.250;
#[cfg(test)]
type ReferencePatchesPayload = artifacts::ReferencePatchesPayload<ShortwireNodePatch>;

pub(crate) enum ShortwireDiffCaptureAttempt {
    Inactive,
    MissingPatch,
    Captured(PassDebugPatchApplyResult),
}

trait PatchSourceArg {
    fn into_patch_source(self) -> Option<String>;
}

impl PatchSourceArg for bool {
    fn into_patch_source(self) -> Option<String> {
        debug_assert!(
            !self,
            "pass debug patch state should pass the actual patch source"
        );
        None
    }
}

impl<'a> PatchSourceArg for Option<&'a str> {
    fn into_patch_source(self) -> Option<String> {
        self.map(str::to_string)
    }
}

#[derive(Clone, Debug)]
struct PassDebugExpandableRowsCache {
    rows_generation: u64,
    row_keys: HashSet<String>,
}

#[derive(Clone, Debug)]
struct PassDebugVisibleRowsCache {
    rows_generation: u64,
    expansion_generation: u64,
    row_indices: Vec<usize>,
}

#[derive(Clone, Debug)]
struct PassDebugTreeWidthCache {
    rows_generation: u64,
    intrinsic_width: f32,
}

#[derive(Clone, Debug)]
pub struct PassDebugWindowDocument {
    pub pass_name: String,
    pub(crate) store: PassDebugStore,
    pub(crate) reference_line_galley_cache: Option<LineGalleyCache>,
    draft_revision: u64,
    draft_analysis_due_secs: Option<f64>,
    pub(crate) line_galley_cache: Option<LineGalleyCache>,
    dependency_expandable_row_keys_cache: Option<PassDebugExpandableRowsCache>,
    visible_dependency_row_indices_cache: Option<PassDebugVisibleRowsCache>,
    dependency_tree_width_cache: Option<PassDebugTreeWidthCache>,
}

impl PassDebugWindowDocument {
    fn new(
        pass_name: String,
        source: Option<PassDebugSource>,
        source_revision: u64,
        patch_source: impl PatchSourceArg,
    ) -> Self {
        let patch_source = patch_source.into_patch_source();
        let mut document = Self {
            pass_name,
            store: PassDebugStore::new(ShaderDocumentState::new(
                source,
                source_revision,
                patch_source,
            )),
            reference_line_galley_cache: None,
            draft_revision: 0,
            draft_analysis_due_secs: None,
            line_galley_cache: None,
            dependency_expandable_row_keys_cache: None,
            visible_dependency_row_indices_cache: None,
            dependency_tree_width_cache: None,
        };
        document.refresh_analysis_rows();
        document
    }

    pub(crate) fn new_from_runtime_patch(
        pass_name: String,
        source: Option<PassDebugSource>,
        source_revision: u64,
        patch_source: Option<&str>,
    ) -> Self {
        Self::new(pass_name, source, source_revision, patch_source)
    }

    pub(crate) fn emit_window_effect(
        &mut self,
        effect: PassDebugEffect,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        self.emit_effect(effect);
        self.drain_window_effects(pending_actions);
    }

    pub(crate) fn emit_effect(&mut self, effect: PassDebugEffect) {
        self.store.dispatch(PassDebugEvent::EmitEffect(effect));
    }

    pub(crate) fn dispatch_event(
        &mut self,
        event: PassDebugEvent,
        pending_actions: Option<&Arc<Mutex<Vec<PassDebugWindowAction>>>>,
    ) {
        match event {
            PassDebugEvent::Tick { now_secs } => {
                self.maybe_refresh_pending_draft_analysis(now_secs);
            }
            PassDebugEvent::EmitEffect(effect) => self.emit_effect(effect),
            PassDebugEvent::SaveRequested => {
                let Some(pending_actions) = pending_actions else {
                    return;
                };
                self.request_save(pending_actions);
            }
            PassDebugEvent::CloseRequested => {
                if let Some(pending_actions) = pending_actions {
                    self.prepare_debug_window_close(pending_actions);
                }
            }
            PassDebugEvent::ToggleShortwireDiff => {
                self.store.dispatch(PassDebugEvent::ToggleShortwireDiff);
            }
            PassDebugEvent::ReferenceReloadRequested { now_secs } => {
                self.reload_reference_workspace(now_secs);
            }
            PassDebugEvent::ReferenceOpenFolderRequested { now_secs } => {
                self.emit_effect(PassDebugEffect::PickReferenceFolder { now_secs });
            }
            PassDebugEvent::ReferenceFileSelected {
                relative_path,
                now_secs,
            } => {
                let change = self.store.dispatch(PassDebugEvent::ReferenceFileSelected {
                    relative_path,
                    now_secs,
                });
                if change.reference_selection_changed {
                    self.clear_reference_line_render_cache();
                }
            }
            PassDebugEvent::ReferenceSyncTick { now_secs } => {
                self.maybe_emit_reference_upsert(now_secs);
            }
            PassDebugEvent::ShaderDraftEdited { now_secs } => {
                self.mark_draft_edited(now_secs);
            }
            PassDebugEvent::ShaderDraftReplaced { source, now_secs } => {
                let change = self
                    .store
                    .dispatch(PassDebugEvent::ShaderDraftReplaced { source, now_secs });
                if change.draft_source_changed {
                    self.mark_draft_edited(now_secs);
                }
            }
            PassDebugEvent::ReferenceDraftEdited { now_secs } => {
                self.mark_reference_edited(now_secs);
            }
            PassDebugEvent::ReferenceEditorReplaced { source, now_secs } => {
                let change = self
                    .store
                    .dispatch(PassDebugEvent::ReferenceEditorReplaced { source, now_secs });
                if change.reference_editor_changed {
                    self.mark_reference_edited(now_secs);
                }
            }
            PassDebugEvent::ShaderEditorClicked { char_index } => {
                self.refresh_draft_analysis();
                let change = self
                    .store
                    .dispatch(PassDebugEvent::ShaderEditorClicked { char_index });
                self.apply_store_change(change);
            }
            PassDebugEvent::DependencyTreeClicked { click } => {
                self.handle_dependency_tree_click(click, pending_actions);
            }
            PassDebugEvent::DependencyFilterEdited { text } => {
                let change = self
                    .store
                    .dispatch(PassDebugEvent::DependencyFilterEdited { text });
                if change.dependency_visibility_changed {
                    self.clear_dependency_visibility_cache();
                }
            }
            PassDebugEvent::DependencyShortwireRequested { row_index } => {
                let Some(pending_actions) = pending_actions else {
                    return;
                };
                self.handle_dependency_shortwire_request(row_index, pending_actions);
            }
            PassDebugEvent::MergeOpenResolver => self.open_merge_resolver(),
            PassDebugEvent::MergeCloseConflictWindows => {
                self.store
                    .dispatch(PassDebugEvent::MergeCloseConflictWindows);
            }
            PassDebugEvent::MergeReopenChoicePopup => {
                self.store.dispatch(PassDebugEvent::MergeReopenChoicePopup);
            }
            PassDebugEvent::MergeResolvedEdited { source } => {
                self.store
                    .dispatch(PassDebugEvent::MergeResolvedEdited { source });
            }
            PassDebugEvent::MergeCancelResolution => self.cancel_merge_resolution(),
            PassDebugEvent::MergeUseIncoming => {
                if let Some(pending_actions) = pending_actions {
                    self.use_merge_incoming(pending_actions);
                }
            }
            PassDebugEvent::MergeKeepLocal => {
                if let Some(pending_actions) = pending_actions {
                    self.keep_merge_local(pending_actions);
                }
            }
            PassDebugEvent::MergeApplyResolved => {
                if let Some(pending_actions) = pending_actions {
                    self.apply_merge_resolved(pending_actions);
                }
            }
        }
    }

    pub(crate) fn drain_effects(&mut self) -> Vec<PassDebugEffect> {
        self.store.drain_effects()
    }

    pub(crate) fn save_enabled(&self) -> bool {
        if self.store.shortwire.active.is_some() {
            self.shortwire_is_editor_interactive()
        } else {
            self.store.shader.dirty && self.store.merge.conflict.is_none()
        }
    }

    fn request_save(&mut self, pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>) {
        if self.store.shortwire.active.is_some() {
            if self.shortwire_is_editor_interactive() {
                self.exit_shortwire_done(pending_actions);
            }
            return;
        }

        if !self.store.shader.dirty || self.store.merge.conflict.is_some() {
            return;
        }

        self.refresh_draft_analysis();
        self.store.shader.mark_apply_requested();
        self.emit_window_effect(
            PassDebugEffect::ApplyPatch {
                pass_name: self.pass_name.clone(),
                source: self.store.shader.draft_source.clone(),
                reference_image: None,
            },
            pending_actions,
        );
    }

    pub(crate) fn clear_reference_line_render_cache(&mut self) {
        self.reference_line_galley_cache = None;
    }

    fn drain_window_effects(&mut self, pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>) {
        for effect in self.store.drain_effects() {
            match effect.into_window_action() {
                Ok(action) => push_action(pending_actions, action),
                Err(effect) => self.store.emit_effect(effect),
            }
        }
    }

    fn replace_draft_source(&mut self, next_source: String) {
        if self.store.shader.replace_draft_source(next_source) {
            self.invalidate_draft_render_cache();
        }
    }

    fn invalidate_draft_render_cache(&mut self) {
        self.draft_revision = self.draft_revision.wrapping_add(1);
    }

    pub(crate) fn mark_draft_edited(&mut self, _now_secs: f64) {
        self.invalidate_draft_render_cache();
        self.store.shader.mark_draft_edited();
        self.draft_analysis_due_secs = None;
    }

    pub(crate) fn update_reference_workspace(
        &mut self,
        workspace_text: Option<&str>,
        reference_files: &[crate::debug_artifacts::DebugArtifactTextSnapshot],
        legacy_reference_source: Option<&str>,
        reference_patches_text: Option<&str>,
    ) {
        self.restore_reference_patches_from_text(reference_patches_text);

        let reference_file_texts = reference_files
            .iter()
            .map(|snapshot| ReferenceArtifactText {
                artifact_id: snapshot.item.id.as_str(),
                name: snapshot.item.name.as_str(),
                text: snapshot.text.as_str(),
            })
            .collect::<Vec<_>>();
        let restore_plan = plan_reference_workspace_artifact_restore(
            &self.pass_name,
            workspace_text,
            &reference_file_texts,
            legacy_reference_source,
        );

        match restore_plan {
            ReferenceArtifactRestorePlan::None => self.apply_empty_reference_artifact_restore(),
            ReferenceArtifactRestorePlan::Loaded {
                state,
                migrated_legacy,
            } => self.apply_reference_artifact_restore(state, migrated_legacy),
            ReferenceArtifactRestorePlan::ReadManifestLocalFiles(request) => {
                self.emit_effect(PassDebugEffect::ReadReferenceManifestFiles {
                    root: PathBuf::from(&request.root_path),
                    manifest: request.manifest,
                });
                self.store
                    .reference_workspace
                    .mark_loading_local_reference();
            }
        }
    }

    pub(crate) fn apply_reference_manifest_local_read_result(
        &mut self,
        incoming: ReferenceWorkspaceState,
    ) {
        self.apply_reference_artifact_restore(incoming, false);
    }

    fn apply_empty_reference_artifact_restore(&mut self) {
        if self
            .store
            .reference_workspace
            .apply_empty_artifact_restore()
        {
            self.reference_line_galley_cache = None;
        }
    }

    fn apply_reference_artifact_restore(
        &mut self,
        incoming: ReferenceWorkspaceState,
        migrated_legacy: bool,
    ) {
        if self
            .store
            .reference_workspace
            .apply_artifact_restore(incoming, migrated_legacy)
        {
            self.reference_line_galley_cache = None;
        }
    }

    pub(crate) fn mark_reference_edited(&mut self, now_secs: f64) {
        self.store
            .reference_workspace
            .mark_edited(now_secs, PASS_DEBUG_REFERENCE_SYNC_DEBOUNCE_SECS);
    }

    pub(crate) fn maybe_emit_reference_upsert(&mut self, now_secs: f64) {
        if let Some(plan) = self.store.reference_workspace.take_due_sync_plan(now_secs) {
            self.emit_effect(PassDebugEffect::RunReferenceSyncPlan { plan });
        }
    }

    pub(crate) fn maybe_refresh_pending_draft_analysis(&mut self, _now_secs: f64) {
        self.draft_analysis_due_secs = None;
    }

    #[cfg(test)]
    fn update_source(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        patch_source: impl PatchSourceArg,
    ) {
        self.update_source_inner(source, source_revision, patch_source, None);
    }

    fn update_source_with_actions(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        patch_source: impl PatchSourceArg,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        self.update_source_inner(source, source_revision, patch_source, Some(pending_actions));
    }

    pub(crate) fn update_source_with_runtime_patch(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        patch_source: Option<&str>,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        self.update_source_with_actions(source, source_revision, patch_source, pending_actions);
    }

    fn update_source_inner(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        patch_source: impl PatchSourceArg,
        pending_actions: Option<&Arc<Mutex<Vec<PassDebugWindowAction>>>>,
    ) {
        let patch_source = patch_source.into_patch_source();
        let patch_active = patch_source.is_some();
        let canonical_source_text = source.map(|s| s.module_source.as_str()).unwrap_or_default();
        let next_editor_source = patch_source
            .as_deref()
            .unwrap_or(canonical_source_text)
            .to_string();
        if let Some(change) = self.store.shader.handle_same_revision_refresh(
            source_revision,
            patch_source.as_deref(),
            self.store.shortwire.active.is_some(),
        ) {
            if change.draft_changed {
                self.invalidate_draft_render_cache();
            }
            return;
        }

        self.store
            .shader
            .begin_source_revision(source, source_revision);

        if self.store.shortwire.active.is_some() {
            if self
                .store
                .shader
                .refresh_shortwire_canonical_snapshot(source, canonical_source_text)
            {
                self.store.shortwire.mark_active_base_stale();
            }
            return;
        }

        if patch_active
            && pending_actions.is_some()
            && canonical_source_text != self.store.shader.generated_base_source
        {
            self.handle_patch_canonical_change(
                source,
                source_revision,
                canonical_source_text,
                patch_source.as_deref().unwrap_or_default(),
                pending_actions.expect("pending actions checked above"),
            );
            return;
        }

        let refresh = self.store.shader.refresh_source_snapshot(
            source,
            canonical_source_text,
            next_editor_source,
        );
        if refresh.draft_changed {
            self.invalidate_draft_render_cache();
        }
        self.draft_analysis_due_secs = None;
        self.refresh_analysis_rows();
        if refresh.source_missing {
            return;
        }
    }

    fn handle_patch_canonical_change(
        &mut self,
        source: Option<&PassDebugSource>,
        _source_revision: u64,
        incoming_source: &str,
        local_source: &str,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        let base_source = self.store.shader.generated_base_source.clone();
        self.store
            .shader
            .refresh_canonical_patch_snapshot(source, incoming_source);
        self.draft_analysis_due_secs = None;
        self.refresh_analysis_rows();

        match self.store.merge.handle_canonical_patch_change(
            base_source,
            incoming_source,
            local_source,
        ) {
            MergeCanonicalChangeResult::Request(request) => {
                self.push_merge_patch_request(request, pending_actions);
                self.store.shader.clear_error();
            }
            MergeCanonicalChangeResult::Conflict {
                local_source,
                status,
            } => {
                if self.store.shader.enter_merge_conflict(local_source, status) {
                    self.invalidate_draft_render_cache();
                }
            }
        }
    }

    fn push_merge_patch_request(
        &mut self,
        request: MergePatchRequest,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        match request {
            MergePatchRequest::Apply { source, status } => {
                self.emit_window_effect(
                    PassDebugEffect::ApplyPatch {
                        pass_name: self.pass_name.clone(),
                        source,
                        reference_image: None,
                    },
                    pending_actions,
                );
                self.store.shader.set_status(status);
            }
            MergePatchRequest::Reset { status } => {
                self.emit_window_effect(
                    PassDebugEffect::ResetPatch {
                        pass_name: self.pass_name.clone(),
                    },
                    pending_actions,
                );
                self.store.shader.set_status(status);
            }
        }
    }

    pub(crate) fn mark_applied(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        draft_source: String,
        status: String,
    ) -> Option<ShortwireDiffCaptureRequest> {
        let is_shortwire_completion = matches!(
            self.store.shortwire.active.as_ref().map(|a| &a.phase),
            Some(ShortwirePhase::PendingApply { .. })
        );
        let mut diff_capture_request = None;
        if self
            .store
            .shader
            .mark_patch_applied(source, source_revision, draft_source, status)
        {
            self.invalidate_draft_render_cache();
        }
        self.draft_analysis_due_secs = None;
        let applied_source = self.store.shader.loaded_source.clone();
        self.commit_pending_merge_patch_update(&applied_source);
        self.store.merge.clear_conflict();

        if is_shortwire_completion {
            if let Some(committed) = self
                .store
                .shortwire
                .commit_active_pending_apply(self.store.shader.generated_base_source_hash)
            {
                eprintln!(
                    "[shortwire-diff] apply_success_queue_capture pass={} patch_key={} hunks={} base_hash={} exit_on_apply={}",
                    self.pass_name,
                    committed.patch_key,
                    committed.hunk_count,
                    committed.base_source_hash,
                    committed.exit_on_apply,
                );
                diff_capture_request = Some(ShortwireDiffCaptureRequest {
                    pass_name: self.pass_name.clone(),
                    patch_key: committed.patch_key,
                });
            }
            let should_exit_shortwire = self.store.shortwire.exit_on_apply;
            self.commit_reference_shortwire_after_left_apply(should_exit_shortwire);
            if self.store.shortwire.exit_on_apply {
                self.store.shortwire.clear_exit_on_apply();
                self.store.shortwire.clear_active();
                self.refresh_analysis_rows();
            } else {
                self.store.shortwire.finish_apply_completion_editing(
                    self.store.shader.generated_base_source.clone(),
                    self.store.shader.generated_base_source_hash,
                );
            }
        } else {
            self.refresh_analysis_rows();
            if let Some(committed) = self
                .store
                .shortwire
                .take_active_and_commit_pending_apply(self.store.shader.generated_base_source_hash)
            {
                eprintln!(
                    "[shortwire-diff] apply_success_store_without_capture pass={} patch_key={} base_hash={}",
                    self.pass_name, committed.patch_key, committed.base_source_hash,
                );
            }
        }

        diff_capture_request
    }

    pub(crate) fn mark_reset(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        status: String,
    ) {
        if self
            .store
            .shader
            .mark_patch_reset(source, source_revision, status)
        {
            self.invalidate_draft_render_cache();
        }
        self.draft_analysis_due_secs = None;
        self.refresh_analysis_rows();
        let reset_source = self.store.shader.generated_base_source.clone();
        self.commit_pending_merge_patch_update(&reset_source);
        self.store.merge.clear_conflict();

        let pending_enter = matches!(
            self.store.shortwire.active.as_ref().map(|a| &a.phase),
            Some(ShortwirePhase::PendingResetThenEnter { .. })
        );
        if pending_enter {
            self.complete_shortwire_entry();
        }
    }

    pub(crate) fn refresh_draft_analysis(&mut self) {
        self.draft_analysis_due_secs = None;
    }

    fn refresh_analysis_rows(&mut self) {
        let change = self
            .store
            .dependencies
            .refresh_from_source(self.store.shader.analysis_source.as_ref());
        self.apply_dependency_tree_change(change);
    }

    fn apply_dependency_tree_change(&mut self, change: DependencyTreeStateChange) {
        if change.rows_changed {
            self.clear_dependency_row_caches();
        } else if change.visibility_changed {
            self.clear_dependency_visibility_cache();
        }
    }

    fn apply_store_change(&mut self, change: crate::ui::pass_debug::store::PassDebugStoreChange) {
        if change.dependency_rows_changed {
            self.clear_dependency_row_caches();
        } else if change.dependency_visibility_changed {
            self.clear_dependency_visibility_cache();
        }
    }

    fn clear_dependency_row_caches(&mut self) {
        self.dependency_expandable_row_keys_cache = None;
        self.visible_dependency_row_indices_cache = None;
        self.dependency_tree_width_cache = None;
    }

    fn clear_dependency_visibility_cache(&mut self) {
        self.visible_dependency_row_indices_cache = None;
    }

    #[cfg(test)]
    fn focus_target(&mut self, target_id: impl Into<String>, _show_dependencies: bool) {
        let change = self.store.dependencies.focus_target(
            self.store.shader.analysis_source.as_ref(),
            target_id,
            true,
        );
        self.apply_dependency_tree_change(change);
    }

    #[cfg(test)]
    fn focus_target_from_editor(&mut self, target_id: impl Into<String>) {
        let change = self
            .store
            .dependencies
            .focus_target_from_editor(self.store.shader.analysis_source.as_ref(), target_id);
        self.apply_dependency_tree_change(change);
    }

    pub(crate) fn focus_tree_click(&mut self, click: PassDebugTreeClick, show_dependencies: bool) {
        let _ = show_dependencies;
        let change = self
            .store
            .dispatch(PassDebugEvent::DependencyTreeClicked { click });
        self.apply_store_change(change);
    }

    #[cfg(test)]
    fn focus_dependency_row_key(
        &mut self,
        row_key: impl Into<String>,
        _show_dependencies: bool,
        jump_editor: bool,
        reveal_row: bool,
    ) {
        let change = self
            .store
            .dependencies
            .focus_row_key(row_key, jump_editor, reveal_row);
        self.apply_dependency_tree_change(change);
    }

    #[cfg(test)]
    fn focus_target_at_char_index(&mut self, char_index: usize) {
        let change = self
            .store
            .dispatch(PassDebugEvent::ShaderEditorClicked { char_index });
        self.apply_store_change(change);
    }

    pub(crate) fn focused_source_range(&self) -> Option<PassDebugSourceRange> {
        self.store
            .dependencies
            .focused_source_range(self.store.shader.analysis_source.as_ref())
    }

    pub(crate) fn focus_is_in_dependency_root(&self) -> bool {
        self.store.dependencies.focus_is_in_root()
    }

    #[cfg(test)]
    fn shortest_dependency_row_key_for_target(&self, target_id: &str) -> Option<String> {
        self.store
            .dependencies
            .shortest_row_key_for_target(target_id)
    }

    #[cfg(test)]
    fn dependency_expandable_row_keys(&self) -> HashSet<String> {
        self.compute_dependency_expandable_row_keys()
    }

    fn compute_dependency_expandable_row_keys(&self) -> HashSet<String> {
        self.store.dependencies.expandable_row_keys()
    }

    fn ensure_dependency_expandable_row_keys_cache(&mut self) {
        let cache_valid = self
            .dependency_expandable_row_keys_cache
            .as_ref()
            .map(|cache| cache.rows_generation == self.store.dependencies.rows_generation)
            .unwrap_or(false);
        if !cache_valid {
            self.dependency_expandable_row_keys_cache = Some(PassDebugExpandableRowsCache {
                rows_generation: self.store.dependencies.rows_generation,
                row_keys: self.compute_dependency_expandable_row_keys(),
            });
        }
    }

    pub(crate) fn cached_dependency_expandable_row_keys(&mut self) -> &HashSet<String> {
        self.ensure_dependency_expandable_row_keys_cache();
        &self
            .dependency_expandable_row_keys_cache
            .as_ref()
            .expect("dependency expandable row cache must be initialized")
            .row_keys
    }

    #[cfg(test)]
    fn toggle_dependency_row_expanded(&mut self, row_key: &str) {
        let change = self.store.dependencies.toggle_row_expanded(row_key);
        self.apply_dependency_tree_change(change);
    }

    #[cfg(test)]
    fn visible_dependency_rows(&self) -> Vec<PassDebugDependencyRow> {
        self.compute_visible_dependency_row_indices()
            .into_iter()
            .map(|index| self.store.dependencies.rows[index].clone())
            .collect()
    }

    pub(crate) fn cached_visible_dependency_row_indices(&mut self) -> &[usize] {
        let cache_valid = self
            .visible_dependency_row_indices_cache
            .as_ref()
            .map(|cache| {
                cache.rows_generation == self.store.dependencies.rows_generation
                    && cache.expansion_generation == self.store.dependencies.expansion_generation
            })
            .unwrap_or(false);
        if !cache_valid {
            self.visible_dependency_row_indices_cache = Some(PassDebugVisibleRowsCache {
                rows_generation: self.store.dependencies.rows_generation,
                expansion_generation: self.store.dependencies.expansion_generation,
                row_indices: self.compute_visible_dependency_row_indices(),
            });
        }
        &self
            .visible_dependency_row_indices_cache
            .as_ref()
            .expect("visible dependency row cache must be initialized")
            .row_indices
    }

    pub(crate) fn cached_dependency_tree_intrinsic_width(
        &mut self,
        ui: &egui::Ui,
        font_id: &egui::FontId,
    ) -> f32 {
        let cache_valid = self
            .dependency_tree_width_cache
            .as_ref()
            .map(|cache| cache.rows_generation == self.store.dependencies.rows_generation)
            .unwrap_or(false);
        if !cache_valid {
            let text_color = ui.visuals().text_color();
            let source_jump_button_width = source_jump_button_size(ui, font_id).x;
            let intrinsic_width = self
                .store
                .dependencies
                .rows
                .iter()
                .map(|row| {
                    let indent = row.depth as f32 * TREE_ROW_INDENT_WIDTH;
                    let toggle_slot = TREE_ROW_INDENT_WIDTH;
                    let label_width = ui
                        .painter()
                        .layout_no_wrap(row.label.clone(), font_id.clone(), text_color)
                        .size()
                        .x;
                    let source_jump_width = if row.source_jump_range.is_some() {
                        TREE_ROW_SOURCE_JUMP_GAP + source_jump_button_width
                    } else {
                        0.0
                    };
                    indent
                        + toggle_slot
                        + label_width
                        + source_jump_width
                        + TREE_ROW_TRAILING_PADDING
                })
                .fold(0.0, f32::max);
            self.dependency_tree_width_cache = Some(PassDebugTreeWidthCache {
                rows_generation: self.store.dependencies.rows_generation,
                intrinsic_width,
            });
        }
        self.dependency_tree_width_cache
            .as_ref()
            .map(|cache| cache.intrinsic_width)
            .unwrap_or(0.0)
    }

    fn compute_visible_dependency_row_indices(&self) -> Vec<usize> {
        self.store.dependencies.visible_row_indices()
    }

    pub(crate) fn dependency_focus_path_row_keys(&self) -> Vec<String> {
        self.store.dependencies.focus_path_row_keys()
    }

    pub(crate) fn consume_dependency_reveal_row_key(&mut self) -> Option<String> {
        self.store.dependencies.consume_reveal_row_key()
    }

    pub(crate) fn take_pending_editor_jump(&mut self) -> Option<PassDebugSourceRange> {
        self.store.dependencies.take_pending_editor_jump()
    }

    fn handle_dependency_tree_click(
        &mut self,
        click: PassDebugTreeClick,
        pending_actions: Option<&Arc<Mutex<Vec<PassDebugWindowAction>>>>,
    ) {
        let mut handle_click = true;
        if let Some(active_row_key) = self
            .store
            .shortwire
            .active
            .as_ref()
            .map(|active| active.identity.row_key_hint.as_str())
        {
            if shortwire_click_matches_active_row(active_row_key, &click) {
                handle_click = false;
            } else if let Some(pending_actions) = pending_actions {
                self.exit_shortwire_navigate(pending_actions);
            }
        }
        if handle_click {
            self.refresh_draft_analysis();
            self.focus_tree_click(click, true);
        }
    }

    fn handle_dependency_shortwire_request(
        &mut self,
        row_index: usize,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        let Some(row) = self.store.dependencies.rows.get(row_index).cloned() else {
            return;
        };
        let patch_key = shortwire_patch_key(&row);
        let has_stored_patch = self.store.shortwire.patches.contains_key(&patch_key);

        if has_stored_patch && !self.store.shader.patch_active {
            self.enter_shortwire_and_apply(&row, pending_actions);
        } else {
            self.enter_shortwire(&row, pending_actions);
        }
    }

    pub(crate) fn record_error(&mut self, error: String) {
        if let Some(ref active) = self.store.shortwire.active {
            match &active.phase {
                ShortwirePhase::PendingResetThenEnter { .. } => {
                    self.store.shortwire.clear_active();
                    self.store.shader.record_reset_error(error);
                    return;
                }
                ShortwirePhase::PendingApply { .. } => {
                    self.store.shortwire.return_pending_apply_to_editing();
                    self.store.shader.refresh_dirty_flag();
                    self.store.shader.record_error(error);
                    return;
                }
                ShortwirePhase::Editing => {}
            }
        }
        self.store.shader.record_error(error);
    }

    fn commit_pending_merge_patch_update(&mut self, applied_source: &str) {
        let patch_hunks = self.store.shortwire.patch_hunks_snapshot();
        let Some(rebase) = self
            .store
            .merge
            .take_rebase_for_applied_source(applied_source, patch_hunks)
        else {
            return;
        };
        self.store.shortwire.apply_rebase(rebase);
    }

    pub(crate) fn open_merge_resolver(&mut self) {
        self.store.merge.open_resolver();
    }

    pub(crate) fn apply_merge_resolved(
        &mut self,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        let Some(request) = self.store.merge.apply_resolved() else {
            return;
        };
        self.push_merge_patch_request(request, pending_actions);
        self.store.shader.clear_error();
    }

    pub(crate) fn use_merge_incoming(
        &mut self,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        let Some(request) = self.store.merge.use_incoming() else {
            return;
        };
        self.push_merge_patch_request(request, pending_actions);
        self.store.shader.clear_error();
    }

    pub(crate) fn keep_merge_local(
        &mut self,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        let Some(request) = self.store.merge.keep_local() else {
            return;
        };
        self.push_merge_patch_request(request, pending_actions);
        self.store.shader.clear_error();
    }

    pub(crate) fn cancel_merge_resolution(&mut self) {
        let Some(cancel_result) = self.store.merge.cancel_resolution() else {
            return;
        };
        if self
            .store
            .shader
            .restore_runtime_source(cancel_result.restored_source, cancel_result.status)
        {
            self.invalidate_draft_render_cache();
        }
    }

    #[cfg(test)]
    fn import_reference_file_from_path(&mut self, path: &Path, now_secs: f64) {
        if self
            .store
            .reference_workspace
            .shortwire_active_key
            .is_some()
        {
            self.store.reference_workspace.last_status =
                Some("Close shortwire before opening a file".to_string());
            return;
        }
        let Some(parent) = path.parent() else {
            self.store.reference_workspace.last_status =
                Some("Cannot open file without parent".to_string());
            return;
        };
        match read_reference_file(path, parent, &self.pass_name, true) {
            Ok(file) => {
                let selected_file = Some(file.relative_path.clone());
                self.store.reference_workspace.replace_files(
                    Some(parent.to_string_lossy().to_string()),
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Reference file")
                        .to_string(),
                    vec![file.into()],
                    selected_file,
                    0,
                    true,
                );
                self.store.reference_workspace.sync_due_secs = Some(now_secs);
                self.store.reference_workspace.last_status = Some("File imported".to_string());
                self.reference_line_galley_cache = None;
            }
            Err(error) => {
                self.store.reference_workspace.last_status = Some(error);
            }
        }
    }

    pub(crate) fn apply_reference_folder_import_result(
        &mut self,
        path: &Path,
        now_secs: f64,
        result: Result<(Vec<ReferenceFileRead>, usize), String>,
    ) {
        if self
            .store
            .reference_workspace
            .apply_folder_import_result(path, now_secs, result)
        {
            self.reference_line_galley_cache = None;
        }
    }

    pub(crate) fn reload_reference_workspace(&mut self, now_secs: f64) {
        if let Some(plan) = self.store.reference_workspace.prepare_reload(now_secs) {
            self.emit_effect(PassDebugEffect::ReloadReferenceWorkspace {
                root: plan.root,
                root_label: plan.root_label,
                selected_file: plan.selected_file,
                single_file: plan.single_file,
                now_secs: plan.now_secs,
            });
        }
    }

    pub(crate) fn mark_reference_reload_missing_path(&mut self) {
        self.store.reference_workspace.mark_reload_missing_path();
    }

    pub(crate) fn apply_reference_file_reload_result(
        &mut self,
        root: &Path,
        root_label: String,
        relative_path: String,
        now_secs: f64,
        result: Result<ReferenceFileRead, String>,
    ) {
        if self.store.reference_workspace.apply_file_reload_result(
            root,
            root_label,
            relative_path,
            now_secs,
            result,
        ) {
            self.reference_line_galley_cache = None;
        }
    }

    pub(crate) fn apply_reference_folder_reload_result(
        &mut self,
        root: &Path,
        root_label: String,
        selected_file: Option<String>,
        now_secs: f64,
        result: Result<(Vec<ReferenceFileRead>, usize), String>,
    ) {
        if self.store.reference_workspace.apply_folder_reload_result(
            root,
            root_label,
            selected_file,
            now_secs,
            result,
        ) {
            self.reference_line_galley_cache = None;
        }
    }

    #[cfg(test)]
    fn take_reference_workspace_dirty_artifacts(&mut self) -> Vec<(DebugArtifactItem, String)> {
        self.store.reference_workspace.commit_editor_to_selected();
        let plan = self.store.reference_workspace.build_sync_plan();
        let completion = self.run_reference_sync_plan(plan);
        self.apply_reference_sync_completion(completion)
    }

    #[cfg(test)]
    fn collect_reference_workspace_artifact_from_plan(
        &self,
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
        let item = reference_workspace_artifact_item(&self.pass_name, &content_text);
        Some((item, content_text))
    }

    #[cfg(test)]
    fn run_reference_sync_plan(&self, plan: ReferenceSyncPlan) -> ReferenceSyncCompletion {
        let mut artifacts = Vec::new();
        let mut synced_files = Vec::new();
        let mut write_errors = Vec::new();
        let mut wrote_any_file = false;

        if let Some(root_path) = plan.root_path.as_deref() {
            let root = PathBuf::from(root_path);
            for file in plan.files.iter().filter(|file| file.is_dirty()) {
                match write_reference_workspace_file(&root, &file.relative_path, &file.source) {
                    Ok(()) => {
                        wrote_any_file = true;
                        synced_files.push(ReferenceSyncedFile {
                            relative_path: file.relative_path.clone(),
                            source: file.source.clone(),
                        });
                    }
                    Err(error) => {
                        write_errors.push(error);
                    }
                }
            }

            let should_upsert_manifest = plan.manifest_dirty || wrote_any_file;
            let mut emitted_manifest = false;
            if should_upsert_manifest
                && let Some(artifact) = self.collect_reference_workspace_artifact_from_plan(&plan)
            {
                emitted_manifest = true;
                artifacts.push(artifact);
            }
            return ReferenceSyncCompletion {
                plan,
                artifacts,
                synced_files,
                write_errors,
                emitted_manifest,
            };
        }

        let mut emitted_manifest = false;
        if plan.manifest_dirty
            && let Some(artifact) = self.collect_reference_workspace_artifact_from_plan(&plan)
        {
            emitted_manifest = true;
            artifacts.push(artifact);
        }

        for file in plan.files.iter().filter(|file| file.is_dirty()) {
            let item = pass_reference_file_artifact_item(
                &self.pass_name,
                &file.relative_path,
                &file.artifact_id,
                file.size,
                file.content_hash.clone(),
            );
            artifacts.push((item, file.source.clone()));
            synced_files.push(ReferenceSyncedFile {
                relative_path: file.relative_path.clone(),
                source: file.source.clone(),
            });
        }

        ReferenceSyncCompletion {
            plan,
            artifacts,
            synced_files,
            write_errors,
            emitted_manifest,
        }
    }

    pub(crate) fn apply_reference_sync_completion(
        &mut self,
        completion: ReferenceSyncCompletion,
    ) -> Vec<(DebugArtifactItem, String)> {
        self.store
            .reference_workspace
            .apply_sync_completion(completion)
    }

    fn restore_reference_patches_from_text(&mut self, text: Option<&str>) {
        self.store
            .reference_workspace
            .restore_reference_patches_from_text(text);
    }

    pub(crate) fn take_reference_patches_dirty_artifact(
        &mut self,
    ) -> Option<(DebugArtifactItem, String)> {
        self.store
            .reference_workspace
            .take_reference_patches_dirty_artifact(&self.pass_name)
    }

    fn enter_reference_shortwire(&mut self, identity: &ShortwireRowIdentity) {
        let Some(result) = self
            .store
            .reference_workspace
            .enter_shortwire(&identity.patch_key)
        else {
            return;
        };
        if let Some(request) = result.local_read {
            self.emit_effect(PassDebugEffect::ReadReferenceShortwireFile {
                path: request.path,
                write_after_read: request.write_after_read,
            });
        }
        self.reference_line_galley_cache = None;
    }

    pub(crate) fn apply_reference_shortwire_local_snapshot(
        &mut self,
        path: PathBuf,
        result: Result<String, String>,
        write_after_read: bool,
    ) {
        let request = self
            .store
            .reference_workspace
            .apply_shortwire_local_snapshot(path, result, write_after_read);
        self.emit_reference_shortwire_write_request(request);
    }

    fn prepare_reference_shortwire_save(&mut self) {
        self.store.reference_workspace.prepare_shortwire_save();
    }

    fn commit_reference_shortwire_after_left_apply(&mut self, restore_after: bool) {
        let should_write_local_file = self
            .store
            .reference_workspace
            .commit_pending_shortwire_after_left_apply();
        if restore_after {
            let result = self.store.reference_workspace.restore_after_shortwire();
            self.emit_reference_shortwire_restore_result(result);
        } else if should_write_local_file {
            let request = self.store.reference_workspace.shortwire_write_request();
            self.emit_reference_shortwire_write_request(request);
        }
    }

    pub(crate) fn apply_reference_shortwire_local_write_result(
        &mut self,
        path: PathBuf,
        result: Result<(), String>,
    ) {
        self.store
            .reference_workspace
            .apply_shortwire_local_write_result(&path, result);
    }

    pub(crate) fn apply_reference_shortwire_local_restore_result(
        &mut self,
        path: PathBuf,
        result: Result<(), String>,
    ) {
        if let Some(message) = self
            .store
            .reference_workspace
            .apply_shortwire_local_restore_result(&path, result)
        {
            eprintln!("[pass-debug] {message}");
        }
    }

    fn save_and_exit_reference_shortwire_without_left_apply(&mut self) {
        let result = self
            .store
            .reference_workspace
            .save_and_exit_shortwire_without_left_apply();
        self.emit_reference_shortwire_restore_result(result);
    }

    #[cfg(test)]
    fn cancel_reference_shortwire_without_save(&mut self) {
        let result = self
            .store
            .reference_workspace
            .cancel_shortwire_without_save();
        self.emit_reference_shortwire_restore_result(result);
    }

    fn emit_reference_shortwire_write_request(
        &mut self,
        request: Option<ReferenceShortwireFileWrite>,
    ) {
        if let Some(request) = request {
            self.emit_effect(PassDebugEffect::WriteReferenceShortwireFile {
                path: request.path,
                content: request.content,
            });
        }
    }

    fn emit_reference_shortwire_restore_result(&mut self, result: ReferenceShortwireRestoreResult) {
        if let Some(request) = result.restore_file {
            self.emit_effect(PassDebugEffect::RestoreReferenceShortwireFile {
                path: request.path,
                content: request.content,
            });
        }
        if result.editor_restored {
            self.reference_line_galley_cache = None;
        }
    }

    // --- Shortwire methods ---

    pub(crate) fn enter_shortwire(
        &mut self,
        row: &PassDebugDependencyRow,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        if self.store.shortwire.active.is_some()
            || self.store.shader.generated_base_source.is_empty()
        {
            return;
        }

        let identity = ShortwireRowIdentity {
            patch_key: shortwire_patch_key(row),
            row_key_hint: row.row_key.clone(),
            label: row.label.clone(),
            target_id: row.target_id.clone(),
        };
        eprintln!(
            "[shortwire-diff] enter_shortwire pass={} label={} target={:?} patch_key={} patch_active={} base_empty={} {}",
            self.pass_name,
            identity.label,
            identity.target_id,
            identity.patch_key,
            self.store.shader.patch_active,
            self.store.shader.generated_base_source.is_empty(),
            shortwire_patch_summary(self.store.shortwire.patches.get(&identity.patch_key)),
        );
        let existing_reference_image = self
            .store
            .shortwire
            .reference_image_for_patch(&identity.patch_key);

        if self.store.shader.patch_active {
            self.store
                .shortwire
                .start_pending_reset_then_enter(identity, existing_reference_image);
            self.emit_window_effect(
                PassDebugEffect::ResetPatch {
                    pass_name: self.pass_name.clone(),
                },
                pending_actions,
            );
            self.store.shader.set_status("Resetting...".to_string());
        } else {
            self.store.shortwire.start_editing(
                identity,
                self.store.shader.generated_base_source.clone(),
                self.store.shader.generated_base_source_hash,
                existing_reference_image,
            );
            self.complete_shortwire_entry();
        }
    }

    fn complete_shortwire_entry(&mut self) {
        let Some(identity) = self.store.shortwire.complete_entry(
            self.store.shader.generated_base_source.clone(),
            self.store.shader.generated_base_source_hash,
        ) else {
            return;
        };
        self.enter_reference_shortwire(&identity);

        let mut draft = self.store.shader.generated_base_source.clone();
        eprintln!(
            "[shortwire-diff] complete_entry pass={} patch_key={} current_base_hash={} {}",
            self.pass_name,
            identity.patch_key,
            self.store.shader.generated_base_source_hash,
            shortwire_patch_summary(self.store.shortwire.patches.get(&identity.patch_key)),
        );
        if let Some(patch) = self.store.shortwire.patches.get(&identity.patch_key) {
            if patch.base_source_hash == self.store.shader.generated_base_source_hash {
                match apply_hunks(&self.store.shader.generated_base_source, &patch.hunks) {
                    Ok(patched) => {
                        draft = patched;
                        self.store.shortwire.enable_active_diff_view();
                    }
                    Err(_) => {
                        self.store.shortwire.remove_patch(&identity.patch_key);
                        self.store.shader.set_error(
                            "Shortwire patch outdated — base shader changed".to_string(),
                        );
                    }
                }
            } else {
                match apply_hunks(&self.store.shader.generated_base_source, &patch.hunks) {
                    Ok(patched) => {
                        draft = patched;
                        self.store.shortwire.enable_active_diff_view();
                    }
                    Err(_) => {
                        self.store.shortwire.remove_patch(&identity.patch_key);
                        self.store.shader.set_error(
                            "Shortwire patch outdated — base shader changed".to_string(),
                        );
                    }
                }
            }
        }

        self.replace_draft_source(draft);
        self.store.shader.refresh_dirty_flag();
        self.draft_analysis_due_secs = None;
        self.store.shader.clear_status();
        eprintln!(
            "[shortwire-diff] active pass={} label={} target={:?} patch_key={} draft_len={} dirty={}",
            self.pass_name,
            identity.label,
            identity.target_id,
            identity.patch_key,
            self.store.shader.draft_source.len(),
            self.store.shader.dirty,
        );
    }

    pub(crate) fn exit_shortwire_done(
        &mut self,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        let Some(ref active) = self.store.shortwire.active else {
            return;
        };
        if !matches!(active.phase, ShortwirePhase::Editing) {
            return;
        }

        let mut final_draft = self.store.shader.draft_source.clone();
        let base_source_stale = active.base_source_stale;
        let base_source = active.base_source.clone();
        let active_label = active.identity.label.clone();
        let active_target_id = active.identity.target_id.clone();

        if base_source_stale {
            let user_hunks = compute_hunks(&base_source, &self.store.shader.draft_source);
            if user_hunks.is_empty() {
                final_draft = self.store.shader.generated_base_source.clone();
            } else {
                match apply_hunks(&self.store.shader.generated_base_source, &user_hunks) {
                    Ok(rebased) => {
                        final_draft = rebased;
                    }
                    Err(_) => {
                        self.store.shader.set_error(
                            "Cannot rebase onto new base — resolve conflicts manually".to_string(),
                        );
                        return;
                    }
                }
            }
            self.replace_draft_source(final_draft.clone());
        }

        let final_hunks = compute_hunks(&self.store.shader.generated_base_source, &final_draft);
        eprintln!(
            "[shortwire-diff] save_apply pass={} label={} target={:?} hunks={} base_stale={} previous_{}",
            self.pass_name,
            active_label,
            active_target_id,
            final_hunks.len(),
            base_source_stale,
            shortwire_patch_summary(self.store.shortwire.active.as_ref().and_then(|active| {
                self.store.shortwire.patches.get(&active.identity.patch_key)
            }),),
        );
        self.prepare_reference_shortwire_save();
        self.store.shortwire.clear_exit_on_apply();
        self.store.shortwire.set_active_pending_apply(final_hunks);

        self.emit_window_effect(
            PassDebugEffect::ApplyPatch {
                pass_name: self.pass_name.clone(),
                source: final_draft,
                reference_image: self
                    .store
                    .shortwire
                    .active
                    .as_ref()
                    .and_then(|active| active.reference_image.clone()),
            },
            pending_actions,
        );
        self.store.shader.mark_shortwire_saving();
    }

    #[cfg(test)]
    fn exit_shortwire_cancel(&mut self) {
        self.cancel_reference_shortwire_without_save();
        self.store.shortwire.clear_active();
        self.replace_draft_source(self.store.shader.generated_base_source.clone());
        self.store.shader.refresh_dirty_flag();
        self.refresh_analysis_rows();
        self.store.shader.clear_error();
        self.store.shader.clear_status();
    }

    pub(crate) fn exit_shortwire_navigate(
        &mut self,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        let Some(active) = self.store.shortwire.active.clone() else {
            return;
        };
        eprintln!(
            "[shortwire-diff] close pass={} label={} target={:?} phase={:?}",
            self.pass_name, active.identity.label, active.identity.target_id, active.phase,
        );

        match &active.phase {
            ShortwirePhase::Editing => {
                self.save_and_exit_reference_shortwire_without_left_apply();
                let hunks = compute_hunks(
                    &self.store.shader.generated_base_source,
                    &self.store.shader.draft_source,
                );
                if !hunks.is_empty() || active.reference_image.is_some() {
                    let patch_key = active.identity.patch_key.clone();
                    let previous_summary =
                        shortwire_patch_summary(self.store.shortwire.patches.get(&patch_key));
                    let stored = self.store.shortwire.store_close_patch(
                        patch_key.clone(),
                        hunks,
                        self.store.shader.generated_base_source_hash,
                        active.reference_image.clone(),
                    );
                    eprintln!(
                        "[shortwire-diff] close_store_pending pass={} patch_key={} hunks={} base_hash={} preserved_diff={} previous_{}",
                        self.pass_name,
                        patch_key,
                        stored.hunk_count,
                        stored.base_source_hash,
                        stored.preserved_diff_result,
                        previous_summary,
                    );
                }
                self.store.shortwire.clear_active();
                if self.store.shader.restore_generated_base_runtime() {
                    self.invalidate_draft_render_cache();
                }
                if self.store.shader.patch_active {
                    self.emit_window_effect(
                        PassDebugEffect::ResetPatch {
                            pass_name: self.pass_name.clone(),
                        },
                        pending_actions,
                    );
                    self.store.shader.mark_resetting_after_shortwire_exit();
                }
                self.refresh_analysis_rows();
            }
            ShortwirePhase::PendingApply { .. } => {
                self.store
                    .shortwire
                    .request_exit_on_apply_and_clear_active();
                if self.store.shader.restore_generated_base_runtime() {
                    self.invalidate_draft_render_cache();
                }
                if self.store.shader.patch_active {
                    self.emit_window_effect(
                        PassDebugEffect::ResetPatch {
                            pass_name: self.pass_name.clone(),
                        },
                        pending_actions,
                    );
                    self.store.shader.mark_resetting_after_shortwire_exit();
                }
                self.refresh_analysis_rows();
            }
            ShortwirePhase::PendingResetThenEnter { .. } => {
                self.store.shortwire.clear_active();
                if self.store.shader.restore_generated_base_runtime() {
                    self.invalidate_draft_render_cache();
                }
                self.refresh_analysis_rows();
            }
        }
        self.store
            .shader
            .clear_shortwire_exit_error_and_idle_status();
    }

    pub(crate) fn prepare_debug_window_close(
        &mut self,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        if self.store.shortwire.active.is_some() {
            self.exit_shortwire_navigate(pending_actions);
        }
    }

    pub(crate) fn enter_shortwire_and_apply(
        &mut self,
        row: &PassDebugDependencyRow,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        if self.store.shortwire.active.is_some()
            || self.store.shader.generated_base_source.is_empty()
        {
            return;
        }

        let patch_key = shortwire_patch_key(row);
        let patch = match self.store.shortwire.patches.get(&patch_key) {
            Some(p) => p.clone(),
            None => {
                self.enter_shortwire(row, pending_actions);
                return;
            }
        };
        eprintln!(
            "[shortwire-diff] enter_and_apply_stored pass={} patch_key={} {}",
            self.pass_name,
            patch_key,
            shortwire_patch_summary(Some(&patch)),
        );

        match apply_hunks(&self.store.shader.generated_base_source, &patch.hunks) {
            Ok(patched) => {
                let identity = ShortwireRowIdentity {
                    patch_key: patch_key.clone(),
                    row_key_hint: row.row_key.clone(),
                    label: row.label.clone(),
                    target_id: row.target_id.clone(),
                };
                self.store.shortwire.start_pending_apply(
                    identity,
                    self.store.shader.generated_base_source.clone(),
                    self.store.shader.generated_base_source_hash,
                    patch.reference_image.clone(),
                    patch.hunks.clone(),
                );
                let identity = self
                    .store
                    .shortwire
                    .active
                    .as_ref()
                    .map(|active| active.identity.clone());
                if let Some(identity) = identity.as_ref() {
                    self.enter_reference_shortwire(identity);
                }
                self.replace_draft_source(patched.clone());
                self.store.shader.mark_stored_patch_applying();
                self.emit_window_effect(
                    PassDebugEffect::ApplyPatch {
                        pass_name: self.pass_name.clone(),
                        source: patched,
                        reference_image: patch.reference_image.clone(),
                    },
                    pending_actions,
                );
            }
            Err(_) => {
                self.store.shortwire.remove_patch(&patch_key);
                self.enter_shortwire(row, pending_actions);
                self.store
                    .shader
                    .set_error("Stored patch outdated — entering edit mode".to_string());
            }
        }
    }

    pub(crate) fn shortwire_is_editor_interactive(&self) -> bool {
        self.store.shortwire.is_editor_interactive()
    }

    pub(crate) fn has_active_shortwire(&self) -> bool {
        self.store.shortwire.active.is_some()
    }

    pub(crate) fn can_restore_shortwire_patches_from_artifact(&self) -> bool {
        self.store.shortwire.active.is_none()
            && !self.store.shortwire.patches_dirty
            && !self.store.shader.dirty
    }

    pub(crate) fn restore_shortwire_patches_from_text(&mut self, text: &str) -> bool {
        self.store.shortwire.restore_patches_from_text(text)
    }

    pub(crate) fn shortwire_patch_count(&self) -> usize {
        self.store.shortwire.patches.len()
    }

    pub(crate) fn take_patches_dirty_artifact(&mut self) -> Option<(DebugArtifactItem, String)> {
        self.store
            .shortwire
            .take_patches_dirty_artifact(&self.pass_name)
    }

    pub(crate) fn record_shortwire_diff_result(
        &mut self,
        request: &ShortwireDiffCaptureRequest,
        diff_result: ShortwireDiffResult,
    ) -> Vec<(DebugArtifactItem, String)> {
        let status = shortwire_diff_status(&diff_result);
        let summary = shortwire_diff_result_summary(Some(&diff_result));
        if !self
            .store
            .shortwire
            .record_diff_result(request.patch_key.as_str(), diff_result)
        {
            eprintln!(
                "[shortwire-diff] record_result_missing_patch pass={} patch_key={} {}",
                request.pass_name, request.patch_key, summary,
            );
            return Vec::new();
        }
        eprintln!(
            "[shortwire-diff] record_result pass={} patch_key={} status={:?} pass_threshold={:.6} {}",
            request.pass_name, request.patch_key, status, SHORTWIRE_DIFF_PASS_MAX_AE, summary,
        );
        self.take_patches_dirty_artifact().into_iter().collect()
    }

    pub(crate) fn request_shortwire_diff_capture(
        &mut self,
        pasted_reference: &mut Option<ShortwirePastedReferenceImage>,
    ) -> ShortwireDiffCaptureAttempt {
        let Some(active) = self.store.shortwire.active.as_ref() else {
            return ShortwireDiffCaptureAttempt::Inactive;
        };
        let patch_key = active.identity.patch_key.clone();
        let pass_name = self.pass_name.clone();
        let mut binary_artifacts = Vec::new();
        let reference_image = pasted_reference.take().map(|pasted| {
            let (image, item, bytes) =
                shortwire_reference_image_artifact(&pass_name, &patch_key, pasted);
            binary_artifacts.push((item, bytes));
            image
        });
        self.store
            .shortwire
            .set_active_reference_image(reference_image.clone());
        let hunks = compute_hunks(
            &self.store.shader.generated_base_source,
            &self.store.shader.draft_source,
        );
        let hunk_count = hunks.len();
        if self.store.shortwire.create_image_patch_if_missing(
            patch_key.clone(),
            hunks,
            self.store.shader.generated_base_source_hash,
            reference_image.clone(),
        ) {
            eprintln!(
                "[shortwire-diff] request_capture_create_image_patch pass={} patch_key={} hunks={} has_image=true",
                pass_name, patch_key, hunk_count,
            );
        }
        let Some(patch_summary) = self
            .store
            .shortwire
            .patches
            .get(patch_key.as_str())
            .map(|patch| shortwire_patch_summary(Some(patch)))
        else {
            eprintln!(
                "[shortwire-diff] request_capture_no_patch pass={} patch_key={}",
                pass_name, patch_key,
            );
            return ShortwireDiffCaptureAttempt::MissingPatch;
        };

        eprintln!(
            "[shortwire-diff] request_capture_clear_previous pass={} patch_key={} {}",
            pass_name, patch_key, patch_summary,
        );
        self.store
            .shortwire
            .prepare_diff_capture_patch(patch_key.as_str(), reference_image);
        let artifacts = self.take_patches_dirty_artifact().into_iter().collect();
        eprintln!(
            "[shortwire-diff] request_capture_queued pass={} patch_key={}",
            pass_name, patch_key,
        );
        ShortwireDiffCaptureAttempt::Captured(PassDebugPatchApplyResult {
            artifacts,
            binary_artifacts,
            diff_capture: Some(ShortwireDiffCaptureRequest {
                pass_name,
                patch_key,
            }),
        })
    }
}

fn push_action(
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    action: PassDebugWindowAction,
) {
    push_window_action(pending_actions, action);
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        PassDebugDependencyRow, PassDebugEffect, PassDebugTreeClick, PassDebugWindowDocument,
        ReferenceWorkspaceFile, ReferenceWorkspaceState, byte_index_to_char_index,
        dependency_path_for_row_key, flatten_dependency_tree, shortwire_click_matches_active_row,
    };
    use crate::app::{
        RefImageAlphaMode, RefImageMode, ShortwirePastedReferenceImage, ShortwireReferenceImage,
    };
    use crate::dsl::{DebugArtifactAnchor, DebugArtifactItem, DebugArtifactRole};
    use crate::renderer::{
        PassDebugDependencyNode, PassDebugDependencyTarget, PassDebugSource, PassDebugSourceRange,
    };

    fn has_target_named(document: &PassDebugWindowDocument, name: &str) -> bool {
        document
            .store
            .shader
            .analysis_source
            .as_ref()
            .map(|source| {
                source
                    .dependency_targets
                    .iter()
                    .any(|target| target.name == name)
            })
            .unwrap_or(false)
    }

    fn target_id_by_name(document: &PassDebugWindowDocument, name: &str) -> String {
        document
            .store
            .shader
            .analysis_source
            .as_ref()
            .and_then(|source| {
                source
                    .dependency_targets
                    .iter()
                    .find(|target| target.name == name)
            })
            .map(|target| target.id.clone())
            .unwrap_or_else(|| panic!("missing target named {name}"))
    }

    fn dependency_root_target_name(document: &PassDebugWindowDocument) -> String {
        let source = document
            .store
            .shader
            .analysis_source
            .as_ref()
            .expect("missing analysis source");
        let root_id = document
            .store
            .dependencies
            .root_target_id
            .as_deref()
            .expect("missing dependency root target");
        source
            .dependency_targets
            .iter()
            .find(|target| target.id == root_id)
            .map(|target| target.name.clone())
            .unwrap_or_else(|| panic!("missing root target id {root_id}"))
    }

    fn root_return_shader(root_name: &str, value: f32) -> String {
        format!(
            "@fragment\nfn fs_main() -> @location(0) f32 {{ let {root_name} = {value:.1}; return {root_name}; }}\n"
        )
    }

    fn dependency_rows_contain_label_fragment(
        document: &PassDebugWindowDocument,
        fragment: &str,
    ) -> bool {
        document
            .store
            .dependencies
            .rows
            .iter()
            .any(|row| row.label.contains(fragment))
    }

    fn source_target_id_by_name(source: &PassDebugSource, name: &str) -> String {
        source
            .dependency_targets
            .iter()
            .find(|target| target.name == name)
            .map(|target| target.id.clone())
            .unwrap_or_else(|| panic!("missing target named {name}"))
    }

    fn unique_reference_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "node-forge-pass-debug-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn drain_reference_shortwire_file_effects(document: &mut PassDebugWindowDocument) {
        loop {
            let effects = document.drain_effects();
            if effects.is_empty() {
                break;
            }
            let mut handled_any = false;
            for effect in effects {
                match effect {
                    PassDebugEffect::ReadReferenceShortwireFile {
                        path,
                        write_after_read,
                    } => {
                        let result = fs::read_to_string(&path).map_err(|error| {
                            format!("Reference local restore unavailable: {error}")
                        });
                        document.apply_reference_shortwire_local_snapshot(
                            path,
                            result,
                            write_after_read,
                        );
                        handled_any = true;
                    }
                    PassDebugEffect::WriteReferenceShortwireFile { path, content } => {
                        let result = fs::write(&path, content)
                            .map_err(|error| format!("Reference file write failed: {error}"));
                        document.apply_reference_shortwire_local_write_result(path, result);
                        handled_any = true;
                    }
                    PassDebugEffect::RestoreReferenceShortwireFile { path, content } => {
                        let result = fs::write(&path, content)
                            .map_err(|error| format!("Reference file write failed: {error}"));
                        document.apply_reference_shortwire_local_restore_result(path, result);
                        handled_any = true;
                    }
                    other => document.emit_effect(other),
                }
            }
            if !handled_any {
                break;
            }
        }
    }

    fn drain_reference_manifest_read_effects(document: &mut PassDebugWindowDocument) {
        loop {
            let effects = document.drain_effects();
            if effects.is_empty() {
                break;
            }
            let mut handled_any = false;
            for effect in effects {
                match effect {
                    PassDebugEffect::ReadReferenceManifestFiles { root, manifest } => {
                        let mut files = Vec::new();
                        let mut local_loaded_count = 0usize;
                        let mut missing_count = 0usize;
                        for manifest_file in manifest.files {
                            match crate::ui::pass_debug::file_io::read_manifest_reference_file(
                                &root,
                                &manifest_file,
                            ) {
                                Ok(source) => {
                                    local_loaded_count += 1;
                                    files.push(ReferenceWorkspaceFile {
                                        relative_path: manifest_file.relative_path,
                                        artifact_id: manifest_file.artifact_id,
                                        source: source.clone(),
                                        loaded_source: source,
                                    });
                                }
                                Err(_) => missing_count += 1,
                            }
                        }
                        let mut state = ReferenceWorkspaceState::default();
                        state.replace_files(
                            Some(root.to_string_lossy().to_string()),
                            manifest.root_label,
                            files,
                            manifest.selected_file,
                            manifest.skipped_files + missing_count,
                            false,
                        );
                        state.last_status = if missing_count > 0 {
                            Some(if local_loaded_count > 0 {
                                format!("Loaded local reference ({missing_count} missing)")
                            } else {
                                format!("Local reference missing ({missing_count} missing)")
                            })
                        } else {
                            Some("Loaded local reference".to_string())
                        };
                        document.apply_reference_manifest_local_read_result(state);
                        handled_any = true;
                    }
                    other => document.emit_effect(other),
                }
            }
            if !handled_any {
                break;
            }
        }
    }

    fn row_parent_label(rows: &[PassDebugDependencyRow], label: &str) -> Option<String> {
        let row = rows
            .iter()
            .find(|row| row.label == label)
            .unwrap_or_else(|| panic!("missing dependency row label {label}"));
        let parent_row_key = row.parent_row_key.as_deref()?;
        rows.iter()
            .find(|row| row.row_key == parent_row_key)
            .map(|row| row.label.clone())
    }

    fn assert_row_parent_label(rows: &[PassDebugDependencyRow], label: &str, parent_label: &str) {
        let found = rows.iter().any(|row| {
            row.label == label
                && row.parent_row_key.as_deref().and_then(|parent_row_key| {
                    rows.iter()
                        .find(|parent| parent.row_key == parent_row_key)
                        .map(|parent| parent.label.as_str())
                }) == Some(parent_label)
        });
        assert!(
            found,
            "missing dependency row `{label}` under `{parent_label}`\nrows:\n{}",
            rows.iter()
                .map(|row| {
                    let parent_label = row
                        .parent_row_key
                        .as_deref()
                        .and_then(|parent_row_key| {
                            rows.iter()
                                .find(|parent| parent.row_key == parent_row_key)
                                .map(|parent| parent.label.as_str())
                        })
                        .unwrap_or("<root>");
                    format!("{} <- {parent_label}", row.label)
                })
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    fn dependency_row_by_label<'a>(
        rows: &'a [PassDebugDependencyRow],
        label: &str,
    ) -> &'a PassDebugDependencyRow {
        rows.iter()
            .find(|row| row.label == label)
            .unwrap_or_else(|| {
                panic!(
                    "missing dependency row `{label}`\nrows:\n{}",
                    rows.iter()
                        .map(|row| row.label.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            })
    }

    fn seed_reference_file(
        document: &mut PassDebugWindowDocument,
        relative_path: &str,
        source: &str,
    ) {
        let file = super::ReferenceWorkspaceFile {
            relative_path: relative_path.to_string(),
            artifact_id: super::pass_reference_file_artifact_id(&document.pass_name, relative_path),
            source: source.to_string(),
            loaded_source: source.to_string(),
        };
        document.store.reference_workspace.replace_files(
            None,
            "Reference test".to_string(),
            vec![file],
            Some(relative_path.to_string()),
            0,
            false,
        );
    }

    fn reference_file_item(pass_name: &str, relative_path: &str) -> DebugArtifactItem {
        DebugArtifactItem {
            id: super::pass_reference_file_artifact_id(pass_name, relative_path),
            anchor: DebugArtifactAnchor::Pass {
                pass_name: pass_name.to_string(),
            },
            role: DebugArtifactRole::ReferenceCode,
            name: format!("Reference: {relative_path}"),
            mime_type: "text/plain".to_string(),
            path: format!("debug-artifacts/test/{relative_path}"),
            size: None,
            content_hash: None,
            slot_key: Some(super::pass_reference_file_slot_key(relative_path)),
        }
    }

    fn reference_snapshot(
        pass_name: &str,
        relative_path: &str,
        text: &str,
    ) -> crate::debug_artifacts::DebugArtifactTextSnapshot {
        crate::debug_artifacts::DebugArtifactTextSnapshot {
            item: reference_file_item(pass_name, relative_path),
            text: text.to_string(),
        }
    }

    #[test]
    fn dirty_draft_is_not_replaced_by_source_refresh() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.store.shader.draft_source = "fn edited() {}\n".to_string();
        document.store.shader.dirty = true;

        let refreshed = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        document.update_source(Some(&refreshed), 1, false);

        assert_eq!(document.store.shader.draft_source, "fn edited() {}\n");
        assert!(document.store.shader.dirty);
    }

    #[test]
    fn clean_document_tracks_source_refresh() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);

        let refreshed = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        document.update_source(Some(&refreshed), 1, Some("fn patched() {}\n"));

        assert_eq!(document.store.shader.draft_source, "fn patched() {}\n");
        assert_eq!(
            document.store.shader.generated_base_source,
            "fn generated() {}\n"
        );
        assert!(document.store.shader.patch_active);
        assert!(!document.store.shader.dirty);
    }

    #[test]
    fn same_source_revision_does_not_refresh_document() {
        let source_text = root_return_shader("a", 1.0);
        let source = PassDebugSource::from_wgsl("p", source_text.clone());
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 7, false);

        let refreshed = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        document.update_source(Some(&refreshed), 7, None);

        assert_eq!(document.store.shader.draft_source, source_text);
        assert!(!document.store.shader.patch_active);
    }

    #[test]
    fn target_list_refreshes_after_source_update() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var before: f32 = 0.0; return before; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        assert!(has_target_named(&document, "before"));

        let refreshed = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var after: f32 = 1.0; return after; }\n",
        );
        document.update_source(Some(&refreshed), 1, false);

        assert!(!has_target_named(&document, "before"));
        assert!(has_target_named(&document, "after"));
        assert!(!document.store.dependencies.rows.is_empty());
    }

    #[test]
    fn dirty_draft_does_not_replace_canonical_dependency_tree() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var loaded: f32 = 0.0; return loaded; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.store.shader.draft_source =
            "fn a() -> f32 { var draft: f32 = 1.0; return draft; }\n".to_string();
        document.store.shader.dirty = true;
        document.refresh_draft_analysis();
        assert!(has_target_named(&document, "loaded"));
        assert!(!has_target_named(&document, "draft"));

        let refreshed = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var generated: f32 = 2.0; return generated; }\n",
        );
        document.update_source(Some(&refreshed), 1, false);

        assert_eq!(
            document.store.shader.draft_source,
            "fn a() -> f32 { var draft: f32 = 1.0; return draft; }\n"
        );
        assert!(!has_target_named(&document, "draft"));
        assert!(has_target_named(&document, "generated"));
    }

    #[test]
    fn draft_edits_do_not_schedule_dependency_analysis() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var before: f32 = 0.0; return before; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.replace_draft_source(
            "fn a() -> f32 { var after: f32 = 1.0; return after; }\n".to_string(),
        );
        document.mark_draft_edited(10.0);

        document.maybe_refresh_pending_draft_analysis(10.10);
        assert!(has_target_named(&document, "before"));
        assert!(!has_target_named(&document, "after"));
        assert!(document.draft_analysis_due_secs.is_none());

        document.maybe_refresh_pending_draft_analysis(10.16);
        assert!(has_target_named(&document, "before"));
        assert!(!has_target_named(&document, "after"));
        assert_eq!(document.draft_analysis_due_secs, None);
    }

    #[test]
    fn forced_draft_analysis_keeps_canonical_dependency_source() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var before: f32 = 0.0; return before; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.replace_draft_source(
            "fn a() -> f32 { var forced: f32 = 1.0; return forced; }\n".to_string(),
        );
        document.mark_draft_edited(20.0);

        document.refresh_draft_analysis();

        assert!(has_target_named(&document, "before"));
        assert!(!has_target_named(&document, "forced"));
        assert_eq!(document.draft_analysis_due_secs, None);
    }

    #[test]
    fn dependency_render_caches_invalidate_on_expansion_and_source_refresh() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) f32 { var a: f32 = 0.0; let b = a + 1.0; return b; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        assert!(document.visible_dependency_row_indices_cache.is_none());

        let first_visible = document.cached_visible_dependency_row_indices().to_vec();
        assert!(!first_visible.is_empty());
        assert!(document.visible_dependency_row_indices_cache.is_some());
        let first_rows_generation = document.store.dependencies.rows_generation;
        let first_expansion_generation = document.store.dependencies.expansion_generation;

        document.toggle_dependency_row_expanded("0");
        assert!(document.visible_dependency_row_indices_cache.is_none());
        assert_ne!(
            document.store.dependencies.expansion_generation,
            first_expansion_generation
        );

        let _ = document.cached_visible_dependency_row_indices();
        assert!(document.visible_dependency_row_indices_cache.is_some());
        let refreshed = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) f32 { var c: f32 = 2.0; return c; }\n",
        );
        document.update_source(Some(&refreshed), 1, false);

        assert!(document.visible_dependency_row_indices_cache.is_none());
        assert!(document.dependency_expandable_row_keys_cache.is_none());
        assert!(document.dependency_tree_width_cache.is_none());
        assert_ne!(
            document.store.dependencies.rows_generation,
            first_rows_generation
        );
    }

    #[test]
    fn focusing_dependency_child_does_not_replace_root_tree() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) vec4f { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; return vec4f(c, c, c, 1.0); }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let root_id = document.store.dependencies.root_target_id.clone().unwrap();
        let child_id = target_id_by_name(&document, "b");

        document.focus_target(child_id.clone(), true);

        assert_eq!(
            document.store.dependencies.root_target_id.as_deref(),
            Some(root_id.as_str())
        );
        assert_eq!(
            document.store.dependencies.focused_target_id.as_deref(),
            Some(child_id.as_str())
        );
        assert_eq!(
            document.store.dependencies.rows[0].target_id.as_deref(),
            Some(root_id.as_str())
        );
    }

    #[test]
    fn dependency_root_is_fragment_return_target() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) f32 { var a: f32 = 0.0; let b = a + 1.0; return b; }\n",
        );
        let document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);

        assert_eq!(
            document.store.dependencies.root_target_id.as_deref(),
            Some("fs_main::return")
        );
        assert_eq!(
            document.store.dependencies.rows[0].target_id.as_deref(),
            Some("fs_main::return")
        );
    }

    #[test]
    fn dependency_rows_default_to_only_root_expanded() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) f32 { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; return c; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);

        assert_eq!(
            document.store.dependencies.expanded_row_keys,
            HashSet::from(["0".to_string()])
        );
        let visible_labels = document
            .visible_dependency_rows()
            .iter()
            .map(|row| row.label.clone())
            .collect::<Vec<_>>();
        assert_eq!(visible_labels, vec!["return".to_string(), "c".to_string()]);

        document.toggle_dependency_row_expanded("0");
        assert_eq!(
            document
                .visible_dependency_rows()
                .iter()
                .map(|row| row.label.clone())
                .collect::<Vec<_>>(),
            vec!["return".to_string()]
        );
    }

    #[test]
    fn editor_focus_expands_only_shortest_path_from_root() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) f32 { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; return c; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let a_id = target_id_by_name(&document, "a");
        let a_row_key = document
            .shortest_dependency_row_key_for_target(&a_id)
            .unwrap();
        let path = dependency_path_for_row_key(&document.store.dependencies.rows, &a_row_key);

        document.store.dependencies.expanded_row_keys = document
            .dependency_expandable_row_keys()
            .into_iter()
            .collect();
        document.focus_target_from_editor(a_id);

        let expected_expanded = path
            .iter()
            .take(path.len().saturating_sub(1))
            .cloned()
            .collect::<HashSet<_>>();
        assert_eq!(
            document.store.dependencies.expanded_row_keys,
            expected_expanded
        );
        assert_eq!(
            document
                .visible_dependency_rows()
                .iter()
                .map(|row| row.label.clone())
                .collect::<Vec<_>>(),
            vec![
                "return".to_string(),
                "c".to_string(),
                "b (Add)".to_string(),
                "a (Add)".to_string()
            ]
        );
    }

    #[test]
    fn dependency_tree_click_focuses_without_queueing_reveal_scroll() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) vec4f { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; return vec4f(c, c, c, 1.0); }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let row_key = document.visible_dependency_rows()[1].row_key.clone();

        document.store.dependencies.pending_reveal_row_key = None;
        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(row_key.clone()),
                target_id: None,
                source_range: None,
                toggle_row_key: None,
            },
            true,
        );

        assert_eq!(
            document.store.dependencies.focused_row_key.as_deref(),
            Some(row_key.as_str())
        );
        assert_eq!(document.store.dependencies.pending_reveal_row_key, None);
    }

    #[test]
    fn focusing_target_outside_current_map_does_not_move_root() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) vec4f { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; let outside = 9.0; return vec4f(c, c, c, 1.0); }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let root_id = document.store.dependencies.root_target_id.clone().unwrap();
        let outside_id = target_id_by_name(&document, "outside");

        document.focus_target(outside_id.clone(), true);

        assert_eq!(
            document.store.dependencies.root_target_id.as_deref(),
            Some(root_id.as_str())
        );
        assert_eq!(
            document.store.dependencies.focused_target_id.as_deref(),
            Some(outside_id.as_str())
        );
        assert_eq!(document.store.dependencies.focused_row_key, None);
        assert!(!document.focus_is_in_dependency_root());
    }

    #[test]
    fn dependency_rows_hide_unselectable_intermediate_nodes() {
        let source = PassDebugSource {
            pass_name: "p".to_string(),
            module_source: String::new(),
            ast_tree: Vec::new(),
            dependency_targets: Vec::new(),
            dependency_trees: HashMap::new(),
            dependency_root_target_id: None,
            dependency_error: None,
            parse_error: None,
        };
        let rows = flatten_dependency_tree(
            &PassDebugDependencyNode {
                label: "fs_main x (local)".to_string(),
                edge_label: None,
                display_label: None,
                source_range: None,
                definition_source_range: None,
                target_id: Some("target::x".to_string()),
                reference: false,
                children: vec![PassDebugDependencyNode {
                    label: "[rhs] Binary Add".to_string(),
                    edge_label: None,
                    display_label: None,
                    source_range: None,
                    definition_source_range: None,
                    target_id: None,
                    reference: false,
                    children: vec![
                        PassDebugDependencyNode {
                            label: "[source] function argument fs_main::0".to_string(),
                            edge_label: None,
                            display_label: None,
                            source_range: None,
                            definition_source_range: None,
                            target_id: None,
                            reference: false,
                            children: Vec::new(),
                        },
                        PassDebugDependencyNode {
                            label: "fs_main uv (argument)".to_string(),
                            edge_label: None,
                            display_label: None,
                            source_range: None,
                            definition_source_range: None,
                            target_id: Some("target::uv".to_string()),
                            reference: false,
                            children: Vec::new(),
                        },
                    ],
                }],
            },
            &source,
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "fs_main x (local)");
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[0].row_key, "0");
        assert_eq!(rows[0].parent_row_key, None);
        assert_eq!(rows[0].relation_path, "");
        assert_eq!(rows[0].target_id.as_deref(), Some("target::x"));
        assert_eq!(rows[1].label, "fs_main uv (argument)");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[1].row_key, "0/0/1");
        assert_eq!(rows[1].parent_row_key.as_deref(), Some("0"));
        assert!(rows[1].relation_path.contains("rhs Binary Add"));
        assert_eq!(rows[1].target_id.as_deref(), Some("target::uv"));
    }

    #[test]
    fn dependency_rows_display_target_name_with_edge_label() {
        let source = PassDebugSource {
            pass_name: "p".to_string(),
            module_source: "let a = input.foo.bar.x;".to_string(),
            ast_tree: Vec::new(),
            dependency_targets: vec![
                PassDebugDependencyTarget {
                    id: "target::d".to_string(),
                    name: "d".to_string(),
                    label: "debug_main let d".to_string(),
                    scope: "debug_main".to_string(),
                    kind: "let".to_string(),
                    source_range: None,
                },
                PassDebugDependencyTarget {
                    id: "target::a".to_string(),
                    name: "a".to_string(),
                    label: "debug_main let a".to_string(),
                    scope: "debug_main".to_string(),
                    kind: "let".to_string(),
                    source_range: Some(PassDebugSourceRange {
                        start_byte: 4,
                        end_byte: 5,
                        line: 1,
                        column: 5,
                    }),
                },
            ],
            dependency_trees: HashMap::new(),
            dependency_root_target_id: None,
            dependency_error: None,
            parse_error: None,
        };
        let rows = flatten_dependency_tree(
            &PassDebugDependencyNode {
                label: "debug_main let d (let)".to_string(),
                edge_label: None,
                display_label: None,
                source_range: None,
                definition_source_range: None,
                target_id: Some("target::d".to_string()),
                reference: false,
                children: vec![PassDebugDependencyNode {
                    label: "debug_main let a (let)".to_string(),
                    edge_label: Some("math_multiply".to_string()),
                    display_label: Some("input.foo.bar.x".to_string()),
                    source_range: Some(PassDebugSourceRange {
                        start_byte: 8,
                        end_byte: 23,
                        line: 1,
                        column: 9,
                    }),
                    definition_source_range: None,
                    target_id: Some("target::a".to_string()),
                    reference: false,
                    children: Vec::new(),
                }],
            },
            &source,
        );

        assert_eq!(rows[0].label, "d");
        assert_eq!(rows[1].label, "input.foo.bar.x (math_multiply)");
        let row_range = rows[1]
            .source_range
            .expect("expected row source range for full access path");
        assert_eq!(
            &source.module_source[row_range.start_byte..row_range.end_byte],
            "input.foo.bar.x"
        );
    }

    #[test]
    fn function_call_dependency_rows_keep_call_site_argument_subtrees() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn foo(b: f32, c: f32) -> f32 {
    return b + c;
}

fn bar(b: f32, c: f32) -> f32 {
    return b - c;
}

@fragment
fn fs_main() -> @location(0) f32 {
    let source_b = 1.0;
    let source_c = 2.0;
    let b = source_b + 10.0;
    let c = source_c + 20.0;
    let a = foo(b, c);
    let d = bar(b, c);
    return a + d;
}
"#,
        );

        let a_id = source_target_id_by_name(&source, "a");
        let a_rows = flatten_dependency_tree(
            source
                .dependency_trees
                .get(&a_id)
                .expect("a dependency tree"),
            &source,
        );
        assert_eq!(row_parent_label(&a_rows, "b (foo)").as_deref(), Some("a"));
        assert_eq!(
            row_parent_label(&a_rows, "source_b (Add)").as_deref(),
            Some("b (foo)")
        );
        assert_eq!(row_parent_label(&a_rows, "c (foo)").as_deref(), Some("a"));
        assert_eq!(
            row_parent_label(&a_rows, "source_c (Add)").as_deref(),
            Some("c (foo)")
        );

        let d_id = source_target_id_by_name(&source, "d");
        let d_rows = flatten_dependency_tree(
            source
                .dependency_trees
                .get(&d_id)
                .expect("d dependency tree"),
            &source,
        );
        assert_eq!(row_parent_label(&d_rows, "b (bar)").as_deref(), Some("d"));
        assert_eq!(
            row_parent_label(&d_rows, "source_b (Add)").as_deref(),
            Some("b (bar)")
        );
        assert_eq!(row_parent_label(&d_rows, "c (bar)").as_deref(), Some("d"));
        assert_eq!(
            row_parent_label(&d_rows, "source_c (Add)").as_deref(),
            Some("c (bar)")
        );

        let root_id = source
            .dependency_root_target_id
            .as_ref()
            .expect("dependency root target");
        let root_rows = flatten_dependency_tree(
            source
                .dependency_trees
                .get(root_id)
                .expect("root dependency tree"),
            &source,
        );
        assert_row_parent_label(&root_rows, "b (foo)", "a (Add)");
        assert_row_parent_label(&root_rows, "source_b (Add)", "b (foo)");
        assert_row_parent_label(&root_rows, "c (foo)", "a (Add)");
        assert_row_parent_label(&root_rows, "source_c (Add)", "c (foo)");
        assert_row_parent_label(&root_rows, "b (bar)", "d (Add)");
        assert_row_parent_label(&root_rows, "source_b (Add)", "b (bar)");
        assert_row_parent_label(&root_rows, "c (bar)", "d (Add)");
        assert_row_parent_label(&root_rows, "source_c (Add)", "c (bar)");
    }

    #[test]
    fn dependency_tree_click_jumps_to_reference_occurrence_range() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn foo(b: f32, c: f32) -> f32 {
    return b + c;
}

fn bar(a: f32, c: f32) -> f32 {
    return a + c;
}

@fragment
fn fs_main() -> @location(0) f32 {
    let b = 1.0;
    let c = 2.0;
    let a = foo(b, c);
    let d = bar(a, c);
    return d;
}
"#,
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let a_row = dependency_row_by_label(&document.store.dependencies.rows, "a (bar)").clone();

        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(a_row.row_key.clone()),
                target_id: None,
                source_range: None,
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .store
            .dependencies
            .pending_editor_jump
            .expect("expected dependency click to queue editor jump");
        let expected_start = document
            .store
            .shader
            .draft_source
            .find("bar(a, c)")
            .unwrap()
            + "bar(".len();
        assert_eq!(jump.start_byte, expected_start);
        assert_eq!(
            &document.store.shader.draft_source[jump.start_byte..jump.end_byte],
            "a"
        );
        assert_eq!(
            document.store.dependencies.focused_row_key.as_deref(),
            Some(a_row.row_key.as_str())
        );
    }

    #[test]
    fn reference_row_source_jump_range_jumps_to_target_source() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn foo(b: f32, c: f32) -> f32 {
    return b + c;
}

fn bar(a: f32, c: f32) -> f32 {
    return a + c;
}

@fragment
fn fs_main() -> @location(0) f32 {
    let b = 1.0;
    let c = 2.0;
    let a = foo(b, c);
    let d = bar(a, c);
    return d;
}
"#,
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let a_row = dependency_row_by_label(&document.store.dependencies.rows, "a (bar)").clone();
        let source_jump_range = a_row
            .source_jump_range
            .expect("expected reference row to expose source jump range");

        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(a_row.row_key.clone()),
                target_id: None,
                source_range: Some(source_jump_range),
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .store
            .dependencies
            .pending_editor_jump
            .expect("expected source jump to queue editor jump");
        let expected_start = document
            .store
            .shader
            .draft_source
            .find("let a = foo")
            .unwrap()
            + "let ".len();
        assert_eq!(jump.start_byte, expected_start);
        assert_eq!(
            &document.store.shader.draft_source[jump.start_byte..jump.end_byte],
            "a"
        );
        assert_eq!(
            document.store.dependencies.focused_row_key.as_deref(),
            Some(a_row.row_key.as_str())
        );
    }

    #[test]
    fn local_reference_row_clicks_occurrence_and_src_jumps_to_declaration() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
@fragment
fn fs_main() -> @location(0) f32 {
    let edge = 30.0;
    let edge_sdf = edge + 1.0;
    let aa_depth = edge * 2.0;
    var final_alpha = smoothstep(0.0, aa_depth, -edge_sdf);
    let out = 0.5 * final_alpha;
    return out;
}
"#,
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let final_alpha_row =
            dependency_row_by_label(&document.store.dependencies.rows, "final_alpha (Multiply)")
                .clone();

        let occurrence_start = document
            .store
            .shader
            .draft_source
            .find("0.5 * final_alpha")
            .unwrap()
            + "0.5 * ".len();
        let row_range = final_alpha_row
            .source_range
            .expect("expected final_alpha row occurrence range");
        assert_eq!(row_range.start_byte, occurrence_start);
        assert_eq!(
            &document.store.shader.draft_source[row_range.start_byte..row_range.end_byte],
            "final_alpha"
        );
        assert!(
            document.store.dependencies.rows.iter().any(|row| {
                row.parent_row_key.as_deref() == Some(final_alpha_row.row_key.as_str())
            }),
            "final_alpha reference row should have dependency children"
        );

        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(final_alpha_row.row_key.clone()),
                target_id: None,
                source_range: None,
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .store
            .dependencies
            .pending_editor_jump
            .expect("expected final_alpha row click to queue editor jump");
        assert_eq!(jump.start_byte, occurrence_start);
        assert_eq!(
            &document.store.shader.draft_source[jump.start_byte..jump.end_byte],
            "final_alpha"
        );

        let source_jump_range = final_alpha_row
            .source_jump_range
            .expect("expected final_alpha row to expose declaration jump");
        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(final_alpha_row.row_key.clone()),
                target_id: None,
                source_range: Some(source_jump_range),
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .store
            .dependencies
            .pending_editor_jump
            .expect("expected src jump to queue editor jump");
        let declaration_start = document
            .store
            .shader
            .draft_source
            .find("var final_alpha")
            .unwrap()
            + "var ".len();
        assert_eq!(jump.start_byte, declaration_start);
        assert_eq!(
            &document.store.shader.draft_source[jump.start_byte..jump.end_byte],
            "final_alpha"
        );
    }

    #[test]
    fn reassigned_local_definition_row_click_jumps_to_store_without_src_jump() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn foo(v: f32) -> f32 {
    return v;
}

@fragment
fn fs_main() -> @location(0) f32 {
    var x: f32 = 0.0;
    x = foo(x);
    x = foo(x);
    return x;
}
"#,
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let x_id = target_id_by_name(&document, "x");
        let latest_store_start = document
            .store
            .shader
            .draft_source
            .rfind("x = foo(x);")
            .unwrap();
        let latest_x_row = document
            .store
            .dependencies
            .rows
            .iter()
            .find(|row| {
                row.target_id.as_deref() == Some(x_id.as_str())
                    && row
                        .source_range
                        .map(|range| range.start_byte == latest_store_start)
                        .unwrap_or(false)
            })
            .cloned()
            .expect("expected latest reassignment dependency row");

        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(latest_x_row.row_key.clone()),
                target_id: None,
                source_range: None,
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .store
            .dependencies
            .pending_editor_jump
            .expect("expected reassignment row click to jump to store");
        assert_eq!(jump.start_byte, latest_store_start);
        assert_eq!(
            &document.store.shader.draft_source[jump.start_byte..jump.end_byte],
            "x"
        );

        assert_eq!(latest_x_row.source_jump_range, None);
    }

    #[test]
    fn reassigned_reference_row_clicks_occurrence_and_src_jumps_to_reaching_definition() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn fun(v: f32) -> f32 {
    return v;
}

fn foo(v: f32) -> f32 {
    return v;
}

fn bar(v: f32) -> f32 {
    return v;
}

@fragment
fn fs_main() -> @location(0) f32 {
    var a: f32 = 1.0;
    a = fun(a);
    let b = foo(a);
    let c = bar(a);
    return b + c;
}
"#,
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let analysis_source = document
            .store
            .shader
            .analysis_source
            .as_ref()
            .expect("missing analysis source");
        let a_id = source_target_id_by_name(analysis_source, "a");
        let foo_arg_start =
            document.store.shader.draft_source.find("foo(a)").unwrap() + "foo(".len();
        let store_start = document
            .store
            .shader
            .draft_source
            .find("a = fun(a);")
            .unwrap();
        let declaration_start =
            document.store.shader.draft_source.find("var a").unwrap() + "var ".len();
        let a_foo_row = document
            .store
            .dependencies
            .rows
            .iter()
            .find(|row| {
                row.target_id.as_deref() == Some(a_id.as_str())
                    && row
                        .source_range
                        .is_some_and(|range| range.start_byte == foo_arg_start)
            })
            .cloned()
            .expect("expected foo(a) dependency row");

        let row_range = a_foo_row
            .source_range
            .expect("expected foo(a) occurrence range");
        assert_eq!(row_range.start_byte, foo_arg_start);
        assert_eq!(
            &document.store.shader.draft_source[row_range.start_byte..row_range.end_byte],
            "a"
        );

        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(a_foo_row.row_key.clone()),
                target_id: None,
                source_range: None,
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .store
            .dependencies
            .pending_editor_jump
            .expect("expected row click to jump to foo(a) occurrence");
        assert_eq!(jump.start_byte, foo_arg_start);
        assert_eq!(
            &document.store.shader.draft_source[jump.start_byte..jump.end_byte],
            "a"
        );

        let source_jump_range = a_foo_row
            .source_jump_range
            .expect("expected foo(a) row to expose reaching definition jump");
        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(a_foo_row.row_key.clone()),
                target_id: None,
                source_range: Some(source_jump_range),
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .store
            .dependencies
            .pending_editor_jump
            .expect("expected src jump to go to reaching definition");
        assert_eq!(jump.start_byte, store_start);
        assert_eq!(
            &document.store.shader.draft_source[jump.start_byte..jump.end_byte],
            "a"
        );

        let fun_arg_start = store_start + "a = fun(".len();
        let a_fun_row = document
            .store
            .dependencies
            .rows
            .iter()
            .find(|row| {
                row.target_id.as_deref() == Some(a_id.as_str())
                    && row
                        .source_range
                        .is_some_and(|range| range.start_byte == fun_arg_start)
                    && dependency_path_for_row_key(&document.store.dependencies.rows, &row.row_key)
                        .iter()
                        .any(|row_key| row_key == &a_foo_row.row_key)
            })
            .cloned()
            .expect("expected nested fun(a) dependency row under foo(a)");
        let nested_range = a_fun_row
            .source_range
            .expect("expected fun(a) occurrence range");
        assert_eq!(nested_range.start_byte, fun_arg_start);
        assert_eq!(
            &document.store.shader.draft_source[nested_range.start_byte..nested_range.end_byte],
            "a"
        );
        let nested_source_jump_range = a_fun_row
            .source_jump_range
            .expect("expected nested fun(a) src jump to previous definition");
        assert_eq!(nested_source_jump_range.start_byte, declaration_start);
        assert_eq!(
            &document.store.shader.draft_source
                [nested_source_jump_range.start_byte..nested_source_jump_range.end_byte],
            "a"
        );
    }

    #[test]
    fn editor_click_on_reference_focuses_matching_dependency_row() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn foo(b: f32, c: f32) -> f32 {
    return b + c;
}

fn bar(a: f32, c: f32) -> f32 {
    return a + c;
}

@fragment
fn fs_main() -> @location(0) f32 {
    let b = 1.0;
    let c = 2.0;
    let a = foo(b, c);
    let d = bar(a, c);
    return d;
}
"#,
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let reference_start = document
            .store
            .shader
            .draft_source
            .find("bar(a, c)")
            .unwrap()
            + "bar(".len();
        let reference_char_index =
            byte_index_to_char_index(&document.store.shader.draft_source, reference_start);

        document.focus_target_at_char_index(reference_char_index);

        let focused_row = document
            .store
            .dependencies
            .focused_row_key
            .as_deref()
            .and_then(|row_key| {
                document
                    .store
                    .dependencies
                    .rows
                    .iter()
                    .find(|row| row.row_key == row_key)
            })
            .expect("expected editor click to focus dependency row");
        assert_eq!(focused_row.label, "a (bar)");
        let focused_range = document
            .focused_source_range()
            .expect("expected focused occurrence range");
        assert_eq!(focused_range.start_byte, reference_start);
        assert_eq!(
            &document.store.shader.draft_source[focused_range.start_byte..focused_range.end_byte],
            "a"
        );
    }

    #[test]
    fn duplicate_dependency_rows_share_target_id_but_keep_row_occurrences() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn bar(left: f32, right: f32) -> f32 {
    return left + right;
}

@fragment
fn fs_main() -> @location(0) f32 {
    let a = 1.0;
    let d = bar(a, a);
    return d;
}
"#,
        );
        let document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let source = document
            .store
            .shader
            .analysis_source
            .as_ref()
            .expect("missing analysis source");
        let d_id = source_target_id_by_name(source, "d");
        let d_rows = flatten_dependency_tree(
            source
                .dependency_trees
                .get(&d_id)
                .expect("d dependency tree"),
            source,
        );
        let a_id = source_target_id_by_name(source, "a");
        let a_rows = d_rows
            .iter()
            .filter(|row| row.label == "a (bar)" && row.target_id.as_deref() == Some(&a_id))
            .collect::<Vec<_>>();
        assert_eq!(a_rows.len(), 2);
        assert_ne!(a_rows[0].row_key, a_rows[1].row_key);
        assert_eq!(a_rows[0].target_id, a_rows[1].target_id);

        let call_start = document
            .store
            .shader
            .draft_source
            .find("bar(a, a)")
            .unwrap();
        let first_range = a_rows[0].source_range.expect("first occurrence range");
        let second_range = a_rows[1].source_range.expect("second occurrence range");
        assert_eq!(first_range.start_byte, call_start + "bar(".len());
        assert_eq!(second_range.start_byte, call_start + "bar(a, ".len());
    }

    #[test]
    fn dependency_focus_path_returns_root_to_focus_chain() {
        let source = PassDebugSource {
            pass_name: "p".to_string(),
            module_source: String::new(),
            ast_tree: Vec::new(),
            dependency_targets: Vec::new(),
            dependency_trees: HashMap::new(),
            dependency_root_target_id: None,
            dependency_error: None,
            parse_error: None,
        };
        let rows = flatten_dependency_tree(
            &PassDebugDependencyNode {
                label: "root c (let)".to_string(),
                edge_label: None,
                display_label: None,
                source_range: None,
                definition_source_range: None,
                target_id: Some("target::c".to_string()),
                reference: false,
                children: vec![PassDebugDependencyNode {
                    label: "[value] named expression".to_string(),
                    edge_label: None,
                    display_label: None,
                    source_range: None,
                    definition_source_range: None,
                    target_id: None,
                    reference: false,
                    children: vec![PassDebugDependencyNode {
                        label: "mid b (let)".to_string(),
                        edge_label: None,
                        display_label: None,
                        source_range: None,
                        definition_source_range: None,
                        target_id: Some("target::b".to_string()),
                        reference: false,
                        children: vec![PassDebugDependencyNode {
                            label: "[value] named expression".to_string(),
                            edge_label: None,
                            display_label: None,
                            source_range: None,
                            definition_source_range: None,
                            target_id: None,
                            reference: false,
                            children: vec![PassDebugDependencyNode {
                                label: "leaf a (local)".to_string(),
                                edge_label: None,
                                display_label: None,
                                source_range: None,
                                definition_source_range: None,
                                target_id: Some("target::a".to_string()),
                                reference: false,
                                children: Vec::new(),
                            }],
                        }],
                    }],
                }],
            },
            &source,
        );
        let leaf_key = rows
            .iter()
            .find(|row| row.target_id.as_deref() == Some("target::a"))
            .map(|row| row.row_key.as_str())
            .unwrap();

        assert_eq!(
            dependency_path_for_row_key(&rows, leaf_key),
            vec![
                "0".to_string(),
                "0/0/0".to_string(),
                "0/0/0/0/0".to_string()
            ]
        );
    }

    #[test]
    fn duplicate_target_matches_focus_specific_row_key() {
        let source = PassDebugSource {
            pass_name: "p".to_string(),
            module_source: String::new(),
            ast_tree: Vec::new(),
            dependency_targets: Vec::new(),
            dependency_trees: HashMap::new(),
            dependency_root_target_id: None,
            dependency_error: None,
            parse_error: None,
        };
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.store.dependencies.rows = vec![
            PassDebugDependencyRow {
                depth: 0,
                row_key: "0".to_string(),
                parent_row_key: None,
                label: "root c (let)".to_string(),
                relation_path: String::new(),
                target_id: Some("target::c".to_string()),
                source_range: None,
                source_jump_range: None,
                selectable: true,
            },
            PassDebugDependencyRow {
                depth: 1,
                row_key: "0/0".to_string(),
                parent_row_key: Some("0".to_string()),
                label: "shared a (local)".to_string(),
                relation_path: "left".to_string(),
                target_id: Some("target::a".to_string()),
                source_range: None,
                source_jump_range: None,
                selectable: true,
            },
            PassDebugDependencyRow {
                depth: 1,
                row_key: "0/1".to_string(),
                parent_row_key: Some("0".to_string()),
                label: "shared a (local)".to_string(),
                relation_path: "right".to_string(),
                target_id: Some("target::a".to_string()),
                source_range: None,
                source_jump_range: None,
                selectable: true,
            },
        ];
        document.focus_dependency_row_key("0/1", true, false, false);

        assert_eq!(
            document.store.dependencies.focused_target_id.as_deref(),
            Some("target::a")
        );
        assert_eq!(
            document.store.dependencies.focused_row_key.as_deref(),
            Some("0/1")
        );
    }

    #[test]
    fn editor_focus_prefers_dependency_access_path_range() {
        let source = PassDebugSource {
            pass_name: "p".to_string(),
            module_source: "let a = input.foo.bar.x;".to_string(),
            ast_tree: Vec::new(),
            dependency_targets: vec![PassDebugDependencyTarget {
                id: "target::input".to_string(),
                name: "input".to_string(),
                label: "debug_main argument input".to_string(),
                scope: "debug_main".to_string(),
                kind: "argument".to_string(),
                source_range: Some(PassDebugSourceRange {
                    start_byte: 8,
                    end_byte: 13,
                    line: 1,
                    column: 9,
                }),
            }],
            dependency_trees: HashMap::new(),
            dependency_root_target_id: None,
            dependency_error: None,
            parse_error: None,
        };
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.store.dependencies.rows = vec![
            PassDebugDependencyRow {
                depth: 0,
                row_key: "0".to_string(),
                parent_row_key: None,
                label: "input".to_string(),
                relation_path: String::new(),
                target_id: Some("target::input".to_string()),
                source_range: Some(PassDebugSourceRange {
                    start_byte: 8,
                    end_byte: 13,
                    line: 1,
                    column: 9,
                }),
                source_jump_range: None,
                selectable: true,
            },
            PassDebugDependencyRow {
                depth: 1,
                row_key: "0/0".to_string(),
                parent_row_key: Some("0".to_string()),
                label: "input.foo.bar.x".to_string(),
                relation_path: "use_value".to_string(),
                target_id: Some("target::input".to_string()),
                source_range: Some(PassDebugSourceRange {
                    start_byte: 8,
                    end_byte: 23,
                    line: 1,
                    column: 9,
                }),
                source_jump_range: None,
                selectable: true,
            },
        ];

        document.focus_target_at_char_index(18);

        assert_eq!(
            document.store.dependencies.focused_row_key.as_deref(),
            Some("0/0")
        );
        let focused_range = document
            .focused_source_range()
            .expect("expected focused access path range");
        assert_eq!(
            &document.store.shader.draft_source[focused_range.start_byte..focused_range.end_byte],
            "input.foo.bar.x"
        );
    }

    #[test]
    fn draft_analysis_does_not_replace_canonical_dependency_source() {
        let source = PassDebugSource::from_wgsl("p", "fn a() -> f32 { return 1.0; }\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.store.shader.draft_source = "fn nope() -> { return vec4f(1.0); }\n".to_string();
        document.store.shader.dirty = true;
        document.refresh_draft_analysis();

        assert_eq!(
            document.store.shader.draft_source,
            "fn nope() -> { return vec4f(1.0); }\n"
        );
        assert!(
            document
                .store
                .shader
                .analysis_source
                .as_ref()
                .and_then(|source| source.parse_error.as_ref())
                .is_none()
        );
        assert!(
            document
                .store
                .shader
                .analysis_source_text
                .contains("return 1.0")
        );
        assert_eq!(
            document.store.shader.loaded_source,
            "fn a() -> f32 { return 1.0; }\n"
        );
    }

    // --- Shortwire tests ---

    fn test_diff_result(max_ae: f32) -> super::ShortwireDiffResult {
        super::ShortwireDiffResult {
            metric: "AE".to_string(),
            max_ae,
            min: 0.0,
            avg: 0.5,
            rms: 0.75,
            p95_abs: 1.0,
            sample_count: 16,
            non_finite_count: 0,
            render_size: [4, 4],
            reference_size: [4, 4],
            reference_offset: [0, 0],
        }
    }

    fn test_patch_with_diff(max_ae: Option<f32>) -> super::ShortwireNodePatch {
        super::ShortwireNodePatch {
            hunks: Vec::new(),
            base_source_hash: 1,
            reference_image: None,
            diff_result: max_ae.map(test_diff_result),
        }
    }

    fn test_reference_image() -> ShortwireReferenceImage {
        ShortwireReferenceImage {
            artifact_id: "pass__p__shortwire-reference-image__k".to_string(),
            name: "clip.png".to_string(),
            width: 2,
            height: 1,
            alpha_mode: RefImageAlphaMode::Premultiplied,
            mode: RefImageMode::Overlay,
            opacity: 0.5,
            offset: [1.0, -2.0],
        }
    }

    fn test_pasted_reference_image() -> ShortwirePastedReferenceImage {
        ShortwirePastedReferenceImage {
            name: "clip.png".to_string(),
            png_bytes: vec![137, 80, 78, 71, 13, 10, 26, 10],
            width: 2,
            height: 1,
            alpha_mode: RefImageAlphaMode::Premultiplied,
            mode: RefImageMode::Overlay,
            opacity: 0.5,
            offset: [0.0, 0.0],
        }
    }

    #[test]
    fn compact_diff_view_shows_only_changed_line_and_three_context_lines() {
        let base = (1..=10)
            .map(|line| format!("line{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let edited = (1..=10)
            .map(|line| {
                if line == 5 {
                    "line5 edited".to_string()
                } else {
                    format!("line{line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let view = super::build_shortwire_diff_view(&base, &edited);

        assert_eq!(view.rows.first().unwrap().old_line, Some(2));
        assert_eq!(view.rows.first().unwrap().new_line, Some(2));
        assert_eq!(view.rows.last().unwrap().old_line, Some(8));
        assert_eq!(view.rows.last().unwrap().new_line, Some(8));
        assert!(view.rows.iter().any(|row| {
            row.kind == super::ShortwireDiffRowKind::Removed
                && row.old_line == Some(5)
                && row.new_line.is_none()
                && row.text == "line5"
        }));
        assert!(view.rows.iter().any(|row| {
            row.kind == super::ShortwireDiffRowKind::Added
                && row.old_line.is_none()
                && row.new_line == Some(5)
                && row.text == "line5 edited"
        }));
        assert!(!view.rows.iter().any(|row| row.old_line == Some(1)));
        assert!(!view.rows.iter().any(|row| row.old_line == Some(9)));
    }

    #[test]
    fn compact_diff_view_keeps_distant_hunks_separated() {
        let base = (1..=18)
            .map(|line| format!("line{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let edited = (1..=18)
            .map(|line| match line {
                2 => "line2 edited".to_string(),
                17 => "line17 edited".to_string(),
                _ => format!("line{line}"),
            })
            .collect::<Vec<_>>()
            .join("\n");

        let view = super::build_shortwire_diff_view(&base, &edited);

        assert!(
            view.rows
                .iter()
                .any(|row| row.kind == super::ShortwireDiffRowKind::Separator)
        );
        assert!(view.rows.iter().any(|row| row.text == "line2 edited"));
        assert!(view.rows.iter().any(|row| row.text == "line17 edited"));
    }

    #[test]
    fn compact_diff_view_records_insert_delete_and_replace_line_numbers() {
        let replace = super::build_shortwire_diff_view("a\nold\nc\n", "a\nnew\nc\n");
        assert!(replace.rows.iter().any(|row| {
            row.kind == super::ShortwireDiffRowKind::Removed
                && row.old_line == Some(2)
                && row.new_line.is_none()
                && row.text == "old"
        }));
        assert!(replace.rows.iter().any(|row| {
            row.kind == super::ShortwireDiffRowKind::Added
                && row.old_line.is_none()
                && row.new_line == Some(2)
                && row.text == "new"
        }));

        let insert = super::build_shortwire_diff_view("a\nb\n", "a\nx\nb\n");
        assert!(insert.rows.iter().any(|row| {
            row.kind == super::ShortwireDiffRowKind::Added
                && row.old_line.is_none()
                && row.new_line == Some(2)
                && row.text == "x"
        }));

        let delete = super::build_shortwire_diff_view("a\nb\nc\n", "a\nc\n");
        assert!(delete.rows.iter().any(|row| {
            row.kind == super::ShortwireDiffRowKind::Removed
                && row.old_line == Some(2)
                && row.new_line.is_none()
                && row.text == "b"
        }));
    }

    #[test]
    fn legacy_shortwire_patch_json_restores_without_diff_result() {
        let text = r#"{"version":1,"patches":{"k":{"hunks":[],"base_source_hash":42}}}"#;
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.restore_shortwire_patches_from_text(text);

        let patch = document.store.shortwire.patches.get("k").unwrap();
        assert_eq!(patch.base_source_hash, 42);
        assert!(patch.diff_result.is_none());
    }

    #[test]
    fn shortwire_diff_result_round_trips_in_patch_artifact() {
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        document
            .store
            .shortwire
            .patches
            .insert("k".to_string(), test_patch_with_diff(Some(1.25)));
        document.store.shortwire.patches_dirty = true;

        let (_item, content) = document.take_patches_dirty_artifact().unwrap();
        assert!(content.contains("diffResult"));

        let mut restored = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        restored.restore_shortwire_patches_from_text(&content);

        assert_eq!(
            restored
                .store
                .shortwire
                .patches
                .get("k")
                .and_then(|patch| patch.diff_result.as_ref())
                .map(|result| result.max_ae),
            Some(1.25)
        );
    }

    #[test]
    fn shortwire_reference_image_round_trips_in_patch_artifact() {
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        let reference_image = test_reference_image();
        document.store.shortwire.patches.insert(
            "k".to_string(),
            super::ShortwireNodePatch {
                hunks: Vec::new(),
                base_source_hash: 1,
                reference_image: Some(reference_image.clone()),
                diff_result: None,
            },
        );
        document.store.shortwire.patches_dirty = true;

        let (_item, content) = document.take_patches_dirty_artifact().unwrap();
        assert!(content.contains("referenceImage"));
        assert!(content.contains(reference_image.artifact_id.as_str()));

        let mut restored = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        restored.restore_shortwire_patches_from_text(&content);

        assert_eq!(
            restored
                .store
                .shortwire
                .patches
                .get("k")
                .and_then(|patch| patch.reference_image.as_ref())
                .map(|image| image.artifact_id.as_str()),
            Some(reference_image.artifact_id.as_str())
        );
    }

    #[test]
    fn shortwire_dot_status_uses_diff_result_threshold() {
        assert_eq!(
            super::shortwire_dot_info_for_patch(&test_patch_with_diff(None)).status,
            super::ShortwireDotStatus::PendingDiff
        );
        assert_eq!(
            super::shortwire_dot_info_for_patch(&test_patch_with_diff(Some(
                super::SHORTWIRE_DIFF_PASS_MAX_AE * 0.99
            )))
            .status,
            super::ShortwireDotStatus::Passing
        );
        assert_eq!(
            super::shortwire_dot_info_for_patch(&test_patch_with_diff(Some(
                super::SHORTWIRE_DIFF_PASS_MAX_AE
            )))
            .status,
            super::ShortwireDotStatus::Failing
        );
        assert_eq!(
            super::shortwire_dot_info_for_patch(&test_patch_with_diff(Some(0.992458))).status,
            super::ShortwireDotStatus::Failing
        );
    }

    #[test]
    fn closing_shortwire_preserves_matching_diff_result() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };

        document.enter_shortwire(&row, &pending_actions);
        document.store.shader.draft_source = "fn edited() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);
        let patched_source = PassDebugSource::from_wgsl("p", "fn edited() {}\n");
        document.mark_applied(
            Some(&patched_source),
            1,
            "fn edited() {}\n".to_string(),
            "Applied".to_string(),
        );

        let patch_key = super::shortwire_patch_key(&row);
        document
            .store
            .shortwire
            .patches
            .get_mut(&patch_key)
            .unwrap()
            .diff_result = Some(test_diff_result(0.25));

        document.exit_shortwire_navigate(&pending_actions);

        assert_eq!(
            document
                .store
                .shortwire
                .patches
                .get(&patch_key)
                .and_then(|patch| patch.diff_result.as_ref())
                .map(|result| result.max_ae),
            Some(0.25)
        );
    }

    #[test]
    fn active_shortwire_diff_capture_clears_stale_result() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut windows = super::PassDebugWindowMap::new();
        windows.insert(
            "p".to_string(),
            super::PassDebugWindowState::new("p".to_string(), Some(source), 0, None),
        );

        let state = windows.get("p").unwrap();
        let pending_actions = state.pending_actions.clone();
        let patch_key = {
            let mut document = state.document.lock().unwrap();
            let row = document.store.dependencies.rows.first().cloned().unwrap();
            let patch_key = super::shortwire_patch_key(&row);
            document.enter_shortwire(&row, &pending_actions);
            document
                .store
                .shortwire
                .patches
                .insert(patch_key.clone(), test_patch_with_diff(Some(1.25)));
            patch_key
        };

        let result = super::request_active_shortwire_diff_capture(&mut windows, None);

        assert_eq!(
            result.diff_capture,
            Some(super::ShortwireDiffCaptureRequest {
                pass_name: "p".to_string(),
                patch_key: patch_key.clone(),
            })
        );
        assert_eq!(result.artifacts.len(), 1);
        let state = windows.get("p").unwrap();
        let document = state.document.lock().unwrap();
        assert!(
            document
                .store
                .shortwire
                .patches
                .get(&patch_key)
                .unwrap()
                .diff_result
                .is_none()
        );
        assert!(!document.store.shortwire.patches_dirty);
    }

    #[test]
    fn pasted_shortwire_reference_creates_image_only_patch() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut windows = super::PassDebugWindowMap::new();
        windows.insert(
            "p".to_string(),
            super::PassDebugWindowState::new("p".to_string(), Some(source), 0, None),
        );

        let state = windows.get("p").unwrap();
        let pending_actions = state.pending_actions.clone();
        let patch_key = {
            let mut document = state.document.lock().unwrap();
            let row = document.store.dependencies.rows.first().cloned().unwrap();
            let patch_key = super::shortwire_patch_key(&row);
            document.enter_shortwire(&row, &pending_actions);
            patch_key
        };

        let result = super::request_active_shortwire_diff_capture(
            &mut windows,
            Some(test_pasted_reference_image()),
        );

        assert_eq!(
            result.diff_capture,
            Some(super::ShortwireDiffCaptureRequest {
                pass_name: "p".to_string(),
                patch_key: patch_key.clone(),
            })
        );
        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(result.binary_artifacts.len(), 1);
        let (image_item, image_bytes) = &result.binary_artifacts[0];
        assert_eq!(image_item.role, DebugArtifactRole::Image);
        assert_eq!(image_item.mime_type, "image/png");
        assert!(image_item.path.starts_with("debug-artifacts/"));
        assert!(
            image_item
                .slot_key
                .as_deref()
                .is_some_and(|slot| slot.starts_with("shortwire-reference:"))
        );
        assert_eq!(image_bytes.as_slice(), &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert!(result.artifacts[0].1.contains(image_item.id.as_str()));

        let state = windows.get("p").unwrap();
        let document = state.document.lock().unwrap();
        let patch = document.store.shortwire.patches.get(&patch_key).unwrap();
        assert!(patch.hunks.is_empty());
        assert_eq!(
            patch
                .reference_image
                .as_ref()
                .map(|image| image.artifact_id.as_str()),
            Some(image_item.id.as_str())
        );
        assert!(patch.diff_result.is_none());
        assert!(!document.store.shortwire.patches_dirty);
    }

    #[test]
    fn window_state_restores_shortwire_patches_when_artifact_arrives_late() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut state = super::PassDebugWindowState::new("p".to_string(), Some(source), 0, None);
        let payload = super::ShortwirePatchesPayload {
            version: 1,
            patches: HashMap::from([(
                "k".to_string(),
                test_patch_with_diff(Some(super::SHORTWIRE_DIFF_PASS_MAX_AE * 0.5)),
            )]),
        };
        let content = serde_json::to_string(&payload).unwrap();

        state.sync_shortwire_patches_from_artifact(None);
        assert!(
            state
                .document
                .lock()
                .unwrap()
                .store
                .shortwire
                .patches
                .is_empty()
        );

        state.sync_shortwire_patches_from_artifact(Some(&content));

        let document = state.document.lock().unwrap();
        assert_eq!(document.store.shortwire.patches.len(), 1);
        assert_eq!(
            super::shortwire_dot_info_for_patch(document.store.shortwire.patches.get("k").unwrap())
                .status,
            super::ShortwireDotStatus::Passing
        );
    }

    #[test]
    fn legacy_default_reference_artifact_migrates_to_workspace() {
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.update_reference_workspace(None, &[], Some("legacy reference\n"), None);

        assert_eq!(
            document.store.reference_workspace.selected_file.as_deref(),
            Some("reference.txt")
        );
        assert_eq!(
            document.store.reference_workspace.editor_source,
            "legacy reference\n"
        );
        assert!(document.store.reference_workspace.manifest_dirty);
        assert!(document.store.reference_workspace.selected_file_dirty());

        let artifacts = document.take_reference_workspace_dirty_artifacts();
        assert_eq!(artifacts.len(), 2);
        assert!(artifacts.iter().any(|(item, _)| {
            item.role == DebugArtifactRole::Attachment
                && item.slot_key.as_deref() == Some(super::DEBUG_ARTIFACT_REFERENCE_WORKSPACE_SLOT)
        }));
        assert!(artifacts.iter().any(|(item, content)| {
            item.role == DebugArtifactRole::ReferenceCode
                && item
                    .slot_key
                    .as_deref()
                    .is_some_and(|slot| slot.starts_with("file:"))
                && content == "legacy reference\n"
        }));
    }

    #[test]
    fn reference_workspace_restores_multiple_files_from_artifacts() {
        let manifest = serde_json::json!({
            "version": 1,
            "rootPath": null,
            "rootLabel": "reference",
            "selectedFile": "metal/pass.metal",
            "files": [
                {
                    "relativePath": "glsl/pass.frag",
                    "artifactId": super::pass_reference_file_artifact_id("p", "glsl/pass.frag"),
                    "contentHash": "aaa",
                    "size": 12
                },
                {
                    "relativePath": "metal/pass.metal",
                    "artifactId": super::pass_reference_file_artifact_id("p", "metal/pass.metal"),
                    "contentHash": "bbb",
                    "size": 13
                }
            ],
            "skippedFiles": 2
        })
        .to_string();
        let files = vec![
            reference_snapshot("p", "glsl/pass.frag", "glsl source\n"),
            reference_snapshot("p", "metal/pass.metal", "metal source\n"),
        ];
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.update_reference_workspace(Some(&manifest), &files, None, None);

        assert_eq!(document.store.reference_workspace.files.len(), 2);
        assert_eq!(
            document.store.reference_workspace.selected_file.as_deref(),
            Some("metal/pass.metal")
        );
        assert_eq!(
            document.store.reference_workspace.editor_source,
            "metal source\n"
        );
        assert_eq!(document.store.reference_workspace.skipped_files, 2);
        assert!(!document.store.reference_workspace.has_dirty_files());
    }

    #[test]
    fn rooted_reference_workspace_restore_decodes_to_local_read_effect() {
        let relative_path = "metal/pass.metal";
        let artifact_id = super::pass_reference_file_artifact_id("p", relative_path);
        let manifest = serde_json::json!({
            "version": 1,
            "rootPath": "/tmp/node-forge-reference-root",
            "rootLabel": "reference",
            "selectedFile": relative_path,
            "files": [
                {
                    "relativePath": relative_path,
                    "artifactId": artifact_id,
                    "contentHash": "archive",
                    "size": 15
                }
            ],
            "skippedFiles": 0
        })
        .to_string();
        let files = [reference_snapshot("p", relative_path, "archive source\n")];
        let file_texts = files
            .iter()
            .map(|snapshot| super::ReferenceArtifactText {
                artifact_id: snapshot.item.id.as_str(),
                name: snapshot.item.name.as_str(),
                text: snapshot.text.as_str(),
            })
            .collect::<Vec<_>>();

        let plan = super::plan_reference_workspace_artifact_restore(
            "p",
            Some(&manifest),
            &file_texts,
            None,
        );

        match plan {
            super::ReferenceArtifactRestorePlan::ReadManifestLocalFiles(request) => {
                assert_eq!(request.root_path, "/tmp/node-forge-reference-root");
                assert_eq!(request.manifest.files.len(), 1);
                assert_eq!(request.manifest.files[0].relative_path, relative_path);
            }
            _ => panic!("expected rooted manifest to emit local read effect"),
        }
    }

    #[test]
    fn reference_workspace_with_root_path_loads_local_file_instead_of_artifact_text() {
        let temp_dir = unique_reference_temp_dir("root-loads-local");
        fs::create_dir_all(temp_dir.join("metal")).unwrap();
        fs::write(temp_dir.join("metal/pass.metal"), "local source\n").unwrap();
        let relative_path = "metal/pass.metal";
        let artifact_id = super::pass_reference_file_artifact_id("p", relative_path);
        let manifest = serde_json::json!({
            "version": 1,
            "rootPath": temp_dir.to_string_lossy(),
            "rootLabel": "reference",
            "selectedFile": relative_path,
            "files": [
                {
                    "relativePath": relative_path,
                    "artifactId": artifact_id,
                    "contentHash": "archive",
                    "size": 15
                }
            ],
            "skippedFiles": 0
        })
        .to_string();
        let files = vec![reference_snapshot("p", relative_path, "archive source\n")];
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.update_reference_workspace(Some(&manifest), &files, None, None);
        drain_reference_manifest_read_effects(&mut document);

        assert_eq!(
            document.store.reference_workspace.editor_source,
            "local source\n"
        );
        assert_eq!(
            document.store.reference_workspace.last_status.as_deref(),
            Some("Loaded local reference")
        );
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn rooted_reference_workspace_restore_does_not_clobber_dirty_draft() {
        let relative_path = "metal/pass.metal";
        let artifact_id = super::pass_reference_file_artifact_id("p", relative_path);
        let archive_manifest = serde_json::json!({
            "version": 1,
            "rootPath": null,
            "rootLabel": "reference",
            "selectedFile": relative_path,
            "files": [
                {
                    "relativePath": relative_path,
                    "artifactId": artifact_id,
                    "contentHash": "archive",
                    "size": 15
                }
            ],
            "skippedFiles": 0
        })
        .to_string();
        let files = vec![reference_snapshot("p", relative_path, "archive source\n")];
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        document.update_reference_workspace(Some(&archive_manifest), &files, None, None);
        document.store.reference_workspace.editor_source = "dirty draft\n".to_string();
        document.mark_reference_edited(1.0);

        let temp_dir = unique_reference_temp_dir("root-dirty-no-clobber");
        fs::create_dir_all(temp_dir.join("metal")).unwrap();
        fs::write(temp_dir.join("metal/pass.metal"), "local source\n").unwrap();
        let rooted_manifest = serde_json::json!({
            "version": 1,
            "rootPath": temp_dir.to_string_lossy(),
            "rootLabel": "reference",
            "selectedFile": relative_path,
            "files": [
                {
                    "relativePath": relative_path,
                    "artifactId": artifact_id,
                    "contentHash": "archive",
                    "size": 15
                }
            ],
            "skippedFiles": 0
        })
        .to_string();

        document.update_reference_workspace(Some(&rooted_manifest), &files, None, None);
        drain_reference_manifest_read_effects(&mut document);

        assert_eq!(
            document.store.reference_workspace.editor_source,
            "dirty draft\n"
        );
        assert!(document.store.reference_workspace.selected_file_dirty());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn reference_workspace_with_missing_root_path_file_does_not_fallback_to_artifact_text() {
        let temp_dir = unique_reference_temp_dir("root-missing-no-fallback");
        fs::create_dir_all(&temp_dir).unwrap();
        let relative_path = "metal/pass.metal";
        let artifact_id = super::pass_reference_file_artifact_id("p", relative_path);
        let manifest = serde_json::json!({
            "version": 1,
            "rootPath": temp_dir.to_string_lossy(),
            "rootLabel": "reference",
            "selectedFile": relative_path,
            "files": [
                {
                    "relativePath": relative_path,
                    "artifactId": artifact_id,
                    "contentHash": "archive",
                    "size": 15
                }
            ],
            "skippedFiles": 0
        })
        .to_string();
        let files = vec![reference_snapshot("p", relative_path, "archive source\n")];
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.update_reference_workspace(Some(&manifest), &files, None, None);
        drain_reference_manifest_read_effects(&mut document);

        assert!(document.store.reference_workspace.files.is_empty());
        assert!(document.store.reference_workspace.editor_source.is_empty());
        assert_eq!(
            document.store.reference_workspace.last_status.as_deref(),
            Some("Local reference missing (1 missing)")
        );
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn pathless_reference_workspace_still_restores_from_artifact_text() {
        let relative_path = "metal/pass.metal";
        let artifact_id = super::pass_reference_file_artifact_id("p", relative_path);
        let manifest = serde_json::json!({
            "version": 1,
            "rootPath": null,
            "rootLabel": "reference",
            "selectedFile": relative_path,
            "files": [
                {
                    "relativePath": relative_path,
                    "artifactId": artifact_id,
                    "contentHash": "archive",
                    "size": 15
                }
            ],
            "skippedFiles": 0
        })
        .to_string();
        let files = vec![reference_snapshot("p", relative_path, "archive source\n")];
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.update_reference_workspace(Some(&manifest), &files, None, None);

        assert_eq!(
            document.store.reference_workspace.editor_source,
            "archive source\n"
        );
        assert_eq!(
            document.store.reference_workspace.last_status.as_deref(),
            Some("Loaded archived reference")
        );
    }

    #[test]
    fn reference_patch_store_survives_workspace_restore() {
        let relative_path = "metal/pass.metal";
        let artifact_id = super::pass_reference_file_artifact_id("p", relative_path);
        let manifest = serde_json::json!({
            "version": 1,
            "rootPath": null,
            "rootLabel": "reference",
            "selectedFile": relative_path,
            "files": [
                {
                    "relativePath": relative_path,
                    "artifactId": artifact_id,
                    "contentHash": "archive",
                    "size": 15
                }
            ],
            "skippedFiles": 0
        })
        .to_string();
        let files = vec![reference_snapshot("p", relative_path, "fn ref() {}\n")];
        let patch_key = super::reference_shortwire_patch_key(relative_path, "row");
        let payload = super::ReferencePatchesPayload {
            version: super::REFERENCE_WORKSPACE_VERSION,
            patches: HashMap::from([(
                patch_key.clone(),
                super::ShortwireNodePatch {
                    hunks: super::compute_hunks("fn ref() {}\n", "fn ref_patched() {}\n"),
                    base_source_hash: super::hash_source("fn ref() {}\n"),
                    reference_image: None,
                    diff_result: None,
                },
            )]),
        };
        let patch_text = serde_json::to_string(&payload).unwrap();
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.update_reference_workspace(Some(&manifest), &files, None, Some(&patch_text));
        assert!(
            document
                .store
                .reference_workspace
                .reference_patches
                .contains_key(&patch_key)
        );

        let renamed_manifest = serde_json::json!({
            "version": 1,
            "rootPath": null,
            "rootLabel": "renamed reference",
            "selectedFile": relative_path,
            "files": [
                {
                    "relativePath": relative_path,
                    "artifactId": artifact_id,
                    "contentHash": "archive",
                    "size": 15
                }
            ],
            "skippedFiles": 0
        })
        .to_string();
        document.update_reference_workspace(Some(&renamed_manifest), &files, None, None);

        assert_eq!(
            document.store.reference_workspace.root_label,
            "renamed reference"
        );
        assert!(
            document
                .store
                .reference_workspace
                .reference_patches
                .contains_key(&patch_key)
        );
    }

    #[test]
    fn rooted_reference_sync_writes_local_file_and_only_emits_manifest() {
        let temp_dir = unique_reference_temp_dir("root-sync-writes-local");
        fs::create_dir_all(&temp_dir).unwrap();
        let reference_path = temp_dir.join("pass.metal");
        fs::write(&reference_path, "fn original() {}\n").unwrap();
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        document.import_reference_file_from_path(&reference_path, 0.0);
        document.store.reference_workspace.editor_source = "fn edited() {}\n".to_string();

        let artifacts = document.take_reference_workspace_dirty_artifacts();

        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn edited() {}\n"
        );
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].0.role, DebugArtifactRole::Attachment);
        assert_eq!(
            artifacts[0].0.slot_key.as_deref(),
            Some(super::DEBUG_ARTIFACT_REFERENCE_WORKSPACE_SLOT)
        );
        assert!(
            !artifacts
                .iter()
                .any(|(item, _)| item.role == DebugArtifactRole::ReferenceCode)
        );
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn rooted_reference_sync_write_failure_keeps_dirty_draft() {
        let temp_dir = unique_reference_temp_dir("root-sync-write-fails");
        fs::create_dir_all(&temp_dir).unwrap();
        let reference_path = temp_dir.join("pass.metal");
        fs::write(&reference_path, "fn original() {}\n").unwrap();
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        document.import_reference_file_from_path(&reference_path, 0.0);

        assert_eq!(document.take_reference_workspace_dirty_artifacts().len(), 1);
        assert!(!document.store.reference_workspace.selected_file_dirty());
        assert!(!document.store.reference_workspace.manifest_dirty);

        document.store.reference_workspace.editor_source = "fn edited() {}\n".to_string();
        document.mark_reference_edited(1.0);
        fs::remove_dir_all(&temp_dir).unwrap();

        let artifacts = document.take_reference_workspace_dirty_artifacts();

        assert!(artifacts.is_empty());
        assert!(document.store.reference_workspace.selected_file_dirty());
        assert!(!document.store.reference_workspace.manifest_dirty);
        assert!(
            document
                .store
                .reference_workspace
                .last_status
                .as_deref()
                .unwrap_or_default()
                .contains("Failed to write")
        );
    }

    #[test]
    fn reference_sync_completion_does_not_ack_stale_file_source() {
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        seed_reference_file(&mut document, "metal/pass.metal", "fn original() {}\n");
        document.store.reference_workspace.editor_source = "fn first_edit() {}\n".to_string();
        document.mark_reference_edited(1.0);
        let plan = document.store.reference_workspace.build_sync_plan();
        let completion = document.run_reference_sync_plan(plan);

        document.store.reference_workspace.editor_source = "fn second_edit() {}\n".to_string();
        document.mark_reference_edited(2.0);
        let artifacts = document.apply_reference_sync_completion(completion);

        assert_eq!(artifacts.len(), 1);
        assert_eq!(
            document
                .store
                .reference_workspace
                .selected_file()
                .unwrap()
                .source,
            "fn second_edit() {}\n"
        );
        assert_eq!(
            document
                .store
                .reference_workspace
                .selected_file()
                .unwrap()
                .loaded_source,
            "fn original() {}\n"
        );
        assert!(document.store.reference_workspace.selected_file_dirty());
    }

    #[test]
    fn reference_shortwire_patch_key_isolated_by_file() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document.store.dependencies.rows.first().cloned().unwrap();
        let base = "fn ref() {}\n";
        let file_a = super::ReferenceWorkspaceFile {
            relative_path: "a.metal".to_string(),
            artifact_id: super::pass_reference_file_artifact_id("p", "a.metal"),
            source: base.to_string(),
            loaded_source: base.to_string(),
        };
        let file_b = super::ReferenceWorkspaceFile {
            relative_path: "b.metal".to_string(),
            artifact_id: super::pass_reference_file_artifact_id("p", "b.metal"),
            source: base.to_string(),
            loaded_source: base.to_string(),
        };
        document.store.reference_workspace.replace_files(
            None,
            "Reference test".to_string(),
            vec![file_a, file_b],
            Some("a.metal".to_string()),
            0,
            false,
        );
        let row_patch_key = super::shortwire_patch_key(&row);
        document.store.reference_workspace.reference_patches.insert(
            super::reference_shortwire_patch_key("a.metal", &row_patch_key),
            super::ShortwireNodePatch {
                hunks: super::compute_hunks(base, "fn ref_a() {}\n"),
                base_source_hash: super::hash_source(base),
                reference_image: None,
                diff_result: None,
            },
        );
        document.store.reference_workspace.reference_patches.insert(
            super::reference_shortwire_patch_key("b.metal", &row_patch_key),
            super::ShortwireNodePatch {
                hunks: super::compute_hunks(base, "fn ref_b() {}\n"),
                base_source_hash: super::hash_source(base),
                reference_image: None,
                diff_result: None,
            },
        );

        document.enter_shortwire(&row, &pending_actions);
        assert_eq!(
            document.store.reference_workspace.editor_source,
            "fn ref_a() {}\n"
        );
        document.exit_shortwire_cancel();
        assert!(document.store.reference_workspace.select_file("b.metal"));
        document.enter_shortwire(&row, &pending_actions);
        assert_eq!(
            document.store.reference_workspace.editor_source,
            "fn ref_b() {}\n"
        );
    }

    #[test]
    fn reference_shortwire_save_commits_after_left_apply_and_stays_active() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document.store.dependencies.rows.first().cloned().unwrap();
        seed_reference_file(&mut document, "metal/pass.metal", "fn ref() {}\n");

        document.enter_shortwire(&row, &pending_actions);
        document.store.reference_workspace.editor_source = "fn ref_patched() {}\n".to_string();
        document.store.shader.draft_source = "fn patched_left() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);

        assert!(
            document
                .store
                .reference_workspace
                .pending_shortwire_patch
                .is_some()
        );
        document.mark_applied(
            Some(&source),
            1,
            "fn patched_left() {}\n".to_string(),
            "Applied".to_string(),
        );

        assert_eq!(
            document.store.reference_workspace.editor_source,
            "fn ref_patched() {}\n"
        );
        assert!(
            document
                .store
                .reference_workspace
                .shortwire_active_key
                .is_some()
        );
        assert_eq!(
            document.store.reference_workspace.reference_patches.len(),
            1
        );
        assert!(document.store.reference_workspace.reference_patches_dirty);
        let patch = document
            .store
            .reference_workspace
            .reference_patches
            .values()
            .next()
            .unwrap();
        assert_eq!(
            super::apply_hunks("fn ref() {}\n", &patch.hunks).unwrap(),
            "fn ref_patched() {}\n"
        );

        document.exit_shortwire_navigate(&pending_actions);

        assert_eq!(
            document.store.reference_workspace.editor_source,
            "fn ref() {}\n"
        );
        assert!(
            document
                .store
                .reference_workspace
                .shortwire_active_key
                .is_none()
        );
    }

    #[test]
    fn reference_shortwire_save_writes_local_file_and_exit_restores_original() {
        let temp_dir = unique_reference_temp_dir("save-restore");
        fs::create_dir_all(&temp_dir).unwrap();
        let reference_path = temp_dir.join("pass.metal");
        fs::write(&reference_path, "fn original() {}\n").unwrap();

        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        document.import_reference_file_from_path(&reference_path, 0.0);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document.store.dependencies.rows.first().cloned().unwrap();

        document.enter_shortwire(&row, &pending_actions);
        drain_reference_shortwire_file_effects(&mut document);
        document.store.reference_workspace.editor_source = "fn patched_ref() {}\n".to_string();
        document.store.shader.draft_source = "fn patched_left() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);

        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn original() {}\n"
        );

        document.mark_applied(
            Some(&source),
            1,
            "fn patched_left() {}\n".to_string(),
            "Applied".to_string(),
        );
        drain_reference_shortwire_file_effects(&mut document);

        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn patched_ref() {}\n"
        );

        document.exit_shortwire_navigate(&pending_actions);
        drain_reference_shortwire_file_effects(&mut document);

        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn original() {}\n"
        );
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn reference_shortwire_reenter_writes_stored_patch_to_local_file() {
        let temp_dir = unique_reference_temp_dir("reenter-write");
        fs::create_dir_all(&temp_dir).unwrap();
        let reference_path = temp_dir.join("pass.metal");
        fs::write(&reference_path, "fn original() {}\n").unwrap();

        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        document.import_reference_file_from_path(&reference_path, 0.0);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document.store.dependencies.rows.first().cloned().unwrap();

        document.enter_shortwire(&row, &pending_actions);
        drain_reference_shortwire_file_effects(&mut document);
        document.store.reference_workspace.editor_source = "fn patched_ref() {}\n".to_string();
        document.store.shader.draft_source = "fn patched_left() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);
        document.mark_applied(
            Some(&source),
            1,
            "fn patched_left() {}\n".to_string(),
            "Applied".to_string(),
        );
        drain_reference_shortwire_file_effects(&mut document);
        document.exit_shortwire_navigate(&pending_actions);
        drain_reference_shortwire_file_effects(&mut document);
        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn original() {}\n"
        );

        document.mark_reset(Some(&source), 2, "Reset".to_string());
        pending_actions.lock().unwrap().clear();
        document.enter_shortwire(&row, &pending_actions);
        drain_reference_shortwire_file_effects(&mut document);

        assert_eq!(
            document.store.reference_workspace.editor_source,
            "fn patched_ref() {}\n"
        );
        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn patched_ref() {}\n"
        );

        document.exit_shortwire_navigate(&pending_actions);
        drain_reference_shortwire_file_effects(&mut document);
        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn original() {}\n"
        );
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn reference_shortwire_close_restores_local_file() {
        let temp_dir = unique_reference_temp_dir("close-restore");
        fs::create_dir_all(&temp_dir).unwrap();
        let reference_path = temp_dir.join("pass.metal");
        fs::write(&reference_path, "fn original() {}\n").unwrap();

        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        document.import_reference_file_from_path(&reference_path, 0.0);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document.store.dependencies.rows.first().cloned().unwrap();

        document.enter_shortwire(&row, &pending_actions);
        drain_reference_shortwire_file_effects(&mut document);
        document.store.reference_workspace.editor_source = "fn patched_ref() {}\n".to_string();
        document.store.shader.draft_source = "fn patched_left() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);
        document.mark_applied(
            Some(&source),
            1,
            "fn patched_left() {}\n".to_string(),
            "Applied".to_string(),
        );
        drain_reference_shortwire_file_effects(&mut document);
        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn patched_ref() {}\n"
        );

        pending_actions.lock().unwrap().clear();
        document.prepare_debug_window_close(&pending_actions);
        drain_reference_shortwire_file_effects(&mut document);

        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn original() {}\n"
        );
        assert!(document.store.shortwire.active.is_none());
        let actions = pending_actions.lock().unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            super::PassDebugWindowAction::ResetPatch { .. }
        ));
        drop(actions);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn mark_patch_applied_returns_shortwire_artifacts_immediately() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut windows = super::PassDebugWindowMap::new();
        windows.insert(
            "p".to_string(),
            super::PassDebugWindowState::new("p".to_string(), Some(source.clone()), 0, None),
        );

        let state = windows.get("p").unwrap();
        let pending_actions = state.pending_actions.clone();
        {
            let mut document = state.document.lock().unwrap();
            let row = document.store.dependencies.rows.first().cloned().unwrap();
            seed_reference_file(&mut document, "metal/pass.metal", "fn ref() {}\n");
            document.enter_shortwire(&row, &pending_actions);
            document.store.reference_workspace.editor_source = "fn ref_patched() {}\n".to_string();
            document.store.shader.draft_source = "fn patched_left() {}\n".to_string();
            document.exit_shortwire_done(&pending_actions);
        }

        let result = super::mark_patch_applied(
            &mut windows,
            "p",
            Some(&source),
            1,
            "fn patched_left() {}\n".to_string(),
            "Applied".to_string(),
        );
        let artifacts = result.artifacts;

        assert_eq!(artifacts.len(), 2);
        assert!(result.diff_capture.is_some());
        assert!(artifacts.iter().any(|(item, content)| {
            item.role == DebugArtifactRole::Patch
                && item.slot_key.as_deref() == Some(super::DEBUG_ARTIFACT_DEFAULT_SLOT)
                && content.contains("patched_left")
        }));
        assert!(artifacts.iter().any(|(item, content)| {
            item.role == DebugArtifactRole::Patch
                && item.slot_key.as_deref() == Some(super::DEBUG_ARTIFACT_REFERENCE_PATCHES_SLOT)
                && content.contains("ref_patched")
        }));

        let state = windows.get("p").unwrap();
        let document = state.document.lock().unwrap();
        assert!(!document.store.shortwire.patches_dirty);
        assert!(!document.store.reference_workspace.reference_patches_dirty);
    }

    #[test]
    fn reference_shortwire_apply_error_keeps_patch_draft_uncommitted() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document.store.dependencies.rows.first().cloned().unwrap();
        seed_reference_file(&mut document, "metal/pass.metal", "fn ref() {}\n");

        document.enter_shortwire(&row, &pending_actions);
        document.store.reference_workspace.editor_source = "fn ref_patched() {}\n".to_string();
        document.store.shader.draft_source = "fn patched_left() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);
        document.record_error("left apply failed".to_string());

        assert_eq!(
            document.store.reference_workspace.editor_source,
            "fn ref_patched() {}\n"
        );
        assert!(
            document
                .store
                .reference_workspace
                .pending_shortwire_patch
                .is_some()
        );
        assert!(
            document
                .store
                .reference_workspace
                .reference_patches
                .is_empty()
        );
        assert!(!document.store.reference_workspace.reference_patches_dirty);
        assert!(document.shortwire_is_editor_interactive());
    }

    #[test]
    fn reference_reload_missing_path_keeps_archive_snapshot() {
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        seed_reference_file(&mut document, "metal/pass.metal", "fn ref() {}\n");
        document.store.reference_workspace.root_path =
            Some("/tmp/node-forge-missing-reference-root-for-test".to_string());

        document.reload_reference_workspace(0.0);

        let effects = document.drain_effects();
        assert_eq!(effects.len(), 1);
        match &effects[0] {
            PassDebugEffect::ReloadReferenceWorkspace {
                root,
                selected_file,
                single_file,
                ..
            } => {
                assert_eq!(
                    root,
                    &PathBuf::from("/tmp/node-forge-missing-reference-root-for-test")
                );
                assert_eq!(selected_file.as_deref(), Some("metal/pass.metal"));
                assert!(*single_file);
            }
            other => panic!("expected reload effect, got {other:?}"),
        }

        document.mark_reference_reload_missing_path();

        assert_eq!(
            document.store.reference_workspace.editor_source,
            "fn ref() {}\n"
        );
        assert!(
            document
                .store
                .reference_workspace
                .last_status
                .as_deref()
                .unwrap_or_default()
                .contains("missing")
        );
    }

    #[test]
    fn patch_source_updates_editor_but_dependency_tree_stays_canonical() {
        let canonical = root_return_shader("canonical_root", 1.0);
        let patched = root_return_shader("shortwire_root", 2.0);
        let source = PassDebugSource::from_wgsl("p", canonical.clone());
        let document =
            PassDebugWindowDocument::new("p".to_string(), Some(source), 0, Some(patched.as_str()));

        assert_eq!(document.store.shader.draft_source, patched);
        assert_eq!(document.store.shader.loaded_source, patched);
        assert_eq!(document.store.shader.generated_base_source, canonical);
        assert_eq!(dependency_root_target_name(&document), "return");
        assert!(has_target_named(&document, "canonical_root"));
        assert!(!has_target_named(&document, "shortwire_root"));
        assert!(dependency_rows_contain_label_fragment(
            &document,
            "canonical_root"
        ));
        assert!(!dependency_rows_contain_label_fragment(
            &document,
            "shortwire_root"
        ));
    }

    #[test]
    fn applying_shortwire_does_not_change_dependency_root() {
        let canonical = root_return_shader("canonical_root", 1.0);
        let patched = root_return_shader("shortwire_root", 2.0);
        let source = PassDebugSource::from_wgsl("p", canonical.clone());
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let root_before = document.store.dependencies.root_target_id.clone();
        let row = document
            .store
            .dependencies
            .rows
            .first()
            .cloned()
            .expect("dependency root row");

        document.enter_shortwire(&row, &pending_actions);
        document.store.shader.draft_source = patched.clone();
        document.exit_shortwire_done(&pending_actions);
        document.mark_applied(Some(&source), 1, patched.clone(), "Applied".to_string());

        assert_eq!(document.store.shader.draft_source, patched);
        assert_eq!(document.store.shader.generated_base_source, canonical);
        assert_eq!(document.store.dependencies.root_target_id, root_before);
        assert_eq!(dependency_root_target_name(&document), "return");
        assert!(has_target_named(&document, "canonical_root"));
        assert!(!has_target_named(&document, "shortwire_root"));
        assert!(dependency_rows_contain_label_fragment(
            &document,
            "canonical_root"
        ));
        assert!(!dependency_rows_contain_label_fragment(
            &document,
            "shortwire_root"
        ));
    }

    #[test]
    fn canonical_source_refresh_updates_deps_tree_while_patch_exists() {
        let canonical_before = root_return_shader("before_root", 1.0);
        let canonical_after = root_return_shader("after_root", 3.0);
        let patched = root_return_shader("shortwire_root", 2.0);
        let source_before = PassDebugSource::from_wgsl("p", canonical_before);
        let source_after = PassDebugSource::from_wgsl("p", canonical_after.clone());
        let mut document = PassDebugWindowDocument::new(
            "p".to_string(),
            Some(source_before),
            0,
            Some(patched.as_str()),
        );
        assert_eq!(dependency_root_target_name(&document), "return");
        assert!(dependency_rows_contain_label_fragment(
            &document,
            "before_root"
        ));

        document.update_source(Some(&source_after), 1, Some(patched.as_str()));

        assert_eq!(document.store.shader.draft_source, patched);
        assert_eq!(document.store.shader.generated_base_source, canonical_after);
        assert_eq!(dependency_root_target_name(&document), "return");
        assert!(has_target_named(&document, "after_root"));
        assert!(!has_target_named(&document, "shortwire_root"));
        assert!(dependency_rows_contain_label_fragment(
            &document,
            "after_root"
        ));
        assert!(!dependency_rows_contain_label_fragment(
            &document,
            "shortwire_root"
        ));
    }

    #[test]
    fn navigating_to_other_dependency_exits_shortwire_without_auto_apply() {
        let canonical = root_return_shader("canonical_root", 1.0);
        let patched = root_return_shader("shortwire_root", 2.0);
        let source = PassDebugSource::from_wgsl("p", canonical.clone());
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document
            .store
            .dependencies
            .rows
            .first()
            .cloned()
            .expect("dependency root row");

        document.enter_shortwire(&row, &pending_actions);
        document.store.shader.draft_source = patched.clone();
        document.mark_draft_edited(0.0);

        document.exit_shortwire_navigate(&pending_actions);

        assert!(document.store.shortwire.active.is_none());
        assert_eq!(document.store.shader.draft_source, canonical);
        assert_eq!(document.store.shader.loaded_source, canonical);
        assert!(!document.store.shader.patch_active);
        assert!(!document.store.shader.dirty);
        assert!(!document.store.shortwire.patches.is_empty());
        let actions = pending_actions.lock().unwrap();
        assert!(actions.is_empty());
    }

    #[test]
    fn shortwire_active_row_click_is_noop_but_other_row_exits() {
        let active_click = PassDebugTreeClick {
            row_key: Some("0".to_string()),
            target_id: None,
            source_range: None,
            toggle_row_key: None,
        };
        let active_toggle = PassDebugTreeClick {
            row_key: None,
            target_id: None,
            source_range: None,
            toggle_row_key: Some("0".to_string()),
        };
        let other_click = PassDebugTreeClick {
            row_key: Some("1".to_string()),
            target_id: None,
            source_range: None,
            toggle_row_key: None,
        };

        assert!(shortwire_click_matches_active_row("0", &active_click));
        assert!(shortwire_click_matches_active_row("0", &active_toggle));
        assert!(!shortwire_click_matches_active_row("0", &other_click));
    }

    #[test]
    fn generated_base_source_tracks_canonical_when_patch_active() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        assert_eq!(document.store.shader.generated_base_source, "fn a() {}\n");

        let refreshed = PassDebugSource::from_wgsl("p", "fn b() {}\n");
        document.update_source(Some(&refreshed), 1, Some("fn patched() {}\n"));

        assert_eq!(document.store.shader.generated_base_source, "fn b() {}\n");
        assert_eq!(document.store.shader.draft_source, "fn patched() {}\n");
        assert!(document.store.shader.patch_active);
    }

    #[test]
    fn canonical_change_with_applied_patch_auto_merges_cleanly() {
        let base = "fn a() {\n    let x = 1;\n    let y = 2;\n}\n";
        let local = "fn a() {\n    let x = 99;\n    let y = 2;\n}\n";
        let incoming = "fn a() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}\n";
        let expected = "fn a() {\n    let x = 99;\n    let y = 2;\n    let z = 3;\n}\n";

        let source = PassDebugSource::from_wgsl("p", base);
        let incoming_source = PassDebugSource::from_wgsl("p", incoming);
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source), 0, Some(local));
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        document.update_source_with_actions(
            Some(&incoming_source),
            1,
            Some(local),
            &pending_actions,
        );

        assert!(document.store.merge.conflict.is_none());
        assert_eq!(document.store.shader.generated_base_source, incoming);
        assert!(
            document
                .store
                .shader
                .last_status
                .as_ref()
                .unwrap()
                .contains("rebasing")
        );
        let actions = pending_actions.lock().unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            super::PassDebugWindowAction::ApplyPatch { source, .. } => {
                assert_eq!(source, expected);
            }
            _ => panic!("expected ApplyPatch action"),
        }
    }

    #[test]
    fn auto_merge_rebases_matching_shortwire_patch_after_apply_success() {
        let base = "fn a() {\n    let x = 1;\n    let y = 2;\n}\n";
        let local = "fn a() {\n    let x = 99;\n    let y = 2;\n}\n";
        let incoming = "fn a() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}\n";
        let expected = "fn a() {\n    let x = 99;\n    let y = 2;\n    let z = 3;\n}\n";

        let source = PassDebugSource::from_wgsl("p", base);
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document
            .store
            .dependencies
            .rows
            .first()
            .cloned()
            .expect("dependency root row");

        document.enter_shortwire(&row, &pending_actions);
        document.store.shader.draft_source = local.to_string();
        document.exit_shortwire_done(&pending_actions);
        document.mark_applied(Some(&source), 1, local.to_string(), "Applied".to_string());
        assert_eq!(document.store.shortwire.patches.len(), 1);

        pending_actions.lock().unwrap().clear();
        let incoming_source = PassDebugSource::from_wgsl("p", incoming);
        document.update_source_with_actions(
            Some(&incoming_source),
            2,
            Some(local),
            &pending_actions,
        );
        assert!(
            document
                .store
                .shortwire
                .active
                .as_ref()
                .unwrap()
                .base_source_stale
        );
        assert!(pending_actions.lock().unwrap().is_empty());

        document.exit_shortwire_done(&pending_actions);
        let actions = pending_actions.lock().unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            super::PassDebugWindowAction::ApplyPatch { source, .. } => {
                assert_eq!(source, expected);
            }
            _ => panic!("expected ApplyPatch action"),
        }
        drop(actions);
        document.mark_applied(
            Some(&incoming_source),
            3,
            expected.to_string(),
            "Applied".to_string(),
        );

        let patch = document
            .store
            .shortwire
            .patches
            .values()
            .next()
            .expect("rebased patch");
        assert_eq!(patch.base_source_hash, super::hash_source(incoming));
        assert_eq!(
            super::apply_hunks(incoming, &patch.hunks).unwrap(),
            expected
        );
    }

    #[test]
    fn canonical_change_with_applied_patch_enters_merge_conflict() {
        let base = "fn a() {\n    let x = 1;\n}\n";
        let local = "fn a() {\n    let x = 99;\n}\n";
        let incoming = "fn a() {\n    let x = 2;\n}\n";

        let source = PassDebugSource::from_wgsl("p", base);
        let incoming_source = PassDebugSource::from_wgsl("p", incoming);
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source), 0, Some(local));
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        document.update_source_with_actions(
            Some(&incoming_source),
            1,
            Some(local),
            &pending_actions,
        );

        assert!(pending_actions.lock().unwrap().is_empty());
        let conflict = document
            .store
            .merge
            .conflict
            .as_ref()
            .expect("merge conflict");
        assert_eq!(conflict.base_source, base);
        assert_eq!(conflict.incoming_source, incoming);
        assert_eq!(conflict.local_source, local);
        assert_eq!(conflict.resolved_source, local);
        assert!(conflict.choice_popup_open);
        assert!(!conflict.resolver_window_open);
        assert_eq!(document.store.shader.generated_base_source, incoming);
        assert!(
            document
                .store
                .shader
                .last_error
                .as_ref()
                .unwrap()
                .contains("conflicts")
        );

        document.open_merge_resolver();
        let conflict = document
            .store
            .merge
            .conflict
            .as_ref()
            .expect("merge conflict");
        assert!(!conflict.choice_popup_open);
        assert!(conflict.resolver_window_open);
    }

    #[test]
    fn generated_base_source_updated_when_not_patch_active() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);

        let refreshed = PassDebugSource::from_wgsl("p", "fn b() {}\n");
        document.update_source(Some(&refreshed), 1, false);

        assert_eq!(document.store.shader.generated_base_source, "fn b() {}\n");
    }

    #[test]
    fn update_source_during_active_shortwire_does_not_overwrite_draft() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row =
            document
                .store
                .dependencies
                .rows
                .first()
                .cloned()
                .unwrap_or(PassDebugDependencyRow {
                    depth: 0,
                    row_key: "0".to_string(),
                    parent_row_key: None,
                    label: "test".to_string(),
                    relation_path: String::new(),
                    target_id: Some("t".to_string()),
                    source_range: None,
                    source_jump_range: None,
                    selectable: true,
                });
        document.enter_shortwire(&row, &pending_actions);
        assert!(document.store.shortwire.active.is_some());

        document.store.shader.draft_source = "fn user_edit() {}\n".to_string();

        let refreshed = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        document.update_source(Some(&refreshed), 1, false);

        assert_eq!(document.store.shader.draft_source, "fn user_edit() {}\n");
    }

    #[test]
    fn update_source_during_active_shortwire_sets_base_source_stale() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);

        let refreshed = PassDebugSource::from_wgsl("p", "fn new_base() {}\n");
        document.update_source(Some(&refreshed), 1, false);

        assert!(
            document
                .store
                .shortwire
                .active
                .as_ref()
                .unwrap()
                .base_source_stale
        );
    }

    #[test]
    fn mark_reset_triggers_pending_reset_then_enter_transition() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new(
            "p".to_string(),
            Some(source.clone()),
            0,
            Some("fn patched() {}\n"),
        );

        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);
        assert!(matches!(
            document.store.shortwire.active.as_ref().unwrap().phase,
            super::ShortwirePhase::PendingResetThenEnter { .. }
        ));

        let fresh_source = PassDebugSource::from_wgsl("p", "fn fresh() {}\n");
        document.mark_reset(Some(&fresh_source), 2, "Reset".to_string());

        assert!(matches!(
            document.store.shortwire.active.as_ref().unwrap().phase,
            super::ShortwirePhase::Editing
        ));
        assert!(
            !document
                .store
                .shortwire
                .active
                .as_ref()
                .unwrap()
                .base_source_stale
        );
        assert_eq!(
            document.store.shader.generated_base_source,
            "fn fresh() {}\n"
        );
    }

    #[test]
    fn record_error_during_pending_reset_clears_shortwire() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new(
            "p".to_string(),
            Some(source),
            0,
            Some("fn patched() {}\n"),
        );

        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);

        document.record_error("reset failed".to_string());

        assert!(document.store.shortwire.active.is_none());
        assert!(
            document
                .store
                .shader
                .last_error
                .as_ref()
                .unwrap()
                .contains("Failed to reset patch")
        );
    }

    #[test]
    fn record_error_during_pending_apply_reverts_to_editing() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);
        if let Some(ref mut active) = document.store.shortwire.active {
            active.diff_view_enabled = true;
        }
        document.store.shader.draft_source = "fn edited() {}\n".to_string();
        document.mark_draft_edited(1.0);
        document.exit_shortwire_done(&pending_actions);

        assert!(matches!(
            document.store.shortwire.active.as_ref().unwrap().phase,
            super::ShortwirePhase::PendingApply { .. }
        ));

        document.record_error("apply failed".to_string());

        assert!(matches!(
            document.store.shortwire.active.as_ref().unwrap().phase,
            super::ShortwirePhase::Editing
        ));
        assert!(document.shortwire_is_editor_interactive());
        assert!(!document.store.shortwire.exit_on_apply);
        assert!(document.store.shader.dirty);
        assert!(
            !document
                .store
                .shortwire
                .active
                .as_ref()
                .unwrap()
                .diff_view_enabled
        );
        assert_eq!(
            document.store.shader.last_error.as_deref(),
            Some("apply failed")
        );
    }

    #[test]
    fn cancel_during_editing_no_patch_stored() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);
        document.store.shader.draft_source = "fn edited() {}\n".to_string();

        document.exit_shortwire_cancel();

        assert!(document.store.shortwire.active.is_none());
        assert!(document.store.shortwire.patches.is_empty());
        assert_eq!(document.store.shader.draft_source, "fn a() {}\n");
    }

    #[test]
    fn apply_hunks_reverse_order() {
        let base = "line1\nline2\nline3\nline4\n";
        let hunks = vec![
            super::ShortwireHunk {
                old_start: 0,
                old_lines: vec!["line1".to_string()],
                new_lines: vec!["LINE1".to_string()],
                context_before: vec![],
                context_after: vec!["line2".to_string()],
            },
            super::ShortwireHunk {
                old_start: 2,
                old_lines: vec!["line3".to_string()],
                new_lines: vec!["LINE3".to_string()],
                context_before: vec!["line2".to_string()],
                context_after: vec!["line4".to_string()],
            },
        ];
        let result = super::apply_hunks(base, &hunks).unwrap();
        assert_eq!(result, "LINE1\nline2\nLINE3\nline4\n");
    }

    #[test]
    fn fuzzy_hunk_application_at_shifted_offset() {
        let hunks = vec![super::ShortwireHunk {
            old_start: 1,
            old_lines: vec!["line2".to_string()],
            new_lines: vec!["LINE2".to_string()],
            context_before: vec!["line1".to_string()],
            context_after: vec!["line3".to_string()],
        }];

        let shifted_base = "extra\nheader\nline1\nline2\nline3\n";
        let result = super::apply_hunks(shifted_base, &hunks).unwrap();
        assert_eq!(result, "extra\nheader\nline1\nLINE2\nline3\n");
    }

    #[test]
    fn insert_only_hunks_use_context_for_positioning() {
        let base = "a\nb\nc\n";
        let hunks = vec![super::ShortwireHunk {
            old_start: 1,
            old_lines: vec![],
            new_lines: vec!["INSERTED".to_string()],
            context_before: vec!["a".to_string()],
            context_after: vec!["b".to_string()],
        }];
        let result = super::apply_hunks(base, &hunks).unwrap();
        assert_eq!(result, "a\nINSERTED\nb\nc\n");
    }

    #[test]
    fn failed_hunk_application_returns_error() {
        let base = "line1\nline2\nline3\n";
        let hunks = vec![super::ShortwireHunk {
            old_start: 0,
            old_lines: vec!["nonexistent".to_string()],
            new_lines: vec!["replaced".to_string()],
            context_before: vec![],
            context_after: vec![],
        }];
        let result = super::apply_hunks(base, &hunks);
        assert!(result.is_err());
    }

    #[test]
    fn patch_key_stability_different_relation_paths() {
        let row_a = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "x".to_string(),
            relation_path: "path_a".to_string(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        let row_b = PassDebugDependencyRow {
            depth: 0,
            row_key: "1".to_string(),
            parent_row_key: None,
            label: "x".to_string(),
            relation_path: "path_b".to_string(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        assert_ne!(
            super::shortwire_patch_key(&row_a),
            super::shortwire_patch_key(&row_b)
        );
    }

    #[test]
    fn patch_key_with_source_range_fingerprint() {
        let row_a = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "x".to_string(),
            relation_path: "path".to_string(),
            target_id: Some("t".to_string()),
            source_range: Some(PassDebugSourceRange {
                start_byte: 10,
                end_byte: 20,
                line: 1,
                column: 1,
            }),
            source_jump_range: None,
            selectable: true,
        };
        let row_b = PassDebugDependencyRow {
            depth: 0,
            row_key: "1".to_string(),
            parent_row_key: None,
            label: "x".to_string(),
            relation_path: "path".to_string(),
            target_id: Some("t".to_string()),
            source_range: Some(PassDebugSourceRange {
                start_byte: 30,
                end_byte: 40,
                line: 2,
                column: 1,
            }),
            source_jump_range: None,
            selectable: true,
        };
        assert_ne!(
            super::shortwire_patch_key(&row_a),
            super::shortwire_patch_key(&row_b)
        );
    }

    #[test]
    fn document_opened_with_patch_active_keeps_canonical_base() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let document = PassDebugWindowDocument::new(
            "p".to_string(),
            Some(source),
            0,
            Some("fn patched() {}\n"),
        );

        assert_eq!(document.store.shader.generated_base_source, "fn a() {}\n");
        assert_eq!(document.store.shader.draft_source, "fn patched() {}\n");
        assert!(document.store.shader.patch_active);
    }

    #[test]
    fn compute_and_apply_hunks_roundtrip() {
        let base = "fn main() {\n    let x = 1;\n    let y = 2;\n    return x + y;\n}\n";
        let edited = "fn main() {\n    let x = 10;\n    let y = 2;\n    let z = 3;\n    return x + y + z;\n}\n";

        let hunks = super::compute_hunks(base, edited);
        assert!(!hunks.is_empty());

        let result = super::apply_hunks(base, &hunks).unwrap();
        assert_eq!(result, edited);
    }

    #[test]
    fn re_entering_same_node_after_apply_restores_patch() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);
        document.store.shader.draft_source = "fn edited() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);

        let patched_source = PassDebugSource::from_wgsl("p", "fn edited() {}\n");
        document.mark_applied(
            Some(&patched_source),
            1,
            "fn edited() {}\n".to_string(),
            "Applied".to_string(),
        );

        assert!(document.store.shortwire.active.is_some());
        assert!(document.shortwire_is_editor_interactive());
        assert!(document.store.shader.patch_active);
        assert!(!document.store.shortwire.patches.is_empty());

        pending_actions.lock().unwrap().clear();
        document.exit_shortwire_navigate(&pending_actions);
        assert!(document.store.shortwire.active.is_none());
        assert!(!document.store.shader.patch_active);
        {
            let actions = pending_actions.lock().unwrap();
            assert_eq!(actions.len(), 1);
            assert!(matches!(
                actions[0],
                super::PassDebugWindowAction::ResetPatch { .. }
            ));
        }

        let reset_source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        document.mark_reset(Some(&reset_source), 2, "Reset".to_string());

        document.enter_shortwire(&row, &pending_actions);

        assert!(document.store.shortwire.active.is_some());
        assert_eq!(document.store.shader.draft_source, "fn edited() {}\n");
        assert!(
            document
                .store
                .shortwire
                .active
                .as_ref()
                .unwrap()
                .diff_view_enabled
        );
    }

    #[test]
    fn enter_shortwire_and_apply_auto_applies_stored_patch() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };

        document.enter_shortwire(&row, &pending_actions);
        document.store.shader.draft_source = "fn edited() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);
        let patched_source = PassDebugSource::from_wgsl("p", "fn edited() {}\n");
        document.mark_applied(
            Some(&patched_source),
            1,
            "fn edited() {}\n".to_string(),
            "Applied".to_string(),
        );
        let patch_key = super::shortwire_patch_key(&row);
        document
            .store
            .shortwire
            .patches
            .get_mut(&patch_key)
            .unwrap()
            .reference_image = Some(test_reference_image());
        pending_actions.lock().unwrap().clear();
        document.exit_shortwire_navigate(&pending_actions);
        let reset_source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        document.mark_reset(Some(&reset_source), 2, "Reset".to_string());

        pending_actions.lock().unwrap().clear();
        document.enter_shortwire_and_apply(&row, &pending_actions);

        assert!(matches!(
            document.store.shortwire.active.as_ref().unwrap().phase,
            super::ShortwirePhase::PendingApply { .. }
        ));
        let actions = pending_actions.lock().unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            super::PassDebugWindowAction::ApplyPatch {
                source,
                reference_image,
                ..
            } => {
                assert_eq!(source, "fn edited() {}\n");
                assert_eq!(
                    reference_image
                        .as_ref()
                        .map(|image| image.artifact_id.as_str()),
                    Some("pass__p__shortwire-reference-image__k")
                );
            }
            _ => panic!("expected ApplyPatch action"),
        }
    }

    #[test]
    fn enter_shortwire_and_apply_falls_back_to_edit_on_conflict() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };

        document.enter_shortwire(&row, &pending_actions);
        document.store.shader.draft_source = "fn edited() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);
        let patched_source = PassDebugSource::from_wgsl("p", "fn edited() {}\n");
        document.mark_applied(
            Some(&patched_source),
            1,
            "fn edited() {}\n".to_string(),
            "Applied".to_string(),
        );
        pending_actions.lock().unwrap().clear();
        document.exit_shortwire_navigate(&pending_actions);

        let completely_different = PassDebugSource::from_wgsl(
            "p",
            "struct X { v: f32 }\nfn totally_different() -> X { return X(0.0); }\n",
        );
        document.mark_reset(Some(&completely_different), 2, "Reset".to_string());

        pending_actions.lock().unwrap().clear();
        document.enter_shortwire_and_apply(&row, &pending_actions);

        assert!(matches!(
            document.store.shortwire.active.as_ref().unwrap().phase,
            super::ShortwirePhase::Editing
        ));
        assert!(document.store.shortwire.patches.is_empty());
        assert!(
            document
                .store
                .shader
                .last_error
                .as_ref()
                .unwrap()
                .contains("outdated")
        );
    }

    #[test]
    fn done_with_stale_base_rebases_edits() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {\n    let x = 1;\n}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);

        document.store.shader.draft_source = "fn a() {\n    let x = 99;\n}\n".to_string();

        let new_base =
            PassDebugSource::from_wgsl("p", "fn a() {\n    let x = 1;\n    let y = 2;\n}\n");
        document.update_source(Some(&new_base), 1, false);
        assert!(
            document
                .store
                .shortwire
                .active
                .as_ref()
                .unwrap()
                .base_source_stale
        );

        document.exit_shortwire_done(&pending_actions);

        assert!(matches!(
            document.store.shortwire.active.as_ref().unwrap().phase,
            super::ShortwirePhase::PendingApply { .. }
        ));
        assert_eq!(
            document.store.shader.draft_source,
            "fn a() {\n    let x = 99;\n    let y = 2;\n}\n"
        );
    }

    #[test]
    fn pending_analysis_discarded_on_shortwire_entry() {
        let source = PassDebugSource::from_wgsl("p", "fn a() -> f32 { return 1.0; }\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        document.store.shader.draft_source = "fn edited() -> f32 { return 2.0; }\n".to_string();
        document.mark_draft_edited(10.0);
        document.maybe_refresh_pending_draft_analysis(10.2);

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);

        assert_eq!(document.draft_analysis_due_secs, None);
    }
}
