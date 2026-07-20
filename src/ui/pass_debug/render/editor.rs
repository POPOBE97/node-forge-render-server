use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use rust_wgpu_fiber::eframe::egui;

use crate::metric_log;
use crate::renderer::PassDebugSourceRange;
use crate::ui::pass_debug::dependency_tree::byte_index_to_char_index;
use crate::ui::pass_debug::event::{PassDebugEvent, PassDebugWindowAction};
use crate::ui::pass_debug::patch::build_shortwire_diff_view;
use crate::ui::pass_debug::render::code_layout::{
    build_full_layout_job, highlighted_line_sections_for_layout, line_boundaries_for_layout,
    line_index_at_char_index, line_start_char_indices_for_layout,
};
use crate::ui::pass_debug::render::fonts::{
    PASS_DEBUG_CODE_FONT_SIZE, PASS_DEBUG_LINE_NUMBER_FONT_SIZE, pass_debug_mono_font,
};
use crate::ui::pass_debug::render::merge_dialog::render_merge_conflict_popups;
use crate::ui::pass_debug::selectors::{
    PassDebugReferenceEditorView, reference_editor_view, shader_editor_view,
};
use crate::ui::pass_debug_window::PassDebugWindowDocument;

const PASS_DEBUG_CODE_EDITOR_MARGIN_Y: i8 = 3;
const PASS_DEBUG_CODE_EDITOR_MARGIN_X: i8 = 6;
const PASS_DEBUG_LINE_NUMBER_GUTTER_MIN_WIDTH: f32 = 30.0;
const PASS_DEBUG_LINE_NUMBER_GUTTER_MAX_WIDTH: f32 = 96.0;
const PASS_DEBUG_LINE_NUMBER_GUTTER_DIGIT_WIDTH: f32 = 7.0;
const PASS_DEBUG_LINE_NUMBER_GUTTER_RIGHT_PADDING: f32 = 8.0;

#[derive(Clone, Debug)]
pub(crate) struct LineGalleyCache {
    wrap_width: f32,
    pixels_per_point: f32,
    line_hashes: Vec<u64>,
    line_sections: Vec<Vec<egui::text::LayoutSection>>,
    line_galleys: Vec<std::sync::Arc<egui::Galley>>,
    merged: std::sync::Arc<egui::Galley>,
}

fn render_shortwire_diff_editor(
    ui: &mut egui::Ui,
    id_salt: (&'static str, &str),
    base_source: &str,
    current_source: &str,
) {
    let view = build_shortwire_diff_view(base_source, current_source);
    let mut diff_text = view.to_display_text();

    ui.scope(|ui| {
        ui.visuals_mut().text_cursor.preview = false;
        egui::ScrollArea::both()
            .id_salt(id_salt)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut diff_text)
                        .id_salt((id_salt.0, "text", id_salt.1))
                        .font(pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE))
                        .code_editor()
                        .interactive(false)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin {
                            left: PASS_DEBUG_CODE_EDITOR_MARGIN_X,
                            right: PASS_DEBUG_CODE_EDITOR_MARGIN_X,
                            top: PASS_DEBUG_CODE_EDITOR_MARGIN_Y,
                            bottom: PASS_DEBUG_CODE_EDITOR_MARGIN_Y,
                        })
                        .desired_rows(24)
                        .desired_width(f32::INFINITY)
                        .lock_focus(true),
                );
            });
    });
}

pub(crate) fn render_current_editor_column(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    render_code_editor(ui, document);
    render_merge_conflict_popups(ui.ctx(), document, pending_actions);
}

pub(crate) fn render_reference_editor_column(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    let now_secs = ui.input(|input| input.time);
    render_reference_editor(ui, document);
    let _ = pending_actions;
    document.dispatch_event(PassDebugEvent::ReferenceSyncTick { now_secs }, None);
}

fn layout_with_line_cache_incremental(
    ui: &egui::Ui,
    text: &str,
    wrap_width: f32,
    theme: &crate::ui::wgsl_highlight::WgslTheme,
    cache_cell: &RefCell<Option<LineGalleyCache>>,
) -> std::sync::Arc<egui::Galley> {
    let pixels_per_point = ui.ctx().pixels_per_point();
    let rounded_wrap = wrap_width.round();
    let hasher_state = ahash::RandomState::with_seeds(1, 2, 3, 4);

    let cache_reusable = cache_cell.borrow().as_ref().is_some_and(|c| {
        (c.wrap_width - rounded_wrap).abs() < 0.5
            && (c.pixels_per_point - pixels_per_point).abs() < f32::EPSILON
    });

    let t_phase1 = Instant::now();
    let mut line_hashes_new: Vec<u64> = Vec::with_capacity(800);
    let line_boundaries = line_boundaries_for_layout(text);
    for &(start, end) in &line_boundaries {
        let hash = hasher_state.hash_one(&text[start..end]);
        line_hashes_new.push(hash);
    }
    let phase1_ms = t_phase1.elapsed().as_secs_f64() * 1000.0;

    if cache_reusable {
        let cache_ref = cache_cell.borrow();
        if let Some(ref c) = *cache_ref {
            if c.line_hashes.len() == line_hashes_new.len() && c.line_hashes == line_hashes_new {
                let merged = std::sync::Arc::clone(&c.merged);
                drop(cache_ref);
                metric_log!(
                    "[pass-debug] line_cache lines={} all_hit (fast path)",
                    line_hashes_new.len(),
                );
                return merged;
            }
        }
    }

    let t_phase3 = Instant::now();
    let prev_cache = if cache_reusable {
        cache_cell.borrow_mut().take()
    } else {
        None
    };

    struct PrevEntry<'a> {
        galley: &'a std::sync::Arc<egui::Galley>,
        sections: &'a Vec<egui::text::LayoutSection>,
    }
    let prev_lookup: HashMap<u64, PrevEntry<'_>> = prev_cache
        .as_ref()
        .map(|c| {
            c.line_hashes
                .iter()
                .zip(c.line_galleys.iter().zip(c.line_sections.iter()))
                .map(|(&h, (g, s))| {
                    (
                        h,
                        PrevEntry {
                            galley: g,
                            sections: s,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let phase3_setup_ms = t_phase3.elapsed().as_secs_f64() * 1000.0;

    let num_lines = line_boundaries.len();
    let mut line_galleys: Vec<std::sync::Arc<egui::Galley>> = Vec::with_capacity(num_lines);
    let mut line_sections_vec: Vec<Vec<egui::text::LayoutSection>> = Vec::with_capacity(num_lines);
    let mut cache_hits = 0usize;

    for (i, &(start, end)) in line_boundaries.iter().enumerate() {
        if let Some(entry) = prev_lookup.get(&line_hashes_new[i]) {
            line_galleys.push(std::sync::Arc::clone(entry.galley));
            line_sections_vec.push(entry.sections.clone());
            cache_hits += 1;
            continue;
        }

        let line_text = &text[start..end];
        let sections = highlighted_line_sections_for_layout(line_text, theme);

        let paragraph_job = egui::text::LayoutJob {
            text: line_text.to_owned(),
            wrap: egui::text::TextWrapping {
                max_width: rounded_wrap,
                max_rows: usize::MAX,
                ..Default::default()
            },
            sections: sections.clone(),
            break_on_newline: true,
            halign: egui::Align::LEFT,
            justify: false,
            first_row_min_height: 0.0,
            round_output_to_gui: true,
        };

        let galley = ui.fonts_mut(|fonts| fonts.layout_job(paragraph_job));
        line_galleys.push(galley);
        line_sections_vec.push(sections);
    }

    let t_concat = Instant::now();
    let full_job = build_full_layout_job(
        text,
        &line_boundaries,
        &line_sections_vec,
        rounded_wrap,
        theme,
    );
    let full_job_arc = Arc::new(full_job);

    let merged = if cache_hits == num_lines {
        if let Some(ref prev) = prev_cache {
            if prev.merged.job.text == text {
                std::sync::Arc::clone(&prev.merged)
            } else {
                std::sync::Arc::new(egui::Galley::concat(
                    full_job_arc,
                    &line_galleys,
                    pixels_per_point,
                ))
            }
        } else {
            std::sync::Arc::new(egui::Galley::concat(
                full_job_arc,
                &line_galleys,
                pixels_per_point,
            ))
        }
    } else {
        std::sync::Arc::new(egui::Galley::concat(
            full_job_arc,
            &line_galleys,
            pixels_per_point,
        ))
    };
    let concat_ms = t_concat.elapsed().as_secs_f64() * 1000.0;

    metric_log!(
        "[pass-debug] line_cache lines={} hits={} misses={} p1={:.2}ms p3s={:.2}ms concat={:.2}ms",
        num_lines,
        cache_hits,
        num_lines - cache_hits,
        phase1_ms,
        phase3_setup_ms,
        concat_ms,
    );

    *cache_cell.borrow_mut() = Some(LineGalleyCache {
        wrap_width: rounded_wrap,
        pixels_per_point,
        line_hashes: line_hashes_new,
        line_sections: line_sections_vec,
        line_galleys,
        merged: std::sync::Arc::clone(&merged),
    });

    merged
}

fn render_code_editor(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    let now_secs = ui.input(|input| input.time);
    document.dispatch_event(PassDebugEvent::Tick { now_secs }, None);
    let view = shader_editor_view(document);

    metric_log!(
        "[pass-debug] code_editor pass={} source_len={}",
        view.pass_name,
        view.source_len,
    );

    if let Some(diff) = view.diff.as_ref() {
        render_shortwire_diff_editor(
            ui,
            ("pass-debug-source-diff", view.pass_name.as_str()),
            &diff.base_source,
            &diff.current_source,
        );
        return;
    }

    let pass_name = view.pass_name;
    let focused_source_range = view.focused_source_range;
    let editor_interactive = view.editor_interactive;
    let shortwire_active = view.shortwire_active;
    let mut draft_source = view.draft_source;
    let existing_galley = document.line_galley_cache.as_ref().and_then(|c| {
        if c.merged.job.text == draft_source {
            Some(std::sync::Arc::clone(&c.merged))
        } else {
            None
        }
    });
    let precomputed_galley: RefCell<Option<std::sync::Arc<egui::Galley>>> =
        RefCell::new(existing_galley);

    let line_cache_cell: RefCell<Option<LineGalleyCache>> =
        RefCell::new(document.line_galley_cache.take());

    let font_id = pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE);
    let wgsl_theme = if ui.visuals().dark_mode {
        crate::ui::wgsl_highlight::WgslTheme::dark(font_id)
    } else {
        crate::ui::wgsl_highlight::WgslTheme::light(font_id)
    };

    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        if let Some(ref galley) = *precomputed_galley.borrow() {
            if galley.job.text == buf.as_str()
                && (galley.job.wrap.max_width - wrap_width).abs() < 0.5
            {
                return std::sync::Arc::clone(galley);
            }
        }

        let t_layouter = Instant::now();
        let galley = layout_with_line_cache_incremental(
            ui,
            buf.as_str(),
            wrap_width,
            &wgsl_theme,
            &line_cache_cell,
        );

        let layouter_ms = t_layouter.elapsed().as_secs_f64() * 1000.0;
        metric_log!(
            "[pass-debug] layouter_call={:.2}ms wrap_width={:.0} (incremental)",
            layouter_ms,
            wrap_width,
        );
        *precomputed_galley.borrow_mut() = Some(std::sync::Arc::clone(&galley));
        galley
    };

    ui.scope(|ui| {
        ui.visuals_mut().text_cursor.preview = false;
        egui::ScrollArea::vertical()
            .id_salt(("pass-debug-source-editor", pass_name.as_str()))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let initial_line_count = line_boundaries_for_layout(&draft_source).len();
                let gutter_width = line_number_gutter_width(initial_line_count);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let gutter_top_left = ui.cursor().left_top();
                    ui.add_space(gutter_width);

                    let editor = egui::TextEdit::multiline(&mut draft_source)
                        .id_salt(("pass-debug-source-text", pass_name.as_str()))
                        .font(pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE))
                        .code_editor()
                        .interactive(editor_interactive)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin {
                            left: PASS_DEBUG_CODE_EDITOR_MARGIN_X,
                            right: PASS_DEBUG_CODE_EDITOR_MARGIN_X,
                            top: PASS_DEBUG_CODE_EDITOR_MARGIN_Y,
                            bottom: PASS_DEBUG_CODE_EDITOR_MARGIN_Y,
                        })
                        .desired_rows(24)
                        .desired_width(f32::INFINITY)
                        .lock_focus(true)
                        .layouter(&mut layouter);

                    let t_show = Instant::now();
                    let output = editor.show(ui);
                    let show_ms = t_show.elapsed().as_secs_f64() * 1000.0;
                    metric_log!("[pass-debug] editor.show={:.2}ms", show_ms,);

                    let gutter_rect = egui::Rect::from_min_max(
                        gutter_top_left,
                        egui::pos2(
                            gutter_top_left.x + gutter_width,
                            output.response.rect.bottom(),
                        ),
                    );
                    let line_boundaries = line_boundaries_for_layout(&draft_source);
                    paint_line_number_gutter(
                        ui,
                        &output,
                        &draft_source,
                        &line_boundaries,
                        gutter_rect,
                    );

                    if !shortwire_active {
                        if let Some(source_range) = focused_source_range {
                            paint_focus_highlight_overlay(ui, &output, &draft_source, source_range);
                        }
                    }

                    if output.response.changed() {
                        document.dispatch_event(
                            PassDebugEvent::ShaderDraftReplaced {
                                source: draft_source.clone(),
                                now_secs,
                            },
                            None,
                        );
                    }
                    if let Some(source_range) = document.take_pending_editor_jump() {
                        jump_editor_to_source_range(ui, &output, &draft_source, source_range);
                    }
                    if !shortwire_active
                        && output.response.clicked()
                        && let Some(cursor_range) = output.cursor_range
                    {
                        document.dispatch_event(
                            PassDebugEvent::ShaderEditorClicked {
                                char_index: cursor_range.primary.index,
                            },
                            None,
                        );
                    }
                });
            });
    });

    document.line_galley_cache = line_cache_cell.into_inner();
}

fn render_reference_editor(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    let now_secs = ui.input(|input| input.time);
    let (pass_name, mut editor_source) = match reference_editor_view(document) {
        PassDebugReferenceEditorView::Empty => {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Open a UTF-8 text file or folder to add reference code.")
                    .monospace()
                    .small(),
            );
            return;
        }
        PassDebugReferenceEditorView::Diff(diff) => {
            render_shortwire_diff_editor(
                ui,
                ("pass-debug-reference-diff", document.pass_name.as_str()),
                &diff.base_source,
                &diff.current_source,
            );
            return;
        }
        PassDebugReferenceEditorView::Editor {
            pass_name,
            editor_source,
        } => (pass_name, editor_source),
    };
    let existing_galley = document.reference_line_galley_cache.as_ref().and_then(|c| {
        if c.merged.job.text == editor_source {
            Some(std::sync::Arc::clone(&c.merged))
        } else {
            None
        }
    });
    let precomputed_galley: RefCell<Option<std::sync::Arc<egui::Galley>>> =
        RefCell::new(existing_galley);

    let line_cache_cell: RefCell<Option<LineGalleyCache>> =
        RefCell::new(document.reference_line_galley_cache.take());

    let font_id = pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE);
    let wgsl_theme = if ui.visuals().dark_mode {
        crate::ui::wgsl_highlight::WgslTheme::dark(font_id)
    } else {
        crate::ui::wgsl_highlight::WgslTheme::light(font_id)
    };

    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        if let Some(ref galley) = *precomputed_galley.borrow() {
            if galley.job.text == buf.as_str()
                && (galley.job.wrap.max_width - wrap_width).abs() < 0.5
            {
                return std::sync::Arc::clone(galley);
            }
        }

        let galley = layout_with_line_cache_incremental(
            ui,
            buf.as_str(),
            wrap_width,
            &wgsl_theme,
            &line_cache_cell,
        );
        *precomputed_galley.borrow_mut() = Some(std::sync::Arc::clone(&galley));
        galley
    };

    ui.scope(|ui| {
        ui.visuals_mut().text_cursor.preview = false;
        egui::ScrollArea::vertical()
            .id_salt(("pass-debug-reference-editor", pass_name.as_str()))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let initial_line_count = line_boundaries_for_layout(&editor_source).len();
                let gutter_width = line_number_gutter_width(initial_line_count);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let gutter_top_left = ui.cursor().left_top();
                    ui.add_space(gutter_width);

                    let editor = egui::TextEdit::multiline(&mut editor_source)
                        .id_salt(("pass-debug-reference-text", pass_name.as_str()))
                        .font(pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE))
                        .code_editor()
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin {
                            left: PASS_DEBUG_CODE_EDITOR_MARGIN_X,
                            right: PASS_DEBUG_CODE_EDITOR_MARGIN_X,
                            top: PASS_DEBUG_CODE_EDITOR_MARGIN_Y,
                            bottom: PASS_DEBUG_CODE_EDITOR_MARGIN_Y,
                        })
                        .desired_rows(24)
                        .desired_width(f32::INFINITY)
                        .lock_focus(true)
                        .layouter(&mut layouter);

                    let output = editor.show(ui);
                    let gutter_rect = egui::Rect::from_min_max(
                        gutter_top_left,
                        egui::pos2(
                            gutter_top_left.x + gutter_width,
                            output.response.rect.bottom(),
                        ),
                    );
                    let line_boundaries = line_boundaries_for_layout(&editor_source);
                    paint_line_number_gutter(
                        ui,
                        &output,
                        &editor_source,
                        &line_boundaries,
                        gutter_rect,
                    );

                    if output.response.changed() {
                        document.dispatch_event(
                            PassDebugEvent::ReferenceEditorReplaced {
                                source: editor_source.clone(),
                                now_secs,
                            },
                            None,
                        );
                    }
                });
            });
    });

    document.reference_line_galley_cache = line_cache_cell.into_inner();
}

fn line_number_gutter_width(line_count: usize) -> f32 {
    let digits = line_count.max(1).to_string().len() as f32;
    (digits * PASS_DEBUG_LINE_NUMBER_GUTTER_DIGIT_WIDTH
        + PASS_DEBUG_LINE_NUMBER_GUTTER_RIGHT_PADDING
        + 10.0)
        .clamp(
            PASS_DEBUG_LINE_NUMBER_GUTTER_MIN_WIDTH,
            PASS_DEBUG_LINE_NUMBER_GUTTER_MAX_WIDTH,
        )
        .ceil()
}

fn paint_line_number_gutter(
    ui: &egui::Ui,
    output: &egui::widgets::text_edit::TextEditOutput,
    source: &str,
    line_boundaries: &[(usize, usize)],
    gutter_rect: egui::Rect,
) {
    if line_boundaries.is_empty() {
        return;
    }

    let clip_rect = gutter_rect.intersect(ui.clip_rect());
    if clip_rect.is_negative() {
        return;
    }

    let painter = ui.painter_at(clip_rect);
    let separator_x = gutter_rect.right() - 0.5;
    painter.line_segment(
        [
            egui::pos2(separator_x, gutter_rect.top()),
            egui::pos2(separator_x, gutter_rect.bottom()),
        ],
        egui::Stroke::new(1.0_f32, line_number_separator_color(ui)),
    );

    let active_line = output
        .cursor_range
        .and_then(|range| line_index_at_char_index(source, range.primary.index, line_boundaries));
    let line_start_chars = line_start_char_indices_for_layout(source, line_boundaries);
    let number_x = gutter_rect.right() - PASS_DEBUG_LINE_NUMBER_GUTTER_RIGHT_PADDING;
    let font_id = pass_debug_mono_font(PASS_DEBUG_LINE_NUMBER_FONT_SIZE);

    for (line_idx, &start_char) in line_start_chars.iter().enumerate() {
        let cursor_rect = output
            .galley
            .pos_from_cursor(egui::text::CCursor::new(start_char))
            .translate(output.galley_pos.to_vec2());
        if cursor_rect.bottom() < clip_rect.top() || cursor_rect.top() > clip_rect.bottom() {
            continue;
        }

        let is_active = active_line == Some(line_idx);
        painter.text(
            egui::pos2(number_x, cursor_rect.center().y),
            egui::Align2::RIGHT_CENTER,
            (line_idx + 1).to_string(),
            font_id.clone(),
            line_number_text_color(ui, is_active),
        );
    }
}

fn line_number_separator_color(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 26)
    } else {
        egui::Color32::from_rgba_unmultiplied(15, 23, 42, 30)
    }
}

fn line_number_text_color(ui: &egui::Ui, active: bool) -> egui::Color32 {
    if active {
        if ui.visuals().dark_mode {
            egui::Color32::from_rgb(191, 219, 254)
        } else {
            egui::Color32::from_rgb(30, 64, 175)
        }
    } else if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(203, 213, 225, 96)
    } else {
        egui::Color32::from_rgba_unmultiplied(51, 65, 85, 106)
    }
}

fn paint_focus_highlight_overlay(
    ui: &egui::Ui,
    output: &egui::widgets::text_edit::TextEditOutput,
    source: &str,
    source_range: PassDebugSourceRange,
) {
    let highlight_start = source_range.start_byte;
    let highlight_end = source_range.end_byte;
    if highlight_start >= highlight_end
        || highlight_end > source.len()
        || !source.is_char_boundary(highlight_start)
        || !source.is_char_boundary(highlight_end)
    {
        return;
    }

    let start_char = byte_index_to_char_index(source, highlight_start);
    let end_char = byte_index_to_char_index(source, highlight_end);
    let highlight_color = egui::Color32::from_rgba_premultiplied(251, 191, 36, 56);
    let galley = &output.galley;
    let galley_pos = output.galley_pos;

    let start_cursor = galley.layout_from_cursor(egui::text::CCursor::new(start_char));
    let end_cursor = galley.layout_from_cursor(egui::text::CCursor::new(end_char));

    if start_cursor.row == end_cursor.row {
        let start_rect = galley.pos_from_layout_cursor(&start_cursor);
        let end_rect = galley.pos_from_layout_cursor(&end_cursor);
        let row = &galley.rows[start_cursor.row];
        let rect = egui::Rect::from_min_max(
            egui::pos2(start_rect.left() + galley_pos.x, row.pos.y + galley_pos.y),
            egui::pos2(
                end_rect.left() + galley_pos.x,
                row.pos.y + row.row.size.y + galley_pos.y,
            ),
        );
        ui.painter().rect_filled(rect, 0.0, highlight_color);
    } else {
        for row_idx in start_cursor.row..=end_cursor.row {
            let Some(row) = galley.rows.get(row_idx) else {
                break;
            };
            let row_top = row.pos.y + galley_pos.y;
            let row_bottom = row_top + row.row.size.y;

            let left = if row_idx == start_cursor.row {
                let cursor_rect = galley.pos_from_layout_cursor(&start_cursor);
                cursor_rect.left() + galley_pos.x
            } else {
                row.pos.x + galley_pos.x
            };
            let right = if row_idx == end_cursor.row {
                let cursor_rect = galley.pos_from_layout_cursor(&end_cursor);
                cursor_rect.left() + galley_pos.x
            } else {
                row.pos.x + row.row.size.x + galley_pos.x
            };

            if right > left {
                let rect = egui::Rect::from_min_max(
                    egui::pos2(left, row_top),
                    egui::pos2(right, row_bottom),
                );
                ui.painter().rect_filled(rect, 0.0, highlight_color);
            }
        }
    }
}

fn jump_editor_to_source_range(
    ui: &mut egui::Ui,
    output: &egui::widgets::text_edit::TextEditOutput,
    source: &str,
    source_range: PassDebugSourceRange,
) {
    if source_range.start_byte >= source_range.end_byte || source_range.end_byte > source.len() {
        return;
    }

    let start_char = byte_index_to_char_index(source, source_range.start_byte);
    let end_char = byte_index_to_char_index(source, source_range.end_byte).max(start_char + 1);
    let selection = egui::text::CCursorRange::two(
        egui::text::CCursor::new(start_char),
        egui::text::CCursor::new(end_char),
    );
    let mut state = output.state.clone();
    state.cursor.set_char_range(Some(selection));
    state.store(ui.ctx(), output.response.id);
    output.response.request_focus();

    let cursor_rect = output
        .galley
        .pos_from_cursor(egui::text::CCursor::new(start_char))
        .translate(output.galley_pos.to_vec2())
        .expand2(egui::vec2(0.0, 64.0));
    ui.scroll_to_rect(cursor_rect, Some(egui::Align::Center));
}
