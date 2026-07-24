use std::collections::HashSet;

use rust_wgpu_fiber::eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use serde_json::{Map, Value, json};

use crate::{
    dsl::{Node, SceneDSL, incoming_connection},
    protocol::DesignParamPatchPayload,
    renderer::camera::{legacy_projection_camera_matrix, resolve_effective_camera_for_pass_node},
    ui::{
        color_popover::{ColorPopoverConfig, show_color_popover},
        design_tokens,
        resource_tree::PassDesignTarget,
    },
};

use super::{
    DesignInteractionClaims, DesignOverlayInput, DesignOverlayOutput, DesignOverlayStatus,
    interaction::{project_local_point, unproject_screen_point},
    state::MeshGradientDesignState,
};

const DEFAULT_GRID_ROWS: usize = 3;
const DEFAULT_GRID_COLS: usize = 3;
const MIN_GRID_SIZE: usize = 3;
const MAX_GRID_SIZE: usize = 10;
const MAX_POINT_COUNT: usize = MAX_GRID_SIZE * MAX_GRID_SIZE;
const DEFAULT_TARGET_SIZE: (u32, u32) = (1024, 768);
const HANDLE_RADIUS: f32 = 8.0;
const HANDLE_PICK_RADIUS: f32 = 28.0;

const DEFAULT_COLORS: [&str; 25] = [
    "#ff6b6b", "#ffa94d", "#ffd43b", "#f4f46b", "#c0eb75", "#69db7c", "#38d9a9", "#66d9e8",
    "#74c0fc", "#4dabf7", "#748ffc", "#91a7ff", "#ffffff", "#b197fc", "#9775fa", "#da77f2",
    "#e599f7", "#f783ac", "#faa2c1", "#ff8787", "#ffc9c9", "#ffd8a8", "#fff3bf", "#d8f5a2",
    "#b2f2bb",
];

#[derive(Clone, Debug)]
struct MeshGradientValues {
    grid_cols: usize,
    grid_rows: usize,
    positions: [[f32; 2]; MAX_POINT_COUNT],
    colors: [Color32; MAX_POINT_COUNT],
    locked_ports: HashSet<String>,
    target_size: [f32; 2],
    camera: [f32; 16],
}

pub fn is_mesh_gradient_design_param(key: &str) -> bool {
    key == "background"
        || key == "patchDivCountU"
        || key == "patchDivCountV"
        || key.strip_prefix("pos").is_some_and(|suffix| {
            suffix
                .parse::<usize>()
                .is_ok_and(|index| index < MAX_POINT_COUNT)
        })
        || key.strip_prefix("color").is_some_and(|suffix| {
            suffix
                .parse::<usize>()
                .is_ok_and(|index| index < MAX_POINT_COUNT)
        })
}

pub fn show_overlay(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    target: &PassDesignTarget,
    session_id: &str,
    state: &mut MeshGradientDesignState,
    input: DesignOverlayInput<'_>,
) -> DesignOverlayOutput {
    let _display_resolution = input.display_resolution;
    let mut output = DesignOverlayOutput {
        claims: active_claims(&input),
        ..Default::default()
    };

    if target.node_type != "MeshGradient" {
        return stale(
            ui,
            input.canvas_rect,
            target,
            "No design controller registered.",
        );
    }
    if !input.editor_connected {
        return stale(ui, input.canvas_rect, target, "Editor is disconnected.");
    }

    let pass_is_present = input
        .resource_snapshot
        .map(|snapshot| {
            snapshot
                .passes
                .iter()
                .any(|pass| pass.name == target.pass_name)
        })
        .unwrap_or(false);
    let Some(scene) = input.scene else {
        return stale(ui, input.canvas_rect, target, "Waiting for editor scene.");
    };
    let Some(node) = scene.nodes.iter().find(|node| node.id == target.node_id) else {
        return stale(
            ui,
            input.canvas_rect,
            target,
            "Target node no longer exists.",
        );
    };
    if node.node_type != target.node_type {
        return stale(ui, input.canvas_rect, target, "Target node type changed.");
    }
    if !pass_is_present {
        return stale(
            ui,
            input.canvas_rect,
            target,
            "Target pass is no longer present.",
        );
    }

    let values = read_mesh_gradient_values(scene, node, target, state);
    handle_interaction(
        ctx,
        input.pointer_response,
        input.image_rect,
        target,
        session_id,
        state,
        &values,
        &mut output.patches,
    );
    draw_preview_handles(
        &ui.painter_at(input.canvas_rect),
        input.image_rect,
        &values,
        state.selected_point,
    );
    show_selected_color_popover(
        ui,
        input.image_rect,
        target,
        session_id,
        state,
        &values,
        &mut output.patches,
    );
    output
}

fn active_claims(input: &DesignOverlayInput<'_>) -> DesignInteractionClaims {
    DesignInteractionClaims {
        primary_pointer: input
            .pointer_response
            .clicked_by(egui::PointerButton::Primary)
            || input
                .pointer_response
                .drag_started_by(egui::PointerButton::Primary)
            || input
                .pointer_response
                .dragged_by(egui::PointerButton::Primary),
        suppress_pixel_sampling: true,
        suppress_reference_drag: true,
        suppress_analysis_overlays: true,
    }
}

fn stale(
    ui: &egui::Ui,
    rect: Rect,
    target: &PassDesignTarget,
    reason: &str,
) -> DesignOverlayOutput {
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        egui::CornerRadius::ZERO,
        Color32::from_rgba_unmultiplied(0, 0, 0, 96),
    );
    let center = rect.center();
    painter.text(
        center + Vec2::new(0.0, -14.0),
        egui::Align2::CENTER_CENTER,
        reason,
        design_tokens::font_id(
            design_tokens::FONT_SIZE_13,
            design_tokens::FontWeight::Medium,
        ),
        Color32::from_rgb(236, 239, 242),
    );
    painter.text(
        center + Vec2::new(0.0, 8.0),
        egui::Align2::CENTER_CENTER,
        format!("{} / {}", target.node_id, target.pass_name),
        design_tokens::font_id(
            design_tokens::FONT_SIZE_11,
            design_tokens::FontWeight::Normal,
        ),
        Color32::from_rgb(160, 167, 174),
    );

    DesignOverlayOutput {
        claims: DesignInteractionClaims {
            suppress_pixel_sampling: true,
            suppress_reference_drag: true,
            suppress_analysis_overlays: true,
            ..Default::default()
        },
        status: DesignOverlayStatus::Stale(reason.to_string()),
        ..Default::default()
    }
}

fn handle_interaction(
    ctx: &egui::Context,
    response: &egui::Response,
    rect: Rect,
    target: &PassDesignTarget,
    session_id: &str,
    state: &mut MeshGradientDesignState,
    values: &MeshGradientValues,
    actions: &mut Vec<DesignParamPatchPayload>,
) {
    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(pointer_pos) = response.interact_pointer_pos()
        && let Some(index) = nearest_point(pointer_pos, rect, values)
    {
        state.selected_point = index;
        state.color_popover_point = None;
        let key = position_key(index);
        if values.locked_ports.contains(&key) {
            state.active_drag_point = None;
        } else {
            state.active_drag_point = Some(index);
            emit_position_patch(
                "begin",
                target,
                session_id,
                state,
                values,
                index,
                pointer_pos,
                rect,
                actions,
            );
        }
    }

    if response.dragged_by(egui::PointerButton::Primary)
        && let Some(index) = state.active_drag_point
        && let Some(pointer_pos) = response.interact_pointer_pos()
    {
        state.selected_point = index;
        emit_position_patch(
            "change",
            target,
            session_id,
            state,
            values,
            index,
            pointer_pos,
            rect,
            actions,
        );
    }

    if state.active_drag_point.is_some()
        && !ctx.input(|input| input.pointer.button_down(egui::PointerButton::Primary))
    {
        if let Some(index) = state.active_drag_point
            && let Some(pointer_pos) = ctx.input(|input| input.pointer.hover_pos())
        {
            state.selected_point = index;
            emit_position_patch(
                "end",
                target,
                session_id,
                state,
                values,
                index,
                pointer_pos,
                rect,
                actions,
            );
        }
        state.active_drag_point = None;
    }

    if response.clicked_by(egui::PointerButton::Primary) {
        if let Some(pointer_pos) = response.interact_pointer_pos()
            && let Some(index) = nearest_point(pointer_pos, rect, values)
        {
            state.selected_point = index;
            let key = color_key(index);
            if values.locked_ports.contains(&key) {
                state.color_popover_point = None;
            } else {
                state.color_popover_point = Some(index);
                state
                    .color_popover_state
                    .sync_from_color(values.colors[index]);
            }
        } else {
            state.color_popover_point = None;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_position_patch(
    phase: &str,
    target: &PassDesignTarget,
    session_id: &str,
    state: &mut MeshGradientDesignState,
    values: &MeshGradientValues,
    point_index: usize,
    pointer_pos: Pos2,
    rect: Rect,
    actions: &mut Vec<DesignParamPatchPayload>,
) {
    let key = position_key(point_index);
    if values.locked_ports.contains(&key) {
        return;
    }
    let position = screen_to_point(pointer_pos, rect, values.target_size, values.camera);
    let mut params = Map::new();
    params.insert(key, json!(position));
    emit_patch(phase, target, session_id, state, params, actions);
}

fn emit_patch(
    phase: &str,
    target: &PassDesignTarget,
    session_id: &str,
    state: &mut MeshGradientDesignState,
    params: Map<String, Value>,
    actions: &mut Vec<DesignParamPatchPayload>,
) {
    if params.is_empty() {
        return;
    }
    if params
        .keys()
        .any(|key| !is_mesh_gradient_design_param(key.as_str()))
    {
        eprintln!(
            "[pass-design] rejected patch with disallowed MeshGradient param for node {}",
            target.node_id
        );
        return;
    }
    for (key, value) in &params {
        state.optimistic_params.insert(key.clone(), value.clone());
    }
    actions.push(DesignParamPatchPayload {
        session_id: session_id.to_string(),
        node_id: target.node_id.clone(),
        node_type: target.node_type.clone(),
        phase: phase.to_string(),
        params,
    });
}

fn show_selected_color_popover(
    ui: &mut egui::Ui,
    rect: Rect,
    target: &PassDesignTarget,
    session_id: &str,
    state: &mut MeshGradientDesignState,
    values: &MeshGradientValues,
    actions: &mut Vec<DesignParamPatchPayload>,
) {
    let Some(index) = state
        .color_popover_point
        .filter(|index| *index < values.grid_cols * values.grid_rows)
    else {
        return;
    };
    let key = color_key(index);
    if values.locked_ports.contains(&key) {
        state.color_popover_point = None;
        return;
    }

    let point = point_to_screen(
        values.positions[index],
        rect,
        values.target_size,
        values.camera,
    );
    let anchor_rect = Rect::from_center_size(point, Vec2::splat(HANDLE_PICK_RADIUS));
    let mut color = values.colors[index];
    let response = show_color_popover(
        ui.ctx(),
        ui.id().with(("mesh-gradient-color", session_id, index)),
        anchor_rect,
        &mut state.color_popover_state,
        &mut color,
        ColorPopoverConfig::default(),
    );
    if response.changed {
        let mut params = Map::new();
        params.insert(key, Value::String(color_to_hex(color)));
        emit_patch("change", target, session_id, state, params, actions);
    }
    if response.close_requested {
        state.color_popover_point = None;
    }
}

fn draw_preview_handles(
    painter: &egui::Painter,
    rect: Rect,
    values: &MeshGradientValues,
    selected_point: usize,
) {
    if rect.width() <= 1.0 || rect.height() <= 1.0 {
        return;
    }

    for index in 0..(values.grid_cols * values.grid_rows) {
        let key = position_key(index);
        let center = point_to_screen(
            values.positions[index],
            rect,
            values.target_size,
            values.camera,
        );
        let locked = values.locked_ports.contains(&key);
        let fill = if locked {
            design_tokens::white(30)
        } else {
            values.colors[index]
        };
        let stroke = if index == selected_point {
            Stroke::new(2.0_f32, design_tokens::white(100))
        } else {
            Stroke::new(1.0_f32, design_tokens::white(50))
        };
        painter.circle_filled(
            center + Vec2::new(0.0, 1.0),
            HANDLE_RADIUS + 3.0,
            design_tokens::black(60),
        );
        painter.circle_filled(center, HANDLE_RADIUS + 2.0, design_tokens::black(80));
        painter.circle_filled(center, HANDLE_RADIUS, fill);
        painter.circle_stroke(center, HANDLE_RADIUS + 1.5, stroke);
        if locked {
            painter.line_segment(
                [
                    Pos2::new(center.x - 4.0, center.y),
                    Pos2::new(center.x + 4.0, center.y),
                ],
                Stroke::new(1.0_f32, design_tokens::white(90)),
            );
        }
    }
}

fn read_mesh_gradient_values(
    scene: &SceneDSL,
    node: &Node,
    target: &PassDesignTarget,
    state: &MeshGradientDesignState,
) -> MeshGradientValues {
    let target_size = target.target_size.unwrap_or(DEFAULT_TARGET_SIZE);
    let target_size = [target_size.0 as f32, target_size.1 as f32];
    let locked_ports = locked_mesh_gradient_ports(scene, node.id.as_str());
    let grid_cols = read_grid_size(node, state, "width", DEFAULT_GRID_COLS);
    let grid_rows = read_grid_size(node, state, "height", DEFAULT_GRID_ROWS);
    let point_count = grid_cols * grid_rows;
    let mut positions = [[0.0; 2]; MAX_POINT_COUNT];
    let mut colors = [Color32::WHITE; MAX_POINT_COUNT];
    let nodes_by_id = scene
        .nodes
        .iter()
        .map(|candidate| (candidate.id.clone(), candidate.clone()))
        .collect();
    let camera = resolve_effective_camera_for_pass_node(scene, &nodes_by_id, node, target_size)
        .unwrap_or_else(|_| legacy_projection_camera_matrix(target_size));

    for index in 0..point_count {
        let pos_key = position_key(index);
        positions[index] = param_value(node, state, &pos_key)
            .and_then(parse_vec2_value)
            .unwrap_or_else(|| default_position(index, grid_cols, grid_rows, target_size));

        let color_key = color_key(index);
        colors[index] = param_value(node, state, &color_key)
            .and_then(parse_color_value)
            .unwrap_or_else(|| parse_hex_color(default_color_hex(index)).unwrap_or(Color32::WHITE));
    }

    MeshGradientValues {
        grid_cols,
        grid_rows,
        positions,
        colors,
        locked_ports,
        target_size,
        camera,
    }
}

fn read_grid_size(
    node: &Node,
    state: &MeshGradientDesignState,
    key: &str,
    fallback: usize,
) -> usize {
    param_value(node, state, key)
        .and_then(json_f32)
        .map(|value| value.trunc() as usize)
        .unwrap_or(fallback)
        .clamp(MIN_GRID_SIZE, MAX_GRID_SIZE)
}

fn locked_mesh_gradient_ports(scene: &SceneDSL, node_id: &str) -> HashSet<String> {
    let mut locked = HashSet::new();
    for index in 0..MAX_POINT_COUNT {
        for port_id in [position_key(index), color_key(index)] {
            if incoming_connection(scene, node_id, &port_id).is_some() {
                locked.insert(port_id);
            }
        }
    }
    for port_id in ["background", "patchDivCountU", "patchDivCountV"] {
        if incoming_connection(scene, node_id, port_id).is_some() {
            locked.insert(port_id.to_string());
        }
    }
    locked
}

fn param_value<'a>(
    node: &'a Node,
    state: &'a MeshGradientDesignState,
    key: &str,
) -> Option<&'a Value> {
    state
        .optimistic_params
        .get(key)
        .or_else(|| node.params.get(key))
}

fn nearest_point(pointer_pos: Pos2, rect: Rect, values: &MeshGradientValues) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for index in 0..(values.grid_cols * values.grid_rows) {
        let point = point_to_screen(
            values.positions[index],
            rect,
            values.target_size,
            values.camera,
        );
        let distance = point.distance(pointer_pos);
        if distance <= HANDLE_PICK_RADIUS
            && best.is_none_or(|(_, best_distance)| distance < best_distance)
        {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

fn point_to_screen(
    position: [f32; 2],
    rect: Rect,
    target_size: [f32; 2],
    camera: [f32; 16],
) -> Pos2 {
    project_local_point(position, rect, target_size, camera)
}

fn screen_to_point(pos: Pos2, rect: Rect, target_size: [f32; 2], camera: [f32; 16]) -> [f32; 2] {
    let point = unproject_screen_point(pos, rect, target_size, camera);
    [
        point[0].clamp(0.0, target_size[0].max(1.0)),
        point[1].clamp(0.0, target_size[1].max(1.0)),
    ]
}

fn default_position(
    index: usize,
    grid_cols: usize,
    grid_rows: usize,
    target_size: [f32; 2],
) -> [f32; 2] {
    let row = index / grid_cols;
    let col = index % grid_cols;
    [
        col as f32 / (grid_cols - 1) as f32 * target_size[0],
        row as f32 / (grid_rows - 1) as f32 * target_size[1],
    ]
}

fn position_key(index: usize) -> String {
    format!("pos{index}")
}

fn color_key(index: usize) -> String {
    format!("color{index}")
}

fn default_color_hex(index: usize) -> &'static str {
    DEFAULT_COLORS[index % DEFAULT_COLORS.len()]
}

fn parse_vec2_value(value: &Value) -> Option<[f32; 2]> {
    if let Some(arr) = value.as_array() {
        return Some([
            arr.first().and_then(json_f32).unwrap_or(0.0),
            arr.get(1).and_then(json_f32).unwrap_or(0.0),
        ]);
    }
    if let Some(obj) = value.as_object() {
        return Some([
            obj.get("x").and_then(json_f32).unwrap_or(0.0),
            obj.get("y").and_then(json_f32).unwrap_or(0.0),
        ]);
    }
    None
}

fn parse_color_value(value: &Value) -> Option<Color32> {
    if let Some(hex) = value.as_str() {
        return parse_hex_color(hex);
    }
    if let Some(arr) = value.as_array() {
        return Some(Color32::from_rgba_unmultiplied(
            color_component_to_u8(arr.first().and_then(json_f32).unwrap_or(0.0)),
            color_component_to_u8(arr.get(1).and_then(json_f32).unwrap_or(0.0)),
            color_component_to_u8(arr.get(2).and_then(json_f32).unwrap_or(0.0)),
            color_component_to_u8(arr.get(3).and_then(json_f32).unwrap_or(1.0)),
        ));
    }
    if let Some(obj) = value.as_object() {
        return Some(Color32::from_rgba_unmultiplied(
            color_component_to_u8(obj.get("r").and_then(json_f32).unwrap_or(0.0)),
            color_component_to_u8(obj.get("g").and_then(json_f32).unwrap_or(0.0)),
            color_component_to_u8(obj.get("b").and_then(json_f32).unwrap_or(0.0)),
            color_component_to_u8(obj.get("a").and_then(json_f32).unwrap_or(1.0)),
        ));
    }
    None
}

fn parse_hex_color(value: &str) -> Option<Color32> {
    let raw = value.trim().strip_prefix('#')?;
    match raw.len() {
        3 => {
            let r = u8::from_str_radix(&raw[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&raw[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&raw[2..3], 16).ok()? * 17;
            Some(Color32::from_rgb(r, g, b))
        }
        4 => {
            let r = u8::from_str_radix(&raw[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&raw[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&raw[2..3], 16).ok()? * 17;
            let a = u8::from_str_radix(&raw[3..4], 16).ok()? * 17;
            Some(Color32::from_rgba_unmultiplied(r, g, b, a))
        }
        6 => Some(Color32::from_rgb(
            u8::from_str_radix(&raw[0..2], 16).ok()?,
            u8::from_str_radix(&raw[2..4], 16).ok()?,
            u8::from_str_radix(&raw[4..6], 16).ok()?,
        )),
        8 => Some(Color32::from_rgba_unmultiplied(
            u8::from_str_radix(&raw[0..2], 16).ok()?,
            u8::from_str_radix(&raw[2..4], 16).ok()?,
            u8::from_str_radix(&raw[4..6], 16).ok()?,
            u8::from_str_radix(&raw[6..8], 16).ok()?,
        )),
        _ => None,
    }
}

fn json_f32(value: &Value) -> Option<f32> {
    let value = value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_u64().map(|value| value as f64))?;
    value.is_finite().then_some(value as f32)
}

fn color_component_to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn color_to_hex(color: Color32) -> String {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    if a == u8::MAX {
        format!("#{:02x}{:02x}{:02x}", r, g, b)
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", r, g, b, a)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn color_hex_round_trips_to_patch_string() {
        assert_eq!(color_to_hex(parse_hex_color("#fc0").unwrap()), "#ffcc00");
        let translucent = color_to_hex(parse_hex_color("#11223344").unwrap());
        assert!(translucent.starts_with('#'));
        assert_eq!(translucent.len(), 9);
        assert!(translucent.ends_with("44"));
    }

    #[test]
    fn default_positions_use_target_pixel_space() {
        assert_eq!(default_position(0, 3, 3, [400.0, 200.0]), [0.0, 0.0]);
        assert_eq!(default_position(4, 3, 3, [400.0, 200.0]), [200.0, 100.0]);
        assert_eq!(default_position(8, 3, 3, [400.0, 200.0]), [400.0, 200.0]);
        assert_eq!(default_position(24, 5, 5, [400.0, 200.0]), [400.0, 200.0]);
        assert_eq!(default_position(99, 10, 10, [400.0, 200.0]), [400.0, 200.0]);
    }

    #[test]
    fn default_color_palette_wraps_for_large_grids() {
        assert_eq!(default_color_hex(0), "#ff6b6b");
        assert_eq!(default_color_hex(25), "#ff6b6b");
    }

    #[test]
    fn screen_mapping_uses_render_bottom_origin_for_y() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(400.0, 200.0));
        let target_size = [400.0, 200.0];
        let camera = legacy_projection_camera_matrix(target_size);

        assert_eq!(
            point_to_screen([0.0, 0.0], rect, target_size, camera),
            Pos2::new(10.0, 220.0)
        );
        assert_eq!(
            point_to_screen([400.0, 200.0], rect, target_size, camera),
            Pos2::new(410.0, 20.0)
        );
        assert_eq!(
            screen_to_point(Pos2::new(10.0, 20.0), rect, target_size, camera),
            [0.0, 200.0]
        );
        assert_eq!(
            screen_to_point(Pos2::new(410.0, 220.0), rect, target_size, camera),
            [400.0, 0.0]
        );
    }

    #[test]
    fn screen_mapping_tracks_mesh_gradient_pass_camera() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0));
        let target_size = [400.0, 200.0];
        let mut camera = legacy_projection_camera_matrix(target_size);
        camera[13] -= 0.5;
        let local_point = [200.0, 100.0];

        let screen_point = point_to_screen(local_point, rect, target_size, camera);

        assert_eq!(screen_point, Pos2::new(200.0, 150.0));
        assert_eq!(
            screen_to_point(screen_point, rect, target_size, camera),
            local_point
        );
    }

    #[test]
    fn mesh_gradient_design_param_allowlist_rejects_unknown_keys() {
        assert!(is_mesh_gradient_design_param("pos0"));
        assert!(is_mesh_gradient_design_param("pos8"));
        assert!(is_mesh_gradient_design_param("pos24"));
        assert!(is_mesh_gradient_design_param("pos99"));
        assert!(is_mesh_gradient_design_param("color0"));
        assert!(is_mesh_gradient_design_param("color8"));
        assert!(is_mesh_gradient_design_param("color24"));
        assert!(is_mesh_gradient_design_param("color99"));
        assert!(is_mesh_gradient_design_param("background"));
        assert!(is_mesh_gradient_design_param("patchDivCountU"));
        assert!(is_mesh_gradient_design_param("patchDivCountV"));
        assert!(!is_mesh_gradient_design_param("pos100"));
        assert!(!is_mesh_gradient_design_param("camera"));
    }

    #[test]
    fn connected_mesh_gradient_ports_are_locked() {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: crate::dsl::Metadata {
                name: "lock test".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                Node {
                    id: "vec".to_string(),
                    node_type: "Vector2Input".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    outputs: vec![],
                    input_bindings: vec![],
                    wgsl_override: None,
                },
                Node {
                    id: "mesh".to_string(),
                    node_type: "MeshGradient".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    outputs: vec![],
                    input_bindings: vec![],
                    wgsl_override: None,
                },
            ],
            connections: vec![crate::dsl::Connection {
                id: "edge".to_string(),
                from: crate::dsl::Endpoint {
                    node_id: "vec".to_string(),
                    port_id: "vector".to_string(),
                },
                to: crate::dsl::Endpoint {
                    node_id: "mesh".to_string(),
                    port_id: "pos4".to_string(),
                },
            }],
            outputs: None,
            groups: vec![],
            assets: HashMap::new(),
            state_machine: None,
            debug_artifacts: None,
        };

        let locked = locked_mesh_gradient_ports(&scene, "mesh");
        assert!(locked.contains("pos4"));
        assert!(!locked.contains("pos5"));
    }
}
