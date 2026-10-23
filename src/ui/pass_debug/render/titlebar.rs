use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use rust_wgpu_fiber::eframe::egui;

use crate::ui::pass_debug::event::{PassDebugEvent, PassDebugWindowAction};
use crate::ui::pass_debug::selectors::titlebar_view;
use crate::ui::pass_debug_window::PassDebugWindowDocument;
use crate::ui::tool_window;

fn request_pass_debug_close(
    ui: &egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    close_requested: &AtomicBool,
    send_viewport_close: bool,
) {
    document.dispatch_event(PassDebugEvent::CloseRequested, Some(pending_actions));
    close_requested.store(true, Ordering::Relaxed);
    if send_viewport_close {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

pub(crate) fn render_pass_debug_titlebar(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    close_requested: &AtomicBool,
    send_viewport_close: bool,
) {
    let view = titlebar_view(document);
    let save_requested =
        ui.input(|input| input.modifiers.command && input.key_pressed(egui::Key::S));
    ui.horizontal(|ui| {
        tool_window::heading(ui, "Shader Debugger");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if tool_window::action(ui, "Close", true).clicked() {
                request_pass_debug_close(
                    ui,
                    document,
                    pending_actions,
                    close_requested,
                    send_viewport_close,
                );
            }
            if tool_window::action(ui, "Save", view.save_enabled)
                .on_hover_text("Save shader patch · Cmd/Ctrl+S")
                .clicked()
            {
                document.dispatch_event(PassDebugEvent::SaveRequested, Some(pending_actions));
            }
            ui.add_enabled_ui(view.diff_enabled, |ui| {
                if ui.selectable_label(view.diff_active, "Diff").clicked() {
                    document.dispatch_event(PassDebugEvent::ToggleShortwireDiff, None);
                }
            });
        });
    });
    ui.horizontal(|ui| {
        if let Some(target_size) = view.target_size_label.as_deref() {
            let response = tool_window::badge(ui, format!("RT {target_size}"));
            if let Some(target_texture) = view.target_texture_label.as_deref() {
                response.on_hover_text(target_texture);
            }
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(&view.pass_name)
                    .monospace()
                    .size(11.0)
                    .color(ui.visuals().weak_text_color()),
            )
            .truncate(),
        )
        .on_hover_text(&view.pass_name);
    });

    if save_requested && view.save_enabled {
        document.dispatch_event(PassDebugEvent::SaveRequested, Some(pending_actions));
    }
}
