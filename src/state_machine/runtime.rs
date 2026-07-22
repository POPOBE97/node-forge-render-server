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

    /// Whether the initial logical State targets have been installed into the
    /// animation engine. Later idle frames must retain post-Mutation current
    /// values so a new transaction inherits the actual presentation.
    logical_state_initialized: bool,

    /// Per-property physical/timeline presentation drivers.
    motion_engine: MotionEngine,

    /// Latest runtime input snapshot available to mutations.
    runtime_input: RuntimeInputSnapshot,

    /// Persistent key/mouse press bookkeeping used by Event Trigger holdingTime outputs.
    trigger_holds: TriggerHoldState,

    /// Whether the state machine has reached the exit state.
    pub finished: bool,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FiredEvent {
    pub event_type: String,
    pub key: Option<String>,
    pub button: Option<String>,
    pub repeat: bool,
    pub modifiers: EventModifiers,
}

impl From<&str> for FiredEvent {
    fn from(event_type: &str) -> Self {
        Self {
            event_type: event_type.to_string(),
            ..Default::default()
        }
    }
}

impl From<String> for FiredEvent {
    fn from(event_type: String) -> Self {
        Self {
            event_type,
            ..Default::default()
        }
    }
}

impl From<&String> for FiredEvent {
    fn from(event_type: &String) -> Self {
        Self::from(event_type.as_str())
    }
}

/// Complete interaction events fired this tick.
pub type FiredEvents = Vec<FiredEvent>;

#[derive(Debug, Clone)]
struct ActiveKeyHold {
    key: Option<String>,
    modifiers: EventModifiers,
    started_at: f64,
}

#[derive(Debug, Clone)]
struct ReleasedKeyHold {
    key: Option<String>,
    modifiers: EventModifiers,
    duration: f64,
}

#[derive(Debug, Clone, Default)]
struct TriggerHoldState {
    active_keys: Vec<ActiveKeyHold>,
    active_mouse_buttons: HashMap<String, f64>,
    released_keys: Vec<ReleasedKeyHold>,
    released_mouse_buttons: Vec<f64>,
}

impl TriggerHoldState {
    fn begin_tick(&mut self) {
        self.released_keys.clear();
        self.released_mouse_buttons.clear();
    }

    fn process_events(&mut self, scene_time: f64, events: &FiredEvents) {
        self.begin_tick();
        for event in events {
            match event.event_type.as_str() {
                "keydown" if !event.repeat => {
                    let already_active =
                        self.active_keys
                            .iter()
                            .any(|active| match (&active.key, &event.key) {
                                (Some(active), Some(incoming)) => keys_match(active, incoming),
                                (None, None) => true,
                                _ => false,
                            });
                    if !already_active {
                        self.active_keys.push(ActiveKeyHold {
                            key: event.key.clone(),
                            modifiers: event.modifiers,
                            started_at: scene_time,
                        });
                    }
                }
                "keyup" => {
                    let index = event.key.as_deref().and_then(|released_key| {
                        self.active_keys.iter().position(|active| {
                            active
                                .key
                                .as_deref()
                                .is_some_and(|active_key| keys_match(active_key, released_key))
                        })
                    });
                    let duration = if let Some(index) = index {
                        (scene_time - self.active_keys.remove(index).started_at).max(0.0)
                    } else if event.key.is_none() {
                        let duration = self
                            .active_keys
                            .iter()
                            .map(|active| (scene_time - active.started_at).max(0.0))
                            .fold(0.0, f64::max);
                        self.active_keys.clear();
                        duration
                    } else {
                        0.0
                    };
                    self.released_keys.push(ReleasedKeyHold {
                        key: event.key.clone(),
                        modifiers: event.modifiers,
                        duration,
                    });
                }
                "mousedown" => {
                    let button = event.button.clone().unwrap_or_else(|| "__unknown__".into());
                    self.active_mouse_buttons
                        .entry(button)
                        .or_insert(scene_time);
                }
                "mouseup" => {
                    let duration = if let Some(button) = event.button.as_ref() {
                        self.active_mouse_buttons
                            .remove(button)
                            .map(|started_at| (scene_time - started_at).max(0.0))
                            .unwrap_or(0.0)
                    } else {
                        let duration = self
                            .active_mouse_buttons
                            .values()
                            .map(|started_at| (scene_time - *started_at).max(0.0))
                            .fold(0.0, f64::max);
                        self.active_mouse_buttons.clear();
                        duration
                    };
                    self.released_mouse_buttons.push(duration);
                }
                _ => {}
            }
        }
    }

    fn holding_time(
        &self,
        event_type: &str,
        key: Option<&str>,
        modifiers: EventModifiers,
        scene_time: f64,
    ) -> f64 {
        match event_type {
            "keydown" => self
                .active_keys
                .iter()
                .filter(|active| match key {
                    Some(expected) => {
                        active.modifiers == modifiers
                            && active
                                .key
                                .as_deref()
                                .is_some_and(|actual| keys_match(expected, actual))
                    }
                    None => true,
                })
                .map(|active| (scene_time - active.started_at).max(0.0))
                .fold(0.0, f64::max),
            "keyup" => self
                .released_keys
                .iter()
                .filter(|released| match key {
                    Some(expected) => {
                        released.modifiers == modifiers
                            && released
                                .key
                                .as_deref()
                                .is_some_and(|actual| keys_match(expected, actual))
                    }
                    None => true,
                })
                .map(|released| released.duration)
                .fold(0.0, f64::max),
            "mousedown" => self
                .active_mouse_buttons
                .values()
                .map(|started_at| (scene_time - *started_at).max(0.0))
                .fold(0.0, f64::max),
            "mouseup" => self
                .released_mouse_buttons
                .iter()
                .copied()
                .fold(0.0, f64::max),
            _ => 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ConditionValue {
    Bool(bool),
    Number(f64),
}

impl ConditionValue {
    fn as_bool(self) -> bool {
        match self {
            Self::Bool(value) => value,
            Self::Number(value) => value != 0.0,
        }
    }

    fn as_number(self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(value),
            Self::Bool(_) => None,
        }
    }
}

fn condition_value_from_json(value: &serde_json::Value) -> Option<ConditionValue> {
    value
        .as_bool()
        .map(ConditionValue::Bool)
        .or_else(|| value.as_f64().map(ConditionValue::Number))
}

fn compare_numbers(
    left: ConditionValue,
    right: ConditionValue,
    compare: impl FnOnce(f64, f64) -> bool,
) -> bool {
    left.as_number()
        .zip(right.as_number())
        .is_some_and(|(left, right)| compare(left, right))
}

fn normalized_key(key: &str) -> String {
    match key {
        "Space" | "Spacebar" => " ".to_string(),
        other if other.len() == 1 => other.to_ascii_lowercase(),
        other => other.to_string(),
    }
}

fn keys_match(expected: &str, actual: &str) -> bool {
    normalized_key(expected) == normalized_key(actual)
}

fn event_trigger_matches(
    event: &FiredEvent,
    event_type: &str,
    key: Option<&str>,
    modifiers: EventModifiers,
    ignore_repeat: bool,
) -> bool {
    if event.event_type != event_type || (ignore_repeat && event.repeat) {
        return false;
    }
    let Some(expected_key) = key else {
        return true;
    };
    event
        .key
        .as_deref()
        .is_some_and(|actual| keys_match(expected_key, actual))
        && event.modifiers == modifiers
}

fn input_number<F>(
    input: &F,
    port_id: &str,
    cache: &mut HashMap<(String, String), ConditionValue>,
    visiting: &mut HashSet<String>,
    default: f64,
) -> f64
where
    F: Fn(
        &str,
        &mut HashMap<(String, String), ConditionValue>,
        &mut HashSet<String>,
    ) -> Option<ConditionValue>,
{
    input(port_id, cache, visiting)
        .and_then(ConditionValue::as_number)
        .unwrap_or(default)
}

impl StateMachineRuntime {
    /// Construct a new runtime from a validated `StateMachine` definition.
    ///
    /// Call [`super::validation::validate`] before constructing if you want
    /// fail-fast diagnostics.
    pub fn new(definition: StateMachine) -> Self {
        Self::with_initial_values(definition, HashMap::new())
    }

    /// Construct a runtime with the scene's current uniform snapshot owned by
    /// the animation engine from the first frame onward.
    pub fn with_initial_values(
        definition: StateMachine,
        initial_values: HashMap<OverrideKey, serde_json::Value>,
    ) -> Self {
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
            logical_state_initialized: false,
            motion_engine: MotionEngine::with_initial_values(initial_values),
            runtime_input: RuntimeInputSnapshot::default(),
            trigger_holds: TriggerHoldState::default(),
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
        self.logical_state_initialized = false;
        self.motion_engine.reset();
        self.runtime_input = RuntimeInputSnapshot::default();
        self.trigger_holds = TriggerHoldState::default();
        self.finished = false;
    }

    /// Update the latest mouse position visible to mutation nodes.
    pub fn set_mouse_position(&mut self, position: MousePosition) {
        self.runtime_input.mouse_position = Some(position);
    }

    /// Merge external UniformDelta values into the animation engine. Running
    /// channels retain transaction priority; idle channels accept the update
    /// immediately.
    pub fn update_current_values(&mut self, updates: &[(OverrideKey, serde_json::Value)]) {
        self.motion_engine.update_external_values(updates);
    }

    /// Advance the state machine by `dt` seconds and produce overrides.
    pub fn tick(&mut self, dt: f64, params: &ExternalParams, events: &FiredEvents) -> TickResult {
        if self.finished {
            return TickResult {
                overrides: self.motion_engine.current_values().clone(),
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
        self.trigger_holds.process_events(self.scene_time, events);
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

        // Establish the logical state's values through the animation engine
        // before routing. This makes an initial state's presentation the
        // source of a transition fired on the first tick.
        if !self.logical_state_initialized {
            self.motion_engine
                .commit_logical_values(self.state_parameter_patch(&current_id));
            self.logical_state_initialized = true;
        }

        // Transitions remain interruptible while a previous visual driver is
        // active. Routing uses the logical current state (already the target
        // of the previous transition).
        if let Some(transition) = self.pick_transition(params, events) {
            self.state_local_times
                .insert(transition.target.clone(), 0.0);
            let target_patch = self.state_parameter_patch(&transition.target);
            let graph = self
                .motion_graph_index
                .get(&transition.motion_graph_id)
                .and_then(|index| self.definition.motion_graphs.get(*index))
                .cloned();
            if let Some(graph) = graph {
                self.motion_engine
                    .transition_to(&transition.id, &target_patch, &graph);
            }
            self.current_state_id = transition.target;
        }

        let target_state_id = self.current_state_id.clone();

        // Motion always advances first and freezes the single source snapshot
        // observed by both Any and target-state Mutation graphs.
        let motion_step = self.motion_engine.step(dt);
        let post_motion_snapshot = self.motion_engine.current_values().clone();
        let mut mutation_patch = HashMap::new();

        if let Some(any_state) = self
            .definition
            .states
            .iter()
            .find(|state| state.resolved_type() == AnimationStateType::AnyState)
            .cloned()
            && let Some(mutation_id) = any_state.mutation_id.as_deref()
        {
            match self.evaluate_mutation_state(mutation_id, &any_state.id, &post_motion_snapshot) {
                Ok(patch) => mutation_patch.extend(patch),
                Err(error) => diagnostics.push(format!(
                    "mutation evaluation error (state={}, mutation={}): {error}",
                    any_state.id, mutation_id
                )),
            }
        }

        if let Some(target_state) = self.find_state(&target_state_id).cloned()
            && let Some(mutation_id) = target_state.mutation_id.as_deref()
        {
            match self.evaluate_mutation_state(mutation_id, &target_state.id, &post_motion_snapshot)
            {
                // Target State wins conflicts with Any.
                Ok(patch) => mutation_patch.extend(patch),
                Err(error) => diagnostics.push(format!(
                    "mutation evaluation error (state={}, mutation={}): {error}",
                    target_state.id, mutation_id
                )),
            }
        }
        self.motion_engine.commit_post_process(mutation_patch);

        // Exit becomes terminal only after its visual transition completes.
        if let Some(state) = self.find_state(&self.current_state_id) {
            if state.resolved_type() == AnimationStateType::ExitState
                && self.motion_engine.active_transition_id().is_none()
            {
                self.finished = true;
            }
        }

        TickResult {
            overrides: self.motion_engine.current_values().clone(),
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

    fn state_parameter_patch(&self, state_id: &str) -> HashMap<OverrideKey, serde_json::Value> {
        let mut patch = HashMap::new();
        let Some(state) = self.find_state(state_id) else {
            return patch;
        };
        for (key_str, value) in &state.parameter_overrides {
            if let Some(key) = OverrideKey::parse(key_str) {
                patch.insert(key, value.clone());
            }
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
        for t in &candidates {
            let Some(graph) = self
                .motion_graph_index
                .get(&t.motion_graph_id)
                .and_then(|index| self.definition.motion_graphs.get(*index))
            else {
                continue;
            };
            if !self.evaluate_transition_condition(graph, params, events) {
                continue;
            }
            return Some((*t).clone());
        }

        None
    }

    fn evaluate_transition_condition(
        &self,
        graph: &TransitionMotionGraph,
        params: &ExternalParams,
        events: &FiredEvents,
    ) -> bool {
        let Some(binding) = graph.condition_binding.as_ref() else {
            return true;
        };
        let mut cache = HashMap::new();
        let mut visiting = HashSet::new();
        match binding {
            TransitionConditionBinding::Input { input_port_id } => self
                .resolve_transition_input(input_port_id, params)
                .is_some_and(ConditionValue::as_bool),
            TransitionConditionBinding::Node { from } => self
                .evaluate_condition_node(
                    graph,
                    &from.node_id,
                    &from.port_id,
                    params,
                    events,
                    &mut cache,
                    &mut visiting,
                )
                .is_some_and(ConditionValue::as_bool),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate_condition_node(
        &self,
        graph: &TransitionMotionGraph,
        node_id: &str,
        output_port_id: &str,
        params: &ExternalParams,
        events: &FiredEvents,
        cache: &mut HashMap<(String, String), ConditionValue>,
        visiting: &mut HashSet<String>,
    ) -> Option<ConditionValue> {
        let cache_key = (node_id.to_string(), output_port_id.to_string());
        if let Some(value) = cache.get(&cache_key) {
            return Some(*value);
        }
        if !visiting.insert(node_id.to_string()) {
            return None;
        }
        let node = graph.nodes.iter().find(|node| node.id() == node_id)?;
        let input = |port_id: &str,
                     cache: &mut HashMap<(String, String), ConditionValue>,
                     visiting: &mut HashSet<String>| {
            if let Some(connection) = graph.connections.iter().find(|connection| {
                connection.to.node_id == node_id && connection.to.port_id == port_id
            }) {
                return self.evaluate_condition_node(
                    graph,
                    &connection.from.node_id,
                    &connection.from.port_id,
                    params,
                    events,
                    cache,
                    visiting,
                );
            }
            graph
                .input_bindings
                .iter()
                .find(|binding| binding.to.node_id == node_id && binding.to.port_id == port_id)
                .and_then(|binding| self.resolve_transition_input(&binding.port_id, params))
        };
        let value = match node {
            TransitionMotionNode::EventTrigger {
                event_type,
                key,
                modifiers,
                ignore_repeat,
                ..
            } => match output_port_id {
                "fired" => ConditionValue::Bool(events.iter().any(|event| {
                    event_trigger_matches(
                        event,
                        event_type,
                        key.as_deref(),
                        *modifiers,
                        *ignore_repeat,
                    )
                })),
                "holdingTime" => ConditionValue::Number(self.trigger_holds.holding_time(
                    event_type,
                    key.as_deref(),
                    *modifiers,
                    self.scene_time,
                )),
                _ => {
                    visiting.remove(node_id);
                    return None;
                }
            },
            TransitionMotionNode::Logic { op, .. } => {
                let a = input("a", cache, visiting).unwrap_or(ConditionValue::Bool(false));
                let b = input("b", cache, visiting).unwrap_or(ConditionValue::Bool(false));
                ConditionValue::Bool(match op {
                    LogicOp::And => a.as_bool() && b.as_bool(),
                    LogicOp::Or => a.as_bool() || b.as_bool(),
                    LogicOp::Not => !a.as_bool(),
                    LogicOp::Equal => a == b,
                    LogicOp::NotEqual => a != b,
                    LogicOp::Greater => compare_numbers(a, b, |left, right| left > right),
                    LogicOp::GreaterEqual => compare_numbers(a, b, |left, right| left >= right),
                    LogicOp::Less => compare_numbers(a, b, |left, right| left < right),
                    LogicOp::LessEqual => compare_numbers(a, b, |left, right| left <= right),
                })
            }
            TransitionMotionNode::BoolInput { value, .. } => ConditionValue::Bool(*value),
            TransitionMotionNode::FloatInput { value, .. } => ConditionValue::Number(*value),
            TransitionMotionNode::MathAdd { .. } => ConditionValue::Number(
                input_number(&input, "a", cache, visiting, 0.0)
                    + input_number(&input, "b", cache, visiting, 0.0),
            ),
            TransitionMotionNode::MathSubtract { .. } => ConditionValue::Number(
                input_number(&input, "a", cache, visiting, 0.0)
                    - input_number(&input, "b", cache, visiting, 0.0),
            ),
            TransitionMotionNode::MathMultiply { .. } => ConditionValue::Number(
                input_number(&input, "a", cache, visiting, 0.0)
                    * input_number(&input, "b", cache, visiting, 0.0),
            ),
            TransitionMotionNode::MathDivide { .. } => {
                let numerator = input_number(&input, "a", cache, visiting, 0.0);
                let denominator = input_number(&input, "b", cache, visiting, 0.0);
                ConditionValue::Number(if denominator == 0.0 {
                    0.0
                } else {
                    numerator / denominator
                })
            }
            TransitionMotionNode::Lerp { .. } => {
                let a = input_number(&input, "a", cache, visiting, 0.0);
                let b = input_number(&input, "b", cache, visiting, 0.0);
                let t = input_number(&input, "t", cache, visiting, 0.5);
                ConditionValue::Number(a + (b - a) * t)
            }
            _ => {
                visiting.remove(node_id);
                return None;
            }
        };
        visiting.remove(node_id);
        cache.insert(cache_key, value);
        Some(value)
    }

    fn resolve_transition_input(
        &self,
        port_id: &str,
        params: &ExternalParams,
    ) -> Option<ConditionValue> {
        match port_id {
            "sceneElapsedTime" => Some(ConditionValue::Number(self.scene_time)),
            "localElapsedTime" => Some(ConditionValue::Number(
                self.state_local_times
                    .get(&self.current_state_id)
                    .copied()
                    .unwrap_or(0.0),
            )),
            "mouse.position.x" => self
                .runtime_input
                .mouse_position
                .map(|position| ConditionValue::Number(position.x)),
            "mouse.position.y" => self
                .runtime_input
                .mouse_position
                .map(|position| ConditionValue::Number(position.y)),
            _ => params
                .get(port_id)
                .or_else(|| {
                    OverrideKey::parse(port_id)
                        .as_ref()
                        .and_then(|key| self.motion_engine.current_values().get(key))
                })
                .and_then(condition_value_from_json),
        }
    }

    fn evaluate_mutation_state(
        &self,
        mutation_id: &str,
        state_id: &str,
        current_snapshot: &HashMap<OverrideKey, serde_json::Value>,
    ) -> Result<HashMap<OverrideKey, serde_json::Value>> {
        let mutation_idx = self
            .mutation_index
            .get(mutation_id)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("mutation '{mutation_id}' not found"))?;
        let mutation = &self.definition.mutations[mutation_idx];

        // Build input context from the frozen post-motion current snapshot.
        let mut input_values: HashMap<String, MutationValue> = HashMap::new();
        for input_port in &mutation.inputs {
            if let Some(key) = OverrideKey::parse(&input_port.id)
                && let Some(value) = current_snapshot.get(&key)
                && let Some(value) = MutationValue::from_json(value)
            {
                input_values.insert(input_port.id.clone(), value);
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
                    state_type: AnimationStateType::EntryState,
                    mutation_id: None,
                },
                AnimationState {
                    id: "any".into(),
                    name: "Any".into(),
                    position: None,
                    parameter_overrides: Default::default(),
                    state_type: AnimationStateType::AnyState,
                    mutation_id: None,
                },
                AnimationState {
                    id: "exit".into(),
                    name: "Exit".into(),
                    position: None,
                    parameter_overrides: Default::default(),
                    state_type: AnimationStateType::ExitState,
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
            condition_binding: None,
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

    fn with_event_condition(
        mut graph: TransitionMotionGraph,
        event_type: &str,
    ) -> TransitionMotionGraph {
        graph.nodes.push(TransitionMotionNode::EventTrigger {
            id: "trigger".into(),
            position: Position::default(),
            label: None,
            event_type: event_type.into(),
            key: None,
            modifiers: EventModifiers::default(),
            ignore_repeat: true,
        });
        graph.condition_binding = Some(TransitionConditionBinding::Node {
            from: MutationEndpoint {
                node_id: "trigger".into(),
                port_id: "fired".into(),
            },
        });
        graph
    }

    fn with_bool_input_condition(
        mut graph: TransitionMotionGraph,
        input_port_id: &str,
    ) -> TransitionMotionGraph {
        graph.inputs.push(MutationPort {
            id: input_port_id.into(),
            name: Some(input_port_id.into()),
            port_type: Some("bool".into()),
        });
        graph.condition_binding = Some(TransitionConditionBinding::Input {
            input_port_id: input_port_id.into(),
        });
        graph
    }

    fn with_event_and_bool_input_condition(
        mut graph: TransitionMotionGraph,
        event_type: &str,
        input_port_id: &str,
    ) -> TransitionMotionGraph {
        graph.inputs.push(MutationPort {
            id: input_port_id.into(),
            name: Some(input_port_id.into()),
            port_type: Some("bool".into()),
        });
        graph.nodes.push(TransitionMotionNode::EventTrigger {
            id: "trigger".into(),
            position: Position::default(),
            label: None,
            event_type: event_type.into(),
            key: None,
            modifiers: EventModifiers::default(),
            ignore_repeat: true,
        });
        graph.nodes.push(TransitionMotionNode::Logic {
            id: "condition".into(),
            position: Position::default(),
            label: None,
            op: LogicOp::And,
        });
        graph.connections.push(MutationConnection {
            id: "trigger-to-condition".into(),
            from: MutationEndpoint {
                node_id: "trigger".into(),
                port_id: "fired".into(),
            },
            to: MutationEndpoint {
                node_id: "condition".into(),
                port_id: "a".into(),
            },
        });
        graph.input_bindings.push(TransitionMotionInputBinding {
            port_id: input_port_id.into(),
            to: MutationEndpoint {
                node_id: "condition".into(),
                port_id: "b".into(),
            },
        });
        graph.condition_binding = Some(TransitionConditionBinding::Node {
            from: MutationEndpoint {
                node_id: "condition".into(),
                port_id: "result".into(),
            },
        });
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
            state_type: AnimationStateType::AnimationState,
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
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
            state_type: AnimationStateType::AnimationState,
            mutation_id: None,
        });
        sm.states.push(AnimationState {
            id: "b".into(),
            name: "B".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "entry_to_a".into(),
            source: "entry".into(),
            target: "a".into(),
            motion_graph_id: "instant".into(),
        });
        sm.motion_graphs.push(with_event_condition(
            instant_motion_graph("go-instant"),
            "go",
        ));
        sm.transitions.push(AnimationTransition {
            id: "a_to_b".into(),
            source: "a".into(),
            target: "b".into(),
            motion_graph_id: "go-instant".into(),
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
            state_type: AnimationStateType::AnimationState,
            mutation_id: None,
        });
        sm.states.push(AnimationState {
            id: "b".into(),
            name: "B".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "entry_to_a".into(),
            source: "entry".into(),
            target: "a".into(),
            motion_graph_id: "instant".into(),
        });
        sm.motion_graphs.push(with_event_condition(
            timeline_motion_graph("go-timeline", 1.0),
            "go",
        ));
        sm.transitions.push(AnimationTransition {
            id: "a_to_b".into(),
            source: "a".into(),
            target: "b".into(),
            motion_graph_id: "go-timeline".into(),
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
            state_type: AnimationStateType::AnimationState,
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
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
            state_type: AnimationStateType::AnimationState,
            mutation_id: None,
        });
        sm.motion_graphs.push(with_bool_input_condition(
            instant_motion_graph("flag-condition"),
            "flag",
        ));
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
            motion_graph_id: "flag-condition".into(),
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
            state_type: AnimationStateType::AnimationState,
            mutation_id: None,
        });
        sm.motion_graphs.push(with_event_condition(
            instant_motion_graph("click-condition"),
            "click",
        ));
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
            motion_graph_id: "click-condition".into(),
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
            state_type: AnimationStateType::AnimationState,
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
        sm.motion_graphs.push(with_event_condition(
            timeline_motion_graph("mousedown-condition", 0.3),
            "mousedown",
        ));
        sm.transitions.push(AnimationTransition {
            id: "tr_any_mutation".into(),
            source: "any".into(),
            target: "mutation".into(),
            motion_graph_id: "mousedown-condition".into(),
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
    fn post_motion_mutation_overrides_timeline_with_current_local_time() {
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
            state_type: AnimationStateType::AnimationState,
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
        assert!((value - 0.1).abs() < 1e-8, "value={value}");
        assert_eq!(advancing.state_local_times.get("dynamic"), Some(&0.1));
    }

    #[test]
    fn transition_after_idle_mutation_inherits_final_current_value() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "a".into(),
            name: "A".into(),
            position: None,
            parameter_overrides: [("Node:x".into(), serde_json::json!(0.0))]
                .into_iter()
                .collect(),
            state_type: AnimationStateType::AnimationState,
            mutation_id: Some("a_mutation".into()),
        });
        sm.states.push(AnimationState {
            id: "b".into(),
            name: "B".into(),
            position: None,
            parameter_overrides: [("Node:x".into(), serde_json::json!(10.0))]
                .into_iter()
                .collect(),
            state_type: AnimationStateType::AnimationState,
            mutation_id: None,
        });
        sm.mutations.push(MutationDefinition {
            id: "a_mutation".into(),
            name: "A Mutation".into(),
            inputs: vec![MutationPort {
                id: "localElapsedTime".into(),
                name: None,
                port_type: Some("float".into()),
            }],
            outputs: vec![MutationPort {
                id: "Node:x".into(),
                name: None,
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
            id: "entry_to_a".into(),
            source: "entry".into(),
            target: "a".into(),
            motion_graph_id: "instant".into(),
        });
        sm.motion_graphs
            .push(with_event_condition(timeline_motion_graph("go", 1.0), "go"));
        sm.transitions.push(AnimationTransition {
            id: "a_to_b".into(),
            source: "a".into(),
            target: "b".into(),
            motion_graph_id: "go".into(),
        });

        let mut runtime = StateMachineRuntime::new(sm);
        runtime.tick(0.0, &HashMap::new(), &vec![]);
        let idle = runtime.tick(0.2, &HashMap::new(), &vec![]);
        assert_eq!(
            idle.overrides.get(&OverrideKey::new("Node", "x")),
            Some(&serde_json::json!(0.2))
        );
        let interrupted = runtime.tick(0.0, &HashMap::new(), &vec!["go".into()]);
        assert_eq!(interrupted.active_transition_id.as_deref(), Some("a_to_b"));
        assert_eq!(
            interrupted.overrides.get(&OverrideKey::new("Node", "x")),
            Some(&serde_json::json!(0.2))
        );
    }

    #[test]
    fn any_and_target_mutations_share_post_motion_snapshot_and_target_wins() {
        fn mutation(id: &str, seen_output: &str, conflict_value: f64) -> MutationDefinition {
            MutationDefinition {
                id: id.into(),
                name: id.into(),
                inputs: vec![MutationPort {
                    id: "Node:x".into(),
                    name: None,
                    port_type: Some("float".into()),
                }],
                outputs: vec![
                    MutationPort {
                        id: seen_output.into(),
                        name: None,
                        port_type: Some("float".into()),
                    },
                    MutationPort {
                        id: "Node:conflict".into(),
                        name: None,
                        port_type: Some("float".into()),
                    },
                ],
                nodes: vec![MutationInnerNode {
                    id: "constant".into(),
                    node_type: MutationInnerNodeType::FloatInput,
                    params: HashMap::from([("value".into(), serde_json::json!(conflict_value))]),
                    inputs: vec![],
                    outputs: vec![MutationPort {
                        id: "value".into(),
                        name: None,
                        port_type: Some("float".into()),
                    }],
                }],
                connections: vec![],
                input_bindings: vec![],
                output_bindings: vec![MutationOutputBinding {
                    port_id: "Node:conflict".into(),
                    from: MutationEndpoint {
                        node_id: "constant".into(),
                        port_id: "value".into(),
                    },
                }],
                passthrough_bindings: vec![MutationPassthroughBinding {
                    from_port_id: "Node:x".into(),
                    to_port_id: seen_output.into(),
                }],
                viewport: None,
            }
        }

        let mut sm = minimal_sm();
        sm.states
            .iter_mut()
            .find(|state| state.id == "entry")
            .unwrap()
            .parameter_overrides
            .insert("Node:x".into(), serde_json::json!(0.0));
        sm.states
            .iter_mut()
            .find(|state| state.id == "any")
            .unwrap()
            .mutation_id = Some("any_mutation".into());
        sm.states.push(AnimationState {
            id: "target".into(),
            name: "Target".into(),
            position: None,
            parameter_overrides: [("Node:x".into(), serde_json::json!(10.0))]
                .into_iter()
                .collect(),
            state_type: AnimationStateType::AnimationState,
            mutation_id: Some("target_mutation".into()),
        });
        sm.mutations
            .push(mutation("any_mutation", "Node:anySeen", 1.0));
        sm.mutations
            .push(mutation("target_mutation", "Node:targetSeen", 2.0));
        sm.transitions.push(AnimationTransition {
            id: "entry_to_target".into(),
            source: "entry".into(),
            target: "target".into(),
            motion_graph_id: "timeline-1".into(),
        });

        let result = StateMachineRuntime::new(sm).tick(0.5, &HashMap::new(), &vec![]);
        assert_eq!(
            result.overrides.get(&OverrideKey::new("Node", "anySeen")),
            Some(&serde_json::json!(5.0))
        );
        assert_eq!(
            result
                .overrides
                .get(&OverrideKey::new("Node", "targetSeen")),
            Some(&serde_json::json!(5.0))
        );
        assert_eq!(
            result.overrides.get(&OverrideKey::new("Node", "conflict")),
            Some(&serde_json::json!(2.0))
        );
    }

    #[test]
    fn event_transition_mutation_reads_same_tick_mouse_position() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "mutation".into(),
            name: "Mutation".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
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
        sm.motion_graphs.push(with_event_condition(
            instant_motion_graph("mousedown-instant"),
            "mousedown",
        ));
        sm.transitions.push(AnimationTransition {
            id: "entry_to_mouse".into(),
            source: "entry".into(),
            target: "mutation".into(),
            motion_graph_id: "mousedown-instant".into(),
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
            state_type: AnimationStateType::AnimationState,
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
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
            state_type: AnimationStateType::AnimationState,
            mutation_id: None,
        });
        sm.motion_graphs.push(with_event_and_bool_input_condition(
            instant_motion_graph("go-and-ready"),
            "go",
            "ready",
        ));
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
            motion_graph_id: "go-and-ready".into(),
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

    #[test]
    fn graph_owned_key_chord_ignores_repeat_and_matches_exact_modifiers() {
        let mut sm = minimal_sm();
        let mut graph = instant_motion_graph("key-condition");
        graph.nodes.push(TransitionMotionNode::EventTrigger {
            id: "space".into(),
            position: Position::default(),
            label: None,
            event_type: "keydown".into(),
            key: Some(" ".into()),
            modifiers: EventModifiers::default(),
            ignore_repeat: true,
        });
        graph.condition_binding = Some(TransitionConditionBinding::Node {
            from: MutationEndpoint {
                node_id: "space".into(),
                port_id: "fired".into(),
            },
        });
        sm.motion_graphs.push(graph);
        sm.transitions.push(AnimationTransition {
            id: "key-transition".into(),
            source: "entry".into(),
            target: "exit".into(),
            motion_graph_id: "key-condition".into(),
        });

        let mut runtime = StateMachineRuntime::new(sm);
        let repeated = FiredEvent {
            event_type: "keydown".into(),
            key: Some(" ".into()),
            button: None,
            repeat: true,
            modifiers: EventModifiers::default(),
        };
        assert_eq!(
            runtime
                .tick(0.0, &HashMap::new(), &vec![repeated])
                .current_state_id,
            "entry"
        );
        let modified = FiredEvent {
            event_type: "keydown".into(),
            key: Some(" ".into()),
            button: None,
            repeat: false,
            modifiers: EventModifiers {
                ctrl: true,
                ..Default::default()
            },
        };
        assert_eq!(
            runtime
                .tick(0.0, &HashMap::new(), &vec![modified])
                .current_state_id,
            "entry"
        );
        let matching = FiredEvent {
            event_type: "keydown".into(),
            key: Some("Space".into()),
            button: None,
            repeat: false,
            modifiers: EventModifiers::default(),
        };
        assert_eq!(
            runtime
                .tick(0.0, &HashMap::new(), &vec![matching])
                .current_state_id,
            "exit"
        );
    }

    #[test]
    fn key_and_mouse_holding_times_split_press_and_release_semantics() {
        let mut holds = TriggerHoldState::default();
        let mouse_down = FiredEvent {
            event_type: "mousedown".into(),
            button: Some("left".into()),
            ..Default::default()
        };
        holds.process_events(1.0, &vec![mouse_down]);
        holds.process_events(1.25, &vec![]);
        assert!(
            (holds.holding_time("mousedown", None, EventModifiers::default(), 1.25) - 0.25).abs()
                < 1e-9
        );

        let mouse_up = FiredEvent {
            event_type: "mouseup".into(),
            button: Some("left".into()),
            ..Default::default()
        };
        holds.process_events(1.3, &vec![mouse_up]);
        assert_eq!(
            holds.holding_time("mousedown", None, EventModifiers::default(), 1.3),
            0.0
        );
        assert!(
            (holds.holding_time("mouseup", None, EventModifiers::default(), 1.3) - 0.3).abs()
                < 1e-9
        );
        holds.process_events(1.31, &vec![]);
        assert_eq!(
            holds.holding_time("mouseup", None, EventModifiers::default(), 1.31),
            0.0
        );

        let key_down = FiredEvent {
            event_type: "keydown".into(),
            key: Some("Space".into()),
            ..Default::default()
        };
        holds.process_events(2.0, &vec![key_down]);
        holds.process_events(2.4, &vec![]);
        assert!(
            (holds.holding_time("keydown", Some(" "), EventModifiers::default(), 2.4) - 0.4).abs()
                < 1e-9
        );

        let key_up = FiredEvent {
            event_type: "keyup".into(),
            key: Some("Space".into()),
            ..Default::default()
        };
        holds.process_events(2.5, &vec![key_up]);
        assert!(
            (holds.holding_time("keyup", Some(" "), EventModifiers::default(), 2.5) - 0.5).abs()
                < 1e-9
        );
    }

    #[test]
    fn graph_owned_mousedown_holding_time_fires_before_release() {
        let mut sm = minimal_sm();
        let mut graph = instant_motion_graph("mouse-hold");
        graph.nodes.extend([
            TransitionMotionNode::EventTrigger {
                id: "down".into(),
                position: Position::default(),
                label: None,
                event_type: "mousedown".into(),
                key: None,
                modifiers: EventModifiers::default(),
                ignore_repeat: true,
            },
            TransitionMotionNode::FloatInput {
                id: "threshold".into(),
                position: Position::default(),
                label: None,
                value: 0.2,
            },
            TransitionMotionNode::Logic {
                id: "held-long-enough".into(),
                position: Position::default(),
                label: None,
                op: LogicOp::GreaterEqual,
            },
        ]);
        graph.connections.extend([
            MutationConnection {
                id: "holding-time".into(),
                from: MutationEndpoint {
                    node_id: "down".into(),
                    port_id: "holdingTime".into(),
                },
                to: MutationEndpoint {
                    node_id: "held-long-enough".into(),
                    port_id: "a".into(),
                },
            },
            MutationConnection {
                id: "threshold".into(),
                from: MutationEndpoint {
                    node_id: "threshold".into(),
                    port_id: "value".into(),
                },
                to: MutationEndpoint {
                    node_id: "held-long-enough".into(),
                    port_id: "b".into(),
                },
            },
        ]);
        graph.condition_binding = Some(TransitionConditionBinding::Node {
            from: MutationEndpoint {
                node_id: "held-long-enough".into(),
                port_id: "result".into(),
            },
        });
        sm.motion_graphs.push(graph);
        sm.transitions.push(AnimationTransition {
            id: "hold-transition".into(),
            source: "entry".into(),
            target: "exit".into(),
            motion_graph_id: "mouse-hold".into(),
        });

        let mut runtime = StateMachineRuntime::new(sm);
        let down = FiredEvent {
            event_type: "mousedown".into(),
            button: Some("left".into()),
            ..Default::default()
        };
        assert_eq!(
            runtime
                .tick(0.0, &HashMap::new(), &vec![down])
                .current_state_id,
            "entry"
        );
        assert_eq!(
            runtime
                .tick(0.19, &HashMap::new(), &vec![])
                .current_state_id,
            "entry"
        );
        assert_eq!(
            runtime
                .tick(0.02, &HashMap::new(), &vec![])
                .current_state_id,
            "exit"
        );
    }

    #[test]
    fn mouseup_holding_time_output_keeps_completed_duration_for_release_tick() {
        let mut runtime = StateMachineRuntime::new(minimal_sm());
        let down = FiredEvent {
            event_type: "mousedown".into(),
            button: Some("left".into()),
            ..Default::default()
        };
        runtime.tick(0.0, &HashMap::new(), &vec![down]);
        runtime.tick(0.12, &HashMap::new(), &vec![]);

        let up = FiredEvent {
            event_type: "mouseup".into(),
            button: Some("left".into()),
            ..Default::default()
        };
        runtime.tick(0.0, &HashMap::new(), &vec![up.clone()]);

        let mut graph = instant_motion_graph("release-hold");
        graph.nodes.push(TransitionMotionNode::EventTrigger {
            id: "up".into(),
            position: Position::default(),
            label: None,
            event_type: "mouseup".into(),
            key: None,
            modifiers: EventModifiers::default(),
            ignore_repeat: true,
        });
        let value = runtime.evaluate_condition_node(
            &graph,
            "up",
            "holdingTime",
            &HashMap::new(),
            &vec![up],
            &mut HashMap::new(),
            &mut HashSet::new(),
        );

        assert_eq!(value, Some(ConditionValue::Number(0.12)));
    }

    #[test]
    fn graph_owned_mouse_range_is_composed_from_logic_nodes() {
        let mut sm = minimal_sm();
        let mut graph = instant_motion_graph("mouse-range");
        graph.inputs.push(MutationPort {
            id: "mouse.position.x".into(),
            name: Some("Mouse Position X".into()),
            port_type: Some("float".into()),
        });
        graph.nodes.extend([
            TransitionMotionNode::FloatInput {
                id: "lower".into(),
                position: Position::default(),
                label: None,
                value: 20.0,
            },
            TransitionMotionNode::FloatInput {
                id: "upper".into(),
                position: Position::default(),
                label: None,
                value: 80.0,
            },
            TransitionMotionNode::Logic {
                id: "gte".into(),
                position: Position::default(),
                label: None,
                op: LogicOp::GreaterEqual,
            },
            TransitionMotionNode::Logic {
                id: "lte".into(),
                position: Position::default(),
                label: None,
                op: LogicOp::LessEqual,
            },
            TransitionMotionNode::Logic {
                id: "inside".into(),
                position: Position::default(),
                label: None,
                op: LogicOp::And,
            },
        ]);
        for node_id in ["gte", "lte"] {
            graph.input_bindings.push(TransitionMotionInputBinding {
                port_id: "mouse.position.x".into(),
                to: MutationEndpoint {
                    node_id: node_id.into(),
                    port_id: "a".into(),
                },
            });
        }
        graph.connections.extend([
            MutationConnection {
                id: "lower-gte".into(),
                from: MutationEndpoint {
                    node_id: "lower".into(),
                    port_id: "value".into(),
                },
                to: MutationEndpoint {
                    node_id: "gte".into(),
                    port_id: "b".into(),
                },
            },
            MutationConnection {
                id: "upper-lte".into(),
                from: MutationEndpoint {
                    node_id: "upper".into(),
                    port_id: "value".into(),
                },
                to: MutationEndpoint {
                    node_id: "lte".into(),
                    port_id: "b".into(),
                },
            },
            MutationConnection {
                id: "gte-inside".into(),
                from: MutationEndpoint {
                    node_id: "gte".into(),
                    port_id: "result".into(),
                },
                to: MutationEndpoint {
                    node_id: "inside".into(),
                    port_id: "a".into(),
                },
            },
            MutationConnection {
                id: "lte-inside".into(),
                from: MutationEndpoint {
                    node_id: "lte".into(),
                    port_id: "result".into(),
                },
                to: MutationEndpoint {
                    node_id: "inside".into(),
                    port_id: "b".into(),
                },
            },
        ]);
        graph.condition_binding = Some(TransitionConditionBinding::Node {
            from: MutationEndpoint {
                node_id: "inside".into(),
                port_id: "result".into(),
            },
        });
        sm.motion_graphs.push(graph);
        sm.transitions.push(AnimationTransition {
            id: "mouse-transition".into(),
            source: "entry".into(),
            target: "exit".into(),
            motion_graph_id: "mouse-range".into(),
        });

        let mut runtime = StateMachineRuntime::new(sm);
        runtime.set_mouse_position(MousePosition { x: 10.0, y: 0.0 });
        assert_eq!(
            runtime.tick(0.0, &HashMap::new(), &vec![]).current_state_id,
            "entry"
        );
        runtime.set_mouse_position(MousePosition { x: 50.0, y: 0.0 });
        assert_eq!(
            runtime.tick(0.0, &HashMap::new(), &vec![]).current_state_id,
            "exit"
        );
    }
}
