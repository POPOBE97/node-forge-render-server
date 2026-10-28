//! Presentation-only layout: fold dependency ranks into alternating rows so a
//! serial GPU pipeline uses both dimensions of the window. Ranks still contain
//! parallel branches; neither execution order nor dependency edges are changed.

use rust_wgpu_fiber::eframe::egui;

use super::pass_graph::PassGraph;

pub(super) const CARD: egui::Vec2 = egui::vec2(240.0, 112.0);
const COLUMN_STEP: f32 = 328.0;
const BRANCH_STEP: f32 = 156.0;
const BAND_GAP: f32 = 112.0;
const MARGIN: f32 = 44.0;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum LayoutMode {
    #[default]
    Compact,
    Vertical,
}

pub(super) struct GraphLayout {
    pub rects: Vec<egui::Rect>,
    pub paths: Vec<Vec<egui::Pos2>>,
    pub bounds: egui::Rect,
}

impl GraphLayout {
    /// Choose the fold count by the scale at which the entire graph fits. This
    /// also accounts for wide parallel branches and the gutters used by edges.
    pub fn columns_for(graph: &PassGraph, available: egui::Vec2) -> usize {
        (1..=graph.rows.len().clamp(1, 12))
            .map(|columns| {
                let layout = Self::new(graph, LayoutMode::Compact, columns);
                let scale =
                    (available.x / layout.bounds.width()).min(available.y / layout.bounds.height());
                (columns, scale)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(1, |(columns, _)| columns)
    }

    pub fn new(graph: &PassGraph, mode: LayoutMode, columns: usize) -> Self {
        let mut rects = vec![egui::Rect::NOTHING; graph.nodes.len()];
        let mut ranks = vec![0; graph.nodes.len()];
        let mut bands = Vec::new();
        let columns = columns.max(1);
        if mode == LayoutMode::Compact {
            let mut y = 0.0;
            for (band, rows) in graph.rows.chunks(columns).enumerate() {
                let branches = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                let height = CARD.y + (branches - 1) as f32 * BRANCH_STEP;
                bands.push(egui::Rect::from_min_size(
                    egui::pos2(0.0, y),
                    egui::vec2((columns - 1) as f32 * COLUMN_STEP + CARD.x, height),
                ));
                for (column, nodes) in rows.iter().enumerate() {
                    let x = if band % 2 == 0 {
                        column
                    } else {
                        columns - 1 - column
                    };
                    let top = y + (branches - nodes.len()) as f32 * BRANCH_STEP / 2.0;
                    for (branch, &index) in nodes.iter().enumerate() {
                        ranks[index] = band * columns + column;
                        rects[index] = egui::Rect::from_min_size(
                            egui::pos2(x as f32 * COLUMN_STEP, top + branch as f32 * BRANCH_STEP),
                            CARD,
                        );
                    }
                }
                y += height + BAND_GAP;
            }
        } else {
            for (row, nodes) in graph.rows.iter().enumerate() {
                for (column, &index) in nodes.iter().enumerate() {
                    let center_x = (column as f32 - (nodes.len() as f32 - 1.0) / 2.0) * 284.0;
                    rects[index] = egui::Rect::from_min_size(
                        egui::pos2(center_x - CARD.x / 2.0, row as f32 * 184.0),
                        CARD,
                    );
                }
            }
        }
        let mut bounds = rects.iter().fold(egui::Rect::NOTHING, |a, b| a.union(*b));
        let paths = graph
            .edges
            .iter()
            .enumerate()
            .map(|(index, edge)| {
                let from = rects[edge.from];
                let to = rects[edge.to];
                let points = if mode == LayoutMode::Compact {
                    compact_path(
                        from,
                        to,
                        ranks[edge.from],
                        ranks[edge.to],
                        columns,
                        &bands,
                        index,
                    )
                } else {
                    vertical_path(from, to, bounds.left() - MARGIN, index)
                };
                // Remove zero-length and redundant corners before rounding paths.
                let mut path: Vec<egui::Pos2> = Vec::new();
                for point in points {
                    if path.last() == Some(&point) {
                        continue;
                    }
                    if path.len() >= 2 {
                        let a = path[path.len() - 2];
                        let b = path[path.len() - 1];
                        let ab = b - a;
                        let bc = point - b;
                        if ab.x * bc.y == ab.y * bc.x && ab.dot(bc) > 0.0 {
                            path.pop();
                        }
                    }
                    path.push(point);
                }
                path
            })
            .collect::<Vec<_>>();
        for point in paths.iter().flatten() {
            bounds.extend_with(*point);
        }
        Self {
            rects,
            paths,
            bounds: bounds.expand(MARGIN),
        }
    }
}

fn compact_path(
    from: egui::Rect,
    to: egui::Rect,
    from_rank: usize,
    to_rank: usize,
    columns: usize,
    bands: &[egui::Rect],
    index: usize,
) -> Vec<egui::Pos2> {
    let a = from_rank / columns;
    let b = to_rank / columns;
    let forward = |band| if band % 2 == 0 { 1.0 } else { -1.0 };
    let side = |rect: egui::Rect, direction: f32| {
        rect.center() + egui::vec2(direction * rect.width() / 2.0, 0.0)
    };
    let start = side(from, forward(a));
    let end = side(to, -forward(b)) - egui::vec2(forward(b) * 3.0, 0.0);
    if a == b && to_rank == from_rank + 1 {
        let x = (start.x + end.x) / 2.0;
        return vec![start, egui::pos2(x, start.y), egui::pos2(x, end.y), end];
    }
    let lane = (index % 5) as f32 * 5.0;
    if to_rank == from_rank + 1 {
        // The end of one row turns around its outside edge into the next row.
        let x = start.x + forward(a) * (28.0 + lane);
        return vec![start, egui::pos2(x, start.y), egui::pos2(x, end.y), end];
    }
    let exit_x = start.x + forward(a) * (24.0 + lane);
    let entry_x = end.x - forward(b) * (24.0 + lane);
    if a == b {
        // Skip edges run above the entire band, including its parallel branches.
        let y = bands[a].top() - 24.0 - lane;
        vec![
            start,
            egui::pos2(exit_x, start.y),
            egui::pos2(exit_x, y),
            egui::pos2(entry_x, y),
            egui::pos2(entry_x, end.y),
            end,
        ]
    } else {
        let exit_y = bands[a].bottom() + 24.0 + lane;
        let entry_y = bands[b].top() - 24.0 - lane;
        let outside_x = if forward(a) > 0.0 {
            bands[a].right() + 56.0 + lane
        } else {
            bands[a].left() - 56.0 - lane
        };
        vec![
            start,
            egui::pos2(exit_x, start.y),
            egui::pos2(exit_x, exit_y),
            egui::pos2(outside_x, exit_y),
            egui::pos2(outside_x, entry_y),
            egui::pos2(entry_x, entry_y),
            egui::pos2(entry_x, end.y),
            end,
        ]
    }
}

fn vertical_path(from: egui::Rect, to: egui::Rect, left: f32, index: usize) -> Vec<egui::Pos2> {
    let start = from.center_bottom();
    let end = to.center_top() - egui::vec2(0.0, 3.0);
    if to.top() - from.bottom() > 184.0 {
        let lane = left + (index % 6) as f32 * 5.0;
        vec![
            start,
            start + egui::vec2(0.0, 24.0),
            egui::pos2(lane, start.y + 24.0),
            egui::pos2(lane, end.y - 24.0),
            end - egui::vec2(0.0, 24.0),
            end,
        ]
    } else {
        let middle = (start.y + end.y) / 2.0;
        vec![
            start,
            egui::pos2(start.x, middle),
            egui::pos2(end.x, middle),
            end,
        ]
    }
}
