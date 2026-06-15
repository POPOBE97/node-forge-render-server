use std::{
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::Instant,
};

use rust_wgpu_fiber::eframe::egui;

use crate::metric_log;
use crate::ui::pass_debug::event::PassDebugWindowAction;
use crate::ui::pass_debug::render::dependency_panel::render_side_panel;
use crate::ui::pass_debug::render::editor::{
    render_current_editor_column, render_reference_editor_column,
};
use crate::ui::pass_debug::render::headers::render_pass_debug_column_headers;
use crate::ui::pass_debug::render::titlebar::render_pass_debug_titlebar;
use crate::ui::pass_debug::selectors::root_view;
use crate::ui::pass_debug_window::PassDebugWindowDocument;

const SIDE_PANEL_DEFAULT_WIDTH: f32 = 340.0;
const SIDE_PANEL_MIN_WIDTH: f32 = 220.0;
const SIDE_PANEL_MAX_WIDTH: f32 = 560.0;
const PASS_DEBUG_SPLIT_HANDLE_WIDTH: f32 = 6.0;
const PASS_DEBUG_SPLIT_LINE_WIDTH: f32 = 1.0;
const PASS_DEBUG_EDITOR_MIN_WIDTH: f32 = 320.0;
const PASS_DEBUG_COLUMN_HEADER_HEIGHT: f32 = 28.0;

pub(crate) fn render_pass_debug_viewport(
    ui: &mut egui::Ui,
    document: &Arc<Mutex<PassDebugWindowDocument>>,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    close_requested: &AtomicBool,
) {
    let pass_name = document
        .lock()
        .map(|document| document.pass_name.clone())
        .unwrap_or_else(|_| "unavailable".to_string());

    let t_central = Instant::now();
    egui::CentralPanel::default().show_inside(ui, |ui| {
        let Ok(mut document) = document.lock() else {
            ui.label("Debug document unavailable");
            return;
        };
        if !root_view(&document).source_available {
            render_missing_source_message(ui);
            return;
        }
        render_dependency_editor_split(ui, &mut document, pending_actions, close_requested, true);
    });
    let central_dur = t_central.elapsed();

    metric_log!(
        "[pass-debug] viewport-inner pass={} central_panel={:.2}ms",
        pass_name,
        central_dur.as_secs_f64() * 1000.0,
    );
}

pub(crate) fn render_pass_debug_embedded_content(
    ui: &mut egui::Ui,
    document: &Arc<Mutex<PassDebugWindowDocument>>,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    close_requested: &AtomicBool,
) {
    let Ok(mut document) = document.lock() else {
        ui.label("Debug document unavailable");
        return;
    };

    if !root_view(&document).source_available {
        ui.add_space(8.0);
        render_missing_source_message(ui);
        return;
    }

    render_dependency_editor_split(ui, &mut document, pending_actions, close_requested, false);
}

fn render_missing_source_message(ui: &mut egui::Ui) {
    ui.colored_label(
        egui::Color32::from_rgb(255, 180, 120),
        "Pass no longer exists in the current scene.",
    );
}

fn render_dependency_editor_split(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    close_requested: &AtomicBool,
    send_viewport_close: bool,
) {
    render_pass_debug_titlebar(
        ui,
        document,
        pending_actions,
        close_requested,
        send_viewport_close,
    );
    ui.separator();

    let full_rect = ui.available_rect_before_wrap();
    if full_rect.width() <= 0.0 || full_rect.height() <= PASS_DEBUG_COLUMN_HEADER_HEIGHT {
        return;
    }
    let header_height = PASS_DEBUG_COLUMN_HEADER_HEIGHT.min(full_rect.height());
    let body_rect = egui::Rect::from_min_max(
        egui::pos2(full_rect.left(), full_rect.top() + header_height),
        full_rect.right_bottom(),
    );

    let tree_split_id = egui::Id::new(("pass-debug-split-width", document.pass_name.as_str()));
    let editor_split_id =
        egui::Id::new(("pass-debug-editor-split-width", document.pass_name.as_str()));
    let available_for_panel = (body_rect.width() - PASS_DEBUG_SPLIT_HANDLE_WIDTH * 2.0).max(0.0);
    let max_panel_width = SIDE_PANEL_MAX_WIDTH
        .min(
            (available_for_panel - PASS_DEBUG_EDITOR_MIN_WIDTH * 2.0)
                .max(SIDE_PANEL_MIN_WIDTH)
                .min(available_for_panel),
        )
        .max(0.0);
    let min_panel_width = SIDE_PANEL_MIN_WIDTH.min(max_panel_width);
    let panel_width = ui
        .ctx()
        .data_mut(|data| {
            data.get_persisted::<f32>(tree_split_id)
                .unwrap_or(SIDE_PANEL_DEFAULT_WIDTH)
        })
        .clamp(min_panel_width, max_panel_width);

    let panel_rect = egui::Rect::from_min_max(
        body_rect.min,
        egui::pos2(body_rect.left() + panel_width, body_rect.bottom()),
    );
    let handle_rect = egui::Rect::from_min_max(
        egui::pos2(panel_rect.right(), full_rect.top()),
        egui::pos2(
            panel_rect.right() + PASS_DEBUG_SPLIT_HANDLE_WIDTH,
            full_rect.bottom(),
        ),
    );
    let editors_rect = egui::Rect::from_min_max(
        egui::pos2(handle_rect.right(), body_rect.top()),
        body_rect.right_bottom(),
    );

    let handle_response = ui.interact(
        handle_rect,
        tree_split_id.with("handle"),
        egui::Sense::click_and_drag(),
    );
    if handle_response.hovered() || handle_response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if handle_response.dragged() {
        let next_width =
            (panel_width + handle_response.drag_delta().x).clamp(min_panel_width, max_panel_width);
        ui.ctx()
            .data_mut(|data| data.insert_persisted(tree_split_id, next_width));
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

    let editors_available_width = (editors_rect.width() - PASS_DEBUG_SPLIT_HANDLE_WIDTH).max(0.0);
    let max_current_width = (editors_available_width - PASS_DEBUG_EDITOR_MIN_WIDTH)
        .max(PASS_DEBUG_EDITOR_MIN_WIDTH)
        .min(editors_available_width);
    let min_current_width = PASS_DEBUG_EDITOR_MIN_WIDTH.min(max_current_width);
    let current_width = ui
        .ctx()
        .data_mut(|data| {
            data.get_persisted::<f32>(editor_split_id)
                .unwrap_or(editors_available_width * 0.5)
        })
        .clamp(min_current_width, max_current_width);

    let current_rect = egui::Rect::from_min_max(
        editors_rect.min,
        egui::pos2(editors_rect.left() + current_width, editors_rect.bottom()),
    );
    let editor_handle_rect = egui::Rect::from_min_max(
        egui::pos2(current_rect.right(), full_rect.top()),
        egui::pos2(
            current_rect.right() + PASS_DEBUG_SPLIT_HANDLE_WIDTH,
            full_rect.bottom(),
        ),
    );
    let reference_rect = egui::Rect::from_min_max(
        egui::pos2(editor_handle_rect.right(), editors_rect.top()),
        editors_rect.right_bottom(),
    );

    let header_top = full_rect.top();
    let header_bottom = header_top + header_height;
    render_pass_debug_column_headers(
        ui,
        document,
        egui::Rect::from_min_max(
            egui::pos2(panel_rect.left(), header_top),
            egui::pos2(panel_rect.right(), header_bottom),
        ),
        egui::Rect::from_min_max(
            egui::pos2(current_rect.left(), header_top),
            egui::pos2(current_rect.right(), header_bottom),
        ),
        egui::Rect::from_min_max(
            egui::pos2(reference_rect.left(), header_top),
            egui::pos2(reference_rect.right(), header_bottom),
        ),
    );

    let editor_handle_response = ui.interact(
        editor_handle_rect,
        editor_split_id.with("handle"),
        egui::Sense::click_and_drag(),
    );
    if editor_handle_response.hovered() || editor_handle_response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if editor_handle_response.dragged() {
        let next_width = (current_width + editor_handle_response.drag_delta().x)
            .clamp(min_current_width, max_current_width);
        ui.ctx()
            .data_mut(|data| data.insert_persisted(editor_split_id, next_width));
        ui.ctx().request_repaint();
    }

    let editor_line_x = editor_handle_rect.center().x;
    let editor_line_color = if editor_handle_response.hovered() || editor_handle_response.dragged()
    {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    };
    ui.painter().rect_filled(
        egui::Rect::from_center_size(
            egui::pos2(editor_line_x, editor_handle_rect.center().y),
            egui::vec2(PASS_DEBUG_SPLIT_LINE_WIDTH, editor_handle_rect.height()),
        ),
        0.0,
        editor_line_color,
    );
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(full_rect.left(), header_bottom),
            egui::pos2(
                full_rect.right(),
                header_bottom + PASS_DEBUG_SPLIT_LINE_WIDTH,
            ),
        ),
        0.0,
        ui.visuals().widgets.noninteractive.bg_stroke.color,
    );

    let t_dep = Instant::now();
    let mut panel_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-side-child", document.pass_name.as_str()))
            .max_rect(panel_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    panel_ui.set_clip_rect(panel_rect.intersect(ui.clip_rect()));
    render_side_panel(&mut panel_ui, document, pending_actions);
    let dep_dur = t_dep.elapsed();

    let t_editor = Instant::now();
    let mut editor_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-editor-child", document.pass_name.as_str()))
            .max_rect(current_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    editor_ui.set_clip_rect(current_rect.intersect(ui.clip_rect()));
    render_current_editor_column(&mut editor_ui, document, pending_actions);
    let editor_dur = t_editor.elapsed();

    let t_reference = Instant::now();
    let mut reference_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-reference-child", document.pass_name.as_str()))
            .max_rect(reference_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    reference_ui.set_clip_rect(reference_rect.intersect(ui.clip_rect()));
    render_reference_editor_column(&mut reference_ui, document, pending_actions);
    let reference_dur = t_reference.elapsed();

    metric_log!(
        "[pass-debug] split pass={} dependency_panel={:.2}ms code_editor={:.2}ms reference_editor={:.2}ms",
        document.pass_name,
        dep_dur.as_secs_f64() * 1000.0,
        editor_dur.as_secs_f64() * 1000.0,
        reference_dur.as_secs_f64() * 1000.0,
    );

    ui.advance_cursor_after_rect(full_rect);
}
