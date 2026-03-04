use std::collections::BTreeMap;

use rust_wgpu_fiber::eframe::egui::{self, Rect};

use crate::protocol::{
    InteractionEventData, InteractionEventPayload, InteractionKeyData, InteractionModifiers,
    InteractionPosition, InteractionTouchData, InteractionWheelData,
};

pub fn is_clean_rendering_state(has_preview_texture: bool, has_reference_compare: bool) -> bool {
    !has_preview_texture && !has_reference_compare
}

pub fn collect_interaction_payloads(
    events: &[egui::Event],
    canvas_rect: Rect,
    pointer_hover_pos: Option<egui::Pos2>,
    clean_state: bool,
    focus_latched: &mut bool,
) -> Vec<InteractionEventPayload> {
    let mut immediate = Vec::new();
    let mut latest_mousemove: Option<InteractionEventPayload> = None;
    let mut latest_wheel: Option<InteractionEventPayload> = None;
    let mut latest_touchmove: BTreeMap<u64, InteractionEventPayload> = BTreeMap::new();

    for event in events {
        update_focus_latch(event, canvas_rect, focus_latched);

        if !clean_state {
            continue;
        }

        match event {
            egui::Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                modifiers,
            } => {
                if !*focus_latched {
                    continue;
                }
                let data = InteractionEventData {
                    key: Some(InteractionKeyData {
                        key: key_to_w3c(*key),
                        physical_key: physical_key.map(key_to_w3c),
                        repeat: *repeat,
                    }),
                    modifiers: Some(modifiers_to_payload(*modifiers)),
                    ..InteractionEventData::default()
                };
                immediate.push(InteractionEventPayload {
                    event_type: if *pressed {
                        "keydown".to_string()
                    } else {
                        "keyup".to_string()
                    },
                    seq: 0,
                    data: Some(data),
                });
            }
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers,
            } => {
                if !canvas_rect.contains(*pos) {
                    continue;
                }
                let data = InteractionEventData {
                    position: Some(position_from_point(*pos, canvas_rect)),
                    button: Some(pointer_button_to_name(*button).to_string()),
                    modifiers: Some(modifiers_to_payload(*modifiers)),
                    ..InteractionEventData::default()
                };
                immediate.push(InteractionEventPayload {
                    event_type: if *pressed {
                        "mousedown".to_string()
                    } else {
                        "mouseup".to_string()
                    },
                    seq: 0,
                    data: Some(data),
                });
            }
            egui::Event::PointerMoved(pos) => {
                if !canvas_rect.contains(*pos) {
                    continue;
                }
                latest_mousemove = Some(InteractionEventPayload {
                    event_type: "mousemove".to_string(),
                    seq: 0,
                    data: Some(InteractionEventData {
                        position: Some(position_from_point(*pos, canvas_rect)),
                        ..InteractionEventData::default()
                    }),
                });
            }
            egui::Event::Touch {
                device_id,
                id,
                phase,
                pos,
                force,
            } => {
                if !canvas_rect.contains(*pos) {
                    continue;
                }
                let touch_payload = InteractionEventPayload {
                    event_type: touch_phase_to_w3c(*phase).to_string(),
                    seq: 0,
                    data: Some(InteractionEventData {
                        position: Some(position_from_point(*pos, canvas_rect)),
                        touch: Some(InteractionTouchData {
                            device_id: device_id.0,
                            touch_id: id.0,
                            force: *force,
                        }),
                        ..InteractionEventData::default()
                    }),
                };

                if matches!(phase, egui::TouchPhase::Move) {
                    latest_touchmove.insert(id.0, touch_payload);
                } else {
                    immediate.push(touch_payload);
                }
            }
            egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
            } => {
                let on_canvas = pointer_hover_pos.is_some_and(|pos| canvas_rect.contains(pos));
                if !on_canvas {
                    continue;
                }
                latest_wheel = Some(InteractionEventPayload {
                    event_type: "wheel".to_string(),
                    seq: 0,
                    data: Some(InteractionEventData {
                        modifiers: Some(modifiers_to_payload(*modifiers)),
                        wheel: Some(InteractionWheelData {
                            delta_x: delta.x,
                            delta_y: delta.y,
                            delta_mode: mouse_wheel_delta_mode(*unit),
                        }),
                        ..InteractionEventData::default()
                    }),
                });
            }
            _ => {}
        }
    }

    if let Some(payload) = latest_mousemove {
        immediate.push(payload);
    }
    immediate.extend(latest_touchmove.into_values());
    if let Some(payload) = latest_wheel {
        immediate.push(payload);
    }

    immediate
}

fn update_focus_latch(event: &egui::Event, canvas_rect: Rect, focus_latched: &mut bool) {
    match event {
        egui::Event::PointerButton { pos, pressed, .. } if *pressed => {
            *focus_latched = canvas_rect.contains(*pos);
        }
        egui::Event::WindowFocused(false) => {
            *focus_latched = false;
        }
        _ => {}
    }
}

fn modifiers_to_payload(modifiers: egui::Modifiers) -> InteractionModifiers {
    InteractionModifiers {
        alt: modifiers.alt,
        ctrl: modifiers.ctrl,
        shift: modifiers.shift,
        meta: modifiers.command || modifiers.mac_cmd,
    }
}

fn position_from_point(point: egui::Pos2, canvas_rect: Rect) -> InteractionPosition {
    InteractionPosition {
        client_x: point.x,
        client_y: point.y,
        canvas_x: point.x - canvas_rect.min.x,
        canvas_y: point.y - canvas_rect.min.y,
    }
}

fn pointer_button_to_name(button: egui::PointerButton) -> &'static str {
    match button {
        egui::PointerButton::Primary => "left",
        egui::PointerButton::Secondary => "right",
        egui::PointerButton::Middle => "middle",
        egui::PointerButton::Extra1 => "back",
        egui::PointerButton::Extra2 => "forward",
    }
}

fn mouse_wheel_delta_mode(unit: egui::MouseWheelUnit) -> u8 {
    match unit {
        egui::MouseWheelUnit::Point => 0,
        egui::MouseWheelUnit::Line => 1,
        egui::MouseWheelUnit::Page => 2,
    }
}

fn touch_phase_to_w3c(phase: egui::TouchPhase) -> &'static str {
    match phase {
        egui::TouchPhase::Start => "touchstart",
        egui::TouchPhase::Move => "touchmove",
        egui::TouchPhase::End => "touchend",
        egui::TouchPhase::Cancel => "touchcancel",
    }
}

fn key_to_w3c(key: egui::Key) -> String {
    match key {
        egui::Key::ArrowDown => "ArrowDown".to_string(),
        egui::Key::ArrowLeft => "ArrowLeft".to_string(),
        egui::Key::ArrowRight => "ArrowRight".to_string(),
        egui::Key::ArrowUp => "ArrowUp".to_string(),
        egui::Key::Escape => "Escape".to_string(),
        egui::Key::Tab => "Tab".to_string(),
        egui::Key::Backspace => "Backspace".to_string(),
        egui::Key::Enter => "Enter".to_string(),
        egui::Key::Space => " ".to_string(),
        egui::Key::Insert => "Insert".to_string(),
        egui::Key::Delete => "Delete".to_string(),
        egui::Key::Home => "Home".to_string(),
        egui::Key::End => "End".to_string(),
        egui::Key::PageUp => "PageUp".to_string(),
        egui::Key::PageDown => "PageDown".to_string(),
        egui::Key::Num0 => "0".to_string(),
        egui::Key::Num1 => "1".to_string(),
        egui::Key::Num2 => "2".to_string(),
        egui::Key::Num3 => "3".to_string(),
        egui::Key::Num4 => "4".to_string(),
        egui::Key::Num5 => "5".to_string(),
        egui::Key::Num6 => "6".to_string(),
        egui::Key::Num7 => "7".to_string(),
        egui::Key::Num8 => "8".to_string(),
        egui::Key::Num9 => "9".to_string(),
        egui::Key::A => "a".to_string(),
        egui::Key::B => "b".to_string(),
        egui::Key::C => "c".to_string(),
        egui::Key::D => "d".to_string(),
        egui::Key::E => "e".to_string(),
        egui::Key::F => "f".to_string(),
        egui::Key::G => "g".to_string(),
        egui::Key::H => "h".to_string(),
        egui::Key::I => "i".to_string(),
        egui::Key::J => "j".to_string(),
        egui::Key::K => "k".to_string(),
        egui::Key::L => "l".to_string(),
        egui::Key::M => "m".to_string(),
        egui::Key::N => "n".to_string(),
        egui::Key::O => "o".to_string(),
        egui::Key::P => "p".to_string(),
        egui::Key::Q => "q".to_string(),
        egui::Key::R => "r".to_string(),
        egui::Key::S => "s".to_string(),
        egui::Key::T => "t".to_string(),
        egui::Key::U => "u".to_string(),
        egui::Key::V => "v".to_string(),
        egui::Key::W => "w".to_string(),
        egui::Key::X => "x".to_string(),
        egui::Key::Y => "y".to_string(),
        egui::Key::Z => "z".to_string(),
        _ => format!("{key:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{collect_interaction_payloads, is_clean_rendering_state};
    use rust_wgpu_fiber::eframe::egui::{self, Pos2, Rect, vec2};

    fn canvas_rect() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 100.0), vec2(200.0, 100.0))
    }

    fn event_types(events: &[crate::protocol::InteractionEventPayload]) -> Vec<String> {
        events.iter().map(|e| e.event_type.clone()).collect()
    }

    #[test]
    fn clean_state_requires_no_preview_and_no_reference() {
        assert!(is_clean_rendering_state(false, false));
        assert!(!is_clean_rendering_state(true, false));
        assert!(!is_clean_rendering_state(false, true));
    }

    #[test]
    fn key_events_require_focus_latch() {
        let mut focus = false;
        let events = vec![egui::Event::Key {
            key: egui::Key::A,
            physical_key: Some(egui::Key::A),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];

        let output = collect_interaction_payloads(&events, canvas_rect(), None, true, &mut focus);
        assert!(output.is_empty());

        let focus_events = vec![
            egui::Event::PointerButton {
                pos: Pos2::new(120.0, 120.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let output =
            collect_interaction_payloads(&focus_events, canvas_rect(), None, true, &mut focus);
        assert_eq!(event_types(&output), vec!["mousedown", "keydown"]);
        assert!(focus);
    }

    #[test]
    fn focus_is_cleared_by_outside_click_and_window_blur() {
        let mut focus = true;

        let outside_click = vec![
            egui::Event::PointerButton {
                pos: Pos2::new(10.0, 10.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key: egui::Key::B,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let output =
            collect_interaction_payloads(&outside_click, canvas_rect(), None, true, &mut focus);
        assert!(output.is_empty());
        assert!(!focus);

        focus = true;
        let blur_events = vec![
            egui::Event::WindowFocused(false),
            egui::Event::Key {
                key: egui::Key::C,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let output =
            collect_interaction_payloads(&blur_events, canvas_rect(), None, true, &mut focus);
        assert!(output.is_empty());
        assert!(!focus);
    }

    #[test]
    fn no_output_when_not_clean_state() {
        let mut focus = false;
        let events = vec![egui::Event::PointerButton {
            pos: Pos2::new(120.0, 120.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }];
        let output = collect_interaction_payloads(&events, canvas_rect(), None, false, &mut focus);
        assert!(output.is_empty());
        assert!(focus);
    }

    #[test]
    fn coalesces_mousemove_touchmove_and_wheel() {
        let mut focus = true;
        let events = vec![
            egui::Event::PointerMoved(Pos2::new(110.0, 110.0)),
            egui::Event::PointerMoved(Pos2::new(150.0, 130.0)),
            egui::Event::Touch {
                device_id: egui::TouchDeviceId(1),
                id: egui::TouchId(7),
                phase: egui::TouchPhase::Move,
                pos: Pos2::new(130.0, 120.0),
                force: None,
            },
            egui::Event::Touch {
                device_id: egui::TouchDeviceId(1),
                id: egui::TouchId(7),
                phase: egui::TouchPhase::Move,
                pos: Pos2::new(160.0, 120.0),
                force: None,
            },
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, 3.0),
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(0.0, 1.0),
                modifiers: egui::Modifiers::NONE,
            },
        ];

        let output = collect_interaction_payloads(
            &events,
            canvas_rect(),
            Some(Pos2::new(160.0, 120.0)),
            true,
            &mut focus,
        );
        assert_eq!(
            event_types(&output),
            vec!["mousemove", "touchmove", "wheel"]
        );
        let mouse = output[0]
            .data
            .as_ref()
            .and_then(|d| d.position.as_ref())
            .expect("mousemove position");
        assert_eq!(mouse.canvas_x, 50.0);
        assert_eq!(mouse.canvas_y, 30.0);

        let wheel = output[2]
            .data
            .as_ref()
            .and_then(|d| d.wheel.as_ref())
            .expect("wheel payload");
        assert_eq!(wheel.delta_mode, 1);
        assert_eq!(wheel.delta_y, 1.0);
    }

    #[test]
    fn maps_touch_phases_to_w3c_event_names() {
        let mut focus = false;
        let events = vec![
            egui::Event::Touch {
                device_id: egui::TouchDeviceId(1),
                id: egui::TouchId(1),
                phase: egui::TouchPhase::Start,
                pos: Pos2::new(120.0, 120.0),
                force: Some(0.5),
            },
            egui::Event::Touch {
                device_id: egui::TouchDeviceId(1),
                id: egui::TouchId(1),
                phase: egui::TouchPhase::End,
                pos: Pos2::new(120.0, 120.0),
                force: None,
            },
            egui::Event::Touch {
                device_id: egui::TouchDeviceId(1),
                id: egui::TouchId(1),
                phase: egui::TouchPhase::Cancel,
                pos: Pos2::new(120.0, 120.0),
                force: None,
            },
        ];
        let output = collect_interaction_payloads(&events, canvas_rect(), None, true, &mut focus);
        assert_eq!(
            event_types(&output),
            vec!["touchstart", "touchend", "touchcancel"]
        );
    }

    #[test]
    fn maps_key_up_and_key_down() {
        let mut focus = true;
        let events = vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let output = collect_interaction_payloads(&events, canvas_rect(), None, true, &mut focus);
        assert_eq!(event_types(&output), vec!["keydown", "keyup"]);

        let key = output[0]
            .data
            .as_ref()
            .and_then(|d| d.key.as_ref())
            .expect("key payload");
        assert_eq!(key.key, "a");
        assert_eq!(key.physical_key.as_deref(), Some("a"));
    }

    #[test]
    fn maps_mouse_down_and_up() {
        let mut focus = false;
        let events = vec![
            egui::Event::PointerButton {
                pos: Pos2::new(120.0, 120.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: Pos2::new(120.0, 120.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let output = collect_interaction_payloads(&events, canvas_rect(), None, true, &mut focus);
        assert_eq!(event_types(&output), vec!["mousedown", "mouseup"]);
    }
}
