//! Tick-driven state machine runtime.
//!
//! The runtime is intentionally decoupled from the render pipeline.
//! It consumes a compiled `StateMachine` definition (from DSL) and
//! produces `HashMap<OverrideKey, serde_json::Value>` parameter
//! overrides each tick.
//!
//! # Lifecycle
//!
//! ```text
//! StateMachineRuntime::new(sm)   // compile from DSL
//!     .tick(dt, params)          // called each frame → overrides
//!     .tick(dt, params)
//!     ...
//!     .reset()                   // optional — rewind to initial state
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;

use super::easing::ease;
use super::mutation::{self, MutationInputContext, MutationValue};
use super::types::*;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Opaque runtime for a single `StateMachine` definition.
#[derive(Debug, Clone)]
pub struct StateMachineRuntime {
    /// The compiled definition (immutable after construction).
    definition: StateMachine,

    /// Lookup: mutation id → index into `definition.mutations`.
    mutation_index: HashMap<String, usize>,

    /// Current active state id.
    current_state_id: String,

    /// Wall-clock time accumulated since scene start (seconds).
    scene_time: f64,

    /// Per-state local elapsed time (seconds).
    /// Each state independently tracks how long it has been "active"
    /// (ticking).  Entry/Any/Exit states stay at 0.
    state_local_times: HashMap<String, f64>,

    /// Active transition (if any).
    active_transition: Option<ActiveTransition>,

    /// Latest runtime input snapshot available to mutations.
    runtime_input: RuntimeInputSnapshot,

    /// Current animation parameter values emitted by the state machine.
    ///
    /// State `parameter_overrides` and mutation outputs are patches: if a state
    /// does not write a key, the last written value stays active until reset or
    /// until another state/mutation writes that key.
    current_overrides: HashMap<OverrideKey, serde_json::Value>,

    /// Whether the state machine has reached the exit state.
    pub finished: bool,
}

/// Bookkeeping for an in-progress transition.
#[derive(Debug, Clone)]
struct ActiveTransition {
    transition_id: String,
    source_state_id: String,
    target_state_id: String,
    /// Explicit delay before blend begins (seconds).
    delay: f64,
    /// Blend duration (seconds).
    duration: f64,
    easing: EasingKind,
    elapsed: f64,
}

/// The result of a single `tick` call.
#[derive(Debug, Clone, Default)]
pub struct TickResult {
    /// Current animation parameter state to apply to the scene.
    /// Keyed by `OverrideKey` (nodeId + paramName).
    pub overrides: HashMap<OverrideKey, serde_json::Value>,

    /// Diagnostics emitted during this tick (non-fatal).
    pub diagnostics: Vec<String>,

    /// Whether the state machine has reached the exit state.
    pub finished: bool,

    /// The id of the current active state (after this tick).
    pub current_state_id: String,

    /// Blend factor if a transition is in progress (0.0 → 1.0).
    pub transition_blend: Option<f64>,

    /// Scene elapsed time in seconds after this tick.
    pub scene_time_secs: f64,

    /// Per-state local elapsed times (state_id → seconds).
    pub state_local_times: BTreeMap<String, f64>,

    /// Active transition id, when transitioning.
    pub active_transition_id: Option<String>,
}

/// External parameter state visible to condition evaluation.
///
/// Maps param ids to current values.
pub type ExternalParams = HashMap<String, serde_json::Value>;

/// Events fired this tick (for event-type conditions).
pub type FiredEvents = Vec<String>;

impl StateMachineRuntime {
    /// Construct a new runtime from a validated `StateMachine` definition.
    ///
    /// Call [`super::validation::validate`] before constructing if you want
    /// fail-fast diagnostics.
    pub fn new(definition: StateMachine) -> Self {
        let mutation_index: HashMap<String, usize> = definition
            .mutations
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.clone(), i))
            .collect();

        let initial = definition
            .initial_state_id
            .clone()
            .or_else(|| {
                definition
                    .states
                    .iter()
                    .find(|s| s.resolved_type() == AnimationStateType::EntryState)
                    .map(|s| s.id.clone())
            })
            .unwrap_or_default();

        // Initialize local times for ALL states to 0.0 so the trace always
        // reports every state (even those that haven't been entered yet).
        let state_local_times: HashMap<String, f64> = definition
            .states
            .iter()
            .map(|s| (s.id.clone(), 0.0))
            .collect();

        Self {
            definition,
            mutation_index,
            current_state_id: initial,
            scene_time: 0.0,
            state_local_times,
            active_transition: None,
            runtime_input: RuntimeInputSnapshot::default(),
            current_overrides: HashMap::new(),
            finished: false,
        }
    }

    /// Reset to initial state.
    pub fn reset(&mut self) {
        let initial = self
            .definition
            .initial_state_id
            .clone()
            .or_else(|| {
                self.definition
                    .states
                    .iter()
                    .find(|s| s.resolved_type() == AnimationStateType::EntryState)
                    .map(|s| s.id.clone())
            })
            .unwrap_or_default();

        self.current_state_id = initial;
        self.scene_time = 0.0;
        // Re-initialize all state local times to 0.0 (same as construction).
        for v in self.state_local_times.values_mut() {
            *v = 0.0;
        }
        self.active_transition = None;
        self.runtime_input = RuntimeInputSnapshot::default();
        self.current_overrides.clear();
        self.finished = false;
    }

    /// Update the latest mouse position visible to mutation nodes.
    pub fn set_mouse_position(&mut self, position: MousePosition) {
        self.runtime_input.mouse_position = Some(position);
    }

    /// Advance the state machine by `dt` seconds and produce overrides.
    pub fn tick(&mut self, dt: f64, params: &ExternalParams, events: &FiredEvents) -> TickResult {
        if self.finished {
            return TickResult {
                overrides: self.current_overrides.clone(),
                finished: true,
                current_state_id: self.current_state_id.clone(),
                scene_time_secs: self.scene_time,
                state_local_times: self.snapshot_local_times(),
                active_transition_id: self
                    .active_transition
                    .as_ref()
                    .map(|at| at.transition_id.clone()),
                ..Default::default()
            };
        }

        self.scene_time += dt;

        let mut diagnostics: Vec<String> = Vec::new();

        // ── Advance active transition ──────────────────────────────────
        if let Some(ref mut at) = self.active_transition {
            at.elapsed += dt;

            // The source state (current_state_id) keeps ticking during
            // the transition (both delay and blend phases).
            let source_id = at.source_state_id.clone();
            if let Some(t) = self.state_local_times.get_mut(&source_id) {
                *t += dt;
            }

            // During the blend phase (past delay), tick the *target* state's
            // local time so its mutation sees advancing localElapsedTime.
            if at.elapsed > at.delay {
                let target_id = at.target_state_id.clone();
                if let Some(t) = self.state_local_times.get_mut(&target_id) {
                    *t += dt;
                }
            }

            let total = at.delay + at.duration;
            if at.elapsed >= total {
                // Transition complete — enter target state.
                // Target state's local time is preserved (accumulated during blend).
                let target = at.target_state_id.clone();
                self.active_transition = None;
                self.current_state_id = target;
            }
        } else {
            // No active transition — tick local time for the current state
            // (all state types except ExitState).
            let should_tick = self
                .find_state(&self.current_state_id)
                .map(|s| s.resolved_type() != AnimationStateType::ExitState)
                .unwrap_or(false);
            if should_tick {
                let id = self.current_state_id.clone();
                if let Some(t) = self.state_local_times.get_mut(&id) {
                    *t += dt;
                }
            }
        }

        // AnyState always ticks, regardless of which state is current or
        // whether a transition is active — unless it's the current state
        // (already ticked above).
        if let Some(any_state) = self
            .definition
            .states
            .iter()
            .find(|s| s.resolved_type() == AnimationStateType::AnyState)
        {
            let any_id = any_state.id.clone();
            let already_ticked = self.current_state_id == any_id
                || self
                    .active_transition
                    .as_ref()
                    .map(|at| at.source_state_id == any_id || at.target_state_id == any_id)
                    .unwrap_or(false);
            if !already_ticked {
                if let Some(t) = self.state_local_times.get_mut(&any_id) {
                    *t += dt;
                }
            }
        }

        // ── Evaluate transition candidates (only if no active transition) ──
        if self.active_transition.is_none() {
            if let Some(transition) = self.pick_transition(params, events) {
                let total = transition.delay + transition.duration;
                if total <= 0.0 {
                    // Instant transition (no delay, no blend).
                    self.enter_state(&transition.target);
                } else {
                    // Reset the target state's local time for the upcoming
                    // blend phase (it will start ticking once delay expires).
                    self.state_local_times
                        .insert(transition.target.clone(), 0.0);
                    self.active_transition = Some(ActiveTransition {
                        transition_id: transition.id.clone(),
                        source_state_id: self.current_state_id.clone(),
                        target_state_id: transition.target.clone(),
                        delay: transition.delay,
                        duration: transition.duration,
                        easing: transition.easing,
                        elapsed: 0.0,
                    });
                }
            }
        }

        // ── Build explicit state patch ─────────────────────────────────
        let source_patch =
            self.evaluate_state_patch(&self.current_state_id, params, &mut diagnostics);

        // 3. If in-transition, compute blend factor.
        //    - During delay phase (elapsed < delay): blend = 0 (source-state output).
        //    - During blend phase: normal eased interpolation 0→1.
        let transition_blend = if let Some(ref at) = self.active_transition {
            if at.elapsed < at.delay {
                // Still in delay window — keep source-state output.
                Some(0.0)
            } else if at.duration > 0.0 {
                let blend_elapsed = at.elapsed - at.delay;
                let raw_t = (blend_elapsed / at.duration).clamp(0.0, 1.0);
                Some(ease(at.easing, raw_t))
            } else {
                // No blend duration — snap to target after delay.
                Some(1.0)
            }
        } else {
            None
        };

        // 4. Blend source→target patches when in the blend phase.
        let patch = if let (Some(at), Some(blend)) = (&self.active_transition, transition_blend) {
            if blend > 0.0 {
                let target_patch =
                    self.evaluate_state_patch(&at.target_state_id, params, &mut diagnostics);
                self.blend_state_patches(&source_patch, &target_patch, blend)
            } else {
                source_patch
            }
        } else {
            source_patch
        };

        for (key, value) in patch {
            self.current_overrides.insert(key, value);
        }

        // Check if current state is exit.
        if let Some(state) = self.find_state(&self.current_state_id) {
            if state.resolved_type() == AnimationStateType::ExitState {
                self.finished = true;
            }
        }

        TickResult {
            overrides: self.current_overrides.clone(),
            diagnostics,
            finished: self.finished,
            current_state_id: self.current_state_id.clone(),
            transition_blend,
            scene_time_secs: self.scene_time,
            state_local_times: self.snapshot_local_times(),
            active_transition_id: self
                .active_transition
                .as_ref()
                .map(|at| at.transition_id.clone()),
        }
    }

    /// Get the current state id.
    pub fn current_state_id(&self) -> &str {
        &self.current_state_id
    }

    /// Get the active transition id, if a transition is currently running.
    pub fn active_transition_id(&self) -> Option<&str> {
        self.active_transition
            .as_ref()
            .map(|at| at.transition_id.as_str())
    }

    /// Get the definition.
    pub fn definition(&self) -> &StateMachine {
        &self.definition
    }

    // ── Internal helpers ───────────────────────────────────────────────

    fn enter_state(&mut self, state_id: &str) {
        self.current_state_id = state_id.to_string();
        // Reset this state's local time on entry (instant transitions).
        self.state_local_times.insert(state_id.to_string(), 0.0);
    }

    /// Snapshot all per-state local times as a sorted BTreeMap for output.
    fn snapshot_local_times(&self) -> BTreeMap<String, f64> {
        self.state_local_times
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    fn find_state(&self, state_id: &str) -> Option<&AnimationState> {
        self.definition.states.iter().find(|s| s.id == state_id)
    }

    fn evaluate_state_patch(
        &self,
        state_id: &str,
        params: &ExternalParams,
        diagnostics: &mut Vec<String>,
    ) -> HashMap<OverrideKey, serde_json::Value> {
        let mut patch = HashMap::new();
        let Some(state) = self.find_state(state_id) else {
            return patch;
        };

        for (key_str, value) in &state.parameter_overrides {
            if let Some(key) = OverrideKey::parse(key_str) {
                patch.insert(key, value.clone());
            }
        }

        if state.resolved_type() == AnimationStateType::MutationNode
            && let Some(ref mid) = state.mutation_id
        {
            match self.evaluate_mutation_state(mid, state_id, params) {
                Ok(mutation_overrides) => {
                    patch.extend(mutation_overrides);
                }
                Err(e) => {
                    diagnostics.push(format!(
                        "mutation evaluation error (state={state_id}, mutation={mid}): {e}"
                    ));
                }
            }
        }

        patch
    }

    fn blend_state_patches(
        &self,
        source_patch: &HashMap<OverrideKey, serde_json::Value>,
        target_patch: &HashMap<OverrideKey, serde_json::Value>,
        blend: f64,
    ) -> HashMap<OverrideKey, serde_json::Value> {
        let all_keys: HashSet<OverrideKey> = source_patch
            .keys()
            .chain(target_patch.keys())
            .cloned()
            .collect();
        let mut patch = HashMap::new();

        for key in all_keys {
            let value = match (source_patch.get(&key), target_patch.get(&key)) {
                (Some(source), Some(target)) => blend_json_values(source, target, blend),
                (Some(source), None) => source.clone(),
                (None, Some(target)) => match self.current_overrides.get(&key) {
                    Some(current) => blend_json_values(current, target, blend),
                    None => target.clone(),
                },
                (None, None) => continue,
            };
            patch.insert(key, value);
        }

        patch
    }

    /// Pick the highest-priority satisfied transition from the current state
    /// plus anyState outgoing transitions.
    fn pick_transition(
        &self,
        params: &ExternalParams,
        events: &FiredEvents,
    ) -> Option<AnimationTransition> {
        let mut candidates: Vec<&AnimationTransition> = Vec::new();

        // Current-state outgoing transitions first.
        for t in &self.definition.transitions {
            if t.source == self.current_state_id {
                candidates.push(t);
            }
        }

        // anyState outgoing transitions.
        let any_state_id = self
            .definition
            .states
            .iter()
            .find(|s| s.resolved_type() == AnimationStateType::AnyState)
            .map(|s| s.id.as_str());
        if let Some(any_id) = any_state_id {
            for t in &self.definition.transitions {
                if t.source == any_id {
                    candidates.push(t);
                }
            }
        }

        // Evaluate in deterministic order (scene order preserved).
        // For each candidate: check trigger first (every frame), then condition.
        // Both must pass for the transition to fire.
        for t in &candidates {
            // 1. Check trigger — if trigger is None, it's always triggered.
            if !self.evaluate_condition(t.trigger.as_ref(), params, events) {
                continue;
            }
            // 2. Trigger passed — now check condition guard.
            if !self.evaluate_condition(t.condition.as_ref(), params, events) {
                continue;
            }
            // Both passed — fire this transition.
            return Some((*t).clone());
        }

        None
    }

    fn evaluate_condition(
        &self,
        condition: Option<&TransitionCondition>,
        params: &ExternalParams,
        events: &FiredEvents,
    ) -> bool {
        let Some(cond) = condition else {
            // No condition → always satisfied (unconditional transition).
            return true;
        };

        match cond {
            TransitionCondition::Trigger { param_id } => {
                // Trigger is satisfied if the param is truthy.
                params
                    .get(param_id)
                    .map(|v| json_is_truthy(v))
                    .unwrap_or(false)
            }
            TransitionCondition::Bool { param_id, value } => {
                let expected = value.unwrap_or(true);
                params
                    .get(param_id)
                    .and_then(|v| v.as_bool())
                    .map(|v| v == expected)
                    .unwrap_or(false)
            }
            TransitionCondition::Threshold { param_id, value } => params
                .get(param_id)
                .and_then(|v| v.as_f64())
                .map(|v| v >= *value)
                .unwrap_or(false),
            TransitionCondition::Event { event_name } => events.contains(event_name),
            TransitionCondition::Compound { op, conditions } => match op {
                CompoundOp::And => conditions
                    .iter()
                    .all(|c| self.evaluate_condition(Some(c), params, events)),
                CompoundOp::Or => conditions
                    .iter()
                    .any(|c| self.evaluate_condition(Some(c), params, events)),
            },
        }
    }

    fn evaluate_mutation_state(
        &self,
        mutation_id: &str,
        state_id: &str,
        params: &ExternalParams,
    ) -> Result<HashMap<OverrideKey, serde_json::Value>> {
        let mutation_idx = self
            .mutation_index
            .get(mutation_id)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("mutation '{mutation_id}' not found"))?;
        let mutation = &self.definition.mutations[mutation_idx];

        // Build input context.
        let mut input_values: HashMap<String, MutationValue> = HashMap::new();
        for input_port in &mutation.inputs {
            // Try to resolve from external params via port name.
            if let Some(ref name) = input_port.name {
                if let Some(val) = params.get(name).and_then(|v| v.as_f64()) {
                    input_values.insert(input_port.id.clone(), val);
                }
            }
        }
        let ctx = MutationInputContext {
            values: input_values,
            scene_elapsed_time: self.scene_time,
            local_elapsed_time: self.state_local_times.get(state_id).copied().unwrap_or(0.0),
            mouse_position: self.runtime_input.mouse_position,
        };

        let outputs = mutation::evaluate_mutation(mutation, &ctx)?;

        // Map output port ids → OverrideKeys via unified target resolution.
        let mut overrides: HashMap<OverrideKey, serde_json::Value> = HashMap::new();

        // From output bindings.
        for b in &mutation.output_bindings {
            if let Some(&val) = outputs.get(&b.port_id) {
                if let Some(key) = mutation::resolve_output_target(&b.port_id) {
                    overrides.insert(key, serde_json::json!(val));
                }
            }
        }

        // From passthrough bindings (evaluate_mutation already placed these
        // in the outputs map keyed by to_port_id).
        for pt in &mutation.passthrough_bindings {
            if let Some(&val) = outputs.get(&pt.to_port_id) {
                if let Some(key) = mutation::resolve_output_target(&pt.to_port_id) {
                    overrides
                        .entry(key)
                        .or_insert_with(|| serde_json::json!(val));
                }
            }
        }

        Ok(overrides)
    }
}

/// Linearly interpolate two JSON values by blend factor `t` (0→1).
/// Falls back to the target value for non-numeric types.
fn blend_json_values(
    source: &serde_json::Value,
    target: &serde_json::Value,
    t: f64,
) -> serde_json::Value {
    match (source, target) {
        (serde_json::Value::Number(sn), serde_json::Value::Number(gn)) => {
            let sv = sn.as_f64().unwrap_or(0.0);
            let gv = gn.as_f64().unwrap_or(0.0);
            let blended = sv + (gv - sv) * t;
            serde_json::json!(blended)
        }
        (serde_json::Value::Array(sa), serde_json::Value::Array(ga)) => {
            let len = sa.len().max(ga.len());
            let mut out = Vec::with_capacity(len);
            for i in 0..len {
                let sv = sa.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0);
                let gv = ga.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0);
                out.push(serde_json::json!(sv + (gv - sv) * t));
            }
            serde_json::Value::Array(out)
        }
        _ => {
            // Non-numeric: snap to target when blend > 0.5.
            if t >= 0.5 {
                target.clone()
            } else {
                source.clone()
            }
        }
    }
}

fn json_is_truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Null => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_sm() -> StateMachine {
        StateMachine {
            id: "sm1".into(),
            name: "Test".into(),
            states: vec![
                AnimationState {
                    id: "entry".into(),
                    name: "Entry".into(),
                    position: None,
                    parameter_overrides: Default::default(),
                    state_type: Some(AnimationStateType::EntryState),
                    mutation_id: None,
                },
                AnimationState {
                    id: "any".into(),
                    name: "Any".into(),
                    position: None,
                    parameter_overrides: Default::default(),
                    state_type: Some(AnimationStateType::AnyState),
                    mutation_id: None,
                },
                AnimationState {
                    id: "exit".into(),
                    name: "Exit".into(),
                    position: None,
                    parameter_overrides: Default::default(),
                    state_type: Some(AnimationStateType::ExitState),
                    mutation_id: None,
                },
            ],
            transitions: vec![],
            mutations: vec![],
            initial_state_id: Some("entry".into()),
            viewport: None,
        }
    }

    #[test]
    fn starts_at_initial_state() {
        let rt = StateMachineRuntime::new(minimal_sm());
        assert_eq!(rt.current_state_id(), "entry");
        assert!(!rt.finished);
    }

    #[test]
    fn unconditional_instant_transition() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            parameter_overrides: [("Node1:color".into(), serde_json::json!([1, 0, 0, 1]))]
                .into_iter()
                .collect(),
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
            trigger: None,
            condition: None,
            delay: 0.0,
            duration: 0.0,
            easing: EasingKind::Linear,
        });

        let mut rt = StateMachineRuntime::new(sm);
        let result = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(result.current_state_id, "s1");
        assert!(
            result
                .overrides
                .contains_key(&OverrideKey::new("Node1", "color"))
        );
    }

    #[test]
    fn missing_override_keeps_previous_value_after_instant_transition() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "a".into(),
            name: "A".into(),
            position: None,
            parameter_overrides: [("Node:x".into(), serde_json::json!(5.0))]
                .into_iter()
                .collect(),
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        sm.states.push(AnimationState {
            id: "b".into(),
            name: "B".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "entry_to_a".into(),
            source: "entry".into(),
            target: "a".into(),
            trigger: None,
            condition: None,
            delay: 0.0,
            duration: 0.0,
            easing: EasingKind::Linear,
        });
        sm.transitions.push(AnimationTransition {
            id: "a_to_b".into(),
            source: "a".into(),
            target: "b".into(),
            trigger: Some(TransitionCondition::Event {
                event_name: "go".into(),
            }),
            condition: None,
            delay: 0.0,
            duration: 0.0,
            easing: EasingKind::Linear,
        });

        let mut rt = StateMachineRuntime::new(sm);
        let a = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(a.current_state_id, "a");
        assert_eq!(
            a.overrides.get(&OverrideKey::new("Node", "x")),
            Some(&serde_json::json!(5.0))
        );

        let b = rt.tick(0.016, &HashMap::new(), &vec!["go".into()]);
        assert_eq!(b.current_state_id, "b");
        assert_eq!(
            b.overrides.get(&OverrideKey::new("Node", "x")),
            Some(&serde_json::json!(5.0))
        );
    }

    #[test]
    fn timed_transition_source_only_key_does_not_blend_to_zero() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "a".into(),
            name: "A".into(),
            position: None,
            parameter_overrides: [("Node:x".into(), serde_json::json!(5.0))]
                .into_iter()
                .collect(),
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        sm.states.push(AnimationState {
            id: "b".into(),
            name: "B".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "entry_to_a".into(),
            source: "entry".into(),
            target: "a".into(),
            trigger: None,
            condition: None,
            delay: 0.0,
            duration: 0.0,
            easing: EasingKind::Linear,
        });
        sm.transitions.push(AnimationTransition {
            id: "a_to_b".into(),
            source: "a".into(),
            target: "b".into(),
            trigger: Some(TransitionCondition::Event {
                event_name: "go".into(),
            }),
            condition: None,
            delay: 0.0,
            duration: 1.0,
            easing: EasingKind::Linear,
        });

        let mut rt = StateMachineRuntime::new(sm);
        let a = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(a.current_state_id, "a");

        let triggered = rt.tick(0.016, &HashMap::new(), &vec!["go".into()]);
        assert_eq!(triggered.current_state_id, "a");
        assert_eq!(
            triggered.overrides.get(&OverrideKey::new("Node", "x")),
            Some(&serde_json::json!(5.0))
        );

        let blending = rt.tick(0.5, &HashMap::new(), &vec![]);
        assert_eq!(blending.current_state_id, "a");
        assert_eq!(
            blending.overrides.get(&OverrideKey::new("Node", "x")),
            Some(&serde_json::json!(5.0))
        );

        let completed = rt.tick(0.6, &HashMap::new(), &vec![]);
        assert_eq!(completed.current_state_id, "b");
        assert_eq!(
            completed.overrides.get(&OverrideKey::new("Node", "x")),
            Some(&serde_json::json!(5.0))
        );
    }

    #[test]
    fn timed_transition_blends() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
            trigger: None,
            condition: None,
            delay: 0.0,
            duration: 1.0,
            easing: EasingKind::Linear,
        });

        let mut rt = StateMachineRuntime::new(sm);
        let r1 = rt.tick(0.5, &HashMap::new(), &vec![]);
        // Should be mid-transition, not yet at s1.
        assert_eq!(r1.current_state_id, "entry");
        assert!(r1.transition_blend.is_some());

        let r2 = rt.tick(1.1, &HashMap::new(), &vec![]);
        // Transition complete.
        assert_eq!(r2.current_state_id, "s1");
        assert!(r2.transition_blend.is_none());
    }

    #[test]
    fn bool_condition() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
            trigger: None,
            condition: Some(TransitionCondition::Bool {
                param_id: "flag".into(),
                value: Some(true),
            }),
            delay: 0.0,
            duration: 0.0,
            easing: EasingKind::Linear,
        });

        let mut rt = StateMachineRuntime::new(sm);

        // Without param → no transition.
        let r1 = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(r1.current_state_id, "entry");

        // With param → transition.
        let mut p = HashMap::new();
        p.insert("flag".into(), serde_json::json!(true));
        let r2 = rt.tick(0.016, &p, &vec![]);
        assert_eq!(r2.current_state_id, "s1");
    }

    #[test]
    fn event_condition() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
            trigger: None,
            condition: Some(TransitionCondition::Event {
                event_name: "click".into(),
            }),
            delay: 0.0,
            duration: 0.0,
            easing: EasingKind::Linear,
        });

        let mut rt = StateMachineRuntime::new(sm);

        // No event → stays.
        let r1 = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(r1.current_state_id, "entry");

        // Fire event → transitions.
        let r2 = rt.tick(0.016, &HashMap::new(), &vec!["click".into()]);
        assert_eq!(r2.current_state_id, "s1");
    }

    #[test]
    fn any_state_event_transition_can_start_from_entry_state() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "mutation".into(),
            name: "Mutation".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::MutationNode),
            mutation_id: Some("m1".into()),
        });
        sm.mutations.push(MutationDefinition {
            id: "m1".into(),
            name: "Mutation".into(),
            inputs: vec![],
            outputs: vec![],
            nodes: vec![],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![],
            passthrough_bindings: vec![],
            viewport: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "tr_any_mutation".into(),
            source: "any".into(),
            target: "mutation".into(),
            trigger: Some(TransitionCondition::Event {
                event_name: "mousedown".into(),
            }),
            condition: None,
            delay: 0.0,
            duration: 0.3,
            easing: EasingKind::Linear,
        });

        let mut rt = StateMachineRuntime::new(sm);

        let idle = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(idle.current_state_id, "entry");
        assert_eq!(idle.active_transition_id, None);

        let triggered = rt.tick(0.016, &HashMap::new(), &vec!["mousedown".into()]);
        assert_eq!(triggered.current_state_id, "entry");
        assert_eq!(
            triggered.active_transition_id.as_deref(),
            Some("tr_any_mutation")
        );

        let completed = rt.tick(0.4, &HashMap::new(), &vec![]);
        assert_eq!(completed.current_state_id, "mutation");
    }

    #[test]
    fn event_transition_mutation_reads_same_tick_mouse_position() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "mutation".into(),
            name: "Mutation".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::MutationNode),
            mutation_id: Some("m_mouse".into()),
        });
        sm.mutations.push(MutationDefinition {
            id: "m_mouse".into(),
            name: "Mouse Mutation".into(),
            inputs: vec![],
            outputs: vec![
                MutationPort {
                    id: "MouseX:value".into(),
                    name: Some("MouseX.value".into()),
                    port_type: Some("float".into()),
                },
                MutationPort {
                    id: "MouseY:value".into(),
                    name: Some("MouseY.value".into()),
                    port_type: Some("float".into()),
                },
            ],
            nodes: vec![],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![],
            passthrough_bindings: vec![
                MutationPassthroughBinding {
                    from_port_id: "mouse.position.x".into(),
                    to_port_id: "MouseX:value".into(),
                },
                MutationPassthroughBinding {
                    from_port_id: "mouse.position.y".into(),
                    to_port_id: "MouseY:value".into(),
                },
            ],
            viewport: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "entry_to_mouse".into(),
            source: "entry".into(),
            target: "mutation".into(),
            trigger: Some(TransitionCondition::Event {
                event_name: "mousedown".into(),
            }),
            condition: None,
            delay: 0.0,
            duration: 0.0,
            easing: EasingKind::Linear,
        });

        let mut rt = StateMachineRuntime::new(sm);
        rt.set_mouse_position(MousePosition { x: 321.0, y: 654.0 });

        let result = rt.tick(0.016, &HashMap::new(), &vec!["mousedown".into()]);

        assert_eq!(result.current_state_id, "mutation");
        assert_eq!(
            result.overrides.get(&OverrideKey::new("MouseX", "value")),
            Some(&serde_json::json!(321.0))
        );
        assert_eq!(
            result.overrides.get(&OverrideKey::new("MouseY", "value")),
            Some(&serde_json::json!(654.0))
        );
    }

    #[test]
    fn exit_state_marks_finished() {
        let mut sm = minimal_sm();
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "exit".into(),
            trigger: None,
            condition: None,
            delay: 0.0,
            duration: 0.0,
            easing: EasingKind::Linear,
        });

        let mut rt = StateMachineRuntime::new(sm);
        let r = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(r.current_state_id, "exit");
        assert!(r.finished);
    }

    #[test]
    fn reset_returns_to_initial() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
            trigger: None,
            condition: None,
            delay: 0.0,
            duration: 0.0,
            easing: EasingKind::Linear,
        });

        let mut rt = StateMachineRuntime::new(sm);
        rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(rt.current_state_id(), "s1");

        rt.reset();
        assert_eq!(rt.current_state_id(), "entry");
        assert!(!rt.finished);
    }

    #[test]
    fn trigger_and_condition_both_required() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        // Trigger: event "go", Condition: bool "ready" == true.
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
            trigger: Some(TransitionCondition::Event {
                event_name: "go".into(),
            }),
            condition: Some(TransitionCondition::Bool {
                param_id: "ready".into(),
                value: Some(true),
            }),
            delay: 0.0,
            duration: 0.0,
            easing: EasingKind::Linear,
        });

        let mut rt = StateMachineRuntime::new(sm);

        // Neither trigger nor condition → stays.
        let r1 = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(r1.current_state_id, "entry");

        // Trigger fires but condition not met → stays.
        let r2 = rt.tick(0.016, &HashMap::new(), &vec!["go".into()]);
        assert_eq!(r2.current_state_id, "entry");

        // Condition met but trigger not fired → stays.
        let mut p = HashMap::new();
        p.insert("ready".into(), serde_json::json!(true));
        let r3 = rt.tick(0.016, &p, &vec![]);
        assert_eq!(r3.current_state_id, "entry");

        // Both trigger and condition → transitions.
        let r4 = rt.tick(0.016, &p, &vec!["go".into()]);
        assert_eq!(r4.current_state_id, "s1");
    }
}
