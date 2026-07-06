//! Animation session with deterministic fixed-step clock.
//!
//! The session owns a `StateMachineRuntime` and a `FixedStepClock`, producing
//! per-frame override maps that the app can apply to the scene before GPU
//! uniform packing.

use std::collections::HashMap;

use anyhow::Result;

use crate::dsl::SceneDSL;
use crate::state_machine::{self, MousePosition, OverrideKey, StateMachineRuntime};

use super::runloop::{Runloop, RunloopTickResult};
use super::task::{AnimationTask, TaskKind};

// ---------------------------------------------------------------------------
// Fixed-step clock
// ---------------------------------------------------------------------------

/// A deterministic fixed-step clock suitable for animation.
///
/// On each `advance(real_dt)` call the accumulator absorbs the real dt and
/// yields zero or more fixed-size ticks.  This guarantees identical sequences
/// across runs regardless of frame rate.  The clock also caps the number of
/// ticks per frame to avoid spiralling.
#[derive(Debug, Clone)]
pub struct FixedStepClock {
    /// Duration of one tick in seconds.
    pub step_secs: f64,
    /// Accumulated time not yet consumed by a tick.
    accumulator: f64,
    /// Monotonic scene time (sum of all consumed ticks).
    scene_time: f64,
    /// Safety cap: max ticks per frame to prevent spiral-of-death.
    pub max_steps_per_frame: usize,
}

impl FixedStepClock {
    /// Create a new clock with the given fixed step size.
    pub fn new(step_secs: f64, max_steps_per_frame: usize) -> Self {
        Self {
            step_secs,
            accumulator: 0.0,
            scene_time: 0.0,
            max_steps_per_frame,
        }
    }

    /// Default 60 fps clock.
    pub fn default_60fps() -> Self {
        Self::new(1.0 / 60.0, 10)
    }

    /// Advance the clock by `real_dt` seconds and return the number of fixed
    /// ticks that should be executed.
    pub fn advance(&mut self, real_dt: f64) -> usize {
        self.accumulator += real_dt;
        let mut ticks = 0usize;
        while self.accumulator >= self.step_secs && ticks < self.max_steps_per_frame {
            self.accumulator -= self.step_secs;
            self.scene_time += self.step_secs;
            ticks += 1;
        }
        // If we hit the cap, drain remaining accumulator to prevent spiral.
        if ticks >= self.max_steps_per_frame {
            self.accumulator = 0.0;
        }
        ticks
    }

    /// Current scene time in seconds.
    pub fn scene_time(&self) -> f64 {
        self.scene_time
    }

    /// Reset the clock to time zero.
    pub fn reset(&mut self) {
        self.accumulator = 0.0;
        self.scene_time = 0.0;
    }
}

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
    /// Blend factor if a transition is in progress (0.0 → 1.0).
    pub transition_blend: Option<f64>,
    /// Whether the runtime has finished (reached exit state).
    pub finished: bool,
}

// ---------------------------------------------------------------------------
// Animation session
// ---------------------------------------------------------------------------

/// A self-contained animation session that wraps the state machine runtime
/// with a deterministic fixed-step clock.
///
/// The session tracks baseline scene values for overridden params so it can
/// restore them when playback is reset or stopped.
#[derive(Debug, Clone)]
pub struct AnimationSession {
    /// The state machine runtime.
    runtime: StateMachineRuntime,
    /// Deterministic fixed-step clock.
    clock: FixedStepClock,
    /// Runloop orchestrator (owns ValuePool + TaskPool).
    runloop: Runloop,
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
    /// Cached transition blend from the last tick.
    cached_transition_blend: Option<f64>,
    /// Cached finished flag from the last tick.
    cached_finished: bool,
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

        // Initialize the Runloop with a single StateMachineDriven task and
        // populate the ValuePool with baseline values from the scene.
        let mut runloop = Runloop::new();
        for (key, json_val) in &base_values {
            let baseline = json_val.as_f64().unwrap_or(0.0);
            runloop.value_pool.insert(key.clone(), baseline);
        }
        runloop
            .task_pool
            .add(AnimationTask::new("sm", TaskKind::StateMachineDriven));

        Ok(Some(Self {
            runtime,
            clock: FixedStepClock::default_60fps(),
            runloop,
            base_values,
            active_overrides: HashMap::new(),
            pending_events: Vec::new(),
            first_tick_fired: false,
            cached_state_local_times: std::collections::BTreeMap::new(),
            cached_transition_blend: None,
            cached_finished: false,
        }))
    }

    /// Advance the session by `real_dt` seconds (wall-clock delta).
    ///
    /// Internally runs N fixed-step ticks, produces the merged override set,
    /// and detects whether a redraw is needed.
    pub fn step(&mut self, real_dt: f64) -> AnimationStep {
        if self.runtime.finished {
            return AnimationStep {
                active_overrides: self.active_overrides.clone(),
                needs_redraw: false,
                scene_time_secs: self.clock.scene_time(),
                active: false,
                diagnostics: vec![],
                current_state_id: self.runtime.current_state_id().to_string(),
                active_transition_id: self.runtime.active_transition_id().map(str::to_string),
                state_local_times: std::collections::BTreeMap::new(),
                transition_blend: None,
                finished: true,
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
            last_tick_result = Some(self.runloop_tick(0.0, &no_events, &mut diagnostics));
        }

        let tick_count = self.clock.advance(real_dt);
        if tick_count > 0 {
            last_tick_result =
                Some(self.runloop_tick(self.clock.step_secs, &no_events, &mut diagnostics));
        }

        for event in events {
            let event = vec![event];
            last_tick_result = Some(self.runloop_tick(0.0, &event, &mut diagnostics));
        }

        if tick_count > 1 {
            for _ in 1..tick_count {
                last_tick_result =
                    Some(self.runloop_tick(self.clock.step_secs, &no_events, &mut diagnostics));
            }
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

        let (is_finished, state_local_times, transition_blend) =
            if let Some(tr) = last_tick_result.and_then(|r| r.tick_result) {
                // A tick fired — update cached values.
                self.cached_state_local_times = tr.state_local_times.clone();
                self.cached_transition_blend = tr.transition_blend;
                self.cached_finished = tr.finished;
                (tr.finished, tr.state_local_times, tr.transition_blend)
            } else {
                // No tick this frame — reuse cached values so the UI stays stable.
                (
                    self.cached_finished,
                    self.cached_state_local_times.clone(),
                    self.cached_transition_blend,
                )
            };

        AnimationStep {
            active_overrides: self.active_overrides.clone(),
            needs_redraw,
            scene_time_secs: self.clock.scene_time(),
            active: !is_finished,
            diagnostics,
            current_state_id: self.runtime.current_state_id().to_string(),
            active_transition_id: self.runtime.active_transition_id().map(str::to_string),
            state_local_times,
            transition_blend,
            finished: is_finished,
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

    /// Current fixed-step scene time.
    pub fn scene_time(&self) -> f64 {
        self.clock.scene_time()
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
        self.clock.reset();
        self.runloop.reset();
        self.active_overrides.clear();
        self.pending_events.clear();
        self.first_tick_fired = false;
        self.cached_state_local_times.clear();
        self.cached_transition_blend = None;
        self.cached_finished = false;

        restores
    }

    /// Access the underlying runtime (for diagnostics/testing).
    pub fn runtime(&self) -> &StateMachineRuntime {
        &self.runtime
    }

    fn runloop_tick(
        &mut self,
        dt: f64,
        events: &Vec<String>,
        diagnostics: &mut Vec<String>,
    ) -> RunloopTickResult {
        let result = self
            .runloop
            .tick(&mut self.runtime, dt, &HashMap::new(), events);
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_step_clock_basic() {
        let mut clock = FixedStepClock::new(1.0 / 60.0, 10);
        assert!((clock.scene_time() - 0.0).abs() < 1e-12);

        // Advance by exactly one step.
        let ticks = clock.advance(1.0 / 60.0);
        assert_eq!(ticks, 1);
        assert!((clock.scene_time() - 1.0 / 60.0).abs() < 1e-12);

        // Advance by 2.5 steps → 2 ticks, remainder accumulates.
        let ticks = clock.advance(2.5 / 60.0);
        assert_eq!(ticks, 2);
        assert!((clock.scene_time() - 3.0 / 60.0).abs() < 1e-12);
    }

    #[test]
    fn fixed_step_clock_caps_ticks() {
        let mut clock = FixedStepClock::new(1.0 / 60.0, 5);
        // Advance by a huge dt.
        let ticks = clock.advance(1.0); // would be 60 ticks
        assert_eq!(ticks, 5); // capped
        assert!((clock.scene_time() - 5.0 / 60.0).abs() < 1e-12);
    }

    #[test]
    fn fixed_step_clock_reset() {
        let mut clock = FixedStepClock::new(1.0 / 60.0, 10);
        clock.advance(0.5);
        clock.reset();
        assert!((clock.scene_time() - 0.0).abs() < 1e-12);
    }
}
