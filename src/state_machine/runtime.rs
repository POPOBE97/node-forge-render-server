//! Tick-driven state machine runtime.
//!
//! The runtime is intentionally decoupled from the render pipeline.
//! It consumes a compiled `StateMachine` definition (from DSL) and
//! produces GPU uniform overrides only after the active Derivation runs.
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

use anyhow::{Result, bail};

use super::graph::{self, GraphEvaluationPhase, GraphInputContext, GraphValue};
use super::motion::{MotionChannelDebug, MotionEngine};
use super::types::*;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Opaque runtime for a single `StateMachine` definition.
#[derive(Debug, Clone)]
pub struct StateMachineRuntime {
    /// The compiled definition (immutable after construction).
    definition: StateMachine,

    /// Lookup: Derivation id → index into `definition.derivations`.
    derivation_index: HashMap<String, usize>,

    /// Lookup: transition motion graph id → index into `definition.motion_graphs`.
    motion_graph_index: HashMap<String, usize>,

    /// Current active state id.
    current_state_id: String,

    /// Debug-only forced state. While set, routing is disabled and the
    /// selected state's logical targets are reasserted every frame.
    forced_state_id: Option<String>,

    /// Wall-clock time accumulated since scene start (seconds).
    scene_time: f64,

    /// Per-state local elapsed time (seconds).
    /// Each state independently tracks how long it has been "active"
    /// (ticking).  Entry/Any/Exit states stay at 0.
    state_local_times: HashMap<String, f64>,

    /// Whether the initial logical State targets have been installed into the
    /// animation engine. Mutation values never participate in this state.
    logical_state_initialized: bool,

    /// Per-property physical/timeline presentation drivers.
    motion_engine: MotionEngine,

    /// Plain Derivation outputs computed from the current physical P snapshot.
    /// These values are merged into the render overrides but are not motion
    /// coordinates and therefore own no Q/E/P or driver state.
    derived_values: HashMap<OverrideKey, serde_json::Value>,

    /// Last successful output snapshot for each Derivation. A failed D frame
    /// retains only that Derivation's own snapshot.
    derivation_snapshots: HashMap<String, HashMap<OverrideKey, serde_json::Value>>,

    /// Latest runtime input snapshot available to State Mutation and Derivation graphs.
    runtime_input: RuntimeInputSnapshot,

    /// Cold-start error captured while preparing persistent Graph Function contexts.
    function_prepare_error: Option<String>,

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
        let initial_values = definition
            .state_params
            .iter()
            .map(|declaration| {
                (
                    StateParamKey::new(&declaration.id),
                    declaration.default_value.clone(),
                )
            })
            .collect();
        Self::with_initial_values(definition, initial_values)
    }

    /// Construct a runtime from the scene's current declaration snapshot.
    /// Only independent Motion fields are installed into MotionEngine.
    pub fn with_initial_values(
        definition: StateMachine,
        initial_values: HashMap<StateParamKey, serde_json::Value>,
    ) -> Self {
        let motion_initial_values = initial_values;
        let function_prepare_error = super::graph_function::prepare_state_machine(&definition)
            .err()
            .map(|error| format!("{error:#}"));
        let derivation_index: HashMap<String, usize> = definition
            .derivations
            .iter()
            .enumerate()
            .map(|(index, derivation)| (derivation.id.clone(), index))
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
            derivation_index,
            motion_graph_index,
            current_state_id: initial,
            forced_state_id: None,
            scene_time: 0.0,
            state_local_times,
            logical_state_initialized: false,
            motion_engine: MotionEngine::with_initial_values(motion_initial_values),
            derived_values: HashMap::new(),
            derivation_snapshots: HashMap::new(),
            runtime_input: RuntimeInputSnapshot::default(),
            function_prepare_error,
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
        self.forced_state_id = None;
        self.scene_time = 0.0;
        // Re-initialize all state local times to 0.0 (same as construction).
        for v in self.state_local_times.values_mut() {
            *v = 0.0;
        }
        self.logical_state_initialized = false;
        self.motion_engine.reset();
        self.derived_values.clear();
        self.derivation_snapshots.clear();
        self.runtime_input = RuntimeInputSnapshot::default();
        self.trigger_holds = TriggerHoldState::default();
        self.finished = false;
    }

    /// Force the runtime to remain in one State for debug inspection.
    ///
    /// Entry, Any, Exit, and ordinary animation states are all valid debug
    /// targets. Derivation nodes are graph resources rather than selectable
    /// States and are rejected.
    pub fn force_state(&mut self, state_id: &str) -> Result<()> {
        let state = self
            .find_state(state_id)
            .ok_or_else(|| anyhow::anyhow!("state '{state_id}' not found"))?;
        if state.resolved_type() == AnimationStateType::DerivationNode {
            bail!("derivation node '{state_id}' cannot be forced as a State");
        }

        self.reset();
        self.current_state_id = state_id.to_string();
        self.forced_state_id = Some(state_id.to_string());
        Ok(())
    }

    /// Update the latest mouse position visible to graph runtime inputs.
    pub fn set_mouse_position(&mut self, position: MousePosition) {
        self.runtime_input.mouse_position = Some(position);
    }

    /// Advance the state machine by `dt` seconds and produce overrides.
    pub fn tick(&mut self, dt: f64, params: &ExternalParams, events: &FiredEvents) -> TickResult {
        if self.finished {
            return TickResult {
                overrides: self.derived_values.clone(),
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
        let forced = self.forced_state_id.is_some();
        self.scene_time += dt;
        self.trigger_holds.process_events(self.scene_time, events);
        let mut diagnostics: Vec<String> = Vec::new();

        // Logical state changes immediately on transition fire. Presentation
        // drivers retain the visual source independently, so both source and
        // target local clocks may still advance during a handoff.
        let current_id = self.current_state_id.clone();
        if self
            .find_state(&current_id)
            .is_some_and(|state| forced || state.resolved_type() != AnimationStateType::ExitState)
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

        let mut target_initialized_this_tick = false;

        // Establish the initial target system before routing. State S and
        // Mutation Q/Qdot commit atomically.
        if !self.logical_state_initialized {
            let mut working = self.motion_engine.clone();
            working.begin_mutation_frame();
            working.commit_logical_values(self.state_parameter_patch(&current_id));
            let writable_keys = self.state_mutation_writable_keys(&current_id);
            working.seed_targets_from_state(writable_keys.iter());
            match self.evaluate_state_mutation(&current_id, 0.0, &mut working) {
                Ok(_) => {
                    self.motion_engine = working;
                    self.logical_state_initialized = true;
                    target_initialized_this_tick = true;
                }
                Err(error) => diagnostics.push(format!(
                    "Mutation evaluation error during activation (state={current_id}): {error}"
                )),
            }
        }

        // Transitions remain interruptible while a previous visual driver is
        // active. Routing uses the logical current state (already the target
        // of the previous transition).
        if self.logical_state_initialized
            && !forced
            && let Some(transition) = self.pick_transition(params, events)
        {
            let previous = self.motion_engine.clone();
            let target_patch = self.state_parameter_patch(&transition.target);
            // State overrides alone select the authored Transition route.
            // Mutation Motion outputs establish Q before the residual is
            // created, but plain derived outputs never join this key set.
            let mut transition_keys = self
                .state_parameter_patch(&transition.source)
                .into_keys()
                .collect::<HashSet<_>>();
            transition_keys.extend(target_patch.keys().cloned());
            let graph = self
                .motion_graph_index
                .get(&transition.motion_graph_id)
                .and_then(|index| self.definition.motion_graphs.get(*index))
                .cloned();
            if let Some(graph) = graph {
                let mut target_engine = self.motion_engine.clone();
                target_engine.begin_mutation_frame();
                target_engine.commit_logical_values(target_patch);
                let writable_keys = self.state_mutation_writable_keys(&transition.target);
                target_engine.seed_targets_from_state(writable_keys.iter());
                match self.evaluate_state_mutation(&transition.target, 0.0, &mut target_engine) {
                    Ok(_) => {
                        target_engine.begin_transition_from(
                            &transition.id,
                            &graph,
                            &previous,
                            &transition_keys,
                        );
                        self.motion_engine = target_engine;
                        self.state_local_times
                            .insert(transition.target.clone(), 0.0);
                        self.current_state_id = transition.target;
                        // Advance the newly activated M once in the ordinary
                        // frame after its dt=0 activation transaction.
                        target_initialized_this_tick = false;
                    }
                    Err(error) => diagnostics.push(format!(
                        "Mutation evaluation rejected State activation (state={}): {error}",
                        transition.target
                    )),
                }
            }
        }

        let target_state_id = self.current_state_id.clone();

        // A failed ordinary Mutation frame discards every Q/Qdot/driver write.
        let mut working_motion = self.motion_engine.clone();
        if !target_initialized_this_tick {
            working_motion.begin_mutation_frame();
        }
        if self.logical_state_initialized && !target_initialized_this_tick {
            match self.evaluate_state_mutation(&target_state_id, dt, &mut working_motion) {
                Ok(_) => self.motion_engine = working_motion,
                Err(error) => {
                    diagnostics.push(format!(
                        "Mutation frame transaction rolled back (state={}): {error}",
                        target_state_id
                    ));
                }
            }
        }

        // Motion outputs have now produced Q. Transition advances residual
        // E/Edot and publishes physical P for independent motion channels.
        let motion_step = self.motion_engine.step(dt);

        // Derivation D observes only solved physical P plus explicit frame
        // inputs. It cannot mutate or roll back MotionEngine.
        let mut render_motion = self.motion_engine.clone();
        self.derived_values.clear();
        if let Some(derivation_id) = self
            .derivation_id_bound_to_state(&target_state_id)
            .map(str::to_string)
        {
            match self.evaluate_derivation(&derivation_id, &target_state_id, dt, &mut render_motion)
            {
                Ok(outputs) => {
                    let mut snapshot = HashMap::new();
                    extend_derivation_values(&mut snapshot, outputs);
                    self.derivation_snapshots
                        .insert(derivation_id.clone(), snapshot.clone());
                    self.derived_values = snapshot;
                }
                Err(error) => {
                    diagnostics.push(format!(
                        "Derivation evaluation failed (state={}, derivation={}): {error}",
                        target_state_id, derivation_id
                    ));
                    self.derived_values = self
                        .derivation_snapshots
                        .get(&derivation_id)
                        .cloned()
                        .unwrap_or_default();
                }
            }
        }

        // Exit becomes terminal only after its visual transition completes.
        if !forced && let Some(state) = self.find_state(&self.current_state_id) {
            if state.resolved_type() == AnimationStateType::ExitState
                && self.motion_engine.active_transition_id().is_none()
            {
                self.finished = true;
            }
        }

        TickResult {
            overrides: self.derived_values.clone(),
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

    /// Get the debug-forced state id, when routing is disabled.
    pub fn forced_state_id(&self) -> Option<&str> {
        self.forced_state_id.as_deref()
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

    fn state_param(&self, state_param_id: &str) -> Option<&StateParamDeclaration> {
        self.definition
            .state_params
            .iter()
            .find(|declaration| declaration.id == state_param_id)
    }

    fn state_parameter_patch(&self, state_id: &str) -> HashMap<StateParamKey, serde_json::Value> {
        let mut patch = HashMap::new();
        let Some(state) = self.find_state(state_id) else {
            return patch;
        };
        for (state_param_id, value) in &state.state_param_overrides {
            patch.insert(StateParamKey::new(state_param_id), value.clone());
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
            TransitionConditionBinding::Input { input } => self
                .resolve_transition_input(input, params)
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
                .and_then(|binding| self.resolve_transition_input(&binding.source, params))
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
        source: &StateValueSource,
        params: &ExternalParams,
    ) -> Option<ConditionValue> {
        match source {
            StateValueSource::FrameInput { frame_input_id }
                if frame_input_id == "sceneElapsedTime" =>
            {
                Some(ConditionValue::Number(self.scene_time))
            }
            StateValueSource::FrameInput { frame_input_id }
                if frame_input_id == "localElapsedTime" =>
            {
                Some(ConditionValue::Number(
                    self.state_local_times
                        .get(&self.current_state_id)
                        .copied()
                        .unwrap_or(0.0),
                ))
            }
            StateValueSource::FrameInput { frame_input_id }
                if frame_input_id == "mouse.position.x" =>
            {
                self.runtime_input
                    .mouse_position
                    .map(|position| ConditionValue::Number(position.x))
            }
            StateValueSource::FrameInput { frame_input_id }
                if frame_input_id == "mouse.position.y" =>
            {
                self.runtime_input
                    .mouse_position
                    .map(|position| ConditionValue::Number(position.y))
            }
            StateValueSource::FrameInput { frame_input_id } => params
                .get(frame_input_id)
                .and_then(condition_value_from_json),
            StateValueSource::StateParam { state_param_id } => self
                .motion_engine
                .current_values()
                .get(&StateParamKey::new(state_param_id))
                .and_then(condition_value_from_json),
        }
    }

    fn evaluate_state_mutation(
        &self,
        state_id: &str,
        dt: f64,
        motion_engine: &mut MotionEngine,
    ) -> Result<HashMap<String, GraphValue>> {
        if let Some(error) = &self.function_prepare_error {
            bail!("Graph Function preparation failed before playback: {error}");
        }
        let Some(state) = self.find_state(state_id) else {
            bail!("state '{state_id}' not found");
        };
        let Some(mutation_graph) = state.mutation_graph.as_ref() else {
            return Ok(HashMap::new());
        };
        let graph = mutation_graph.as_executable(state_id);

        // Mutation sees State S and explicit runtime inputs; it never observes
        // render Derivation values.
        let mut input_values: HashMap<String, GraphValue> = HashMap::new();
        for binding in &mutation_graph.input_bindings {
            let StateValueSource::StateParam { state_param_id } = &binding.source else {
                continue;
            };
            let key = StateParamKey::new(state_param_id);
            let Some(value) = motion_engine.state_value(&key) else {
                continue;
            };
            let port_type = self
                .state_param(state_param_id)
                .map(|declaration| declaration.param_type.as_str());
            if let Some(value) = GraphValue::from_json_typed(&value, port_type) {
                input_values.insert(state_param_id.clone(), value);
            }
        }
        let ctx = GraphInputContext {
            values: input_values,
            scene_elapsed_time: self.scene_time,
            local_elapsed_time: self.state_local_times.get(state_id).copied().unwrap_or(0.0),
            mouse_position: self.runtime_input.mouse_position,
            dt,
        };
        graph::evaluate_graph_with_motion_phase(
            &graph,
            &ctx,
            motion_engine,
            GraphEvaluationPhase::Target,
        )
    }

    fn evaluate_derivation(
        &self,
        derivation_id: &str,
        state_id: &str,
        dt: f64,
        motion_engine: &mut MotionEngine,
    ) -> Result<HashMap<String, GraphValue>> {
        if let Some(error) = &self.function_prepare_error {
            bail!("Graph Function preparation failed before playback: {error}");
        }
        let derivation = self
            .derivation_index
            .get(derivation_id)
            .and_then(|index| self.definition.derivations.get(*index))
            .ok_or_else(|| anyhow::anyhow!("Derivation '{derivation_id}' not found"))?;
        let mut input_values = HashMap::new();
        for source in derivation
            .input_bindings
            .iter()
            .map(|binding| &binding.source)
            .chain(
                derivation
                    .passthrough_bindings
                    .iter()
                    .map(|binding| &binding.source),
            )
        {
            let StateValueSource::StateParam { state_param_id } = source else {
                continue;
            };
            let key = StateParamKey::new(state_param_id);
            let Some(value) = motion_engine.physical_value(&key) else {
                continue;
            };
            let port_type = self
                .state_param(state_param_id)
                .map(|declaration| declaration.param_type.as_str());
            if let Some(value) = GraphValue::from_json_typed(&value, port_type) {
                input_values.insert(state_param_id.clone(), value);
            }
        }
        let ctx = GraphInputContext {
            values: input_values,
            scene_elapsed_time: self.scene_time,
            local_elapsed_time: self.state_local_times.get(state_id).copied().unwrap_or(0.0),
            mouse_position: self.runtime_input.mouse_position,
            dt,
        };
        graph::evaluate_graph_with_motion_phase(
            derivation,
            &ctx,
            motion_engine,
            GraphEvaluationPhase::Render,
        )
    }

    fn derivation_id_bound_to_state(&self, state_id: &str) -> Option<&str> {
        let binding = self
            .definition
            .derivation_bindings
            .iter()
            .find(|binding| binding.state_id == state_id)?;
        self.definition
            .states
            .iter()
            .find(|state| {
                state.id == binding.derivation_node_id
                    && state.state_type == AnimationStateType::DerivationNode
            })?
            .derivation_id
            .as_deref()
    }

    fn state_mutation_writable_keys(&self, state_id: &str) -> Vec<StateParamKey> {
        let Some(graph) = self
            .find_state(state_id)
            .and_then(|state| state.mutation_graph.as_ref())
        else {
            return Vec::new();
        };
        graph
            .nodes
            .iter()
            .filter(|node| node.node_type == GraphInnerNodeType::MutationFunction)
            .flat_map(|node| {
                node.outputs
                    .iter()
                    .filter(|port| port.motion == Some(true))
                    .filter_map(|port| {
                        graph
                            .output_bindings
                            .iter()
                            .find(|binding| {
                                binding.from.node_id == node.id && binding.from.port_id == port.id
                            })
                            .map(|binding| StateParamKey::new(&binding.state_param_id))
                    })
            })
            .collect()
    }
}

fn extend_derivation_values(
    target: &mut HashMap<OverrideKey, serde_json::Value>,
    outputs: HashMap<String, GraphValue>,
) {
    for (port_id, value) in outputs {
        for (key, value) in graph::expand_output_overrides(&port_id, &value) {
            target.insert(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_sm() -> StateMachine {
        StateMachine {
            id: "sm1".into(),
            name: "Test".into(),
            state_params: vec![],
            state_param_layout: Default::default(),
            states: vec![
                AnimationState {
                    id: "entry".into(),
                    name: "Entry".into(),
                    position: None,
                    state_param_overrides: Default::default(),
                    state_type: AnimationStateType::EntryState,
                    mutation_graph: None,
                    derivation_id: None,
                },
                AnimationState {
                    id: "any".into(),
                    name: "Any".into(),
                    position: None,
                    state_param_overrides: Default::default(),
                    state_type: AnimationStateType::AnyState,
                    mutation_graph: None,
                    derivation_id: None,
                },
                AnimationState {
                    id: "exit".into(),
                    name: "Exit".into(),
                    position: None,
                    state_param_overrides: Default::default(),
                    state_type: AnimationStateType::ExitState,
                    mutation_graph: None,
                    derivation_id: None,
                },
            ],
            transitions: vec![],
            derivation_bindings: vec![],
            derivations: vec![],
            motion_graphs: vec![
                instant_motion_graph("instant"),
                timeline_motion_graph("timeline", 0.3),
                timeline_motion_graph("timeline-1", 1.0),
            ],
            initial_state_id: Some("entry".into()),
            viewport: None,
        }
    }

    fn declare_state_param(
        sm: &mut StateMachine,
        id: &str,
        param_type: &str,
        default_value: serde_json::Value,
    ) {
        sm.state_params.push(StateParamDeclaration {
            id: id.into(),
            name: id.into(),
            param_type: param_type.into(),
            default_value,
            array_length: None,
        });
    }

    fn bind_derivation(sm: &mut StateMachine, state_id: &str, derivation_id: &str) {
        let derivation_node_id = format!("derivation_node_{state_id}");
        sm.states.push(AnimationState {
            id: derivation_node_id.clone(),
            name: format!("{state_id} Derivation"),
            position: None,
            state_param_overrides: Default::default(),
            state_type: AnimationStateType::DerivationNode,
            mutation_graph: None,
            derivation_id: Some(derivation_id.into()),
        });
        sm.derivation_bindings.push(DerivationStateBinding {
            id: format!("binding_{state_id}"),
            state_id: state_id.into(),
            derivation_node_id,
        });
    }

    fn motion_ports() -> (Vec<GraphPort>, Vec<GraphPort>) {
        let port = GraphPort {
            id: "*".into(),
            name: Some("Any".into()),
            port_type: Some("any".into()),
            array_length: None,
            motion: None,
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
                source: StateValueSource::StateParam {
                    state_param_id: "*".into(),
                },
                to: GraphEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
            }],
            output_bindings: vec![TransitionMotionOutputBinding {
                state_param_id: "*".into(),
                from: GraphEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
            }],
            passthrough_bindings: vec![],
            condition_binding: None,
            layout: None,
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
            from: GraphEndpoint {
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
        graph.inputs.push(GraphPort {
            id: input_port_id.into(),
            name: Some(input_port_id.into()),
            port_type: Some("bool".into()),
            array_length: None,
            motion: None,
        });
        graph.condition_binding = Some(TransitionConditionBinding::Input {
            input: StateValueSource::StateParam {
                state_param_id: input_port_id.into(),
            },
        });
        graph
    }

    fn with_event_and_bool_input_condition(
        mut graph: TransitionMotionGraph,
        event_type: &str,
        input_port_id: &str,
    ) -> TransitionMotionGraph {
        graph.inputs.push(GraphPort {
            id: input_port_id.into(),
            name: Some(input_port_id.into()),
            port_type: Some("bool".into()),
            array_length: None,
            motion: None,
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
        graph.connections.push(GraphConnection {
            id: "trigger-to-condition".into(),
            from: GraphEndpoint {
                node_id: "trigger".into(),
                port_id: "fired".into(),
            },
            to: GraphEndpoint {
                node_id: "condition".into(),
                port_id: "a".into(),
            },
        });
        graph.input_bindings.push(TransitionMotionInputBinding {
            source: StateValueSource::StateParam {
                state_param_id: input_port_id.into(),
            },
            to: GraphEndpoint {
                node_id: "condition".into(),
                port_id: "b".into(),
            },
        });
        graph.condition_binding = Some(TransitionConditionBinding::Node {
            from: GraphEndpoint {
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
        declare_state_param(
            &mut sm,
            "Node1:color",
            "color",
            serde_json::json!([0, 0, 0, 1]),
        );
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            state_param_overrides: [("Node1:color".into(), serde_json::json!([1, 0, 0, 1]))]
                .into_iter()
                .collect(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
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
        assert_eq!(
            rt.motion_engine
                .physical_value(&StateParamKey::new("Node1:color")),
            Some(serde_json::json!([1.0, 0.0, 0.0, 1.0]))
        );
        assert!(
            result.overrides.is_empty(),
            "no Derivation means no GPU writes"
        );
    }

    #[test]
    fn missing_override_keeps_previous_value_after_instant_transition() {
        let mut sm = minimal_sm();
        declare_state_param(&mut sm, "Node:x", "float", serde_json::json!(0.0));
        sm.states.push(AnimationState {
            id: "a".into(),
            name: "A".into(),
            position: None,
            state_param_overrides: [("Node:x".into(), serde_json::json!(5.0))]
                .into_iter()
                .collect(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
        });
        sm.states.push(AnimationState {
            id: "b".into(),
            name: "B".into(),
            position: None,
            state_param_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
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
            rt.motion_engine
                .physical_value(&StateParamKey::new("Node:x")),
            Some(serde_json::json!(5.0))
        );

        let b = rt.tick(0.016, &HashMap::new(), &vec!["go".into()]);
        assert_eq!(b.current_state_id, "b");
        assert_eq!(
            rt.motion_engine
                .physical_value(&StateParamKey::new("Node:x")),
            Some(serde_json::json!(5.0))
        );
        assert!(a.overrides.is_empty() && b.overrides.is_empty());
    }

    #[test]
    fn timed_transition_source_only_key_does_not_blend_to_zero() {
        let mut sm = minimal_sm();
        declare_state_param(&mut sm, "Node:x", "float", serde_json::json!(0.0));
        sm.states.push(AnimationState {
            id: "a".into(),
            name: "A".into(),
            position: None,
            state_param_overrides: [("Node:x".into(), serde_json::json!(5.0))]
                .into_iter()
                .collect(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
        });
        sm.states.push(AnimationState {
            id: "b".into(),
            name: "B".into(),
            position: None,
            state_param_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
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
            rt.motion_engine
                .physical_value(&StateParamKey::new("Node:x")),
            Some(serde_json::json!(5.0))
        );

        let blending = rt.tick(0.5, &HashMap::new(), &vec![]);
        assert_eq!(blending.current_state_id, "b");
        assert_eq!(
            rt.motion_engine
                .physical_value(&StateParamKey::new("Node:x")),
            Some(serde_json::json!(5.0))
        );

        let completed = rt.tick(0.6, &HashMap::new(), &vec![]);
        assert_eq!(completed.current_state_id, "b");
        assert_eq!(
            rt.motion_engine
                .physical_value(&StateParamKey::new("Node:x")),
            Some(serde_json::json!(5.0))
        );
        assert!(
            triggered.overrides.is_empty()
                && blending.overrides.is_empty()
                && completed.overrides.is_empty()
        );
    }

    #[test]
    fn timed_transition_advances() {
        let mut sm = minimal_sm();
        declare_state_param(&mut sm, "Node:x", "float", serde_json::json!(0.0));
        sm.states
            .iter_mut()
            .find(|state| state.id == "entry")
            .unwrap()
            .state_param_overrides
            .insert("Node:x".into(), serde_json::json!(0.0));
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            state_param_overrides: [("Node:x".into(), serde_json::json!(1.0))]
                .into_iter()
                .collect(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
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
            rt.motion_engine
                .physical_value(&StateParamKey::new("Node:x")),
            Some(serde_json::json!(0.5))
        );
        assert!(r1.overrides.is_empty());

        let r2 = rt.tick(1.1, &HashMap::new(), &vec![]);
        // Transition complete.
        assert_eq!(r2.current_state_id, "s1");
        assert_eq!(r2.active_transition_id, None);
    }

    #[test]
    fn bool_condition() {
        let mut sm = minimal_sm();
        declare_state_param(&mut sm, "flag", "bool", serde_json::json!(false));
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            state_param_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
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

        let mut rt = StateMachineRuntime::new(sm.clone());

        // Without param → no transition.
        let r1 = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(r1.current_state_id, "entry");

        // With a true State Param default → transition.
        sm.state_params[0].default_value = serde_json::json!(true);
        let mut rt = StateMachineRuntime::new(sm);
        let r2 = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(r2.current_state_id, "s1");
    }

    #[test]
    fn event_condition() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            state_param_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
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
            id: "derived".into(),
            name: "Derived".into(),
            position: None,
            state_param_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
        });
        sm.derivations.push(DerivationDefinition {
            id: "d1".into(),
            name: "Derivation".into(),
            inputs: vec![],
            outputs: vec![],
            nodes: vec![],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![],
            passthrough_bindings: vec![],
            layout: None,
            viewport: None,
        });
        bind_derivation(&mut sm, "derived", "d1");
        sm.motion_graphs.push(with_event_condition(
            timeline_motion_graph("mousedown-condition", 0.3),
            "mousedown",
        ));
        sm.transitions.push(AnimationTransition {
            id: "tr_any_derived".into(),
            source: "any".into(),
            target: "derived".into(),
            motion_graph_id: "mousedown-condition".into(),
        });

        let mut rt = StateMachineRuntime::new(sm);

        let idle = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(idle.current_state_id, "entry");
        assert_eq!(idle.active_transition_id, None);

        let triggered = rt.tick(0.016, &HashMap::new(), &vec!["mousedown".into()]);
        assert_eq!(triggered.current_state_id, "derived");
        assert_eq!(triggered.active_transition_id, None);

        let completed = rt.tick(0.4, &HashMap::new(), &vec![]);
        assert_eq!(completed.current_state_id, "derived");
    }

    #[test]
    fn explicit_passthrough_is_render_derived_without_a_motion_channel() {
        let mut sm = minimal_sm();
        sm.states
            .iter_mut()
            .find(|state| state.id == "entry")
            .unwrap()
            .state_param_overrides
            .insert("Node:x".into(), serde_json::json!(0.0));
        sm.states.push(AnimationState {
            id: "dynamic".into(),
            name: "Dynamic".into(),
            position: None,
            state_param_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
        });
        sm.derivations.push(DerivationDefinition {
            id: "dynamic_target".into(),
            name: "Dynamic Target".into(),
            inputs: vec![GraphPort {
                id: "localElapsedTime".into(),
                name: Some("Local Elapsed Time".into()),
                port_type: Some("float".into()),
                array_length: None,
                motion: None,
            }],
            outputs: vec![GraphPort {
                id: "Node:derived".into(),
                name: Some("Node.derived".into()),
                port_type: Some("float".into()),
                array_length: None,
                motion: None,
            }],
            nodes: vec![],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![],
            passthrough_bindings: vec![DerivationPassthroughBinding {
                source: StateValueSource::FrameInput {
                    frame_input_id: "localElapsedTime".into(),
                },
                uniform: GpuUniformRef {
                    node_id: "Node".into(),
                    param_id: "derived".into(),
                },
            }],
            layout: None,
            viewport: None,
        });
        bind_derivation(&mut sm, "dynamic", "dynamic_target");
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
            entered.overrides.get(&OverrideKey::new("Node", "derived")),
            Some(&serde_json::json!(0.0))
        );

        let advancing = runtime.tick(0.1, &HashMap::new(), &vec![]);
        let value = advancing
            .overrides
            .get(&OverrideKey::new("Node", "derived"))
            .and_then(serde_json::Value::as_f64)
            .expect("Derivation passthrough output");
        assert!((value - 0.1).abs() < 1e-8, "value={value}");
        assert_eq!(advancing.state_local_times.get("dynamic"), Some(&0.1));
        assert!(
            advancing
                .motion_channels
                .iter()
                .all(|channel| channel.key != "Node:derived")
        );
    }

    #[test]
    fn transition_source_reads_motion_p_not_render_derived_overlay() {
        let mut sm = minimal_sm();
        declare_state_param(&mut sm, "Node:x", "float", serde_json::json!(0.0));
        sm.states.push(AnimationState {
            id: "a".into(),
            name: "A".into(),
            position: None,
            state_param_overrides: [("Node:x".into(), serde_json::json!(0.0))]
                .into_iter()
                .collect(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
        });
        sm.states.push(AnimationState {
            id: "b".into(),
            name: "B".into(),
            position: None,
            state_param_overrides: [("Node:x".into(), serde_json::json!(10.0))]
                .into_iter()
                .collect(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
        });
        sm.derivations.push(DerivationDefinition {
            id: "a_derivation".into(),
            name: "A Derivation".into(),
            inputs: vec![GraphPort {
                id: "localElapsedTime".into(),
                name: None,
                port_type: Some("float".into()),
                array_length: None,
                motion: None,
            }],
            outputs: vec![GraphPort {
                id: "Node:x".into(),
                name: None,
                port_type: Some("float".into()),
                array_length: None,
                motion: None,
            }],
            nodes: vec![],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![],
            passthrough_bindings: vec![DerivationPassthroughBinding {
                source: StateValueSource::FrameInput {
                    frame_input_id: "localElapsedTime".into(),
                },
                uniform: GpuUniformRef {
                    node_id: "Node".into(),
                    param_id: "x".into(),
                },
            }],
            layout: None,
            viewport: None,
        });
        bind_derivation(&mut sm, "a", "a_derivation");
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
        let interrupted_x = runtime
            .motion_engine
            .physical_value(&StateParamKey::new("Node:x"))
            .and_then(|value| value.as_f64())
            .expect("MotionEngine P");
        assert!((interrupted_x - 0.0).abs() <= 1.0e-9);
    }

    #[test]
    fn active_state_derivation_reads_the_transitioned_physical_snapshot() {
        fn derivation(id: &str, seen_output: &str, conflict_value: f64) -> DerivationDefinition {
            DerivationDefinition {
                id: id.into(),
                name: id.into(),
                inputs: vec![GraphPort {
                    id: "Node:x".into(),
                    name: None,
                    port_type: Some("float".into()),
                    array_length: None,
                    motion: None,
                }],
                outputs: vec![
                    GraphPort {
                        id: seen_output.into(),
                        name: None,
                        port_type: Some("float".into()),
                        array_length: None,
                        motion: None,
                    },
                    GraphPort {
                        id: "Node:conflict".into(),
                        name: None,
                        port_type: Some("float".into()),
                        array_length: None,
                        motion: None,
                    },
                ],
                nodes: vec![GraphInnerNode {
                    id: "constant".into(),
                    node_type: GraphInnerNodeType::FloatInput,
                    params: HashMap::from([("value".into(), serde_json::json!(conflict_value))]),
                    inputs: vec![],
                    outputs: vec![GraphPort {
                        id: "value".into(),
                        name: None,
                        port_type: Some("float".into()),
                        array_length: None,
                        motion: None,
                    }],
                }],
                connections: vec![],
                input_bindings: vec![],
                output_bindings: vec![DerivationOutputBinding {
                    uniform: GpuUniformRef {
                        node_id: "Node".into(),
                        param_id: "conflict".into(),
                    },
                    from: GraphEndpoint {
                        node_id: "constant".into(),
                        port_id: "value".into(),
                    },
                }],
                passthrough_bindings: vec![DerivationPassthroughBinding {
                    source: StateValueSource::StateParam {
                        state_param_id: "Node:x".into(),
                    },
                    uniform: GpuUniformRef {
                        node_id: seen_output.split_once(':').unwrap().0.into(),
                        param_id: seen_output.split_once(':').unwrap().1.into(),
                    },
                }],
                layout: None,
                viewport: None,
            }
        }

        let mut sm = minimal_sm();
        declare_state_param(&mut sm, "Node:x", "float", serde_json::json!(0.0));
        sm.states
            .iter_mut()
            .find(|state| state.id == "entry")
            .unwrap()
            .state_param_overrides
            .insert("Node:x".into(), serde_json::json!(0.0));
        sm.states.push(AnimationState {
            id: "target".into(),
            name: "Target".into(),
            position: None,
            state_param_overrides: [("Node:x".into(), serde_json::json!(10.0))]
                .into_iter()
                .collect(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
        });
        sm.derivations
            .push(derivation("target_derivation", "Node:targetSeen", 2.0));
        bind_derivation(&mut sm, "target", "target_derivation");
        sm.transitions.push(AnimationTransition {
            id: "entry_to_target".into(),
            source: "entry".into(),
            target: "target".into(),
            motion_graph_id: "timeline-1".into(),
        });

        let result = StateMachineRuntime::new(sm).tick(0.5, &HashMap::new(), &vec![]);
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
    fn mouse_passthrough_is_render_derived_without_motion_channels() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "derived".into(),
            name: "Derived".into(),
            position: None,
            state_param_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
        });
        sm.derivations.push(DerivationDefinition {
            id: "d_mouse".into(),
            name: "Mouse Derivation".into(),
            inputs: vec![],
            outputs: vec![
                GraphPort {
                    id: "MouseX:value".into(),
                    name: Some("MouseX.value".into()),
                    port_type: Some("float".into()),
                    array_length: None,
                    motion: None,
                },
                GraphPort {
                    id: "MouseY:value".into(),
                    name: Some("MouseY.value".into()),
                    port_type: Some("float".into()),
                    array_length: None,
                    motion: None,
                },
            ],
            nodes: vec![],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![],
            passthrough_bindings: vec![
                DerivationPassthroughBinding {
                    source: StateValueSource::FrameInput {
                        frame_input_id: "mouse.position.x".into(),
                    },
                    uniform: GpuUniformRef {
                        node_id: "MouseX".into(),
                        param_id: "value".into(),
                    },
                },
                DerivationPassthroughBinding {
                    source: StateValueSource::FrameInput {
                        frame_input_id: "mouse.position.y".into(),
                    },
                    uniform: GpuUniformRef {
                        node_id: "MouseY".into(),
                        param_id: "value".into(),
                    },
                },
            ],
            layout: None,
            viewport: None,
        });
        bind_derivation(&mut sm, "derived", "d_mouse");
        sm.motion_graphs.push(with_event_condition(
            instant_motion_graph("mousedown-instant"),
            "mousedown",
        ));
        sm.transitions.push(AnimationTransition {
            id: "entry_to_mouse".into(),
            source: "entry".into(),
            target: "derived".into(),
            motion_graph_id: "mousedown-instant".into(),
        });

        let mut rt = StateMachineRuntime::new(sm);
        rt.set_mouse_position(MousePosition { x: 321.0, y: 654.0 });

        let result = rt.tick(0.016, &HashMap::new(), &vec!["mousedown".into()]);

        assert_eq!(result.current_state_id, "derived");
        assert_eq!(
            result.overrides.get(&OverrideKey::new("MouseX", "value")),
            Some(&serde_json::json!(321.0))
        );
        assert_eq!(
            result.overrides.get(&OverrideKey::new("MouseY", "value")),
            Some(&serde_json::json!(654.0))
        );
        assert!(
            result
                .motion_channels
                .iter()
                .all(|channel| channel.key != "MouseX:value")
        );
        assert!(
            result
                .motion_channels
                .iter()
                .all(|channel| channel.key != "MouseY:value")
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
            state_param_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
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
        declare_state_param(&mut sm, "ready", "bool", serde_json::json!(false));
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            state_param_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
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

        let mut rt = StateMachineRuntime::new(sm.clone());

        // Neither trigger nor condition → stays.
        let r1 = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(r1.current_state_id, "entry");

        // Trigger fires but condition not met → stays.
        let r2 = rt.tick(0.016, &HashMap::new(), &vec!["go".into()]);
        assert_eq!(r2.current_state_id, "entry");

        // A true condition without the trigger still stays.
        sm.state_params[0].default_value = serde_json::json!(true);
        let mut rt = StateMachineRuntime::new(sm);
        let r3 = rt.tick(0.016, &HashMap::new(), &vec![]);
        assert_eq!(r3.current_state_id, "entry");

        // Both trigger and condition → transitions.
        let r4 = rt.tick(0.016, &HashMap::new(), &vec!["go".into()]);
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
            from: GraphEndpoint {
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
            GraphConnection {
                id: "holding-time".into(),
                from: GraphEndpoint {
                    node_id: "down".into(),
                    port_id: "holdingTime".into(),
                },
                to: GraphEndpoint {
                    node_id: "held-long-enough".into(),
                    port_id: "a".into(),
                },
            },
            GraphConnection {
                id: "threshold".into(),
                from: GraphEndpoint {
                    node_id: "threshold".into(),
                    port_id: "value".into(),
                },
                to: GraphEndpoint {
                    node_id: "held-long-enough".into(),
                    port_id: "b".into(),
                },
            },
        ]);
        graph.condition_binding = Some(TransitionConditionBinding::Node {
            from: GraphEndpoint {
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
        graph.inputs.push(GraphPort {
            id: "mouse.position.x".into(),
            name: Some("Mouse Position X".into()),
            port_type: Some("float".into()),
            array_length: None,
            motion: None,
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
                source: StateValueSource::FrameInput {
                    frame_input_id: "mouse.position.x".into(),
                },
                to: GraphEndpoint {
                    node_id: node_id.into(),
                    port_id: "a".into(),
                },
            });
        }
        graph.connections.extend([
            GraphConnection {
                id: "lower-gte".into(),
                from: GraphEndpoint {
                    node_id: "lower".into(),
                    port_id: "value".into(),
                },
                to: GraphEndpoint {
                    node_id: "gte".into(),
                    port_id: "b".into(),
                },
            },
            GraphConnection {
                id: "upper-lte".into(),
                from: GraphEndpoint {
                    node_id: "upper".into(),
                    port_id: "value".into(),
                },
                to: GraphEndpoint {
                    node_id: "lte".into(),
                    port_id: "b".into(),
                },
            },
            GraphConnection {
                id: "gte-inside".into(),
                from: GraphEndpoint {
                    node_id: "gte".into(),
                    port_id: "result".into(),
                },
                to: GraphEndpoint {
                    node_id: "inside".into(),
                    port_id: "a".into(),
                },
            },
            GraphConnection {
                id: "lte-inside".into(),
                from: GraphEndpoint {
                    node_id: "lte".into(),
                    port_id: "result".into(),
                },
                to: GraphEndpoint {
                    node_id: "inside".into(),
                    port_id: "b".into(),
                },
            },
        ]);
        graph.condition_binding = Some(TransitionConditionBinding::Node {
            from: GraphEndpoint {
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

    #[test]
    fn forced_states_disable_routing_and_exit_completion() {
        let mut sm = minimal_sm();
        sm.transitions.push(AnimationTransition {
            id: "entry-to-exit".into(),
            source: "entry".into(),
            target: "exit".into(),
            motion_graph_id: "instant".into(),
        });

        let mut runtime = StateMachineRuntime::new(sm);
        runtime.force_state("entry").unwrap();
        let entry = runtime.tick(0.5, &HashMap::new(), &vec![]);
        assert_eq!(entry.current_state_id, "entry");
        assert_eq!(entry.state_local_times.get("entry"), Some(&0.5));
        assert!(!entry.finished);

        runtime.force_state("exit").unwrap();
        let exit = runtime.tick(0.5, &HashMap::new(), &vec![]);
        assert_eq!(exit.current_state_id, "exit");
        assert_eq!(exit.state_local_times.get("exit"), Some(&0.5));
        assert!(!exit.finished);

        runtime.force_state("any").unwrap();
        let any = runtime.tick(0.25, &HashMap::new(), &vec![]);
        assert_eq!(any.current_state_id, "any");
        assert_eq!(any.state_local_times.get("any"), Some(&0.25));
    }

    #[test]
    fn forced_state_resets_to_base_and_rejects_derivation_nodes() {
        let mut sm = minimal_sm();
        sm.state_params.push(StateParamDeclaration {
            id: "visibility".into(),
            name: "Visibility".into(),
            param_type: "float".into(),
            default_value: serde_json::json!(0.0),
            array_length: None,
        });
        sm.states.push(AnimationState {
            id: "visible".into(),
            name: "Visible".into(),
            position: None,
            state_param_overrides: HashMap::from([("visibility".into(), serde_json::json!(1.0))]),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: None,
            derivation_id: None,
        });
        sm.states.push(AnimationState {
            id: "derivation-node".into(),
            name: "Derivation".into(),
            position: None,
            state_param_overrides: HashMap::new(),
            state_type: AnimationStateType::DerivationNode,
            mutation_graph: None,
            derivation_id: Some("derivation".into()),
        });
        let key = StateParamKey::new("visibility");
        let mut runtime = StateMachineRuntime::with_initial_values(
            sm,
            HashMap::from([(key.clone(), serde_json::json!(0.0))]),
        );

        runtime.force_state("visible").unwrap();
        runtime.tick(0.0, &HashMap::new(), &vec![]);
        assert_eq!(
            runtime.motion_engine.physical_value(&key),
            Some(serde_json::json!(1.0))
        );

        runtime.force_state("entry").unwrap();
        runtime.tick(0.0, &HashMap::new(), &vec![]);
        assert_eq!(
            runtime.motion_engine.physical_value(&key),
            Some(serde_json::json!(0.0))
        );
        assert!(runtime.force_state("derivation-node").is_err());
        assert!(runtime.force_state("missing").is_err());
    }

    #[test]
    fn render_only_outputs_never_enter_motion_engine() {
        let mut sm = minimal_sm();
        let render_key = OverrideKey::new("ComputedOutput", "value");
        sm.derivations.push(DerivationDefinition {
            id: "computed-output".into(),
            name: "Computed output".into(),
            inputs: vec![],
            outputs: vec![GraphPort {
                id: "ComputedOutput:value".into(),
                name: Some("Computed output".into()),
                port_type: Some("float".into()),
                array_length: None,
                motion: None,
            }],
            nodes: vec![GraphInnerNode {
                id: "constant".into(),
                node_type: GraphInnerNodeType::FloatInput,
                params: HashMap::from([("value".into(), serde_json::json!(1.0))]),
                inputs: vec![],
                outputs: vec![GraphPort {
                    id: "value".into(),
                    name: Some("Value".into()),
                    port_type: Some("float".into()),
                    array_length: None,
                    motion: None,
                }],
            }],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![DerivationOutputBinding {
                uniform: GpuUniformRef {
                    node_id: "ComputedOutput".into(),
                    param_id: "value".into(),
                },
                from: GraphEndpoint {
                    node_id: "constant".into(),
                    port_id: "value".into(),
                },
            }],
            passthrough_bindings: vec![],
            layout: None,
            viewport: None,
        });
        let runtime = StateMachineRuntime::with_initial_values(sm, HashMap::new());
        let accidental_state_key = StateParamKey::new("ComputedOutput:value");

        assert_eq!(
            runtime
                .motion_engine
                .current_values()
                .get(&accidental_state_key),
            None
        );
        assert_eq!(render_key, OverrideKey::new("ComputedOutput", "value"));
    }
}
