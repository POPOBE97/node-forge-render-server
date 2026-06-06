use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use rust_wgpu_fiber::eframe::egui;

use crate::renderer::{
    PassDebugAstNode, PassDebugDependencyNode, PassDebugDependencyTarget, PassDebugSource,
    PassDebugSourceRange,
};

const AST_PANEL_DEFAULT_WIDTH: f32 = 340.0;
const AST_PANEL_MIN_WIDTH: f32 = 220.0;
const AST_PANEL_MAX_WIDTH: f32 = 560.0;
const AST_SCROLL_CONTENT_WIDTH: f32 = 1800.0;
const AST_ROW_INDENT_WIDTH: f32 = 14.0;
const PASS_DEBUG_TREE_FONT_SIZE: f32 = 13.0;
const PASS_DEBUG_CODE_FONT_SIZE: f32 = 14.0;

#[derive(Clone, Debug)]
pub enum PassDebugWindowAction {
    ApplyPatch { pass_name: String, source: String },
    ResetPatch { pass_name: String },
    ResetAllPatches,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PassDebugAstRow {
    depth: usize,
    label: String,
    target_id: Option<String>,
    source_range: Option<PassDebugSourceRange>,
    selectable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PassDebugDependencyRow {
    depth: usize,
    label: String,
    target_id: Option<String>,
    source_range: Option<PassDebugSourceRange>,
    selectable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PassDebugTreeClick {
    target_id: Option<String>,
    source_range: Option<PassDebugSourceRange>,
}

trait PassDebugTreeRow {
    fn depth(&self) -> usize;
    fn label(&self) -> &str;
    fn target_id(&self) -> Option<&str>;
    fn source_range(&self) -> Option<PassDebugSourceRange>;
    fn selectable(&self) -> bool;
}

impl PassDebugTreeRow for PassDebugAstRow {
    fn depth(&self) -> usize {
        self.depth
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn target_id(&self) -> Option<&str> {
        self.target_id.as_deref()
    }

    fn source_range(&self) -> Option<PassDebugSourceRange> {
        self.source_range
    }

    fn selectable(&self) -> bool {
        self.selectable
    }
}

impl PassDebugTreeRow for PassDebugDependencyRow {
    fn depth(&self) -> usize {
        self.depth
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn target_id(&self) -> Option<&str> {
        self.target_id.as_deref()
    }

    fn source_range(&self) -> Option<PassDebugSourceRange> {
        self.source_range
    }

    fn selectable(&self) -> bool {
        self.selectable
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PassDebugSidePanelMode {
    Ast,
    Dependencies,
}

#[derive(Clone, Debug)]
pub struct PassDebugWindowDocument {
    pub pass_name: String,
    pub source: Option<PassDebugSource>,
    analysis_source: Option<PassDebugSource>,
    analysis_source_text: String,
    source_revision: Option<u64>,
    ast_rows: Vec<PassDebugAstRow>,
    dependency_rows: Vec<PassDebugDependencyRow>,
    focused_target_id: Option<String>,
    dependency_root_target_id: Option<String>,
    pending_editor_jump: Option<PassDebugSourceRange>,
    pending_tree_reveal_target_id: Option<String>,
    target_search: String,
    side_panel_mode: PassDebugSidePanelMode,
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
            ast_rows: Vec::new(),
            dependency_rows: Vec::new(),
            focused_target_id: None,
            dependency_root_target_id: None,
            pending_editor_jump: None,
            pending_tree_reveal_target_id: None,
            target_search: String::new(),
            side_panel_mode: PassDebugSidePanelMode::Ast,
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
        self.ast_rows = self
            .analysis_source
            .as_ref()
            .map(|source| flatten_ast_tree(&source.ast_tree))
            .unwrap_or_default();
        self.ensure_navigation_targets();
        self.refresh_dependency_rows();
    }

    fn ensure_navigation_targets(&mut self) {
        let Some(source) = self.analysis_source.as_ref() else {
            self.focused_target_id = None;
            self.dependency_root_target_id = None;
            self.pending_editor_jump = None;
            self.pending_tree_reveal_target_id = None;
            return;
        };

        if !target_exists(source, self.dependency_root_target_id.as_deref()) {
            self.dependency_root_target_id = source
                .dependency_targets
                .first()
                .map(|target| target.id.clone());
        }

        if !target_exists(source, self.focused_target_id.as_deref()) {
            self.focused_target_id = self.dependency_root_target_id.clone().or_else(|| {
                source
                    .dependency_targets
                    .first()
                    .map(|target| target.id.clone())
            });
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
    }

    fn set_dependency_root(&mut self, target_id: impl Into<String>) {
        let target_id = target_id.into();
        let Some(source) = self.analysis_source.as_ref() else {
            return;
        };
        if target_exists(source, Some(&target_id)) {
            self.dependency_root_target_id = Some(target_id.clone());
            self.focus_target(target_id, true);
            self.refresh_dependency_rows();
        }
    }

    fn focus_target(&mut self, target_id: impl Into<String>, show_dependencies: bool) {
        self.focus_target_inner(target_id, show_dependencies, true);
    }

    fn focus_target_from_editor(&mut self, target_id: impl Into<String>) {
        self.focus_target_inner(target_id, true, false);
    }

    fn focus_target_inner(
        &mut self,
        target_id: impl Into<String>,
        show_dependencies: bool,
        jump_editor: bool,
    ) {
        let target_id = target_id.into();
        let Some(source) = self.analysis_source.as_ref() else {
            return;
        };
        if let Some(target) = source
            .dependency_targets
            .iter()
            .find(|target| target.id == target_id)
        {
            self.focused_target_id = Some(target_id.clone());
            self.pending_tree_reveal_target_id = Some(target_id);
            if jump_editor {
                self.pending_editor_jump = target.source_range;
            }
            if show_dependencies {
                self.side_panel_mode = PassDebugSidePanelMode::Dependencies;
            }
        }
    }

    fn focus_tree_click(&mut self, click: PassDebugTreeClick, show_dependencies: bool) {
        if let Some(target_id) = click.target_id {
            self.focus_target(target_id, show_dependencies);
        } else if let Some(source_range) = click.source_range {
            self.pending_editor_jump = Some(source_range);
        }
    }

    fn focus_first_target_matching_search(&mut self) {
        let Some(source) = self.analysis_source.as_ref() else {
            self.focused_target_id = None;
            self.pending_tree_reveal_target_id = None;
            return;
        };
        if let Some(target) = source
            .dependency_targets
            .iter()
            .find(|target| target_matches_search(target, &self.target_search))
        {
            self.focus_target(target.id.clone(), true);
        }
    }

    fn focus_target_at_char_index(&mut self, char_index: usize) {
        let byte_index = char_index_to_byte_index(&self.draft_source, char_index);
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

    fn set_focus_as_dependency_root(&mut self) {
        if let Some(target_id) = self.focused_target_id.clone() {
            self.set_dependency_root(target_id);
        }
    }

    fn focused_target_label(&self) -> Option<String> {
        self.target_label(self.focused_target_id.as_deref())
    }

    fn focused_source_range(&self) -> Option<PassDebugSourceRange> {
        let source = self.analysis_source.as_ref()?;
        let focused_target_id = self.focused_target_id.as_deref()?;
        source
            .dependency_targets
            .iter()
            .find(|target| target.id == focused_target_id)
            .and_then(|target| target.source_range)
    }

    fn dependency_root_label(&self) -> Option<String> {
        self.target_label(self.dependency_root_target_id.as_deref())
    }

    fn target_label(&self, target_id: Option<&str>) -> Option<String> {
        let source = self.analysis_source.as_ref()?;
        let target_id = target_id?;
        source
            .dependency_targets
            .iter()
            .find(|target| target.id == target_id)
            .map(|target| format!("{} {}", target.scope, target.name))
    }

    fn focus_is_in_dependency_root(&self) -> bool {
        let Some(focused_target_id) = self.focused_target_id.as_deref() else {
            return true;
        };
        self.dependency_rows
            .iter()
            .any(|row| row.target_id.as_deref() == Some(focused_target_id))
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

fn consume_tree_reveal_target<Row: PassDebugTreeRow>(
    pending_target_id: &mut Option<String>,
    rows: &[Row],
) -> Option<String> {
    let target_id = pending_target_id.clone()?;
    if rows
        .iter()
        .any(|row| row.target_id().map(|id| id == target_id).unwrap_or(false))
    {
        *pending_target_id = None;
        Some(target_id)
    } else {
        None
    }
}

pub struct PassDebugWindowState {
    pass_name: String,
    viewport_id: egui::ViewportId,
    document: Arc<Mutex<PassDebugWindowDocument>>,
    close_requested: Arc<AtomicBool>,
    pending_actions: Arc<Mutex<Vec<PassDebugWindowAction>>>,
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
        let title = format!("RenderPass Debug - {}", state.pass_name);
        let viewport_builder = egui::ViewportBuilder::default()
            .with_title(title.clone())
            .with_inner_size(egui::vec2(1180.0, 760.0))
            .with_min_inner_size(egui::vec2(640.0, 360.0));

        ctx.show_viewport_deferred(viewport_id, viewport_builder, move |ctx, class| {
            if ctx.input(|input| input.viewport().close_requested()) {
                close_requested.store(true, Ordering::Relaxed);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }

            match class {
                egui::ViewportClass::EmbeddedWindow => {
                    let mut open = true;
                    egui::Window::new(title.as_str())
                        .id(egui::Id::new(("pass-debug-embedded", title.as_str())))
                        .open(&mut open)
                        .default_size(egui::vec2(1180.0, 760.0))
                        .show(ctx, |ui| {
                            render_pass_debug_embedded_content(ui, &document, &pending_actions);
                        });
                    if !open {
                        close_requested.store(true, Ordering::Relaxed);
                    }
                }
                _ => {
                    render_pass_debug_viewport(ctx, &document, &pending_actions);
                }
            }
        });

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

fn render_pass_debug_viewport(
    ctx: &egui::Context,
    document: &Arc<Mutex<PassDebugWindowDocument>>,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    let pass_name = document
        .lock()
        .map(|document| document.pass_name.clone())
        .unwrap_or_else(|_| "unavailable".to_string());

    egui::TopBottomPanel::top(egui::Id::new(("pass-debug-toolbar", pass_name.as_str())))
        .resizable(false)
        .show(ctx, |ui| {
            let Ok(mut document) = document.lock() else {
                ui.label("Debug document unavailable");
                return;
            };
            render_pass_debug_toolbar(ui, &mut document, pending_actions);
            render_patch_messages(ui, &document);
        });

    egui::SidePanel::left(egui::Id::new(("pass-debug-ast-panel", pass_name.as_str())))
        .default_width(AST_PANEL_DEFAULT_WIDTH)
        .width_range(AST_PANEL_MIN_WIDTH..=AST_PANEL_MAX_WIDTH)
        .resizable(true)
        .show(ctx, |ui| {
            let Ok(mut document) = document.lock() else {
                ui.label("Debug document unavailable");
                return;
            };
            render_side_panel(ui, &mut document);
        });

    egui::CentralPanel::default().show(ctx, |ui| {
        let Ok(mut document) = document.lock() else {
            ui.label("Debug document unavailable");
            return;
        };
        if document.source.is_none() {
            render_missing_source_message(ui);
            return;
        }
        render_code_editor(ui, &mut document);
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
    render_ast_editor_split(ui, &mut document);
}

fn render_side_panel(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    ui.horizontal(|ui| {
        ui.selectable_value(
            &mut document.side_panel_mode,
            PassDebugSidePanelMode::Ast,
            "AST",
        );
        ui.selectable_value(
            &mut document.side_panel_mode,
            PassDebugSidePanelMode::Dependencies,
            "Dependencies",
        );
    });
    ui.add_space(4.0);

    match document.side_panel_mode {
        PassDebugSidePanelMode::Ast => render_ast_panel(ui, document),
        PassDebugSidePanelMode::Dependencies => render_dependency_panel(ui, document),
    }
}

fn render_ast_panel(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    ui.heading("WGSL AST");
    ui.label(
        egui::RichText::new(document.pass_name.as_str())
            .monospace()
            .small(),
    );

    let Some(source) = document.analysis_source.as_ref() else {
        ui.add_space(8.0);
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
    }

    if document.ast_rows.is_empty() {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("No AST rows").monospace().small());
        return;
    }

    let reveal_target_id = consume_tree_reveal_target(
        &mut document.pending_tree_reveal_target_id,
        &document.ast_rows,
    );
    if let Some(click) = render_scrollable_tree_rows(
        ui,
        egui::Id::new(("pass-debug-ast", document.pass_name.as_str())),
        &document.ast_rows,
        document.focused_target_id.as_deref(),
        reveal_target_id.as_deref(),
    ) {
        document.focus_tree_click(click, false);
    }
}

fn render_dependency_panel(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    ui.heading("WGSL Dependencies");
    ui.label(
        egui::RichText::new(document.pass_name.as_str())
            .monospace()
            .small(),
    );

    let Some(source) = document.analysis_source.as_ref() else {
        ui.add_space(8.0);
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
        ui.add_space(8.0);
        ui.colored_label(
            egui::Color32::from_rgb(255, 180, 120),
            "Dependency analysis failed",
        );
        ui.label(egui::RichText::new(error.as_str()).monospace().small());
        ui.add_space(8.0);
    }

    let search_response = ui.add(
        egui::TextEdit::singleline(&mut document.target_search)
            .hint_text("Search variable")
            .desired_width(f32::INFINITY),
    );
    if search_response.changed() {
        document.focus_first_target_matching_search();
    }

    if !document.target_search.trim().is_empty() {
        let matched_targets = document
            .analysis_source
            .as_ref()
            .map(|source| {
                source
                    .dependency_targets
                    .iter()
                    .filter(|target| target_matches_search(target, &document.target_search))
                    .take(24)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if matched_targets.is_empty() {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("No matching targets")
                    .font(pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE)),
            );
            return;
        }

        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt(("pass-debug-target-list", document.pass_name.as_str()))
            .max_height(128.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for target in matched_targets {
                    let selected = document
                        .focused_target_id
                        .as_ref()
                        .map(|selected| *selected == target.id)
                        .unwrap_or(false);
                    let label = format!("{}  {}", target.scope, target.name);
                    let response = ui.selectable_label(
                        selected,
                        egui::RichText::new(label)
                            .font(pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE)),
                    );
                    if response.clicked() {
                        document.focus_target(target.id, true);
                    }
                }
            });

        ui.add_space(6.0);
    } else {
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
    ui.horizontal_wrapped(|ui| {
        let root = document
            .dependency_root_label()
            .unwrap_or_else(|| "<none>".to_string());
        let focus = document
            .focused_target_label()
            .unwrap_or_else(|| "<none>".to_string());
        ui.label(
            egui::RichText::new(format!("Root: {root}"))
                .font(pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE)),
        );
        ui.separator();
        ui.label(
            egui::RichText::new(format!("Focus: {focus}"))
                .font(pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE)),
        );
        let can_set_root = document.focused_target_id.is_some()
            && document.focused_target_id != document.dependency_root_target_id;
        if ui
            .add_enabled(can_set_root, egui::Button::new("Set focus as root"))
            .clicked()
        {
            document.set_focus_as_dependency_root();
        }
    });

    if !document.focus_is_in_dependency_root() {
        ui.label(
            egui::RichText::new("Focus is outside the current dependency map")
                .font(pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE))
                .color(egui::Color32::from_rgb(255, 180, 120)),
        );
    }

    ui.add_space(6.0);
    let reveal_target_id = consume_tree_reveal_target(
        &mut document.pending_tree_reveal_target_id,
        &document.dependency_rows,
    );
    if let Some(click) = render_scrollable_tree_rows(
        ui,
        egui::Id::new(("pass-debug-dependencies", document.pass_name.as_str())),
        &document.dependency_rows,
        document.focused_target_id.as_deref(),
        reveal_target_id.as_deref(),
    ) {
        document.focus_tree_click(click, true);
    }
}

fn render_scrollable_tree_rows<Row: PassDebugTreeRow>(
    ui: &mut egui::Ui,
    id: egui::Id,
    rows: &[Row],
    focused_target_id: Option<&str>,
    reveal_target_id: Option<&str>,
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
            ui.set_min_size(egui::vec2(AST_SCROLL_CONTENT_WIDTH, total_height));

            let min_row = (viewport.min.y / row_height_with_spacing).floor().max(0.0) as usize;
            let max_row =
                ((viewport.max.y / row_height_with_spacing).ceil() as usize + 1).min(rows.len());
            let content_origin = ui.min_rect().min;

            if let Some(reveal_target_id) = reveal_target_id
                && let Some(row_index) = rows.iter().position(|row| {
                    row.target_id()
                        .map(|target_id| target_id == reveal_target_id)
                        .unwrap_or(false)
                })
            {
                let row_top = content_origin.y + row_index as f32 * row_height_with_spacing;
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(content_origin.x, row_top),
                    egui::vec2(AST_SCROLL_CONTENT_WIDTH, row_height_with_spacing),
                );
                ui.scroll_to_rect(row_rect, Some(egui::Align::Center));
            }

            for row_index in min_row..max_row {
                let row = &rows[row_index];
                let row_top = content_origin.y + row_index as f32 * row_height_with_spacing;
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(content_origin.x, row_top),
                    egui::vec2(AST_SCROLL_CONTENT_WIDTH, row_height_with_spacing),
                );

                let selected = focused_target_id
                    .zip(row.target_id())
                    .map(|(selected, target)| selected == target)
                    .unwrap_or(false);
                let response = if row.selectable() {
                    ui.interact(row_rect, id.with(row_index), egui::Sense::click())
                } else {
                    ui.interact(row_rect, id.with(row_index), egui::Sense::hover())
                };

                if selected {
                    ui.painter()
                        .rect_filled(row_rect, 0.0, ui.visuals().selection.bg_fill);
                } else if row.selectable() && response.hovered() {
                    ui.painter().rect_filled(
                        row_rect,
                        0.0,
                        ui.visuals().widgets.hovered.weak_bg_fill,
                    );
                }

                if response.clicked() && (row.target_id().is_some() || row.source_range().is_some())
                {
                    clicked_row = Some(PassDebugTreeClick {
                        target_id: row.target_id().map(str::to_string),
                        source_range: row.source_range(),
                    });
                }

                let indent = row.depth() as f32 * AST_ROW_INDENT_WIDTH;
                let text_color = if selected {
                    ui.visuals().selection.stroke.color
                } else {
                    ui.visuals().text_color()
                };
                let galley = ui.painter().layout_no_wrap(
                    row.label().to_string(),
                    font_id.clone(),
                    text_color,
                );
                let text_pos = egui::pos2(
                    row_rect.left() + indent,
                    row_rect.center().y - galley.size().y * 0.5,
                );
                ui.painter().galley(text_pos, galley, text_color);
            }
        });

    clicked_row
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

fn render_ast_editor_split(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    let panel_id = egui::Id::new(("pass-debug-ast-split", document.pass_name.as_str()));
    egui::SidePanel::left(panel_id)
        .default_width(AST_PANEL_DEFAULT_WIDTH)
        .width_range(AST_PANEL_MIN_WIDTH..=AST_PANEL_MAX_WIDTH)
        .resizable(true)
        .show_inside(ui, |ui| {
            render_side_panel(ui, document);
        });

    ui.allocate_ui_with_layout(
        ui.available_size(),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            render_code_editor(ui, document);
        },
    );
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

fn flatten_ast_tree(nodes: &[PassDebugAstNode]) -> Vec<PassDebugAstRow> {
    let mut rows = Vec::new();
    for node in nodes {
        push_ast_rows(node, 0, &mut rows);
    }
    rows
}

fn push_ast_rows(node: &PassDebugAstNode, depth: usize, rows: &mut Vec<PassDebugAstRow>) {
    rows.push(PassDebugAstRow {
        depth,
        label: clean_debug_tree_row_label(&node.label),
        target_id: node.target_id.clone(),
        source_range: node.source_range,
        selectable: node.target_id.is_some() || node.source_range.is_some(),
    });
    for child in &node.children {
        push_ast_rows(child, depth + 1, rows);
    }
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
    push_dependency_rows(root, source, 0, &mut rows);
    rows
}

fn push_dependency_rows(
    node: &PassDebugDependencyNode,
    source: &PassDebugSource,
    depth: usize,
    rows: &mut Vec<PassDebugDependencyRow>,
) {
    let child_depth = if node.target_id.is_some() {
        rows.push(PassDebugDependencyRow {
            depth,
            label: clean_debug_tree_row_label(&node.label),
            target_id: node.target_id.clone(),
            source_range: node
                .target_id
                .as_deref()
                .and_then(|target_id| target_source_range(source, target_id)),
            selectable: true,
        });
        depth + 1
    } else {
        depth
    };
    for child in &node.children {
        push_dependency_rows(child, source, child_depth, rows);
    }
}

fn target_source_range(source: &PassDebugSource, target_id: &str) -> Option<PassDebugSourceRange> {
    source
        .dependency_targets
        .iter()
        .find(|target| target.id == target_id)
        .and_then(|target| target.source_range)
}

fn target_matches_search(target: &PassDebugDependencyTarget, search: &str) -> bool {
    let search = search.trim().to_ascii_lowercase();
    if search.is_empty() {
        return false;
    }
    target.name.to_ascii_lowercase().contains(&search)
        || target.scope.to_ascii_lowercase().contains(&search)
        || target.kind.to_ascii_lowercase().contains(&search)
        || target.label.to_ascii_lowercase().contains(&search)
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
    use std::collections::HashMap;

    use super::{
        PassDebugWindowDocument, flatten_ast_tree, flatten_dependency_tree, target_matches_search,
    };
    use crate::renderer::{
        PassDebugAstNode, PassDebugDependencyNode, PassDebugDependencyTarget, PassDebugSource,
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
    fn ast_rows_are_flattened_once_per_source_update() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let initial_rows = document.ast_rows.len();

        let refreshed = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        document.update_source(Some(&refreshed), 1, false);

        assert!(initial_rows > 0);
        assert!(!document.ast_rows.is_empty());
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
            "fn f() -> f32 { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; return c; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let root_id = target_id_by_name(&document, "c");
        let child_id = target_id_by_name(&document, "b");

        document.set_dependency_root(root_id.clone());
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
    fn target_search_focuses_without_replacing_root_tree() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "fn f() -> f32 { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; return c; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let root_id = target_id_by_name(&document, "c");
        let child_id = target_id_by_name(&document, "b");

        document.set_dependency_root(root_id.clone());
        document.target_search = "b".to_string();
        document.focus_first_target_matching_search();

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
    fn ast_rows_strip_leading_naga_handle_noise() {
        let rows = flatten_ast_tree(&[PassDebugAstNode {
            label: "[12]: Binary Add".to_string(),
            target_id: Some("target::12".to_string()),
            role: Some("let".to_string()),
            source_range: None,
            children: vec![PassDebugAstNode {
                label: "[3] helper".to_string(),
                target_id: None,
                role: None,
                source_range: None,
                children: Vec::new(),
            }],
        }]);

        assert_eq!(rows[0].label, "Binary Add");
        assert_eq!(rows[0].target_id.as_deref(), Some("target::12"));
        assert_eq!(rows[1].label, "helper");
    }

    #[test]
    fn dependency_rows_hide_unselectable_intermediate_nodes() {
        let source = PassDebugSource {
            pass_name: "p".to_string(),
            module_source: String::new(),
            ast_tree: Vec::new(),
            dependency_targets: Vec::new(),
            dependency_trees: HashMap::new(),
            dependency_error: None,
            parse_error: None,
        };
        let rows = flatten_dependency_tree(
            &PassDebugDependencyNode {
                label: "fs_main x (local)".to_string(),
                target_id: Some("target::x".to_string()),
                children: vec![PassDebugDependencyNode {
                    label: "[rhs] Binary Add".to_string(),
                    target_id: None,
                    children: vec![
                        PassDebugDependencyNode {
                            label: "[source] function argument fs_main::0".to_string(),
                            target_id: None,
                            children: Vec::new(),
                        },
                        PassDebugDependencyNode {
                            label: "fs_main uv (argument)".to_string(),
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
        assert_eq!(rows[0].target_id.as_deref(), Some("target::x"));
        assert_eq!(rows[1].label, "fs_main uv (argument)");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[1].target_id.as_deref(), Some("target::uv"));
    }

    #[test]
    fn empty_target_search_matches_no_options() {
        let target = PassDebugDependencyTarget {
            id: "target::uv".to_string(),
            name: "uv".to_string(),
            label: "uv".to_string(),
            scope: "fs_main".to_string(),
            kind: "argument".to_string(),
            source_range: None,
        };

        assert!(!target_matches_search(&target, ""));
        assert!(!target_matches_search(&target, "   "));
        assert!(target_matches_search(&target, "uv"));
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
