//! Animation session with deterministic fixed-step clock.
//!
//! The session owns a `StateMachineRuntime` and a `FixedStepClock`, producing
//! per-frame override maps that the app can apply to the scene before GPU
//! uniform packing.

use std::collections::HashMap;

use anyhow::Result;

use crate::dsl::SceneDSL;
use crate::state_machine::{self, OverrideKey, StateMachineRuntime};

use super::runloop::Runloop;
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
    /// All currently active overrides (full set, not deltas).
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
    /// Time elapsed in the current state (from last tick).
    pub state_local_time_secs: f64,
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
/// restore them when the state machine stops overriding a key.
#[derive(Debug, Clone)]
pub struct AnimationSession {
    /// The state machine runtime.
    runtime: StateMachineRuntime,
    /// Deterministic fixed-step clock.
    clock: FixedStepClock,
    /// Runloop orchestrator (owns ValuePool + TaskPool).
    runloop: Runloop,
    /// Baseline values for tracked keys (from scene at compile time).
    /// Used to restore params when the runtime stops overriding them.
    base_values: HashMap<OverrideKey, serde_json::Value>,
    /// Currently active overrides (last tick result).
    active_overrides: HashMap<OverrideKey, serde_json::Value>,
    /// Previous override key set — for detecting changes/removals.
    prev_override_keys: Vec<OverrideKey>,
    /// Queued events to fire on the next tick (e.g. "mousedown").
    pending_events: Vec<String>,
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
            prev_override_keys: Vec::new(),
            pending_events: Vec::new(),
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
                state_local_time_secs: 0.0,
                transition_blend: None,
                finished: true,
            };
        }

        let tick_count = self.clock.advance(real_dt);
        let mut diagnostics = Vec::new();
        let mut last_tick_result = None;

        // Drain pending events — fire them on the first tick only (they are
        // instantaneous triggers, not sustained state).
        let events = std::mem::take(&mut self.pending_events);

        for i in 0..tick_count {
            let tick_events = if i == 0 { &events } else { &Vec::new() };
            let result = self.runloop.tick(
                &mut self.runtime,
                self.clock.step_secs,
                &HashMap::new(),
                tick_events,
            );
            diagnostics.extend(result.diagnostics.iter().cloned());
            last_tick_result = Some(result);
        }

        // Determine new active overrides from the last tick's flush.
        let new_overrides = last_tick_result
            .as_ref()
            .map(|r| r.overrides.clone())
            .unwrap_or_default();

        // Detect if values changed.
        let needs_redraw = tick_count > 0 && new_overrides != self.active_overrides;

        // Track removed keys for base-value restoration.
        let new_keys: Vec<OverrideKey> = new_overrides.keys().cloned().collect();
        let _removed: Vec<OverrideKey> = self
            .prev_override_keys
            .iter()
            .filter(|k| !new_overrides.contains_key(k))
            .cloned()
            .collect();

        self.active_overrides = new_overrides;
        self.prev_override_keys = new_keys;

        let (is_finished, state_local_time_secs, transition_blend) = last_tick_result
            .and_then(|r| r.tick_result)
            .map(|tr| (tr.finished, tr.state_local_time_secs, tr.transition_blend))
            .unwrap_or((false, 0.0, None));

        AnimationStep {
            active_overrides: self.active_overrides.clone(),
            needs_redraw,
            scene_time_secs: self.clock.scene_time(),
            active: !is_finished,
            diagnostics,
            current_state_id: self.runtime.current_state_id().to_string(),
            active_transition_id: self.runtime.active_transition_id().map(str::to_string),
            state_local_time_secs,
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

    /// Queue an event to fire on the next `step()` tick.
    ///
    /// Events are consumed on the first fixed-step tick of the next `step()`
    /// call, then cleared. Use this to feed canvas interaction events
    /// (e.g. `"mousedown"`, `"mouseup"`) into the state machine.
    pub fn fire_event(&mut self, event_name: impl Into<String>) {
        self.pending_events.push(event_name.into());
    }

    /// Get overrides that need to be restored (removed from active set).
    ///
    /// Call this after `step()` to find keys whose overrides were dropped
    /// and that need their base values written back to the scene.
    pub fn restoration_overrides(&self) -> HashMap<OverrideKey, serde_json::Value> {
        let mut restores = HashMap::new();
        for key in &self.prev_override_keys {
            if !self.active_overrides.contains_key(key)
                && let Some(base) = self.base_values.get(key)
            {
                restores.insert(key.clone(), base.clone());
            }
        }
        restores
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
    /// state, and clear all active overrides.  Returns the set of overrides
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
        self.prev_override_keys.clear();
        self.pending_events.clear();

        restores
    }

    /// Access the underlying runtime (for diagnostics/testing).
    pub fn runtime(&self) -> &StateMachineRuntime {
        &self.runtime
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
    scene
        .nodes
        .iter()
        .find(|n| n.id == key.node_id)
        .and_then(|n| n.params.get(key.param_name.as_str()))
        .cloned()
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
