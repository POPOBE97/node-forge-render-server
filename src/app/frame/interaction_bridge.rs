use rust_wgpu_fiber::eframe::egui;

use crate::{
    animation::AnimationStep,
    app::{interaction_report, types::App},
    protocol,
    state_machine::MousePosition,
    ui,
};

fn next_sequence(seq: &mut u64) -> u64 {
    *seq = seq.saturating_add(1);
    *seq
}

fn assign_sequence_numbers(payloads: &mut [protocol::InteractionEventPayload], seq: &mut u64) {
    for payload in payloads {
        payload.seq = next_sequence(seq);
    }
}

fn state_event_payload(
    event_type: &str,
    state_id: &str,
    transition_id: Option<&str>,
    seq: &mut u64,
) -> protocol::InteractionEventPayload {
    protocol::InteractionEventPayload {
        event_type: event_type.to_string(),
        seq: next_sequence(seq),
        data: Some(protocol::InteractionEventData {
            state: Some(protocol::InteractionStateData {
                state_id: state_id.to_string(),
                transition_id: transition_id.map(str::to_string),
            }),
            ..protocol::InteractionEventData::default()
        }),
    }
}

fn interaction_message_text(payload: &protocol::InteractionEventPayload) -> Option<String> {
    serde_json::to_string(&protocol::WSMessage {
        msg_type: "interaction_event".to_string(),
        timestamp: protocol::now_millis(),
        request_id: None,
        payload: Some(payload.clone()),
    })
    .ok()
}

fn frag_pixel_position_from_screen_pos(
    point: egui::Pos2,
    image_rect: egui::Rect,
    display_resolution: [u32; 2],
) -> Option<MousePosition> {
    if image_rect.width() <= 0.0
        || image_rect.height() <= 0.0
        || display_resolution[0] == 0
        || display_resolution[1] == 0
    {
        return None;
    }

    let x = ((point.x - image_rect.left()) / image_rect.width()).clamp(0.0, 1.0)
        * display_resolution[0] as f32;
    let y = ((image_rect.bottom() - point.y) / image_rect.height()).clamp(0.0, 1.0)
        * display_resolution[1] as f32;

    Some(MousePosition {
        x: x as f64,
        y: y as f64,
    })
}

fn annotate_frag_pixel_positions(
    payloads: &mut [protocol::InteractionEventPayload],
    canvas_rect: egui::Rect,
    image_rect: Option<egui::Rect>,
    display_resolution: Option<[u32; 2]>,
) -> Option<MousePosition> {
    let image_rect = image_rect?;
    let display_resolution = display_resolution?;
    let mut latest = None;

    for payload in payloads {
        let Some(position) = payload
            .data
            .as_mut()
            .and_then(|data| data.position.as_mut())
        else {
            continue;
        };
        let point = egui::pos2(
            canvas_rect.min.x + position.canvas_x,
            canvas_rect.min.y + position.canvas_y,
        );
        let Some(mouse_position) =
            frag_pixel_position_from_screen_pos(point, image_rect, display_resolution)
        else {
            continue;
        };
        position.frag_pixel_x = Some(mouse_position.x as f32);
        position.frag_pixel_y = Some(mouse_position.y as f32);
        latest = Some(mouse_position);
    }

    latest
}

fn state_transition_payloads(
    previous_state_id: Option<&str>,
    current_state_id: Option<&str>,
    transition_id: Option<&str>,
    seq: &mut u64,
) -> Vec<protocol::InteractionEventPayload> {
    if previous_state_id == current_state_id {
        return Vec::new();
    }

    let mut payloads = Vec::new();
    if let Some(prev) = previous_state_id {
        payloads.push(state_event_payload("stateleave", prev, transition_id, seq));
    }
    if let Some(curr) = current_state_id {
        payloads.push(state_event_payload("stateenter", curr, transition_id, seq));
    }
    payloads
}

pub fn collect_early_canvas_interactions(
    app: &mut App,
    ctx: &egui::Context,
) -> Vec<protocol::InteractionEventPayload> {
    let frame_events = ctx.input(|i| i.events.clone());
    let interaction_clean_state = interaction_report::is_clean_rendering_state(
        app.canvas.display.preview_texture_name.is_some(),
        app.canvas.reference.ref_image.is_some(),
        app.canvas.design.active.is_some(),
    );
    let canvas_rect = app
        .canvas
        .interactions
        .last_canvas_rect
        .unwrap_or_else(|| ctx.content_rect());
    let pointer_hover_pos = ctx.input(|i| i.pointer.hover_pos());

    let mut payloads = interaction_report::collect_interaction_payloads(
        frame_events.as_slice(),
        canvas_rect,
        pointer_hover_pos,
        interaction_clean_state,
        &mut app.canvas.interactions.canvas_event_focus_latched,
    );

    assign_sequence_numbers(
        &mut payloads,
        &mut app.interaction_bridge.interaction_event_seq,
    );

    let latest_mouse_position = annotate_frag_pixel_positions(
        &mut payloads,
        canvas_rect,
        app.canvas.interactions.last_image_rect,
        app.canvas.interactions.last_display_resolution,
    );

    if app.runtime.animation_playing
        && let Some(session) = app.runtime.animation_session.as_mut()
    {
        if let Some(mouse_position) = latest_mouse_position {
            session.update_mouse_position(mouse_position);
        }
        for payload in &payloads {
            session.fire_event(&payload.event_type);
        }
    }

    payloads
}

pub fn broadcast_payloads(app: &App, payloads: &[protocol::InteractionEventPayload]) {
    for payload in payloads {
        if let Some(text) = interaction_message_text(payload) {
            app.core.ws_hub.broadcast(text);
        }
    }
}

pub fn sync_animation_state(
    app: &mut App,
    current_state_id: Option<&str>,
    transition_id: Option<&str>,
) {
    let previous_state_id = app
        .interaction_bridge
        .last_synced_animation_state_id
        .as_deref();
    let payloads = state_transition_payloads(
        previous_state_id,
        current_state_id,
        transition_id,
        &mut app.interaction_bridge.interaction_event_seq,
    );
    broadcast_payloads(app, &payloads);
    app.interaction_bridge.last_synced_animation_state_id = current_state_id.map(str::to_string);
}

pub fn update_debug_state(app: &mut App, step: &AnimationStep) {
    app.interaction_bridge.cached_state_local_times =
        step.state_local_times.clone().into_iter().collect();
    app.interaction_bridge.cached_transition_blend = step.transition_blend;
    app.interaction_bridge.cached_override_values = step
        .active_overrides
        .iter()
        .map(|(key, value)| {
            (
                format!("{}:{}", key.node_id, key.param_name),
                ui::state_machine_panel::format_json_value_2dp(value),
            )
        })
        .collect();
    app.interaction_bridge
        .cached_override_values
        .sort_by(|a, b| a.0.cmp(&b.0));
}

#[cfg(test)]
mod tests {
    use rust_wgpu_fiber::eframe::egui;

    use super::{
        assign_sequence_numbers, frag_pixel_position_from_screen_pos, interaction_message_text,
        state_transition_payloads,
    };
    use crate::protocol::InteractionEventPayload;

    #[test]
    fn assign_sequence_numbers_increments_monotonically() {
        let mut seq = 10;
        let mut payloads = vec![
            InteractionEventPayload {
                event_type: "mousedown".to_string(),
                seq: 0,
                data: None,
            },
            InteractionEventPayload {
                event_type: "mouseup".to_string(),
                seq: 0,
                data: None,
            },
        ];
        assign_sequence_numbers(&mut payloads, &mut seq);
        assert_eq!(payloads[0].seq, 11);
        assert_eq!(payloads[1].seq, 12);
        assert_eq!(seq, 12);
    }

    #[test]
    fn state_transition_sync_emits_leave_then_enter() {
        let mut seq = 0;
        let payloads = state_transition_payloads(Some("idle"), Some("hover"), Some("tr"), &mut seq);
        assert_eq!(payloads.len(), 2);
        assert_eq!(payloads[0].event_type, "stateleave");
        assert_eq!(payloads[0].seq, 1);
        assert_eq!(payloads[1].event_type, "stateenter");
        assert_eq!(payloads[1].seq, 2);
    }

    #[test]
    fn interaction_payloads_are_sequenced_before_state_sync_payloads() {
        let mut seq = 0;
        let mut interaction_payloads = vec![InteractionEventPayload {
            event_type: "mousedown".to_string(),
            seq: 0,
            data: None,
        }];
        assign_sequence_numbers(&mut interaction_payloads, &mut seq);

        let state_payloads =
            state_transition_payloads(Some("entry"), Some("mutation"), Some("tr"), &mut seq);

        assert_eq!(interaction_payloads[0].event_type, "mousedown");
        assert_eq!(interaction_payloads[0].seq, 1);
        assert_eq!(state_payloads[0].event_type, "stateleave");
        assert_eq!(state_payloads[0].seq, 2);
        assert_eq!(state_payloads[1].event_type, "stateenter");
        assert_eq!(state_payloads[1].seq, 3);
    }

    #[test]
    fn interaction_payload_serializes_without_command_dispatch() {
        let payload = InteractionEventPayload {
            event_type: "mousemove".to_string(),
            seq: 7,
            data: None,
        };
        let text = interaction_message_text(&payload).expect("payload should serialize");
        let value: serde_json::Value = serde_json::from_str(&text).expect("message should parse");
        assert_eq!(value["type"], "interaction_event");
        assert_eq!(value["payload"]["eventType"], "mousemove");
    }

    #[test]
    fn screen_position_maps_to_frag_pixel_bottom_left_origin() {
        let image_rect =
            egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 200.0));
        let resolution = [800, 600];

        let top_left =
            frag_pixel_position_from_screen_pos(egui::pos2(10.0, 20.0), image_rect, resolution)
                .expect("top-left should map");
        assert!((top_left.x - 0.0).abs() < f64::EPSILON);
        assert!((top_left.y - 600.0).abs() < f64::EPSILON);

        let bottom_left =
            frag_pixel_position_from_screen_pos(egui::pos2(10.0, 220.0), image_rect, resolution)
                .expect("bottom-left should map");
        assert!((bottom_left.x - 0.0).abs() < f64::EPSILON);
        assert!((bottom_left.y - 0.0).abs() < f64::EPSILON);

        let center =
            frag_pixel_position_from_screen_pos(egui::pos2(210.0, 120.0), image_rect, resolution)
                .expect("center should map");
        assert!((center.x - 400.0).abs() < f64::EPSILON);
        assert!((center.y - 300.0).abs() < f64::EPSILON);
    }
}
