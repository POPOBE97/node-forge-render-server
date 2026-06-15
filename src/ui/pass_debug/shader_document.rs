use crate::renderer::PassDebugSource;

#[derive(Clone, Debug)]
pub(crate) struct ShaderDocumentState {
    pub(crate) source: Option<PassDebugSource>,
    pub(crate) analysis_source: Option<PassDebugSource>,
    pub(crate) analysis_source_text: String,
    pub(crate) source_revision: Option<u64>,
    pub(crate) draft_source: String,
    pub(crate) loaded_source: String,
    pub(crate) dirty: bool,
    pub(crate) patch_active: bool,
    pub(crate) last_error: Option<String>,
    pub(crate) last_status: Option<String>,
    pub(crate) generated_base_source: String,
    pub(crate) generated_base_source_hash: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShaderDraftChange {
    pub(crate) draft_changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShaderSourceRefreshOutcome {
    pub(crate) draft_changed: bool,
    pub(crate) source_missing: bool,
}

impl ShaderDocumentState {
    pub(crate) fn new(
        source: Option<PassDebugSource>,
        source_revision: u64,
        patch_source: Option<String>,
    ) -> Self {
        let canonical_source = source
            .as_ref()
            .map(|source| source.module_source.clone())
            .unwrap_or_default();
        let patch_active = patch_source.is_some();
        let loaded_source = patch_source
            .as_deref()
            .unwrap_or(canonical_source.as_str())
            .to_string();
        Self {
            source: source.clone(),
            analysis_source: source,
            analysis_source_text: canonical_source.clone(),
            source_revision: Some(source_revision),
            draft_source: loaded_source.clone(),
            loaded_source,
            dirty: false,
            patch_active,
            last_error: None,
            last_status: None,
            generated_base_source_hash: hash_source(&canonical_source),
            generated_base_source: canonical_source,
        }
    }

    pub(crate) fn replace_draft_source(&mut self, next_source: String) -> bool {
        if self.draft_source == next_source {
            return false;
        }
        self.draft_source = next_source;
        true
    }

    pub(crate) fn refresh_dirty_flag(&mut self) {
        self.dirty = self.draft_source != self.loaded_source;
    }

    pub(crate) fn mark_draft_edited(&mut self) {
        self.refresh_dirty_flag();
        self.last_status = None;
    }

    pub(crate) fn mark_apply_requested(&mut self) {
        self.last_error = None;
        self.last_status = Some("Applying patch...".to_string());
    }

    pub(crate) fn mark_shortwire_saving(&mut self) {
        self.last_error = None;
        self.last_status = Some("Saving...".to_string());
    }

    pub(crate) fn mark_stored_patch_applying(&mut self) {
        self.dirty = true;
        self.last_error = None;
        self.last_status = Some("Applying stored patch...".to_string());
    }

    pub(crate) fn set_status(&mut self, status: String) {
        self.last_status = Some(status);
    }

    pub(crate) fn set_error(&mut self, error: String) {
        self.last_error = Some(error);
    }

    pub(crate) fn clear_error(&mut self) {
        self.last_error = None;
    }

    pub(crate) fn clear_status(&mut self) {
        self.last_status = None;
    }

    pub(crate) fn set_generated_base_source(&mut self, source: String) {
        self.generated_base_source = source;
        self.generated_base_source_hash = hash_source(&self.generated_base_source);
    }

    pub(crate) fn clear_generated_base_source(&mut self) {
        self.generated_base_source.clear();
        self.generated_base_source_hash = hash_source("");
    }

    pub(crate) fn refresh_canonical_patch_snapshot(
        &mut self,
        source: Option<&PassDebugSource>,
        canonical_source: &str,
    ) {
        self.set_generated_base_source(canonical_source.to_string());
        self.analysis_source = source.cloned();
        self.analysis_source_text = canonical_source.to_string();
    }

    pub(crate) fn handle_same_revision_refresh(
        &mut self,
        source_revision: u64,
        patch_source: Option<&str>,
        shortwire_active: bool,
    ) -> Option<ShaderDraftChange> {
        self.patch_active = patch_source.is_some();
        if self.source_revision != Some(source_revision) {
            return None;
        }

        let mut draft_changed = false;
        if !shortwire_active
            && !self.dirty
            && let Some(patch_source) = patch_source
            && self.loaded_source != patch_source
        {
            self.loaded_source = patch_source.to_string();
            draft_changed = self.replace_draft_source(patch_source.to_string());
            self.last_error = None;
        }
        Some(ShaderDraftChange { draft_changed })
    }

    pub(crate) fn begin_source_revision(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
    ) {
        self.source_revision = Some(source_revision);
        self.source = source.cloned();
    }

    pub(crate) fn refresh_shortwire_canonical_snapshot(
        &mut self,
        source: Option<&PassDebugSource>,
        canonical_source: &str,
    ) -> bool {
        self.analysis_source = source.cloned();
        self.analysis_source_text = canonical_source.to_string();
        if canonical_source == self.generated_base_source {
            return false;
        }
        self.set_generated_base_source(canonical_source.to_string());
        true
    }

    pub(crate) fn refresh_source_snapshot(
        &mut self,
        source: Option<&PassDebugSource>,
        canonical_source: &str,
        next_editor_source: String,
    ) -> ShaderSourceRefreshOutcome {
        if canonical_source != self.generated_base_source {
            self.set_generated_base_source(canonical_source.to_string());
        }

        let mut draft_changed = false;
        let mut source_missing = false;
        if !self.dirty {
            if source.is_none() {
                self.loaded_source.clear();
                draft_changed = self.replace_draft_source(String::new());
                self.analysis_source = None;
                self.analysis_source_text.clear();
                self.clear_generated_base_source();
                self.last_error = None;
                source_missing = true;
                return ShaderSourceRefreshOutcome {
                    draft_changed,
                    source_missing,
                };
            }

            self.loaded_source = next_editor_source.clone();
            draft_changed = self.replace_draft_source(next_editor_source);
            self.last_error = None;
        }

        self.analysis_source = source.cloned();
        self.analysis_source_text = canonical_source.to_string();
        ShaderSourceRefreshOutcome {
            draft_changed,
            source_missing,
        }
    }

    pub(crate) fn enter_merge_conflict(&mut self, local_source: String, status: String) -> bool {
        self.loaded_source = local_source.clone();
        let draft_changed = self.replace_draft_source(local_source);
        self.dirty = false;
        self.last_error = Some(status);
        self.last_status = None;
        draft_changed
    }

    pub(crate) fn restore_runtime_source(
        &mut self,
        restored_source: String,
        status: String,
    ) -> bool {
        self.loaded_source = restored_source.clone();
        let draft_changed = self.replace_draft_source(restored_source);
        self.dirty = false;
        self.last_error = None;
        self.last_status = Some(status);
        draft_changed
    }

    pub(crate) fn restore_generated_base_runtime(&mut self) -> bool {
        let generated_base_source = self.generated_base_source.clone();
        self.loaded_source = generated_base_source.clone();
        let draft_changed = self.replace_draft_source(generated_base_source);
        self.dirty = false;
        draft_changed
    }

    pub(crate) fn mark_resetting_after_shortwire_exit(&mut self) {
        self.patch_active = false;
        self.last_status = Some("Resetting...".to_string());
    }

    pub(crate) fn clear_shortwire_exit_error_and_idle_status(&mut self) {
        self.last_error = None;
        if !self.patch_active && self.last_status.as_deref() != Some("Resetting...") {
            self.last_status = None;
        }
    }

    pub(crate) fn mark_patch_applied(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        draft_source: String,
        status: String,
    ) -> bool {
        let canonical_source = source
            .map(|source| source.module_source.as_str())
            .unwrap_or_default()
            .to_string();
        self.source_revision = Some(source_revision);
        self.source = source.cloned();
        self.loaded_source = draft_source.clone();
        let draft_changed = self.replace_draft_source(draft_source);
        self.analysis_source = source.cloned();
        self.analysis_source_text = canonical_source.clone();
        self.set_generated_base_source(canonical_source);
        self.dirty = false;
        self.patch_active = true;
        self.last_error = None;
        self.last_status = Some(status);
        draft_changed
    }

    pub(crate) fn mark_patch_reset(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        status: String,
    ) -> bool {
        self.source_revision = Some(source_revision);
        self.source = source.cloned();
        let mut draft_changed = false;
        if let Some(source) = source {
            self.loaded_source = source.module_source.clone();
            draft_changed = self.replace_draft_source(source.module_source.clone());
            self.analysis_source = Some(source.clone());
            self.analysis_source_text = self.draft_source.clone();
            self.set_generated_base_source(source.module_source.clone());
        } else {
            self.analysis_source = None;
            self.analysis_source_text.clear();
            self.clear_generated_base_source();
        }
        self.dirty = false;
        self.patch_active = false;
        self.last_error = None;
        self.last_status = Some(status);
        draft_changed
    }

    pub(crate) fn record_error(&mut self, error: String) {
        self.last_error = Some(error);
        self.last_status = None;
    }

    pub(crate) fn record_reset_error(&mut self, error: String) {
        self.record_error(format!("Failed to reset patch: {error}"));
    }
}

pub(crate) fn hash_source(source: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = ahash::AHasher::default();
    source.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_without_patch_tracks_canonical_as_loaded_and_draft() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let state = ShaderDocumentState::new(Some(source), 7, None);

        assert_eq!(state.source_revision, Some(7));
        assert_eq!(state.generated_base_source, "fn generated() {}\n");
        assert_eq!(state.loaded_source, "fn generated() {}\n");
        assert_eq!(state.draft_source, "fn generated() {}\n");
        assert!(!state.patch_active);
        assert!(!state.dirty);
    }

    #[test]
    fn new_with_patch_keeps_canonical_base_separate_from_runtime_patch() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let state = ShaderDocumentState::new(
            Some(source),
            7,
            Some("fn patched_runtime() {}\n".to_string()),
        );

        assert_eq!(state.generated_base_source, "fn generated() {}\n");
        assert_eq!(state.loaded_source, "fn patched_runtime() {}\n");
        assert_eq!(state.draft_source, "fn patched_runtime() {}\n");
        assert!(state.patch_active);
        assert!(!state.dirty);
    }

    #[test]
    fn draft_and_generated_base_helpers_update_derived_state() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let mut state = ShaderDocumentState::new(Some(source), 7, None);
        let original_hash = state.generated_base_source_hash;

        assert!(state.replace_draft_source("fn edited() {}\n".to_string()));
        assert!(!state.replace_draft_source("fn edited() {}\n".to_string()));
        state.refresh_dirty_flag();
        assert!(state.dirty);

        state.set_generated_base_source("fn generated_next() {}\n".to_string());
        assert_eq!(state.generated_base_source, "fn generated_next() {}\n");
        assert_ne!(state.generated_base_source_hash, original_hash);

        state.clear_generated_base_source();
        assert!(state.generated_base_source.is_empty());
        assert_eq!(state.generated_base_source_hash, hash_source(""));
    }

    #[test]
    fn draft_edit_and_apply_request_update_status_fields() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let mut state = ShaderDocumentState::new(Some(source), 7, None);
        state.last_status = Some("Previous".to_string());

        assert!(state.replace_draft_source("fn edited() {}\n".to_string()));
        state.mark_draft_edited();

        assert!(state.dirty);
        assert!(state.last_status.is_none());
        state.last_error = Some("old error".to_string());

        state.mark_apply_requested();

        assert!(state.last_error.is_none());
        assert_eq!(state.last_status.as_deref(), Some("Applying patch..."));
    }

    #[test]
    fn canonical_patch_snapshot_updates_analysis_and_base() {
        let original = PassDebugSource::from_wgsl("p", "fn generated_old() {}\n");
        let incoming = PassDebugSource::from_wgsl("p", "fn generated_new() {}\n");
        let mut state = ShaderDocumentState::new(Some(original), 7, None);

        state.refresh_canonical_patch_snapshot(Some(&incoming), incoming.module_source.as_str());

        assert_eq!(state.generated_base_source, "fn generated_new() {}\n");
        assert_eq!(state.analysis_source_text, "fn generated_new() {}\n");
        assert_eq!(
            state
                .analysis_source
                .as_ref()
                .map(|source| source.module_source.as_str()),
            Some("fn generated_new() {}\n")
        );
    }

    #[test]
    fn merge_conflict_and_restore_runtime_source_update_runtime_fields() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let mut state = ShaderDocumentState::new(Some(source), 7, None);

        assert!(state.enter_merge_conflict("fn local() {}\n".to_string(), "conflict".to_string()));

        assert_eq!(state.loaded_source, "fn local() {}\n");
        assert_eq!(state.draft_source, "fn local() {}\n");
        assert!(!state.dirty);
        assert_eq!(state.last_error.as_deref(), Some("conflict"));
        assert!(state.last_status.is_none());

        assert!(
            state.restore_runtime_source("fn restored() {}\n".to_string(), "restored".to_string())
        );

        assert_eq!(state.loaded_source, "fn restored() {}\n");
        assert_eq!(state.draft_source, "fn restored() {}\n");
        assert!(!state.dirty);
        assert!(state.last_error.is_none());
        assert_eq!(state.last_status.as_deref(), Some("restored"));
    }

    #[test]
    fn shortwire_exit_helpers_restore_generated_base_and_status() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let mut state = ShaderDocumentState::new(
            Some(source),
            7,
            Some("fn patched_runtime() {}\n".to_string()),
        );
        state.set_generated_base_source("fn latest_base() {}\n".to_string());
        state.last_error = Some("old".to_string());

        assert!(state.restore_generated_base_runtime());
        assert_eq!(state.loaded_source, "fn latest_base() {}\n");
        assert_eq!(state.draft_source, "fn latest_base() {}\n");
        assert!(!state.dirty);

        state.mark_resetting_after_shortwire_exit();
        assert!(!state.patch_active);
        assert_eq!(state.last_status.as_deref(), Some("Resetting..."));
        state.clear_shortwire_exit_error_and_idle_status();
        assert!(state.last_error.is_none());
        assert_eq!(state.last_status.as_deref(), Some("Resetting..."));

        state.last_status = Some("Other".to_string());
        state.clear_shortwire_exit_error_and_idle_status();
        assert!(state.last_status.is_none());
    }

    #[test]
    fn mark_patch_applied_updates_runtime_and_canonical_snapshot() {
        let original = PassDebugSource::from_wgsl("p", "fn generated_old() {}\n");
        let incoming = PassDebugSource::from_wgsl("p", "fn generated_new() {}\n");
        let mut state = ShaderDocumentState::new(Some(original), 7, None);
        assert!(state.replace_draft_source("fn dirty() {}\n".to_string()));
        state.refresh_dirty_flag();

        let draft_changed = state.mark_patch_applied(
            Some(&incoming),
            8,
            "fn patched_runtime() {}\n".to_string(),
            "Applied".to_string(),
        );

        assert!(draft_changed);
        assert_eq!(state.source_revision, Some(8));
        assert_eq!(state.generated_base_source, "fn generated_new() {}\n");
        assert_eq!(state.loaded_source, "fn patched_runtime() {}\n");
        assert_eq!(state.draft_source, "fn patched_runtime() {}\n");
        assert!(state.patch_active);
        assert!(!state.dirty);
        assert_eq!(state.last_status.as_deref(), Some("Applied"));
        assert!(state.last_error.is_none());
    }

    #[test]
    fn same_revision_patch_refresh_updates_clean_runtime_draft() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let mut state = ShaderDocumentState::new(Some(source), 7, None);

        let change =
            state.handle_same_revision_refresh(7, Some("fn patched_runtime() {}\n"), false);

        assert_eq!(
            change,
            Some(ShaderDraftChange {
                draft_changed: true
            })
        );
        assert!(state.patch_active);
        assert_eq!(state.loaded_source, "fn patched_runtime() {}\n");
        assert_eq!(state.draft_source, "fn patched_runtime() {}\n");
        assert!(!state.dirty);
    }

    #[test]
    fn same_revision_patch_refresh_does_not_clobber_dirty_draft() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let mut state = ShaderDocumentState::new(Some(source), 7, None);
        assert!(state.replace_draft_source("fn user_edit() {}\n".to_string()));
        state.refresh_dirty_flag();

        let change =
            state.handle_same_revision_refresh(7, Some("fn patched_runtime() {}\n"), false);

        assert_eq!(
            change,
            Some(ShaderDraftChange {
                draft_changed: false
            })
        );
        assert!(state.patch_active);
        assert_eq!(state.loaded_source, "fn generated() {}\n");
        assert_eq!(state.draft_source, "fn user_edit() {}\n");
        assert!(state.dirty);
    }

    #[test]
    fn same_revision_patch_refresh_is_skipped_while_shortwire_is_active() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let mut state = ShaderDocumentState::new(Some(source), 7, None);

        let change = state.handle_same_revision_refresh(7, Some("fn patched_runtime() {}\n"), true);

        assert_eq!(
            change,
            Some(ShaderDraftChange {
                draft_changed: false
            })
        );
        assert!(state.patch_active);
        assert_eq!(state.loaded_source, "fn generated() {}\n");
        assert_eq!(state.draft_source, "fn generated() {}\n");
    }

    #[test]
    fn changed_revision_refresh_is_left_to_facade_flow() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let mut state = ShaderDocumentState::new(Some(source), 7, None);

        let change =
            state.handle_same_revision_refresh(8, Some("fn patched_runtime() {}\n"), false);

        assert_eq!(change, None);
        assert!(state.patch_active);
        assert_eq!(state.loaded_source, "fn generated() {}\n");
        assert_eq!(state.draft_source, "fn generated() {}\n");
    }

    #[test]
    fn source_refresh_updates_clean_editor_to_next_runtime_source() {
        let source = PassDebugSource::from_wgsl("p", "fn generated_old() {}\n");
        let next = PassDebugSource::from_wgsl("p", "fn generated_new() {}\n");
        let mut state = ShaderDocumentState::new(Some(source), 7, None);

        state.begin_source_revision(Some(&next), 8);
        let outcome = state.refresh_source_snapshot(
            Some(&next),
            next.module_source.as_str(),
            "fn patched_runtime() {}\n".to_string(),
        );

        assert_eq!(
            outcome,
            ShaderSourceRefreshOutcome {
                draft_changed: true,
                source_missing: false
            }
        );
        assert_eq!(state.source_revision, Some(8));
        assert_eq!(state.generated_base_source, "fn generated_new() {}\n");
        assert_eq!(state.loaded_source, "fn patched_runtime() {}\n");
        assert_eq!(state.draft_source, "fn patched_runtime() {}\n");
        assert!(!state.dirty);
        assert!(state.last_error.is_none());
    }

    #[test]
    fn source_refresh_keeps_dirty_editor_but_updates_canonical_snapshot() {
        let source = PassDebugSource::from_wgsl("p", "fn generated_old() {}\n");
        let next = PassDebugSource::from_wgsl("p", "fn generated_new() {}\n");
        let mut state = ShaderDocumentState::new(Some(source), 7, None);
        assert!(state.replace_draft_source("fn user_edit() {}\n".to_string()));
        state.refresh_dirty_flag();

        state.begin_source_revision(Some(&next), 8);
        let outcome = state.refresh_source_snapshot(
            Some(&next),
            next.module_source.as_str(),
            "fn generated_new() {}\n".to_string(),
        );

        assert_eq!(
            outcome,
            ShaderSourceRefreshOutcome {
                draft_changed: false,
                source_missing: false
            }
        );
        assert_eq!(state.generated_base_source, "fn generated_new() {}\n");
        assert_eq!(state.loaded_source, "fn generated_old() {}\n");
        assert_eq!(state.draft_source, "fn user_edit() {}\n");
        assert!(state.dirty);
    }

    #[test]
    fn source_refresh_clears_clean_editor_when_source_is_missing() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let mut state = ShaderDocumentState::new(Some(source), 7, None);

        state.begin_source_revision(None, 8);
        let outcome = state.refresh_source_snapshot(None, "", String::new());

        assert_eq!(
            outcome,
            ShaderSourceRefreshOutcome {
                draft_changed: true,
                source_missing: true
            }
        );
        assert!(state.source.is_none());
        assert!(state.analysis_source.is_none());
        assert!(state.analysis_source_text.is_empty());
        assert!(state.generated_base_source.is_empty());
        assert!(state.loaded_source.is_empty());
        assert!(state.draft_source.is_empty());
        assert!(!state.dirty);
    }

    #[test]
    fn shortwire_canonical_refresh_reports_base_change_without_touching_draft() {
        let source = PassDebugSource::from_wgsl("p", "fn generated_old() {}\n");
        let next = PassDebugSource::from_wgsl("p", "fn generated_new() {}\n");
        let mut state = ShaderDocumentState::new(Some(source), 7, None);
        assert!(state.replace_draft_source("fn shortwire_draft() {}\n".to_string()));
        state.refresh_dirty_flag();

        let changed =
            state.refresh_shortwire_canonical_snapshot(Some(&next), next.module_source.as_str());

        assert!(changed);
        assert_eq!(state.generated_base_source, "fn generated_new() {}\n");
        assert_eq!(state.draft_source, "fn shortwire_draft() {}\n");
        assert!(state.dirty);
    }

    #[test]
    fn mark_patch_reset_clears_patch_active_and_restores_source() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let reset_source = PassDebugSource::from_wgsl("p", "fn reset() {}\n");
        let mut state = ShaderDocumentState::new(
            Some(source),
            7,
            Some("fn patched_runtime() {}\n".to_string()),
        );
        state.dirty = true;
        state.last_error = Some("stale".to_string());

        let draft_changed = state.mark_patch_reset(Some(&reset_source), 8, "Reset".to_string());

        assert!(draft_changed);
        assert_eq!(state.source_revision, Some(8));
        assert_eq!(state.generated_base_source, "fn reset() {}\n");
        assert_eq!(state.loaded_source, "fn reset() {}\n");
        assert_eq!(state.draft_source, "fn reset() {}\n");
        assert!(!state.patch_active);
        assert!(!state.dirty);
        assert_eq!(state.last_status.as_deref(), Some("Reset"));
        assert!(state.last_error.is_none());
    }

    #[test]
    fn record_errors_update_status_fields() {
        let source = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        let mut state = ShaderDocumentState::new(Some(source), 7, None);
        state.last_status = Some("Applying".to_string());

        state.record_error("apply failed".to_string());
        assert_eq!(state.last_error.as_deref(), Some("apply failed"));
        assert!(state.last_status.is_none());

        state.record_reset_error("reset failed".to_string());
        assert_eq!(
            state.last_error.as_deref(),
            Some("Failed to reset patch: reset failed")
        );
    }
}
