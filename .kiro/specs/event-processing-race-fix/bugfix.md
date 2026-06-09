# Bugfix Requirements Document

## Introduction

There is a race condition in the `App::update()` flow between the animation session's `step()` call and the canvas controller's interaction event collection (`fire_event`). Because `session.step()` runs early in the frame (draining `pending_events`) while `fire_event()` only queues events much later inside `show_canvas_panel()`, interaction events from the current frame's egui input are never consumed in the same frame they arrive. This causes event-triggered state machine transitions (e.g. `mousedown`) to be delayed by one or more frames, with the delay being unbounded when no continuous repaint is active — the queued event sits idle until the OS or another user action forces the next frame.

## Bug Analysis

### Current Behavior (Defect)

1.1 WHEN the animation session is active and a user interaction event (e.g. `mousedown`) occurs during a frame, THEN the system calls `session.step()` before the event has been queued via `fire_event()`, so `pending_events` is empty and the event is not consumed in that frame.

1.2 WHEN `session.step()` returns `needs_redraw: false` (because no events were pending) and no other continuous-repaint trigger is active (no time-driven scene, no sidebar animation, no pan-zoom animation, no capture session), THEN the system does not call `ctx.request_repaint()`, so the next frame is deferred until the OS or another external trigger causes a repaint.

1.3 WHEN the queued event from a previous frame finally gets consumed on a subsequent frame (triggered by a second user click or an OS repaint), THEN the system processes the stale event with a potentially incorrect `effective_dt`, causing the state machine transition to fire at the wrong scene time.

### Expected Behavior (Correct)

2.1 WHEN the animation session is active and a user interaction event occurs during a frame, THEN the system SHALL ensure the event is queued into `pending_events` before `session.step()` runs, so the event is consumed in the same frame it arrives.

2.2 WHEN interaction events are queued during a frame and `session.step()` consumes them, THEN the system SHALL reflect the event-triggered state machine transition (and any resulting override changes) in that same frame's render output, with no extra-frame latency.

2.3 WHEN interaction events are collected and fed to the session before `step()`, THEN the system SHALL still correctly broadcast those events over WebSocket and call `sync_animation_state_interaction_events` with the up-to-date state, preserving the existing event reporting behavior.

### Unchanged Behavior (Regression Prevention)

3.1 WHEN no interaction events occur during a frame, THEN the system SHALL CONTINUE TO run `session.step()` with an empty `pending_events` and produce the same animation output as before.

3.2 WHEN the canvas controller's `show_canvas_panel()` runs, THEN the system SHALL CONTINUE TO correctly perform all layout-dependent operations (pan/zoom, pixel overlay, reference image compositing, viewport indicators, context menu) that rely on the egui `CentralPanel` rect being established.

3.3 WHEN `show_canvas_panel()` collects interaction payloads, THEN the system SHALL CONTINUE TO broadcast them over WebSocket with correct sequence numbers and payload data.

3.4 WHEN `should_request_immediate_repaint()` evaluates its conditions (time-driven scene, sidebar animation, pan-zoom animation, operation indicator, capture session), THEN the system SHALL CONTINUE TO request or skip immediate repaint using the same logic as before.

3.5 WHEN the animation session is not active or `animation_playing` is false, THEN the system SHALL CONTINUE TO advance `time_value_secs` with wall-clock delta and skip the session step, with no change in behavior.
