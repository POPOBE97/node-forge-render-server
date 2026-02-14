//! Custom-painted file-tree widget for egui, styled after shadcn's collapsible file tree.
//!
//! All rendering is done with `egui::Painter` to give pixel-precise control over
//! row highlight, chevron, icon, label, and detail text.

use rust_wgpu_fiber::eframe::egui::{self, Color32, Pos2, Rect, Vec2};

use super::design_tokens::{self, TextRole};
use super::resource_tree::{FileTreeNode, NodeKind, TreeIcon};

// ---------------------------------------------------------------------------
// Style constants
// ---------------------------------------------------------------------------

const ROW_HEIGHT: f32 = 26.0;
const INDENT_PX: f32 = 16.0;
const LEFT_PAD: f32 = 4.0;
const CHEVRON_SIZE: f32 = 8.0;
const ICON_SIZE: f32 = 14.0;
const GAP_CHEVRON_ICON: f32 = 4.0;
const GAP_ICON_LABEL: f32 = 6.0;
const GAP_LABEL_DETAIL: f32 = 6.0;
const HOVER_RADIUS: f32 = 4.0;

// Colours
const COLOR_HOVER_BG: Color32 = Color32::from_gray(32);
const COLOR_SELECTED_BG: Color32 = design_tokens::RESOURCE_ACTIVE_BG;
const COLOR_ACCENT: Color32 = Color32::from_rgb(80, 140, 220);
const COLOR_CHEVRON: Color32 = Color32::from_gray(110);
const COLOR_ICON_FOLDER: Color32 = Color32::from_rgb(100, 140, 200);
const COLOR_ICON_PASS: Color32 = Color32::from_rgb(130, 180, 100);
const COLOR_ICON_TEXTURE: Color32 = Color32::from_rgb(200, 150, 80);
const COLOR_ICON_BUFFER: Color32 = Color32::from_rgb(160, 100, 160);
const COLOR_ICON_SAMPLER: Color32 = Color32::from_rgb(100, 160, 160);

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Persistent state for the file-tree widget.
#[derive(Default)]
pub struct FileTreeState {
    pub selected_id: Option<String>,
    pub keyboard_hover_id: Option<String>,
}

/// Result returned from `show_file_tree` each frame.
pub struct FileTreeResponse {
    pub clicked: Option<FileTreeNode>,
    pub copied_texture_name: Option<String>,
}

struct VisibleTreeEntry {
    node: FileTreeNode,
    parent_id: Option<String>,
    collapse_id: Option<egui::Id>,
    collapse_default_open: bool,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn show_file_tree(
    ui: &mut egui::Ui,
    nodes: &[FileTreeNode],
    state: &mut FileTreeState,
) -> FileTreeResponse {
    let mut response = FileTreeResponse {
        clicked: None,
        copied_texture_name: None,
    };
    let root_path = ui.id().with("file_tree_root");
    let mut visible_entries: Vec<VisibleTreeEntry> = Vec::new();

    for node in nodes {
        draw_node(
            ui,
            node,
            0,
            None,
            state,
            &mut response,
            &mut visible_entries,
            root_path,
        );
    }

    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.keyboard_hover_id = None;
    }

    let mut keyboard_hover_id = state
        .keyboard_hover_id
        .clone()
        .or_else(|| state.selected_id.clone())
        .or_else(|| visible_entries.first().map(|entry| entry.node.id.clone()));

    let keyboard_hover_index = keyboard_hover_id.as_ref().and_then(|id| {
        visible_entries
            .iter()
            .position(|entry| &entry.node.id == id)
    });

    if ui.input(|i| i.key_pressed(egui::Key::ArrowUp))
        && let Some(current) = keyboard_hover_index
        && current > 0
    {
        keyboard_hover_id = Some(visible_entries[current - 1].node.id.clone());
    }

    if ui.input(|i| i.key_pressed(egui::Key::ArrowDown))
        && let Some(current) = keyboard_hover_index
        && current + 1 < visible_entries.len()
    {
        keyboard_hover_id = Some(visible_entries[current + 1].node.id.clone());
    }

    let keyboard_hover_index = keyboard_hover_id.as_ref().and_then(|id| {
        visible_entries
            .iter()
            .position(|entry| &entry.node.id == id)
    });

    if ui.input(|i| i.key_pressed(egui::Key::ArrowLeft))
        && let Some(current) = keyboard_hover_index
    {
        let current_entry = &visible_entries[current];

        let collapse_target = current_entry
            .collapse_id
            .map(|id| (id, current_entry.collapse_default_open, false))
            .or_else(|| {
                current_entry.parent_id.as_ref().and_then(|parent_id| {
                    visible_entries
                        .iter()
                        .find(|entry| &entry.node.id == parent_id)
                        .and_then(|entry| {
                            entry
                                .collapse_id
                                .map(|id| (id, entry.collapse_default_open, true))
                        })
                })
            });

        if let Some((collapse_id, default_open, set_parent_hovered)) = collapse_target {
            let mut collapsing_state =
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    collapse_id,
                    default_open,
                );
            if collapsing_state.is_open() {
                collapsing_state.set_open(false);
                collapsing_state.store(ui.ctx());

                if set_parent_hovered && let Some(parent_id) = current_entry.parent_id.as_ref() {
                    keyboard_hover_id = Some(parent_id.clone());
                }
            }
        }
    }

    if ui.input(|i| i.key_pressed(egui::Key::ArrowRight))
        && let Some(current) = keyboard_hover_index
    {
        let current_entry = &visible_entries[current];

        let expand_target = current_entry
            .collapse_id
            .map(|id| (id, current_entry.collapse_default_open))
            .or_else(|| {
                current_entry.parent_id.as_ref().and_then(|parent_id| {
                    visible_entries
                        .iter()
                        .find(|entry| &entry.node.id == parent_id)
                        .and_then(|entry| {
                            entry
                                .collapse_id
                                .map(|id| (id, entry.collapse_default_open))
                        })
                })
            });

        if let Some((collapse_id, default_open)) = expand_target {
            let mut collapsing_state =
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    collapse_id,
                    default_open,
                );
            if !collapsing_state.is_open() {
                collapsing_state.set_open(true);
                collapsing_state.store(ui.ctx());
            }
        }
    }

    if ui.input(|i| i.key_pressed(egui::Key::Enter))
        && let Some(hover_id) = keyboard_hover_id.as_ref()
        && let Some(entry) = visible_entries
            .iter()
            .find(|entry| &entry.node.id == hover_id)
        && matches!(
            entry.node.kind,
            NodeKind::Pass {
                target_texture: Some(_)
            }
        )
    {
        state.selected_id = Some(entry.node.id.clone());
        response.clicked = Some(entry.node.clone());
    }

    state.keyboard_hover_id = keyboard_hover_id;

    response
}

// ---------------------------------------------------------------------------
// Recursive draw
// ---------------------------------------------------------------------------

fn draw_node(
    ui: &mut egui::Ui,
    node: &FileTreeNode,
    depth: usize,
    parent_id: Option<&str>,
    state: &mut FileTreeState,
    response: &mut FileTreeResponse,
    visible_entries: &mut Vec<VisibleTreeEntry>,
    path_hash: egui::Id,
) {
    let is_folder = !node.children.is_empty();
    let indent = LEFT_PAD + depth as f32 * INDENT_PX;

    // Use path_hash (accumulated from ancestors) to disambiguate duplicate nodes.
    let node_path = path_hash.with(&node.id);

    // --- Allocate row ---
    let available_width = ui.available_width();
    let row_rect = ui.allocate_space(Vec2::new(available_width, ROW_HEIGHT)).1;

    // --- Interaction ---
    let row_id = node_path.with("row");
    let row_response = ui.interact(row_rect, row_id, egui::Sense::click());

    if let NodeKind::Pass {
        target_texture: Some(tex_name),
    } = &node.kind
    {
        row_response.context_menu(|ui| {
            if ui.button("复制材质名").clicked() {
                response.copied_texture_name = Some(tex_name.clone());
                ui.close();
            }
        });
    }

    let hovered = row_response.hovered() || state.keyboard_hover_id.as_deref() == Some(&node.id);
    let is_selected = state.selected_id.as_deref() == Some(&node.id);

    let chevron_x = row_rect.min.x + indent;
    let chevron_rect = Rect::from_min_max(
        Pos2::new(chevron_x - 2.0, row_rect.min.y),
        Pos2::new(chevron_x + CHEVRON_SIZE + 2.0, row_rect.max.y),
    );
    let chevron_hovered = is_folder
        && row_response
            .hover_pos()
            .is_some_and(|pos| chevron_rect.contains(pos));
    if chevron_hovered {
        ui.output_mut(|o| {
            o.cursor_icon = egui::CursorIcon::PointingHand;
        });
    }
    let chevron_clicked = is_folder
        && row_response.clicked()
        && row_response
            .interact_pointer_pos()
            .is_some_and(|pos| chevron_rect.contains(pos));

    if row_response.clicked() && !chevron_clicked {
        state.selected_id = Some(node.id.clone());
        state.keyboard_hover_id = Some(node.id.clone());
        response.clicked = Some(node.clone());
    }

    let painter = ui.painter_at(row_rect);

    // --- Hover / selected background ---
    let row_inner = Rect::from_min_max(
        Pos2::new(row_rect.min.x + 2.0, row_rect.min.y),
        Pos2::new(row_rect.max.x - 2.0, row_rect.max.y),
    );
    if is_selected {
        // Accent bar on left edge.
        let bar = Rect::from_min_size(
            Pos2::new(row_inner.min.x, row_inner.min.y + 4.0),
            Vec2::new(2.0, row_inner.height() - 8.0),
        );
        painter.rect_filled(bar, egui::CornerRadius::same(1), COLOR_ACCENT);
        // Selected background with 2px gap after the accent bar.
        let bg_rect = Rect::from_min_max(
            Pos2::new(row_inner.min.x + 2.0 + 2.0, row_inner.min.y),
            row_inner.max,
        );
        painter.rect_filled(
            bg_rect,
            egui::CornerRadius::same(HOVER_RADIUS as u8),
            COLOR_SELECTED_BG,
        );
    } else if hovered {
        painter.rect_filled(
            row_inner,
            egui::CornerRadius::same(HOVER_RADIUS as u8),
            COLOR_HOVER_BG,
        );
    }

    // --- Cursor X ---
    let mut cx = chevron_x;
    let cy = row_rect.center().y;

    // --- Chevron (folders only) ---
    let collapse_default_open = node.id == "section.deps";
    let collapse_id = is_folder.then_some(node_path.with("collapse"));

    let openness = if is_folder {
        let collapsing_id = collapse_id.expect("folder must have collapse id");
        let mut collapsing_state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            collapsing_id,
            // Dependencies section defaults open.
            collapse_default_open,
        );

        if chevron_clicked {
            collapsing_state.toggle(ui);
        }

        let openness = collapsing_state.openness(ui.ctx());
        collapsing_state.store(ui.ctx());

        // Draw chevron triangle.
        draw_chevron(&painter, cx, cy, openness);
        cx += CHEVRON_SIZE + GAP_CHEVRON_ICON;
        Some(openness)
    } else {
        // Align with nodes that have chevrons.
        cx += CHEVRON_SIZE + GAP_CHEVRON_ICON;
        None
    };

    visible_entries.push(VisibleTreeEntry {
        node: node.clone(),
        parent_id: parent_id.map(ToOwned::to_owned),
        collapse_id,
        collapse_default_open,
    });

    // --- Icon ---
    // Only use folder icons for actual Folder nodes; passes with children keep their pass icon.
    let resolved_icon = match node.kind {
        NodeKind::Folder if is_folder => {
            if openness.unwrap_or(0.0) > 0.5 {
                TreeIcon::FolderOpen
            } else {
                TreeIcon::FolderClosed
            }
        }
        _ => node.icon,
    };
    draw_icon(&painter, cx, cy, resolved_icon);
    cx += ICON_SIZE + GAP_ICON_LABEL;

    // --- Label ---
    let label_role = if is_folder {
        TextRole::ActiveItemTitle
    } else {
        TextRole::InactiveItemTitle
    };
    let label_style = design_tokens::text_style(label_role);
    let label_color = label_style.color;
    let label_font = design_tokens::font_id(label_style.size, label_style.weight);

    // If the node has detail text, use no-wrap for the label and show detail after.
    // Otherwise let the label fill remaining space, truncating from the front with "…".
    let max_label_width = row_rect.max.x - cx - 4.0; // 4px right margin
    if node.detail.is_some() {
        let label_galley =
            painter.layout_no_wrap(node.label.clone(), label_font.clone(), label_color);
        let label_pos = Pos2::new(cx, cy - label_galley.size().y * 0.5);
        painter.galley(label_pos, label_galley.clone(), Color32::PLACEHOLDER);
        cx += label_galley.size().x + GAP_LABEL_DETAIL;
    } else {
        let display_label =
            truncate_label_front(ui, &node.label, &label_font, max_label_width.max(20.0));
        let label_galley = painter.layout_no_wrap(display_label, label_font, label_color);
        let label_pos = Pos2::new(cx, cy - label_galley.size().y * 0.5);
        painter.galley(label_pos, label_galley.clone(), Color32::PLACEHOLDER);
        cx += label_galley.size().x + GAP_LABEL_DETAIL;
    }

    // --- Detail text ---
    if let Some(detail) = &node.detail {
        let detail_style = design_tokens::text_style(TextRole::ValueLabel);
        let detail_font = design_tokens::font_id(detail_style.size, detail_style.weight);
        let detail_galley = painter.layout_no_wrap(detail.clone(), detail_font, detail_style.color);
        let detail_pos = Pos2::new(cx, cy - detail_galley.size().y * 0.5);
        painter.galley(detail_pos, detail_galley, Color32::PLACEHOLDER);
    }

    // --- Children (animated collapse) ---
    if let Some(openness) = openness {
        if openness > 0.0 {
            // Use a fading clip approach for smooth collapse animation.
            // We use a simple approach: render children indented, with alpha fade.
            let alpha = (openness * 255.0).round() as u8;
            if alpha > 0 {
                // Indent guide line.
                let guide_x = row_rect.min.x + indent + CHEVRON_SIZE * 0.5;
                let guide_top = row_rect.max.y;

                // Draw children within a faded group.
                if openness < 1.0 {
                    ui.set_opacity(openness);
                }

                for child in &node.children {
                    draw_node(
                        ui,
                        child,
                        depth + 1,
                        Some(node.id.as_str()),
                        state,
                        response,
                        visible_entries,
                        node_path,
                    );
                }

                if openness < 1.0 {
                    ui.set_opacity(1.0);
                }

                let child_end_y = ui.cursor().min.y;

                // Draw the indent guide line.
                if child_end_y > guide_top {
                    ui.painter().line_segment(
                        [
                            Pos2::new(guide_x, guide_top),
                            Pos2::new(guide_x, child_end_y - 4.0),
                        ],
                        egui::Stroke::new(1.0, Color32::from_gray(36)),
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Label truncation (front)
// ---------------------------------------------------------------------------

/// Truncate `text` from the front so it fits within `max_width` pixels,
/// prefixing with "…" when characters are removed.
fn truncate_label_front(ui: &egui::Ui, text: &str, font: &egui::FontId, max_width: f32) -> String {
    let painter = ui.painter();
    let measure = |s: String| -> f32 {
        painter
            .layout_no_wrap(s, font.clone(), Color32::PLACEHOLDER)
            .size()
            .x
    };

    let full_width = measure(text.to_string());
    if full_width <= max_width {
        return text.to_string();
    }

    // Binary search for how many chars to skip from the front.
    let chars: Vec<char> = text.chars().collect();
    let mut lo = 1usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let candidate: String = std::iter::once('…')
            .chain(chars[mid..].iter().copied())
            .collect();
        let w = measure(candidate);
        if w > max_width {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    std::iter::once('…')
        .chain(chars[lo..].iter().copied())
        .collect()
}

// ---------------------------------------------------------------------------
// Chevron drawing
// ---------------------------------------------------------------------------

fn draw_chevron(painter: &egui::Painter, x: f32, y: f32, openness: f32) {
    // Rotate from pointing-right (▶) to pointing-down (▼) based on openness.
    let half = CHEVRON_SIZE * 0.5;
    let center = Pos2::new(x + half, y);

    // Triangle vertices when pointing right.
    let angle = openness * std::f32::consts::FRAC_PI_2; // 0 → 90°
    let cos = angle.cos();
    let sin = angle.sin();

    let rotate = |dx: f32, dy: f32| -> Pos2 {
        Pos2::new(
            center.x + dx * cos - dy * sin,
            center.y + dx * sin + dy * cos,
        )
    };

    let s = half * 0.65;
    let points = vec![rotate(-s * 0.5, -s), rotate(s, 0.0), rotate(-s * 0.5, s)];

    painter.add(egui::Shape::convex_polygon(
        points,
        COLOR_CHEVRON,
        egui::Stroke::NONE,
    ));
}

// ---------------------------------------------------------------------------
// Icon drawing
// ---------------------------------------------------------------------------

fn draw_icon(painter: &egui::Painter, x: f32, y: f32, icon: TreeIcon) {
    let half = ICON_SIZE * 0.5;
    let center = Pos2::new(x + half, y);
    let rect = Rect::from_center_size(center, Vec2::splat(ICON_SIZE));

    match icon {
        TreeIcon::FolderClosed | TreeIcon::FolderOpen => {
            draw_folder_icon(painter, rect, icon == TreeIcon::FolderOpen);
        }
        TreeIcon::Pass => {
            // Filled circle.
            painter.circle_filled(center, half * 0.6, COLOR_ICON_PASS);
        }
        TreeIcon::Texture => {
            // Filled square with a diagonal line.
            let inner = rect.shrink(2.0);
            painter.rect_filled(inner, egui::CornerRadius::same(2), COLOR_ICON_TEXTURE);
            painter.line_segment(
                [inner.left_bottom(), inner.right_top()],
                egui::Stroke::new(1.0, Color32::from_rgba_premultiplied(0, 0, 0, 80)),
            );
        }
        TreeIcon::Buffer => {
            // Three horizontal bars.
            let bar_h = 2.0;
            let spacing = 3.5;
            let w = ICON_SIZE - 4.0;
            for i in 0..3 {
                let by = center.y - spacing + i as f32 * spacing;
                let bar_rect = Rect::from_min_size(
                    Pos2::new(center.x - w * 0.5, by - bar_h * 0.5),
                    Vec2::new(w, bar_h),
                );
                painter.rect_filled(bar_rect, egui::CornerRadius::same(1), COLOR_ICON_BUFFER);
            }
        }
        TreeIcon::Sampler => {
            // Diamond shape.
            let s = half * 0.65;
            let points = vec![
                Pos2::new(center.x, center.y - s),
                Pos2::new(center.x + s, center.y),
                Pos2::new(center.x, center.y + s),
                Pos2::new(center.x - s, center.y),
            ];
            painter.add(egui::Shape::convex_polygon(
                points,
                COLOR_ICON_SAMPLER,
                egui::Stroke::NONE,
            ));
        }
    }
}

fn draw_folder_icon(painter: &egui::Painter, rect: Rect, open: bool) {
    let r = rect.shrink(1.5);
    // Folder body.
    let body = Rect::from_min_max(Pos2::new(r.min.x, r.min.y + 3.0), r.max);
    painter.rect_filled(body, egui::CornerRadius::same(2), COLOR_ICON_FOLDER);

    // Folder tab.
    let tab = Rect::from_min_max(r.min, Pos2::new(r.min.x + r.width() * 0.45, r.min.y + 4.5));
    painter.rect_filled(
        tab,
        egui::CornerRadius {
            nw: 2,
            ne: 2,
            sw: 0,
            se: 0,
        },
        COLOR_ICON_FOLDER,
    );

    if open {
        // Small opening indicator — a darker inner rect.
        let inner = Rect::from_min_max(
            Pos2::new(body.min.x + 2.0, body.min.y + 2.0),
            Pos2::new(body.max.x - 2.0, body.max.y - 2.0),
        );
        painter.rect_filled(
            inner,
            egui::CornerRadius::same(1),
            Color32::from_rgba_premultiplied(0, 0, 0, 40),
        );
    }
}
