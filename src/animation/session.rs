//! Animation session driven once per render frame with the full frame delta.

use std::collections::HashMap;

use anyhow::Result;

use crate::dsl::SceneDSL;
use crate::protocol::InteractionEventPayload;
use crate::state_machine::{
    self, EventModifiers, FiredEvent, FiredEvents, MotionChannelDebug, MousePosition, OverrideKey,
    StateMachineRuntime, TickResult,
};

// ---------------------------------------------------------------------------
// Animation step result
// ---------------------------------------------------------------------------

/// Result of a single `AnimationSession::step()` call.
#[derive(Debug, Clone)]
pub struct AnimationStep {
    /// Current animation parameter state (full sticky set, not per-state deltas).
    pub active_overrides: HashMap<OverrideKey, serde_json::Value>,
    /// True if any override values changed since last step.
    pub needs_redraw: bool,
    /// Current scene time after this step.
    pub scene_time_secs: f64,
    /// Whether the session is still active (runtime not finished).
    pub active: bool,
    /// Diagnostics from the runtime tick(s).
    pub diagnostics: Vec<String>,
    /// Current active state id after this step.
    pub current_state_id: String,
    /// Active transition id after this step, when transitioning.
    pub active_transition_id: Option<String>,
    /// Per-state local elapsed times (state_id → seconds).
    pub state_local_times: std::collections::BTreeMap<String, f64>,
    /// Whether the runtime has finished (reached exit state).
    pub finished: bool,
    /// Per-property physical/timeline diagnostics.
    pub motion_channels: Vec<MotionChannelDebug>,
}

// ---------------------------------------------------------------------------
// Animation session
// ---------------------------------------------------------------------------

/// A self-contained animation session that wraps the state machine runtime.
///
/// The session tracks baseline scene values for overridden params so it can
/// restore them when playback is reset or stopped.
#[derive(Debug, Clone)]
pub struct AnimationSession {
    /// The state machine runtime.
    runtime: StateMachineRuntime,
    /// Monotonic sum of accepted render-frame deltas.
    scene_time: f64,
    /// Baseline values for tracked keys (from scene at compile time).
    /// Used to restore params when playback is reset.
    base_values: HashMap<OverrideKey, serde_json::Value>,
    /// Current animation parameter state (last runloop flush).
    active_overrides: HashMap<OverrideKey, serde_json::Value>,
    /// Queued events to fire on the next step (e.g. "mousedown").
    pending_events: FiredEvents,
    /// Whether the initial dt=0 tick has been fired.
    /// The first call to `step()` always fires a single tick with dt=0
    /// to establish the initial state (matching the test trace path).
    first_tick_fired: bool,
    /// Cached per-state local times from the last tick (preserved across
    /// no-tick frames to prevent UI flashing).
    cached_state_local_times: std::collections::BTreeMap<String, f64>,
    /// Cached finished flag from the last tick.
    cached_finished: bool,
    cached_motion_channels: Vec<MotionChannelDebug>,
}

impl From<&InteractionEventPayload> for FiredEvent {
    fn from(payload: &InteractionEventPayload) -> Self {
        let data = payload.data.as_ref();
        let modifiers = data.and_then(|data| data.modifiers.as_ref());
        let key = data.and_then(|data| data.key.as_ref());
        Self {
            event_type: payload.event_type.clone(),
            key: key.map(|key| key.key.clone()),
            button: data.and_then(|data| data.button.clone()),
            repeat: key.is_some_and(|key| key.repeat),
            modifiers: EventModifiers {
                ctrl: modifiers.is_some_and(|modifiers| modifiers.ctrl),
                alt: modifiers.is_some_and(|modifiers| modifiers.alt),
                shift: modifiers.is_some_and(|modifiers| modifiers.shift),
                meta: modifiers.is_some_and(|modifiers| modifiers.meta),
            },
        }
    }
}

impl AnimationSession {
    /// Build an animation session from a scene DSL.
    ///
    /// Returns `None` if the scene has no state machine, or an error if
    /// validation fails.
    pub fn from_scene(scene: &SceneDSL) -> Result<Option<Self>> {
        let runtime = match state_machine::compile_from_scene(scene)? {
            Some(rt) => rt,
            None => return Ok(None),
        };

        let base_values = state_machine::collect_scene_current_values(scene);

        Ok(Some(Self {
            runtime,
            scene_time: 0.0,
            base_values,
            active_overrides: HashMap::new(),
            pending_events: Vec::new(),
            first_tick_fired: false,
            cached_state_local_times: std::collections::BTreeMap::new(),
            cached_finished: false,
            cached_motion_channels: Vec::new(),
        }))
    }

    /// Advance the session by `real_dt` seconds (wall-clock delta).
    ///
    /// Every driver receives the full delta exactly once; there is no accumulator or substep.
    pub fn step(&mut self, real_dt: f64) -> AnimationStep {
        if self.runtime.finished {
            return AnimationStep {
                active_overrides: self.active_overrides.clone(),
                needs_redraw: false,
                scene_time_secs: self.scene_time,
                active: false,
                diagnostics: vec![],
                current_state_id: self.runtime.current_state_id().to_string(),
                active_transition_id: self.runtime.active_transition_id().map(str::to_string),
                state_local_times: std::collections::BTreeMap::new(),
                finished: true,
                motion_channels: self.cached_motion_channels.clone(),
            };
        }

        let mut diagnostics = Vec::new();
        let mut last_tick_result = None;
        let no_events = Vec::new();
        let events = std::mem::take(&mut self.pending_events);

        // On the very first step, fire a single dt=0 tick to establish
        // initial state before the clock starts advancing.  This matches
        // the test trace path where frame 0 has dt=0.
        if !self.first_tick_fired {
            self.first_tick_fired = true;
            last_tick_result = Some(self.runtime_tick(0.0, &no_events, &mut diagnostics));
        }

        for event in events {
            let event = vec![event];
            last_tick_result = Some(self.runtime_tick(0.0, &event, &mut diagnostics));
        }

        if real_dt.is_finite() && real_dt > 0.0 {
            self.scene_time += real_dt;
            last_tick_result = Some(self.runtime_tick(real_dt, &no_events, &mut diagnostics));
        }

        // Determine new active overrides from the last tick's flush.
        // When no tick fired this frame (tick_count == 0), preserve the
        // existing overrides so the UI doesn't flash between populated and
        // empty on frames where the fixed-step clock hasn't accumulated
        // enough time for a tick.
        let needs_redraw;
        if let Some(ref result) = last_tick_result {
            let new_overrides = result.overrides.clone();
            needs_redraw = new_overrides != self.active_overrides;

            self.active_overrides = new_overrides;
        } else {
            // No tick this frame — keep existing overrides, nothing changed.
            needs_redraw = false;
        }

        let (is_finished, state_local_times, motion_channels) = if let Some(tr) = last_tick_result {
            // A tick fired — update cached values.
            self.cached_state_local_times = tr.state_local_times.clone();
            self.cached_finished = tr.finished;
            self.cached_motion_channels = tr.motion_channels.clone();
            (tr.finished, tr.state_local_times, tr.motion_channels)
        } else {
            // No tick this frame — reuse cached values so the UI stays stable.
            (
                self.cached_finished,
                self.cached_state_local_times.clone(),
                self.cached_motion_channels.clone(),
            )
        };

        AnimationStep {
            active_overrides: self.active_overrides.clone(),
            needs_redraw,
            scene_time_secs: self.scene_time,
            active: !is_finished,
            diagnostics,
            current_state_id: self.runtime.current_state_id().to_string(),
            active_transition_id: self.runtime.active_transition_id().map(str::to_string),
            state_local_times,
            finished: is_finished,
            motion_channels,
        }
    }

    /// Update the baseline values when a WebSocket UniformDelta arrives.
    ///
    /// This adjusts the restore-point but does not interfere with the
    /// currently active overrides (SM wins).
    pub fn update_base_values(&mut self, updates: &[(OverrideKey, serde_json::Value)]) {
        for (key, value) in updates {
            self.base_values.insert(key.clone(), value.clone());
        }
        self.runtime.update_current_values(updates);
    }

    /// Queue an event to fire on the next `step()`.
    ///
    /// Events are consumed in order as dt=0 control updates so input edges are
    /// not dropped on frames without a fixed-step animation tick.
    pub fn fire_event(&mut self, event: impl Into<FiredEvent>) {
        self.pending_events.push(event.into());
    }

    /// Update the latest mouse frag-pixel position visible to mutation nodes.
    pub fn update_mouse_position(&mut self, position: MousePosition) {
        self.runtime.set_mouse_position(position);
    }

    /// Reset and force the session to remain in one selectable State.
    /// Returns the initial frame for immediate application by the UI.
    pub fn force_state(&mut self, state_id: &str) -> Result<AnimationStep> {
        self.runtime.force_state(state_id)?;
        self.scene_time = 0.0;
        self.active_overrides.clear();
        self.pending_events.clear();
        self.first_tick_fired = false;
        self.cached_state_local_times.clear();
        self.cached_finished = false;
        self.cached_motion_channels.clear();
        Ok(self.step(0.0))
    }

    /// Whether the session is still active (runtime not finished).
    pub fn is_active(&self) -> bool {
        !self.runtime.finished
    }

    /// Current render-frame scene time.
    pub fn scene_time(&self) -> f64 {
        self.scene_time
    }

    /// Build the immutable declaration-side value snapshot consumed by renderers.
    ///
    /// Mutation output is overlaid for presentation only. Callers must not feed
    /// this snapshot back into the motion engine or a later Mutation evaluation.
    pub(crate) fn presentation_snapshot(&self) -> HashMap<OverrideKey, serde_json::Value> {
        let mut snapshot = state_machine::trace::tracked_override_keys(self.runtime.definition())
            .into_iter()
            .filter_map(|raw_key| {
                let key = OverrideKey::parse(&raw_key)?;
                let value = self.base_values.get(&key)?.clone();
                Some((key, value))
            })
            .collect::<HashMap<_, _>>();
        snapshot.extend(self.active_overrides.clone());
        snapshot
    }

    /// Reset the session: rewind the clock, reset the runtime to its initial
    /// state, and clear all active overrides. Returns the set of active keys
    /// that should be restored to their base values.
    pub fn reset(&mut self) -> HashMap<OverrideKey, serde_json::Value> {
        // Collect restoration overrides for all currently active keys.
        let mut restores = HashMap::new();
        for key in self.active_overrides.keys() {
            if let Some(base) = self.base_values.get(key) {
                restores.insert(key.clone(), base.clone());
            }
        }

        self.runtime.reset();
        self.scene_time = 0.0;
        self.active_overrides.clear();
        self.pending_events.clear();
        self.first_tick_fired = false;
        self.cached_state_local_times.clear();
        self.cached_finished = false;
        self.cached_motion_channels.clear();

        restores
    }

    /// Access the underlying runtime (for diagnostics/testing).
    pub fn runtime(&self) -> &StateMachineRuntime {
        &self.runtime
    }

    fn runtime_tick(
        &mut self,
        dt: f64,
        events: &FiredEvents,
        diagnostics: &mut Vec<String>,
    ) -> TickResult {
        let result = self.runtime.tick(dt, &HashMap::new(), events);
        diagnostics.extend(result.diagnostics.iter().cloned());
        result
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
