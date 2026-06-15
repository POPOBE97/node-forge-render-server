use crate::ui::pass_debug::dependency_tree::{DependencyTreeState, DependencyTreeStateChange};
use crate::ui::pass_debug::event::{PassDebugEffect, PassDebugEvent};
use crate::ui::pass_debug::merge::MergeState;
use crate::ui::pass_debug::reference_workspace::ReferenceWorkspaceState;
use crate::ui::pass_debug::shader_document::ShaderDocumentState;
use crate::ui::pass_debug::shortwire::ShortwireState;

#[derive(Clone, Debug)]
pub(crate) struct PassDebugStore {
    pub(crate) shader: ShaderDocumentState,
    pub(crate) dependencies: DependencyTreeState,
    pub(crate) reference_workspace: ReferenceWorkspaceState,
    pub(crate) shortwire: ShortwireState,
    pub(crate) merge: MergeState,
    outbox: Vec<PassDebugEffect>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PassDebugStoreChange {
    pub(crate) draft_source_changed: bool,
    pub(crate) reference_editor_changed: bool,
    pub(crate) reference_selection_changed: bool,
    pub(crate) dependency_rows_changed: bool,
    pub(crate) dependency_visibility_changed: bool,
}

impl PassDebugStoreChange {
    fn merge_dependency_change(&mut self, change: DependencyTreeStateChange) {
        self.dependency_rows_changed |= change.rows_changed;
        self.dependency_visibility_changed |= change.visibility_changed;
    }
}

impl PassDebugStore {
    pub(crate) fn new(shader: ShaderDocumentState) -> Self {
        Self {
            shader,
            dependencies: DependencyTreeState::default(),
            reference_workspace: ReferenceWorkspaceState::default(),
            shortwire: ShortwireState::default(),
            merge: MergeState::default(),
            outbox: Vec::new(),
        }
    }

    pub(crate) fn dispatch(&mut self, event: PassDebugEvent) -> PassDebugStoreChange {
        let mut change = PassDebugStoreChange::default();
        match event {
            PassDebugEvent::Tick { .. } => {}
            PassDebugEvent::EmitEffect(effect) => self.emit_effect(effect),
            PassDebugEvent::ToggleShortwireDiff => {
                if let Some(active) = self.shortwire.active.as_mut() {
                    active.diff_view_enabled = !active.diff_view_enabled;
                }
            }
            PassDebugEvent::ReferenceFileSelected {
                relative_path,
                now_secs,
            } => {
                if self.reference_workspace.select_file(&relative_path) {
                    self.reference_workspace.sync_due_secs = Some(now_secs);
                    change.reference_selection_changed = true;
                }
            }
            PassDebugEvent::ShaderDraftReplaced { source, .. } => {
                if self.shader.draft_source != source {
                    self.shader.draft_source = source;
                    change.draft_source_changed = true;
                }
            }
            PassDebugEvent::ReferenceEditorReplaced { source, .. } => {
                if self.reference_workspace.editor_source != source {
                    self.reference_workspace.editor_source = source;
                    change.reference_editor_changed = true;
                }
            }
            PassDebugEvent::DependencyFilterEdited { text } => {
                if self.dependencies.filter_text != text {
                    self.dependencies.filter_text = text;
                    change.dependency_visibility_changed = true;
                }
            }
            PassDebugEvent::ShaderEditorClicked { char_index } => {
                change.merge_dependency_change(self.dependencies.focus_target_at_char_index(
                    self.shader.analysis_source.as_ref(),
                    &self.shader.draft_source,
                    char_index,
                ));
            }
            PassDebugEvent::DependencyTreeClicked { click } => {
                change.merge_dependency_change(
                    self.dependencies
                        .focus_tree_click(self.shader.analysis_source.as_ref(), click),
                );
            }
            PassDebugEvent::MergeCloseConflictWindows => self.merge.close_conflict_windows(),
            PassDebugEvent::MergeReopenChoicePopup => self.merge.reopen_choice_popup(),
            PassDebugEvent::MergeResolvedEdited { source } => {
                if let Some(conflict) = self.merge.conflict.as_mut() {
                    conflict.resolved_source = source;
                }
            }
            _ => {}
        }
        change
    }

    pub(crate) fn emit_effect(&mut self, effect: PassDebugEffect) {
        self.outbox.push(effect);
    }

    pub(crate) fn drain_effects(&mut self) -> Vec<PassDebugEffect> {
        self.outbox.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::PassDebugSource;

    #[test]
    fn dispatch_emit_effect_drains_outbox_once() {
        let mut store = PassDebugStore::new(ShaderDocumentState::new(None, 0, None));

        store.dispatch(PassDebugEvent::EmitEffect(PassDebugEffect::ResetPatch {
            pass_name: "main".to_string(),
        }));

        let effects = store.drain_effects();
        assert_eq!(effects.len(), 1);
        assert!(store.drain_effects().is_empty());
    }

    #[test]
    fn dispatch_shader_draft_replaced_reports_change_once() {
        let mut store = PassDebugStore::new(ShaderDocumentState::new(None, 0, None));

        let change = store.dispatch(PassDebugEvent::ShaderDraftReplaced {
            source: "fn edited() {}\n".to_string(),
            now_secs: 1.0,
        });
        assert!(change.draft_source_changed);
        assert_eq!(store.shader.draft_source, "fn edited() {}\n");

        let change = store.dispatch(PassDebugEvent::ShaderDraftReplaced {
            source: "fn edited() {}\n".to_string(),
            now_secs: 2.0,
        });
        assert!(!change.draft_source_changed);
    }

    #[test]
    fn dispatch_reference_editor_replaced_reports_change_once() {
        let mut store = PassDebugStore::new(ShaderDocumentState::new(None, 0, None));

        let change = store.dispatch(PassDebugEvent::ReferenceEditorReplaced {
            source: "reference text\n".to_string(),
            now_secs: 1.0,
        });
        assert!(change.reference_editor_changed);
        assert_eq!(store.reference_workspace.editor_source, "reference text\n");

        let change = store.dispatch(PassDebugEvent::ReferenceEditorReplaced {
            source: "reference text\n".to_string(),
            now_secs: 2.0,
        });
        assert!(!change.reference_editor_changed);
    }

    #[test]
    fn dispatch_dependency_filter_reports_visibility_change() {
        let mut store = PassDebugStore::new(ShaderDocumentState::new(None, 0, None));

        let change = store.dispatch(PassDebugEvent::DependencyFilterEdited {
            text: "alpha".to_string(),
        });
        assert!(change.dependency_visibility_changed);
        assert_eq!(store.dependencies.filter_text, "alpha");

        let change = store.dispatch(PassDebugEvent::DependencyFilterEdited {
            text: "alpha".to_string(),
        });
        assert!(!change.dependency_visibility_changed);
    }

    #[test]
    fn dispatch_shader_editor_clicked_focuses_dependency_row() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) f32 { var a: f32 = 1.0; let b = a + 1.0; return b; }\n",
        );
        let mut store = PassDebugStore::new(ShaderDocumentState::new(Some(source), 0, None));
        store
            .dependencies
            .refresh_from_source(store.shader.analysis_source.as_ref());
        let char_index = store
            .shader
            .draft_source
            .find("a + 1.0")
            .expect("reference occurrence exists");

        let _change = store.dispatch(PassDebugEvent::ShaderEditorClicked { char_index });

        let focused_row = store
            .dependencies
            .focused_row_key
            .as_deref()
            .and_then(|row_key| {
                store
                    .dependencies
                    .rows
                    .iter()
                    .find(|row| row.row_key == row_key)
            })
            .expect("focused dependency row");
        assert!(focused_row.label.contains("a"));
    }
}
