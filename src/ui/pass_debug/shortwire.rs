use std::collections::HashMap;

use crate::app::ShortwireReferenceImage;
use crate::dsl::DebugArtifactItem;
use crate::ui::pass_debug::artifacts::{
    self, ShortwireDiffResult, ShortwireNodePatch, shortwire_patches_artifact_item,
};
use crate::ui::pass_debug::dependency_tree::{PassDebugDependencyRow, PassDebugTreeClick};
use crate::ui::pass_debug::merge::MergePatchRebase;
use crate::ui::pass_debug::patch::ShortwireHunk;
use crate::ui::pass_debug::shader_document::hash_source;

pub(crate) type ShortwirePatchesPayload = artifacts::ShortwirePatchesPayload<ShortwireNodePatch>;

pub(crate) const SHORTWIRE_DIFF_PASS_MAX_AE: f32 = 2.0 / 255.0;

pub(crate) fn shortwire_patch_key(row: &PassDebugDependencyRow) -> String {
    let range_suffix = row
        .source_range
        .map(|r| format!("#{}-{}", r.start_byte, r.end_byte))
        .unwrap_or_default();
    match row.target_id.as_deref() {
        Some(target_id) => {
            if row.relation_path.is_empty() {
                format!("target:{target_id}{range_suffix}")
            } else {
                format!("target:{target_id}@{}{range_suffix}", row.relation_path)
            }
        }
        None => format!("label:{}{range_suffix}", row.label),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ShortwireRowIdentity {
    pub(crate) patch_key: String,
    pub(crate) row_key_hint: String,
    pub(crate) label: String,
    #[allow(dead_code)]
    pub(crate) target_id: Option<String>,
}

pub(crate) fn shortwire_diff_result_summary(diff_result: Option<&ShortwireDiffResult>) -> String {
    match diff_result {
        Some(result) => format!(
            "diff=max_ae:{:.6},min:{:.6},avg:{:.6},rms:{:.6},p95:{:.6},n:{},nonfinite:{},render:{}x{},ref:{}x{},offset:{},{}",
            result.max_ae,
            result.min,
            result.avg,
            result.rms,
            result.p95_abs,
            result.sample_count,
            result.non_finite_count,
            result.render_size[0],
            result.render_size[1],
            result.reference_size[0],
            result.reference_size[1],
            result.reference_offset[0],
            result.reference_offset[1],
        ),
        None => "diff:none".to_string(),
    }
}

pub(crate) fn shortwire_patch_summary(patch: Option<&ShortwireNodePatch>) -> String {
    match patch {
        Some(patch) => format!(
            "patch=hunks:{},base_hash:{},status:{:?},{}",
            patch.hunks.len(),
            patch.base_source_hash,
            shortwire_dot_info_for_patch(patch).status,
            shortwire_diff_result_summary(patch.diff_result.as_ref()),
        ),
        None => "patch:none".to_string(),
    }
}

pub(crate) fn shortwire_diff_status(diff_result: &ShortwireDiffResult) -> ShortwireDotStatus {
    if diff_result.max_ae < SHORTWIRE_DIFF_PASS_MAX_AE {
        ShortwireDotStatus::Passing
    } else {
        ShortwireDotStatus::Failing
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortwireDiffCaptureRequest {
    pub pass_name: String,
    pub patch_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShortwireDotStatus {
    PendingDiff,
    Passing,
    Failing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShortwireDotInfo {
    pub(crate) status: ShortwireDotStatus,
    pub(crate) max_ae: Option<f32>,
    pub(crate) sample_count: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) enum ShortwirePhase {
    Editing,
    PendingApply { pending_hunks: Vec<ShortwireHunk> },
    PendingResetThenEnter { next_identity: ShortwireRowIdentity },
}

#[derive(Clone, Debug)]
pub(crate) struct ShortwireActiveState {
    pub(crate) identity: ShortwireRowIdentity,
    pub(crate) base_source: String,
    pub(crate) base_source_hash: u64,
    pub(crate) base_source_stale: bool,
    pub(crate) diff_view_enabled: bool,
    pub(crate) reference_image: Option<ShortwireReferenceImage>,
    pub(crate) phase: ShortwirePhase,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ShortwireState {
    pub(crate) patches: HashMap<String, ShortwireNodePatch>,
    pub(crate) patches_dirty: bool,
    pub(crate) active: Option<ShortwireActiveState>,
    pub(crate) exit_on_apply: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShortwireCommittedPatch {
    pub(crate) patch_key: String,
    pub(crate) hunk_count: usize,
    pub(crate) base_source_hash: u64,
    pub(crate) exit_on_apply: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShortwireStoredPatchSummary {
    pub(crate) hunk_count: usize,
    pub(crate) base_source_hash: u64,
    pub(crate) preserved_diff_result: bool,
}

impl ShortwireState {
    pub(crate) fn is_editor_interactive(&self) -> bool {
        matches!(
            self.active.as_ref().map(|active| &active.phase),
            Some(ShortwirePhase::Editing)
        )
    }

    pub(crate) fn collect_patches_artifact(
        &self,
        pass_name: &str,
    ) -> Option<(DebugArtifactItem, String)> {
        let payload = ShortwirePatchesPayload {
            version: 1,
            patches: self.patches.clone(),
        };
        let content_text = serde_json::to_string(&payload).ok()?;
        let item = shortwire_patches_artifact_item(pass_name, &content_text);
        Some((item, content_text))
    }

    pub(crate) fn restore_patches_from_text(&mut self, text: &str) -> bool {
        let Ok(payload) = serde_json::from_str::<ShortwirePatchesPayload>(text) else {
            return false;
        };
        if payload.version != 1 {
            return false;
        }
        self.patches = payload.patches;
        self.patches_dirty = false;
        true
    }

    pub(crate) fn take_patches_dirty_artifact(
        &mut self,
        pass_name: &str,
    ) -> Option<(DebugArtifactItem, String)> {
        if !self.patches_dirty {
            return None;
        }
        self.patches_dirty = false;
        self.collect_patches_artifact(pass_name)
    }

    pub(crate) fn patch_hunks_snapshot(&self) -> Vec<(String, Vec<ShortwireHunk>)> {
        self.patches
            .iter()
            .map(|(key, patch)| (key.clone(), patch.hunks.clone()))
            .collect()
    }

    pub(crate) fn apply_rebase(&mut self, rebase: MergePatchRebase) {
        if rebase.next_hunks.is_empty() {
            self.patches.remove(&rebase.patch_key);
        } else {
            let reference_image = self
                .patches
                .get(&rebase.patch_key)
                .and_then(|patch| patch.reference_image.clone());
            self.patches.insert(
                rebase.patch_key,
                ShortwireNodePatch {
                    hunks: rebase.next_hunks,
                    base_source_hash: hash_source(&rebase.incoming_source),
                    reference_image,
                    diff_result: None,
                },
            );
        }
        self.patches_dirty = true;
    }

    pub(crate) fn remove_patch(&mut self, patch_key: &str) {
        self.patches.remove(patch_key);
        self.patches_dirty = true;
    }

    pub(crate) fn store_close_patch(
        &mut self,
        patch_key: String,
        hunks: Vec<ShortwireHunk>,
        base_source_hash: u64,
        reference_image: Option<ShortwireReferenceImage>,
    ) -> ShortwireStoredPatchSummary {
        let previous_patch = self.patches.get(&patch_key);
        let preserved_diff_result = previous_patch
            .filter(|patch| patch.base_source_hash == base_source_hash && patch.hunks == hunks)
            .and_then(|patch| patch.diff_result.clone());
        let reference_image = reference_image
            .or_else(|| previous_patch.and_then(|patch| patch.reference_image.clone()));
        let preserved_diff = preserved_diff_result.is_some();
        let hunk_count = hunks.len();
        self.store_patch(
            patch_key,
            hunks,
            base_source_hash,
            reference_image,
            preserved_diff_result,
        );
        ShortwireStoredPatchSummary {
            hunk_count,
            base_source_hash,
            preserved_diff_result: preserved_diff,
        }
    }

    pub(crate) fn record_diff_result(
        &mut self,
        patch_key: &str,
        diff_result: ShortwireDiffResult,
    ) -> bool {
        let Some(patch) = self.patches.get_mut(patch_key) else {
            return false;
        };
        patch.diff_result = Some(diff_result);
        self.patches_dirty = true;
        true
    }

    pub(crate) fn set_active_reference_image(
        &mut self,
        reference_image: Option<ShortwireReferenceImage>,
    ) {
        if let Some(reference_image) = reference_image
            && let Some(active) = self.active.as_mut()
        {
            active.reference_image = Some(reference_image);
        }
    }

    pub(crate) fn create_image_patch_if_missing(
        &mut self,
        patch_key: String,
        hunks: Vec<ShortwireHunk>,
        base_source_hash: u64,
        reference_image: Option<ShortwireReferenceImage>,
    ) -> bool {
        if self.patches.contains_key(patch_key.as_str()) || reference_image.is_none() {
            return false;
        }
        self.store_patch(patch_key, hunks, base_source_hash, reference_image, None);
        true
    }

    pub(crate) fn prepare_diff_capture_patch(
        &mut self,
        patch_key: &str,
        reference_image: Option<ShortwireReferenceImage>,
    ) -> bool {
        let Some(patch) = self.patches.get_mut(patch_key) else {
            return false;
        };
        if let Some(reference_image) = reference_image {
            patch.reference_image = Some(reference_image);
        }
        patch.diff_result = None;
        self.patches_dirty = true;
        true
    }

    pub(crate) fn clear_active(&mut self) {
        self.active = None;
    }

    pub(crate) fn reference_image_for_patch(
        &self,
        patch_key: &str,
    ) -> Option<ShortwireReferenceImage> {
        self.patches
            .get(patch_key)
            .and_then(|patch| patch.reference_image.clone())
    }

    pub(crate) fn start_pending_reset_then_enter(
        &mut self,
        identity: ShortwireRowIdentity,
        reference_image: Option<ShortwireReferenceImage>,
    ) {
        self.active = Some(ShortwireActiveState {
            identity: identity.clone(),
            base_source: String::new(),
            base_source_hash: 0,
            base_source_stale: false,
            diff_view_enabled: false,
            reference_image,
            phase: ShortwirePhase::PendingResetThenEnter {
                next_identity: identity,
            },
        });
    }

    pub(crate) fn start_editing(
        &mut self,
        identity: ShortwireRowIdentity,
        base_source: String,
        base_source_hash: u64,
        reference_image: Option<ShortwireReferenceImage>,
    ) {
        self.active = Some(ShortwireActiveState {
            identity,
            base_source,
            base_source_hash,
            base_source_stale: false,
            diff_view_enabled: false,
            reference_image,
            phase: ShortwirePhase::Editing,
        });
    }

    pub(crate) fn start_pending_apply(
        &mut self,
        identity: ShortwireRowIdentity,
        base_source: String,
        base_source_hash: u64,
        reference_image: Option<ShortwireReferenceImage>,
        pending_hunks: Vec<ShortwireHunk>,
    ) {
        self.active = Some(ShortwireActiveState {
            identity,
            base_source,
            base_source_hash,
            base_source_stale: false,
            diff_view_enabled: false,
            reference_image,
            phase: ShortwirePhase::PendingApply { pending_hunks },
        });
    }

    pub(crate) fn mark_active_base_stale(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.base_source_stale = true;
        }
    }

    pub(crate) fn return_pending_apply_to_editing(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.phase = ShortwirePhase::Editing;
            active.diff_view_enabled = false;
        }
        self.exit_on_apply = false;
    }

    pub(crate) fn finish_apply_completion_editing(
        &mut self,
        base_source: String,
        base_source_hash: u64,
    ) {
        if let Some(active) = self.active.as_mut() {
            active.base_source = base_source;
            active.base_source_hash = base_source_hash;
            active.base_source_stale = false;
            active.phase = ShortwirePhase::Editing;
        }
    }

    pub(crate) fn complete_entry(
        &mut self,
        base_source: String,
        base_source_hash: u64,
    ) -> Option<ShortwireRowIdentity> {
        let active = self.active.as_mut()?;
        let identity = match &active.phase {
            ShortwirePhase::PendingResetThenEnter { next_identity } => next_identity.clone(),
            ShortwirePhase::Editing => active.identity.clone(),
            _ => return None,
        };

        active.identity = identity.clone();
        active.base_source = base_source;
        active.base_source_hash = base_source_hash;
        active.base_source_stale = false;
        active.phase = ShortwirePhase::Editing;
        Some(identity)
    }

    pub(crate) fn enable_active_diff_view(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.diff_view_enabled = true;
        }
    }

    pub(crate) fn set_active_pending_apply(&mut self, pending_hunks: Vec<ShortwireHunk>) {
        if let Some(active) = self.active.as_mut() {
            active.phase = ShortwirePhase::PendingApply { pending_hunks };
        }
    }

    pub(crate) fn clear_exit_on_apply(&mut self) {
        self.exit_on_apply = false;
    }

    pub(crate) fn request_exit_on_apply_and_clear_active(&mut self) {
        self.exit_on_apply = true;
        self.active = None;
    }

    pub(crate) fn commit_active_pending_apply(
        &mut self,
        base_source_hash: u64,
    ) -> Option<ShortwireCommittedPatch> {
        let (patch_key, pending_hunks, reference_image) = {
            let active = self.active.as_ref()?;
            let ShortwirePhase::PendingApply { pending_hunks } = &active.phase else {
                return None;
            };
            (
                active.identity.patch_key.clone(),
                pending_hunks.clone(),
                active.reference_image.clone(),
            )
        };
        let hunk_count = pending_hunks.len();
        self.store_patch(
            patch_key.clone(),
            pending_hunks,
            base_source_hash,
            reference_image,
            None,
        );
        Some(ShortwireCommittedPatch {
            patch_key,
            hunk_count,
            base_source_hash,
            exit_on_apply: self.exit_on_apply,
        })
    }

    pub(crate) fn take_active_and_commit_pending_apply(
        &mut self,
        base_source_hash: u64,
    ) -> Option<ShortwireCommittedPatch> {
        let active = self.active.take()?;
        let ShortwirePhase::PendingApply { pending_hunks } = active.phase else {
            return None;
        };
        let patch_key = active.identity.patch_key;
        let hunk_count = pending_hunks.len();
        self.store_patch(
            patch_key.clone(),
            pending_hunks,
            base_source_hash,
            active.reference_image,
            None,
        );
        Some(ShortwireCommittedPatch {
            patch_key,
            hunk_count,
            base_source_hash,
            exit_on_apply: self.exit_on_apply,
        })
    }

    fn store_patch(
        &mut self,
        patch_key: String,
        hunks: Vec<ShortwireHunk>,
        base_source_hash: u64,
        reference_image: Option<ShortwireReferenceImage>,
        diff_result: Option<ShortwireDiffResult>,
    ) {
        self.patches.insert(
            patch_key,
            ShortwireNodePatch {
                hunks,
                base_source_hash,
                reference_image,
                diff_result,
            },
        );
        self.patches_dirty = true;
    }
}

pub(crate) fn shortwire_click_matches_active_row(
    active_row_key: &str,
    click: &PassDebugTreeClick,
) -> bool {
    click.row_key.as_deref() == Some(active_row_key)
        || click.toggle_row_key.as_deref() == Some(active_row_key)
}

pub(crate) fn shortwire_dot_info_for_patch(patch: &ShortwireNodePatch) -> ShortwireDotInfo {
    match patch.diff_result.as_ref() {
        Some(diff_result) => ShortwireDotInfo {
            status: shortwire_diff_status(diff_result),
            max_ae: Some(diff_result.max_ae),
            sample_count: Some(diff_result.sample_count),
        },
        None => ShortwireDotInfo {
            status: ShortwireDotStatus::PendingDiff,
            max_ae: None,
            sample_count: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::pass_debug::merge::MergePatchRebase;

    fn test_diff_result(max_ae: f32) -> ShortwireDiffResult {
        ShortwireDiffResult {
            metric: "AE".to_string(),
            max_ae,
            min: 0.0,
            avg: 0.0,
            rms: 0.0,
            p95_abs: max_ae,
            sample_count: 10,
            non_finite_count: 0,
            render_size: [2, 2],
            reference_size: [2, 2],
            reference_offset: [0, 0],
        }
    }

    fn test_hunk() -> ShortwireHunk {
        ShortwireHunk {
            old_start: 0,
            old_lines: vec!["old".to_string()],
            new_lines: vec!["new".to_string()],
            context_before: vec![],
            context_after: vec![],
        }
    }

    fn pending_active(patch_key: &str) -> ShortwireActiveState {
        ShortwireActiveState {
            identity: ShortwireRowIdentity {
                patch_key: patch_key.to_string(),
                row_key_hint: "row".to_string(),
                label: "Row".to_string(),
                target_id: None,
            },
            base_source: "old".to_string(),
            base_source_hash: 1,
            base_source_stale: false,
            diff_view_enabled: false,
            reference_image: None,
            phase: ShortwirePhase::PendingApply {
                pending_hunks: vec![test_hunk()],
            },
        }
    }

    #[test]
    fn dot_status_uses_diff_result_threshold() {
        let pending = ShortwireNodePatch {
            hunks: Vec::new(),
            base_source_hash: 0,
            reference_image: None,
            diff_result: None,
        };
        assert_eq!(
            shortwire_dot_info_for_patch(&pending).status,
            ShortwireDotStatus::PendingDiff
        );

        let passing = ShortwireNodePatch {
            diff_result: Some(test_diff_result(1.0 / 255.0)),
            ..pending.clone()
        };
        assert_eq!(
            shortwire_dot_info_for_patch(&passing).status,
            ShortwireDotStatus::Passing
        );

        let failing = ShortwireNodePatch {
            diff_result: Some(test_diff_result(2.0 / 255.0)),
            ..pending
        };
        assert_eq!(
            shortwire_dot_info_for_patch(&failing).status,
            ShortwireDotStatus::Failing
        );
    }

    #[test]
    fn commit_active_pending_apply_stores_patch_and_keeps_active_session() {
        let mut state = ShortwireState {
            active: Some(pending_active("patch")),
            exit_on_apply: true,
            ..Default::default()
        };

        let committed = state
            .commit_active_pending_apply(99)
            .expect("pending apply should commit");

        assert_eq!(committed.patch_key, "patch");
        assert_eq!(committed.hunk_count, 1);
        assert_eq!(committed.base_source_hash, 99);
        assert!(committed.exit_on_apply);
        assert!(state.active.is_some());
        assert!(state.patches_dirty);
        assert_eq!(state.patches["patch"].base_source_hash, 99);
    }

    #[test]
    fn take_active_and_commit_pending_apply_clears_active_session() {
        let mut state = ShortwireState {
            active: Some(pending_active("patch")),
            ..Default::default()
        };

        let committed = state
            .take_active_and_commit_pending_apply(77)
            .expect("pending apply should commit");

        assert_eq!(committed.patch_key, "patch");
        assert!(state.active.is_none());
        assert!(state.patches_dirty);
        assert_eq!(state.patches["patch"].base_source_hash, 77);
    }

    #[test]
    fn apply_rebase_rewrites_or_removes_stored_patch() {
        let mut state = ShortwireState::default();
        state.patches.insert(
            "patch".to_string(),
            ShortwireNodePatch {
                hunks: vec![test_hunk()],
                base_source_hash: 1,
                reference_image: None,
                diff_result: Some(test_diff_result(0.0)),
            },
        );

        state.apply_rebase(MergePatchRebase {
            patch_key: "patch".to_string(),
            incoming_source: "incoming".to_string(),
            next_hunks: vec![ShortwireHunk {
                new_lines: vec!["rebased".to_string()],
                ..test_hunk()
            }],
        });

        assert!(state.patches_dirty);
        assert_eq!(
            state.patches["patch"].base_source_hash,
            hash_source("incoming")
        );
        assert!(state.patches["patch"].diff_result.is_none());

        state.apply_rebase(MergePatchRebase {
            patch_key: "patch".to_string(),
            incoming_source: "incoming".to_string(),
            next_hunks: vec![],
        });

        assert!(!state.patches.contains_key("patch"));
    }

    #[test]
    fn store_close_patch_preserves_matching_diff_result_and_remove_marks_dirty() {
        let mut state = ShortwireState::default();
        let diff = test_diff_result(0.0);
        let hunks = vec![test_hunk()];
        state.patches.insert(
            "patch".to_string(),
            ShortwireNodePatch {
                hunks: hunks.clone(),
                base_source_hash: 12,
                reference_image: None,
                diff_result: Some(diff.clone()),
            },
        );
        state.patches_dirty = false;

        let stored = state.store_close_patch("patch".to_string(), hunks, 12, None);

        assert_eq!(
            stored,
            ShortwireStoredPatchSummary {
                hunk_count: 1,
                base_source_hash: 12,
                preserved_diff_result: true,
            }
        );
        assert_eq!(state.patches["patch"].diff_result, Some(diff));
        assert!(state.patches_dirty);

        state.patches_dirty = false;
        state.remove_patch("patch");

        assert!(!state.patches.contains_key("patch"));
        assert!(state.patches_dirty);
    }

    #[test]
    fn diff_capture_patch_methods_update_patch_store() {
        let mut state = ShortwireState::default();

        assert!(state.create_image_patch_if_missing(
            "patch".to_string(),
            vec![test_hunk()],
            44,
            Some(ShortwireReferenceImage {
                artifact_id: "image".to_string(),
                name: "Image".to_string(),
                width: 1,
                height: 1,
                alpha_mode: Default::default(),
                mode: Default::default(),
                opacity: 1.0,
                offset: [0.0, 0.0],
            }),
        ));
        assert_eq!(state.patches["patch"].base_source_hash, 44);
        assert!(state.patches["patch"].reference_image.is_some());

        assert!(state.record_diff_result("patch", test_diff_result(0.0)));
        assert!(state.patches["patch"].diff_result.is_some());
        assert!(state.prepare_diff_capture_patch("patch", None));
        assert!(state.patches["patch"].diff_result.is_none());

        assert!(!state.record_diff_result("missing", test_diff_result(0.0)));
        assert!(!state.prepare_diff_capture_patch("missing", None));
    }

    #[test]
    fn active_exit_flag_helpers_update_session_state() {
        let mut state = ShortwireState {
            active: Some(pending_active("patch")),
            exit_on_apply: true,
            ..Default::default()
        };

        state.clear_exit_on_apply();
        assert!(!state.exit_on_apply);

        state.request_exit_on_apply_and_clear_active();
        assert!(state.exit_on_apply);
        assert!(state.active.is_none());

        state.active = Some(pending_active("patch"));
        state.clear_active();
        assert!(state.active.is_none());
    }

    #[test]
    fn start_session_helpers_create_expected_active_phases() {
        let mut state = ShortwireState::default();
        let identity = ShortwireRowIdentity {
            patch_key: "patch".to_string(),
            row_key_hint: "row".to_string(),
            label: "Row".to_string(),
            target_id: None,
        };

        state.start_pending_reset_then_enter(identity.clone(), None);
        assert!(matches!(
            state.active.as_ref().map(|active| &active.phase),
            Some(ShortwirePhase::PendingResetThenEnter { .. })
        ));

        state.start_editing(identity.clone(), "base".to_string(), 10, None);
        let active = state.active.as_ref().expect("active editing session");
        assert!(matches!(active.phase, ShortwirePhase::Editing));
        assert_eq!(active.base_source, "base");
        assert_eq!(active.base_source_hash, 10);

        state.start_pending_apply(identity, "base".to_string(), 11, None, vec![test_hunk()]);
        let active = state.active.as_ref().expect("active pending apply session");
        assert!(matches!(active.phase, ShortwirePhase::PendingApply { .. }));
        assert_eq!(active.base_source_hash, 11);
    }
}
