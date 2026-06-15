use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use rust_wgpu_fiber::eframe::egui;

use crate::ui::pass_debug::dependency_tree::{
    PassDebugDependencyRow, PassDebugTreeClick, PassDebugTreeRow, filtered_dependency_row_indices,
};
use crate::ui::pass_debug::event::PassDebugEvent;
use crate::ui::pass_debug::render::fonts::{PASS_DEBUG_TREE_FONT_SIZE, pass_debug_mono_font};
use crate::ui::pass_debug::render::tree_paint::{
    dependency_path_color, paint_source_jump_button, paint_tree_toggle_symbol, shortwire_dot_color,
    shortwire_dot_hover_text, source_jump_button_size, tree_highlight_text_color,
    tree_hovered_row_bg, tree_selected_row_bg,
};
use crate::ui::pass_debug::selectors::{
    PassDebugDependencyPanelStatus, dependency_panel_status, dependency_rows_view,
};
use crate::ui::pass_debug::shortwire::ShortwireDotInfo;
use crate::ui::pass_debug_window::{PassDebugWindowAction, PassDebugWindowDocument};

const TREE_ROW_INDENT_WIDTH: f32 = 14.0;
const TREE_ROW_SOURCE_JUMP_GAP: f32 = 8.0;

pub(crate) fn render_side_panel(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    render_dependency_panel(ui, document, pending_actions);
}

fn render_dependency_panel(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    match dependency_panel_status(document) {
        PassDebugDependencyPanelStatus::MissingSource => {
            ui.colored_label(
                egui::Color32::from_rgb(255, 180, 120),
                "Pass no longer exists",
            );
        }
        PassDebugDependencyPanelStatus::ParseError(error) => {
            ui.add_space(8.0);
            ui.colored_label(egui::Color32::from_rgb(255, 118, 118), "WGSL parse failed");
            ui.label(egui::RichText::new(error.as_str()).monospace().small());
            ui.add_space(8.0);
        }
        PassDebugDependencyPanelStatus::DependencyError(error) => {
            ui.colored_label(
                egui::Color32::from_rgb(255, 180, 120),
                "Dependency analysis failed",
            );
            ui.label(egui::RichText::new(error.as_str()).monospace().small());
            ui.add_space(8.0);
            let view = dependency_rows_view(document);
            if view.rows.is_empty() {
                render_empty_dependency_message(ui);
            } else {
                render_dependency_rows(ui, document, pending_actions, view);
            }
        }
        PassDebugDependencyPanelStatus::Empty => render_empty_dependency_message(ui),
        PassDebugDependencyPanelStatus::Ready => {
            let view = dependency_rows_view(document);
            render_dependency_rows(ui, document, pending_actions, view);
        }
    }
}

fn render_empty_dependency_message(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("Select a dependency target")
            .font(pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE)),
    );
}

fn render_dependency_rows(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    mut view: crate::ui::pass_debug::selectors::PassDebugDependencyRowsView,
) {
    if !view.focus_is_in_dependency_root {
        ui.label(
            egui::RichText::new("Focus is outside the current dependency map")
                .font(pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE))
                .color(egui::Color32::from_rgb(255, 180, 120)),
        );
    }

    let filter_font = pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE);
    let mut filter_text = view.filter_text.clone();
    let filter_response = ui.add(
        egui::TextEdit::singleline(&mut filter_text)
            .font(filter_font)
            .hint_text("Filter...")
            .desired_width(ui.available_width()),
    );
    if filter_response.changed() {
        document.dispatch_event(
            PassDebugEvent::DependencyFilterEdited {
                text: filter_text.clone(),
            },
            None,
        );
        view.filter_text = filter_text.clone();
    }
    ui.add_space(4.0);

    let reveal_row_key = document.consume_dependency_reveal_row_key();
    let path_row_keys = document.dependency_focus_path_row_keys();
    let mut visible_dependency_row_indices =
        document.cached_visible_dependency_row_indices().to_vec();

    if let Some(filtered_indices) = filtered_dependency_row_indices(&view.rows, &view.filter_text) {
        visible_dependency_row_indices = filtered_indices;
    }
    let font_id = pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE);
    let content_width = document.cached_dependency_tree_intrinsic_width(ui, &font_id);
    let result = {
        let expandable_row_keys = document.cached_dependency_expandable_row_keys().clone();
        let tree_state = PassDebugTreeRenderState {
            focused_target_id: view.focused_target_id.as_deref(),
            focused_row_key: view.focused_row_key.as_deref(),
            reveal_row_key: reveal_row_key.as_deref(),
            path_row_keys: &path_row_keys,
            expandable_row_keys: Some(&expandable_row_keys),
            expanded_row_keys: Some(&view.expanded_row_keys),
            shortwire_active_row_key: view.shortwire_active_row_key.as_deref(),
            shortwire_can_enter: view.shortwire_can_enter,
            shortwire_dot_info: &view.shortwire_dot_info,
        };
        render_scrollable_tree_rows(
            ui,
            egui::Id::new(("pass-debug-dependencies", view.pass_name.as_str())),
            &view.rows,
            &visible_dependency_row_indices,
            &tree_state,
            &font_id,
            content_width,
        )
    };
    if let Some(click) = result.click {
        document.dispatch_event(
            PassDebugEvent::DependencyTreeClicked { click },
            Some(pending_actions),
        );
    }
    if let Some(row_idx) = result.context_menu_row_index {
        document.dispatch_event(
            PassDebugEvent::DependencyShortwireRequested { row_index: row_idx },
            Some(pending_actions),
        );
    }
}

struct PassDebugTreeRenderState<'a> {
    focused_target_id: Option<&'a str>,
    focused_row_key: Option<&'a str>,
    reveal_row_key: Option<&'a str>,
    path_row_keys: &'a [String],
    expandable_row_keys: Option<&'a HashSet<String>>,
    expanded_row_keys: Option<&'a HashSet<String>>,
    shortwire_active_row_key: Option<&'a str>,
    shortwire_can_enter: bool,
    shortwire_dot_info: &'a HashMap<String, ShortwireDotInfo>,
}

struct ShortwireTreeResult {
    click: Option<PassDebugTreeClick>,
    context_menu_row_index: Option<usize>,
}

fn render_scrollable_tree_rows(
    ui: &mut egui::Ui,
    id: egui::Id,
    rows: &[PassDebugDependencyRow],
    row_indices: &[usize],
    tree_state: &PassDebugTreeRenderState<'_>,
    font_id: &egui::FontId,
    intrinsic_content_width: f32,
) -> ShortwireTreeResult {
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font_id));
    let row_height_with_spacing = row_height + ui.spacing().item_spacing.y;
    let mut clicked_row: Option<PassDebugTreeClick> = None;
    let mut context_menu_row_index: Option<usize> = None;
    let is_shortwire_active = tree_state.shortwire_active_row_key.is_some();

    egui::ScrollArea::both()
        .id_salt(id)
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let total_height = row_height_with_spacing * row_indices.len() as f32;
            let content_width = ui.available_width().max(intrinsic_content_width).max(0.0);
            ui.set_min_size(egui::vec2(content_width, total_height));

            let min_row = (viewport.min.y / row_height_with_spacing).floor().max(0.0) as usize;
            let max_row = ((viewport.max.y / row_height_with_spacing).ceil() as usize + 1)
                .min(row_indices.len());
            let content_origin = ui.min_rect().min;

            let reveal_row_index = tree_state.reveal_row_key.and_then(|reveal_row_key| {
                row_indices.iter().position(|row_index| {
                    rows[*row_index]
                        .row_key()
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
                let actual_row_index = row_indices[row_index];
                let row = &rows[actual_row_index];
                let row_top = content_origin.y + row_index as f32 * row_height_with_spacing;
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(content_origin.x, row_top),
                    egui::vec2(content_width, row_height_with_spacing),
                );

                let is_active_shortwire_row = tree_state
                    .shortwire_active_row_key
                    .zip(row.row_key())
                    .map(|(active, current)| active == current)
                    .unwrap_or(false);

                let row_alpha = if is_shortwire_active && !is_active_shortwire_row {
                    0.3
                } else {
                    1.0
                };

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

                if row.selectable() && !is_shortwire_active {
                    response.context_menu(|ui| {
                        let enabled = tree_state.shortwire_can_enter;
                        if ui
                            .add_enabled(enabled, egui::Button::new("Shortwire"))
                            .clicked()
                        {
                            context_menu_row_index = Some(actual_row_index);
                            ui.close();
                        }
                    });
                }

                let row_patch_key = crate::ui::pass_debug::shortwire::shortwire_patch_key(row);
                let shortwire_dot_info = row
                    .selectable()
                    .then(|| tree_state.shortwire_dot_info.get(&row_patch_key))
                    .flatten()
                    .copied();
                let mut hover_lines = Vec::new();
                if let Some(relation_path) = row.relation_path() {
                    hover_lines.push(format!("Path: {relation_path}"));
                }
                if let Some(dot_info) = shortwire_dot_info {
                    hover_lines.push(shortwire_dot_hover_text(dot_info));
                }
                let response = if hover_lines.is_empty() {
                    response
                } else {
                    response.on_hover_text(hover_lines.join("\n"))
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
                if is_active_shortwire_row {
                    ui.painter()
                        .rect_filled(row_rect, 0.0, tree_selected_row_bg(ui));
                } else if selected {
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

                let text_color = if selected || is_active_shortwire_row {
                    tree_highlight_text_color(ui)
                } else {
                    let base_color = ui.visuals().text_color();
                    if row_alpha < 1.0 {
                        let [r, g, b, _] = base_color.to_srgba_unmultiplied();
                        egui::Color32::from_rgba_unmultiplied(r, g, b, (255.0 * row_alpha) as u8)
                    } else {
                        base_color
                    }
                };
                let has_stored_patch = shortwire_dot_info.is_some();
                let dot_offset = if has_stored_patch { 8.0 } else { 0.0 };

                if let Some(dot_info) = shortwire_dot_info {
                    let dot_radius = 3.0;
                    let dot_center = egui::pos2(text_x + dot_radius, row_rect.center().y);
                    let dot_color = shortwire_dot_color(ui, dot_info.status, row_alpha);
                    ui.painter()
                        .circle_filled(dot_center, dot_radius, dot_color);
                }

                let galley = ui.painter().layout_no_wrap(
                    row.label().to_string(),
                    font_id.clone(),
                    text_color,
                );
                let text_pos = egui::pos2(
                    text_x + dot_offset,
                    row_rect.center().y - galley.size().y * 0.5,
                );
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

    ShortwireTreeResult {
        click: clicked_row,
        context_menu_row_index,
    }
}
