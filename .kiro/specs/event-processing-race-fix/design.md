# Event Processing Race Fix — Bugfix Design

## Overview

The `App::update()` method calls `session.step()` (which drains `pending_events`) before `show_canvas_panel()` has a chance to call `session.fire_event()` with the current frame's interaction events. This means interaction-triggered state machine transitions (e.g. `mousedown`) are always delayed by at least one frame, and potentially indefinitely when no continuous repaint is active.

The fix extracts the interaction event collection logic from `show_canvas_panel()` into a separate early phase that runs before `session.step()`, while leaving all layout-dependent canvas work (pan/zoom, pixel overlay, reference compositing, viewport indicators, context menu) inside `show_canvas_panel()` where it depends on the `CentralPanel` rect.

## Glossary

- **Bug_Condition (C)**: An animation session is active, a user interaction event (e.g. `mousedown`, `keydown`) arrives via egui input during a frame, and `session.step()` runs before `fire_event()` queues the event — so `pending_events` is empty when `step()` drains it.
- **Property (P)**: Interaction events collected from egui input are queued into `pending_events` before `session.step()` runs, so the state machine processes them in the same frame they arrive.
- **Preservation**: All layout-dependent canvas operations, WebSocket broadcasting, repaint scheduling, and non-interaction animation behavior must remain unchanged.
- **`session.step(real_dt)`**: Method on `AnimationSession` in `src/animation/session.rs` that advances the fixed-step clock, drains `pending_events`, and ticks the state machine runloop.
- **`session.fire_event(name)`**: Method on `AnimationSession` that pushes an event name into `pending_events` for consumption by the next `step()` call.
- **`collect_interaction_payloads()`**: Function in `src/app/interaction_report.rs` that reads egui `frame_events` and produces `InteractionEventPayload` structs for each canvas interaction.
- **`show_canvas_panel()`**: Function in `src/app/canvas_controller.rs` that renders the canvas UI, handles pan/zoom, pixel overlay, reference images, and currently also collects interaction events and calls `fire_event()`.

## Bug Details

### Fault Condition

The bug manifests when a user interaction event occurs during a frame while an animation session is active. The `App::update()` method calls `session.step()` at approximately line 260, which calls `std::mem::take(&mut self.pending_events)` to drain events. However, `fire_event()` is only called much later inside `show_canvas_panel()` (called at approximately line 700 inside the `CentralPanel` closure), so the events from the current frame are never available to `step()` in the same frame.

**Formal Specification:**
```
FUNCTION isBugCondition(input)
  INPUT: input of type (FrameState, EguiEvents)
  OUTPUT: boolean

  RETURN input.animation_session IS Some
         AND input.animation_playing == true
         AND input.egui_events CONTAINS at least one canvas interaction event
              (mousedown, mouseup, mousemove, keydown, keyup, wheel, touch*)
         AND session.step() is called BEFORE fire_event() queues those events
END FUNCTION
```

### Examples

- User clicks on canvas (`mousedown`) while animation is playing → `step()` sees empty `pending_events` → state machine transition that listens for `mousedown` does not fire this frame → transition fires next frame (if repaint happens) with stale timing.
- User clicks on canvas with no time-driven scene and no other continuous repaint trigger → `step()` returns `needs_redraw: false` → no `ctx.request_repaint()` → event sits idle until OS or another user action forces a repaint (unbounded delay).
- User presses a key (`keydown`) during animation → same race: `step()` drains empty `pending_events` before `show_canvas_panel()` collects the key event and calls `fire_event()`.
- No interaction events during a frame → `step()` correctly sees empty `pending_events` → no bug (this is the non-buggy path).

## Expected Behavior

### Preservation Requirements

**Unchanged Behaviors:**
- All layout-dependent operations in `show_canvas_panel()` (pan/zoom, pixel overlay, reference image compositing, viewport indicators, context menu) must continue to work correctly with the `CentralPanel` rect established.
- WebSocket broadcasting of interaction events with correct sequence numbers and payload data must continue unchanged.
- `sync_animation_state_interaction_events()` must continue to receive up-to-date state IDs after `step()` runs.
- `should_request_immediate_repaint()` must continue to evaluate its conditions (time-driven scene, sidebar animation, pan-zoom animation, operation indicator, capture session) with the same logic.
- When no animation session is active or `animation_playing` is false, `time_value_secs` must continue to advance with wall-clock delta and session step must be skipped.
- When no interaction events occur during a frame, `session.step()` must run with empty `pending_events` and produce the same animation output as before.

**Scope:**
All inputs that do NOT involve canvas interaction events arriving while an animation session is active should be completely unaffected by this fix. This includes:
- Frames with no user interaction
- Frames where animation is not playing
- Sidebar UI interactions (handled separately from canvas)
- Scene updates arriving via WebSocket

## Hypothesized Root Cause

Based on the bug description and code analysis, the root cause is a ordering problem in `App::update()`:

1. **Execution Order Mismatch**: `session.step()` runs at ~line 260 of `update()`, which calls `std::mem::take(&mut self.pending_events)` to drain all pending events. But `fire_event()` is only called inside `show_canvas_panel()` at ~line 700, deep inside the `CentralPanel` closure. By the time `fire_event()` pushes events into `pending_events`, `step()` has already consumed the (empty) queue for this frame.

2. **Tight Coupling of Event Collection and Canvas Layout**: The interaction event collection (`collect_interaction_payloads()` + `fire_event()`) is currently embedded at the end of `show_canvas_panel()`, interleaved with layout-dependent work (pan/zoom, pixel overlay, reference compositing). This makes it impossible to run event collection before `step()` without also running all the layout code early — which would break because `CentralPanel` hasn't been set up yet.

3. **Missing Repaint Request for Queued Events**: When `step()` sees no events and returns `needs_redraw: false`, and no other continuous-repaint trigger is active, the system does not call `ctx.request_repaint()`. The event queued by `fire_event()` after `step()` then sits idle until an external trigger forces the next frame — causing unbounded delay.

4. **`effective_dt` Staleness**: When the queued event is finally consumed on a later frame, the `real_dt` passed to `step()` reflects the wall-clock time since the last frame, not the time since the event occurred. This means the state machine transition fires with an incorrect scene time offset.

## Correctness Properties

Property 1: Fault Condition - Interaction Events Consumed Same Frame

_For any_ frame where an animation session is active (`animation_playing == true`), and at least one canvas interaction event is present in egui's input events, the fixed `update()` flow SHALL ensure those events are queued into `pending_events` before `session.step()` runs, so that `step()` drains and processes them in the same frame they arrive.

**Validates: Requirements 2.1, 2.2**

Property 2: Preservation - Non-Interaction Frame Behavior

_For any_ frame where no canvas interaction events are present in egui's input events, the fixed `update()` flow SHALL produce exactly the same `AnimationStep` result (same `active_overrides`, `needs_redraw`, `scene_time_secs`, `current_state_id`) as the original code, preserving all existing animation behavior for non-interaction frames.

**Validates: Requirements 3.1, 3.4, 3.5**

Property 3: Preservation - WebSocket Broadcasting

_For any_ frame where canvas interaction events are collected, the fixed code SHALL continue to broadcast those events over WebSocket with correct sequence numbers and payload data, and SHALL continue to call `sync_animation_state_interaction_events` with the post-step state IDs, preserving the existing event reporting behavior.

**Validates: Requirements 2.3, 3.3**

Property 4: Preservation - Layout-Dependent Canvas Operations

_For any_ frame, the fixed code SHALL continue to perform all layout-dependent operations in `show_canvas_panel()` (pan/zoom, pixel overlay, reference image compositing, viewport indicators, context menu) only after the `CentralPanel` rect has been established by egui, preserving correct layout behavior.

**Validates: Requirements 3.2**

## Fix Implementation

### Changes Required

Assuming our root cause analysis is correct:

**File**: `src/app/canvas_controller.rs`

**Function**: `show_canvas_panel()`

**Specific Changes**:
1. **Extract Early Event Collection**: Create a new public function `collect_canvas_interaction_events(app, ctx)` that:
   - Reads `ctx.input(|i| i.events.clone())` to get frame events
   - Computes `interaction_clean_state` from current app state
   - Calls `collect_interaction_payloads()` with the frame events and a cached or default canvas rect (from the previous frame's `canvas_center_prev` and known layout, or using `ctx.available_rect()` as a reasonable approximation before `CentralPanel` is laid out)
   - Calls `session.fire_event()` for each collected interaction payload
   - Returns the collected payloads for later WebSocket broadcasting

2. **Use Previous-Frame Canvas Rect**: Since `CentralPanel` hasn't been laid out yet when we collect events early, use the previous frame's `animated_canvas_rect` (stored as a new field on `App`, e.g. `app.last_canvas_rect`). On the first frame, fall back to `ctx.available_rect()`. The canvas rect is stable frame-to-frame in practice (it only changes during sidebar open/close transitions), so hit-testing against the previous rect is accurate enough for interaction classification.

3. **Store Canvas Rect Each Frame**: At the end of `show_canvas_panel()`, store `animated_canvas_rect` into `app.last_canvas_rect` so it's available for the next frame's early event collection.

4. **Remove Duplicate Event Collection from show_canvas_panel**: Remove the `collect_interaction_payloads()` + `fire_event()` block from the end of `show_canvas_panel()`. Instead, have `show_canvas_panel()` receive the pre-collected payloads and only perform the WebSocket broadcasting part.

**File**: `src/app/mod.rs`

**Function**: `App::update()`

**Specific Changes**:
5. **Call Early Event Collection Before step()**: Insert a call to `canvas_controller::collect_canvas_interaction_events(self, ctx)` before the animation session tick block (~line 255). Store the returned payloads.

6. **Broadcast Events After step()**: After `session.step()` and `sync_animation_state_interaction_events()`, broadcast the pre-collected payloads over WebSocket. This preserves the existing broadcast behavior while ensuring events are queued before `step()`.

7. **Add `last_canvas_rect` Field to App**: Add an `Option<egui::Rect>` field to the `App` struct to cache the previous frame's canvas rect.

## Testing Strategy

### Validation Approach

The testing strategy follows a two-phase approach: first, surface counterexamples that demonstrate the bug on unfixed code, then verify the fix works correctly and preserves existing behavior.

### Exploratory Fault Condition Checking

**Goal**: Surface counterexamples that demonstrate the bug BEFORE implementing the fix. Confirm or refute the root cause analysis. If we refute, we will need to re-hypothesize.

**Test Plan**: Write unit tests that construct an `AnimationSession`, simulate the current `update()` ordering (step then fire_event), and assert that events are NOT consumed in the same frame. Run these tests on the UNFIXED code to observe failures and confirm the root cause.

**Test Cases**:
1. **Same-Frame Event Consumption Test**: Create a session, fire an event, then step — verify the event is consumed. Then reverse the order (step then fire) — verify the event is NOT consumed. (will demonstrate the bug on unfixed ordering)
2. **Delayed Transition Test**: Create a session with a state machine that transitions on `mousedown`, simulate the buggy ordering, verify the transition does not fire on the event frame. (will fail on unfixed code)
3. **Unbounded Delay Test**: Simulate the scenario where `step()` returns `needs_redraw: false` and no continuous repaint is active, verify that a queued event would not be processed until the next external repaint. (will demonstrate the delay on unfixed code)

**Expected Counterexamples**:
- `step()` returns `needs_redraw: false` and `current_state_id` unchanged when events are fired after `step()`
- Events queued after `step()` are only consumed on the next `step()` call

### Fix Checking

**Goal**: Verify that for all inputs where the bug condition holds, the fixed function produces the expected behavior.

**Pseudocode:**
```
FOR ALL input WHERE isBugCondition(input) DO
  // Collect events BEFORE step
  payloads := collect_canvas_interaction_events(app, ctx)
  // Events are now in pending_events
  result := session.step(effective_dt)
  ASSERT result reflects event-triggered transitions
  ASSERT result.current_state_id matches expected post-transition state
END FOR
```

### Preservation Checking

**Goal**: Verify that for all inputs where the bug condition does NOT hold, the fixed function produces the same result as the original function.

**Pseudocode:**
```
FOR ALL input WHERE NOT isBugCondition(input) DO
  ASSERT update_original(input) = update_fixed(input)
END FOR
```

**Testing Approach**: Property-based testing is recommended for preservation checking because:
- It generates many test cases automatically across the input domain
- It catches edge cases that manual unit tests might miss
- It provides strong guarantees that behavior is unchanged for all non-buggy inputs

**Test Plan**: Observe behavior on UNFIXED code first for frames with no interaction events, then write property-based tests capturing that behavior.

**Test Cases**:
1. **No-Event Frame Preservation**: Verify that frames with no interaction events produce identical `AnimationStep` results (same overrides, same `needs_redraw`, same `scene_time_secs`) before and after the fix.
2. **WebSocket Broadcast Preservation**: Verify that interaction events are still broadcast over WebSocket with correct sequence numbers and payload data after the fix.
3. **Layout Operation Preservation**: Verify that `show_canvas_panel()` still correctly performs pan/zoom, pixel overlay, and reference compositing after the event collection is extracted.
4. **Repaint Scheduling Preservation**: Verify that `should_request_immediate_repaint()` continues to evaluate its conditions identically.

### Unit Tests

- Test `AnimationSession::step()` consumes events that were fired before the call
- Test `AnimationSession::step()` does NOT consume events fired after the call (demonstrates bug)
- Test `collect_canvas_interaction_events()` correctly reads egui events and calls `fire_event()`
- Test that the new `last_canvas_rect` field is correctly updated each frame
- Test edge case: first frame with no previous canvas rect falls back to `ctx.available_rect()`

### Property-Based Tests

- Generate random sequences of (frame_has_events: bool, animation_playing: bool, effective_dt: f64) and verify that the fixed ordering always processes events in the same frame they arrive
- Generate random `AnimationStep` inputs with no events and verify the fixed code produces identical results to the original
- Generate random interaction payloads and verify WebSocket broadcast sequence numbers are monotonically increasing and payload data is preserved

### Integration Tests

- Test full `update()` flow with a scene containing a state machine that transitions on `mousedown`, verify the transition fires in the same frame as the click
- Test that sidebar open/close transitions don't break the cached canvas rect used for early event collection
- Test that the fix works correctly when multiple interaction events arrive in the same frame
