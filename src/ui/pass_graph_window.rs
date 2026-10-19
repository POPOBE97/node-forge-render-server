use std::sync::{Arc, Mutex};

use rust_wgpu_fiber::{eframe::egui, shader_space::ShaderSpace};

use super::{
    button::{self, ButtonOptions, ButtonSize, ButtonVariant},
    design_tokens::{self, FontWeight, TextRole},
    pass_graph::{self, GraphNode, PassGraph},
    resource_tree::ResourceSnapshot,
};

const CANVAS_BG: egui::Color32 = egui::Color32::from_gray(12);
const PANEL_BG: egui::Color32 = egui::Color32::from_gray(20);
const CARD_BG: egui::Color32 = egui::Color32::from_gray(23);
const CARD: egui::Vec2 = egui::vec2(240.0, 112.0);
const STEP: egui::Vec2 = egui::vec2(284.0, 184.0);
const MIN_ZOOM: f32 = 0.04;
const MAX_ZOOM: f32 = 2.0;

#[derive(Default)]
pub struct PassGraphWindow {
    state: Arc<Mutex<WindowState>>,
}

#[derive(Default)]
struct WindowState {
    open: bool,
    focus: bool,
    initialized: bool,
    generation: Option<u64>,
    execution: Vec<String>,
    graph: PassGraph,
    view: View,
    debug_pass: Option<String>,
}

struct View {
    zoom: f32,
    pan: egui::Vec2,
    fit: bool,
    initial_frame: bool,
    focus_search: bool,
    selected: Option<usize>,
    query: String,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            fit: true,
            initial_frame: true,
            focus_search: false,
            selected: None,
            query: String::new(),
        }
    }
}

fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("render-pass-graph")
}

impl PassGraphWindow {
    pub fn open(&self, ctx: &egui::Context) {
        if let Ok(mut state) = self.state.lock() {
            state.open = true;
            state.focus = true;
        }
        ctx.request_repaint();
    }

    /// Synchronize only when the pipeline or enabled execution route changes.
    pub fn show(
        &self,
        ctx: &egui::Context,
        snapshot: Option<&ResourceSnapshot>,
        space: &ShaderSpace,
        generation: u64,
    ) -> Option<String> {
        let mut state = self.state.lock().ok()?;
        let action = state.debug_pass.take();
        if !state.open {
            return action;
        }
        let execution = pass_graph::execution_names(space);
        let changed = state.generation != Some(generation) || state.execution != execution;
        if changed {
            // A live blur route can switch without replacing the scene. Preserve the
            // user's camera whenever the new route still contains known passes.
            let reframe = state.graph.nodes.is_empty()
                || !execution.iter().any(|name| state.execution.contains(name));
            let selected = state.view.selected.and_then(|index| {
                let node = state.graph.nodes.get(index)?;
                let occurrence = state.graph.nodes[..index]
                    .iter()
                    .filter(|previous| previous.pass.name == node.pass.name)
                    .count();
                Some((node.pass.name.clone(), occurrence))
            });
            state.graph = snapshot
                .map(|snapshot| PassGraph::capture(snapshot, space))
                .unwrap_or_default();
            state.view.selected = selected.and_then(|(name, occurrence)| {
                state
                    .graph
                    .nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, node)| node.pass.name == name)
                    .nth(occurrence)
                    .map(|(index, _)| index)
            });
            if reframe {
                state.view.fit = true;
                state.view.initial_frame = true;
            }
            state.view.focus_search = !state.view.query.trim().is_empty();
            state.execution = execution;
            state.generation = Some(generation);
        }
        let mut builder = egui::ViewportBuilder::default()
            .with_title("Render Pass Dependencies")
            .with_min_inner_size([640.0, 420.0]);
        if !state.initialized {
            builder = builder.with_inner_size([1120.0, 780.0]);
            state.initialized = true;
        }
        let focus = std::mem::take(&mut state.focus);
        drop(state);
        let shared = Arc::clone(&self.state);
        ctx.show_viewport_deferred(viewport_id(), builder, move |ui, class| {
            let Ok(mut state) = shared.lock() else { return };
            if class == egui::ViewportClass::EmbeddedWindow {
                let mut open = state.open;
                egui::Window::new("Render Pass Dependencies")
                    .open(&mut open)
                    .default_size([1000.0, 720.0])
                    .resizable(true)
                    .show(ui.ctx(), |ui| contents(ui, &mut state));
                state.open = open;
            } else if ui.ctx().input(|input| input.viewport().close_requested()) {
                state.open = false;
            } else {
                contents(ui, &mut state);
            }
            if !state.open {
                state.initialized = false;
            }
            if !state.open || state.debug_pass.is_some() {
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
        });
        if focus {
            ctx.send_viewport_cmd_to(viewport_id(), egui::ViewportCommand::Focus);
        }
        if changed || focus {
            ctx.request_repaint_of(viewport_id());
        }
        action
    }
}

/// Compact graph glyph, shared by the sidebar entry and the window heading.
pub fn paint_graph_icon(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let center = rect.center();
    let top = egui::Rect::from_center_size(center + egui::vec2(0.0, -5.0), egui::vec2(6.0, 4.0));
    for x in [-5.0, 5.0] {
        let bottom =
            egui::Rect::from_center_size(center + egui::vec2(x, 5.0), egui::vec2(6.0, 4.0));
        painter.add(egui::Shape::line(
            vec![
                top.center_bottom(),
                center,
                egui::pos2(bottom.center().x, center.y),
                bottom.center_top(),
            ],
            egui::Stroke::new(1.2_f32, color),
        ));
        painter.rect_stroke(
            bottom,
            1.0,
            egui::Stroke::new(1.2_f32, color),
            egui::StrokeKind::Inside,
        );
    }
    painter.rect_stroke(
        top,
        1.0,
        egui::Stroke::new(1.2_f32, color),
        egui::StrokeKind::Inside,
    );
}

fn control(ui: &mut egui::Ui, label: &str, tooltip: &str) -> egui::Response {
    button::button(
        ui,
        ButtonOptions {
            size: ButtonSize::Small,
            variant: ButtonVariant::Secondary,
            tooltip: Some(tooltip),
            ..ButtonOptions::new(label)
        },
    )
}

fn contents(ui: &mut egui::Ui, state: &mut WindowState) {
    ui.scope(|ui| {
        // Inherit the render server's visuals, including search fields and tooltips.
        let mut zoom_factor = 1.0;
        egui::Panel::top("pass-graph-toolbar")
            .frame(
                egui::Frame::NONE
                    .fill(PANEL_BG)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    let (icon, _) =
                        ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                    paint_graph_icon(ui.painter(), icon, design_tokens::RESOURCE_ACCENT);
                    ui.label(design_tokens::rich_text(
                        "Render Passes",
                        TextRole::SectionTitle,
                    ));
                    ui.add_space(6.0);
                    ui.label(design_tokens::rich_text(
                        format!(
                            "{} passes · {} dependencies",
                            state.graph.nodes.len(),
                            state.graph.edges.len()
                        ),
                        TextRole::AttributeTitle,
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if control(ui, "Fit", "Fit the graph in the window (F)").clicked() {
                            state.view.fit = true;
                            state.view.initial_frame = false;
                        }
                        if control(ui, "+", "Zoom in").clicked() {
                            zoom_factor = 1.2;
                        }
                        if control(
                            ui,
                            &format!("{:.0}%", state.view.zoom * 100.0),
                            "Reset to 100%",
                        )
                        .clicked()
                        {
                            zoom_factor = 1.0 / state.view.zoom;
                        }
                        if control(ui, "−", "Zoom out").clicked() {
                            zoom_factor = 1.0 / 1.2;
                        }
                    });
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut state.view.query)
                            .hint_text("Find a pass or texture…")
                            .desired_width(220.0),
                    );
                    if search.changed() {
                        state.view.focus_search = true;
                        state.view.selected = None;
                    }
                    ui.add_space(10.0);
                    legend(ui, design_tokens::RESOURCE_PASS_FG, "Render");
                    legend(ui, design_tokens::RESOURCE_BUFFER_FG, "Compute");
                    legend(ui, design_tokens::RESOURCE_TEXTURE_FG, "Output");
                });
            });

        egui::Panel::bottom("pass-graph-footer")
            .frame(
                egui::Frame::NONE
                    .fill(PANEL_BG)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(design_tokens::rich_text(
                        "Drag to pan · Scroll / pinch to zoom · F to fit",
                        TextRole::AttributeTitle,
                    ));
                    ui.separator();
                    ui.label(design_tokens::rich_text(
                        "Solid: data dependency    Dashed: resource reuse order",
                        TextRole::AttributeTitle,
                    ));
                });
            });

        if let Some(index) = state
            .view
            .selected
            .filter(|&index| index < state.graph.nodes.len())
        {
            egui::Panel::right("pass-graph-details")
                .default_size(270.0)
                .resizable(false)
                .frame(egui::Frame::NONE.fill(PANEL_BG).inner_margin(16))
                .show_inside(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| details(ui, state, index));
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(CANVAS_BG))
            .show_inside(ui, |ui| canvas(ui, state, zoom_factor));
    });
}

fn legend(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 14.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.0, color);
    ui.label(design_tokens::rich_text(label, TextRole::AttributeTitle));
}

fn title(node: &GraphNode) -> &str {
    node.pass.display_label.as_deref().unwrap_or_else(|| {
        let name = node
            .pass
            .name
            .strip_prefix("sys.")
            .unwrap_or(&node.pass.name);
        name.strip_suffix(".pass").unwrap_or(name)
    })
}

fn matches_query(node: &GraphNode, query: &str) -> bool {
    query.is_empty()
        || title(node).to_lowercase().contains(query)
        || node.pass.name.to_lowercase().contains(query)
        || node
            .pass
            .target_texture
            .as_ref()
            .is_some_and(|target| target.to_lowercase().contains(query))
        || node
            .pass
            .sampled_textures
            .iter()
            .any(|texture| texture.to_lowercase().contains(query))
}

fn details(ui: &mut egui::Ui, state: &mut WindowState, index: usize) {
    ui.horizontal(|ui| {
        ui.label(design_tokens::rich_text(
            "Pass details",
            TextRole::SectionTitle,
        ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if control(ui, "×", "Close details").clicked() {
                state.view.selected = None;
            }
        });
    });
    let node = &state.graph.nodes[index];
    ui.add_space(12.0);
    ui.label(design_tokens::rich_text(
        title(node),
        TextRole::ActiveItemTitle,
    ));
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(&node.pass.name)
            .monospace()
            .size(11.0)
            .color(design_tokens::white(60)),
    );
    ui.add_space(14.0);
    if control(ui, "Open shader", "Open this pass in the shader debugger").clicked() {
        state.debug_pass = Some(node.pass.name.clone());
    }
    ui.add_space(16.0);
    detail(
        ui,
        "Execution",
        &format!(
            "#{:02} · {}",
            index + 1,
            if node.pass.is_compute {
                "Compute pass"
            } else {
                "Render pass"
            }
        ),
    );
    if let Some(target) = &node.pass.target_texture {
        detail(ui, "Target", target);
    }
    if let Some((w, h)) = node.pass.target_size {
        detail(ui, "Size", &format!("{w} × {h}"));
    }
    if let Some(format) = &node.pass.target_format {
        detail(ui, "Format", format);
    }
    if let Some(source) = &node.pass.source_node_id {
        detail(ui, "Source node", source);
    }
    if node.pass.is_compute {
        detail(
            ui,
            "Dispatch",
            &format!("{} workgroups", node.pass.workgroup_count),
        );
    } else {
        detail(
            ui,
            "Draw",
            &format!(
                "{} vertices · {} instances",
                node.pass.vertex_count, node.pass.instance_count
            ),
        );
    }
    ui.separator();
    detail(ui, "Input dependencies", "");
    let incoming: Vec<_> = state
        .graph
        .edges
        .iter()
        .filter(|edge| edge.to == index)
        .collect();
    if incoming.is_empty() {
        ui.label(design_tokens::rich_text(
            "No preceding passes",
            TextRole::InactiveItemTitle,
        ));
    }
    for edge in incoming {
        ui.label(design_tokens::rich_text(
            format!(
                "#{:02}  {}",
                edge.from + 1,
                title(&state.graph.nodes[edge.from])
            ),
            TextRole::ActiveItemTitle,
        ));
        ui.label(
            egui::RichText::new(if edge.ordering {
                "Reuse order"
            } else {
                "Reads / depends on"
            })
            .size(10.0)
            .color(design_tokens::RESOURCE_ACCENT),
        );
        for resource in &edge.resources {
            ui.label(
                egui::RichText::new(resource)
                    .size(11.0)
                    .color(design_tokens::white(60)),
            );
        }
        ui.add_space(8.0);
    }
    if !node.external_inputs.is_empty() {
        ui.separator();
        detail(ui, "External / previous frame inputs", "");
        for input in &node.external_inputs {
            ui.label(
                egui::RichText::new(input)
                    .size(11.0)
                    .color(design_tokens::white(60)),
            );
        }
    }
}

fn detail(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(design_tokens::rich_text(label, TextRole::AttributeTitle));
    if !value.is_empty() {
        ui.label(design_tokens::rich_text(value, TextRole::ValueLabel));
    }
    ui.add_space(10.0);
}

fn node_rects(graph: &PassGraph) -> Vec<egui::Rect> {
    let mut rects = vec![egui::Rect::NOTHING; graph.nodes.len()];
    for (row, nodes) in graph.rows.iter().enumerate() {
        for (column, &index) in nodes.iter().enumerate() {
            let center_x = (column as f32 - (nodes.len() as f32 - 1.0) / 2.0) * STEP.x;
            rects[index] = egui::Rect::from_min_size(
                egui::pos2(center_x - CARD.x / 2.0, row as f32 * STEP.y),
                CARD,
            );
        }
    }
    rects
}

fn zoom_at(view: &mut View, anchor: egui::Vec2, factor: f32) {
    let zoom = (view.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    view.pan = anchor - (anchor - view.pan) * (zoom / view.zoom);
    view.zoom = zoom;
}

fn canvas(ui: &mut egui::Ui, state: &mut WindowState, toolbar_zoom: f32) {
    let (canvas, response) =
        ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    let painter = ui.painter_at(canvas);
    painter.rect_filled(canvas, 0.0, CANVAS_BG);
    if state.graph.nodes.is_empty() {
        painter.text(
            canvas.center(),
            egui::Align2::CENTER_CENTER,
            "No active render passes\nLoad a scene to see its dependencies",
            egui::FontId::proportional(15.0),
            design_tokens::white(60),
        );
        return;
    }
    let rects = node_rects(&state.graph);
    let bounds = rects
        .iter()
        .fold(egui::Rect::NOTHING, |bounds, rect| bounds.union(*rect))
        .expand(44.0);
    if !ui.ctx().egui_wants_keyboard_input() && ui.input(|input| input.key_pressed(egui::Key::F)) {
        state.view.fit = true;
        state.view.initial_frame = false;
    }
    if state.view.fit {
        let fitted_zoom = (canvas.width() / bounds.width())
            .min(canvas.height() / bounds.height())
            .clamp(MIN_ZOOM, 1.0);
        if state.view.initial_frame && fitted_zoom < 0.75 {
            // Start large graphs at a readable scale. Fit remains an explicit overview.
            state.view.zoom = 0.75;
            state.view.pan = egui::vec2(
                canvas.width() / 2.0 - bounds.center().x * state.view.zoom,
                24.0 - bounds.top() * state.view.zoom,
            );
        } else {
            state.view.zoom = fitted_zoom;
            state.view.pan = canvas.size() / 2.0 - bounds.center().to_vec2() * state.view.zoom;
        }
        state.view.fit = false;
        state.view.initial_frame = false;
    }
    let query = state.view.query.trim().to_lowercase();
    if std::mem::take(&mut state.view.focus_search)
        && !query.is_empty()
        && let Some(index) = state
            .graph
            .nodes
            .iter()
            .position(|node| matches_query(node, &query))
    {
        state.view.zoom = state.view.zoom.max(0.75);
        state.view.pan = canvas.size() / 2.0 - rects[index].center().to_vec2() * state.view.zoom;
    }
    if toolbar_zoom != 1.0 {
        zoom_at(&mut state.view, canvas.size() / 2.0, toolbar_zoom);
    }
    if response.dragged() {
        state.view.pan += response.drag_delta();
    }
    if response.hovered() {
        let (zoom, scroll, pointer) = ui.input(|input| {
            (
                input.zoom_delta(),
                input.smooth_scroll_delta.y,
                input.pointer.hover_pos(),
            )
        });
        if let Some(pointer) = pointer {
            let factor = if zoom != 1.0 {
                zoom
            } else {
                (scroll * 0.0025).exp()
            };
            if factor != 1.0 {
                zoom_at(&mut state.view, pointer - canvas.min, factor);
            }
        }
        response.clone().on_hover_cursor(if response.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }
    let zoom = state.view.zoom;
    let origin = canvas.min + state.view.pan;
    paint_grid(&painter, canvas, origin, zoom);
    let screen_rects: Vec<_> = rects
        .iter()
        .map(|rect| {
            egui::Rect::from_min_size(origin + rect.min.to_vec2() * zoom, rect.size() * zoom)
        })
        .collect();
    let related = state
        .view
        .selected
        .map(|index| state.graph.connected_to(index));
    let matching: Vec<_> = state
        .graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            matches_query(node, &query) && related.as_ref().is_none_or(|set| set.contains(&index))
        })
        .collect();
    for (index, edge) in state.graph.edges.iter().enumerate() {
        let from = screen_rects[edge.from];
        let to = screen_rects[edge.to];
        let highlighted = state.view.selected.is_some() && matching[edge.from] && matching[edge.to];
        let color = if highlighted {
            design_tokens::RESOURCE_ACCENT
        } else if matching[edge.from] && matching[edge.to] {
            design_tokens::white(30)
        } else {
            design_tokens::white(10)
        };
        let points = edge_path(
            from,
            to,
            canvas.min.x + state.view.pan.x,
            bounds.left() * zoom,
            index,
            zoom,
        );
        paint_edge(
            &painter,
            &points,
            egui::Stroke::new(if highlighted { 1.7_f32 } else { 1.1_f32 }, color),
            edge.ordering,
            zoom,
        );
        if highlighted && zoom >= 0.65 && to.top() - from.bottom() < STEP.y * zoom {
            let center = egui::pos2(
                (from.center().x + to.center().x) / 2.0,
                (from.bottom() + to.top()) / 2.0,
            );
            let label = if edge.ordering {
                "reuse order"
            } else {
                "depends on"
            };
            let galley = painter.layout_no_wrap(
                label.into(),
                egui::FontId::proportional(10.0 * zoom),
                color,
            );
            let rect = egui::Rect::from_center_size(center, galley.size() + egui::vec2(10.0, 4.0));
            painter.rect_filled(rect, 4.0, CANVAS_BG);
            painter.galley(rect.center() - galley.size() / 2.0, galley, color);
        }
    }
    let mut clicked_node = false;
    for (index, node) in state.graph.nodes.iter().enumerate() {
        let rect = screen_rects[index];
        if !canvas.intersects(rect) {
            continue;
        }
        let card_response = ui.interact(rect, response.id.with(index), egui::Sense::click());
        card_response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title(node)));
        if card_response.clicked() {
            state.view.selected = Some(index);
            clicked_node = true;
        }
        if card_response.double_clicked() {
            state.debug_pass = Some(node.pass.name.clone());
        }
        paint_card(
            &painter,
            node,
            rect,
            zoom,
            state.view.selected == Some(index),
            card_response.hovered(),
            matching[index],
        );
        card_response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!(
                "{}\n{}\nClick for dependencies · Double-click to open shader",
                title(node),
                node.pass.name
            ));
    }
    if response.clicked() && !clicked_node {
        state.view.selected = None;
    }
}

fn paint_grid(painter: &egui::Painter, canvas: egui::Rect, origin: egui::Pos2, zoom: f32) {
    let step = if zoom < 0.45 { 96.0 } else { 24.0 } * zoom;
    for vertical in [true, false] {
        let (min, max, offset) = if vertical {
            (canvas.left(), canvas.right(), origin.x)
        } else {
            (canvas.top(), canvas.bottom(), origin.y)
        };
        let start = ((min - offset) / step).floor() as i32;
        let end = ((max - offset) / step).ceil() as i32;
        for index in start..=end {
            let position = offset + index as f32 * step;
            let points = if vertical {
                [
                    egui::pos2(position, canvas.top()),
                    egui::pos2(position, canvas.bottom()),
                ]
            } else {
                [
                    egui::pos2(canvas.left(), position),
                    egui::pos2(canvas.right(), position),
                ]
            };
            let shade = if index.rem_euclid(4) == 0 { 28 } else { 20 };
            painter.line_segment(
                points,
                egui::Stroke::new(0.6_f32, egui::Color32::from_gray(shade)),
            );
        }
    }
}

fn edge_path(
    from: egui::Rect,
    to: egui::Rect,
    origin_x: f32,
    left: f32,
    index: usize,
    zoom: f32,
) -> Vec<egui::Pos2> {
    let start = from.center_bottom();
    let end = to.center_top() - egui::vec2(0.0, 3.0 * zoom);
    if to.top() - from.bottom() > STEP.y * zoom {
        // Long edges travel outside the cards instead of cutting through intermediate rows.
        let lane = origin_x + left + (index % 6) as f32 * 5.0 * zoom;
        let gap = 24.0 * zoom;
        vec![
            start,
            start + egui::vec2(0.0, gap),
            egui::pos2(lane, start.y + gap),
            egui::pos2(lane, end.y - gap),
            end - egui::vec2(0.0, gap),
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

fn paint_edge(
    painter: &egui::Painter,
    points: &[egui::Pos2],
    stroke: egui::Stroke,
    dashed: bool,
    zoom: f32,
) {
    // Sample rounded orthogonal corners, so dashed and solid routes have the same shape.
    let mut rounded = vec![points[0]];
    for triple in points.windows(3) {
        let [a, b, c] = [triple[0], triple[1], triple[2]];
        let radius = (10.0 * zoom)
            .min(a.distance(b) / 2.0)
            .min(b.distance(c) / 2.0);
        let before = b + (a - b).normalized() * radius;
        let after = b + (c - b).normalized() * radius;
        rounded.push(before);
        for step in 1..=6 {
            let t = step as f32 / 6.0;
            rounded.push(egui::pos2(
                (1.0 - t).powi(2) * before.x + 2.0 * (1.0 - t) * t * b.x + t * t * after.x,
                (1.0 - t).powi(2) * before.y + 2.0 * (1.0 - t) * t * b.y + t * t * after.y,
            ));
        }
    }
    rounded.push(*points.last().unwrap_or(&points[0]));
    if dashed {
        painter.extend(egui::Shape::dashed_line(&rounded, stroke, 4.0, 4.0));
    } else {
        painter.add(egui::Shape::line(rounded, stroke));
    }
    let end = *points.last().unwrap_or(&points[0]);
    let size = (4.0 * zoom).max(2.0);
    painter.add(egui::Shape::convex_polygon(
        vec![
            end,
            end + egui::vec2(-size, -size * 1.6),
            end + egui::vec2(size, -size * 1.6),
        ],
        stroke.color,
        egui::Stroke::NONE,
    ));
}

fn paint_card(
    painter: &egui::Painter,
    node: &GraphNode,
    rect: egui::Rect,
    zoom: f32,
    selected: bool,
    hovered: bool,
    matching: bool,
) {
    let faded = |color: egui::Color32| {
        if matching {
            color
        } else {
            color.lerp_to_gamma(CANVAS_BG, 0.65)
        }
    };
    let color = if node.is_output {
        design_tokens::RESOURCE_TEXTURE_FG
    } else if node.pass.is_compute {
        design_tokens::RESOURCE_BUFFER_FG
    } else {
        design_tokens::RESOURCE_PASS_FG
    };
    let radius = design_tokens::BORDER_RADIUS_REGULAR * zoom;
    let fill = if selected {
        design_tokens::RESOURCE_ACTIVE_BG
    } else if hovered {
        egui::Color32::from_gray(32)
    } else {
        CARD_BG
    };
    painter.rect_filled(rect, radius, faded(fill));
    painter.rect_stroke(
        rect,
        radius,
        egui::Stroke::new(
            if selected { 1.6_f32 } else { 1.0_f32 },
            if selected {
                faded(design_tokens::RESOURCE_ACCENT)
            } else if hovered {
                faded(design_tokens::white(30))
            } else {
                faded(design_tokens::white(10))
            },
        ),
        egui::StrokeKind::Inside,
    );
    let at = |x, y| rect.min + egui::vec2(x, y) * zoom;
    painter.circle_stroke(
        at(17.0, 19.0),
        3.3 * zoom,
        egui::Stroke::new(zoom.max(0.6), faded(color)),
    );
    let kind = if node.is_output {
        "Final output"
    } else if node.pass.is_compute {
        "Compute pass"
    } else {
        "Render pass"
    };
    painter.text(
        at(26.0, 19.0),
        egui::Align2::LEFT_CENTER,
        kind,
        design_tokens::font_id(design_tokens::FONT_SIZE_11 * zoom, FontWeight::Normal),
        faded(color),
    );
    painter.text(
        at(225.0, 19.0),
        egui::Align2::RIGHT_CENTER,
        format!("#{:02}", node.pass.order_index + 1),
        egui::FontId::monospace(10.0 * zoom),
        faded(design_tokens::white(60)),
    );
    text_line(
        painter,
        at(14.0, 39.0),
        title(node),
        design_tokens::font_id(design_tokens::FONT_SIZE_13 * zoom, FontWeight::Medium),
        212.0 * zoom,
        faded(design_tokens::white(90)),
    );
    text_line(
        painter,
        at(14.0, 59.0),
        node.pass
            .target_texture
            .as_deref()
            .unwrap_or("Storage output"),
        design_tokens::font_id(design_tokens::FONT_SIZE_11 * zoom, FontWeight::Normal),
        212.0 * zoom,
        faded(design_tokens::white(60)),
    );
    painter.line_segment(
        [at(14.0, 82.0), at(226.0, 82.0)],
        egui::Stroke::new(0.6_f32, faded(design_tokens::white(10))),
    );
    let dimensions = node
        .pass
        .target_size
        .map(|(w, h)| format!("{w} × {h}"))
        .unwrap_or_else(|| format!("{} workgroups", node.pass.workgroup_count));
    painter.text(
        at(14.0, 96.0),
        egui::Align2::LEFT_CENTER,
        dimensions,
        design_tokens::font_id(design_tokens::FONT_SIZE_11 * zoom, FontWeight::Normal),
        faded(design_tokens::white(60)),
    );
    let count = node.pass.sampled_textures.len();
    let inputs = format!("{count} texture{}", if count == 1 { "" } else { "s" });
    painter.text(
        at(226.0, 96.0),
        egui::Align2::RIGHT_CENTER,
        inputs,
        design_tokens::font_id(design_tokens::FONT_SIZE_11 * zoom, FontWeight::Normal),
        faded(design_tokens::white(60)),
    );
}

fn text_line(
    painter: &egui::Painter,
    position: egui::Pos2,
    text: &str,
    font: egui::FontId,
    width: f32,
    color: egui::Color32,
) {
    let mut job = egui::text::LayoutJob::simple(text.into(), font, color, width);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    painter.galley(position, painter.layout_job(job), color);
}
