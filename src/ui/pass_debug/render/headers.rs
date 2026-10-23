use rust_wgpu_fiber::eframe::egui;

use crate::ui::pass_debug::event::PassDebugEvent;
use crate::ui::pass_debug::selectors::{PassDebugReferenceSelectorView, headers_view};
use crate::ui::pass_debug_window::PassDebugWindowDocument;
use crate::ui::tool_window;

fn render_status_badge(ui: &mut egui::Ui, text: impl Into<String>) {
    tool_window::badge(ui, text);
}

fn render_reference_file_selector(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    selector: &PassDebugReferenceSelectorView,
    pass_name: &str,
) {
    let now_secs = ui.input(|input| input.time);

    ui.add_enabled_ui(
        !selector.shortwire_active && !selector.file_choices.is_empty(),
        |ui| {
            egui::ComboBox::from_id_salt(("pass-debug-reference-file", pass_name))
                .selected_text(selector.selected_label.as_str())
                .width(ui.available_width().max(60.0))
                .truncate()
                .show_ui(ui, |ui| {
                    for relative_path in selector.file_choices.iter().cloned() {
                        let selected =
                            selector.selected_file.as_deref() == Some(relative_path.as_str());
                        if ui
                            .selectable_label(selected, relative_path.as_str())
                            .clicked()
                        {
                            document.dispatch_event(
                                PassDebugEvent::ReferenceFileSelected {
                                    relative_path,
                                    now_secs,
                                },
                                None,
                            );
                        }
                    }
                });
        },
    );
}

pub(crate) fn render_pass_debug_column_headers(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    panel_rect: egui::Rect,
    current_rect: egui::Rect,
    reference_rect: egui::Rect,
) {
    let view = headers_view(document);
    let mut header = |rect: egui::Rect, id: &str| {
        ui.painter().rect_filled(rect, 0.0, tool_window::PANEL);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt((id, view.pass_name.as_str()))
                .max_rect(rect.shrink2(egui::vec2(8.0, 6.0)))
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        child
    };
    let mut panel = header(panel_rect, "pass-debug-side-header");
    panel.horizontal(|ui| {
        ui.set_min_height(tool_window::ROW_HEIGHT - 4.0);
        tool_window::heading(ui, "Dependencies");
    });
    tool_window::caption(&mut panel, "Select a value to trace its inputs");

    let mut current = header(current_rect, "pass-debug-editor-header");
    current.horizontal(|ui| {
        ui.set_min_height(tool_window::ROW_HEIGHT - 4.0);
        tool_window::heading(ui, "Pass Shader");
        render_status_badge(ui, &view.shader_status);
    });
    tool_window::caption(&mut current, "Editable WGSL · Cmd/Ctrl+S to save");

    let mut reference = header(reference_rect, "pass-debug-reference-header");
    reference.horizontal(|ui| {
        ui.set_min_height(tool_window::ROW_HEIGHT - 4.0);
        tool_window::heading(ui, "Reference");
        render_status_badge(ui, &view.reference_status);
    });
    reference.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let now_secs = ui.input(|input| input.time);
        if tool_window::action(ui, "Reload", view.reference_selector.can_reload).clicked() {
            document.dispatch_event(PassDebugEvent::ReferenceReloadRequested { now_secs }, None);
        }
        if tool_window::action(ui, "Open", view.reference_selector.can_open)
            .on_hover_text("Open reference folder")
            .clicked()
        {
            document.dispatch_event(
                PassDebugEvent::ReferenceOpenFolderRequested { now_secs },
                None,
            );
        }
        render_reference_file_selector(ui, document, &view.reference_selector, &view.pass_name);
    });
}
