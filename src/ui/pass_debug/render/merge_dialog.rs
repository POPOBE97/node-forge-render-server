use std::sync::{Arc, Mutex};

use rust_wgpu_fiber::eframe::egui;

use crate::ui::pass_debug::event::{PassDebugEvent, PassDebugWindowAction};
use crate::ui::pass_debug::render::fonts::{PASS_DEBUG_CODE_FONT_SIZE, pass_debug_mono_font};
use crate::ui::pass_debug::selectors::{merge_popup_view, merge_resolver_view};
use crate::ui::pass_debug_window::PassDebugWindowDocument;
use crate::ui::tool_window;

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
            .frame(tool_window::dialog_frame(ctx))
            .show(ctx, |ui| {
                tool_window::apply(ui);
                ui.label("Generated WGSL changed while a shortwire patch is applied.");
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if tool_window::action(ui, "Discard Shortwire Patch", true).clicked() {
                        document.dispatch_event(
                            PassDebugEvent::MergeUseIncoming,
                            Some(pending_actions),
                        );
                        document.dispatch_event(PassDebugEvent::MergeCloseConflictWindows, None);
                    }
                    if tool_window::action(ui, "Resolve Conflict", true).clicked() {
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
            .frame(tool_window::dialog_frame(ctx))
            .show(ctx, |ui| {
                tool_window::apply(ui);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        render_merge_conflict_resolver(ui, document, pending_actions);
                    });
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

    tool_window::heading(ui, "Resolve conflict");
    tool_window::caption(
        ui,
        "Compare the generated shader with your local patch, then edit the result below.",
    );
    ui.add_space(tool_window::GAP);
    ui.horizontal_wrapped(|ui| {
        for (label, event) in [
            ("Apply Resolved", PassDebugEvent::MergeApplyResolved),
            ("Use Incoming", PassDebugEvent::MergeUseIncoming),
            ("Keep Local", PassDebugEvent::MergeKeepLocal),
            ("Cancel", PassDebugEvent::MergeCancelResolution),
        ] {
            if tool_window::action(ui, label, true).clicked() {
                document.dispatch_event(event, Some(pending_actions));
            }
        }
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
    tool_window::heading(ui, "Resolved");
    let mut resolved_source = view.resolved_source;
    let font_id = pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE);
    let response = egui::ScrollArea::both()
        .id_salt("merge-resolved-scroll")
        .max_height(220.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut resolved_source)
                    .id_salt(("pass-debug-merge-resolved", view.pass_name.as_str()))
                    .font(font_id)
                    .code_editor()
                    .desired_rows(16)
                    .desired_width(f32::INFINITY)
                    .lock_focus(true),
            )
        })
        .inner;
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
    tool_window::heading(ui, title);
    let mut text = source.to_string();
    egui::ScrollArea::both()
        .id_salt(("merge-source", title))
        .max_height(180.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .font(pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE))
                    .code_editor()
                    .interactive(false)
                    .desired_rows(12)
                    .desired_width(f32::INFINITY),
            );
        });
}
