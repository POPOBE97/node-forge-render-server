use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use rust_wgpu_fiber::eframe::egui;

use crate::renderer::{PassDebugAstNode, PassDebugSource};

#[derive(Clone, Debug)]
pub struct PassDebugWindowDocument {
    pub pass_name: String,
    pub source: Option<PassDebugSource>,
}

pub struct PassDebugWindowState {
    pass_name: String,
    viewport_id: egui::ViewportId,
    document: Arc<Mutex<PassDebugWindowDocument>>,
    close_requested: Arc<AtomicBool>,
    focus_requested: bool,
}

impl PassDebugWindowState {
    fn new(pass_name: String, source: Option<PassDebugSource>) -> Self {
        let viewport_id = egui::ViewportId::from_hash_of(("pass-debug", pass_name.as_str()));
        Self {
            document: Arc::new(Mutex::new(PassDebugWindowDocument {
                pass_name: pass_name.clone(),
                source,
            })),
            close_requested: Arc::new(AtomicBool::new(false)),
            pass_name,
            viewport_id,
            focus_requested: true,
        }
    }

    fn update_source(&self, source: Option<PassDebugSource>) {
        if let Ok(mut document) = self.document.lock() {
            document.source = source;
        }
    }
}

pub type PassDebugWindowMap = HashMap<String, PassDebugWindowState>;

pub fn open_pass_debug_window(
    windows: &mut PassDebugWindowMap,
    pass_sources: &HashMap<String, PassDebugSource>,
    pass_name: String,
) {
    let source = pass_sources.get(pass_name.as_str()).cloned();
    if let Some(existing) = windows.get_mut(pass_name.as_str()) {
        existing.update_source(source);
        existing.focus_requested = true;
        existing.close_requested.store(false, Ordering::Relaxed);
        return;
    }

    windows.insert(
        pass_name.clone(),
        PassDebugWindowState::new(pass_name, source),
    );
}

pub fn show_pass_debug_windows(
    ctx: &egui::Context,
    windows: &mut PassDebugWindowMap,
    pass_sources: &HashMap<String, PassDebugSource>,
) {
    windows.retain(|_, state| !state.close_requested.load(Ordering::Relaxed));

    for state in windows.values_mut() {
        state.update_source(pass_sources.get(state.pass_name.as_str()).cloned());

        let viewport_id = state.viewport_id;
        let document = Arc::clone(&state.document);
        let close_requested = Arc::clone(&state.close_requested);
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
                egui::ViewportClass::Embedded => {
                    let mut open = true;
                    egui::Window::new(title.as_str())
                        .id(egui::Id::new(("pass-debug-embedded", title.as_str())))
                        .open(&mut open)
                        .default_size(egui::vec2(1180.0, 760.0))
                        .show(ctx, |ui| {
                            render_pass_debug_content(ui, &document);
                        });
                    if !open {
                        close_requested.store(true, Ordering::Relaxed);
                    }
                }
                _ => {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        render_pass_debug_content(ui, &document);
                    });
                }
            }
        });

        if state.focus_requested {
            ctx.send_viewport_cmd_to(state.viewport_id, egui::ViewportCommand::Focus);
            state.focus_requested = false;
        }
    }
}

fn render_pass_debug_content(ui: &mut egui::Ui, document: &Arc<Mutex<PassDebugWindowDocument>>) {
    let document = document.lock().ok().map(|guard| guard.clone());
    let Some(document) = document else {
        ui.label("Debug document unavailable");
        return;
    };

    let available = ui.available_size();
    let left_width = (available.x * 0.34).clamp(240.0, 440.0);
    let right_width = (available.x - left_width - 12.0).max(240.0);

    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(left_width, available.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                render_ast_panel(ui, &document);
            },
        );

        ui.separator();

        ui.allocate_ui_with_layout(
            egui::vec2(right_width, available.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                render_source_panel(ui, &document);
            },
        );
    });
}

fn render_ast_panel(ui: &mut egui::Ui, document: &PassDebugWindowDocument) {
    ui.heading("WGSL AST");
    ui.label(
        egui::RichText::new(document.pass_name.as_str())
            .monospace()
            .small(),
    );

    let Some(source) = document.source.as_ref() else {
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

    egui::ScrollArea::both()
        .id_salt(("pass-debug-ast", document.pass_name.as_str()))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, node) in source.ast_tree.iter().enumerate() {
                ui.push_id(index, |ui| {
                    render_ast_node(ui, node, 0);
                });
            }
        });
}

fn render_ast_node(ui: &mut egui::Ui, node: &PassDebugAstNode, depth: usize) {
    if node.children.is_empty() {
        ui.label(egui::RichText::new(node.label.as_str()).monospace().small());
        return;
    }

    egui::CollapsingHeader::new(egui::RichText::new(node.label.as_str()).monospace().small())
        .default_open(depth < 2)
        .show(ui, |ui| {
            for (index, child) in node.children.iter().enumerate() {
                ui.push_id(index, |ui| {
                    render_ast_node(ui, child, depth + 1);
                });
            }
        });
}

fn render_source_panel(ui: &mut egui::Ui, document: &PassDebugWindowDocument) {
    ui.horizontal(|ui| {
        ui.heading("WGSL Module");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(source) = document.source.as_ref() {
                if ui.button("Copy WGSL").clicked() {
                    ui.ctx().copy_text(source.module_source.clone());
                }
            }
        });
    });

    let Some(source) = document.source.as_ref() else {
        ui.add_space(8.0);
        ui.colored_label(
            egui::Color32::from_rgb(255, 180, 120),
            "Pass no longer exists in the current scene.",
        );
        return;
    };

    let numbered_source = line_numbered_source(&source.module_source);
    egui::ScrollArea::both()
        .id_salt(("pass-debug-source", document.pass_name.as_str()))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(numbered_source).monospace().small())
                    .wrap_mode(egui::TextWrapMode::Extend)
                    .selectable(true),
            );
        });
}

fn line_numbered_source(source: &str) -> String {
    if source.is_empty() {
        return "   1 | ".to_string();
    }
    source
        .lines()
        .enumerate()
        .map(|(index, line)| format!("{:4} | {}", index + 1, line))
        .collect::<Vec<_>>()
        .join("\n")
}
