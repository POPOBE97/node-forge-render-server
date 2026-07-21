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

use super::motion::{MotionChannelDebug, MotionEngine};
use super::mutation::{self, MutationInputContext, MutationRuntimeState, MutationValue};
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

    /// Lookup: transition motion graph id → index into `definition.motion_graphs`.
    motion_graph_index: HashMap<String, usize>,

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

    /// Per-property physical/timeline presentation drivers.
    motion_engine: MotionEngine,

    /// Latest runtime input snapshot available to mutations.
    runtime_input: RuntimeInputSnapshot,

    /// Persistent runtime state for mutation nodes, keyed by state id.
    mutation_runtime_states: HashMap<String, MutationRuntimeState>,

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
    source_state_id: String,
    target_state_id: String,
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

    /// Scene elapsed time in seconds after this tick.
    pub scene_time_secs: f64,

    /// Per-state local elapsed times (state_id → seconds).
    pub state_local_times: BTreeMap<String, f64>,

    /// Active transition id, when transitioning.
    pub active_transition_id: Option<String>,

    /// Per-property driver diagnostics for the debug timeline.
    pub motion_channels: Vec<MotionChannelDebug>,
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
        let motion_graph_index = definition
            .motion_graphs
            .iter()
            .enumerate()
            .map(|(index, graph)| (graph.id.clone(), index))
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
            motion_graph_index,
            current_state_id: initial,
            scene_time: 0.0,
            state_local_times,
            active_transition: None,
            motion_engine: MotionEngine::new(),
            runtime_input: RuntimeInputSnapshot::default(),
            mutation_runtime_states: HashMap::new(),
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
        self.motion_engine.reset();
        self.runtime_input = RuntimeInputSnapshot::default();
        self.mutation_runtime_states.clear();
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
                    .motion_engine
                    .active_transition_id()
                    .map(str::to_string),
                ..Default::default()
            };
        }

        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.scene_time += dt;
        let prev_state_local_times = self.state_local_times.clone();
        let mut diagnostics: Vec<String> = Vec::new();

        // Logical state changes immediately on transition fire. Presentation
        // drivers retain the visual source independently, so both source and
        // target local clocks may still advance during a handoff.
        let current_id = self.current_state_id.clone();
        if self
            .find_state(&current_id)
            .is_some_and(|state| state.resolved_type() != AnimationStateType::ExitState)
            && let Some(time) = self.state_local_times.get_mut(&current_id)
        {
            *time += dt;
        }
        if let Some(source_id) = self
            .active_transition
            .as_ref()
            .map(|transition| transition.source_state_id.clone())
            && source_id != current_id
            && let Some(time) = self.state_local_times.get_mut(&source_id)
        {
            *time += dt;
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
            let already_ticked = self.current_state_id == any_id;
            if !already_ticked {
                if let Some(t) = self.state_local_times.get_mut(&any_id) {
                    *t += dt;
                }
            }
        }

        // Transitions remain interruptible while a previous visual driver is
        // active. Routing uses the logical current state (already the target
        // of the previous transition).
        if let Some(transition) = self.pick_transition(params, events) {
            let source_state_id = self.current_state_id.clone();
            let source_advanced =
                self.state_advanced_this_tick(&prev_state_local_times, &source_state_id);
            let source_patch = self.evaluate_state_patch(
                &source_state_id,
                params,
                &mut diagnostics,
                source_advanced,
            );

            self.state_local_times
                .insert(transition.target.clone(), 0.0);
            self.mutation_runtime_states.remove(&transition.target);
            let target_patch =
                self.evaluate_state_patch(&transition.target, params, &mut diagnostics, false);
            let graph = self
                .motion_graph_index
                .get(&transition.motion_graph_id)
                .and_then(|index| self.definition.motion_graphs.get(*index))
                .cloned();
            if let Some(graph) = graph {
                self.motion_engine.start_transition(
                    &transition.id,
                    &graph,
                    &source_patch,
                    &target_patch,
                    &self.current_overrides,
                );
                self.active_transition = Some(ActiveTransition {
                    source_state_id,
                    target_state_id: transition.target.clone(),
                });
            }
            self.current_state_id = transition.target;
        }

        let target_state_id = self.current_state_id.clone();
        let target_advanced =
            self.state_advanced_this_tick(&prev_state_local_times, &target_state_id);
        let target_patch =
            self.evaluate_state_patch(&target_state_id, params, &mut diagnostics, target_advanced);
        let source_patch = if let Some(source_id) = self
            .active_transition
            .as_ref()
            .map(|transition| transition.source_state_id.clone())
        {
            let advanced = self.state_advanced_this_tick(&prev_state_local_times, &source_id);
            self.evaluate_state_patch(&source_id, params, &mut diagnostics, advanced)
        } else {
            target_patch.clone()
        };
        self.motion_engine
            .update_endpoints(&source_patch, &target_patch);
        let motion_step = self.motion_engine.step(dt);
        let patch = if self.motion_engine.active_transition_id().is_some() {
            motion_step.overrides.clone()
        } else {
            self.active_transition = None;
            target_patch
        };

        for (key, value) in patch {
            self.current_overrides.insert(key, value);
        }

        // Exit becomes terminal only after its visual transition completes.
        if let Some(state) = self.find_state(&self.current_state_id) {
            if state.resolved_type() == AnimationStateType::ExitState
                && self.motion_engine.active_transition_id().is_none()
            {
                self.finished = true;
            }
        }

        self.prune_mutation_runtime_states();

        TickResult {
            overrides: self.current_overrides.clone(),
            diagnostics,
            finished: self.finished,
            current_state_id: self.current_state_id.clone(),
            scene_time_secs: self.scene_time,
            state_local_times: self.snapshot_local_times(),
            active_transition_id: self
                .motion_engine
                .active_transition_id()
                .map(str::to_string),
            motion_channels: motion_step.channels,
        }
    }

    /// Get the current state id.
    pub fn current_state_id(&self) -> &str {
        &self.current_state_id
    }

    /// Get the active transition id, if a transition is currently running.
    pub fn active_transition_id(&self) -> Option<&str> {
        self.motion_engine.active_transition_id()
    }

    /// Get the definition.
    pub fn definition(&self) -> &StateMachine {
        &self.definition
    }

    // ── Internal helpers ───────────────────────────────────────────────

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
        &mut self,
        state_id: &str,
        params: &ExternalParams,
        diagnostics: &mut Vec<String>,
        advance_frame: bool,
    ) -> HashMap<OverrideKey, serde_json::Value> {
        let mut patch = HashMap::new();
        let Some(state) = self.find_state(state_id) else {
            return patch;
        };
        let parameter_overrides = state.parameter_overrides.clone();
        let state_type = state.resolved_type();
        let mutation_id = state.mutation_id.clone();

        for (key_str, value) in &parameter_overrides {
            if let Some(key) = OverrideKey::parse(key_str) {
                patch.insert(key, value.clone());
            }
        }

        if state_type == AnimationStateType::MutationNode
            && let Some(ref mid) = mutation_id
        {
            match self.evaluate_mutation_state(mid, state_id, params, advance_frame) {
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

    fn state_advanced_this_tick(
        &self,
        prev_state_local_times: &HashMap<String, f64>,
        state_id: &str,
    ) -> bool {
        let previous = prev_state_local_times.get(state_id).copied().unwrap_or(0.0);
        let current = self.state_local_times.get(state_id).copied().unwrap_or(0.0);
        current > previous
    }

    fn prune_mutation_runtime_states(&mut self) {
        let mut keep = HashSet::new();
        if self
            .find_state(&self.current_state_id)
            .is_some_and(|state| state.resolved_type() == AnimationStateType::MutationNode)
        {
            keep.insert(self.current_state_id.clone());
        }

        if let Some(active_transition) = &self.active_transition {
            for state_id in [
                &active_transition.source_state_id,
                &active_transition.target_state_id,
            ] {
                if self
                    .find_state(state_id)
                    .is_some_and(|state| state.resolved_type() == AnimationStateType::MutationNode)
                {
                    keep.insert(state_id.clone());
                }
            }
        }

        self.mutation_runtime_states
            .retain(|state_id, _| keep.contains(state_id));
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
        &mut self,
        mutation_id: &str,
        state_id: &str,
        params: &ExternalParams,
        advance_frame: bool,
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
                    input_values.insert(input_port.id.clone(), val.into());
                }
            }
        }
        let ctx = MutationInputContext {
            values: input_values,
            scene_elapsed_time: self.scene_time,
            local_elapsed_time: self.state_local_times.get(state_id).copied().unwrap_or(0.0),
            mouse_position: self.runtime_input.mouse_position,
            advance_frame,
        };

        let runtime_state = self
            .mutation_runtime_states
            .entry(state_id.to_string())
            .or_default();
        let outputs = mutation::evaluate_mutation_with_state(mutation, &ctx, runtime_state)?;

        // Map output port ids → OverrideKeys via unified target resolution.
        let mut overrides: HashMap<OverrideKey, serde_json::Value> = HashMap::new();

        // From output bindings.
        for b in &mutation.output_bindings {
            if let Some(val) = outputs.get(&b.port_id) {
                for (key, json_value) in mutation::expand_output_overrides(&b.port_id, val) {
                    overrides.insert(key, json_value);
                }
            }
        }

        // From passthrough bindings (evaluate_mutation already placed these
        // in the outputs map keyed by to_port_id).
        for pt in &mutation.passthrough_bindings {
            if let Some(val) = outputs.get(&pt.to_port_id) {
                for (key, json_value) in mutation::expand_output_overrides(&pt.to_port_id, val) {
                    overrides.entry(key).or_insert(json_value);
                }
            }
        }

        Ok(overrides)
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

    fn intelligent_light_driver_mutation(id: &str) -> MutationDefinition {
        MutationDefinition {
            id: id.into(),
            name: "Intelligent Light Driver".into(),
            inputs: vec![],
            outputs: vec![
                MutationPort {
                    id: "Light:positions".into(),
                    name: Some("Light.positions".into()),
                    port_type: Some("packed<vector2>".into()),
                },
                MutationPort {
                    id: "Light:colors".into(),
                    name: Some("Light.colors".into()),
                    port_type: Some("packed<color>".into()),
                },
            ],
            nodes: vec![MutationInnerNode {
                id: "driver".into(),
                node_type: MutationInnerNodeType::IntelligentLightDefaultDriver,
                params: HashMap::new(),
                inputs: vec![],
                outputs: vec![
                    MutationPort {
                        id: "positions".into(),
                        name: Some("Positions".into()),
                        port_type: Some("packed<vector2>".into()),
                    },
                    MutationPort {
                        id: "colors".into(),
                        name: Some("Colors".into()),
                        port_type: Some("packed<color>".into()),
                    },
                ],
            }],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![
                MutationOutputBinding {
                    port_id: "Light:positions".into(),
                    from: MutationEndpoint {
                        node_id: "driver".into(),
                        port_id: "positions".into(),
                    },
                },
                MutationOutputBinding {
                    port_id: "Light:colors".into(),
                    from: MutationEndpoint {
                        node_id: "driver".into(),
                        port_id: "colors".into(),
                    },
                },
            ],
            passthrough_bindings: vec![],
            viewport: None,
        }
    }

    fn override_vec2(result: &TickResult, node_id: &str, param_name: &str) -> [f64; 2] {
        let value = result
            .overrides
            .get(&OverrideKey::new(node_id, param_name))
            .unwrap_or_else(|| panic!("missing override for {node_id}:{param_name}"));
        let array = value
            .as_array()
            .unwrap_or_else(|| panic!("expected array override for {node_id}:{param_name}"));
        [
            array[0]
                .as_f64()
                .unwrap_or_else(|| panic!("expected numeric x for {node_id}:{param_name}")),
            array[1]
                .as_f64()
                .unwrap_or_else(|| panic!("expected numeric y for {node_id}:{param_name}")),
        ]
    }

    fn override_color(result: &TickResult, node_id: &str, param_name: &str) -> [f64; 4] {
        let value = result
            .overrides
            .get(&OverrideKey::new(node_id, param_name))
            .unwrap_or_else(|| panic!("missing override for {node_id}:{param_name}"));
        let array = value
            .as_array()
            .unwrap_or_else(|| panic!("expected array override for {node_id}:{param_name}"));
        [
            array[0]
                .as_f64()
                .unwrap_or_else(|| panic!("expected numeric r for {node_id}:{param_name}")),
            array[1]
                .as_f64()
                .unwrap_or_else(|| panic!("expected numeric g for {node_id}:{param_name}")),
            array[2]
                .as_f64()
                .unwrap_or_else(|| panic!("expected numeric b for {node_id}:{param_name}")),
            array[3]
                .as_f64()
                .unwrap_or_else(|| panic!("expected numeric a for {node_id}:{param_name}")),
        ]
    }

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
            motion_graphs: vec![
                instant_motion_graph("instant"),
                timeline_motion_graph("timeline", 0.3),
                timeline_motion_graph("timeline-1", 1.0),
            ],
            initial_state_id: Some("entry".into()),
            viewport: None,
        }
    }

    fn motion_ports() -> (Vec<MutationPort>, Vec<MutationPort>) {
        let port = MutationPort {
            id: "*".into(),
            name: Some("Any".into()),
            port_type: Some("any".into()),
        };
        (vec![port.clone()], vec![port])
    }

    fn instant_motion_graph(id: &str) -> TransitionMotionGraph {
        let (inputs, outputs) = motion_ports();
        TransitionMotionGraph {
            id: id.into(),
            name: "Instant".into(),
            inputs,
            outputs,
            nodes: vec![TransitionMotionNode::Instant {
                id: "motion".into(),
                position: Position::default(),
                label: None,
            }],
            connections: vec![],
            input_bindings: vec![TransitionMotionInputBinding {
                port_id: "*".into(),
                to: MutationEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
            }],
            output_bindings: vec![TransitionMotionOutputBinding {
                port_id: "*".into(),
                from: MutationEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
            }],
            passthrough_bindings: vec![],
            viewport: None,
        }
    }

    fn timeline_motion_graph(id: &str, duration: f64) -> TransitionMotionGraph {
        let mut graph = instant_motion_graph(id);
        graph.name = "Timeline".into();
        graph.nodes = vec![TransitionMotionNode::Linear {
            timeline: TimelineMotionNode {
                id: "motion".into(),
                position: Position::default(),
                label: None,
                duration,
                delay: 0.0,
                blending: None,
            },
        }];
        graph
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
            motion_graph_id: "instant".into(),
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
            motion_graph_id: "instant".into(),
        });
        sm.transitions.push(AnimationTransition {
            id: "a_to_b".into(),
            source: "a".into(),
            target: "b".into(),
            trigger: Some(TransitionCondition::Event {
                event_name: "go".into(),
            }),
            condition: None,
            motion_graph_id: "instant".into(),
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
            motion_graph_id: "instant".into(),
        });
        sm.transitions.push(AnimationTransition {
            id: "a_to_b".into(),
            source: "a".into(),
            target: "b".into(),
            trigger: Some(TransitionCondition::Event {
                event_name: "go".into(),
            }),
            condition: None,
            motion_graph_id: "timeline-1".into(),
        });

        let mut rt = StateMachineRuntime::new(sm);
        let a = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(a.current_state_id, "a");

        let triggered = rt.tick(0.016, &HashMap::new(), &vec!["go".into()]);
        assert_eq!(triggered.current_state_id, "b");
        assert_eq!(
            triggered.overrides.get(&OverrideKey::new("Node", "x")),
            Some(&serde_json::json!(5.0))
        );

        let blending = rt.tick(0.5, &HashMap::new(), &vec![]);
        assert_eq!(blending.current_state_id, "b");
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
    fn timed_transition_advances() {
        let mut sm = minimal_sm();
        sm.states
            .iter_mut()
            .find(|state| state.id == "entry")
            .unwrap()
            .parameter_overrides
            .insert("Node:x".into(), serde_json::json!(0.0));
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            parameter_overrides: [("Node:x".into(), serde_json::json!(1.0))]
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
            motion_graph_id: "timeline-1".into(),
        });

        let mut rt = StateMachineRuntime::new(sm);
        let r1 = rt.tick(0.5, &HashMap::new(), &vec![]);
        // Routing switches immediately while presentation remains mid-motion.
        assert_eq!(r1.current_state_id, "s1");
        assert_eq!(
            r1.overrides.get(&OverrideKey::new("Node", "x")),
            Some(&serde_json::json!(0.5))
        );

        let r2 = rt.tick(1.1, &HashMap::new(), &vec![]);
        // Transition complete.
        assert_eq!(r2.current_state_id, "s1");
        assert_eq!(r2.active_transition_id, None);
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
            motion_graph_id: "instant".into(),
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
            motion_graph_id: "instant".into(),
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
            motion_graph_id: "timeline".into(),
        });

        let mut rt = StateMachineRuntime::new(sm);

        let idle = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(idle.current_state_id, "entry");
        assert_eq!(idle.active_transition_id, None);

        let triggered = rt.tick(0.016, &HashMap::new(), &vec!["mousedown".into()]);
        assert_eq!(triggered.current_state_id, "mutation");
        assert_eq!(triggered.active_transition_id, None);

        let completed = rt.tick(0.4, &HashMap::new(), &vec![]);
        assert_eq!(completed.current_state_id, "mutation");
    }

    #[test]
    fn timeline_tracks_a_dynamic_mutation_target_local_time() {
        let mut sm = minimal_sm();
        sm.states
            .iter_mut()
            .find(|state| state.id == "entry")
            .unwrap()
            .parameter_overrides
            .insert("Node:x".into(), serde_json::json!(0.0));
        sm.states.push(AnimationState {
            id: "dynamic".into(),
            name: "Dynamic".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::MutationNode),
            mutation_id: Some("dynamic_target".into()),
        });
        sm.mutations.push(MutationDefinition {
            id: "dynamic_target".into(),
            name: "Dynamic Target".into(),
            inputs: vec![MutationPort {
                id: "localElapsedTime".into(),
                name: Some("Local Elapsed Time".into()),
                port_type: Some("float".into()),
            }],
            outputs: vec![MutationPort {
                id: "Node:x".into(),
                name: Some("Node.x".into()),
                port_type: Some("float".into()),
            }],
            nodes: vec![],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![],
            passthrough_bindings: vec![MutationPassthroughBinding {
                from_port_id: "localElapsedTime".into(),
                to_port_id: "Node:x".into(),
            }],
            viewport: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "entry_to_dynamic".into(),
            source: "entry".into(),
            target: "dynamic".into(),
            trigger: None,
            condition: None,
            motion_graph_id: "timeline".into(),
        });

        let mut runtime = StateMachineRuntime::new(sm);
        let entered = runtime.tick(0.1, &HashMap::new(), &vec![]);
        assert_eq!(entered.current_state_id, "dynamic");
        assert_eq!(
            entered.overrides.get(&OverrideKey::new("Node", "x")),
            Some(&serde_json::json!(0.0))
        );

        let advancing = runtime.tick(0.1, &HashMap::new(), &vec![]);
        let value = advancing
            .overrides
            .get(&OverrideKey::new("Node", "x"))
            .and_then(serde_json::Value::as_f64)
            .expect("dynamic Timeline output");
        assert!((value - (0.1 * 2.0 / 3.0)).abs() < 1e-8, "value={value}");
        assert_eq!(advancing.state_local_times.get("dynamic"), Some(&0.1));
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
            motion_graph_id: "instant".into(),
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
    fn intelligent_light_driver_restarts_after_leaving_and_reentering_state() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "mutation".into(),
            name: "Mutation".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::MutationNode),
            mutation_id: Some("m_light".into()),
        });
        sm.states.push(AnimationState {
            id: "idle".into(),
            name: "Idle".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        sm.mutations
            .push(intelligent_light_driver_mutation("m_light"));
        sm.transitions.push(AnimationTransition {
            id: "entry_to_mutation".into(),
            source: "entry".into(),
            target: "mutation".into(),
            trigger: None,
            condition: None,
            motion_graph_id: "instant".into(),
        });
        sm.transitions.push(AnimationTransition {
            id: "mutation_to_idle".into(),
            source: "mutation".into(),
            target: "idle".into(),
            trigger: Some(TransitionCondition::Event {
                event_name: "pause".into(),
            }),
            condition: None,
            motion_graph_id: "instant".into(),
        });
        sm.transitions.push(AnimationTransition {
            id: "idle_to_mutation".into(),
            source: "idle".into(),
            target: "mutation".into(),
            trigger: Some(TransitionCondition::Event {
                event_name: "resume".into(),
            }),
            condition: None,
            motion_graph_id: "instant".into(),
        });

        let mut rt = StateMachineRuntime::new(sm);

        let entered = rt.tick(0.016, &HashMap::new(), &vec![]);
        let initial_pos = override_vec2(&entered, "Light", "pos0");
        let initial_color = override_color(&entered, "Light", "color0");

        let advanced = rt.tick(0.016, &HashMap::new(), &vec![]);
        let advanced_pos = override_vec2(&advanced, "Light", "pos0");
        let advanced_color = override_color(&advanced, "Light", "color0");
        assert_ne!(advanced_pos, initial_pos);
        assert_eq!(advanced_color, initial_color);

        let paused = rt.tick(0.016, &HashMap::new(), &vec!["pause".into()]);
        assert_eq!(paused.current_state_id, "idle");

        let resumed = rt.tick(0.016, &HashMap::new(), &vec!["resume".into()]);
        let resumed_pos = override_vec2(&resumed, "Light", "pos0");
        let resumed_color = override_color(&resumed, "Light", "color0");
        assert_eq!(resumed.current_state_id, "mutation");
        assert_eq!(resumed_pos, initial_pos);
        assert_eq!(resumed_color, initial_color);
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
            motion_graph_id: "instant".into(),
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
            motion_graph_id: "instant".into(),
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
            motion_graph_id: "instant".into(),
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
