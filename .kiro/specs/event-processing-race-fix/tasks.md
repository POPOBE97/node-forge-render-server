# Implementation Plan

- [x] 1. Write bug condition exploration test
  - **Property 1: Fault Condition** - Interaction Events Not Consumed Same Frame
  - **CRITICAL**: This test MUST FAIL on unfixed code — failure confirms the bug exists
  - **DO NOT attempt to fix the test or the code when it fails**
  - **NOTE**: This test encodes the expected behavior — it will validate the fix when it passes after implementation
  - **GOAL**: Surface counterexamples that demonstrate the race condition between `session.step()` and `fire_event()`
  - **Scoped PBT Approach**: Scope the property to the concrete failing case: create an `AnimationSession` with a state machine that listens for an interaction event (e.g. `mousedown`), then test that firing the event BEFORE `step()` results in the event being consumed in the same `step()` call, while the current code ordering (step then fire) does NOT consume it
  - Write a property-based test in `tests/` that:
    - Constructs an `AnimationSession` with `pending_events` support
    - For random interaction event names and `effective_dt` values: fires the event via `session.fire_event(name)`, then calls `session.step(dt)`
    - Asserts that after `step()`, the event was drained from `pending_events` (i.e. `pending_events` is empty)
    - Also tests the BUGGY ordering: calls `session.step(dt)` first, then `session.fire_event(name)`, and asserts the event IS consumed in that same step — this assertion will FAIL on unfixed code, confirming the bug
  - Run test on UNFIXED code
  - **EXPECTED OUTCOME**: Test FAILS (the buggy-ordering assertion proves events are not consumed in the same frame they arrive)
  - Document counterexamples found (e.g., "`step()` returns with `pending_events` still empty when `fire_event('mousedown')` is called after `step()`")
  - Mark task complete when test is written, run, and failure is documented
  - _Requirements: 1.1, 2.1, 2.2_

- [x] 2. Write preservation property tests (BEFORE implementing fix)
  - **Property 2: Preservation** - Non-Interaction Frame Behavior Preserved
  - **IMPORTANT**: Follow observation-first methodology
  - **Step 1 — Observe on UNFIXED code**:
    - Observe: `session.step(dt)` with empty `pending_events` produces an `AnimationStep` with unchanged `current_state_id`, deterministic `scene_time_secs`, and `needs_redraw` based solely on time-driven logic
    - Observe: `should_request_immediate_repaint()` returns the same result regardless of whether event collection is extracted or not
    - Observe: When `animation_playing` is false, `time_value_secs` advances by wall-clock delta and `session.step()` is not called
  - **Step 2 — Write property-based tests capturing observed behavior**:
    - For random `effective_dt` values (positive f64) and random `animation_playing` states: when no interaction events are present, `session.step(dt)` with empty `pending_events` produces the same `AnimationStep` result (same `active_overrides`, same `needs_redraw`, same `scene_time_secs`, same `current_state_id`) as observed on unfixed code
    - For random frame states with no interaction events: `should_request_immediate_repaint()` evaluates identically
    - Property-based testing generates many test cases for stronger preservation guarantees
  - Run tests on UNFIXED code
  - **EXPECTED OUTCOME**: Tests PASS (confirms baseline behavior to preserve)
  - Mark task complete when tests are written, run, and passing on unfixed code
  - _Requirements: 3.1, 3.4, 3.5_

- [x] 3. Fix event processing race condition in `App::update()`

  - [x] 3.1 Add `last_canvas_rect` field to `App` struct
    - Add `last_canvas_rect: Option<egui::Rect>` field to the `App` struct in `src/app/mod.rs`
    - Initialize to `None` in `App::new()` or default
    - _Bug_Condition: isBugCondition(input) where animation_session is active AND interaction events arrive AND step() runs before fire_event()_
    - _Requirements: 2.1_

  - [x] 3.2 Create `collect_canvas_interaction_events()` function
    - Create a new public function in `src/app/canvas_controller.rs`: `collect_canvas_interaction_events(app, ctx) -> Vec<InteractionEventPayload>`
    - Read egui frame events via `ctx.input(|i| i.events.clone())`
    - Compute `interaction_clean_state` from current app state
    - Use `app.last_canvas_rect` (falling back to `ctx.available_rect()` on first frame) for hit-testing
    - Call `collect_interaction_payloads()` with frame events and the cached canvas rect
    - Call `session.fire_event()` for each collected interaction payload to queue events into `pending_events`
    - Return the collected payloads for later WebSocket broadcasting
    - _Bug_Condition: isBugCondition(input) where step() currently runs before fire_event() queues events_
    - _Expected_Behavior: Events are queued into pending_events BEFORE session.step() runs_
    - _Preservation: Layout-dependent operations in show_canvas_panel() remain inside CentralPanel closure_
    - _Requirements: 2.1, 2.2, 3.2_

  - [x] 3.3 Call early event collection before `session.step()` in `App::update()`
    - In `src/app/mod.rs`, insert a call to `canvas_controller::collect_canvas_interaction_events(self, ctx)` before the animation session tick block (~line 255)
    - Store the returned payloads in a local variable for later WebSocket broadcasting
    - _Bug_Condition: isBugCondition(input) — this is the core ordering fix_
    - _Expected_Behavior: collect_canvas_interaction_events() runs before session.step(), so pending_events contains current-frame events when step() drains them_
    - _Preservation: Non-interaction frames still call step() with empty pending_events as before_
    - _Requirements: 2.1, 2.2, 3.1_

  - [x] 3.4 Remove duplicate event collection from `show_canvas_panel()` and pass pre-collected payloads
    - Remove the `collect_interaction_payloads()` + `fire_event()` block from the end of `show_canvas_panel()`
    - Modify `show_canvas_panel()` signature to accept pre-collected `Vec<InteractionEventPayload>`
    - Have `show_canvas_panel()` only perform WebSocket broadcasting using the passed-in payloads
    - Store `animated_canvas_rect` into `app.last_canvas_rect` at the end of `show_canvas_panel()` for next frame's early collection
    - _Preservation: All layout-dependent operations (pan/zoom, pixel overlay, reference compositing, viewport indicators, context menu) remain unchanged inside CentralPanel closure_
    - _Requirements: 2.3, 3.2, 3.3_

  - [x] 3.5 Broadcast events over WebSocket after `step()` using pre-collected payloads
    - After `session.step()` and `sync_animation_state_interaction_events()`, broadcast the pre-collected payloads over WebSocket
    - Ensure sequence numbers and payload data are preserved
    - _Preservation: WebSocket broadcasting behavior unchanged — same sequence numbers, same payload data, same sync_animation_state_interaction_events call with post-step state IDs_
    - _Requirements: 2.3, 3.3_

  - [x] 3.6 Verify bug condition exploration test now passes
    - **Property 1: Expected Behavior** - Interaction Events Consumed Same Frame
    - **IMPORTANT**: Re-run the SAME test from task 1 — do NOT write a new test
    - The test from task 1 encodes the expected behavior: events fired before `step()` are consumed in the same `step()` call
    - With the fix applied, the ordering is now correct (collect → fire → step), so the test should pass
    - Run bug condition exploration test from step 1
    - **EXPECTED OUTCOME**: Test PASSES (confirms the race condition is fixed and events are consumed same-frame)
    - _Requirements: 2.1, 2.2_

  - [x] 3.7 Verify preservation tests still pass
    - **Property 2: Preservation** - Non-Interaction Frame Behavior Preserved
    - **IMPORTANT**: Re-run the SAME tests from task 2 — do NOT write new tests
    - Run preservation property tests from step 2
    - **EXPECTED OUTCOME**: Tests PASS (confirms no regressions in non-interaction frame behavior)
    - Confirm all preservation tests still pass after fix (no regressions)
    - _Requirements: 3.1, 3.4, 3.5_

- [x] 4. Checkpoint — Ensure all tests pass
  - Run `cargo test` to ensure all existing tests and new property-based tests pass
  - Run `cargo clippy` to ensure no new warnings
  - Ensure all tests pass, ask the user if questions arise
