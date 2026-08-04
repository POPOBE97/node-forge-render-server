//! Per-property motion drivers used by state-machine transitions.
//!
//! Springs use the closed-form solution of the damped oscillator. A render
//! frame advances every driver exactly once with the full frame delta.

use std::collections::HashMap;
use std::collections::HashSet;
use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

use super::easing::ease;
#[cfg(test)]
use super::types::StateValueSource;
use super::types::{
    EasingKind, StateParamKey, TimelineBlending, TimelinePreset, TransitionMotionGraph,
    TransitionMotionNode,
};

const ANY_CHANNEL: &str = "*";
const NANOS_PER_SECOND: f64 = 1_000_000_000.0;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MotionChannelDebug {
    pub key: String,
    pub driver: String,
    pub state_value: Vec<f64>,
    pub value: Vec<f64>,
    pub velocity: Vec<f64>,
    pub target_value: Vec<f64>,
    pub target_velocity: Vec<f64>,
    pub transition_error: Vec<f64>,
    pub transition_error_velocity: Vec<f64>,
    pub mutation_driver: String,
    pub transition_driver: String,
    pub timeline_progress: Option<f64>,
    pub blending_progress: Option<f64>,
    pub completed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct MotionStep {
    pub overrides: HashMap<StateParamKey, serde_json::Value>,
    pub channels: Vec<MotionChannelDebug>,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct MotionEngine {
    channels: HashMap<StateParamKey, Channel>,
    active_transition_id: Option<String>,
    initial_values: HashMap<StateParamKey, serde_json::Value>,
    state_values: HashMap<StateParamKey, serde_json::Value>,
    current_values: HashMap<StateParamKey, serde_json::Value>,
    publication_types: HashMap<StateParamKey, String>,
    mutation_frame_calls: HashSet<StateParamKey>,
}

impl MotionEngine {
    pub fn new() -> Self {
        Self::with_initial_values(HashMap::new())
    }

    pub fn with_initial_values(initial_values: HashMap<StateParamKey, serde_json::Value>) -> Self {
        Self::with_initial_values_and_types(initial_values, HashMap::new())
    }

    pub fn with_initial_values_and_types(
        initial_values: HashMap<StateParamKey, serde_json::Value>,
        publication_types: HashMap<StateParamKey, String>,
    ) -> Self {
        Self {
            channels: HashMap::new(),
            active_transition_id: None,
            state_values: initial_values.clone(),
            current_values: initial_values.clone(),
            initial_values,
            publication_types,
            mutation_frame_calls: HashSet::new(),
        }
    }

    pub fn reset(&mut self) {
        self.channels.clear();
        self.active_transition_id = None;
        self.mutation_frame_calls.clear();
        self.state_values.clone_from(&self.initial_values);
        self.current_values.clone_from(&self.initial_values);
    }

    pub fn active_transition_id(&self) -> Option<&str> {
        self.active_transition_id.as_deref()
    }

    pub fn begin_mutation_frame(&mut self) {
        self.mutation_frame_calls.clear();
    }

    /// Submit a new property animation transaction.
    ///
    /// The caller supplies logical target values only. Presentation sources
    /// are always taken from the engine-owned current-value store, while an
    /// interrupted channel contributes velocity when that velocity is still
    /// meaningful.
    pub fn transition_to(
        &mut self,
        transition_id: &str,
        target: &HashMap<StateParamKey, serde_json::Value>,
        graph: &TransitionMotionGraph,
    ) {
        let previous = self.clone();
        let transition_keys = target.keys().cloned().collect();
        self.commit_logical_values(target.clone());
        self.begin_transition_from(transition_id, graph, &previous, &transition_keys);
    }

    /// Establish transition error after the target State and its Motion
    /// outputs have produced Q/Qdot. `transition_keys` contains only
    /// declarations authored as overrides on the Transition endpoints.
    pub fn begin_transition_from(
        &mut self,
        transition_id: &str,
        graph: &TransitionMotionGraph,
        previous: &MotionEngine,
        transition_keys: &HashSet<StateParamKey>,
    ) {
        let plans = compile_channel_plans(graph);
        let mut keys = transition_keys.iter().cloned().collect::<Vec<_>>();
        keys.sort_by(|left, right| key_string(left).cmp(&key_string(right)));

        for key in keys {
            let previous_sample = previous
                .channels
                .get(&key)
                .map(Channel::sample)
                .or_else(|| {
                    previous
                        .current_values
                        .get(&key)
                        .cloned()
                        .map(Channel::hold)
                        .map(|channel| channel.sample())
                });
            let Some(previous_sample) = previous_sample else {
                continue;
            };
            let channel = self
                .channels
                .entry(key.clone())
                .or_insert_with(|| Channel::hold(previous_sample.value.to_json()));
            let plan = plans
                .specific
                .get(&key_string(&key))
                .or(plans.fallback.as_ref())
                .cloned()
                .unwrap_or(MotionPlan::Instant);
            channel.start_error(previous_sample, plan);
            let value = publish_numeric_value(
                self.publication_types.get(&key).map(String::as_str),
                &channel.sample().value,
            );
            self.current_values.insert(key, value);
        }

        // Motion channels outside the authored State route do not participate
        // in this Transition and must not retain stale residual error.
        for (key, channel) in &mut self.channels {
            if !transition_keys.contains(key) {
                channel.finish_transition();
                let value = publish_numeric_value(
                    self.publication_types.get(key).map(String::as_str),
                    &channel.sample().value,
                );
                self.current_values.insert(key.clone(), value);
            }
        }
        self.active_transition_id = Some(transition_id.to_string());
    }

    /// Compatibility helper for low-level motion tests. Runtime code should
    /// use [`Self::transition_to`] so source ownership remains inside the
    /// animation engine.
    pub fn start_transition(
        &mut self,
        transition_id: &str,
        graph: &TransitionMotionGraph,
        source: &HashMap<StateParamKey, serde_json::Value>,
        target: &HashMap<StateParamKey, serde_json::Value>,
        sticky: &HashMap<StateParamKey, serde_json::Value>,
    ) {
        for (key, value) in sticky.iter().chain(source.iter()) {
            self.current_values.insert(key.clone(), value.clone());
        }
        self.transition_to(transition_id, target, graph);
    }

    /// Resolve State override values into S and seed Q from S. Callers may
    /// subsequently run Mutation to replace or integrate Q before Transition
    /// residual error is established.
    pub fn commit_logical_values(&mut self, patch: HashMap<StateParamKey, serde_json::Value>) {
        for (key, value) in patch {
            self.state_values.insert(key.clone(), value.clone());
            let channel = self
                .channels
                .entry(key.clone())
                .or_insert_with(|| Channel::hold(value.clone()));
            channel.set_static_target(value);
            let value = publish_numeric_value(
                self.publication_types.get(&key).map(String::as_str),
                &channel.sample().value,
            );
            self.current_values.insert(key, value);
        }
    }

    /// Re-seed writable target-system channels from their resolved State
    /// values when a State activates. Ordinary frames never call this, so a
    /// continuously retargeted Mutation spring is not recreated every tick.
    pub fn seed_targets_from_state<'a>(
        &mut self,
        keys: impl IntoIterator<Item = &'a StateParamKey>,
    ) {
        for key in keys {
            let Some(value) = self.state_values.get(key).cloned() else {
                continue;
            };
            let channel = self
                .channels
                .entry(key.clone())
                .or_insert_with(|| Channel::hold(value.clone()));
            channel.set_static_target(value);
            let value = publish_numeric_value(
                self.publication_types.get(key).map(String::as_str),
                &channel.sample().value,
            );
            self.current_values.insert(key.clone(), value);
        }
    }

    /// Override the readable physical snapshot without changing target
    /// channels. Retained for low-level MotionEngine tests; Mutation Function
    /// inputs read resolved S and therefore do not use this snapshot.
    pub fn use_physical_inputs_from(&mut self, previous: &MotionEngine) {
        self.current_values.clone_from(&previous.current_values);
    }

    /// Directly set a Mutation target channel. This updates the target-system
    /// sample Q/Qdot; the presentation sample remains Q - transition error.
    pub fn set_to(
        &mut self,
        key: &StateParamKey,
        target: serde_json::Value,
        velocity: Option<serde_json::Value>,
        dt: f64,
    ) -> anyhow::Result<serde_json::Value> {
        if !self.mutation_frame_calls.insert(key.clone()) {
            anyhow::bail!(
                "Motion output '{}' was applied more than once this frame",
                key_string(key)
            );
        }
        let fallback = self
            .current_values
            .get(key)
            .cloned()
            .unwrap_or_else(|| target.clone());
        let channel = self
            .channels
            .entry(key.clone())
            .or_insert_with(|| Channel::hold(fallback));
        channel.set_to(target, velocity, kotlin_frame_seconds(dt))?;
        let sample = channel.sample();
        let published = publish_numeric_value(
            self.publication_types.get(key).map(String::as_str),
            &sample.value,
        );
        self.current_values.insert(key.clone(), published);
        Ok(publish_numeric_value(
            self.publication_types.get(key).map(String::as_str),
            &channel.target_sample().value,
        ))
    }

    /// Retarget a Mutation-owned spring without replacing its current
    /// position or velocity.
    pub fn to(
        &mut self,
        key: &StateParamKey,
        target: serde_json::Value,
        duration: f64,
        bounce: f64,
        dt: f64,
    ) -> anyhow::Result<serde_json::Value> {
        if !self.mutation_frame_calls.insert(key.clone()) {
            anyhow::bail!(
                "Motion output '{}' was applied more than once this frame",
                key_string(key)
            );
        }
        let fallback = self
            .current_values
            .get(key)
            .cloned()
            .unwrap_or_else(|| target.clone());
        let channel = self
            .channels
            .entry(key.clone())
            .or_insert_with(|| Channel::hold(fallback));
        channel.to(target, duration, bounce, kotlin_frame_seconds(dt))?;
        let sample = channel.sample();
        let published = publish_numeric_value(
            self.publication_types.get(key).map(String::as_str),
            &sample.value,
        );
        self.current_values.insert(key.clone(), published);
        Ok(publish_numeric_value(
            self.publication_types.get(key).map(String::as_str),
            &channel.target_sample().value,
        ))
    }

    /// Update global uniform values from outside the state machine. An active
    /// animation transaction retains priority until its channel completes.
    pub fn update_external_values(&mut self, updates: &[(StateParamKey, serde_json::Value)]) {
        for (key, value) in updates {
            self.initial_values.insert(key.clone(), value.clone());
            self.state_values.insert(key.clone(), value.clone());
            let transaction_is_active = self
                .channels
                .get(key)
                .is_some_and(Channel::transition_active);
            if !transaction_is_active {
                let channel = self
                    .channels
                    .entry(key.clone())
                    .or_insert_with(|| Channel::hold(value.clone()));
                channel.set_static_target(value.clone());
                let value = publish_numeric_value(
                    self.publication_types.get(key).map(String::as_str),
                    &channel.sample().value,
                );
                self.current_values.insert(key.clone(), value);
            }
        }
    }

    pub fn current_values(&self) -> &HashMap<StateParamKey, serde_json::Value> {
        &self.current_values
    }

    pub fn state_value(&self, key: &StateParamKey) -> Option<serde_json::Value> {
        self.state_values
            .get(key)
            .cloned()
            .or_else(|| self.initial_values.get(key).cloned())
    }

    pub fn target_value(&self, key: &StateParamKey) -> Option<serde_json::Value> {
        self.channels
            .get(key)
            .map(|channel| {
                publish_numeric_value(
                    self.publication_types.get(key).map(String::as_str),
                    &channel.target_sample().value,
                )
            })
            .or_else(|| self.current_values.get(key).cloned())
    }

    pub fn target_velocity(&self, key: &StateParamKey) -> Option<serde_json::Value> {
        self.channels
            .get(key)
            .map(|channel| channel.target_sample().velocity.to_json())
    }

    pub fn physical_value(&self, key: &StateParamKey) -> Option<serde_json::Value> {
        self.current_values.get(key).cloned()
    }

    pub fn physical_velocity(&self, key: &StateParamKey) -> Option<serde_json::Value> {
        self.channels
            .get(key)
            .map(|channel| channel.sample().velocity.to_json())
    }

    pub fn step(&mut self, dt: f64) -> MotionStep {
        let dt = kotlin_frame_seconds(dt);
        let mut result = MotionStep::default();
        let mut all_completed = true;

        for (key, channel) in &mut self.channels {
            channel.step(dt);
            let sample = channel.sample();
            let target = channel.target_sample();
            let error = channel.error_sample();
            let state_value = self
                .state_values
                .get(key)
                .and_then(NumericValue::from_json)
                .map(|value| value.components().to_vec())
                .unwrap_or_default();
            let value = publish_numeric_value(
                self.publication_types.get(key).map(String::as_str),
                &sample.value,
            );
            self.current_values.insert(key.clone(), value.clone());
            result.overrides.insert(key.clone(), value);
            result.channels.push(MotionChannelDebug {
                key: key_string(key),
                driver: sample.driver.to_string(),
                state_value,
                value: sample.value.components().to_vec(),
                velocity: sample.velocity.components().to_vec(),
                target_value: target.value.components().to_vec(),
                target_velocity: target.velocity.components().to_vec(),
                transition_error: error.value.components().to_vec(),
                transition_error_velocity: error.velocity.components().to_vec(),
                mutation_driver: target.driver.to_string(),
                transition_driver: error.driver.to_string(),
                timeline_progress: sample.timeline_progress,
                blending_progress: sample.blending_progress,
                completed: sample.completed,
            });
            all_completed &= error.completed;
            if error.completed {
                channel.finish_transition();
            }
        }
        result.channels.sort_by(|a, b| a.key.cmp(&b.key));
        result.active = self.active_transition_id.is_some() && !all_completed;
        if all_completed {
            self.active_transition_id = None;
            for channel in self.channels.values_mut() {
                channel.finish_transition();
            }
        }
        result.overrides.clone_from(&self.current_values);
        result
    }
}

fn kotlin_frame_seconds(dt: f64) -> f64 {
    if !dt.is_finite() || dt <= 0.0 {
        return 0.0;
    }
    let dt_float = dt as f32;
    let nanos = (f64::from(dt_float) * NANOS_PER_SECOND) as i64;
    f64::from((nanos as f64 / NANOS_PER_SECOND) as f32)
}

fn key_string(key: &StateParamKey) -> String {
    key.as_str().to_string()
}

fn publish_numeric_value(port_type: Option<&str>, value: &NumericValue) -> serde_json::Value {
    let value = value.to_json();
    if !matches!(port_type, Some("int" | "packed<int>")) {
        return value;
    }
    round_json_numbers(value)
}

fn round_json_numbers(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Number(number) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .map(|value| serde_json::json!(value.round() as i64))
            .unwrap_or(serde_json::Value::Number(number)),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(round_json_numbers).collect())
        }
        value => value,
    }
}

#[derive(Debug, Clone)]
struct CompiledPlans {
    fallback: Option<MotionPlan>,
    specific: HashMap<String, MotionPlan>,
}

fn compile_channel_plans(graph: &TransitionMotionGraph) -> CompiledPlans {
    let nodes: HashMap<&str, &TransitionMotionNode> =
        graph.nodes.iter().map(|node| (node.id(), node)).collect();
    let inputs_by_node: HashMap<&str, &str> = graph
        .input_bindings
        .iter()
        .map(|binding| (binding.to.node_id.as_str(), binding.source.id()))
        .collect();
    let mut plans = CompiledPlans {
        fallback: None,
        specific: HashMap::new(),
    };

    for binding in &graph.output_bindings {
        let Some(node) = nodes.get(binding.from.node_id.as_str()) else {
            continue;
        };
        let input_port = inputs_by_node
            .get(binding.from.node_id.as_str())
            .copied()
            .unwrap_or(binding.state_param_id.as_str());
        if input_port != binding.state_param_id {
            continue;
        }
        let plan = MotionPlan::from_node(node);
        let Some(plan) = plan else {
            continue;
        };
        if binding.state_param_id == ANY_CHANNEL {
            plans.fallback = Some(plan);
        } else {
            plans.specific.insert(binding.state_param_id.clone(), plan);
        }
    }
    for passthrough in &graph.passthrough_bindings {
        if passthrough.state_param_id == ANY_CHANNEL {
            plans.fallback = Some(MotionPlan::Instant);
        } else {
            plans
                .specific
                .insert(passthrough.state_param_id.clone(), MotionPlan::Instant);
        }
    }
    plans
}

#[derive(Debug, Clone)]
enum MotionPlan {
    Spring {
        duration: f64,
        bounce: f64,
        delay: f64,
    },
    Timeline {
        duration: f64,
        delay: f64,
        curve: TimelinePreset,
        blending: Option<TimelineBlending>,
    },
    Instant,
}

impl MotionPlan {
    fn from_node(node: &TransitionMotionNode) -> Option<Self> {
        if let Some((curve, timeline)) = node.timeline() {
            return Some(Self::Timeline {
                duration: timeline.duration,
                delay: timeline.delay,
                curve,
                blending: timeline.blending.clone(),
            });
        }
        Some(match node {
            TransitionMotionNode::Spring {
                duration,
                bounce,
                delay,
                ..
            } => Self::Spring {
                duration: *duration,
                bounce: *bounce,
                delay: *delay,
            },
            TransitionMotionNode::Instant { .. } => Self::Instant,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
struct Channel {
    target: Driver,
    transition_error: Driver,
}

impl Channel {
    fn hold(value: serde_json::Value) -> Self {
        let target = hold_driver(value);
        let zero = zero_driver_like(&target.sample().value);
        Self {
            target,
            transition_error: zero,
        }
    }

    fn set_static_target(&mut self, value: serde_json::Value) {
        self.target = hold_driver(value);
        if !self.transition_active() {
            self.transition_error = zero_driver_like(&self.target.sample().value);
        }
    }

    fn start_error(&mut self, current: DriverSample, plan: MotionPlan) {
        let target = self.target.sample();
        self.transition_error = match (&current.value, &current.velocity) {
            (current_value, current_velocity)
                if current_value.has_same_shape(&target.value)
                    && current_velocity.has_same_shape(&target.velocity) =>
            {
                let error_value = subtract_values(&target.value, current_value);
                let error_velocity = subtract_values(&target.velocity, current_velocity);
                Driver::start_numeric(
                    None,
                    error_value.clone(),
                    error_value.same_shape(vec![0.0; error_value.len()]),
                    plan,
                )
                .with_initial_velocity(error_velocity)
            }
            _ => zero_driver_like(&target.value),
        };
    }

    fn set_to(
        &mut self,
        target_json: serde_json::Value,
        velocity_json: Option<serde_json::Value>,
        dt: f64,
    ) -> anyhow::Result<()> {
        let target = NumericValue::from_json(&target_json)
            .ok_or_else(|| anyhow::anyhow!("setTo target must be numeric"))?;
        let previous = self.target.sample();
        if !target.has_same_shape(&previous.value) {
            anyhow::bail!("setTo target shape changed");
        }
        let velocity = match velocity_json {
            Some(value) => NumericValue::from_json(&value)
                .filter(|value| value.has_same_shape(&target))
                .ok_or_else(|| anyhow::anyhow!("setTo velocity shape changed"))?,
            None if dt > 0.0 => target.same_shape(
                target
                    .components()
                    .iter()
                    .zip(previous.value.components())
                    .map(|(next, previous)| (next - previous) / dt)
                    .collect(),
            ),
            None => target.same_shape(vec![0.0; target.len()]),
        };
        self.target = Driver::Hold(NumericSample {
            value: target,
            velocity,
        });
        Ok(())
    }

    fn to(
        &mut self,
        target_json: serde_json::Value,
        duration: f64,
        bounce: f64,
        dt: f64,
    ) -> anyhow::Result<()> {
        let target = NumericValue::from_json(&target_json)
            .ok_or_else(|| anyhow::anyhow!("to target must be numeric"))?;
        match &mut self.target {
            Driver::Spring(spring) if spring.value.has_same_shape(&target) => {
                spring.retarget(target);
                spring.step(dt);
            }
            _ => {
                let current = self.target.sample();
                if !current.value.has_same_shape(&target) {
                    anyhow::bail!("to target shape changed");
                }
                let mut spring =
                    SpringDriver::new(current.value, current.velocity, target, duration, bounce);
                spring.step(dt);
                self.target = Driver::Spring(spring);
            }
        }
        Ok(())
    }

    fn step(&mut self, dt: f64) {
        self.transition_error.step(dt);
    }

    fn target_sample(&self) -> DriverSample {
        self.target.sample()
    }

    fn error_sample(&self) -> DriverSample {
        self.transition_error.sample()
    }

    fn sample(&self) -> DriverSample {
        let target = self.target.sample();
        let error = self.transition_error.sample();
        DriverSample {
            value: subtract_values(&target.value, &error.value),
            velocity: subtract_values(&target.velocity, &error.velocity),
            driver: error.driver,
            completed: error.completed,
            persistent: false,
            timeline_progress: error.timeline_progress,
            blending_progress: error.blending_progress,
        }
    }

    fn transition_active(&self) -> bool {
        !self.transition_error.sample().completed
    }

    fn finish_transition(&mut self) {
        self.transition_error = zero_driver_like(&self.target.sample().value);
    }
}

fn hold_driver(value: serde_json::Value) -> Driver {
    match NumericValue::from_json(&value) {
        Some(value) => {
            let velocity = value.same_shape(vec![0.0; value.len()]);
            Driver::Hold(NumericSample { value, velocity })
        }
        None => Driver::Discrete(DiscreteDriver::hold(value)),
    }
}

fn zero_driver_like(value: &NumericValue) -> Driver {
    let zero = value.same_shape(vec![0.0; value.len()]);
    Driver::Hold(NumericSample {
        value: zero.clone(),
        velocity: zero,
    })
}

fn subtract_values(left: &NumericValue, right: &NumericValue) -> NumericValue {
    if !left.has_same_shape(right) {
        return left.clone();
    }
    left.same_shape(
        left.components()
            .iter()
            .zip(right.components())
            .map(|(left, right)| left - right)
            .collect(),
    )
}

#[derive(Debug, Clone)]
enum NumericValue {
    Scalar(Vec<f64>),
    Array(Vec<f64>),
    NestedArray {
        values: Vec<f64>,
        row_lengths: Vec<usize>,
    },
    Json(serde_json::Value),
}

impl NumericValue {
    fn from_json(value: &serde_json::Value) -> Option<Self> {
        if let Some(value) = value.as_f64() {
            return Some(Self::Scalar(vec![value]));
        }
        let array = value.as_array()?;
        if let Some(values) = array
            .iter()
            .map(serde_json::Value::as_f64)
            .collect::<Option<Vec<_>>>()
        {
            return Some(Self::Array(values));
        }

        let rows = array
            .iter()
            .map(serde_json::Value::as_array)
            .collect::<Option<Vec<_>>>()?;
        let row_lengths = rows.iter().map(|row| row.len()).collect::<Vec<_>>();
        let values = rows
            .iter()
            .flat_map(|row| row.iter())
            .map(serde_json::Value::as_f64)
            .collect::<Option<Vec<_>>>()?;
        Some(Self::NestedArray {
            values,
            row_lengths,
        })
    }

    fn zeros(len: usize) -> Self {
        Self::Array(vec![0.0; len])
    }

    fn len(&self) -> usize {
        self.components().len()
    }

    fn components(&self) -> &[f64] {
        match self {
            Self::Scalar(values) | Self::Array(values) | Self::NestedArray { values, .. } => values,
            Self::Json(_) => &[],
        }
    }

    fn has_same_shape(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Scalar(left), Self::Scalar(right)) | (Self::Array(left), Self::Array(right)) => {
                left.len() == right.len()
            }
            (
                Self::NestedArray {
                    row_lengths: left, ..
                },
                Self::NestedArray {
                    row_lengths: right, ..
                },
            ) => left == right,
            _ => false,
        }
    }

    fn same_shape(&self, values: Vec<f64>) -> Self {
        match self {
            Self::Scalar(_) => Self::Scalar(values),
            Self::Array(_) => Self::Array(values),
            Self::NestedArray { row_lengths, .. } => Self::NestedArray {
                values,
                row_lengths: row_lengths.clone(),
            },
            Self::Json(value) => Self::Json(value.clone()),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Scalar(values) => serde_json::json!(values.first().copied().unwrap_or(0.0)),
            Self::Array(values) => serde_json::json!(values),
            Self::NestedArray {
                values,
                row_lengths,
            } => {
                let mut offset = 0;
                serde_json::Value::Array(
                    row_lengths
                        .iter()
                        .map(|row_length| {
                            let end = offset + row_length;
                            let row = values[offset..end]
                                .iter()
                                .copied()
                                .map(serde_json::Value::from)
                                .collect();
                            offset = end;
                            serde_json::Value::Array(row)
                        })
                        .collect(),
                )
            }
            Self::Json(value) => value.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct NumericSample {
    value: NumericValue,
    velocity: NumericValue,
}

#[derive(Debug, Clone)]
enum Driver {
    Hold(NumericSample),
    Spring(SpringDriver),
    Timeline(TimelineDriver),
    Blend(BlendDriver),
    Delayed(DelayedDriver),
    Discrete(DiscreteDriver),
}

impl Driver {
    fn start_numeric(
        old: Option<Self>,
        source: NumericValue,
        target: NumericValue,
        plan: MotionPlan,
    ) -> Self {
        let outgoing = old.unwrap_or_else(|| {
            Driver::Hold(NumericSample {
                velocity: NumericValue::zeros(source.len()),
                value: source.clone(),
            })
        });
        let current = outgoing.sample();
        let current_value = match current.value {
            NumericValue::Json(ref value) => NumericValue::from_json(value),
            value => Some(value),
        }
        .filter(|value| value.len() == source.len())
        .unwrap_or_else(|| source.clone());
        let current_velocity = match current.velocity {
            NumericValue::Json(ref value) => NumericValue::from_json(value),
            value => Some(value),
        }
        .filter(|value| value.len() == source.len())
        .unwrap_or_else(|| source.same_shape(vec![0.0; source.len()]));
        match plan {
            MotionPlan::Spring {
                duration,
                bounce,
                delay,
            } => {
                let spring = Driver::Spring(SpringDriver::new(
                    current_value,
                    current_velocity,
                    target,
                    duration,
                    bounce,
                ));
                if delay > 0.0 {
                    Driver::Delayed(DelayedDriver::new(outgoing, spring, delay, true))
                } else {
                    spring
                }
            }
            MotionPlan::Timeline {
                duration,
                delay,
                curve,
                blending,
            } => {
                let timeline =
                    Driver::Timeline(TimelineDriver::new(source, target, duration, curve));
                let incoming = if let Some(blending) = blending {
                    Driver::Blend(BlendDriver::new(outgoing.clone(), timeline, blending))
                } else {
                    timeline
                };
                if delay > 0.0 {
                    Driver::Delayed(DelayedDriver::new(outgoing, incoming, delay, false))
                } else {
                    incoming
                }
            }
            MotionPlan::Instant => Driver::Hold(NumericSample {
                velocity: NumericValue::zeros(target.len()),
                value: target,
            }),
        }
    }

    fn step(&mut self, dt: f64) {
        match self {
            Self::Hold(_) => {}
            Self::Spring(driver) => driver.step(dt),
            Self::Timeline(driver) => driver.step(dt),
            Self::Blend(driver) => driver.step(dt),
            Self::Delayed(driver) => driver.step(dt),
            Self::Discrete(driver) => driver.step(dt),
        }
    }

    fn sample(&self) -> DriverSample {
        match self {
            Self::Hold(sample) => DriverSample::numeric(sample.clone(), "hold", true, None, None),
            Self::Spring(driver) => driver.sample(),
            Self::Timeline(driver) => driver.sample(),
            Self::Blend(driver) => driver.sample(),
            Self::Delayed(driver) => driver.sample(),
            Self::Discrete(driver) => driver.sample(),
        }
    }

    fn with_initial_velocity(mut self, velocity: NumericValue) -> Self {
        match &mut self {
            Self::Spring(spring) => {
                spring.initial_velocity = velocity.clone();
                spring.velocity = velocity;
            }
            Self::Delayed(delayed) => {
                delayed.incoming =
                    Box::new((*delayed.incoming).clone().with_initial_velocity(velocity));
            }
            _ => {}
        }
        self
    }
}

#[derive(Debug, Clone)]
struct DriverSample {
    value: NumericValue,
    velocity: NumericValue,
    driver: &'static str,
    completed: bool,
    persistent: bool,
    timeline_progress: Option<f64>,
    blending_progress: Option<f64>,
}

impl DriverSample {
    fn numeric(
        sample: NumericSample,
        driver: &'static str,
        completed: bool,
        timeline_progress: Option<f64>,
        blending_progress: Option<f64>,
    ) -> Self {
        Self {
            value: sample.value,
            velocity: sample.velocity,
            driver,
            completed,
            persistent: false,
            timeline_progress,
            blending_progress,
        }
    }
}

#[derive(Debug, Clone)]
struct SpringDriver {
    initial: NumericValue,
    initial_velocity: NumericValue,
    target: NumericValue,
    duration: f64,
    bounce: f64,
    elapsed: f64,
    segment_started_at: f64,
    value: NumericValue,
    velocity: NumericValue,
    completed: bool,
    no_progress_frames: u8,
}

impl SpringDriver {
    fn new(
        initial: NumericValue,
        initial_velocity: NumericValue,
        target: NumericValue,
        duration: f64,
        bounce: f64,
    ) -> Self {
        Self {
            value: initial.clone(),
            velocity: initial_velocity.clone(),
            initial,
            initial_velocity,
            target,
            duration: f64::from(duration as f32),
            bounce: f64::from(bounce as f32),
            elapsed: 0.0,
            segment_started_at: 0.0,
            completed: false,
            no_progress_frames: 0,
        }
    }

    fn retarget(&mut self, target: NumericValue) {
        if same_components(&self.target, &target) {
            return;
        }
        self.initial = self.value.clone();
        self.initial_velocity = self.velocity.clone();
        self.target = target;
        self.segment_started_at = self.elapsed;
        self.completed = false;
        self.no_progress_frames = 0;
    }

    fn step(&mut self, dt: f64) {
        if self.completed || dt <= 0.0 {
            return;
        }
        self.elapsed += dt;
        let segment_elapsed = self.elapsed - self.segment_started_at;
        let before = self
            .value
            .components()
            .iter()
            .map(|value| *value as f32)
            .collect::<Vec<_>>();
        let mut values = Vec::with_capacity(self.initial.len());
        let mut velocities = Vec::with_capacity(self.initial.len());
        for index in 0..self.initial.len() {
            let (value, velocity) = solve_spring_component(
                self.initial.components()[index],
                self.initial_velocity
                    .components()
                    .get(index)
                    .copied()
                    .unwrap_or(0.0),
                self.target.components()[index],
                segment_elapsed,
                self.duration,
                self.bounce,
            );
            values.push(f64::from(value as f32));
            velocities.push(f64::from(velocity as f32));
        }
        self.value = self.initial.same_shape(values);
        self.velocity = self.initial_velocity.same_shape(velocities);

        let after = self
            .value
            .components()
            .iter()
            .map(|value| *value as f32)
            .collect::<Vec<_>>();
        self.no_progress_frames = if before == after {
            self.no_progress_frames.saturating_add(1)
        } else {
            0
        };
        if spring_is_complete(
            &self.value,
            &self.velocity,
            &self.target,
            self.duration,
            self.bounce,
        ) || self.no_progress_frames >= 4
        {
            self.value = self.target.clone();
            self.velocity = NumericValue::zeros(self.target.len());
            self.completed = true;
        }
    }

    fn sample(&self) -> DriverSample {
        DriverSample::numeric(
            NumericSample {
                value: self.value.clone(),
                velocity: self.velocity.clone(),
            },
            "spring",
            self.completed,
            None,
            None,
        )
    }
}

fn solve_spring_component(
    initial: f64,
    initial_velocity: f64,
    target: f64,
    time: f64,
    duration: f64,
    bounce: f64,
) -> (f64, f64) {
    let omega = f64::from((2.0_f32 * std::f32::consts::PI) / duration as f32);
    let zeta = if bounce < 0.0 {
        1.0 / (1.0 + bounce)
    } else {
        1.0 - bounce
    };
    let displacement = initial - target;

    let (relative, velocity) = if zeta < 1.0 {
        let damped = omega * (1.0 - zeta * zeta).sqrt();
        let a = displacement;
        let b = (initial_velocity + zeta * omega * displacement) / damped;
        let decay = (-zeta * omega * time).exp();
        let sin = (damped * time).sin();
        let cos = (damped * time).cos();
        let relative = decay * (a * cos + b * sin);
        let velocity = decay
            * ((-zeta * omega * a + b * damped) * cos + (-a * damped - zeta * omega * b) * sin);
        (relative, velocity)
    } else if zeta == 1.0 {
        let a = displacement;
        let b = initial_velocity + omega * displacement;
        let decay = (-omega * time).exp();
        let relative = (a + b * time) * decay;
        let velocity = (b - omega * (a + b * time)) * decay;
        (relative, velocity)
    } else {
        let root = (zeta * zeta - 1.0).sqrt();
        let r1 = -omega * (zeta - root);
        let r2 = -omega * (zeta + root);
        let c1 = (initial_velocity - r2 * displacement) / (r1 - r2);
        let c2 = displacement - c1;
        let e1 = (r1 * time).exp();
        let e2 = (r2 * time).exp();
        (c1 * e1 + c2 * e2, c1 * r1 * e1 + c2 * r2 * e2)
    };
    (target + relative, velocity)
}

fn spring_is_complete(
    value: &NumericValue,
    velocity: &NumericValue,
    target: &NumericValue,
    duration: f64,
    bounce: f64,
) -> bool {
    let omega = f64::from((2.0_f32 * std::f32::consts::PI) / duration as f32);
    let zeta = if bounce < 0.0 {
        1.0 / (1.0 + bounce)
    } else {
        1.0 - bounce
    };
    let stiffness = omega * omega;
    let damping = 2.0 * zeta * omega;
    let target_magnitude = target
        .components()
        .iter()
        .copied()
        .map(f64::abs)
        .fold(0.0, f64::max) as f32;
    let ulp = f32_ulp(target_magnitude);
    let threshold = 1e-6_f64.max(f64::from(16.0 * ulp).powi(2));
    value
        .components()
        .iter()
        .zip(velocity.components())
        .zip(target.components())
        .all(|((value, velocity), target)| {
            let displacement = value - target;
            displacement * displacement + (velocity * damping / stiffness).powi(2) <= threshold
        })
}

fn f32_ulp(value: f32) -> f32 {
    if !value.is_finite() {
        return f32::INFINITY;
    }
    let value = value.abs();
    if value == 0.0 {
        return f32::from_bits(1);
    }
    f32::from_bits(value.to_bits() + 1) - value
}

#[derive(Debug, Clone)]
struct TimelineDriver {
    from: NumericValue,
    to: NumericValue,
    duration: f64,
    curve: TimelinePreset,
    elapsed: f64,
}

impl TimelineDriver {
    fn new(from: NumericValue, to: NumericValue, duration: f64, curve: TimelinePreset) -> Self {
        Self {
            from,
            to,
            duration,
            curve,
            elapsed: 0.0,
        }
    }

    fn step(&mut self, dt: f64) {
        self.elapsed = (self.elapsed + dt).min(self.duration.max(0.0));
    }

    fn sample(&self) -> DriverSample {
        let raw = if self.duration <= 0.0 {
            1.0
        } else {
            (self.elapsed / self.duration).clamp(0.0, 1.0)
        };
        let (amount, derivative) = timeline_curve(self.curve, raw);
        let velocity_scale = if self.duration > 0.0 {
            derivative / self.duration
        } else {
            0.0
        };
        let values = self
            .from
            .components()
            .iter()
            .zip(self.to.components())
            .map(|(from, to)| from + (to - from) * amount)
            .collect();
        let completed = raw >= 1.0;
        let velocities = if completed {
            vec![0.0; self.from.len()]
        } else {
            self.from
                .components()
                .iter()
                .zip(self.to.components())
                .map(|(from, to)| (to - from) * velocity_scale)
                .collect()
        };
        DriverSample::numeric(
            NumericSample {
                value: self.from.same_shape(values),
                velocity: self.from.same_shape(velocities),
            },
            "timeline",
            completed,
            Some(raw),
            None,
        )
    }
}

fn timeline_curve(curve: TimelinePreset, t: f64) -> (f64, f64) {
    match curve {
        TimelinePreset::Linear => (t, 1.0),
        TimelinePreset::EaseIn => (t * t, 2.0 * t),
        TimelinePreset::EaseOut => (t * (2.0 - t), 2.0 - 2.0 * t),
        TimelinePreset::EaseInOut => {
            if t < 0.5 {
                (2.0 * t * t, 4.0 * t)
            } else {
                (-1.0 + (4.0 - 2.0 * t) * t, 4.0 - 4.0 * t)
            }
        }
        TimelinePreset::SineIn | TimelinePreset::CosineOut => {
            (1.0 - (PI * t / 2.0).cos(), PI * (PI * t / 2.0).sin() / 2.0)
        }
        TimelinePreset::SineOut | TimelinePreset::CosineIn => {
            ((PI * t / 2.0).sin(), PI * (PI * t / 2.0).cos() / 2.0)
        }
        TimelinePreset::SineInOut | TimelinePreset::CosineInOut => {
            ((1.0 - (PI * t).cos()) / 2.0, PI * (PI * t).sin() / 2.0)
        }
    }
}

#[derive(Debug, Clone)]
struct BlendDriver {
    outgoing: Box<Driver>,
    incoming: Box<Driver>,
    duration: f64,
    easing: EasingKind,
    elapsed: f64,
}

impl BlendDriver {
    fn new(outgoing: Driver, incoming: Driver, blending: TimelineBlending) -> Self {
        Self {
            outgoing: Box::new(outgoing),
            incoming: Box::new(incoming),
            duration: blending.duration,
            easing: blending.easing,
            elapsed: 0.0,
        }
    }

    fn step(&mut self, dt: f64) {
        self.outgoing.step(dt);
        self.incoming.step(dt);
        self.elapsed = (self.elapsed + dt).min(self.duration.max(0.0));
    }

    fn sample(&self) -> DriverSample {
        let outgoing = self.outgoing.sample();
        let incoming = self.incoming.sample();
        let raw = if self.duration <= 0.0 {
            1.0
        } else {
            (self.elapsed / self.duration).clamp(0.0, 1.0)
        };
        let weight = ease(self.easing, raw);
        let weight_velocity = if self.duration <= 0.0 || raw >= 1.0 {
            0.0
        } else {
            easing_derivative(self.easing, raw) / self.duration
        };
        let values = outgoing
            .value
            .components()
            .iter()
            .zip(incoming.value.components())
            .map(|(old, new)| old + (new - old) * weight)
            .collect();
        let velocities = outgoing
            .velocity
            .components()
            .iter()
            .zip(incoming.velocity.components())
            .zip(outgoing.value.components())
            .zip(incoming.value.components())
            .map(|(((old_v, new_v), old), new)| {
                (1.0 - weight) * old_v + weight * new_v + weight_velocity * (new - old)
            })
            .collect();
        let completed = raw >= 1.0 && incoming.completed;
        DriverSample::numeric(
            NumericSample {
                value: incoming.value.same_shape(values),
                velocity: if completed {
                    NumericValue::zeros(incoming.value.len())
                } else {
                    incoming.velocity.same_shape(velocities)
                },
            },
            "timeline+tween",
            completed,
            incoming.timeline_progress,
            Some(raw),
        )
    }
}

fn easing_derivative(easing: EasingKind, t: f64) -> f64 {
    match easing {
        EasingKind::Linear => 1.0,
        EasingKind::EaseIn => 2.0 * t,
        EasingKind::EaseOut => 2.0 - 2.0 * t,
        EasingKind::EaseInOut if t < 0.5 => 4.0 * t,
        EasingKind::EaseInOut => 4.0 - 4.0 * t,
    }
}

#[derive(Debug, Clone)]
struct DelayedDriver {
    outgoing: Box<Driver>,
    incoming: Box<Driver>,
    delay: f64,
    elapsed: f64,
    reseed_spring: bool,
}

impl DelayedDriver {
    fn new(outgoing: Driver, incoming: Driver, delay: f64, reseed_spring: bool) -> Self {
        Self {
            outgoing: Box::new(outgoing),
            incoming: Box::new(incoming),
            delay,
            elapsed: 0.0,
            reseed_spring,
        }
    }

    fn step(&mut self, dt: f64) {
        if self.elapsed < self.delay {
            let remaining = self.delay - self.elapsed;
            let outgoing_dt = dt.min(remaining);
            self.outgoing.step(outgoing_dt);
            self.elapsed += outgoing_dt;
            if self.elapsed >= self.delay && self.reseed_spring {
                let sample = self.outgoing.sample();
                if let Driver::Spring(spring) = self.incoming.as_mut() {
                    spring.initial = sample.value.clone();
                    spring.value = sample.value;
                    spring.initial_velocity = sample.velocity.clone();
                    spring.velocity = sample.velocity;
                    spring.elapsed = 0.0;
                }
            }
            if self.elapsed >= self.delay
                && let Driver::Blend(blend) = self.incoming.as_mut()
            {
                // Blending begins after delay. Carry the already-advanced
                // outgoing driver into the composite instead of restarting
                // the stale clone captured when the transition was triggered.
                blend.outgoing = Box::new((*self.outgoing).clone());
            }
            let rest = dt - outgoing_dt;
            if rest > 0.0 {
                self.incoming.step(rest);
            }
        } else {
            self.incoming.step(dt);
        }
    }

    fn sample(&self) -> DriverSample {
        if self.elapsed < self.delay {
            let mut sample = self.outgoing.sample();
            sample.driver = "delay";
            sample.completed = false;
            return sample;
        }
        self.incoming.sample()
    }
}

#[derive(Debug, Clone)]
struct DiscreteDriver {
    outgoing: Option<Box<Driver>>,
    timing: Box<Driver>,
    from: serde_json::Value,
    target: serde_json::Value,
}

impl DiscreteDriver {
    #[cfg(test)]
    fn new(
        outgoing: Option<Driver>,
        from: serde_json::Value,
        target: serde_json::Value,
        plan: MotionPlan,
    ) -> Self {
        // Discrete values cannot be interpolated, but they still obey the
        // selected path's real completion semantics. Drive a numeric 0 -> 1
        // proxy through the same plan so springs wait for their analytic stop
        // condition and Timeline+Tween waits for both sides of the composite.
        let timing = Driver::start_numeric(
            None,
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![1.0]),
            plan,
        );
        Self {
            outgoing: outgoing.map(Box::new),
            timing: Box::new(timing),
            from,
            target,
        }
    }

    fn hold(value: serde_json::Value) -> Self {
        Self {
            outgoing: None,
            timing: Box::new(Driver::Hold(NumericSample {
                value: NumericValue::Scalar(vec![1.0]),
                velocity: NumericValue::Scalar(vec![0.0]),
            })),
            from: value.clone(),
            target: value,
        }
    }

    fn step(&mut self, dt: f64) {
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.step(dt);
        }
        self.timing.step(dt);
    }

    fn sample(&self) -> DriverSample {
        let timing = self.timing.sample();
        let completed = timing.completed;
        DriverSample {
            value: NumericValue::Json(if completed {
                self.target.clone()
            } else {
                self.from.clone()
            }),
            velocity: NumericValue::Json(serde_json::Value::Null),
            driver: "discrete",
            completed,
            persistent: timing.persistent,
            timeline_progress: timing.timeline_progress,
            blending_progress: timing.blending_progress,
        }
    }
}

fn same_components(a: &NumericValue, b: &NumericValue) -> bool {
    a.components() == b.components()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wildcard_spring_graph(duration: f64, bounce: f64) -> TransitionMotionGraph {
        let any_port = super::super::types::GraphPort {
            id: "*".into(),
            name: Some("Any".into()),
            port_type: Some("any".into()),
            array_length: None,
            motion: None,
        };
        TransitionMotionGraph {
            id: "motion".into(),
            name: "Wildcard".into(),
            inputs: vec![any_port.clone()],
            outputs: vec![any_port],
            nodes: vec![TransitionMotionNode::Spring {
                id: "spring".into(),
                position: Default::default(),
                label: None,
                duration,
                bounce,
                delay: 0.0,
            }],
            connections: vec![],
            input_bindings: vec![super::super::types::TransitionMotionInputBinding {
                source: StateValueSource::StateParam {
                    state_param_id: "*".into(),
                },
                to: super::super::types::GraphEndpoint {
                    node_id: "spring".into(),
                    port_id: "value".into(),
                },
            }],
            output_bindings: vec![super::super::types::TransitionMotionOutputBinding {
                state_param_id: "*".into(),
                from: super::super::types::GraphEndpoint {
                    node_id: "spring".into(),
                    port_id: "value".into(),
                },
            }],
            passthrough_bindings: vec![],
            condition_binding: None,
            layout: None,
            viewport: None,
        }
    }

    #[derive(Deserialize)]
    struct GroundTruth {
        source: GroundTruthSource,
        scenarios: Vec<GroundTruthScenario>,
        #[serde(rename = "motionScenarios")]
        motion_scenarios: Vec<GroundTruthMotionScenario>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GroundTruthSource {
        voice_interaction_commit: String,
        omotion_version: String,
    }

    #[derive(Deserialize)]
    struct GroundTruthScenario {
        name: String,
        duration: f64,
        bounce: f64,
        frames: Vec<GroundTruthFrame>,
    }

    #[derive(Deserialize)]
    struct GroundTruthFrame {
        frame: usize,
        dt: f64,
        value: f64,
        velocity: f64,
        target: f64,
        running: bool,
        completed: bool,
    }

    #[derive(Deserialize)]
    struct GroundTruthMotionScenario {
        name: String,
        frames: Vec<GroundTruthMotionFrame>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GroundTruthMotionFrame {
        frame: usize,
        dt: f64,
        value: f64,
        velocity: f64,
        target: f64,
        driver: String,
        timeline_progress: Option<f64>,
        blending_weight: Option<f64>,
        running: bool,
        completed: bool,
    }

    #[test]
    fn timeline_interpolates_nested_numeric_arrays_and_preserves_shape() {
        let previous = Channel::hold(serde_json::json!([[0.0, 2.0], [4.0, 6.0]])).sample();
        let mut channel = Channel::hold(serde_json::json!([[2.0, 4.0], [6.0, 8.0]]));
        channel.start_error(
            previous,
            MotionPlan::Timeline {
                duration: 1.0,
                delay: 0.0,
                curve: TimelinePreset::Linear,
                blending: None,
            },
        );

        channel.step(0.5);

        assert_eq!(
            channel.sample().value.to_json(),
            serde_json::json!([[1.0, 3.0], [5.0, 7.0]])
        );
    }

    #[test]
    fn differently_shaped_nested_arrays_transition_instantly() {
        let previous = Channel::hold(serde_json::json!([[0.0, 2.0], [4.0, 6.0]])).sample();
        let mut channel = Channel::hold(serde_json::json!([[2.0, 4.0, 6.0], [8.0]]));
        channel.start_error(
            previous,
            MotionPlan::Timeline {
                duration: 1.0,
                delay: 0.0,
                curve: TimelinePreset::Linear,
                blending: None,
            },
        );

        channel.step(0.5);

        assert_eq!(
            channel.sample().value.to_json(),
            serde_json::json!([[2.0, 4.0, 6.0], [8.0]])
        );
    }

    #[test]
    fn analytic_spring_matches_frozen_kotlin_omotion_ground_truth() {
        let fixture: GroundTruth = serde_json::from_str(include_str!(
            "../../tests/fixtures/omotion_spring_ground_truth.json"
        ))
        .expect("parse frozen OMotion ground truth");
        assert_eq!(fixture.source.voice_interaction_commit, "b3e4abb");
        assert_eq!(fixture.source.omotion_version, "0.1.0-alpha02-SNAPSHOT");

        for scenario in fixture.scenarios {
            let initial_velocity = if scenario.name == "retarget_velocity_inheritance" {
                4.0
            } else {
                0.0
            };
            let first = scenario.frames.first().expect("scenario has frame zero");
            let mut spring = SpringDriver::new(
                NumericValue::Scalar(vec![first.value]),
                NumericValue::Scalar(vec![initial_velocity]),
                NumericValue::Scalar(vec![first.target]),
                scenario.duration,
                scenario.bounce,
            );

            for frame in &scenario.frames {
                if frame.frame > 0 {
                    if spring.target.components()[0] != frame.target {
                        spring.retarget(NumericValue::Scalar(vec![frame.target]));
                    }
                    spring.step(kotlin_frame_seconds(frame.dt));
                }
                let sample = spring.sample();
                let actual_value = sample.value.components()[0];
                let actual_velocity = sample.velocity.components()[0];
                assert!(
                    (actual_value - frame.value).abs() <= 1e-6,
                    "{} frame {} value: actual={actual_value:.9} expected={:.9}",
                    scenario.name,
                    frame.frame,
                    frame.value
                );
                assert!(
                    (actual_velocity - frame.velocity).abs() <= 1e-6,
                    "{} frame {} velocity: actual={actual_velocity:.9} expected={:.9}",
                    scenario.name,
                    frame.frame,
                    frame.velocity
                );
                assert_eq!(
                    !sample.completed, frame.running,
                    "{} frame {} running state",
                    scenario.name, frame.frame
                );
                assert_eq!(
                    sample.completed, frame.completed,
                    "{} frame {} completion state",
                    scenario.name, frame.frame
                );
                if frame.completed {
                    assert_eq!(actual_value, frame.target, "final snap must be exact");
                    assert_eq!(actual_velocity, 0.0, "stopped velocity must be exact");
                }
            }
        }
    }

    #[test]
    fn timeline_tween_matches_frozen_kotlin_motion_ground_truth() {
        let fixture: GroundTruth = serde_json::from_str(include_str!(
            "../../tests/fixtures/omotion_spring_ground_truth.json"
        ))
        .expect("parse frozen Kotlin motion ground truth");

        for scenario in fixture.motion_scenarios {
            let mut driver = match scenario.name.as_str() {
                "stopped_hold_to_timeline_tween" => Driver::start_numeric(
                    Some(Driver::Hold(NumericSample {
                        value: NumericValue::Scalar(vec![1.0]),
                        velocity: NumericValue::Scalar(vec![0.0]),
                    })),
                    NumericValue::Scalar(vec![1.0]),
                    NumericValue::Scalar(vec![2.0]),
                    MotionPlan::Timeline {
                        duration: 0.3,
                        delay: 0.0,
                        curve: TimelinePreset::Linear,
                        blending: Some(TimelineBlending {
                            blend_type: super::super::types::TimelineBlendingType::Tween,
                            duration: 0.1,
                            easing: EasingKind::EaseInOut,
                        }),
                    },
                ),
                "running_spring_to_timeline_tween" => {
                    let mut outgoing = Driver::Spring(SpringDriver::new(
                        NumericValue::Scalar(vec![0.0]),
                        NumericValue::Scalar(vec![0.0]),
                        NumericValue::Scalar(vec![1.0]),
                        0.45,
                        0.1,
                    ));
                    for _ in 0..6 {
                        outgoing.step(kotlin_frame_seconds(1.0 / 60.0));
                    }
                    Driver::start_numeric(
                        Some(outgoing),
                        NumericValue::Scalar(vec![1.0]),
                        NumericValue::Scalar(vec![2.0]),
                        MotionPlan::Timeline {
                            duration: 0.3,
                            delay: 0.0,
                            curve: TimelinePreset::Linear,
                            blending: Some(TimelineBlending {
                                blend_type: super::super::types::TimelineBlendingType::Tween,
                                duration: 0.12,
                                easing: EasingKind::EaseInOut,
                            }),
                        },
                    )
                }
                name => panic!("unknown Kotlin motion scenario: {name}"),
            };

            for frame in scenario.frames {
                if frame.frame > 0 {
                    driver.step(kotlin_frame_seconds(frame.dt));
                }
                let sample = driver.sample();
                assert_eq!(frame.target, 2.0);
                assert_eq!(
                    sample.driver, frame.driver,
                    "{} frame {}",
                    scenario.name, frame.frame
                );
                assert!(
                    (sample.value.components()[0] - frame.value).abs() <= 1e-6,
                    "{} frame {} value: actual={} expected={}",
                    scenario.name,
                    frame.frame,
                    sample.value.components()[0],
                    frame.value
                );
                assert!(
                    (sample.velocity.components()[0] - frame.velocity).abs() <= 1e-6,
                    "{} frame {} velocity: actual={} expected={}",
                    scenario.name,
                    frame.frame,
                    sample.velocity.components()[0],
                    frame.velocity
                );
                assert_eq!(sample.timeline_progress, frame.timeline_progress);
                let actual_weight = sample
                    .blending_progress
                    .map(|progress| ease(EasingKind::EaseInOut, progress));
                if let (Some(actual), Some(expected)) = (actual_weight, frame.blending_weight) {
                    assert!(
                        (actual - expected).abs() <= 1e-9,
                        "{} frame {} blending weight",
                        scenario.name,
                        frame.frame
                    );
                } else {
                    assert_eq!(actual_weight, frame.blending_weight);
                }
                assert_eq!(!sample.completed, frame.running);
                assert_eq!(sample.completed, frame.completed);
            }
        }
    }

    #[test]
    fn analytic_spring_advances_with_full_frame_delta() {
        let mut spring = SpringDriver::new(
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![1.0]),
            0.45,
            0.1,
        );
        spring.step(kotlin_frame_seconds(1.0 / 30.0));
        let once = spring.sample().value.components()[0];

        let mut split = SpringDriver::new(
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![1.0]),
            0.45,
            0.1,
        );
        split.step(kotlin_frame_seconds(1.0 / 60.0));
        split.step(kotlin_frame_seconds(1.0 / 60.0));
        let twice = split.sample().value.components()[0];
        assert!((once - twice).abs() < 2e-6, "once={once} twice={twice}");
    }

    #[test]
    fn tween_blend_uses_product_rule_velocity() {
        let outgoing = Driver::Hold(NumericSample {
            value: NumericValue::Scalar(vec![0.25]),
            velocity: NumericValue::Scalar(vec![1.0]),
        });
        let incoming = Driver::Timeline(TimelineDriver::new(
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![1.0]),
            1.0,
            TimelinePreset::Linear,
        ));
        let mut blend = BlendDriver::new(
            outgoing,
            incoming,
            TimelineBlending {
                blend_type: super::super::types::TimelineBlendingType::Tween,
                duration: 0.5,
                easing: EasingKind::Linear,
            },
        );
        blend.step(0.25);
        let sample = blend.sample();
        assert!(sample.velocity.components()[0].is_finite());
        assert_eq!(sample.blending_progress, Some(0.5));
    }

    #[test]
    fn completed_tween_weight_is_held_while_timeline_continues() {
        let outgoing = Driver::Hold(NumericSample {
            value: NumericValue::Scalar(vec![0.0]),
            velocity: NumericValue::Scalar(vec![0.0]),
        });
        let incoming = Driver::Timeline(TimelineDriver::new(
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![1.0]),
            1.0,
            TimelinePreset::Linear,
        ));
        let mut blend = BlendDriver::new(
            outgoing,
            incoming,
            TimelineBlending {
                blend_type: super::super::types::TimelineBlendingType::Tween,
                duration: 0.1,
                easing: EasingKind::Linear,
            },
        );

        blend.step(0.2);
        let sample = blend.sample();
        assert_eq!(sample.blending_progress, Some(1.0));
        assert_eq!(sample.timeline_progress, Some(0.2));
        assert!(!sample.completed);
        assert!((sample.value.components()[0] - 0.2).abs() < 1e-9);
        assert!((sample.velocity.components()[0] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn delay_advances_outgoing_before_tween_blending_starts() {
        let outgoing = Driver::Timeline(TimelineDriver::new(
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![1.0]),
            1.0,
            TimelinePreset::Linear,
        ));
        let incoming = Driver::Blend(BlendDriver::new(
            outgoing.clone(),
            Driver::Timeline(TimelineDriver::new(
                NumericValue::Scalar(vec![1.0]),
                NumericValue::Scalar(vec![2.0]),
                1.0,
                TimelinePreset::Linear,
            )),
            TimelineBlending {
                blend_type: super::super::types::TimelineBlendingType::Tween,
                duration: 0.1,
                easing: EasingKind::Linear,
            },
        ));
        let mut delayed = DelayedDriver::new(outgoing, incoming, 0.2, false);
        delayed.step(0.2);
        let at_blend_start = delayed.sample();
        assert!((at_blend_start.value.components()[0] - 0.2).abs() < 1e-9);
        assert_eq!(at_blend_start.blending_progress, Some(0.0));

        delayed.step(0.05);
        let midway = delayed.sample();
        assert_eq!(midway.blending_progress, Some(0.5));
        assert!(midway.value.components()[0] > 0.25);
    }

    #[test]
    fn timeline_to_timeline_tween_advances_both_drivers() {
        let mut outgoing = Driver::Timeline(TimelineDriver::new(
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![1.0]),
            1.0,
            TimelinePreset::Linear,
        ));
        outgoing.step(0.2);
        let mut composite = Driver::start_numeric(
            Some(outgoing),
            NumericValue::Scalar(vec![1.0]),
            NumericValue::Scalar(vec![2.0]),
            MotionPlan::Timeline {
                duration: 0.5,
                delay: 0.0,
                curve: TimelinePreset::Linear,
                blending: Some(TimelineBlending {
                    blend_type: super::super::types::TimelineBlendingType::Tween,
                    duration: 0.1,
                    easing: EasingKind::Linear,
                }),
            },
        );

        let start = composite.sample();
        assert!((start.value.components()[0] - 0.2).abs() < 1e-9);
        composite.step(0.05);
        let midway = composite.sample();
        assert_eq!(midway.timeline_progress, Some(0.1));
        assert_eq!(midway.blending_progress, Some(0.5));
        assert!(midway.value.components()[0] > 0.25);
        assert!(midway.value.components()[0] < 1.1);
    }

    #[test]
    fn timeline_to_spring_inherits_presentation_velocity() {
        let mut timeline = Driver::Timeline(TimelineDriver::new(
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![1.0]),
            1.0,
            TimelinePreset::Linear,
        ));
        timeline.step(0.25);
        let inherited = timeline.sample().velocity.components()[0];
        let spring = Driver::start_numeric(
            Some(timeline),
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![2.0]),
            MotionPlan::Spring {
                duration: 0.45,
                bounce: 0.1,
                delay: 0.0,
            },
        );
        let sample = spring.sample();
        assert_eq!(sample.driver, "spring");
        assert!((sample.velocity.components()[0] - inherited).abs() < 1e-9);
        assert!((sample.value.components()[0] - 0.25).abs() < 1e-9);
    }

    #[test]
    fn discrete_numeric_hold_to_spring_uses_the_presentation_value() {
        let discrete = Driver::Discrete(DiscreteDriver::hold(serde_json::json!(5.0)));
        let mut spring = Driver::start_numeric(
            Some(discrete),
            NumericValue::Scalar(vec![5.0]),
            NumericValue::Scalar(vec![10.0]),
            MotionPlan::Spring {
                duration: 0.45,
                bounce: 0.1,
                delay: 0.0,
            },
        );
        let initial = spring.sample();
        assert_eq!(initial.value.components(), &[5.0]);
        assert_eq!(initial.velocity.components(), &[0.0]);

        spring.step(kotlin_frame_seconds(1.0 / 60.0));
        let running = spring.sample();
        assert!(!running.completed);
        assert!(running.value.components()[0] > 5.0);
        assert!(running.value.components()[0] < 10.0);
    }

    #[test]
    fn completed_timeline_has_zero_velocity_for_later_interruptions() {
        let mut timeline = TimelineDriver::new(
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![1.0]),
            0.25,
            TimelinePreset::Linear,
        );
        timeline.step(0.25);
        let sample = timeline.sample();
        assert!(sample.completed);
        assert_eq!(sample.value.components(), &[1.0]);
        assert_eq!(sample.velocity.components(), &[0.0]);
    }

    #[test]
    fn interrupting_a_tween_keeps_the_composite_as_outgoing() {
        let outgoing = Driver::Spring(SpringDriver::new(
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![0.0]),
            NumericValue::Scalar(vec![1.0]),
            0.45,
            0.1,
        ));
        let first = Driver::Blend(BlendDriver::new(
            outgoing,
            Driver::Timeline(TimelineDriver::new(
                NumericValue::Scalar(vec![0.0]),
                NumericValue::Scalar(vec![2.0]),
                0.5,
                TimelinePreset::EaseInOut,
            )),
            TimelineBlending {
                blend_type: super::super::types::TimelineBlendingType::Tween,
                duration: 0.2,
                easing: EasingKind::EaseInOut,
            },
        ));
        let second = Driver::start_numeric(
            Some(first),
            NumericValue::Scalar(vec![2.0]),
            NumericValue::Scalar(vec![3.0]),
            MotionPlan::Timeline {
                duration: 0.4,
                delay: 0.0,
                curve: TimelinePreset::Linear,
                blending: Some(TimelineBlending {
                    blend_type: super::super::types::TimelineBlendingType::Tween,
                    duration: 0.1,
                    easing: EasingKind::Linear,
                }),
            },
        );
        let Driver::Blend(second_blend) = second else {
            panic!("expected outer blend");
        };
        assert!(matches!(second_blend.outgoing.as_ref(), Driver::Blend(_)));
    }

    #[test]
    fn discrete_spring_switches_only_when_the_spring_path_completes() {
        let mut driver = DiscreteDriver::new(
            None,
            serde_json::json!("source"),
            serde_json::json!("target"),
            MotionPlan::Spring {
                duration: 0.45,
                bounce: 0.1,
                delay: 0.0,
            },
        );

        driver.step(kotlin_frame_seconds(1.0 / 60.0));
        let running = driver.sample();
        assert!(!running.completed);
        assert_eq!(running.value.to_json(), serde_json::json!("source"));

        for _ in 0..600 {
            if driver.sample().completed {
                break;
            }
            driver.step(kotlin_frame_seconds(1.0 / 60.0));
        }
        let completed = driver.sample();
        assert!(completed.completed);
        assert_eq!(completed.value.to_json(), serde_json::json!("target"));
    }

    #[test]
    fn property_specific_motion_overrides_the_any_fallback() {
        let ports = ["*", "Node:x", "Node:y"]
            .into_iter()
            .map(|id| super::super::types::GraphPort {
                id: id.into(),
                name: Some(id.into()),
                port_type: Some("float".into()),
                array_length: None,
                motion: None,
            })
            .collect::<Vec<_>>();
        let graph = TransitionMotionGraph {
            id: "motion".into(),
            name: "Mixed".into(),
            inputs: ports.clone(),
            outputs: ports,
            nodes: vec![
                TransitionMotionNode::Linear {
                    timeline: super::super::types::TimelineMotionNode {
                        id: "any".into(),
                        position: Default::default(),
                        label: None,
                        duration: 0.3,
                        delay: 0.0,
                        blending: None,
                    },
                },
                TransitionMotionNode::Spring {
                    id: "x".into(),
                    position: Default::default(),
                    label: None,
                    duration: 0.45,
                    bounce: 0.1,
                    delay: 0.0,
                },
                TransitionMotionNode::EaseInOut {
                    timeline: super::super::types::TimelineMotionNode {
                        id: "y".into(),
                        position: Default::default(),
                        label: None,
                        duration: 0.4,
                        delay: 0.0,
                        blending: Some(TimelineBlending {
                            blend_type: super::super::types::TimelineBlendingType::Tween,
                            duration: 0.1,
                            easing: EasingKind::EaseInOut,
                        }),
                    },
                },
            ],
            connections: vec![],
            input_bindings: [("*", "any"), ("Node:x", "x"), ("Node:y", "y")]
                .into_iter()
                .map(
                    |(port, node)| super::super::types::TransitionMotionInputBinding {
                        source: StateValueSource::StateParam {
                            state_param_id: port.into(),
                        },
                        to: super::super::types::GraphEndpoint {
                            node_id: node.into(),
                            port_id: "value".into(),
                        },
                    },
                )
                .collect(),
            output_bindings: [("*", "any"), ("Node:x", "x"), ("Node:y", "y")]
                .into_iter()
                .map(
                    |(port, node)| super::super::types::TransitionMotionOutputBinding {
                        state_param_id: port.into(),
                        from: super::super::types::GraphEndpoint {
                            node_id: node.into(),
                            port_id: "value".into(),
                        },
                    },
                )
                .collect(),
            passthrough_bindings: vec![],
            condition_binding: None,
            layout: None,
            viewport: None,
        };
        let source = [
            (StateParamKey::new("Node:x"), serde_json::json!(0.0)),
            (StateParamKey::new("Node:y"), serde_json::json!(0.0)),
            (StateParamKey::new("Node:z"), serde_json::json!(0.0)),
        ]
        .into_iter()
        .collect();
        let target = [
            (StateParamKey::new("Node:x"), serde_json::json!(1.0)),
            (StateParamKey::new("Node:y"), serde_json::json!(1.0)),
            (StateParamKey::new("Node:z"), serde_json::json!(1.0)),
        ]
        .into_iter()
        .collect();
        let mut engine = MotionEngine::new();
        engine.start_transition("transition", &graph, &source, &target, &HashMap::new());
        let step = engine.step(1.0 / 60.0);
        let drivers = step
            .channels
            .into_iter()
            .map(|channel| (channel.key, channel.driver))
            .collect::<HashMap<_, _>>();
        assert_eq!(drivers.get("Node:x").map(String::as_str), Some("spring"));
        assert_eq!(
            drivers.get("Node:y").map(String::as_str),
            Some("timeline+tween")
        );
        assert_eq!(drivers.get("Node:z").map(String::as_str), Some("timeline"));
    }

    #[test]
    fn wildcard_transition_does_not_drive_mutation_only_channels() {
        let graph = wildcard_spring_graph(0.5, 0.2);
        let state_key = StateParamKey::new("StateInput:value");
        let mutation_key = StateParamKey::new("DerivedPositions:value");
        let mut previous = MotionEngine::with_initial_values(HashMap::from([
            (state_key.clone(), serde_json::json!(0.0)),
            (mutation_key.clone(), serde_json::json!([[0.0, 0.0]])),
        ]));
        previous
            .commit_logical_values(HashMap::from([(state_key.clone(), serde_json::json!(0.0))]));
        previous.begin_mutation_frame();
        previous
            .set_to(&mutation_key, serde_json::json!([[0.0, 0.0]]), None, 0.0)
            .unwrap();

        let mut target = previous.clone();
        target.commit_logical_values(HashMap::from([(state_key.clone(), serde_json::json!(1.0))]));
        target.begin_mutation_frame();
        target
            .set_to(&mutation_key, serde_json::json!([[10.0, 20.0]]), None, 0.0)
            .unwrap();
        target.begin_transition_from("transition", &graph, &previous, &HashSet::from([state_key]));
        target.begin_mutation_frame();
        target
            .set_to(
                &mutation_key,
                serde_json::json!([[10.0, 20.0]]),
                None,
                1.0 / 60.0,
            )
            .unwrap();

        let step = target.step(0.0);
        let state = step
            .channels
            .iter()
            .find(|channel| channel.key == "StateInput:value")
            .unwrap();
        let mutation = step
            .channels
            .iter()
            .find(|channel| channel.key == "DerivedPositions:value")
            .unwrap();
        assert_eq!(state.transition_driver, "spring");
        assert_eq!(mutation.transition_driver, "hold");
        assert!(
            mutation
                .transition_error
                .iter()
                .all(|value| value.abs() <= 1.0e-9)
        );
        assert_eq!(mutation.value, mutation.target_value);
        assert!(
            mutation
                .target_velocity
                .iter()
                .all(|value| value.abs() <= 1.0e-9)
        );
    }

    #[test]
    fn state_mutation_spring_is_solved_before_transition_residual() {
        let graph = wildcard_spring_graph(0.25, 0.15);
        let key = StateParamKey::new("Snap:value");
        let mut previous = MotionEngine::with_initial_values(HashMap::from([(
            key.clone(),
            serde_json::json!(0.3),
        )]));
        previous.commit_logical_values(HashMap::from([(key.clone(), serde_json::json!(0.3))]));

        let mut target = previous.clone();
        target.commit_logical_values(HashMap::from([(key.clone(), serde_json::json!(0.5))]));
        target.seed_targets_from_state([&key]);
        target.begin_mutation_frame();
        target
            .to(&key, serde_json::json!(0.4), 0.8, 0.1, 0.0)
            .unwrap();
        target.begin_transition_from(
            "transition",
            &graph,
            &previous,
            &HashSet::from([key.clone()]),
        );

        let step = target.step(0.0);
        let channel = step
            .channels
            .iter()
            .find(|channel| channel.key == "Snap:value")
            .unwrap();
        assert_eq!(channel.state_value, vec![0.5]);
        assert_eq!(channel.target_value, vec![0.5]);
        assert_eq!(channel.transition_error, vec![0.2]);
        assert_eq!(channel.value, vec![0.3]);
        assert_eq!(channel.mutation_driver, "spring");
        assert_eq!(channel.transition_driver, "spring");
        assert!(
            (channel.value[0] - (channel.target_value[0] - channel.transition_error[0])).abs()
                <= 1.0e-9
        );
    }

    #[test]
    fn integer_channels_remain_continuous_internally_and_round_when_published() {
        let key = StateParamKey::new("Count:value");
        let mut engine = MotionEngine::with_initial_values_and_types(
            HashMap::from([(key.clone(), serde_json::json!(0))]),
            HashMap::from([(key.clone(), "int".to_string())]),
        );
        engine.begin_mutation_frame();
        engine
            .to(&key, serde_json::json!(10), 1.0, 0.0, 0.1)
            .unwrap();

        let internal = engine
            .channels
            .get(&key)
            .expect("integer channel")
            .sample()
            .value
            .components()[0];
        assert!(internal.fract().abs() > f64::EPSILON);
        assert!(
            engine
                .physical_value(&key)
                .and_then(|value| value.as_i64())
                .is_some()
        );
    }
}
