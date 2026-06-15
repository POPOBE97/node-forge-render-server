use std::sync::{Arc, Mutex};

use rust_wgpu_fiber::eframe::egui;

use crate::ui::pass_debug::event::{PassDebugEvent, PassDebugWindowAction};
use crate::ui::pass_debug::render::fonts::{PASS_DEBUG_CODE_FONT_SIZE, pass_debug_mono_font};
use crate::ui::pass_debug::selectors::{merge_popup_view, merge_resolver_view};
use crate::ui::pass_debug_window::PassDebugWindowDocument;

pub(crate) fn render_merge_conflict_popups(
    ctx: &egui::Context,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    let view = merge_popup_view(document);
    if view.choice_popup_open {
        egui::Window::new("Shader patch conflict")
            .id(egui::Id::new((
                "pass-debug-merge-choice",
                view.pass_name.as_str(),
            )))
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label("Generated WGSL changed while a shortwire patch is applied.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Discard Shortwire Patch").clicked() {
                        document.dispatch_event(
                            PassDebugEvent::MergeUseIncoming,
                            Some(pending_actions),
                        );
                        document.dispatch_event(PassDebugEvent::MergeCloseConflictWindows, None);
                    }
                    if ui.button("Resolve Conflict").clicked() {
                        document.dispatch_event(PassDebugEvent::MergeOpenResolver, None);
                    }
                });
            });
    }

    if view.resolver_window_open {
        let mut open = true;
        egui::Window::new(format!("Resolve Shader Conflict - {}", view.pass_name))
            .id(egui::Id::new((
                "pass-debug-merge-resolver",
                view.pass_name.as_str(),
            )))
            .default_size(egui::vec2(1180.0, 720.0))
            .min_size(egui::vec2(760.0, 420.0))
            .open(&mut open)
            .show(ctx, |ui| {
                render_merge_conflict_resolver(ui, document, pending_actions);
            });
        if !open {
            document.dispatch_event(PassDebugEvent::MergeReopenChoicePopup, None);
        }
    }
}

fn render_merge_conflict_resolver(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    let Some(view) = merge_resolver_view(document) else {
        return;
    };

    ui.horizontal(|ui| {
        ui.heading("Resolve conflict");
        ui.label(
            egui::RichText::new("Base / Incoming / Local")
                .monospace()
                .small(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Cancel").clicked() {
                document.dispatch_event(PassDebugEvent::MergeCancelResolution, None);
            }
            if ui.button("Keep Local").clicked() {
                document.dispatch_event(PassDebugEvent::MergeKeepLocal, Some(pending_actions));
            }
            if ui.button("Use Incoming").clicked() {
                document.dispatch_event(PassDebugEvent::MergeUseIncoming, Some(pending_actions));
            }
            if ui.button("Apply Resolved").clicked() {
                document.dispatch_event(PassDebugEvent::MergeApplyResolved, Some(pending_actions));
            }
        });
    });

    ui.label(
        egui::RichText::new(format!("Automatic merge failed: {}", view.conflict_error))
            .monospace()
            .small(),
    );
    ui.add_space(6.0);

    ui.columns(3, |columns| {
        render_readonly_merge_panel(&mut columns[0], "Base", &view.base_source);
        render_readonly_merge_panel(&mut columns[1], "Incoming", &view.incoming_source);
        render_readonly_merge_panel(&mut columns[2], "Local Patch", &view.local_source);
    });

    ui.separator();
    ui.heading("Resolved");
    let mut resolved_source = view.resolved_source;
    let font_id = pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE);
    let response = ui.add(
        egui::TextEdit::multiline(&mut resolved_source)
            .id_salt(("pass-debug-merge-resolved", view.pass_name.as_str()))
            .font(font_id)
            .code_editor()
            .desired_rows(16)
            .desired_width(f32::INFINITY)
            .lock_focus(true),
    );
    if response.changed() {
        document.dispatch_event(
            PassDebugEvent::MergeResolvedEdited {
                source: resolved_source,
            },
            None,
        );
    }
}

fn render_readonly_merge_panel(ui: &mut egui::Ui, title: &str, source: &str) {
    ui.label(egui::RichText::new(title).monospace().strong());
    let mut text = source.to_string();
    ui.add(
        egui::TextEdit::multiline(&mut text)
            .font(pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE))
            .code_editor()
            .interactive(false)
            .desired_rows(12)
            .desired_width(f32::INFINITY),
    );
}
