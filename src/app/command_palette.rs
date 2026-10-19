use rust_wgpu_fiber::eframe::egui;

use super::{
    canvas::actions::CanvasAction, display_metrics, frame::commands::AppCommand, types::App,
};

#[derive(Default)]
pub(super) struct CommandPalette {
    open: bool,
    captured_this_frame: bool,
    query: String,
    selected: usize,
}

impl CommandPalette {
    pub fn captures_input(&self) -> bool {
        self.open || self.captured_this_frame
    }
}

struct Entry {
    label: &'static str,
    shortcut: &'static str,
    command: AppCommand,
    enabled: bool,
}

fn entries(app: &App, ctx: &egui::Context) -> Vec<Entry> {
    use AppCommand as C;
    use CanvasAction as A;
    let metrics = display_metrics::current_display_metrics(ctx);
    let has_reference = app.canvas.reference.ref_image.is_some();
    let mut entries = vec![
        Entry {
            label: if app.shell.statistics_visible {
                "Hide Statistics Text"
            } else {
                "Show Statistics Text"
            },
            shortcut: "",
            command: C::ToggleStatistics,
            enabled: true,
        },
        Entry {
            label: "Toggle Canvas Only / Sidebar",
            shortcut: "F",
            command: C::ToggleCanvasOnly,
            enabled: true,
        },
        Entry {
            label: "Toggle Timeline",
            shortcut: "Cmd+J",
            command: C::ToggleTimeline,
            enabled: true,
        },
    ];
    for (label, shortcut, action, enabled) in [
        ("Toggle HDR / SDR Clamp", "S", A::ToggleHdrClamp, true),
        ("Toggle Wireframe", "W", A::ToggleWireframe, true),
        ("Pause / Resume Animation", "Space", A::TogglePause, true),
        (
            "Reset View",
            "R",
            A::ResetView {
                current_display_ppi: metrics.display_ppi,
            },
            true,
        ),
        (
            "Center View at 1:1",
            "Cmd+1",
            A::CenterAt1x {
                pixels_per_point: metrics.pixels_per_point,
                current_display_ppi: metrics.display_ppi,
            },
            true,
        ),
        (
            "Toggle Nearest / Linear Sampling",
            "N",
            A::ToggleSampling,
            true,
        ),
        (
            "Toggle Pixel Values: Integer / Fraction",
            "H",
            A::TogglePixelValueDisplay,
            true,
        ),
        ("Toggle Clipping Overlay", "C", A::ToggleClipping, true),
        (
            "Toggle Reference Alpha Mode",
            "P",
            A::ToggleReferenceAlpha,
            has_reference,
        ),
        (
            "Reset Reference Offset / Alignment",
            "A",
            A::ResetReferenceOffset,
            has_reference,
        ),
        (
            "Hide Reference Image",
            "1",
            A::SetReferenceOpacity(0.0),
            has_reference,
        ),
        (
            "Show Reference Image",
            "2",
            A::SetReferenceOpacity(1.0),
            has_reference,
        ),
        (
            "Toggle Reference Overlay / Diff",
            "D",
            A::ToggleReferenceMode,
            has_reference,
        ),
    ] {
        entries.push(Entry {
            label,
            shortcut,
            command: C::Canvas(action),
            enabled,
        });
    }
    entries.extend([
        Entry {
            label: "Paste Reference Image from Clipboard",
            shortcut: "Cmd+V",
            command: C::PasteReference,
            enabled: true,
        },
        Entry {
            label: "Copy Android Screenshot: Source Profile",
            shortcut: "Cmd+Alt+3",
            command: C::CopyAndroidScreenshot { srgb: false },
            enabled: true,
        },
        Entry {
            label: "Copy Android Screenshot: Figma sRGB",
            shortcut: "Cmd+Alt+4",
            command: C::CopyAndroidScreenshot { srgb: true },
            enabled: true,
        },
        Entry {
            label: "Cancel Design Edit / Exit Preview",
            shortcut: "Esc",
            command: C::CancelPreview,
            enabled: app.canvas.design.active.is_some()
                || app.canvas.display.preview_texture_name.is_some(),
        },
        Entry {
            label: "Clear Timeline Anchor",
            shortcut: "Delete / Backspace",
            command: C::ClearTimelineAnchor,
            enabled: app.runtime.timeline_review.anchor_frame_id.is_some(),
        },
    ]);
    entries
}

fn matches_query(label: &str, shortcut: &str, query: &str) -> bool {
    let haystack = format!("{label} {shortcut}").to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

/// Runs before scene input ingestion so Cmd+P and palette events cannot reach
/// the canvas, including the frame in which the palette is dismissed.
pub(super) fn show(app: &mut App, ctx: &egui::Context) -> Option<AppCommand> {
    let opening = ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::P));
    let entries = if app.shell.command_palette.open || opening {
        entries(app, ctx)
    } else {
        Vec::new()
    };
    show_entries(&mut app.shell.command_palette, ctx, &entries)
}

fn show_entries(
    palette: &mut CommandPalette,
    ctx: &egui::Context,
    entries: &[Entry],
) -> Option<AppCommand> {
    palette.captured_this_frame = palette.open;
    let toggle = ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::P));
    if toggle {
        palette.open = !palette.open;
        palette.captured_this_frame = true;
        palette.query.clear();
        palette.selected = 0;
    }
    let search_id = egui::Id::new("command_palette.search");
    if !palette.open {
        if toggle {
            ctx.memory_mut(|memory| memory.surrender_focus(search_id));
        }
        return None;
    }

    let mut chosen = None;
    let up = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));
    let down =
        ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
    let enter = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let response = egui::Modal::new(egui::Id::new("command_palette")).show(ctx, |ui| {
        ui.set_width(520.0_f32.min((ctx.content_rect().width() - 48.0).max(160.0)));
        ui.heading("Command Palette");
        // Keep typing in search after arrow navigation; egui otherwise moves
        // keyboard focus from the text edit to one of the command buttons.
        ui.memory_mut(|memory| memory.request_focus(search_id));
        let search = ui.add(
            egui::TextEdit::singleline(&mut palette.query)
                .id(search_id)
                .hint_text("Search commands…")
                .desired_width(f32::INFINITY),
        );
        if search.changed() {
            palette.selected = 0;
        }
        let filtered: Vec<_> = entries
            .iter()
            .filter(|entry| matches_query(entry.label, entry.shortcut, &palette.query))
            .collect();
        palette.selected = palette.selected.min(filtered.len().saturating_sub(1));
        if !filtered.is_empty() && (up || down) {
            let count = filtered.len();
            for _ in 0..count {
                palette.selected = if down {
                    (palette.selected + 1) % count
                } else {
                    (palette.selected + count - 1) % count
                };
                if filtered[palette.selected].enabled {
                    break;
                }
            }
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .max_height((ctx.content_rect().height() * 0.55).min(440.0))
            .show(ui, |ui| {
                if filtered.is_empty() {
                    ui.weak("No matching commands");
                }
                for (index, entry) in filtered.iter().enumerate() {
                    // Selectable command rows intentionally use native egui buttons.
                    let shortcut = if cfg!(target_os = "macos") {
                        entry.shortcut.to_owned()
                    } else {
                        entry.shortcut.replace("Cmd", "Ctrl")
                    };
                    let row = ui.add_enabled(
                        entry.enabled,
                        egui::Button::new(entry.label)
                            .shortcut_text(shortcut)
                            .selected(index == palette.selected)
                            .min_size(egui::vec2(ui.available_width(), 30.0)),
                    );
                    if index == palette.selected && (up || down || search.changed()) {
                        row.scroll_to_me(Some(egui::Align::Center));
                    }
                    if row.clicked() || (enter && index == palette.selected && entry.enabled) {
                        chosen = Some(entry.command.clone());
                    }
                }
            });
        ui.separator();
        ui.weak("↑ ↓ Select    Enter Run    Esc Close");
    });
    if response.should_close() || chosen.is_some() {
        palette.open = false;
        ctx.memory_mut(|memory| memory.surrender_focus(search_id));
        ctx.request_repaint();
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn frame(
        ctx: &egui::Context,
        palette: &mut CommandPalette,
        entries: &[Entry],
        events: Vec<egui::Event>,
    ) -> Option<AppCommand> {
        let mut command = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                command = show_entries(palette, ui.ctx(), entries);
                // Cmd+P must not also trigger the existing P reference-alpha action.
                assert!(!ui.ctx().input(|input| input.key_pressed(egui::Key::P)));
            },
        );
        command
    }

    #[test]
    fn palette_keyboard_search_execute_and_escape_capture_input() {
        let ctx = egui::Context::default();
        let mut palette = CommandPalette::default();
        let entries = vec![
            Entry {
                label: "Toggle Timeline",
                shortcut: "Cmd+J",
                command: AppCommand::ToggleTimeline,
                enabled: true,
            },
            Entry {
                label: "Hide Statistics Text",
                shortcut: "",
                command: AppCommand::ToggleStatistics,
                enabled: true,
            },
        ];
        frame(
            &ctx,
            &mut palette,
            &entries,
            vec![key(egui::Key::P, egui::Modifiers::COMMAND)],
        );
        assert!(palette.open);
        assert!(palette.captures_input());
        frame(
            &ctx,
            &mut palette,
            &entries,
            vec![egui::Event::Text("STATISTICS".into())],
        );
        assert_eq!(palette.query, "STATISTICS");
        let command = frame(
            &ctx,
            &mut palette,
            &entries,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert!(matches!(command, Some(AppCommand::ToggleStatistics)));
        assert!(!palette.open);
        assert!(palette.captures_input());
        frame(&ctx, &mut palette, &entries, vec![]);
        assert!(!palette.captures_input());
        frame(
            &ctx,
            &mut palette,
            &entries,
            vec![key(egui::Key::P, egui::Modifiers::COMMAND)],
        );
        assert!(palette.query.is_empty());
        let command = frame(
            &ctx,
            &mut palette,
            &entries,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        assert!(command.is_none());
        assert!(!palette.open);
        assert!(palette.captures_input());
    }

    #[test]
    fn palette_navigation_skips_unavailable_commands_and_empty_results_are_safe() {
        let ctx = egui::Context::default();
        let mut palette = CommandPalette::default();
        let entries = vec![
            Entry {
                label: "Statistics",
                shortcut: "",
                command: AppCommand::ToggleStatistics,
                enabled: true,
            },
            Entry {
                label: "Reference",
                shortcut: "P",
                command: AppCommand::PasteReference,
                enabled: false,
            },
            Entry {
                label: "Timeline",
                shortcut: "Cmd+J",
                command: AppCommand::ToggleTimeline,
                enabled: true,
            },
        ];
        frame(
            &ctx,
            &mut palette,
            &entries,
            vec![key(egui::Key::P, egui::Modifiers::COMMAND)],
        );
        frame(
            &ctx,
            &mut palette,
            &entries,
            vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
        );
        assert_eq!(palette.selected, 2);
        frame(
            &ctx,
            &mut palette,
            &entries,
            vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
        );
        assert_eq!(palette.selected, 0);
        frame(
            &ctx,
            &mut palette,
            &entries,
            vec![egui::Event::Text("missing command".into())],
        );
        assert!(
            frame(
                &ctx,
                &mut palette,
                &entries,
                vec![key(egui::Key::Enter, egui::Modifiers::NONE)]
            )
            .is_none()
        );
        assert!(palette.open);
    }
}
