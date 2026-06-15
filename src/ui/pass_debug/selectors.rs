use std::collections::HashMap;

use crate::renderer::PassDebugSourceRange;
use crate::ui::pass_debug::dependency_tree::PassDebugDependencyRow;
use crate::ui::pass_debug::document::PassDebugWindowDocument;
use crate::ui::pass_debug::shortwire::{ShortwireDotInfo, shortwire_dot_info_for_patch};

pub(crate) const PASS_DEBUG_PATCH_ERROR_SUMMARY_CHARS: usize = 140;

pub(crate) struct PassDebugRootView {
    pub(crate) source_available: bool,
}

pub(crate) struct PassDebugTitlebarView {
    pub(crate) pass_name: String,
    pub(crate) diff_enabled: bool,
    pub(crate) diff_active: bool,
    pub(crate) save_enabled: bool,
}

pub(crate) struct PassDebugReferenceSelectorView {
    pub(crate) shortwire_active: bool,
    pub(crate) selected_label: String,
    pub(crate) selected_file: Option<String>,
    pub(crate) file_choices: Vec<String>,
    pub(crate) can_reload: bool,
    pub(crate) can_open: bool,
}

pub(crate) struct PassDebugHeadersView {
    pub(crate) pass_name: String,
    pub(crate) shader_status: String,
    pub(crate) reference_status: String,
    pub(crate) reference_selector: PassDebugReferenceSelectorView,
}

pub(crate) struct PassDebugDiffEditorView {
    pub(crate) base_source: String,
    pub(crate) current_source: String,
}

pub(crate) struct PassDebugShaderEditorView {
    pub(crate) pass_name: String,
    pub(crate) draft_source: String,
    pub(crate) source_len: usize,
    pub(crate) diff: Option<PassDebugDiffEditorView>,
    pub(crate) editor_interactive: bool,
    pub(crate) shortwire_active: bool,
    pub(crate) focused_source_range: Option<PassDebugSourceRange>,
}

pub(crate) enum PassDebugReferenceEditorView {
    Empty,
    Diff(PassDebugDiffEditorView),
    Editor {
        pass_name: String,
        editor_source: String,
    },
}

pub(crate) enum PassDebugDependencyPanelStatus {
    MissingSource,
    ParseError(String),
    DependencyError(String),
    Ready,
    Empty,
}

pub(crate) struct PassDebugDependencyRowsView {
    pub(crate) pass_name: String,
    pub(crate) rows: Vec<PassDebugDependencyRow>,
    pub(crate) filter_text: String,
    pub(crate) focus_is_in_dependency_root: bool,
    pub(crate) focused_target_id: Option<String>,
    pub(crate) focused_row_key: Option<String>,
    pub(crate) expanded_row_keys: std::collections::HashSet<String>,
    pub(crate) shortwire_active_row_key: Option<String>,
    pub(crate) shortwire_can_enter: bool,
    pub(crate) shortwire_dot_info: HashMap<String, ShortwireDotInfo>,
}

pub(crate) struct PassDebugMergePopupView {
    pub(crate) pass_name: String,
    pub(crate) choice_popup_open: bool,
    pub(crate) resolver_window_open: bool,
}

pub(crate) struct PassDebugMergeResolverView {
    pub(crate) pass_name: String,
    pub(crate) base_source: String,
    pub(crate) incoming_source: String,
    pub(crate) local_source: String,
    pub(crate) resolved_source: String,
    pub(crate) conflict_error: String,
}

pub(crate) fn root_view(document: &PassDebugWindowDocument) -> PassDebugRootView {
    PassDebugRootView {
        source_available: document.store.shader.source.is_some(),
    }
}

pub(crate) fn titlebar_view(document: &PassDebugWindowDocument) -> PassDebugTitlebarView {
    let active = document.store.shortwire.active.as_ref();
    PassDebugTitlebarView {
        pass_name: document.pass_name.clone(),
        diff_enabled: active.is_some(),
        diff_active: active.is_some_and(|active| active.diff_view_enabled),
        save_enabled: document.save_enabled(),
    }
}

pub(crate) fn headers_view(document: &PassDebugWindowDocument) -> PassDebugHeadersView {
    let reference_workspace = &document.store.reference_workspace;
    let reference_shortwire_active = reference_workspace.shortwire_active_key.is_some();
    PassDebugHeadersView {
        pass_name: document.pass_name.clone(),
        shader_status: pass_shader_status(document),
        reference_status: reference_status(document),
        reference_selector: PassDebugReferenceSelectorView {
            shortwire_active: reference_shortwire_active,
            selected_label: reference_workspace
                .selected_file
                .as_deref()
                .unwrap_or("No file")
                .to_string(),
            selected_file: reference_workspace.selected_file.clone(),
            file_choices: reference_workspace
                .files
                .iter()
                .map(|file| file.relative_path.clone())
                .collect(),
            can_reload: !reference_shortwire_active && reference_workspace.root_path.is_some(),
            can_open: !reference_shortwire_active,
        },
    }
}

pub(crate) fn shader_editor_view(document: &PassDebugWindowDocument) -> PassDebugShaderEditorView {
    let draft_source = document.store.shader.draft_source.clone();
    let diff = document
        .store
        .shortwire
        .active
        .as_ref()
        .filter(|active| active.diff_view_enabled)
        .map(|active| PassDebugDiffEditorView {
            base_source: active.base_source.clone(),
            current_source: draft_source.clone(),
        });
    let shortwire_active = document.store.shortwire.active.is_some();
    PassDebugShaderEditorView {
        pass_name: document.pass_name.clone(),
        source_len: draft_source.len(),
        draft_source,
        diff,
        editor_interactive: (document.shortwire_is_editor_interactive() || !shortwire_active)
            && document.store.merge.conflict.is_none(),
        shortwire_active,
        focused_source_range: document.focused_source_range(),
    }
}

pub(crate) fn reference_editor_view(
    document: &PassDebugWindowDocument,
) -> PassDebugReferenceEditorView {
    if document.store.reference_workspace.selected_file().is_none() {
        return PassDebugReferenceEditorView::Empty;
    }

    if let Some(base_source) = document
        .store
        .shortwire
        .active
        .as_ref()
        .filter(|active| active.diff_view_enabled)
        .and_then(|_| {
            document
                .store
                .reference_workspace
                .shortwire_base_source
                .clone()
        })
    {
        return PassDebugReferenceEditorView::Diff(PassDebugDiffEditorView {
            base_source,
            current_source: document.store.reference_workspace.editor_source.clone(),
        });
    }

    PassDebugReferenceEditorView::Editor {
        pass_name: document.pass_name.clone(),
        editor_source: document.store.reference_workspace.editor_source.clone(),
    }
}

pub(crate) fn dependency_panel_status(
    document: &PassDebugWindowDocument,
) -> PassDebugDependencyPanelStatus {
    let Some(source) = document.store.shader.analysis_source.as_ref() else {
        return PassDebugDependencyPanelStatus::MissingSource;
    };
    if let Some(error) = source.parse_error.as_ref() {
        return PassDebugDependencyPanelStatus::ParseError(error.clone());
    }
    if let Some(error) = source.dependency_error.as_ref() {
        return PassDebugDependencyPanelStatus::DependencyError(error.clone());
    }
    if document.store.dependencies.rows.is_empty() {
        PassDebugDependencyPanelStatus::Empty
    } else {
        PassDebugDependencyPanelStatus::Ready
    }
}

pub(crate) fn dependency_rows_view(
    document: &PassDebugWindowDocument,
) -> PassDebugDependencyRowsView {
    PassDebugDependencyRowsView {
        pass_name: document.pass_name.clone(),
        rows: document.store.dependencies.rows.clone(),
        filter_text: document.store.dependencies.filter_text.clone(),
        focus_is_in_dependency_root: document.focus_is_in_dependency_root(),
        focused_target_id: document.store.dependencies.focused_target_id.clone(),
        focused_row_key: document.store.dependencies.focused_row_key.clone(),
        expanded_row_keys: document.store.dependencies.expanded_row_keys.clone(),
        shortwire_active_row_key: document
            .store
            .shortwire
            .active
            .as_ref()
            .map(|active| active.identity.row_key_hint.clone()),
        shortwire_can_enter: document.store.shortwire.active.is_none()
            && document.store.merge.conflict.is_none()
            && !document.store.shader.generated_base_source.is_empty(),
        shortwire_dot_info: document
            .store
            .shortwire
            .patches
            .iter()
            .map(|(key, patch)| (key.clone(), shortwire_dot_info_for_patch(patch)))
            .collect(),
    }
}

pub(crate) fn merge_popup_view(document: &PassDebugWindowDocument) -> PassDebugMergePopupView {
    let conflict = document.store.merge.conflict.as_ref();
    PassDebugMergePopupView {
        pass_name: document.pass_name.clone(),
        choice_popup_open: conflict
            .map(|conflict| conflict.choice_popup_open)
            .unwrap_or(false),
        resolver_window_open: conflict
            .map(|conflict| conflict.resolver_window_open)
            .unwrap_or(false),
    }
}

pub(crate) fn merge_resolver_view(
    document: &PassDebugWindowDocument,
) -> Option<PassDebugMergeResolverView> {
    let conflict = document.store.merge.conflict.as_ref()?;
    Some(PassDebugMergeResolverView {
        pass_name: document.pass_name.clone(),
        base_source: conflict.base_source.clone(),
        incoming_source: conflict.incoming_source.clone(),
        local_source: conflict.local_source.clone(),
        resolved_source: conflict.resolved_source.clone(),
        conflict_error: conflict.error.clone(),
    })
}

pub(crate) fn compact_patch_error(error: &str) -> String {
    let first_line = error
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Unknown patch error");
    let compact = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= PASS_DEBUG_PATCH_ERROR_SUMMARY_CHARS {
        return compact;
    }

    let keep = PASS_DEBUG_PATCH_ERROR_SUMMARY_CHARS.saturating_sub(3);
    let mut out = compact.chars().take(keep).collect::<String>();
    out.push_str("...");
    out
}

fn pass_shader_status(document: &PassDebugWindowDocument) -> String {
    if let Some(error) = document.store.shader.last_error.as_ref() {
        return format!("Patch failed: {}", compact_patch_error(error));
    }
    if let Some(status) = document.store.shader.last_status.as_ref() {
        return status.clone();
    }
    if let Some(active) = document.store.shortwire.active.as_ref() {
        if active.base_source_stale {
            "Shortwire stale".to_string()
        } else {
            "Shortwire".to_string()
        }
    } else if document.store.shader.dirty {
        "Dirty".to_string()
    } else if document.store.merge.conflict.is_some() {
        "Conflict".to_string()
    } else if document.store.shader.patch_active {
        "Patched".to_string()
    } else {
        "Generated".to_string()
    }
}

fn reference_status(document: &PassDebugWindowDocument) -> String {
    let reference_workspace = &document.store.reference_workspace;
    if let Some(status) = reference_workspace.last_status.as_deref() {
        return status.to_string();
    }
    if reference_workspace.shortwire_active_key.is_some() {
        "Shortwire".to_string()
    } else if reference_workspace.selected_file_dirty()
        || reference_workspace.has_dirty_files()
        || reference_workspace.manifest_dirty
    {
        "Syncing".to_string()
    } else if !reference_workspace.has_content() {
        "Empty".to_string()
    } else if reference_workspace.skipped_files > 0 {
        format!("Saved, {} skipped", reference_workspace.skipped_files)
    } else {
        "Saved".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_error_summary_stays_single_line() {
        let error = "\n\nerror: shader failed to compile because a very long generated WGSL line could not be parsed and would otherwise cover the editor with many details about bindings, functions, expressions, and source spans\n  --> generated.wgsl:12:5\n  |\n";
        let summary = compact_patch_error(error);

        assert!(!summary.contains('\n'));
        assert!(summary.ends_with("..."));
        assert!(summary.chars().count() <= PASS_DEBUG_PATCH_ERROR_SUMMARY_CHARS);
    }
}
