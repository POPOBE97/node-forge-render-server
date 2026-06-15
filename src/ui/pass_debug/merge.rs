use crate::ui::pass_debug::patch::{
    ShortwireHunk, apply_hunks, compute_hunks, three_way_merge_sources,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct MergeState {
    pub(crate) conflict: Option<PassDebugMergeConflict>,
    pub(crate) pending_patch_update: Option<PassDebugPendingMergePatchUpdate>,
}

#[derive(Clone, Debug)]
pub(crate) struct PassDebugMergeConflict {
    pub(crate) base_source: String,
    pub(crate) incoming_source: String,
    pub(crate) local_source: String,
    pub(crate) resolved_source: String,
    pub(crate) error: String,
    pub(crate) choice_popup_open: bool,
    pub(crate) resolver_window_open: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PassDebugPendingMergePatchUpdate {
    base_source: String,
    incoming_source: String,
    local_source: String,
    merged_source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MergePatchRequest {
    Apply { source: String, status: String },
    Reset { status: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergeCancelResult {
    pub(crate) restored_source: String,
    pub(crate) status: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MergePatchRebase {
    pub(crate) patch_key: String,
    pub(crate) incoming_source: String,
    pub(crate) next_hunks: Vec<ShortwireHunk>,
}

impl MergeState {
    pub(crate) fn handle_canonical_patch_change(
        &mut self,
        base_source: String,
        incoming_source: &str,
        local_source: &str,
    ) -> MergeCanonicalChangeResult {
        match three_way_merge_sources(&base_source, incoming_source, local_source) {
            Ok(merged_source) => {
                self.conflict = None;
                self.pending_patch_update = Some(PassDebugPendingMergePatchUpdate {
                    base_source,
                    incoming_source: incoming_source.to_string(),
                    local_source: local_source.to_string(),
                    merged_source: merged_source.clone(),
                });
                if merged_source == incoming_source {
                    MergeCanonicalChangeResult::Request(MergePatchRequest::Reset {
                        status: "Canonical source changed — clearing empty patch".to_string(),
                    })
                } else {
                    MergeCanonicalChangeResult::Request(MergePatchRequest::Apply {
                        source: merged_source,
                        status: "Canonical source changed — rebasing patch...".to_string(),
                    })
                }
            }
            Err(error) => {
                self.conflict = Some(PassDebugMergeConflict {
                    base_source,
                    incoming_source: incoming_source.to_string(),
                    local_source: local_source.to_string(),
                    resolved_source: local_source.to_string(),
                    error: error.to_string(),
                    choice_popup_open: true,
                    resolver_window_open: false,
                });
                MergeCanonicalChangeResult::Conflict {
                    local_source: local_source.to_string(),
                    status: "Patch conflicts with updated generated shader — resolve merge"
                        .to_string(),
                }
            }
        }
    }

    pub(crate) fn take_rebase_for_applied_source(
        &mut self,
        applied_source: &str,
        patch_hunks: impl IntoIterator<Item = (String, Vec<ShortwireHunk>)>,
    ) -> Option<MergePatchRebase> {
        let pending = self.pending_patch_update.take()?;
        if pending.merged_source != applied_source {
            return None;
        }

        let mut matching_keys = patch_hunks
            .into_iter()
            .filter_map(|(key, hunks)| {
                apply_hunks(&pending.base_source, &hunks)
                    .ok()
                    .filter(|patched| patched == &pending.local_source)
                    .map(|_| key)
            })
            .collect::<Vec<_>>();
        matching_keys.sort();
        matching_keys.dedup();

        if matching_keys.len() != 1 {
            return None;
        }

        Some(MergePatchRebase {
            patch_key: matching_keys.remove(0),
            incoming_source: pending.incoming_source.clone(),
            next_hunks: compute_hunks(&pending.incoming_source, applied_source),
        })
    }

    pub(crate) fn clear_conflict(&mut self) {
        self.conflict = None;
    }

    pub(crate) fn open_resolver(&mut self) {
        if let Some(conflict) = self.conflict.as_mut() {
            conflict.choice_popup_open = false;
            conflict.resolver_window_open = true;
        }
    }

    pub(crate) fn reopen_choice_popup(&mut self) {
        if let Some(conflict) = self.conflict.as_mut() {
            conflict.resolver_window_open = false;
            conflict.choice_popup_open = true;
        }
    }

    pub(crate) fn close_conflict_windows(&mut self) {
        if let Some(conflict) = self.conflict.as_mut() {
            conflict.choice_popup_open = false;
            conflict.resolver_window_open = false;
        }
    }

    pub(crate) fn apply_resolved(&mut self) -> Option<MergePatchRequest> {
        let conflict = self.conflict.as_ref()?;
        self.pending_patch_update = Some(PassDebugPendingMergePatchUpdate {
            base_source: conflict.base_source.clone(),
            incoming_source: conflict.incoming_source.clone(),
            local_source: conflict.local_source.clone(),
            merged_source: conflict.resolved_source.clone(),
        });
        Some(MergePatchRequest::Apply {
            source: conflict.resolved_source.clone(),
            status: "Applying resolved shader...".to_string(),
        })
    }

    pub(crate) fn use_incoming(&mut self) -> Option<MergePatchRequest> {
        let conflict = self.conflict.as_ref()?;
        self.pending_patch_update = Some(PassDebugPendingMergePatchUpdate {
            base_source: conflict.base_source.clone(),
            incoming_source: conflict.incoming_source.clone(),
            local_source: conflict.local_source.clone(),
            merged_source: conflict.incoming_source.clone(),
        });
        self.close_conflict_windows();
        Some(MergePatchRequest::Reset {
            status: "Using incoming generated shader...".to_string(),
        })
    }

    pub(crate) fn keep_local(&mut self) -> Option<MergePatchRequest> {
        let conflict = self.conflict.as_ref()?;
        self.pending_patch_update = Some(PassDebugPendingMergePatchUpdate {
            base_source: conflict.base_source.clone(),
            incoming_source: conflict.incoming_source.clone(),
            local_source: conflict.local_source.clone(),
            merged_source: conflict.local_source.clone(),
        });
        Some(MergePatchRequest::Apply {
            source: conflict.local_source.clone(),
            status: "Keeping local patch...".to_string(),
        })
    }

    pub(crate) fn cancel_resolution(&mut self) -> Option<MergeCancelResult> {
        let conflict = self.conflict.take()?;
        self.pending_patch_update = None;
        Some(MergeCancelResult {
            restored_source: conflict.local_source,
            status: "Merge resolution cancelled".to_string(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MergeCanonicalChangeResult {
    Request(MergePatchRequest),
    Conflict {
        local_source: String,
        status: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_canonical_change_requests_apply_and_rebases_matching_patch() {
        let base = "fn a() {\n    let x = 1;\n    let y = 2;\n}\n";
        let local = "fn a() {\n    let x = 99;\n    let y = 2;\n}\n";
        let incoming = "fn a() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}\n";
        let expected = "fn a() {\n    let x = 99;\n    let y = 2;\n    let z = 3;\n}\n";
        let mut state = MergeState::default();

        let result = state.handle_canonical_patch_change(base.to_string(), incoming, local);

        assert_eq!(
            result,
            MergeCanonicalChangeResult::Request(MergePatchRequest::Apply {
                source: expected.to_string(),
                status: "Canonical source changed — rebasing patch...".to_string(),
            })
        );

        let rebase = state
            .take_rebase_for_applied_source(
                expected,
                vec![("patch-key".to_string(), compute_hunks(base, local))],
            )
            .expect("expected matching patch rebase");
        assert_eq!(rebase.patch_key, "patch-key");
        assert_eq!(rebase.incoming_source, incoming);
        assert_eq!(apply_hunks(incoming, &rebase.next_hunks).unwrap(), expected);
    }

    #[test]
    fn conflicting_canonical_change_opens_resolver_and_cancel_restores_local() {
        let base = "fn a() {\n    let x = 1;\n}\n";
        let local = "fn a() {\n    let x = 99;\n}\n";
        let incoming = "fn a() {\n    let x = 2;\n}\n";
        let mut state = MergeState::default();

        let result = state.handle_canonical_patch_change(base.to_string(), incoming, local);

        assert_eq!(
            result,
            MergeCanonicalChangeResult::Conflict {
                local_source: local.to_string(),
                status: "Patch conflicts with updated generated shader — resolve merge".to_string(),
            }
        );
        let conflict = state.conflict.as_ref().expect("merge conflict");
        assert!(conflict.choice_popup_open);
        assert!(!conflict.resolver_window_open);

        state.open_resolver();
        let conflict = state.conflict.as_ref().expect("merge conflict");
        assert!(!conflict.choice_popup_open);
        assert!(conflict.resolver_window_open);

        let cancel = state.cancel_resolution().expect("cancel result");
        assert_eq!(cancel.restored_source, local);
        assert!(state.conflict.is_none());
        assert!(state.pending_patch_update.is_none());
    }
}
