use rust_wgpu_fiber::eframe::egui;

use crate::{
    animation::AnimationStep,
    app::{interaction_report, types::App},
    protocol, ui,
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
    );
    let canvas_rect = app
        .canvas
        .interactions
        .last_canvas_rect
        .unwrap_or_else(|| ctx.available_rect());
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

    if let Some(session) = app.runtime.animation_session.as_mut() {
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

pub fn state_machine_snapshot(app: &App) -> Option<ui::state_machine_panel::StateMachineSnapshot> {
    app.runtime.animation_session.as_ref().map(|session| {
        let mut snapshot = ui::state_machine_panel::snapshot_from_session(session);
        snapshot.state_local_times = app.interaction_bridge.cached_state_local_times.clone();
        snapshot.transition_blend = app.interaction_bridge.cached_transition_blend;
        snapshot.override_values = app.interaction_bridge.cached_override_values.clone();
        snapshot
    })
}

#[cfg(test)]
mod tests {
    use super::{assign_sequence_numbers, interaction_message_text, state_transition_payloads};
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
}
