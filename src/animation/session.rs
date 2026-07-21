//! Animation session driven once per render frame with the full frame delta.

use std::collections::HashMap;

use anyhow::Result;

use crate::dsl::SceneDSL;
use crate::state_machine::{
    self, MotionChannelDebug, MousePosition, OverrideKey, StateMachineRuntime, TickResult,
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
    pending_events: Vec<String>,
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

        // Collect base values for all params this state machine can override.
        let base_values = collect_base_values(scene, runtime.definition());

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
    }

    /// Queue an event to fire on the next `step()`.
    ///
    /// Events are consumed in order as dt=0 control updates so input edges are
    /// not dropped on frames without a fixed-step animation tick.
    pub fn fire_event(&mut self, event_name: impl Into<String>) {
        self.pending_events.push(event_name.into());
    }

    /// Update the latest mouse frag-pixel position visible to mutation nodes.
    pub fn update_mouse_position(&mut self, position: MousePosition) {
        self.runtime.set_mouse_position(position);
    }

    /// Whether the session is still active (runtime not finished).
    pub fn is_active(&self) -> bool {
        !self.runtime.finished
    }

    /// Current render-frame scene time.
    pub fn scene_time(&self) -> f64 {
        self.scene_time
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
        events: &Vec<String>,
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

/// Collect the base (initial) values for all params this state machine can override.
fn collect_base_values(
    scene: &SceneDSL,
    sm: &state_machine::StateMachine,
) -> HashMap<OverrideKey, serde_json::Value> {
    use crate::state_machine::mutation;

    let mut base = HashMap::new();

    // From static parameterOverrides in states.
    for state in &sm.states {
        for key_str in state.parameter_overrides.keys() {
            if let Some(ok) = OverrideKey::parse(key_str)
                && let Some(val) = lookup_node_param(scene, &ok)
            {
                base.insert(ok, val);
            }
        }
    }

    // From mutation output targets (unified resolver).
    for m in &sm.mutations {
        for ok in mutation::all_output_target_keys(m) {
            if let std::collections::hash_map::Entry::Vacant(e) = base.entry(ok.clone())
                && let Some(val) = lookup_node_param(scene, &ok)
            {
                e.insert(val);
            }
        }
    }

    base
}

fn lookup_node_param(scene: &SceneDSL, key: &OverrideKey) -> Option<serde_json::Value> {
    // First try top-level nodes (exact match).
    if let Some(val) = scene
        .nodes
        .iter()
        .find(|n| n.id == key.node_id)
        .and_then(|n| n.params.get(key.param_name.as_str()))
        .cloned()
    {
        return Some(val);
    }

    // Fall back to searching inside group definitions.
    // The state machine may reference nodes that live inside a group
    // (e.g. `FloatInput_53`) which only appear in `scene.groups[].nodes`.
    for group in &scene.groups {
        if let Some(val) = group
            .nodes
            .iter()
            .find(|n| n.id == key.node_id)
            .and_then(|n| n.params.get(key.param_name.as_str()))
            .cloned()
        {
            return Some(val);
        }
    }

    None
}
