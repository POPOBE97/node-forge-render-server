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
};

const AST_PANEL_DEFAULT_WIDTH: f32 = 340.0;
const AST_PANEL_MIN_WIDTH: f32 = 220.0;
const AST_PANEL_MAX_WIDTH: f32 = 560.0;
const AST_SCROLL_CONTENT_WIDTH: f32 = 1800.0;
const AST_ROW_INDENT_WIDTH: f32 = 14.0;

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
    selectable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PassDebugDependencyRow {
    depth: usize,
    label: String,
    target_id: Option<String>,
    selectable: bool,
}

trait PassDebugTreeRow {
    fn depth(&self) -> usize;
    fn label(&self) -> &str;
    fn target_id(&self) -> Option<&str>;
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
    selected_target_id: Option<String>,
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
            selected_target_id: None,
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
        self.ensure_selected_target();
        self.refresh_dependency_rows();
    }

    fn ensure_selected_target(&mut self) {
        let Some(source) = self.analysis_source.as_ref() else {
            self.selected_target_id = None;
            return;
        };
        let selected_still_exists = self
            .selected_target_id
            .as_ref()
            .map(|selected| {
                source
                    .dependency_targets
                    .iter()
                    .any(|target| target.id == *selected)
            })
            .unwrap_or(false);
        if selected_still_exists {
            return;
        }
        self.selected_target_id = source
            .dependency_targets
            .iter()
            .find(|target| target_matches_search(target, &self.target_search))
            .or_else(|| source.dependency_targets.first())
            .map(|target| target.id.clone());
    }

    fn refresh_dependency_rows(&mut self) {
        self.dependency_rows = self
            .analysis_source
            .as_ref()
            .and_then(|source| {
                self.selected_target_id
                    .as_ref()
                    .and_then(|target_id| source.dependency_trees.get(target_id))
            })
            .map(flatten_dependency_tree)
            .unwrap_or_default();
    }

    fn select_target(&mut self, target_id: impl Into<String>) {
        let target_id = target_id.into();
        let Some(source) = self.analysis_source.as_ref() else {
            return;
        };
        if source
            .dependency_targets
            .iter()
            .any(|target| target.id == target_id)
        {
            self.selected_target_id = Some(target_id);
            self.side_panel_mode = PassDebugSidePanelMode::Dependencies;
            self.refresh_dependency_rows();
        }
    }

    fn select_first_target_matching_search(&mut self) {
        let Some(source) = self.analysis_source.as_ref() else {
            self.selected_target_id = None;
            self.refresh_dependency_rows();
            return;
        };
        if let Some(target) = source
            .dependency_targets
            .iter()
            .find(|target| target_matches_search(target, &self.target_search))
        {
            self.selected_target_id = Some(target.id.clone());
        }
        self.refresh_dependency_rows();
    }

    fn select_target_named(&mut self, name: &str) {
        let Some(source) = self.analysis_source.as_ref() else {
            return;
        };
        if let Some(target) = source
            .dependency_targets
            .iter()
            .find(|target| target.name == name)
        {
            self.select_target(target.id.clone());
        }
    }

    fn record_error(&mut self, error: String) {
        self.last_error = Some(error);
        self.last_status = None;
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

    if let Some(target_id) = render_scrollable_tree_rows(
        ui,
        egui::Id::new(("pass-debug-ast", document.pass_name.as_str())),
        &document.ast_rows,
        document.selected_target_id.as_deref(),
    ) {
        document.select_target(target_id);
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
        document.select_first_target_matching_search();
    }

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
                .monospace()
                .small(),
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
                    .selected_target_id
                    .as_ref()
                    .map(|selected| *selected == target.id)
                    .unwrap_or(false);
                let label = format!("{}  {}", target.scope, target.name);
                let response =
                    ui.selectable_label(selected, egui::RichText::new(label).monospace().small());
                if response.clicked() {
                    document.select_target(target.id);
                }
            }
        });

    ui.add_space(6.0);
    if document.dependency_rows.is_empty() {
        ui.label(
            egui::RichText::new("Select a dependency target")
                .monospace()
                .small(),
        );
        return;
    }

    render_dependency_rows(ui, document);
}

fn render_dependency_rows(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    if let Some(target_id) = render_scrollable_tree_rows(
        ui,
        egui::Id::new(("pass-debug-dependencies", document.pass_name.as_str())),
        &document.dependency_rows,
        document.selected_target_id.as_deref(),
    ) {
        document.select_target(target_id);
    }
}

fn render_scrollable_tree_rows<Row: PassDebugTreeRow>(
    ui: &mut egui::Ui,
    id: egui::Id,
    rows: &[Row],
    selected_target_id: Option<&str>,
) -> Option<String> {
    let row_height = ui.text_style_height(&egui::TextStyle::Small);
    let row_height_with_spacing = row_height + ui.spacing().item_spacing.y;
    let font_id = egui::TextStyle::Small.resolve(ui.style());
    let mut clicked_target: Option<String> = None;

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

            for row_index in min_row..max_row {
                let row = &rows[row_index];
                let row_top = content_origin.y + row_index as f32 * row_height_with_spacing;
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(content_origin.x, row_top),
                    egui::vec2(AST_SCROLL_CONTENT_WIDTH, row_height_with_spacing),
                );

                let selected = selected_target_id
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

                if response.clicked()
                    && let Some(target_id) = row.target_id()
                {
                    clicked_target = Some(target_id.to_string());
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

    clicked_target
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
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let theme = egui_extras::syntax_highlighting::CodeTheme::from_memory(ui.ctx(), ui.style());
        let mut layout_job = egui_extras::syntax_highlighting::highlight(
            ui.ctx(),
            ui.style(),
            &theme,
            buf.as_str(),
            "rust",
        );
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
                    .font(egui::TextStyle::Monospace)
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
                if (output.response.clicked() || output.response.changed())
                    && let Some(cursor_range) = output.cursor_range
                    && let Some(identifier) =
                        identifier_at_char_index(&document.draft_source, cursor_range.primary.index)
                {
                    document.select_target_named(&identifier);
                }
            });
    });
}

fn push_action(
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    action: PassDebugWindowAction,
) {
    if let Ok(mut pending) = pending_actions.lock() {
        pending.push(action);
    }
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
        label: node.label.clone(),
        target_id: node.target_id.clone(),
        selectable: node.target_id.is_some(),
    });
    for child in &node.children {
        push_ast_rows(child, depth + 1, rows);
    }
}

fn flatten_dependency_tree(root: &PassDebugDependencyNode) -> Vec<PassDebugDependencyRow> {
    let mut rows = Vec::new();
    push_dependency_rows(root, 0, &mut rows);
    rows
}

fn push_dependency_rows(
    node: &PassDebugDependencyNode,
    depth: usize,
    rows: &mut Vec<PassDebugDependencyRow>,
) {
    rows.push(PassDebugDependencyRow {
        depth,
        label: node.label.clone(),
        target_id: node.target_id.clone(),
        selectable: node.target_id.is_some(),
    });
    for child in &node.children {
        push_dependency_rows(child, depth + 1, rows);
    }
}

fn target_matches_search(target: &PassDebugDependencyTarget, search: &str) -> bool {
    let search = search.trim().to_ascii_lowercase();
    if search.is_empty() {
        return true;
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

fn is_wgsl_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_wgsl_identifier_char(ch: char) -> bool {
    is_wgsl_identifier_start(ch) || ch.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::PassDebugWindowDocument;
    use crate::renderer::PassDebugSource;

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
