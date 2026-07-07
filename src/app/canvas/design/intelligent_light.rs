use rust_wgpu_fiber::eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use serde_json::{Map, Value, json};

use crate::{
    dsl::{Node, SceneDSL, incoming_connection},
    protocol::DesignParamPatchPayload,
    renderer::render_plan::pass_assemblers::intelligent_light::{
        INTELLIGENT_LIGHT_ZONE_COUNT, default_light_position, procedural_positions,
    },
    ui::{
        color_popover::{ColorPopoverConfig, show_color_popover},
        design_tokens,
        resource_tree::PassDesignTarget,
    },
};

use super::{
    DesignInteractionClaims, DesignOverlayInput, DesignOverlayOutput, DesignOverlayStatus,
    state::IntelligentLightDesignState,
};

const DEFAULT_TARGET_SIZE: (u32, u32) = (60, 37);
const HANDLE_RADIUS: f32 = 7.0;
const HANDLE_PICK_RADIUS: f32 = 24.0;

const DEFAULT_COLOR_HEXES: [&str; INTELLIGENT_LIGHT_ZONE_COUNT] = [
    "#8086ff", "#ffd3b3", "#ff8635", "#847eff", "#1269f2", "#8086ff", "#ffd3b3", "#ff8635",
    "#ff8635", "#1269f2", "#847eff",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IntelligentLightLayoutMode {
    Procedural,
    Manual,
}

#[derive(Clone, Debug)]
struct IntelligentLightValues {
    layout_mode: IntelligentLightLayoutMode,
    positions: [[f32; 2]; INTELLIGENT_LIGHT_ZONE_COUNT],
    colors: [Color32; INTELLIGENT_LIGHT_ZONE_COUNT],
    position_locks: [bool; INTELLIGENT_LIGHT_ZONE_COUNT],
    color_locks: [bool; INTELLIGENT_LIGHT_ZONE_COUNT],
    position_space: [f32; 2],
}

pub fn is_intelligent_light_design_param(key: &str) -> bool {
    key == "layoutMode"
        || key.strip_prefix("pos").is_some_and(|suffix| {
            suffix
                .parse::<usize>()
                .is_ok_and(|index| index < INTELLIGENT_LIGHT_ZONE_COUNT)
        })
        || key.strip_prefix("color").is_some_and(|suffix| {
            suffix
                .parse::<usize>()
                .is_ok_and(|index| index < INTELLIGENT_LIGHT_ZONE_COUNT)
        })
}

pub fn show_overlay(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    target: &PassDesignTarget,
    session_id: &str,
    state: &mut IntelligentLightDesignState,
    input: DesignOverlayInput<'_>,
) -> DesignOverlayOutput {
    let _display_resolution = input.display_resolution;
    let mut output = DesignOverlayOutput {
        claims: active_claims(&input),
        ..Default::default()
    };

    if target.node_type != "IntelligentLight" {
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

    let values = read_intelligent_light_values(scene, node, target, state, input.animation_playing);
    if !input.animation_playing {
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
    }
    draw_preview_handles(
        &ui.painter_at(input.canvas_rect),
        input.image_rect,
        &values,
        state.selected_zone,
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

pub fn cancel_color_edit(
    target: &PassDesignTarget,
    session_id: &str,
    state: &mut IntelligentLightDesignState,
) -> Option<DesignParamPatchPayload> {
    let index = state.color_popover_zone?;
    let original_hex = state.color_edit_original_hex.clone()?;
    let key = color_key(index);
    state.color_popover_zone = None;
    state.color_edit_original_hex = None;
    state
        .optimistic_params
        .insert(key.clone(), Value::String(original_hex.clone()));

    let mut params = Map::new();
    params.insert(key, Value::String(original_hex));
    Some(DesignParamPatchPayload {
        session_id: session_id.to_string(),
        node_id: target.node_id.clone(),
        node_type: target.node_type.clone(),
        phase: "cancel".to_string(),
        params,
    })
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
    state: &mut IntelligentLightDesignState,
    values: &IntelligentLightValues,
    actions: &mut Vec<DesignParamPatchPayload>,
) {
    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(pointer_pos) = response.interact_pointer_pos()
        && let Some(index) = nearest_zone(pointer_pos, rect, values)
    {
        state.selected_zone = index;
        if let Some(patch) = end_active_color_edit(target, session_id, state, values) {
            actions.push(patch);
        }
        if values.position_locks[index] {
            state.active_drag_zone = None;
        } else {
            state.active_drag_zone = Some(index);
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
        && let Some(index) = state.active_drag_zone
        && let Some(pointer_pos) = response.interact_pointer_pos()
    {
        state.selected_zone = index;
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

    if state.active_drag_zone.is_some()
        && !ctx.input(|input| input.pointer.button_down(egui::PointerButton::Primary))
    {
        if let Some(index) = state.active_drag_zone
            && let Some(pointer_pos) = ctx.input(|input| input.pointer.hover_pos())
        {
            state.selected_zone = index;
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
        state.active_drag_zone = None;
    }

    if response.clicked_by(egui::PointerButton::Primary) {
        if let Some(pointer_pos) = response.interact_pointer_pos()
            && let Some(index) = nearest_zone(pointer_pos, rect, values)
        {
            state.selected_zone = index;
            if values.color_locks[index] {
                if let Some(patch) = end_active_color_edit(target, session_id, state, values) {
                    actions.push(patch);
                } else {
                    state.color_popover_zone = None;
                    state.color_edit_original_hex = None;
                }
            } else {
                open_color_popover(target, session_id, state, values, index, actions);
            }
        } else if let Some(patch) = end_active_color_edit(target, session_id, state, values) {
            actions.push(patch);
        } else {
            state.color_popover_zone = None;
            state.color_edit_original_hex = None;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_position_patch(
    phase: &str,
    target: &PassDesignTarget,
    session_id: &str,
    state: &mut IntelligentLightDesignState,
    values: &IntelligentLightValues,
    zone_index: usize,
    pointer_pos: Pos2,
    rect: Rect,
    actions: &mut Vec<DesignParamPatchPayload>,
) {
    if values.position_locks[zone_index] {
        return;
    }

    let position = screen_to_point(pointer_pos, rect, values.position_space);
    let mut params = Map::new();
    if phase == "begin" && values.layout_mode == IntelligentLightLayoutMode::Procedural {
        params.insert(
            "layoutMode".to_string(),
            Value::String("manual".to_string()),
        );
        for index in 0..INTELLIGENT_LIGHT_ZONE_COUNT {
            params.insert(position_key(index), json!(values.positions[index]));
        }
    }
    params.insert(position_key(zone_index), json!(position));
    emit_patch(phase, target, session_id, state, params, actions);
}

fn emit_patch(
    phase: &str,
    target: &PassDesignTarget,
    session_id: &str,
    state: &mut IntelligentLightDesignState,
    params: Map<String, Value>,
    actions: &mut Vec<DesignParamPatchPayload>,
) {
    if params.is_empty() {
        return;
    }
    if params
        .keys()
        .any(|key| !is_intelligent_light_design_param(key.as_str()))
    {
        eprintln!(
            "[pass-design] rejected patch with disallowed IntelligentLight param for node {}",
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

fn open_color_popover(
    target: &PassDesignTarget,
    session_id: &str,
    state: &mut IntelligentLightDesignState,
    values: &IntelligentLightValues,
    index: usize,
    actions: &mut Vec<DesignParamPatchPayload>,
) {
    if state.color_popover_zone == Some(index) {
        return;
    }

    if let Some(patch) = end_active_color_edit(target, session_id, state, values) {
        actions.push(patch);
    }
    let color_hex = color_to_opaque_hex(values.colors[index]);
    state.color_popover_zone = Some(index);
    state.color_edit_original_hex = Some(color_hex.clone());
    state
        .color_popover_state
        .sync_from_color(values.colors[index]);

    let mut params = Map::new();
    params.insert(color_key(index), Value::String(color_hex));
    actions.push(DesignParamPatchPayload {
        session_id: session_id.to_string(),
        node_id: target.node_id.clone(),
        node_type: target.node_type.clone(),
        phase: "begin".to_string(),
        params,
    });
}

fn end_active_color_edit(
    target: &PassDesignTarget,
    session_id: &str,
    state: &mut IntelligentLightDesignState,
    values: &IntelligentLightValues,
) -> Option<DesignParamPatchPayload> {
    let index = state.color_popover_zone?;
    state.color_popover_zone = None;
    state.color_edit_original_hex = None;

    let mut params = Map::new();
    params.insert(
        color_key(index),
        Value::String(color_to_opaque_hex(values.colors[index])),
    );
    Some(DesignParamPatchPayload {
        session_id: session_id.to_string(),
        node_id: target.node_id.clone(),
        node_type: target.node_type.clone(),
        phase: "end".to_string(),
        params,
    })
}

fn show_selected_color_popover(
    ui: &mut egui::Ui,
    rect: Rect,
    target: &PassDesignTarget,
    session_id: &str,
    state: &mut IntelligentLightDesignState,
    values: &IntelligentLightValues,
    actions: &mut Vec<DesignParamPatchPayload>,
) {
    let Some(index) = state.color_popover_zone else {
        return;
    };
    if index >= INTELLIGENT_LIGHT_ZONE_COUNT {
        return;
    }
    if values.color_locks[index] {
        state.color_popover_zone = None;
        state.color_edit_original_hex = None;
        return;
    }

    let point = point_to_screen(values.positions[index], rect, values.position_space);
    let anchor_rect = Rect::from_center_size(point, Vec2::splat(HANDLE_PICK_RADIUS));
    let mut color = values.colors[index];
    let response = show_color_popover(
        ui.ctx(),
        ui.id().with(("intelligent-light-color", session_id, index)),
        anchor_rect,
        &mut state.color_popover_state,
        &mut color,
        ColorPopoverConfig {
            title: None,
            allow_alpha: false,
        },
    );
    if response.changed {
        let mut params = Map::new();
        params.insert(color_key(index), Value::String(color_to_opaque_hex(color)));
        emit_patch("change", target, session_id, state, params, actions);
    }
    if response.close_requested {
        state.color_popover_zone = None;
        state.color_edit_original_hex = None;
        let mut params = Map::new();
        params.insert(color_key(index), Value::String(color_to_opaque_hex(color)));
        actions.push(DesignParamPatchPayload {
            session_id: session_id.to_string(),
            node_id: target.node_id.clone(),
            node_type: target.node_type.clone(),
            phase: "end".to_string(),
            params,
        });
    }
}

fn draw_preview_handles(
    painter: &egui::Painter,
    rect: Rect,
    values: &IntelligentLightValues,
    selected_zone: usize,
) {
    if rect.width() <= 1.0 || rect.height() <= 1.0 {
        return;
    }

    for index in 0..INTELLIGENT_LIGHT_ZONE_COUNT {
        let center = point_to_screen(values.positions[index], rect, values.position_space);
        let fill = values.colors[index];
        let stroke = if index == selected_zone {
            Stroke::new(2.0, design_tokens::white(100))
        } else {
            Stroke::new(1.0, design_tokens::white(48))
        };
        painter.circle_filled(
            center,
            HANDLE_RADIUS + 8.0,
            Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 32),
        );
        painter.circle_filled(
            center + Vec2::new(0.0, 1.0),
            HANDLE_RADIUS + 3.0,
            design_tokens::black(64),
        );
        painter.circle_filled(center, HANDLE_RADIUS + 2.0, design_tokens::black(84));
        painter.circle_filled(center, HANDLE_RADIUS, fill);
        painter.circle_stroke(center, HANDLE_RADIUS + 1.5, stroke);

        if values.position_locks[index] {
            painter.line_segment(
                [
                    Pos2::new(center.x - 4.0, center.y),
                    Pos2::new(center.x + 4.0, center.y),
                ],
                Stroke::new(1.0, design_tokens::white(90)),
            );
        }
        if values.color_locks[index] {
            painter.line_segment(
                [
                    Pos2::new(center.x, center.y - 4.0),
                    Pos2::new(center.x, center.y + 4.0),
                ],
                Stroke::new(1.0, design_tokens::white(90)),
            );
        }
    }
}

fn read_intelligent_light_values(
    scene: &SceneDSL,
    node: &Node,
    _target: &PassDesignTarget,
    state: &IntelligentLightDesignState,
    animation_playing: bool,
) -> IntelligentLightValues {
    let position_space = [
        node.params
            .get("width")
            .and_then(json_f32)
            .unwrap_or(DEFAULT_TARGET_SIZE.0 as f32)
            .max(1.0),
        node.params
            .get("height")
            .and_then(json_f32)
            .unwrap_or(DEFAULT_TARGET_SIZE.1 as f32)
            .max(1.0),
    ];
    let layout_mode = read_layout_mode(node, state);
    let driver = resolve_driver_value(scene, node);
    let mut positions = [[0.0; 2]; INTELLIGENT_LIGHT_ZONE_COUNT];
    let mut colors = [Color32::WHITE; INTELLIGENT_LIGHT_ZONE_COUNT];
    let mut position_locks = [false; INTELLIGENT_LIGHT_ZONE_COUNT];
    let mut color_locks = [false; INTELLIGENT_LIGHT_ZONE_COUNT];

    let procedural_positions = if layout_mode == IntelligentLightLayoutMode::Procedural {
        Some(procedural_positions(driver))
    } else {
        None
    };

    for index in 0..INTELLIGENT_LIGHT_ZONE_COUNT {
        let pos_key = position_key(index);
        let position_connection = incoming_connection(scene, node.id.as_str(), pos_key.as_str());
        position_locks[index] = position_connection.is_some();
        positions[index] = if let Some(live_positions) = procedural_positions.as_ref() {
            clamp_pixel_position(
                [live_positions[index].0, live_positions[index].1],
                position_space,
            )
        } else {
            position_connection
                .and_then(|conn| {
                    resolve_connected_position_value(
                        scene,
                        &conn.from.node_id,
                        &conn.from.port_id,
                        position_space,
                    )
                })
                .or_else(|| {
                    param_value(node, state, pos_key.as_str(), animation_playing)
                        .and_then(|value| parse_pixel_vec2_value(value, position_space))
                })
                .unwrap_or_else(|| default_light_position(index, position_space))
        };

        let color_key = color_key(index);
        let color_connection = incoming_connection(scene, node.id.as_str(), color_key.as_str());
        color_locks[index] = color_connection.is_some();
        colors[index] = color_connection
            .and_then(|conn| {
                resolve_connected_color_value(scene, &conn.from.node_id, &conn.from.port_id)
            })
            .or_else(|| {
                param_value(node, state, color_key.as_str(), animation_playing)
                    .and_then(parse_color_value)
            })
            .unwrap_or_else(|| {
                parse_hex_color(DEFAULT_COLOR_HEXES[index]).unwrap_or(Color32::WHITE)
            });
    }

    IntelligentLightValues {
        layout_mode,
        positions,
        colors,
        position_locks,
        color_locks,
        position_space,
    }
}

fn resolve_connected_position_value(
    scene: &SceneDSL,
    node_id: &str,
    port_id: &str,
    position_space: [f32; 2],
) -> Option<[f32; 2]> {
    let upstream = scene
        .nodes
        .iter()
        .find(|candidate| candidate.id == node_id)?;
    if upstream.node_type == "Vector2Input" {
        return Some(resolve_pixel_position(
            [
                upstream.params.get("x").and_then(json_f32).unwrap_or(0.0),
                upstream.params.get("y").and_then(json_f32).unwrap_or(0.0),
            ],
            position_space,
        ));
    }

    parse_pixel_vec2_value(
        upstream
            .params
            .get("value")
            .or_else(|| upstream.params.get(port_id))?,
        position_space,
    )
}

fn resolve_connected_color_value(
    scene: &SceneDSL,
    node_id: &str,
    port_id: &str,
) -> Option<Color32> {
    let upstream = scene
        .nodes
        .iter()
        .find(|candidate| candidate.id == node_id)?;
    parse_color_value(
        upstream
            .params
            .get("value")
            .or_else(|| upstream.params.get(port_id))?,
    )
}

fn read_layout_mode(
    _node: &Node,
    _state: &IntelligentLightDesignState,
) -> IntelligentLightLayoutMode {
    // Intelligent Light now edits in manual mode only.
    IntelligentLightLayoutMode::Manual
}

fn resolve_driver_value(scene: &SceneDSL, node: &Node) -> f64 {
    if let Some(conn) = incoming_connection(scene, node.id.as_str(), "driver")
        && let Some(upstream) = scene
            .nodes
            .iter()
            .find(|candidate| candidate.id == conn.from.node_id)
    {
        return upstream
            .params
            .get("value")
            .and_then(json_f64)
            .unwrap_or_else(|| node.params.get("driver").and_then(json_f64).unwrap_or(0.0));
    }

    node.params.get("driver").and_then(json_f64).unwrap_or(0.0)
}

fn param_value<'a>(
    node: &'a Node,
    state: &'a IntelligentLightDesignState,
    key: &str,
    animation_playing: bool,
) -> Option<&'a Value> {
    if animation_playing {
        return node
            .params
            .get(key)
            .or_else(|| state.optimistic_params.get(key));
    }
    state
        .optimistic_params
        .get(key)
        .or_else(|| node.params.get(key))
}

fn nearest_zone(pointer_pos: Pos2, rect: Rect, values: &IntelligentLightValues) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for index in 0..INTELLIGENT_LIGHT_ZONE_COUNT {
        let point = point_to_screen(values.positions[index], rect, values.position_space);
        let distance = point.distance(pointer_pos);
        if distance <= HANDLE_PICK_RADIUS
            && best.is_none_or(|(_, best_distance)| distance < best_distance)
        {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

fn point_to_screen(position: [f32; 2], rect: Rect, position_space: [f32; 2]) -> Pos2 {
    Pos2::new(
        rect.left() + position[0] / position_space[0].max(1.0) * rect.width(),
        rect.bottom() - position[1] / position_space[1].max(1.0) * rect.height(),
    )
}

fn screen_to_point(pos: Pos2, rect: Rect, position_space: [f32; 2]) -> [f32; 2] {
    let x = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) * position_space[0].max(1.0);
    let y = ((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0) * position_space[1].max(1.0);
    [round_position_value(x), round_position_value(y)]
}

fn round_position_value(value: f32) -> f32 {
    (value * 10.0).round() / 10.0
}

fn position_key(index: usize) -> String {
    format!("pos{index}")
}

fn color_key(index: usize) -> String {
    format!("color{index}")
}

fn clamp_pixel_position(position: [f32; 2], position_space: [f32; 2]) -> [f32; 2] {
    [
        position[0].clamp(0.0, position_space[0].max(1.0)),
        position[1].clamp(0.0, position_space[1].max(1.0)),
    ]
}

fn is_legacy_normalized_position(position: [f32; 2]) -> bool {
    (0.0..=1.0).contains(&position[0]) && (0.0..=1.0).contains(&position[1])
}

fn resolve_pixel_position(position: [f32; 2], position_space: [f32; 2]) -> [f32; 2] {
    let pixel = if is_legacy_normalized_position(position) {
        [
            position[0].clamp(0.0, 1.0) * position_space[0].max(1.0),
            (1.0 - position[1].clamp(0.0, 1.0)) * position_space[1].max(1.0),
        ]
    } else {
        position
    };
    clamp_pixel_position(pixel, position_space)
}

fn parse_pixel_vec2_value(value: &Value, position_space: [f32; 2]) -> Option<[f32; 2]> {
    if let Some(arr) = value.as_array() {
        return Some(resolve_pixel_position(
            [
                arr.first().and_then(json_f32).unwrap_or(0.0),
                arr.get(1).and_then(json_f32).unwrap_or(0.0),
            ],
            position_space,
        ));
    }
    if let Some(obj) = value.as_object() {
        return Some(resolve_pixel_position(
            [
                obj.get("x").and_then(json_f32).unwrap_or(0.0),
                obj.get("y").and_then(json_f32).unwrap_or(0.0),
            ],
            position_space,
        ));
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
        3 => Some(Color32::from_rgb(
            u8::from_str_radix(&raw[0..1], 16).ok()? * 17,
            u8::from_str_radix(&raw[1..2], 16).ok()? * 17,
            u8::from_str_radix(&raw[2..3], 16).ok()? * 17,
        )),
        4 => Some(Color32::from_rgba_unmultiplied(
            u8::from_str_radix(&raw[0..1], 16).ok()? * 17,
            u8::from_str_radix(&raw[1..2], 16).ok()? * 17,
            u8::from_str_radix(&raw[2..3], 16).ok()? * 17,
            u8::from_str_radix(&raw[3..4], 16).ok()? * 17,
        )),
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

fn json_f64(value: &Value) -> Option<f64> {
    let value = value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_u64().map(|value| value as f64))?;
    value.is_finite().then_some(value)
}

fn color_component_to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn color_to_opaque_hex(color: Color32) -> String {
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn test_target() -> PassDesignTarget {
        PassDesignTarget {
            node_id: "ilight".to_string(),
            node_type: "IntelligentLight".to_string(),
            pass_name: "sys.ilight.ilight.upsample.pass".to_string(),
            target_texture: None,
            target_size: Some((60, 37)),
        }
    }

    fn test_values(layout_mode: IntelligentLightLayoutMode) -> IntelligentLightValues {
        IntelligentLightValues {
            layout_mode,
            positions: std::array::from_fn(|index| default_light_position(index, [60.0, 37.0])),
            colors: [Color32::WHITE; INTELLIGENT_LIGHT_ZONE_COUNT],
            position_locks: [false; INTELLIGENT_LIGHT_ZONE_COUNT],
            color_locks: [false; INTELLIGENT_LIGHT_ZONE_COUNT],
            position_space: [60.0, 37.0],
        }
    }

    fn test_node(id: &str, node_type: &str, params: HashMap<String, Value>) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params,
            inputs: vec![],
            outputs: vec![],
            input_bindings: vec![],
            wgsl_override: None,
        }
    }

    #[test]
    fn pixel_mapping_uses_render_bottom_origin() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(400.0, 200.0));
        let position_space = [400.0, 200.0];

        assert_eq!(
            point_to_screen([0.0, 0.0], rect, position_space),
            Pos2::new(10.0, 220.0)
        );
        assert_eq!(
            point_to_screen([400.0, 200.0], rect, position_space),
            Pos2::new(410.0, 20.0)
        );
        assert_eq!(
            screen_to_point(Pos2::new(10.0, 20.0), rect, position_space),
            [0.0, 200.0]
        );
        assert_eq!(
            screen_to_point(Pos2::new(410.0, 220.0), rect, position_space),
            [400.0, 0.0]
        );
    }

    #[test]
    fn intelligent_light_design_param_allowlist_rejects_unknown_keys() {
        assert!(is_intelligent_light_design_param("layoutMode"));
        assert!(is_intelligent_light_design_param("pos0"));
        assert!(is_intelligent_light_design_param("pos10"));
        assert!(is_intelligent_light_design_param("color0"));
        assert!(is_intelligent_light_design_param("color10"));
        assert!(!is_intelligent_light_design_param("pos11"));
        assert!(!is_intelligent_light_design_param("driver"));
    }

    #[test]
    fn cancel_color_edit_restores_original_hex() {
        let target = test_target();
        let mut state = IntelligentLightDesignState::default();
        state.color_popover_zone = Some(2);
        state.color_edit_original_hex = Some("#abcdef".to_string());
        state
            .optimistic_params
            .insert("color2".to_string(), Value::String("#112233".to_string()));

        let patch = cancel_color_edit(&target, "session", &mut state).expect("cancel patch");

        assert_eq!(patch.phase, "cancel");
        assert_eq!(
            patch.params.get("color2"),
            Some(&Value::String("#abcdef".to_string()))
        );
        assert_eq!(
            state.optimistic_params.get("color2"),
            Some(&Value::String("#abcdef".to_string()))
        );
        assert!(state.color_popover_zone.is_none());
    }

    #[test]
    fn opening_color_popover_emits_begin_and_tracks_original_hex() {
        let target = test_target();
        let mut state = IntelligentLightDesignState::default();
        let mut values = test_values(IntelligentLightLayoutMode::Manual);
        values.colors[3] = parse_hex_color("#abcdef").expect("hex color");
        let mut actions = Vec::new();

        open_color_popover(&target, "session", &mut state, &values, 3, &mut actions);

        assert_eq!(state.color_popover_zone, Some(3));
        assert_eq!(state.color_edit_original_hex.as_deref(), Some("#abcdef"));
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].phase, "begin");
        assert_eq!(
            actions[0].params.get("color3"),
            Some(&Value::String("#abcdef".to_string()))
        );
    }

    #[test]
    fn opening_new_color_popover_ends_previous_edit_before_beginning_next_one() {
        let target = test_target();
        let mut state = IntelligentLightDesignState::default();
        state.color_popover_zone = Some(1);
        state.color_edit_original_hex = Some("#112233".to_string());
        let mut values = test_values(IntelligentLightLayoutMode::Manual);
        values.colors[1] = parse_hex_color("#334455").expect("hex color");
        values.colors[2] = parse_hex_color("#abcdef").expect("hex color");
        let mut actions = Vec::new();

        open_color_popover(&target, "session", &mut state, &values, 2, &mut actions);

        assert_eq!(state.color_popover_zone, Some(2));
        assert_eq!(state.color_edit_original_hex.as_deref(), Some("#abcdef"));
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].phase, "end");
        assert_eq!(
            actions[0].params.get("color1"),
            Some(&Value::String("#334455".to_string()))
        );
        assert_eq!(actions[1].phase, "begin");
        assert_eq!(
            actions[1].params.get("color2"),
            Some(&Value::String("#abcdef".to_string()))
        );
    }

    #[test]
    fn procedural_begin_patch_freezes_all_zones_and_switches_to_manual() {
        let target = test_target();
        let mut state = IntelligentLightDesignState::default();
        let values = test_values(IntelligentLightLayoutMode::Procedural);
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(300.0, 200.0));
        let mut actions = Vec::new();

        emit_position_patch(
            "begin",
            &target,
            "session",
            &mut state,
            &values,
            0,
            Pos2::new(300.0, 0.0),
            rect,
            &mut actions,
        );

        let patch = actions.pop().expect("freeze patch");
        assert_eq!(patch.phase, "begin");
        assert_eq!(
            patch.params.get("layoutMode"),
            Some(&Value::String("manual".to_string()))
        );
        assert_eq!(patch.params.len(), 1 + INTELLIGENT_LIGHT_ZONE_COUNT);
        assert_eq!(patch.params.get("pos0"), Some(&json!([60.0, 37.0])));
        assert_eq!(
            patch.params.get("pos1"),
            Some(&json!(default_light_position(1, [60.0, 37.0])))
        );
    }

    #[test]
    fn position_and_color_locks_are_tracked_independently() {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: crate::dsl::Metadata {
                name: "ilight locks".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                test_node("vec", "Vector2Input", HashMap::new()),
                test_node("color", "ColorInput", HashMap::new()),
                test_node("ilight", "IntelligentLight", HashMap::new()),
            ],
            connections: vec![
                crate::dsl::Connection {
                    id: "pos-edge".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "vec".to_string(),
                        port_id: "vector".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "ilight".to_string(),
                        port_id: "pos0".to_string(),
                    },
                },
                crate::dsl::Connection {
                    id: "color-edge".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "color".to_string(),
                        port_id: "color".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "ilight".to_string(),
                        port_id: "color1".to_string(),
                    },
                },
            ],
            outputs: None,
            groups: vec![],
            assets: HashMap::new(),
            state_machine: None,
            debug_artifacts: None,
        };
        let target = test_target();
        let node = scene
            .nodes
            .iter()
            .find(|candidate| candidate.id == "ilight")
            .expect("ilight node");

        let values = read_intelligent_light_values(
            &scene,
            node,
            &target,
            &IntelligentLightDesignState::default(),
            false,
        );

        assert!(values.position_locks[0]);
        assert!(!values.color_locks[0]);
        assert!(!values.position_locks[1]);
        assert!(values.color_locks[1]);
    }

    #[test]
    fn manual_overlay_values_prefer_connected_position_and_color() {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: crate::dsl::Metadata {
                name: "ilight manual".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                test_node(
                    "vec",
                    "Vector2Input",
                    HashMap::from([
                        ("x".to_string(), json!(0.75)),
                        ("y".to_string(), json!(0.25)),
                    ]),
                ),
                test_node(
                    "color",
                    "ColorInput",
                    HashMap::from([("value".to_string(), json!("#abcdef"))]),
                ),
                test_node(
                    "ilight",
                    "IntelligentLight",
                    HashMap::from([
                        ("layoutMode".to_string(), json!("manual")),
                        ("pos0".to_string(), json!([0.1, 0.2])),
                        ("color0".to_string(), json!("#112233")),
                    ]),
                ),
            ],
            connections: vec![
                crate::dsl::Connection {
                    id: "pos-edge".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "vec".to_string(),
                        port_id: "vector".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "ilight".to_string(),
                        port_id: "pos0".to_string(),
                    },
                },
                crate::dsl::Connection {
                    id: "color-edge".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "color".to_string(),
                        port_id: "color".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "ilight".to_string(),
                        port_id: "color0".to_string(),
                    },
                },
            ],
            outputs: None,
            groups: vec![],
            assets: HashMap::new(),
            state_machine: None,
            debug_artifacts: None,
        };
        let target = test_target();
        let node = scene
            .nodes
            .iter()
            .find(|candidate| candidate.id == "ilight")
            .expect("ilight node");

        let values = read_intelligent_light_values(
            &scene,
            node,
            &target,
            &IntelligentLightDesignState::default(),
            false,
        );

        assert_eq!(values.layout_mode, IntelligentLightLayoutMode::Manual);
        assert_eq!(values.positions[0], [45.0, 27.75]);
        assert_eq!(
            values.colors[0],
            parse_hex_color("#abcdef").expect("hex color")
        );
    }

    #[test]
    fn legacy_procedural_overlay_values_use_manual_positions() {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: crate::dsl::Metadata {
                name: "ilight procedural".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![test_node(
                "ilight",
                "IntelligentLight",
                HashMap::from([
                    ("layoutMode".to_string(), json!("procedural")),
                    ("driver".to_string(), json!(0.0)),
                    ("pos0".to_string(), json!([0.0, 0.0])),
                ]),
            )],
            connections: vec![],
            outputs: None,
            groups: vec![],
            assets: HashMap::new(),
            state_machine: None,
            debug_artifacts: None,
        };
        let target = test_target();
        let node = scene
            .nodes
            .iter()
            .find(|candidate| candidate.id == "ilight")
            .expect("ilight node");

        let values = read_intelligent_light_values(
            &scene,
            node,
            &target,
            &IntelligentLightDesignState::default(),
            false,
        );

        assert_eq!(values.layout_mode, IntelligentLightLayoutMode::Manual);
        assert_eq!(values.positions[0], [0.0, 37.0]);
    }

    #[test]
    fn animation_playing_prefers_scene_params_over_optimistic_params() {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: crate::dsl::Metadata {
                name: "ilight animated".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![test_node(
                "ilight",
                "IntelligentLight",
                HashMap::from([("pos0".to_string(), json!([42.0, 24.0]))]),
            )],
            connections: vec![],
            outputs: None,
            groups: vec![],
            assets: HashMap::new(),
            state_machine: None,
            debug_artifacts: None,
        };
        let target = test_target();
        let node = scene
            .nodes
            .iter()
            .find(|candidate| candidate.id == "ilight")
            .expect("ilight node");
        let mut state = IntelligentLightDesignState::default();
        state
            .optimistic_params
            .insert("pos0".to_string(), json!([3.0, 4.0]));

        let values = read_intelligent_light_values(&scene, node, &target, &state, true);

        assert_eq!(values.positions[0], [42.0, 24.0]);
    }
}
