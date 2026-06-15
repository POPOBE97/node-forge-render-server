use rust_wgpu_fiber::eframe::egui;

use crate::ui::pass_debug::event::PassDebugEvent;
use crate::ui::pass_debug::selectors::{PassDebugReferenceSelectorView, headers_view};
use crate::ui::pass_debug_window::PassDebugWindowDocument;

fn render_status_badge(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).monospace().small());
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
                .width(220.0)
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
    let mut panel_header_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-side-header", view.pass_name.as_str()))
            .max_rect(panel_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    panel_header_ui.set_clip_rect(panel_rect.intersect(ui.clip_rect()));
    panel_header_ui.label(egui::RichText::new("Deps Tree").strong());

    let mut current_header_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-editor-header", view.pass_name.as_str()))
            .max_rect(current_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    current_header_ui.set_clip_rect(current_rect.intersect(ui.clip_rect()));
    current_header_ui.label(egui::RichText::new("Pass Shader").strong());
    render_status_badge(&mut current_header_ui, view.shader_status.as_str());

    let mut reference_header_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-reference-header", view.pass_name.as_str()))
            .max_rect(reference_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    reference_header_ui.set_clip_rect(reference_rect.intersect(ui.clip_rect()));
    reference_header_ui.label(egui::RichText::new("Reference").strong());
    render_status_badge(&mut reference_header_ui, view.reference_status.as_str());
    reference_header_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let now_secs = ui.input(|input| input.time);
        if ui
            .add_enabled(
                view.reference_selector.can_reload,
                egui::Button::new("Reload"),
            )
            .clicked()
        {
            document.dispatch_event(PassDebugEvent::ReferenceReloadRequested { now_secs }, None);
        }
        if ui
            .add_enabled(view.reference_selector.can_open, egui::Button::new("Open"))
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
