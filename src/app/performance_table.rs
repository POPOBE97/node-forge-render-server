//! Compact metrics table with a hover guide back to the row and column labels.
use rust_wgpu_fiber::eframe::egui;

use crate::ui::{design_tokens, tool_window};

const INSET: f32 = 6.0;
const CELL_PADDING: f32 = 10.0;
const ROW_HEIGHT: f32 = 28.0;

pub(super) struct Column {
    title: &'static str,
    width: f32,
    numeric: bool,
}

impl Column {
    pub(super) fn text(title: &'static str, width: f32) -> Self {
        Self {
            title,
            width,
            numeric: false,
        }
    }

    pub(super) fn metric(title: &'static str, width: f32) -> Self {
        Self {
            title,
            width,
            numeric: true,
        }
    }
}

pub(super) fn show<const N: usize>(
    ui: &mut egui::Ui,
    id_salt: &str,
    columns: [Column; N],
    rows: &[[String; N]],
) {
    let id = ui.make_persistent_id(id_salt);
    // Keep column widths stable as live values change. Narrow windows scroll
    // horizontally, while ordinary wheel input continues to scroll the page.
    egui::ScrollArea::horizontal().id_salt(id).show(ui, |ui| {
        let minimum_width = columns.iter().map(|column| column.width).sum::<f32>();
        let width = (ui.available_width() - 2.0 * INSET).max(minimum_width);
        let extra = (width - minimum_width) / N as f32;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(
                width + 2.0 * INSET,
                (rows.len() + 1) as f32 * ROW_HEIGHT + 2.0 * INSET,
            ),
            egui::Sense::hover(),
        );
        ui.painter().rect(
            rect,
            design_tokens::BORDER_RADIUS_SMALL,
            tool_window::CANVAS,
            egui::Stroke::new(1.0_f32, design_tokens::white(15)),
            egui::StrokeKind::Inside,
        );
        let mut x = rect.left() + INSET;
        let bounds: Vec<_> = columns
            .iter()
            .map(|column| {
                let left = x;
                x += column.width + extra;
                (left, x)
            })
            .collect();
        let cell_rect = |row: usize, column: usize| {
            let top = rect.top() + INSET + row as f32 * ROW_HEIGHT;
            egui::Rect::from_min_max(
                egui::pos2(bounds[column].0, top),
                egui::pos2(bounds[column].1, top + ROW_HEIGHT),
            )
        };
        let mut hovered = None;
        // egui retains interact_pos for the PointerGone frame (e.g. a touch
        // release). A reading guide should only follow a pointer still present.
        let has_pointer = ui.input(|input| input.pointer.has_pointer());
        let mut cells = Vec::with_capacity((rows.len() + 1) * N);
        for row in 0..=rows.len() {
            for (column, spec) in columns.iter().enumerate() {
                let text = if row == 0 {
                    spec.title
                } else {
                    &rows[row - 1][column]
                };
                let cell = cell_rect(row, column);
                let response = ui.interact(cell, id.with((row, column)), egui::Sense::hover());
                response
                    .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
                if has_pointer && row > 0 && column > 0 && response.hovered() {
                    hovered = Some((row, column));
                }
                cells.push((row, column, text, cell, response));
            }
        }
        if let Some((row, column)) = hovered {
            let current = cell_rect(row, column);
            let guide = design_tokens::RESOURCE_ACCENT.gamma_multiply(0.20);
            // The guides stop at the focused cell, like rulers leading back to
            // the two labels. They never extend to unrelated values below/right.
            ui.painter().rect_filled(
                egui::Rect::from_min_max(cell_rect(row, 1).min, current.max),
                0.0,
                guide,
            );
            ui.painter().rect_filled(
                egui::Rect::from_min_max(cell_rect(1, column).min, current.right_top()),
                0.0,
                guide,
            );
            ui.painter().rect_filled(
                current,
                0.0,
                design_tokens::RESOURCE_ACCENT.gamma_multiply(0.38),
            );
        }
        let focus = ui
            .ctx()
            .animate_bool_responsive(id.with("focus"), hovered.is_some());
        for (row, column, text, cell, response) in cells {
            let on_guide = hovered
                .is_some_and(|(r, c)| (row == r && column <= c) || (column == c && row <= r));
            let color = if on_guide {
                design_tokens::white(95)
            } else {
                let opacity = if row == 0 || column == 0 { 0.65 } else { 0.80 };
                egui::Color32::from_white_alpha((255.0 * opacity * (1.0 - 0.60 * focus)) as u8)
            };
            let numeric = row > 0 && columns[column].numeric;
            let font_id = if numeric {
                egui::FontId::monospace(12.0)
            } else {
                design_tokens::font_id(13.0, design_tokens::FontWeight::Medium)
            };
            let mut job = egui::text::LayoutJob::single_section(
                text.to_owned(),
                egui::TextFormat {
                    font_id,
                    color,
                    ..Default::default()
                },
            );
            job.wrap.max_width = cell.width() - 2.0 * CELL_PADDING;
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            let galley = ui.painter().layout_job(job);
            if galley.elided {
                response.on_hover_text(text);
            }
            let x = if columns[column].numeric {
                cell.right() - CELL_PADDING - galley.size().x
            } else {
                cell.left() + CELL_PADDING
            };
            ui.painter().galley(
                egui::pos2(x, cell.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
        }
    });
}
