use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use rust_wgpu_fiber::eframe::egui;

use crate::renderer::{PassDebugDependencyNode, PassDebugSource, PassDebugSourceRange};

const SIDE_PANEL_DEFAULT_WIDTH: f32 = 340.0;
const SIDE_PANEL_MIN_WIDTH: f32 = 220.0;
const SIDE_PANEL_MAX_WIDTH: f32 = 560.0;
const TREE_ROW_INDENT_WIDTH: f32 = 14.0;
const PASS_DEBUG_SPLIT_HANDLE_WIDTH: f32 = 6.0;
const PASS_DEBUG_SPLIT_LINE_WIDTH: f32 = 1.0;
const PASS_DEBUG_EDITOR_MIN_WIDTH: f32 = 260.0;
const TREE_ROW_TRAILING_PADDING: f32 = 24.0;
const TREE_ROW_SOURCE_JUMP_GAP: f32 = 8.0;
const TREE_ROW_SOURCE_JUMP_LABEL: &str = "src";
const TREE_ROW_SOURCE_JUMP_HORIZONTAL_PADDING: f32 = 5.0;
const TREE_ROW_SOURCE_JUMP_VERTICAL_PADDING: f32 = 2.0;
const PASS_DEBUG_CLOSE_RESIZE_DELTA_THRESHOLD: f32 = 48.0;
const PASS_DEBUG_TREE_FONT_SIZE: f32 = 13.0;
const PASS_DEBUG_CODE_FONT_SIZE: f32 = 14.0;

#[derive(Clone, Debug)]
pub enum PassDebugWindowAction {
    ApplyPatch { pass_name: String, source: String },
    ResetPatch { pass_name: String },
    ResetAllPatches,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PassDebugDependencyRow {
    depth: usize,
    row_key: String,
    parent_row_key: Option<String>,
    label: String,
    relation_path: String,
    target_id: Option<String>,
    source_range: Option<PassDebugSourceRange>,
    source_jump_range: Option<PassDebugSourceRange>,
    selectable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PassDebugTreeClick {
    row_key: Option<String>,
    target_id: Option<String>,
    source_range: Option<PassDebugSourceRange>,
    toggle_row_key: Option<String>,
}

trait PassDebugTreeRow {
    fn depth(&self) -> usize;
    fn row_key(&self) -> Option<&str>;
    fn label(&self) -> &str;
    fn relation_path(&self) -> Option<&str>;
    fn target_id(&self) -> Option<&str>;
    fn source_range(&self) -> Option<PassDebugSourceRange>;
    fn source_jump_range(&self) -> Option<PassDebugSourceRange>;
    fn selectable(&self) -> bool;
}

impl PassDebugTreeRow for PassDebugDependencyRow {
    fn depth(&self) -> usize {
        self.depth
    }

    fn row_key(&self) -> Option<&str> {
        Some(&self.row_key)
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn relation_path(&self) -> Option<&str> {
        if self.relation_path.is_empty() {
            None
        } else {
            Some(&self.relation_path)
        }
    }

    fn target_id(&self) -> Option<&str> {
        self.target_id.as_deref()
    }

    fn source_range(&self) -> Option<PassDebugSourceRange> {
        self.source_range
    }

    fn source_jump_range(&self) -> Option<PassDebugSourceRange> {
        self.source_jump_range
    }

    fn selectable(&self) -> bool {
        self.selectable
    }
}

#[derive(Clone, Debug)]
pub struct PassDebugWindowDocument {
    pub pass_name: String,
    pub source: Option<PassDebugSource>,
    analysis_source: Option<PassDebugSource>,
    analysis_source_text: String,
    source_revision: Option<u64>,
    dependency_rows: Vec<PassDebugDependencyRow>,
    focused_target_id: Option<String>,
    focused_dependency_row_key: Option<String>,
    dependency_root_target_id: Option<String>,
    dependency_expanded_row_keys: HashSet<String>,
    pending_editor_jump: Option<PassDebugSourceRange>,
    pending_dependency_reveal_row_key: Option<String>,
    pub draft_source: String,
    loaded_source: String,
    dirty: bool,
    patch_active: bool,
    last_error: Option<String>,
    last_status: Option<String>,
}

impl PassDebugWindowDocument {
    fn new(
        pass_name: String,
        source: Option<PassDebugSource>,
        source_revision: u64,
        patch_active: bool,
    ) -> Self {
        let loaded_source = source
            .as_ref()
            .map(|s| s.module_source.clone())
            .unwrap_or_default();
        let analysis_source = source.clone();
        let mut document = Self {
            pass_name,
            source,
            analysis_source,
            analysis_source_text: loaded_source.clone(),
            source_revision: Some(source_revision),
            dependency_rows: Vec::new(),
            focused_target_id: None,
            focused_dependency_row_key: None,
            dependency_root_target_id: None,
            dependency_expanded_row_keys: HashSet::new(),
            pending_editor_jump: None,
            pending_dependency_reveal_row_key: None,
            draft_source: loaded_source.clone(),
            loaded_source,
            dirty: false,
            patch_active,
            last_error: None,
            last_status: None,
        };
        document.refresh_analysis_rows();
        document
    }

    fn update_source(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        patch_active: bool,
    ) {
        self.patch_active = patch_active;
        if self.source_revision == Some(source_revision) {
            return;
        }

        self.source_revision = Some(source_revision);
        self.source = source.cloned();

        if !self.dirty {
            let Some(next_source_text) = source.map(|s| s.module_source.clone()) else {
                self.loaded_source.clear();
                self.draft_source.clear();
                self.analysis_source = None;
                self.analysis_source_text.clear();
                self.refresh_analysis_rows();
                self.last_error = None;
                return;
            };
            self.loaded_source = next_source_text.clone();
            self.draft_source = next_source_text.clone();
            self.analysis_source = source.cloned();
            self.analysis_source_text = next_source_text;
            self.refresh_analysis_rows();
            self.last_error = None;
        } else {
            self.refresh_analysis_rows();
        }
    }

    fn mark_applied(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        draft_source: String,
        status: String,
    ) {
        self.source_revision = Some(source_revision);
        self.source = source.cloned();
        self.loaded_source = draft_source.clone();
        self.draft_source = draft_source;
        self.analysis_source = source.cloned();
        self.analysis_source_text = self.draft_source.clone();
        self.refresh_analysis_rows();
        self.dirty = false;
        self.patch_active = true;
        self.last_error = None;
        self.last_status = Some(status);
    }

    fn mark_reset(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        status: String,
    ) {
        self.source_revision = Some(source_revision);
        self.source = source.cloned();
        if let Some(source) = source {
            self.loaded_source = source.module_source.clone();
            self.draft_source = source.module_source.clone();
            self.analysis_source = Some(source.clone());
            self.analysis_source_text = self.draft_source.clone();
        } else {
            self.analysis_source = None;
            self.analysis_source_text.clear();
        }
        self.refresh_analysis_rows();
        self.dirty = false;
        self.patch_active = false;
        self.last_error = None;
        self.last_status = Some(status);
    }

    fn refresh_draft_analysis(&mut self) {
        if self.analysis_source_text == self.draft_source {
            return;
        }
        self.analysis_source = Some(PassDebugSource::from_wgsl(
            self.pass_name.clone(),
            self.draft_source.clone(),
        ));
        self.analysis_source_text = self.draft_source.clone();
        self.refresh_analysis_rows();
    }

    fn refresh_analysis_rows(&mut self) {
        self.ensure_navigation_targets();
        self.refresh_dependency_rows();
    }

    fn ensure_navigation_targets(&mut self) {
        let Some(source) = self.analysis_source.as_ref() else {
            self.focused_target_id = None;
            self.focused_dependency_row_key = None;
            self.dependency_root_target_id = None;
            self.dependency_expanded_row_keys.clear();
            self.pending_editor_jump = None;
            self.pending_dependency_reveal_row_key = None;
            return;
        };

        let next_root_target_id = source
            .dependency_root_target_id
            .clone()
            .filter(|target_id| target_exists(source, Some(target_id)))
            .or_else(|| {
                source
                    .dependency_targets
                    .first()
                    .map(|target| target.id.clone())
            });
        let focused_target_exists = target_exists(source, self.focused_target_id.as_deref());
        let fallback_focus_target_id = next_root_target_id.clone().or_else(|| {
            source
                .dependency_targets
                .first()
                .map(|target| target.id.clone())
        });
        if self.dependency_root_target_id != next_root_target_id {
            self.dependency_root_target_id = next_root_target_id;
            self.reset_dependency_expansion_to_root();
        }

        if !focused_target_exists {
            self.focused_target_id = fallback_focus_target_id;
        }
    }

    fn refresh_dependency_rows(&mut self) {
        self.dependency_rows = self
            .analysis_source
            .as_ref()
            .and_then(|source| {
                self.dependency_root_target_id
                    .as_ref()
                    .and_then(|target_id| {
                        source
                            .dependency_trees
                            .get(target_id)
                            .map(|tree| flatten_dependency_tree(tree, source))
                    })
            })
            .unwrap_or_default();
        self.ensure_focused_dependency_row();
        self.prune_dependency_expansion();
        self.ensure_dependency_root_expanded();
    }

    fn focus_target(&mut self, target_id: impl Into<String>, _show_dependencies: bool) {
        self.focus_target_inner(target_id, true);
    }

    fn focus_target_from_editor(&mut self, target_id: impl Into<String>) {
        let target_id = target_id.into();
        self.focus_target_inner(target_id.clone(), false);
        if let Some(row_key) = self.shortest_dependency_row_key_for_target(&target_id) {
            self.focused_dependency_row_key = Some(row_key.clone());
            self.pending_dependency_reveal_row_key = Some(row_key.clone());
            self.reveal_dependency_row_key(&row_key, true);
        }
    }

    fn focus_target_inner(&mut self, target_id: impl Into<String>, jump_editor: bool) {
        let target_id = target_id.into();
        let Some(source) = self.analysis_source.as_ref() else {
            return;
        };
        if let Some(source_range) = source
            .dependency_targets
            .iter()
            .find(|target| target.id == target_id)
            .and_then(|target| target.source_range)
        {
            self.focused_target_id = Some(target_id.clone());
            if let Some(row_key) = self.shortest_dependency_row_key_for_target(&target_id) {
                self.focused_dependency_row_key = Some(row_key.clone());
                self.pending_dependency_reveal_row_key = Some(row_key.clone());
                self.reveal_dependency_row_key(&row_key, false);
            } else {
                self.focused_dependency_row_key = None;
            }
            if jump_editor {
                self.pending_editor_jump = Some(source_range);
            }
        } else if source
            .dependency_targets
            .iter()
            .any(|target| target.id == target_id)
        {
            self.focused_target_id = Some(target_id.clone());
            if let Some(row_key) = self.shortest_dependency_row_key_for_target(&target_id) {
                self.focused_dependency_row_key = Some(row_key.clone());
                self.pending_dependency_reveal_row_key = Some(row_key.clone());
                self.reveal_dependency_row_key(&row_key, false);
            } else {
                self.focused_dependency_row_key = None;
            }
        }
    }

    fn focus_tree_click(&mut self, click: PassDebugTreeClick, show_dependencies: bool) {
        if let Some(row_key) = click.toggle_row_key {
            self.toggle_dependency_row_expanded(&row_key);
        } else if let Some(row_key) = click.row_key {
            let jump_override = click.source_range;
            self.focus_dependency_row_key(
                row_key,
                show_dependencies,
                jump_override.is_none(),
                false,
            );
            if let Some(source_range) = jump_override {
                self.pending_editor_jump = Some(source_range);
            }
        } else if let Some(target_id) = click.target_id {
            self.focus_target(target_id, show_dependencies);
        } else if let Some(source_range) = click.source_range {
            self.pending_editor_jump = Some(source_range);
        }
    }

    fn focus_dependency_row_key(
        &mut self,
        row_key: impl Into<String>,
        _show_dependencies: bool,
        jump_editor: bool,
        reveal_row: bool,
    ) {
        let row_key = row_key.into();
        let Some(row) = self
            .dependency_rows
            .iter()
            .find(|row| row.row_key == row_key)
            .cloned()
        else {
            return;
        };
        self.focused_dependency_row_key = Some(row_key.clone());
        if reveal_row {
            self.pending_dependency_reveal_row_key = Some(row_key.clone());
            self.reveal_dependency_row_key(&row_key, false);
        }
        if let Some(target_id) = row.target_id {
            self.focused_target_id = Some(target_id.clone());
        }
        if jump_editor {
            self.pending_editor_jump = row.source_range;
        }
    }

    fn focus_target_at_char_index(&mut self, char_index: usize) {
        let byte_index = char_index_to_byte_index(&self.draft_source, char_index);
        let matching_dependency_row_key = self
            .dependency_rows
            .iter()
            .filter_map(|row| {
                let range = row.source_range?;
                if range.start_byte <= byte_index && byte_index < range.end_byte {
                    Some((
                        range.end_byte.saturating_sub(range.start_byte),
                        row.depth,
                        row.row_key.clone(),
                    ))
                } else {
                    None
                }
            })
            .min_by(
                |(left_len, left_depth, left_key), (right_len, right_depth, right_key)| {
                    right_depth
                        .cmp(left_depth)
                        .then_with(|| left_len.cmp(right_len))
                        .then_with(|| left_key.cmp(right_key))
                },
            )
            .map(|(_, _, row_key)| row_key);
        if let Some(row_key) = matching_dependency_row_key {
            self.focus_dependency_row_key(row_key, true, false, false);
            return;
        }

        let matching_target_id = self.analysis_source.as_ref().and_then(|source| {
            source
                .dependency_targets
                .iter()
                .find(|target| {
                    target
                        .source_range
                        .map(|range| range.start_byte <= byte_index && byte_index < range.end_byte)
                        .unwrap_or(false)
                })
                .map(|target| target.id.clone())
        });

        if let Some(target_id) = matching_target_id {
            self.focus_target_from_editor(target_id);
            return;
        }

        let matching_target_id =
            identifier_at_char_index(&self.draft_source, char_index).and_then(|identifier| {
                self.analysis_source.as_ref().and_then(|source| {
                    source
                        .dependency_targets
                        .iter()
                        .find(|target| target.name == identifier)
                        .map(|target| target.id.clone())
                })
            });
        if let Some(target_id) = matching_target_id {
            self.focus_target_from_editor(target_id);
        }
    }

    fn focused_source_range(&self) -> Option<PassDebugSourceRange> {
        if let Some(row_source_range) = self
            .focused_dependency_row_key
            .as_deref()
            .and_then(|row_key| {
                self.dependency_rows
                    .iter()
                    .find(|row| row.row_key == row_key)
            })
            .and_then(|row| row.source_range)
        {
            return Some(row_source_range);
        }

        let source = self.analysis_source.as_ref()?;
        let focused_target_id = self.focused_target_id.as_deref()?;
        source
            .dependency_targets
            .iter()
            .find(|target| target.id == focused_target_id)
            .and_then(|target| target.source_range)
    }

    fn focus_is_in_dependency_root(&self) -> bool {
        if self.focused_target_id.is_none() {
            return true;
        }
        let Some(row_key) = self.focused_dependency_row_key.as_deref() else {
            return false;
        };
        self.dependency_rows
            .iter()
            .any(|row| row.row_key == row_key)
    }

    fn ensure_focused_dependency_row(&mut self) {
        let focused_row_exists = self
            .focused_dependency_row_key
            .as_deref()
            .map(|row_key| {
                self.dependency_rows
                    .iter()
                    .any(|row| row.row_key == row_key)
            })
            .unwrap_or(false);
        if focused_row_exists {
            return;
        }

        self.focused_dependency_row_key = self
            .focused_target_id
            .as_deref()
            .and_then(|target_id| self.shortest_dependency_row_key_for_target(target_id));
    }

    fn shortest_dependency_row_key_for_target(&self, target_id: &str) -> Option<String> {
        self.dependency_rows
            .iter()
            .filter(|row| row.target_id.as_deref() == Some(target_id))
            .map(|row| (row.depth, row.row_key.clone()))
            .min_by(|(left_depth, left_key), (right_depth, right_key)| {
                left_depth
                    .cmp(right_depth)
                    .then_with(|| left_key.cmp(right_key))
            })
            .map(|(_, row_key)| row_key)
    }

    fn dependency_expandable_row_keys(&self) -> HashSet<String> {
        self.dependency_rows
            .iter()
            .filter_map(|row| row.parent_row_key.clone())
            .collect()
    }

    fn reset_dependency_expansion_to_root(&mut self) {
        self.dependency_expanded_row_keys.clear();
        self.ensure_dependency_root_expanded();
    }

    fn ensure_dependency_root_expanded(&mut self) {
        if let Some(root_row_key) = self.dependency_rows.first().map(|row| row.row_key.clone()) {
            self.dependency_expanded_row_keys.insert(root_row_key);
        }
    }

    fn prune_dependency_expansion(&mut self) {
        let expandable_row_keys = self.dependency_expandable_row_keys();
        self.dependency_expanded_row_keys
            .retain(|row_key| expandable_row_keys.contains(row_key));
    }

    fn toggle_dependency_row_expanded(&mut self, row_key: &str) {
        let expandable_row_keys = self.dependency_expandable_row_keys();
        if !expandable_row_keys.contains(row_key) {
            return;
        }
        if !self.dependency_expanded_row_keys.remove(row_key) {
            self.dependency_expanded_row_keys
                .insert(row_key.to_string());
        }
    }

    fn reveal_dependency_row_key(&mut self, row_key: &str, collapse_to_path: bool) {
        let path = dependency_path_for_row_key(&self.dependency_rows, row_key);
        if path.is_empty() {
            return;
        }
        let expandable_row_keys = self.dependency_expandable_row_keys();
        let ancestor_keys = path
            .iter()
            .take(path.len().saturating_sub(1))
            .filter(|row_key| expandable_row_keys.contains(*row_key))
            .cloned()
            .collect::<HashSet<_>>();
        if collapse_to_path {
            self.dependency_expanded_row_keys = ancestor_keys;
        } else {
            self.dependency_expanded_row_keys.extend(ancestor_keys);
        }
        self.ensure_dependency_root_expanded();
    }

    fn visible_dependency_rows(&self) -> Vec<PassDebugDependencyRow> {
        let mut visible_rows = Vec::new();
        let mut hidden_depth: Option<usize> = None;
        for row in &self.dependency_rows {
            if let Some(depth) = hidden_depth {
                if row.depth > depth {
                    continue;
                }
                hidden_depth = None;
            }

            visible_rows.push(row.clone());
            if self
                .dependency_rows
                .iter()
                .any(|child| child.parent_row_key.as_deref() == Some(row.row_key.as_str()))
                && !self.dependency_expanded_row_keys.contains(&row.row_key)
            {
                hidden_depth = Some(row.depth);
            }
        }
        visible_rows
    }

    fn dependency_focus_path_row_keys(&self) -> Vec<String> {
        let Some(row_key) = self.focused_dependency_row_key.as_deref() else {
            return Vec::new();
        };
        dependency_path_for_row_key(&self.dependency_rows, row_key)
    }

    fn record_error(&mut self, error: String) {
        self.last_error = Some(error);
        self.last_status = None;
    }
}

fn target_exists(source: &PassDebugSource, target_id: Option<&str>) -> bool {
    let Some(target_id) = target_id else {
        return false;
    };
    source
        .dependency_targets
        .iter()
        .any(|target| target.id == target_id)
}

fn consume_tree_reveal_row_key<Row: PassDebugTreeRow>(
    pending_row_key: &mut Option<String>,
    rows: &[Row],
) -> Option<String> {
    let row_key = pending_row_key.clone()?;
    if rows
        .iter()
        .any(|row| row.row_key().map(|key| key == row_key).unwrap_or(false))
    {
        *pending_row_key = None;
        Some(row_key)
    } else {
        None
    }
}

struct PassDebugTreeRenderState<'a> {
    focused_target_id: Option<&'a str>,
    focused_row_key: Option<&'a str>,
    reveal_row_key: Option<&'a str>,
    path_row_keys: &'a [String],
    expandable_row_keys: Option<&'a HashSet<String>>,
    expanded_row_keys: Option<&'a HashSet<String>>,
}

pub struct PassDebugWindowState {
    pass_name: String,
    viewport_id: egui::ViewportId,
    document: Arc<Mutex<PassDebugWindowDocument>>,
    close_requested: Arc<AtomicBool>,
    pending_actions: Arc<Mutex<Vec<PassDebugWindowAction>>>,
    last_viewport_inner_rect: Arc<Mutex<Option<egui::Rect>>>,
    focus_requested: bool,
}

impl PassDebugWindowState {
    fn new(
        pass_name: String,
        source: Option<PassDebugSource>,
        source_revision: u64,
        patch_active: bool,
    ) -> Self {
        let viewport_id = egui::ViewportId::from_hash_of(("pass-debug", pass_name.as_str()));
        Self {
            document: Arc::new(Mutex::new(PassDebugWindowDocument::new(
                pass_name.clone(),
                source,
                source_revision,
                patch_active,
            ))),
            close_requested: Arc::new(AtomicBool::new(false)),
            pending_actions: Arc::new(Mutex::new(Vec::new())),
            last_viewport_inner_rect: Arc::new(Mutex::new(None)),
            pass_name,
            viewport_id,
            focus_requested: true,
        }
    }

    fn update_source(
        &self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        patch_active: bool,
    ) {
        if let Ok(mut document) = self.document.lock() {
            document.update_source(source, source_revision, patch_active);
        }
    }

    fn drain_actions(&self, out: &mut Vec<PassDebugWindowAction>) {
        if let Ok(mut pending) = self.pending_actions.lock() {
            out.extend(pending.drain(..));
        }
    }
}

pub type PassDebugWindowMap = HashMap<String, PassDebugWindowState>;

pub fn open_pass_debug_window(
    windows: &mut PassDebugWindowMap,
    pass_sources: &HashMap<String, PassDebugSource>,
    pass_sources_revision: u64,
    pass_shader_overrides: &HashMap<String, String>,
    pass_name: String,
) {
    let source = pass_sources.get(pass_name.as_str());
    let patch_active = pass_shader_overrides.contains_key(pass_name.as_str());
    if let Some(existing) = windows.get_mut(pass_name.as_str()) {
        existing.update_source(source, pass_sources_revision, patch_active);
        existing.focus_requested = true;
        existing.close_requested.store(false, Ordering::Relaxed);
        return;
    }

    windows.insert(
        pass_name.clone(),
        PassDebugWindowState::new(
            pass_name,
            source.cloned(),
            pass_sources_revision,
            patch_active,
        ),
    );
}

pub fn show_pass_debug_windows(
    ctx: &egui::Context,
    windows: &mut PassDebugWindowMap,
    pass_sources: &HashMap<String, PassDebugSource>,
    pass_sources_revision: u64,
    pass_shader_overrides: &HashMap<String, String>,
) -> Vec<PassDebugWindowAction> {
    windows.retain(|_, state| !state.close_requested.load(Ordering::Relaxed));

    let mut actions = Vec::new();
    for state in windows.values_mut() {
        let patch_active = pass_shader_overrides.contains_key(state.pass_name.as_str());
        state.update_source(
            pass_sources.get(state.pass_name.as_str()),
            pass_sources_revision,
            patch_active,
        );

        let viewport_id = state.viewport_id;
        let document = Arc::clone(&state.document);
        let close_requested = Arc::clone(&state.close_requested);
        let pending_actions = Arc::clone(&state.pending_actions);
        let last_viewport_inner_rect = Arc::clone(&state.last_viewport_inner_rect);
        let title = format!("RenderPass Debug - {}", state.pass_name);
        let viewport_builder = egui::ViewportBuilder::default()
            .with_title(title.clone())
            .with_inner_size(egui::vec2(1180.0, 760.0))
            .with_min_inner_size(egui::vec2(640.0, 360.0));

        ctx.show_viewport_deferred(
            viewport_id,
            viewport_builder,
            move |ui, class| match class {
                egui::ViewportClass::EmbeddedWindow => {
                    let mut open = true;
                    egui::Window::new(title.as_str())
                        .id(egui::Id::new(("pass-debug-embedded", title.as_str())))
                        .open(&mut open)
                        .default_size(egui::vec2(1180.0, 760.0))
                        .show(ui.ctx(), |window_ui| {
                            render_pass_debug_embedded_content(
                                window_ui,
                                &document,
                                &pending_actions,
                            );
                        });
                    if !open {
                        close_requested.store(true, Ordering::Relaxed);
                    }
                }
                _ => {
                    if handle_pass_debug_viewport_close_request(
                        ui.ctx(),
                        &close_requested,
                        &last_viewport_inner_rect,
                    ) {
                        return;
                    }
                    render_pass_debug_viewport(ui, &document, &pending_actions);
                }
            },
        );

        if state.focus_requested {
            ctx.send_viewport_cmd_to(state.viewport_id, egui::ViewportCommand::Focus);
            state.focus_requested = false;
        }

        state.drain_actions(&mut actions);
    }

    actions
}

pub fn mark_patch_applied(
    windows: &mut PassDebugWindowMap,
    pass_name: &str,
    source: Option<&PassDebugSource>,
    source_revision: u64,
    draft_source: String,
    status: String,
) {
    if let Some(state) = windows.get(pass_name)
        && let Ok(mut document) = state.document.lock()
    {
        document.mark_applied(source, source_revision, draft_source, status);
    }
}

pub fn mark_patch_reset(
    windows: &mut PassDebugWindowMap,
    pass_name: &str,
    source: Option<&PassDebugSource>,
    source_revision: u64,
    status: String,
) {
    if let Some(state) = windows.get(pass_name)
        && let Ok(mut document) = state.document.lock()
    {
        document.mark_reset(source, source_revision, status);
    }
}

pub fn mark_all_patches_reset(
    windows: &mut PassDebugWindowMap,
    pass_sources: &HashMap<String, PassDebugSource>,
    pass_sources_revision: u64,
    status: String,
) {
    for (pass_name, state) in windows.iter() {
        if let Ok(mut document) = state.document.lock() {
            document.mark_reset(
                pass_sources.get(pass_name),
                pass_sources_revision,
                status.clone(),
            );
        }
    }
}

pub fn record_patch_error(windows: &mut PassDebugWindowMap, pass_name: &str, error: String) {
    if let Some(state) = windows.get(pass_name)
        && let Ok(mut document) = state.document.lock()
    {
        document.record_error(error);
    }
}

pub fn record_all_patch_error(windows: &mut PassDebugWindowMap, error: String) {
    for state in windows.values() {
        if let Ok(mut document) = state.document.lock() {
            document.record_error(error.clone());
        }
    }
}

fn handle_pass_debug_viewport_close_request(
    ctx: &egui::Context,
    close_requested: &AtomicBool,
    last_inner_rect: &Mutex<Option<egui::Rect>>,
) -> bool {
    let viewport = ctx.input(|input| input.viewport().clone());
    let previous_inner_rect = last_inner_rect.lock().ok().and_then(|guard| *guard);
    if let Ok(mut guard) = last_inner_rect.lock() {
        *guard = viewport.inner_rect;
    }

    if !viewport.close_requested() {
        return false;
    }

    if is_close_request_during_large_viewport_resize(previous_inner_rect, viewport.inner_rect) {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        return false;
    }

    close_requested.store(true, Ordering::Relaxed);
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    true
}

fn is_close_request_during_large_viewport_resize(
    previous: Option<egui::Rect>,
    current: Option<egui::Rect>,
) -> bool {
    let (Some(previous), Some(current)) = (previous, current) else {
        return false;
    };
    let width_delta = (previous.width() - current.width()).abs();
    let height_delta = (previous.height() - current.height()).abs();
    width_delta.max(height_delta) >= PASS_DEBUG_CLOSE_RESIZE_DELTA_THRESHOLD
}

fn render_pass_debug_viewport(
    ui: &mut egui::Ui,
    document: &Arc<Mutex<PassDebugWindowDocument>>,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    let pass_name = document
        .lock()
        .map(|document| document.pass_name.clone())
        .unwrap_or_else(|_| "unavailable".to_string());

    egui::Panel::top(egui::Id::new(("pass-debug-toolbar", pass_name.as_str())))
        .resizable(false)
        .show_inside(ui, |ui| {
            let Ok(mut document) = document.lock() else {
                ui.label("Debug document unavailable");
                return;
            };
            render_pass_debug_toolbar(ui, &mut document, pending_actions);
            render_patch_messages(ui, &document);
        });

    egui::CentralPanel::default().show_inside(ui, |ui| {
        let Ok(mut document) = document.lock() else {
            ui.label("Debug document unavailable");
            return;
        };
        if document.source.is_none() {
            render_missing_source_message(ui);
            return;
        }
        render_dependency_editor_split(ui, &mut document);
    });
}

fn render_pass_debug_embedded_content(
    ui: &mut egui::Ui,
    document: &Arc<Mutex<PassDebugWindowDocument>>,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    let Ok(mut document) = document.lock() else {
        ui.label("Debug document unavailable");
        return;
    };

    render_pass_debug_toolbar(ui, &mut document, pending_actions);
    render_patch_messages(ui, &document);
    if document.source.is_none() {
        ui.add_space(8.0);
        render_missing_source_message(ui);
        return;
    }

    ui.add_space(8.0);
    render_dependency_editor_split(ui, &mut document);
}

fn render_side_panel(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    render_dependency_panel(ui, document);
}

fn render_dependency_panel(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    let Some(source) = document.analysis_source.as_ref() else {
        ui.colored_label(
            egui::Color32::from_rgb(255, 180, 120),
            "Pass no longer exists",
        );
        return;
    };

    if let Some(error) = source.parse_error.as_ref() {
        ui.add_space(8.0);
        ui.colored_label(egui::Color32::from_rgb(255, 118, 118), "WGSL parse failed");
        ui.label(egui::RichText::new(error.as_str()).monospace().small());
        ui.add_space(8.0);
        return;
    }

    if let Some(error) = source.dependency_error.as_ref() {
        ui.colored_label(
            egui::Color32::from_rgb(255, 180, 120),
            "Dependency analysis failed",
        );
        ui.label(egui::RichText::new(error.as_str()).monospace().small());
        ui.add_space(8.0);
    }

    if document.dependency_rows.is_empty() {
        ui.label(
            egui::RichText::new("Select a dependency target")
                .font(pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE)),
        );
        return;
    }

    render_dependency_rows(ui, document);
}

fn render_dependency_rows(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    if !document.focus_is_in_dependency_root() {
        ui.label(
            egui::RichText::new("Focus is outside the current dependency map")
                .font(pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE))
                .color(egui::Color32::from_rgb(255, 180, 120)),
        );
    }

    let reveal_row_key = consume_tree_reveal_row_key(
        &mut document.pending_dependency_reveal_row_key,
        &document.dependency_rows,
    );
    let path_row_keys = document.dependency_focus_path_row_keys();
    let visible_dependency_rows = document.visible_dependency_rows();
    let expandable_row_keys = document.dependency_expandable_row_keys();
    let tree_state = PassDebugTreeRenderState {
        focused_target_id: document.focused_target_id.as_deref(),
        focused_row_key: document.focused_dependency_row_key.as_deref(),
        reveal_row_key: reveal_row_key.as_deref(),
        path_row_keys: &path_row_keys,
        expandable_row_keys: Some(&expandable_row_keys),
        expanded_row_keys: Some(&document.dependency_expanded_row_keys),
    };
    if let Some(click) = render_scrollable_tree_rows(
        ui,
        egui::Id::new(("pass-debug-dependencies", document.pass_name.as_str())),
        &visible_dependency_rows,
        &tree_state,
    ) {
        document.focus_tree_click(click, true);
    }
}

fn render_scrollable_tree_rows<Row: PassDebugTreeRow>(
    ui: &mut egui::Ui,
    id: egui::Id,
    rows: &[Row],
    tree_state: &PassDebugTreeRenderState<'_>,
) -> Option<PassDebugTreeClick> {
    let font_id = pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE);
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font_id));
    let row_height_with_spacing = row_height + ui.spacing().item_spacing.y;
    let mut clicked_row: Option<PassDebugTreeClick> = None;

    egui::ScrollArea::both()
        .id_salt(id)
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let total_height = row_height_with_spacing * rows.len() as f32;
            let content_width = tree_rows_content_width(ui, rows, tree_state, &font_id);
            ui.set_min_size(egui::vec2(content_width, total_height));

            let min_row = (viewport.min.y / row_height_with_spacing).floor().max(0.0) as usize;
            let max_row =
                ((viewport.max.y / row_height_with_spacing).ceil() as usize + 1).min(rows.len());
            let content_origin = ui.min_rect().min;

            let reveal_row_index = tree_state.reveal_row_key.and_then(|reveal_row_key| {
                rows.iter().position(|row| {
                    row.row_key()
                        .map(|row_key| row_key == reveal_row_key)
                        .unwrap_or(false)
                })
            });
            if let Some(row_index) = reveal_row_index {
                let row_top = content_origin.y + row_index as f32 * row_height_with_spacing;
                let visible_reveal_rect = egui::Rect::from_min_max(
                    egui::pos2(content_origin.x + viewport.min.x, row_top),
                    egui::pos2(
                        content_origin.x + viewport.max.x,
                        row_top + row_height_with_spacing,
                    ),
                );
                ui.scroll_to_rect(visible_reveal_rect, Some(egui::Align::Center));
            }

            for row_index in min_row..max_row {
                let row = &rows[row_index];
                let row_top = content_origin.y + row_index as f32 * row_height_with_spacing;
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(content_origin.x, row_top),
                    egui::vec2(content_width, row_height_with_spacing),
                );

                let selected = tree_state
                    .focused_row_key
                    .zip(row.row_key())
                    .map(|(selected, row_key)| selected == row_key)
                    .or_else(|| {
                        tree_state
                            .focused_target_id
                            .zip(row.target_id())
                            .map(|(selected, target)| selected == target)
                    })
                    .unwrap_or(false);
                let row_key = row.row_key();
                let expandable = row_key
                    .zip(tree_state.expandable_row_keys)
                    .map(|(row_key, expandable_row_keys)| expandable_row_keys.contains(row_key))
                    .unwrap_or(false);
                let expanded = row_key
                    .zip(tree_state.expanded_row_keys)
                    .map(|(row_key, expanded_row_keys)| expanded_row_keys.contains(row_key))
                    .unwrap_or(false);
                let path_index = row_key.and_then(|row_key| {
                    tree_state
                        .path_row_keys
                        .iter()
                        .position(|path_row_key| path_row_key == row_key)
                });
                let response = if row.selectable() {
                    ui.interact(row_rect, id.with(row_index), egui::Sense::click())
                } else {
                    ui.interact(row_rect, id.with(row_index), egui::Sense::hover())
                };
                let response = if let Some(relation_path) = row.relation_path() {
                    response.on_hover_text(format!("Path: {relation_path}"))
                } else {
                    response
                };
                let indent = row.depth() as f32 * TREE_ROW_INDENT_WIDTH;
                let toggle_slot = if tree_state.expandable_row_keys.is_some() && row_key.is_some() {
                    TREE_ROW_INDENT_WIDTH
                } else {
                    0.0
                };
                let text_x = row_rect.left() + indent + toggle_slot;
                let label_width = ui
                    .painter()
                    .layout_no_wrap(
                        row.label().to_string(),
                        font_id.clone(),
                        ui.visuals().text_color(),
                    )
                    .size()
                    .x;
                let source_jump_range = row.source_jump_range();
                let source_jump_rect = source_jump_range.map(|_| {
                    let button_size = source_jump_button_size(ui, &font_id);
                    egui::Rect::from_min_size(
                        egui::pos2(
                            text_x + label_width + TREE_ROW_SOURCE_JUMP_GAP,
                            row_rect.center().y - button_size.y * 0.5,
                        ),
                        button_size,
                    )
                });
                let source_jump_response = source_jump_rect.map(|button_rect| {
                    ui.interact(
                        button_rect,
                        id.with(("source-jump", row_key.unwrap_or_default(), row_index)),
                        egui::Sense::click(),
                    )
                    .on_hover_text("Jump to source")
                });
                let mut toggle_clicked = false;
                let mut toggle_hovered = false;
                let mut toggle_rect = None;
                let toggle_symbol = if expandable {
                    let next_toggle_rect = egui::Rect::from_min_size(
                        egui::pos2(row_rect.left() + indent, row_rect.top()),
                        egui::vec2(TREE_ROW_INDENT_WIDTH, row_height_with_spacing),
                    );
                    let toggle_id = id.with(("toggle", row_key.unwrap_or_default().to_string()));
                    let toggle_response =
                        ui.interact(next_toggle_rect, toggle_id, egui::Sense::click());
                    toggle_clicked = toggle_response.clicked();
                    toggle_hovered = toggle_response.hovered();
                    toggle_rect = Some(next_toggle_rect);
                    Some(if expanded { "-" } else { "+" })
                } else {
                    None
                };

                if let Some(path_index) = path_index {
                    ui.painter().rect_filled(
                        row_rect,
                        0.0,
                        dependency_path_color(ui, path_index, tree_state.path_row_keys.len()),
                    );
                }
                if selected {
                    ui.painter()
                        .rect_filled(row_rect, 0.0, tree_selected_row_bg(ui));
                } else if row.selectable() && response.hovered() {
                    ui.painter()
                        .rect_filled(row_rect, 0.0, tree_hovered_row_bg(ui));
                }

                if toggle_clicked {
                    clicked_row = Some(PassDebugTreeClick {
                        row_key: None,
                        target_id: None,
                        source_range: None,
                        toggle_row_key: row.row_key().map(str::to_string),
                    });
                } else if source_jump_response
                    .as_ref()
                    .map(|response| response.clicked())
                    .unwrap_or(false)
                {
                    clicked_row = Some(PassDebugTreeClick {
                        row_key: row.row_key().map(str::to_string),
                        target_id: row.target_id().map(str::to_string),
                        source_range: source_jump_range,
                        toggle_row_key: None,
                    });
                } else if response.clicked()
                    && (row.target_id().is_some() || row.source_range().is_some())
                {
                    clicked_row = Some(PassDebugTreeClick {
                        row_key: row.row_key().map(str::to_string),
                        target_id: row.target_id().map(str::to_string),
                        source_range: row.source_range(),
                        toggle_row_key: None,
                    });
                }

                let text_color = if selected {
                    tree_highlight_text_color(ui)
                } else {
                    ui.visuals().text_color()
                };
                let galley = ui.painter().layout_no_wrap(
                    row.label().to_string(),
                    font_id.clone(),
                    text_color,
                );
                let text_pos = egui::pos2(text_x, row_rect.center().y - galley.size().y * 0.5);
                ui.painter().galley(text_pos, galley, text_color);
                if let (Some(toggle_rect), Some(toggle_symbol)) = (toggle_rect, toggle_symbol) {
                    paint_tree_toggle_symbol(
                        ui,
                        toggle_rect,
                        toggle_symbol,
                        toggle_hovered,
                        &font_id,
                    );
                }
                if let (Some(button_rect), Some(button_response)) =
                    (source_jump_rect, source_jump_response.as_ref())
                {
                    paint_source_jump_button(ui, button_rect, button_response.hovered(), &font_id);
                }
            }
        });

    clicked_row
}

fn tree_rows_content_width<Row: PassDebugTreeRow>(
    ui: &egui::Ui,
    rows: &[Row],
    tree_state: &PassDebugTreeRenderState<'_>,
    font_id: &egui::FontId,
) -> f32 {
    let available_width = ui.available_width().max(0.0);
    let text_color = ui.visuals().text_color();

    rows.iter()
        .map(|row| {
            let indent = row.depth() as f32 * TREE_ROW_INDENT_WIDTH;
            let toggle_slot = if tree_state.expandable_row_keys.is_some() && row.row_key().is_some()
            {
                TREE_ROW_INDENT_WIDTH
            } else {
                0.0
            };
            let label_width = ui
                .painter()
                .layout_no_wrap(row.label().to_string(), font_id.clone(), text_color)
                .size()
                .x;
            let source_jump_width = if row.source_jump_range().is_some() {
                TREE_ROW_SOURCE_JUMP_GAP + source_jump_button_size(ui, font_id).x
            } else {
                0.0
            };

            indent + toggle_slot + label_width + source_jump_width + TREE_ROW_TRAILING_PADDING
        })
        .fold(available_width, f32::max)
}

fn source_jump_button_size(ui: &egui::Ui, font_id: &egui::FontId) -> egui::Vec2 {
    let text_color = ui.visuals().text_color();
    let label_size = ui
        .painter()
        .layout_no_wrap(
            TREE_ROW_SOURCE_JUMP_LABEL.to_string(),
            font_id.clone(),
            text_color,
        )
        .size();
    egui::vec2(
        label_size.x + TREE_ROW_SOURCE_JUMP_HORIZONTAL_PADDING * 2.0,
        label_size.y + TREE_ROW_SOURCE_JUMP_VERTICAL_PADDING * 2.0,
    )
}

fn paint_tree_toggle_symbol(
    ui: &egui::Ui,
    rect: egui::Rect,
    symbol: &str,
    hovered: bool,
    font_id: &egui::FontId,
) {
    let symbol_color = if hovered {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let symbol_galley =
        ui.painter()
            .layout_no_wrap(symbol.to_string(), font_id.clone(), symbol_color);
    let symbol_pos = egui::pos2(
        rect.center().x - symbol_galley.size().x * 0.5,
        rect.center().y - symbol_galley.size().y * 0.5,
    );
    ui.painter().galley(symbol_pos, symbol_galley, symbol_color);
}

fn paint_source_jump_button(
    ui: &egui::Ui,
    rect: egui::Rect,
    hovered: bool,
    font_id: &egui::FontId,
) {
    let fill = if hovered {
        source_jump_button_hover_bg(ui)
    } else {
        source_jump_button_bg(ui)
    };
    let text_color = if hovered {
        tree_highlight_text_color(ui)
    } else {
        ui.visuals().weak_text_color()
    };
    ui.painter().rect_filled(rect, 3.0, fill);
    let galley = ui.painter().layout_no_wrap(
        TREE_ROW_SOURCE_JUMP_LABEL.to_string(),
        font_id.clone(),
        text_color,
    );
    let text_pos = egui::pos2(
        rect.center().x - galley.size().x * 0.5,
        rect.center().y - galley.size().y * 0.5,
    );
    ui.painter().galley(text_pos, galley, text_color);
}

fn dependency_path_color(ui: &egui::Ui, index: usize, len: usize) -> egui::Color32 {
    let t = if len <= 1 {
        1.0
    } else {
        index as f32 / (len - 1) as f32
    };
    let (start, end) = if ui.visuals().dark_mode {
        (
            egui::Color32::from_rgba_unmultiplied(96, 165, 250, 26),
            egui::Color32::from_rgba_unmultiplied(245, 158, 11, 38),
        )
    } else {
        (
            egui::Color32::from_rgba_unmultiplied(37, 99, 235, 20),
            egui::Color32::from_rgba_unmultiplied(180, 83, 9, 28),
        )
    };
    lerp_color(start, end, t)
}

fn tree_selected_row_bg(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(44, 58, 76)
    } else {
        egui::Color32::from_rgb(218, 231, 248)
    }
}

fn tree_hovered_row_bg(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 18)
    } else {
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 10)
    }
}

fn source_jump_button_bg(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(148, 163, 184, 30)
    } else {
        egui::Color32::from_rgba_unmultiplied(71, 85, 105, 20)
    }
}

fn source_jump_button_hover_bg(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(96, 165, 250, 62)
    } else {
        egui::Color32::from_rgba_unmultiplied(37, 99, 235, 42)
    }
}

fn tree_highlight_text_color(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(238, 242, 247)
    } else {
        egui::Color32::from_rgb(20, 31, 46)
    }
}

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let [ar, ag, ab, aa] = a.to_srgba_unmultiplied();
    let [br, bg, bb, ba] = b.to_srgba_unmultiplied();
    let lerp = |a: u8, b: u8| -> u8 { (a as f32 + (b as f32 - a as f32) * t).round() as u8 };
    egui::Color32::from_rgba_unmultiplied(lerp(ar, br), lerp(ag, bg), lerp(ab, bb), lerp(aa, ba))
}

fn render_pass_debug_toolbar(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    let save_requested =
        ui.input(|input| input.modifiers.command && input.key_pressed(egui::Key::S));

    ui.horizontal(|ui| {
        ui.heading("WGSL Module");
        let badge = if document.dirty {
            "Dirty"
        } else if document.patch_active {
            "Patched"
        } else {
            "Generated"
        };
        ui.label(egui::RichText::new(badge).monospace().small());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Copy WGSL").clicked() {
                ui.ctx().copy_text(document.draft_source.clone());
            }
            if ui.button("Reset All").clicked() {
                push_action(pending_actions, PassDebugWindowAction::ResetAllPatches);
            }
            if ui
                .add_enabled(document.patch_active, egui::Button::new("Reset Patch"))
                .clicked()
            {
                push_action(
                    pending_actions,
                    PassDebugWindowAction::ResetPatch {
                        pass_name: document.pass_name.clone(),
                    },
                );
            }
            if ui
                .add_enabled(document.dirty, egui::Button::new("Revert Draft"))
                .clicked()
            {
                document.draft_source = document.loaded_source.clone();
                document.dirty = false;
                document.last_error = None;
                document.last_status = Some("Draft reverted".to_string());
            }
            let apply_clicked = ui
                .add_enabled(document.dirty, egui::Button::new("Apply"))
                .clicked();
            if apply_clicked || (save_requested && document.dirty) {
                document.last_error = None;
                document.last_status = Some("Applying patch...".to_string());
                push_action(
                    pending_actions,
                    PassDebugWindowAction::ApplyPatch {
                        pass_name: document.pass_name.clone(),
                        source: document.draft_source.clone(),
                    },
                );
            }
        });
    });
}

fn render_patch_messages(ui: &mut egui::Ui, document: &PassDebugWindowDocument) {
    if let Some(error) = document.last_error.as_ref() {
        ui.add_space(6.0);
        ui.colored_label(egui::Color32::from_rgb(255, 118, 118), "Patch failed");
        ui.label(egui::RichText::new(error.as_str()).monospace().small());
    } else if let Some(status) = document.last_status.as_ref() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(status.as_str()).monospace().small());
    }
}

fn render_missing_source_message(ui: &mut egui::Ui) {
    ui.colored_label(
        egui::Color32::from_rgb(255, 180, 120),
        "Pass no longer exists in the current scene.",
    );
}

fn render_dependency_editor_split(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    let full_rect = ui.available_rect_before_wrap();
    if full_rect.width() <= 0.0 || full_rect.height() <= 0.0 {
        return;
    }

    let split_id = egui::Id::new(("pass-debug-split-width", document.pass_name.as_str()));
    let available_for_panel = (full_rect.width() - PASS_DEBUG_SPLIT_HANDLE_WIDTH).max(0.0);
    let max_panel_width = SIDE_PANEL_MAX_WIDTH
        .min(
            (available_for_panel - PASS_DEBUG_EDITOR_MIN_WIDTH)
                .max(SIDE_PANEL_MIN_WIDTH)
                .min(available_for_panel),
        )
        .max(0.0);
    let min_panel_width = SIDE_PANEL_MIN_WIDTH.min(max_panel_width);
    let panel_width = ui
        .ctx()
        .data_mut(|data| {
            data.get_persisted::<f32>(split_id)
                .unwrap_or(SIDE_PANEL_DEFAULT_WIDTH)
        })
        .clamp(min_panel_width, max_panel_width);

    let panel_rect = egui::Rect::from_min_max(
        full_rect.min,
        egui::pos2(full_rect.left() + panel_width, full_rect.bottom()),
    );
    let handle_rect = egui::Rect::from_min_max(
        egui::pos2(panel_rect.right(), full_rect.top()),
        egui::pos2(
            panel_rect.right() + PASS_DEBUG_SPLIT_HANDLE_WIDTH,
            full_rect.bottom(),
        ),
    );
    let editor_rect = egui::Rect::from_min_max(
        egui::pos2(handle_rect.right(), full_rect.top()),
        full_rect.right_bottom(),
    );

    let handle_response = ui.interact(
        handle_rect,
        split_id.with("handle"),
        egui::Sense::click_and_drag(),
    );
    if handle_response.hovered() || handle_response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if handle_response.dragged() {
        let next_width =
            (panel_width + handle_response.drag_delta().x).clamp(min_panel_width, max_panel_width);
        ui.ctx()
            .data_mut(|data| data.insert_persisted(split_id, next_width));
        ui.ctx().request_repaint();
    }

    let line_x = handle_rect.center().x;
    let line_color = if handle_response.hovered() || handle_response.dragged() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    };
    ui.painter().rect_filled(
        egui::Rect::from_center_size(
            egui::pos2(line_x, handle_rect.center().y),
            egui::vec2(PASS_DEBUG_SPLIT_LINE_WIDTH, handle_rect.height()),
        ),
        0.0,
        line_color,
    );

    let mut panel_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-side-child", document.pass_name.as_str()))
            .max_rect(panel_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    panel_ui.set_clip_rect(panel_rect.intersect(ui.clip_rect()));
    render_side_panel(&mut panel_ui, document);

    let mut editor_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-editor-child", document.pass_name.as_str()))
            .max_rect(editor_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    editor_ui.set_clip_rect(editor_rect.intersect(ui.clip_rect()));
    render_code_editor(&mut editor_ui, document);

    ui.advance_cursor_after_rect(full_rect);
}

fn render_code_editor(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    let focused_source_range = document.focused_source_range();
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let theme = egui_extras::syntax_highlighting::CodeTheme::from_memory(ui.ctx(), ui.style());
        let mut layout_job = egui_extras::syntax_highlighting::highlight(
            ui.ctx(),
            ui.style(),
            &theme,
            buf.as_str(),
            "rust",
        );
        if let Some(source_range) = focused_source_range {
            apply_layout_job_highlight(
                &mut layout_job,
                buf.as_str(),
                source_range,
                egui::Color32::from_rgba_premultiplied(251, 191, 36, 56),
            );
        }
        layout_job.wrap.max_width = wrap_width;
        ui.fonts_mut(|fonts| fonts.layout_job(layout_job))
    };

    ui.scope(|ui| {
        ui.visuals_mut().text_cursor.preview = false;
        egui::ScrollArea::vertical()
            .id_salt(("pass-debug-source-editor", document.pass_name.as_str()))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let editor = egui::TextEdit::multiline(&mut document.draft_source)
                    .id_salt(("pass-debug-source-text", document.pass_name.as_str()))
                    .font(pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE))
                    .code_editor()
                    .frame(egui::Frame::NONE)
                    .desired_rows(24)
                    .desired_width(f32::INFINITY)
                    .lock_focus(true)
                    .layouter(&mut layouter);

                let output = editor.show(ui);
                if output.response.changed() {
                    document.dirty = document.draft_source != document.loaded_source;
                    document.last_status = None;
                    document.refresh_draft_analysis();
                }
                if let Some(source_range) = document.pending_editor_jump.take() {
                    jump_editor_to_source_range(ui, &output, &document.draft_source, source_range);
                }
                if (output.response.clicked() || output.response.changed())
                    && let Some(cursor_range) = output.cursor_range
                {
                    document.focus_target_at_char_index(cursor_range.primary.index);
                }
            });
    });
}

fn apply_layout_job_highlight(
    layout_job: &mut egui::text::LayoutJob,
    source: &str,
    source_range: PassDebugSourceRange,
    background: egui::Color32,
) {
    let highlight_start = source_range.start_byte;
    let highlight_end = source_range.end_byte;
    if highlight_start >= highlight_end
        || highlight_end > source.len()
        || !source.is_char_boundary(highlight_start)
        || !source.is_char_boundary(highlight_end)
    {
        return;
    }

    let sections = std::mem::take(&mut layout_job.sections);
    for section in sections {
        let section_start = section.byte_range.start;
        let section_end = section.byte_range.end;
        let overlap_start = section_start.max(highlight_start);
        let overlap_end = section_end.min(highlight_end);

        if overlap_start >= overlap_end {
            layout_job.sections.push(section);
            continue;
        }

        if section_start < overlap_start {
            layout_job.sections.push(egui::text::LayoutSection {
                leading_space: section.leading_space,
                byte_range: section_start..overlap_start,
                format: section.format.clone(),
            });
        }

        let mut highlight_format = section.format.clone();
        highlight_format.background = background;
        layout_job.sections.push(egui::text::LayoutSection {
            leading_space: if section_start == overlap_start {
                section.leading_space
            } else {
                0.0
            },
            byte_range: overlap_start..overlap_end,
            format: highlight_format,
        });

        if overlap_end < section_end {
            layout_job.sections.push(egui::text::LayoutSection {
                leading_space: 0.0,
                byte_range: overlap_end..section_end,
                format: section.format,
            });
        }
    }
}

fn jump_editor_to_source_range(
    ui: &mut egui::Ui,
    output: &egui::widgets::text_edit::TextEditOutput,
    source: &str,
    source_range: PassDebugSourceRange,
) {
    if source_range.start_byte >= source_range.end_byte || source_range.end_byte > source.len() {
        return;
    }

    let start_char = byte_index_to_char_index(source, source_range.start_byte);
    let end_char = byte_index_to_char_index(source, source_range.end_byte).max(start_char + 1);
    let selection = egui::text::CCursorRange::two(
        egui::text::CCursor::new(start_char),
        egui::text::CCursor::new(end_char),
    );
    let mut state = output.state.clone();
    state.cursor.set_char_range(Some(selection));
    state.store(ui.ctx(), output.response.id);
    output.response.request_focus();

    let cursor_rect = output
        .galley
        .pos_from_cursor(egui::text::CCursor::new(start_char))
        .translate(output.galley_pos.to_vec2())
        .expand2(egui::vec2(0.0, 64.0));
    ui.scroll_to_rect(cursor_rect, Some(egui::Align::Center));
}

fn push_action(
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    action: PassDebugWindowAction,
) {
    if let Ok(mut pending) = pending_actions.lock() {
        pending.push(action);
    }
}

fn pass_debug_mono_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("geist_mono".into()))
}

fn clean_debug_tree_row_label(label: &str) -> String {
    let Some(stripped) = strip_leading_naga_handle(label.trim_start()) else {
        return label.to_string();
    };
    stripped.trim_start().to_string()
}

fn strip_leading_naga_handle(label: &str) -> Option<&str> {
    let rest = label.strip_prefix('[')?;
    let (handle, after_handle) = rest.split_once(']')?;
    if handle.is_empty() || !handle.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(after_handle.strip_prefix(':').unwrap_or(after_handle))
}

fn flatten_dependency_tree(
    root: &PassDebugDependencyNode,
    source: &PassDebugSource,
) -> Vec<PassDebugDependencyRow> {
    let mut rows = Vec::new();
    push_dependency_rows(
        root,
        source,
        0,
        None,
        &mut vec![0],
        &mut Vec::new(),
        &mut rows,
    );
    rows
}

fn push_dependency_rows(
    node: &PassDebugDependencyNode,
    source: &PassDebugSource,
    depth: usize,
    parent_row_key: Option<String>,
    path: &mut Vec<usize>,
    relation_path: &mut Vec<String>,
    rows: &mut Vec<PassDebugDependencyRow>,
) {
    if node.target_id.is_some() {
        let row_key = dependency_row_key(path);
        let relation_path_text = relation_path.join(" / ");
        let target_id = node.target_id.clone();
        let label = dependency_target_row_label(
            source,
            target_id.as_deref(),
            &node.label,
            node.display_label.as_deref(),
            node.edge_label.as_deref(),
        );
        let target_range = target_id
            .as_deref()
            .and_then(|target_id| target_source_range(source, target_id));
        let source_range = node.source_range.or(target_range);
        let source_jump_range =
            target_range.filter(|target_range| source_range != Some(*target_range));
        rows.push(PassDebugDependencyRow {
            depth,
            row_key: row_key.clone(),
            parent_row_key,
            label,
            relation_path: relation_path_text,
            target_id,
            source_range,
            source_jump_range,
            selectable: true,
        });
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            let mut child_relation_path = Vec::new();
            push_dependency_rows(
                child,
                source,
                depth + 1,
                Some(row_key.clone()),
                path,
                &mut child_relation_path,
                rows,
            );
            path.pop();
        }
    } else {
        let relation_label = compact_dependency_relation_label(&node.label);
        if !relation_label.is_empty() {
            relation_path.push(relation_label);
        }
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            push_dependency_rows(
                child,
                source,
                depth,
                parent_row_key.clone(),
                path,
                relation_path,
                rows,
            );
            path.pop();
        }
        if !relation_path.is_empty() {
            relation_path.pop();
        }
    }
}

fn dependency_target_row_label(
    source: &PassDebugSource,
    target_id: Option<&str>,
    fallback_label: &str,
    display_label: Option<&str>,
    edge_label: Option<&str>,
) -> String {
    let fallback_label = clean_debug_tree_row_label(fallback_label);
    let base_label = display_label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            target_id
                .and_then(|target_id| {
                    source
                        .dependency_targets
                        .iter()
                        .find(|target| target.id == target_id)
                })
                .map(|target| target.name.clone())
        })
        .unwrap_or_else(|| fallback_label.clone());
    let status = ["[cycle]", "[depth limit]"]
        .into_iter()
        .find(|status| fallback_label.contains(status));
    let mut label = match edge_label.map(str::trim).filter(|edge| !edge.is_empty()) {
        Some(edge) => format!("{base_label} ({edge})"),
        None => base_label,
    };
    if let Some(status) = status {
        label.push(' ');
        label.push_str(status);
    }
    label
}

fn dependency_row_key(path: &[usize]) -> String {
    path.iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join("/")
}

fn compact_dependency_relation_label(label: &str) -> String {
    let label = clean_debug_tree_row_label(label);
    let label = label.trim();
    if let Some(rest) = label.strip_prefix('[')
        && let Some((edge, after_edge)) = rest.split_once(']')
    {
        return format!("{edge}{}", after_edge).trim().to_string();
    }
    label.to_string()
}

fn target_source_range(source: &PassDebugSource, target_id: &str) -> Option<PassDebugSourceRange> {
    source
        .dependency_targets
        .iter()
        .find(|target| target.id == target_id)
        .and_then(|target| target.source_range)
}

fn dependency_path_for_row_key(rows: &[PassDebugDependencyRow], row_key: &str) -> Vec<String> {
    if !rows.iter().any(|row| row.row_key == row_key) {
        return Vec::new();
    }
    let row_parent_by_key = rows
        .iter()
        .map(|row| (row.row_key.as_str(), row.parent_row_key.as_deref()))
        .collect::<HashMap<_, _>>();
    let mut path = Vec::new();
    let mut current = Some(row_key);
    while let Some(row_key) = current {
        path.push(row_key.to_string());
        current = row_parent_by_key.get(row_key).copied().flatten();
    }
    path.reverse();
    path
}

fn identifier_at_char_index(source: &str, char_index: usize) -> Option<String> {
    let byte_index = char_index_to_byte_index(source, char_index);
    if source.is_empty() || byte_index > source.len() {
        return None;
    }

    let mut start = byte_index.min(source.len());
    while start > 0 {
        let Some((prev_index, ch)) = source[..start].char_indices().next_back() else {
            break;
        };
        if is_wgsl_identifier_char(ch) {
            start = prev_index;
        } else {
            break;
        }
    }

    let mut end = byte_index.min(source.len());
    while end < source.len() {
        let Some(ch) = source[end..].chars().next() else {
            break;
        };
        if is_wgsl_identifier_char(ch) {
            end += ch.len_utf8();
        } else {
            break;
        }
    }

    if start == end {
        return None;
    }
    let ident = &source[start..end];
    if ident
        .chars()
        .next()
        .map(is_wgsl_identifier_start)
        .unwrap_or(false)
    {
        Some(ident.to_string())
    } else {
        None
    }
}

fn char_index_to_byte_index(source: &str, char_index: usize) -> usize {
    source
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(source.len())
}

fn byte_index_to_char_index(source: &str, byte_index: usize) -> usize {
    let byte_index = byte_index.min(source.len());
    source[..byte_index].chars().count()
}

fn is_wgsl_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_wgsl_identifier_char(ch: char) -> bool {
    is_wgsl_identifier_start(ch) || ch.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use rust_wgpu_fiber::eframe::egui;

    use super::{
        PassDebugDependencyRow, PassDebugTreeClick, PassDebugWindowDocument,
        byte_index_to_char_index, dependency_path_for_row_key, flatten_dependency_tree,
        is_close_request_during_large_viewport_resize,
    };
    use crate::renderer::{
        PassDebugDependencyNode, PassDebugDependencyTarget, PassDebugSource, PassDebugSourceRange,
    };

    fn has_target_named(document: &PassDebugWindowDocument, name: &str) -> bool {
        document
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

    fn source_target_id_by_name(source: &PassDebugSource, name: &str) -> String {
        source
            .dependency_targets
            .iter()
            .find(|target| target.name == name)
            .map(|target| target.id.clone())
            .unwrap_or_else(|| panic!("missing target named {name}"))
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

    #[test]
    fn close_request_resize_guard_only_matches_large_size_changes() {
        let previous = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(800.0, 600.0));
        let maximized = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1440.0, 900.0));
        let nearly_same =
            egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(804.0, 604.0));

        assert!(is_close_request_during_large_viewport_resize(
            Some(previous),
            Some(maximized),
        ));
        assert!(!is_close_request_during_large_viewport_resize(
            Some(previous),
            Some(nearly_same),
        ));
        assert!(!is_close_request_during_large_viewport_resize(
            None,
            Some(maximized),
        ));
    }

    #[test]
    fn dirty_draft_is_not_replaced_by_source_refresh() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.draft_source = "fn edited() {}\n".to_string();
        document.dirty = true;

        let refreshed = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        document.update_source(Some(&refreshed), 1, false);

        assert_eq!(document.draft_source, "fn edited() {}\n");
        assert!(document.dirty);
    }

    #[test]
    fn clean_document_tracks_source_refresh() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);

        let refreshed = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        document.update_source(Some(&refreshed), 1, true);

        assert_eq!(document.draft_source, "fn generated() {}\n");
        assert!(document.patch_active);
        assert!(!document.dirty);
    }

    #[test]
    fn same_source_revision_does_not_refresh_document() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 7, false);

        let refreshed = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        document.update_source(Some(&refreshed), 7, true);

        assert_eq!(document.draft_source, "fn a() {}\n");
        assert!(document.patch_active);
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
        assert!(!document.dependency_rows.is_empty());
    }

    #[test]
    fn dirty_draft_analysis_does_not_overwrite_draft_text() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var loaded: f32 = 0.0; return loaded; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.draft_source =
            "fn a() -> f32 { var draft: f32 = 1.0; return draft; }\n".to_string();
        document.dirty = true;
        document.refresh_draft_analysis();
        assert!(has_target_named(&document, "draft"));

        let refreshed = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var generated: f32 = 2.0; return generated; }\n",
        );
        document.update_source(Some(&refreshed), 1, false);

        assert_eq!(
            document.draft_source,
            "fn a() -> f32 { var draft: f32 = 1.0; return draft; }\n"
        );
        assert!(has_target_named(&document, "draft"));
        assert!(!has_target_named(&document, "generated"));
    }

    #[test]
    fn focusing_dependency_child_does_not_replace_root_tree() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) vec4f { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; return vec4f(c, c, c, 1.0); }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let root_id = document.dependency_root_target_id.clone().unwrap();
        let child_id = target_id_by_name(&document, "b");

        document.focus_target(child_id.clone(), true);

        assert_eq!(
            document.dependency_root_target_id.as_deref(),
            Some(root_id.as_str())
        );
        assert_eq!(
            document.focused_target_id.as_deref(),
            Some(child_id.as_str())
        );
        assert_eq!(
            document.dependency_rows[0].target_id.as_deref(),
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
            document.dependency_root_target_id.as_deref(),
            Some("fs_main::return")
        );
        assert_eq!(
            document.dependency_rows[0].target_id.as_deref(),
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
            document.dependency_expanded_row_keys,
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
        let path = dependency_path_for_row_key(&document.dependency_rows, &a_row_key);

        document.dependency_expanded_row_keys = document
            .dependency_expandable_row_keys()
            .into_iter()
            .collect();
        document.focus_target_from_editor(a_id);

        let expected_expanded = path
            .iter()
            .take(path.len().saturating_sub(1))
            .cloned()
            .collect::<HashSet<_>>();
        assert_eq!(document.dependency_expanded_row_keys, expected_expanded);
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

        document.pending_dependency_reveal_row_key = None;
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
            document.focused_dependency_row_key.as_deref(),
            Some(row_key.as_str())
        );
        assert_eq!(document.pending_dependency_reveal_row_key, None);
    }

    #[test]
    fn focusing_target_outside_current_map_does_not_move_root() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) vec4f { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; let outside = 9.0; return vec4f(c, c, c, 1.0); }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let root_id = document.dependency_root_target_id.clone().unwrap();
        let outside_id = target_id_by_name(&document, "outside");

        document.focus_target(outside_id.clone(), true);

        assert_eq!(
            document.dependency_root_target_id.as_deref(),
            Some(root_id.as_str())
        );
        assert_eq!(
            document.focused_target_id.as_deref(),
            Some(outside_id.as_str())
        );
        assert_eq!(document.focused_dependency_row_key, None);
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
                target_id: Some("target::x".to_string()),
                children: vec![PassDebugDependencyNode {
                    label: "[rhs] Binary Add".to_string(),
                    edge_label: None,
                    display_label: None,
                    source_range: None,
                    target_id: None,
                    children: vec![
                        PassDebugDependencyNode {
                            label: "[source] function argument fs_main::0".to_string(),
                            edge_label: None,
                            display_label: None,
                            source_range: None,
                            target_id: None,
                            children: Vec::new(),
                        },
                        PassDebugDependencyNode {
                            label: "fs_main uv (argument)".to_string(),
                            edge_label: None,
                            display_label: None,
                            source_range: None,
                            target_id: Some("target::uv".to_string()),
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
                target_id: Some("target::d".to_string()),
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
                    target_id: Some("target::a".to_string()),
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
        let a_row = dependency_row_by_label(&document.dependency_rows, "a (bar)").clone();

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
            .pending_editor_jump
            .expect("expected dependency click to queue editor jump");
        let expected_start = document.draft_source.find("bar(a, c)").unwrap() + "bar(".len();
        assert_eq!(jump.start_byte, expected_start);
        assert_eq!(&document.draft_source[jump.start_byte..jump.end_byte], "a");
        assert_eq!(
            document.focused_dependency_row_key.as_deref(),
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
        let a_row = dependency_row_by_label(&document.dependency_rows, "a (bar)").clone();
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
            .pending_editor_jump
            .expect("expected source jump to queue editor jump");
        let expected_start = document.draft_source.find("let a = foo").unwrap() + "let ".len();
        assert_eq!(jump.start_byte, expected_start);
        assert_eq!(&document.draft_source[jump.start_byte..jump.end_byte], "a");
        assert_eq!(
            document.focused_dependency_row_key.as_deref(),
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
            dependency_row_by_label(&document.dependency_rows, "final_alpha (Multiply)").clone();

        let occurrence_start =
            document.draft_source.find("0.5 * final_alpha").unwrap() + "0.5 * ".len();
        let row_range = final_alpha_row
            .source_range
            .expect("expected final_alpha row occurrence range");
        assert_eq!(row_range.start_byte, occurrence_start);
        assert_eq!(
            &document.draft_source[row_range.start_byte..row_range.end_byte],
            "final_alpha"
        );
        assert!(
            document.dependency_rows.iter().any(|row| {
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
            .pending_editor_jump
            .expect("expected final_alpha row click to queue editor jump");
        assert_eq!(jump.start_byte, occurrence_start);
        assert_eq!(
            &document.draft_source[jump.start_byte..jump.end_byte],
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
            .pending_editor_jump
            .expect("expected src jump to queue editor jump");
        let declaration_start =
            document.draft_source.find("var final_alpha").unwrap() + "var ".len();
        assert_eq!(jump.start_byte, declaration_start);
        assert_eq!(
            &document.draft_source[jump.start_byte..jump.end_byte],
            "final_alpha"
        );
    }

    #[test]
    fn reassigned_local_row_click_jumps_to_store_and_src_jumps_to_declaration() {
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
        let latest_store_start = document.draft_source.rfind("x = foo(x);").unwrap();
        let latest_x_row = document
            .dependency_rows
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
            .pending_editor_jump
            .expect("expected reassignment row click to jump to store");
        assert_eq!(jump.start_byte, latest_store_start);
        assert_eq!(&document.draft_source[jump.start_byte..jump.end_byte], "x");

        let source_jump_range = latest_x_row
            .source_jump_range
            .expect("expected reassignment row to expose declaration jump");
        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(latest_x_row.row_key.clone()),
                target_id: None,
                source_range: Some(source_jump_range),
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .pending_editor_jump
            .expect("expected src jump to go to declaration");
        let declaration_start = document.draft_source.find("var x").unwrap() + "var ".len();
        assert_eq!(jump.start_byte, declaration_start);
        assert_eq!(&document.draft_source[jump.start_byte..jump.end_byte], "x");
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
        let reference_start = document.draft_source.find("bar(a, c)").unwrap() + "bar(".len();
        let reference_char_index =
            byte_index_to_char_index(&document.draft_source, reference_start);

        document.focus_target_at_char_index(reference_char_index);

        let focused_row = document
            .focused_dependency_row_key
            .as_deref()
            .and_then(|row_key| {
                document
                    .dependency_rows
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
            &document.draft_source[focused_range.start_byte..focused_range.end_byte],
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
        let a_id = target_id_by_name(&document, "a");
        let a_rows = document
            .dependency_rows
            .iter()
            .filter(|row| row.label == "a (bar)" && row.target_id.as_deref() == Some(&a_id))
            .collect::<Vec<_>>();
        assert_eq!(a_rows.len(), 2);
        assert_ne!(a_rows[0].row_key, a_rows[1].row_key);
        assert_eq!(a_rows[0].target_id, a_rows[1].target_id);

        let call_start = document.draft_source.find("bar(a, a)").unwrap();
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
                target_id: Some("target::c".to_string()),
                children: vec![PassDebugDependencyNode {
                    label: "[value] named expression".to_string(),
                    edge_label: None,
                    display_label: None,
                    source_range: None,
                    target_id: None,
                    children: vec![PassDebugDependencyNode {
                        label: "mid b (let)".to_string(),
                        edge_label: None,
                        display_label: None,
                        source_range: None,
                        target_id: Some("target::b".to_string()),
                        children: vec![PassDebugDependencyNode {
                            label: "[value] named expression".to_string(),
                            edge_label: None,
                            display_label: None,
                            source_range: None,
                            target_id: None,
                            children: vec![PassDebugDependencyNode {
                                label: "leaf a (local)".to_string(),
                                edge_label: None,
                                display_label: None,
                                source_range: None,
                                target_id: Some("target::a".to_string()),
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
        document.dependency_rows = vec![
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

        assert_eq!(document.focused_target_id.as_deref(), Some("target::a"));
        assert_eq!(document.focused_dependency_row_key.as_deref(), Some("0/1"));
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
        document.dependency_rows = vec![
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

        assert_eq!(document.focused_dependency_row_key.as_deref(), Some("0/0"));
        let focused_range = document
            .focused_source_range()
            .expect("expected focused access path range");
        assert_eq!(
            &document.draft_source[focused_range.start_byte..focused_range.end_byte],
            "input.foo.bar.x"
        );
    }

    #[test]
    fn parse_errors_do_not_clear_editable_source() {
        let source = PassDebugSource::from_wgsl("p", "fn a() -> f32 { return 1.0; }\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.draft_source = "fn nope() -> { return vec4f(1.0); }\n".to_string();
        document.dirty = true;
        document.refresh_draft_analysis();

        assert_eq!(
            document.draft_source,
            "fn nope() -> { return vec4f(1.0); }\n"
        );
        assert!(
            document
                .analysis_source
                .as_ref()
                .and_then(|source| source.parse_error.as_ref())
                .is_some()
        );
        assert_eq!(document.loaded_source, "fn a() -> f32 { return 1.0; }\n");
    }
}
