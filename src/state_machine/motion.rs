//! Per-property motion drivers used by state-machine transitions.
//!
//! Springs use the closed-form solution of the damped oscillator. A render
//! frame advances every driver exactly once with the full frame delta.

use std::collections::HashMap;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::easing::timeline_curve;
#[cfg(test)]
use super::types::StateValueSource;
use super::types::{StateParamKey, TimelinePreset, TransitionMotionGraph, TransitionMotionNode};

const ANY_CHANNEL: &str = "*";
const NANOS_PER_SECOND: f64 = 1_000_000_000.0;
const MAX_MUTATION_PLAN_DEPTH: usize = 64;
const MAX_MUTATION_PLAN_NODES: usize = 4096;
const MAX_MUTATION_PLAN_OPS_PER_STEP: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq)]
pub enum MutationTiming {
    Linear {
        duration: f64,
        delay: f64,
    },
    Spring {
        duration: f64,
        bounce: f64,
        delay: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum MutationPlan {
    SetTo {
        target: serde_json::Value,
        velocity: Option<serde_json::Value>,
    },
    To {
        target: serde_json::Value,
        timing: MutationTiming,
    },
    Sequence(Vec<MutationPlan>),
    Repeat {
        child: Box<MutationPlan>,
        count: i64,
    },
    Delay {
        child: Box<MutationPlan>,
        delay: f64,
    },
}

impl MutationPlan {
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut nodes = 0;
        self.validate_inner(0, &mut nodes)?;
        Ok(())
    }

    fn validate_inner(&self, depth: usize, nodes: &mut usize) -> anyhow::Result<()> {
        if depth > MAX_MUTATION_PLAN_DEPTH {
            anyhow::bail!("Mutation plan exceeds maximum nesting depth");
        }
        *nodes += 1;
        if *nodes > MAX_MUTATION_PLAN_NODES {
            anyhow::bail!("Mutation plan exceeds maximum node count");
        }
        match self {
            Self::SetTo { target, velocity } => {
                let target = finite_numeric_value(target, "setTo target")?;
                if let Some(velocity) = velocity {
                    let velocity = finite_numeric_value(velocity, "setTo velocity")?;
                    if !target.has_same_shape(&velocity) {
                        anyhow::bail!("setTo velocity shape changed");
                    }
                }
            }
            Self::To { target, timing } => {
                finite_numeric_value(target, "to target")?;
                match timing {
                    MutationTiming::Linear { duration, delay } => {
                        validate_non_negative_finite(*duration, "linear duration")?;
                        validate_non_negative_finite(*delay, "linear delay")?;
                    }
                    MutationTiming::Spring {
                        duration,
                        bounce,
                        delay,
                    } => {
                        if !duration.is_finite() || *duration <= 0.0 {
                            anyhow::bail!("spring duration must be > 0");
                        }
                        if !bounce.is_finite() || *bounce <= -1.0 || *bounce >= 1.0 {
                            anyhow::bail!("spring bounce must be in (-1, 1)");
                        }
                        validate_non_negative_finite(*delay, "spring delay")?;
                    }
                }
            }
            Self::Sequence(children) => {
                if children.is_empty() {
                    anyhow::bail!("sequence(...) requires at least one child");
                }
                for child in children {
                    child.validate_inner(depth + 1, nodes)?;
                }
            }
            Self::Repeat { child, count } => {
                if *count != -1 && *count < 1 {
                    anyhow::bail!("repeat count must be -1 or an integer >= 1");
                }
                child.validate_inner(depth + 1, nodes)?;
                if *count == -1 && child.minimum_duration() <= 0.0 {
                    anyhow::bail!("infinite repeat requires a child that consumes time");
                }
            }
            Self::Delay { child, delay } => {
                validate_non_negative_finite(*delay, "plan delay")?;
                child.validate_inner(depth + 1, nodes)?;
            }
        }
        Ok(())
    }

    fn minimum_duration(&self) -> f64 {
        match self {
            Self::SetTo { .. } => 0.0,
            Self::To { timing, .. } => match timing {
                MutationTiming::Linear { duration, delay }
                | MutationTiming::Spring {
                    duration, delay, ..
                } => duration + delay,
            },
            Self::Sequence(children) => children.iter().map(Self::minimum_duration).sum(),
            Self::Repeat { child, count } => {
                if *count == -1 {
                    f64::INFINITY
                } else {
                    child.minimum_duration() * *count as f64
                }
            }
            Self::Delay { child, delay } => delay + child.minimum_duration(),
        }
    }

    fn first_target(&self) -> Option<&serde_json::Value> {
        match self {
            Self::SetTo { target, .. } | Self::To { target, .. } => Some(target),
            Self::Sequence(children) => children.iter().find_map(Self::first_target),
            Self::Repeat { child, .. } | Self::Delay { child, .. } => child.first_target(),
        }
    }
}

fn validate_non_negative_finite(value: f64, label: &str) -> anyhow::Result<()> {
    if !value.is_finite() || value < 0.0 {
        anyhow::bail!("{label} must be finite and >= 0");
    }
    Ok(())
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_plan_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_repeat_iteration: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_repeat_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_delay_remaining: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_plan_completed: Option<bool>,
    pub transition_driver: String,
    pub timeline_progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_timing_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_timing_node_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub canceled_timing_node_ids: Vec<String>,
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
                .unwrap_or_else(PlanTemplate::instant);
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

    /// Apply one immutable Mutation plan descriptor. Bare `setTo` and an
    /// undelayed bare spring retain their frame-driven retargeting behavior;
    /// composed plans own a persistent cursor and advance exactly once here.
    pub fn apply_mutation_plan(
        &mut self,
        key: &StateParamKey,
        plan: MutationPlan,
        dt: f64,
    ) -> anyhow::Result<serde_json::Value> {
        if !self.mutation_frame_calls.insert(key.clone()) {
            anyhow::bail!(
                "Motion output '{}' was applied more than once this frame",
                key_string(key)
            );
        }
        plan.validate()?;
        let fallback = self
            .current_values
            .get(key)
            .cloned()
            .or_else(|| plan.first_target().cloned())
            .ok_or_else(|| anyhow::anyhow!("Mutation plan has no target value"))?;
        let channel = self
            .channels
            .entry(key.clone())
            .or_insert_with(|| Channel::hold(fallback));
        channel.apply_mutation_plan(plan, kotlin_frame_seconds(dt))?;
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
            let mutation_debug = channel.mutation_debug();
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
                mutation_plan_path: mutation_debug.path,
                mutation_repeat_iteration: mutation_debug.repeat_iteration,
                mutation_repeat_count: mutation_debug.repeat_count,
                mutation_delay_remaining: mutation_debug.delay_remaining,
                mutation_plan_completed: mutation_debug.completed,
                transition_driver: error.driver.to_string(),
                timeline_progress: sample.timeline_progress,
                current_timing_node_id: channel.current_timing_node_id(),
                pending_timing_node_ids: channel.pending_timing_node_ids(),
                canceled_timing_node_ids: channel.canceled_timing_node_ids(),
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
    fallback: Option<PlanTemplate>,
    specific: HashMap<String, PlanTemplate>,
}

fn compile_channel_plans(graph: &TransitionMotionGraph) -> CompiledPlans {
    #[derive(Clone)]
    struct Candidate {
        anchor: String,
        order: usize,
    }

    #[derive(Clone)]
    struct ReducedEdge {
        source: String,
        target: String,
        order: usize,
        plan: PlanTemplate,
    }

    fn input_anchor(property: &str) -> String {
        format!("input:{property}")
    }

    fn output_anchor(property: &str) -> String {
        format!("output:{property}")
    }

    fn waypoint_anchor(node_id: &str) -> String {
        format!("waypoint:{node_id}")
    }

    fn reduce_plan(source: &str, target: &str, active: Vec<ReducedEdge>) -> Option<PlanTemplate> {
        let mut edges = active;
        while edges.len() > 1 {
            let mut parallel_group = None;
            'outer: for left in 0..edges.len() {
                let group = (left..edges.len())
                    .filter(|right| {
                        edges[*right].source == edges[left].source
                            && edges[*right].target == edges[left].target
                    })
                    .collect::<Vec<_>>();
                if group.len() > 1 {
                    parallel_group = Some(group);
                    break 'outer;
                }
            }
            if let Some(mut indices) = parallel_group {
                indices.sort_unstable();
                let mut members = indices
                    .iter()
                    .map(|index| edges[*index].clone())
                    .collect::<Vec<_>>();
                members.sort_by_key(|edge| edge.order);
                let source = members[0].source.clone();
                let target = members[0].target.clone();
                let order = members.iter().map(|edge| edge.order).min().unwrap_or(0);
                let mut children = Vec::new();
                for member in members {
                    match member.plan {
                        PlanTemplate::Parallel(nested) => children.extend(nested),
                        plan => children.push(plan),
                    }
                }
                for index in indices.into_iter().rev() {
                    edges.remove(index);
                }
                edges.push(ReducedEdge {
                    source,
                    target,
                    order,
                    plan: PlanTemplate::Parallel(children),
                });
                continue;
            }

            let mut anchors = edges
                .iter()
                .flat_map(|edge| [edge.source.clone(), edge.target.clone()])
                .filter(|anchor| {
                    anchor != source && anchor != target && anchor.starts_with("waypoint:")
                })
                .collect::<Vec<_>>();
            anchors.sort();
            anchors.dedup();
            let serial = anchors.into_iter().find_map(|anchor| {
                let incoming = edges
                    .iter()
                    .enumerate()
                    .filter(|(_, edge)| edge.target == anchor)
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                let outgoing = edges
                    .iter()
                    .enumerate()
                    .filter(|(_, edge)| edge.source == anchor)
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                (incoming.len() == 1 && outgoing.len() == 1).then_some((incoming[0], outgoing[0]))
            });
            let Some((incoming_index, outgoing_index)) = serial else {
                break;
            };
            let incoming = edges[incoming_index].clone();
            let outgoing = edges[outgoing_index].clone();
            let mut children = Vec::new();
            match incoming.plan {
                PlanTemplate::Sequence(nested) => children.extend(nested),
                plan => children.push(plan),
            }
            match outgoing.plan {
                PlanTemplate::Sequence(nested) => children.extend(nested),
                plan => children.push(plan),
            }
            let mut remove = [incoming_index, outgoing_index];
            remove.sort_unstable();
            for index in remove.into_iter().rev() {
                edges.remove(index);
            }
            edges.push(ReducedEdge {
                source: incoming.source,
                target: outgoing.target,
                order: incoming.order,
                plan: PlanTemplate::Sequence(children),
            });
        }
        (edges.len() == 1 && edges[0].source == source && edges[0].target == target)
            .then(|| edges.remove(0).plan)
    }

    let nodes: HashMap<&str, &TransitionMotionNode> =
        graph.nodes.iter().map(|node| (node.id(), node)).collect();
    let mut sources: HashMap<&str, Vec<Candidate>> = HashMap::new();
    let mut targets: HashMap<&str, Vec<Candidate>> = HashMap::new();
    for (index, binding) in graph.input_bindings.iter().enumerate() {
        if nodes
            .get(binding.to.node_id.as_str())
            .is_some_and(|node| node.is_timing())
            && let Some(property) = binding.source.state_param_id()
        {
            sources
                .entry(binding.to.node_id.as_str())
                .or_default()
                .push(Candidate {
                    anchor: input_anchor(property),
                    order: index,
                });
        }
    }
    for (index, binding) in graph.output_bindings.iter().enumerate() {
        if nodes
            .get(binding.from.node_id.as_str())
            .is_some_and(|node| node.is_timing())
        {
            targets
                .entry(binding.from.node_id.as_str())
                .or_default()
                .push(Candidate {
                    anchor: output_anchor(&binding.state_param_id),
                    order: index,
                });
        }
    }
    for (index, connection) in graph.connections.iter().enumerate() {
        let source = nodes.get(connection.from.node_id.as_str()).copied();
        let target = nodes.get(connection.to.node_id.as_str()).copied();
        if source.is_some_and(TransitionMotionNode::is_timing)
            && matches!(target, Some(TransitionMotionNode::Waypoint { .. }))
            && connection.from.port_id == "value"
            && connection.to.port_id == "in"
        {
            targets
                .entry(connection.from.node_id.as_str())
                .or_default()
                .push(Candidate {
                    anchor: waypoint_anchor(&connection.to.node_id),
                    order: index,
                });
        } else if matches!(source, Some(TransitionMotionNode::Waypoint { .. }))
            && target.is_some_and(TransitionMotionNode::is_timing)
            && connection.from.port_id == "value"
            && connection.to.port_id == "value"
        {
            sources
                .entry(connection.to.node_id.as_str())
                .or_default()
                .push(Candidate {
                    anchor: waypoint_anchor(&connection.from.node_id),
                    order: index,
                });
        }
    }

    let mut edges = Vec::new();
    for node in graph.nodes.iter().filter(|node| node.is_timing()) {
        let node_sources = sources.get(node.id()).cloned().unwrap_or_default();
        let node_targets = targets.get(node.id()).cloned().unwrap_or_default();
        if node_sources.len() != 1 || node_targets.len() != 1 {
            continue;
        }
        let source = &node_sources[0];
        let target = &node_targets[0];
        let Some(motion) = MotionPlan::from_node(node) else {
            continue;
        };
        let endpoint = target
            .anchor
            .strip_prefix("waypoint:")
            .and_then(|node_id| nodes.get(node_id))
            .and_then(|node| match node {
                TransitionMotionNode::Waypoint { value, .. } => {
                    NumericValue::from_json(value).map(PlanTarget::Waypoint)
                }
                _ => None,
            })
            .unwrap_or(PlanTarget::StateOut);
        edges.push(ReducedEdge {
            source: source.anchor.clone(),
            target: target.anchor.clone(),
            order: source.order,
            plan: PlanTemplate::Segment(SegmentTemplate {
                timing_node_id: node.id().to_string(),
                motion,
                target: endpoint,
            }),
        });
    }

    let mut plans = CompiledPlans {
        fallback: None,
        specific: HashMap::new(),
    };
    let mut properties = edges
        .iter()
        .filter_map(|edge| edge.source.strip_prefix("input:"))
        .map(str::to_string)
        .collect::<Vec<_>>();
    properties.sort();
    properties.dedup();
    for property in properties {
        let source = input_anchor(&property);
        let target = output_anchor(&property);
        let mut forward = HashSet::from([source.clone()]);
        loop {
            let before = forward.len();
            for edge in &edges {
                if forward.contains(&edge.source) {
                    forward.insert(edge.target.clone());
                }
            }
            if forward.len() == before {
                break;
            }
        }
        let mut backward = HashSet::from([target.clone()]);
        loop {
            let before = backward.len();
            for edge in &edges {
                if backward.contains(&edge.target) {
                    backward.insert(edge.source.clone());
                }
            }
            if backward.len() == before {
                break;
            }
        }
        let active = edges
            .iter()
            .filter(|edge| forward.contains(&edge.source) && backward.contains(&edge.target))
            .cloned()
            .collect::<Vec<_>>();
        let Some(plan) = reduce_plan(&source, &target, active) else {
            continue;
        };
        if property == ANY_CHANNEL {
            plans.fallback = Some(plan);
        } else {
            plans.specific.insert(property, plan);
        }
    }
    for passthrough in &graph.passthrough_bindings {
        let plan = PlanTemplate::Segment(SegmentTemplate {
            timing_node_id: "__passthrough__".into(),
            motion: MotionPlan::Instant,
            target: PlanTarget::StateOut,
        });
        if passthrough.state_param_id == ANY_CHANNEL {
            plans.fallback = Some(plan);
        } else {
            plans
                .specific
                .insert(passthrough.state_param_id.clone(), plan);
        }
    }
    plans
}

#[derive(Debug, Clone)]
enum PlanTemplate {
    Segment(SegmentTemplate),
    Sequence(Vec<PlanTemplate>),
    Parallel(Vec<PlanTemplate>),
}

impl PlanTemplate {
    fn instant() -> Self {
        Self::Segment(SegmentTemplate {
            timing_node_id: "__instant__".into(),
            motion: MotionPlan::Instant,
            target: PlanTarget::StateOut,
        })
    }

    fn direct_state_out(&self) -> Option<&SegmentTemplate> {
        match self {
            Self::Segment(segment) if matches!(segment.target, PlanTarget::StateOut) => {
                Some(segment)
            }
            _ => None,
        }
    }
}

impl From<MotionPlan> for PlanTemplate {
    fn from(motion: MotionPlan) -> Self {
        Self::Segment(SegmentTemplate {
            timing_node_id: "__test__".into(),
            motion,
            target: PlanTarget::StateOut,
        })
    }
}

#[derive(Debug, Clone)]
struct SegmentTemplate {
    timing_node_id: String,
    motion: MotionPlan,
    target: PlanTarget,
}

#[derive(Debug, Clone)]
enum PlanTarget {
    Waypoint(NumericValue),
    StateOut,
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

    fn delay(&self) -> f64 {
        match self {
            Self::Spring { delay, .. } | Self::Timeline { delay, .. } => *delay,
            Self::Instant => 0.0,
        }
    }

    fn without_delay(&self) -> Self {
        match self {
            Self::Spring {
                duration, bounce, ..
            } => Self::Spring {
                duration: *duration,
                bounce: *bounce,
                delay: 0.0,
            },
            Self::Timeline {
                duration, curve, ..
            } => Self::Timeline {
                duration: *duration,
                delay: 0.0,
                curve: *curve,
            },
            Self::Instant => Self::Instant,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanStatus {
    Dormant,
    Pending,
    Running,
    Completed,
    Canceled,
}

#[derive(Debug, Clone)]
struct RuntimePlanNode {
    parent: Option<usize>,
    status: PlanStatus,
    kind: RuntimePlanKind,
}

#[derive(Debug, Clone)]
enum RuntimePlanKind {
    Segment(SegmentTemplate),
    Sequence {
        children: Vec<usize>,
    },
    Parallel {
        children: Vec<usize>,
        owner: Option<usize>,
    },
}

#[derive(Debug, Clone)]
struct StartEvent {
    at: f64,
    order: u64,
    segment_id: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentMode {
    Physical,
    Error,
}

#[derive(Debug, Clone)]
struct ActiveSegment {
    node_id: usize,
    mode: SegmentMode,
    driver: Driver,
}

#[derive(Debug, Clone)]
struct PlanExecutor {
    nodes: Vec<RuntimePlanNode>,
    root: usize,
    events: Vec<StartEvent>,
    next_event_order: u64,
    active: Option<ActiveSegment>,
    physical: NumericSample,
    canceled_timing_node_ids: Vec<String>,
    time: f64,
}

impl PlanExecutor {
    fn new(plan: PlanTemplate, current: DriverSample, target: &DriverSample) -> Self {
        fn build(
            template: PlanTemplate,
            parent: Option<usize>,
            nodes: &mut Vec<RuntimePlanNode>,
        ) -> usize {
            match template {
                PlanTemplate::Segment(segment) => {
                    let id = nodes.len();
                    nodes.push(RuntimePlanNode {
                        parent,
                        status: PlanStatus::Dormant,
                        kind: RuntimePlanKind::Segment(segment),
                    });
                    id
                }
                PlanTemplate::Sequence(templates) => {
                    let id = nodes.len();
                    nodes.push(RuntimePlanNode {
                        parent,
                        status: PlanStatus::Dormant,
                        kind: RuntimePlanKind::Sequence { children: vec![] },
                    });
                    let children = templates
                        .into_iter()
                        .map(|template| build(template, Some(id), nodes))
                        .collect();
                    nodes[id].kind = RuntimePlanKind::Sequence { children };
                    id
                }
                PlanTemplate::Parallel(templates) => {
                    let id = nodes.len();
                    nodes.push(RuntimePlanNode {
                        parent,
                        status: PlanStatus::Dormant,
                        kind: RuntimePlanKind::Parallel {
                            children: vec![],
                            owner: None,
                        },
                    });
                    let children = templates
                        .into_iter()
                        .map(|template| build(template, Some(id), nodes))
                        .collect();
                    nodes[id].kind = RuntimePlanKind::Parallel {
                        children,
                        owner: None,
                    };
                    id
                }
            }
        }

        let physical = NumericSample {
            value: current.value,
            velocity: current.velocity,
        };
        let mut nodes = Vec::new();
        let root = build(plan, None, &mut nodes);
        let mut executor = Self {
            nodes,
            root,
            events: vec![],
            next_event_order: 0,
            active: None,
            physical,
            canceled_timing_node_ids: vec![],
            time: 0.0,
        };
        executor.activate_node(root, 0.0);
        executor.settle(target);
        executor
    }

    fn activate_node(&mut self, node_id: usize, at: f64) {
        if matches!(
            self.nodes[node_id].status,
            PlanStatus::Canceled | PlanStatus::Completed
        ) {
            return;
        }
        let kind = self.nodes[node_id].kind.clone();
        match kind {
            RuntimePlanKind::Segment(segment) => {
                self.nodes[node_id].status = PlanStatus::Pending;
                let order = self.next_event_order;
                self.next_event_order += 1;
                self.events.push(StartEvent {
                    at: at + segment.motion.delay(),
                    order,
                    segment_id: node_id,
                });
            }
            RuntimePlanKind::Sequence { children } => {
                self.nodes[node_id].status = PlanStatus::Running;
                if let Some(first) = children.first() {
                    self.activate_node(*first, at);
                } else {
                    self.complete_node(node_id, at);
                }
            }
            RuntimePlanKind::Parallel { children, .. } => {
                self.nodes[node_id].status = PlanStatus::Running;
                if children.is_empty() {
                    self.complete_node(node_id, at);
                } else {
                    for child in children {
                        self.activate_node(child, at);
                    }
                }
            }
        }
    }

    fn cancel_subtree(&mut self, node_id: usize) {
        if self.nodes[node_id].status == PlanStatus::Canceled {
            return;
        }
        let kind = self.nodes[node_id].kind.clone();
        self.nodes[node_id].status = PlanStatus::Canceled;
        match kind {
            RuntimePlanKind::Segment(segment) => {
                if !self
                    .canceled_timing_node_ids
                    .contains(&segment.timing_node_id)
                {
                    self.canceled_timing_node_ids
                        .push(segment.timing_node_id.clone());
                }
                if self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.node_id == node_id)
                {
                    self.active = None;
                }
            }
            RuntimePlanKind::Sequence { children } | RuntimePlanKind::Parallel { children, .. } => {
                for child in children {
                    self.cancel_subtree(child);
                }
            }
        }
    }

    fn claim_parallel_owners(&mut self, segment_id: usize) {
        let mut ancestry = Vec::new();
        let mut child = segment_id;
        while let Some(parent) = self.nodes[child].parent {
            ancestry.push((parent, child));
            child = parent;
        }
        ancestry.reverse();
        for (parent, direct_child) in ancestry {
            let old_owner = match &self.nodes[parent].kind {
                RuntimePlanKind::Parallel { owner, .. } => *owner,
                _ => continue,
            };
            if old_owner == Some(direct_child) {
                continue;
            }
            if let Some(old_owner) = old_owner {
                self.cancel_subtree(old_owner);
            }
            if let RuntimePlanKind::Parallel { owner, .. } = &mut self.nodes[parent].kind {
                *owner = Some(direct_child);
            }
        }
    }

    fn start_segment(&mut self, node_id: usize, target: &DriverSample) {
        if self.nodes[node_id].status != PlanStatus::Pending {
            return;
        }
        let RuntimePlanKind::Segment(segment) = self.nodes[node_id].kind.clone() else {
            return;
        };
        let current = self.physical_sample(target);
        self.physical = NumericSample {
            value: current.value.clone(),
            velocity: current.velocity.clone(),
        };
        self.claim_parallel_owners(node_id);
        self.nodes[node_id].status = PlanStatus::Running;

        let (mode, source, source_velocity, destination) = match &segment.target {
            PlanTarget::Waypoint(value) => (
                SegmentMode::Physical,
                current.value,
                current.velocity,
                value.clone(),
            ),
            PlanTarget::StateOut => {
                let error = subtract_values(&target.value, &current.value);
                let error_velocity = subtract_values(&target.velocity, &current.velocity);
                let zero = error.same_shape(vec![0.0; error.len()]);
                (SegmentMode::Error, error, error_velocity, zero)
            }
        };
        let outgoing = Driver::Hold(NumericSample {
            value: source.clone(),
            velocity: source_velocity.clone(),
        });
        let motion = segment.motion.without_delay();
        let driver = if matches!(motion, MotionPlan::Spring { .. })
            && same_components(&source, &destination)
            && source_velocity
                .components()
                .iter()
                .all(|velocity| *velocity == 0.0)
        {
            Driver::Hold(NumericSample {
                value: destination.clone(),
                velocity: destination.same_shape(vec![0.0; destination.len()]),
            })
        } else {
            Driver::start_numeric(Some(outgoing), source, destination, motion)
                .with_initial_velocity(source_velocity)
        };
        self.active = Some(ActiveSegment {
            node_id,
            mode,
            driver,
        });
    }

    fn complete_node(&mut self, node_id: usize, at: f64) {
        if matches!(
            self.nodes[node_id].status,
            PlanStatus::Completed | PlanStatus::Canceled
        ) {
            return;
        }
        self.nodes[node_id].status = PlanStatus::Completed;
        let Some(parent) = self.nodes[node_id].parent else {
            return;
        };
        if self.nodes[parent].status == PlanStatus::Canceled {
            return;
        }
        match self.nodes[parent].kind.clone() {
            RuntimePlanKind::Sequence { children } => {
                let Some(index) = children.iter().position(|child| *child == node_id) else {
                    return;
                };
                if let Some(next) = children.get(index + 1) {
                    self.activate_node(*next, at);
                } else {
                    self.complete_node(parent, at);
                }
            }
            RuntimePlanKind::Parallel {
                children, owner, ..
            } => {
                if owner != Some(node_id) {
                    return;
                }
                let waiting = children.iter().any(|child| {
                    !matches!(
                        self.nodes[*child].status,
                        PlanStatus::Completed | PlanStatus::Canceled
                    )
                });
                if !waiting {
                    self.complete_node(parent, at);
                }
            }
            RuntimePlanKind::Segment(_) => {}
        }
    }

    fn next_due_event_index(&self) -> Option<usize> {
        self.events
            .iter()
            .enumerate()
            .filter(|(_, event)| {
                self.nodes[event.segment_id].status == PlanStatus::Pending
                    && event.at <= self.time + f64::EPSILON
            })
            .min_by(|(_, left), (_, right)| {
                left.at
                    .total_cmp(&right.at)
                    .then(left.order.cmp(&right.order))
            })
            .map(|(index, _)| index)
    }

    fn next_event_time(&self) -> Option<f64> {
        self.events
            .iter()
            .filter(|event| self.nodes[event.segment_id].status == PlanStatus::Pending)
            .map(|event| event.at)
            .min_by(f64::total_cmp)
    }

    fn complete_active_if_ready(&mut self, target: &DriverSample) -> bool {
        let Some(active) = &self.active else {
            return false;
        };
        if !active.driver.sample().completed {
            return false;
        }
        let physical = self.physical_sample(target);
        self.physical = NumericSample {
            value: physical.value,
            velocity: physical.velocity,
        };
        let node_id = self.active.take().map(|active| active.node_id).unwrap_or(0);
        self.complete_node(node_id, self.time);
        true
    }

    fn settle(&mut self, target: &DriverSample) -> bool {
        let mut any = false;
        loop {
            if self.complete_active_if_ready(target) {
                any = true;
                continue;
            }
            let Some(index) = self.next_due_event_index() else {
                break;
            };
            let event = self.events.remove(index);
            self.start_segment(event.segment_id, target);
            any = true;
        }
        any
    }

    fn step(&mut self, dt: f64, target: &DriverSample) {
        self.settle(target);
        let end = self.time + dt.max(0.0);
        while self.time < end && self.nodes[self.root].status != PlanStatus::Completed {
            let remaining = end - self.time;
            let event_delta = self.next_event_time().map(|at| (at - self.time).max(0.0));
            let completion_delta = self
                .active
                .as_ref()
                .and_then(|active| active.driver.remaining_duration());
            let mut advance = remaining;
            if let Some(delta) = event_delta {
                advance = advance.min(delta);
            }
            if let Some(delta) = completion_delta {
                advance = advance.min(delta.max(0.0));
            }
            if advance > 0.0 {
                if let Some(active) = &mut self.active {
                    active.driver.step(advance);
                }
                self.time += advance;
                let physical = self.physical_sample(target);
                self.physical = NumericSample {
                    value: physical.value,
                    velocity: physical.velocity,
                };
            }
            let progressed = self.settle(target);
            if advance <= 0.0 && !progressed {
                break;
            }
        }
        if self.time < end {
            self.time = end;
        }
    }

    fn physical_sample(&self, target: &DriverSample) -> DriverSample {
        let completed = self.nodes[self.root].status == PlanStatus::Completed;
        let Some(active) = &self.active else {
            return DriverSample::numeric(self.physical.clone(), "hold", completed, None);
        };
        let sample = active.driver.sample();
        match active.mode {
            SegmentMode::Physical => DriverSample {
                completed,
                ..sample
            },
            SegmentMode::Error => DriverSample {
                value: subtract_values(&target.value, &sample.value),
                velocity: subtract_values(&target.velocity, &sample.velocity),
                driver: sample.driver,
                completed,
                persistent: false,
                timeline_progress: sample.timeline_progress,
            },
        }
    }

    fn error_sample(&self, target: &DriverSample) -> DriverSample {
        let physical = self.physical_sample(target);
        DriverSample {
            value: subtract_values(&target.value, &physical.value),
            velocity: subtract_values(&target.velocity, &physical.velocity),
            driver: physical.driver,
            completed: physical.completed,
            persistent: false,
            timeline_progress: physical.timeline_progress,
        }
    }

    fn current_timing_node_id(&self) -> Option<String> {
        self.active.as_ref().and_then(|active| {
            let RuntimePlanKind::Segment(segment) = &self.nodes[active.node_id].kind else {
                return None;
            };
            Some(segment.timing_node_id.clone())
        })
    }

    fn pending_timing_node_ids(&self) -> Vec<String> {
        let mut events = self
            .events
            .iter()
            .filter(|event| self.nodes[event.segment_id].status == PlanStatus::Pending)
            .collect::<Vec<_>>();
        events.sort_by(|left, right| {
            left.at
                .total_cmp(&right.at)
                .then(left.order.cmp(&right.order))
        });
        events
            .into_iter()
            .filter_map(|event| match &self.nodes[event.segment_id].kind {
                RuntimePlanKind::Segment(segment) => Some(segment.timing_node_id.clone()),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
struct MutationInstruction {
    path: String,
    kind: MutationInstructionKind,
}

#[derive(Debug, Clone)]
enum MutationInstructionKind {
    SetTo {
        target: NumericValue,
        velocity: Option<NumericValue>,
    },
    Linear {
        target: NumericValue,
        duration: f64,
    },
    Spring {
        target: NumericValue,
        duration: f64,
        bounce: f64,
    },
    Wait {
        duration: f64,
    },
    RepeatStart {
        count: i64,
    },
    RepeatEnd {
        start: usize,
    },
}

#[derive(Debug, Clone)]
struct RepeatCursor {
    start: usize,
    count: i64,
    iteration: u64,
}

#[derive(Debug, Clone)]
enum ActiveMutationSegment {
    Wait {
        remaining: f64,
        path: String,
    },
    Linear {
        driver: TimelineDriver,
        path: String,
    },
    Spring {
        driver: SpringDriver,
        path: String,
    },
}

#[derive(Debug, Clone, Default)]
struct MutationPlanDebug {
    path: Option<String>,
    repeat_iteration: Option<u64>,
    repeat_count: Option<i64>,
    delay_remaining: Option<f64>,
    completed: Option<bool>,
}

#[derive(Debug, Clone)]
struct MutationPlanExecutor {
    plan: MutationPlan,
    instructions: Vec<MutationInstruction>,
    pc: usize,
    repeats: Vec<RepeatCursor>,
    active: Option<ActiveMutationSegment>,
    sample: NumericSample,
    completed: bool,
}

impl MutationPlanExecutor {
    fn new(plan: MutationPlan, initial: DriverSample) -> anyhow::Result<Self> {
        plan.validate()?;
        let mut instructions = Vec::new();
        compile_mutation_plan(&plan, "root", &initial.value, &mut instructions)?;
        let mut executor = Self {
            plan,
            instructions,
            pc: 0,
            repeats: Vec::new(),
            active: None,
            sample: NumericSample {
                value: initial.value,
                velocity: initial.velocity,
            },
            completed: false,
        };
        executor.advance(0.0)?;
        Ok(executor)
    }

    fn matches(&self, plan: &MutationPlan) -> bool {
        &self.plan == plan
    }

    fn advance(&mut self, dt: f64) -> anyhow::Result<()> {
        if self.completed {
            return Ok(());
        }
        let mut remaining = dt.max(0.0);
        let mut operations = 0usize;
        loop {
            operations += 1;
            if operations > MAX_MUTATION_PLAN_OPS_PER_STEP {
                anyhow::bail!("Mutation plan exceeded the per-frame operation budget");
            }

            if let Some(active) = &mut self.active {
                match active {
                    ActiveMutationSegment::Wait {
                        remaining: wait, ..
                    } => {
                        if remaining <= 0.0 {
                            break;
                        }
                        let consumed = remaining.min(*wait);
                        *wait -= consumed;
                        remaining -= consumed;
                        if *wait <= f64::EPSILON {
                            self.active = None;
                            self.pc += 1;
                            continue;
                        }
                        break;
                    }
                    ActiveMutationSegment::Linear { driver, .. } => {
                        if remaining <= 0.0 {
                            break;
                        }
                        let consumed =
                            remaining.min((driver.duration.max(0.0) - driver.elapsed).max(0.0));
                        driver.step(consumed);
                        let sample = driver.sample();
                        self.sample = NumericSample {
                            value: sample.value,
                            velocity: sample.velocity,
                        };
                        remaining -= consumed;
                        if sample.completed {
                            self.active = None;
                            self.pc += 1;
                            continue;
                        }
                        break;
                    }
                    ActiveMutationSegment::Spring { driver, .. } => {
                        if remaining <= 0.0 {
                            break;
                        }
                        let consumed = advance_spring_exact(driver, remaining);
                        let sample = driver.sample();
                        self.sample = NumericSample {
                            value: sample.value,
                            velocity: sample.velocity,
                        };
                        remaining = (remaining - consumed).max(0.0);
                        if sample.completed {
                            self.active = None;
                            self.pc += 1;
                            continue;
                        }
                        break;
                    }
                }
            }

            let Some(instruction) = self.instructions.get(self.pc).cloned() else {
                self.completed = true;
                self.sample.velocity = self
                    .sample
                    .value
                    .same_shape(vec![0.0; self.sample.value.len()]);
                break;
            };
            match instruction.kind {
                MutationInstructionKind::SetTo { target, velocity } => {
                    self.sample = NumericSample {
                        velocity: velocity
                            .unwrap_or_else(|| target.same_shape(vec![0.0; target.len()])),
                        value: target,
                    };
                    self.pc += 1;
                }
                MutationInstructionKind::Wait { duration } => {
                    if duration <= 0.0 {
                        self.pc += 1;
                        continue;
                    }
                    self.sample.velocity =
                        self.sample
                            .value
                            .same_shape(vec![0.0; self.sample.value.len()]);
                    self.active = Some(ActiveMutationSegment::Wait {
                        remaining: duration,
                        path: instruction.path,
                    });
                }
                MutationInstructionKind::Linear { target, duration } => {
                    if duration <= 0.0 {
                        self.sample = NumericSample {
                            velocity: target.same_shape(vec![0.0; target.len()]),
                            value: target,
                        };
                        self.pc += 1;
                        continue;
                    }
                    self.active = Some(ActiveMutationSegment::Linear {
                        driver: TimelineDriver::new(
                            self.sample.value.clone(),
                            target,
                            duration,
                            TimelinePreset::Linear,
                        ),
                        path: instruction.path,
                    });
                }
                MutationInstructionKind::Spring {
                    target,
                    duration,
                    bounce,
                } => {
                    if same_components(&self.sample.value, &target)
                        && self
                            .sample
                            .velocity
                            .components()
                            .iter()
                            .all(|velocity| *velocity == 0.0)
                    {
                        self.sample.value = target;
                        self.pc += 1;
                        continue;
                    }
                    self.active = Some(ActiveMutationSegment::Spring {
                        driver: SpringDriver::new(
                            self.sample.value.clone(),
                            self.sample.velocity.clone(),
                            target,
                            duration,
                            bounce,
                        ),
                        path: instruction.path,
                    });
                }
                MutationInstructionKind::RepeatStart { count } => {
                    self.repeats.push(RepeatCursor {
                        start: self.pc,
                        count,
                        iteration: 0,
                    });
                    self.pc += 1;
                }
                MutationInstructionKind::RepeatEnd { start } => {
                    let cursor = self.repeats.last_mut().ok_or_else(|| {
                        anyhow::anyhow!("Mutation repeat cursor stack is unbalanced")
                    })?;
                    if cursor.start != start {
                        anyhow::bail!("Mutation repeat cursor does not match its plan");
                    }
                    cursor.iteration = cursor.iteration.saturating_add(1);
                    if cursor.count == -1 || cursor.iteration < cursor.count as u64 {
                        self.pc = start + 1;
                    } else {
                        self.repeats.pop();
                        self.pc += 1;
                    }
                }
            }
        }
        Ok(())
    }

    fn sample(&self) -> DriverSample {
        let (driver, timeline_progress) = match &self.active {
            Some(ActiveMutationSegment::Wait { .. }) => ("mutation-delay", None),
            Some(ActiveMutationSegment::Linear { driver, .. }) => {
                ("mutation-linear", driver.sample().timeline_progress)
            }
            Some(ActiveMutationSegment::Spring { .. }) => ("mutation-spring", None),
            None => ("mutation-plan", None),
        };
        DriverSample {
            value: self.sample.value.clone(),
            velocity: self.sample.velocity.clone(),
            driver,
            completed: self.completed,
            persistent: !self.completed,
            timeline_progress,
        }
    }

    fn debug(&self) -> MutationPlanDebug {
        let path = self.active.as_ref().map(|active| match active {
            ActiveMutationSegment::Wait { path, .. }
            | ActiveMutationSegment::Linear { path, .. }
            | ActiveMutationSegment::Spring { path, .. } => path.clone(),
        });
        let delay_remaining = match &self.active {
            Some(ActiveMutationSegment::Wait { remaining, .. }) => Some(*remaining),
            _ => None,
        };
        let repeat = self.repeats.last();
        MutationPlanDebug {
            path,
            repeat_iteration: repeat.map(|cursor| cursor.iteration + 1),
            repeat_count: repeat.map(|cursor| cursor.count),
            delay_remaining,
            completed: Some(self.completed),
        }
    }
}

fn compile_mutation_plan(
    plan: &MutationPlan,
    path: &str,
    expected_shape: &NumericValue,
    instructions: &mut Vec<MutationInstruction>,
) -> anyhow::Result<()> {
    match plan {
        MutationPlan::SetTo { target, velocity } => {
            let target = finite_numeric_value(target, "setTo target")?;
            ensure_mutation_shape(expected_shape, &target)?;
            let velocity = velocity
                .as_ref()
                .map(|value| finite_numeric_value(value, "setTo velocity"))
                .transpose()?;
            if velocity
                .as_ref()
                .is_some_and(|velocity| !target.has_same_shape(velocity))
            {
                anyhow::bail!("setTo velocity shape changed");
            }
            instructions.push(MutationInstruction {
                path: path.to_string(),
                kind: MutationInstructionKind::SetTo { target, velocity },
            });
        }
        MutationPlan::To { target, timing } => {
            let target = finite_numeric_value(target, "to target")?;
            ensure_mutation_shape(expected_shape, &target)?;
            let (delay, kind) = match timing {
                MutationTiming::Linear { duration, delay } => (
                    *delay,
                    MutationInstructionKind::Linear {
                        target,
                        duration: *duration,
                    },
                ),
                MutationTiming::Spring {
                    duration,
                    bounce,
                    delay,
                } => (
                    *delay,
                    MutationInstructionKind::Spring {
                        target,
                        duration: *duration,
                        bounce: *bounce,
                    },
                ),
            };
            if delay > 0.0 {
                instructions.push(MutationInstruction {
                    path: format!("{path}/timing-delay"),
                    kind: MutationInstructionKind::Wait { duration: delay },
                });
            }
            instructions.push(MutationInstruction {
                path: path.to_string(),
                kind,
            });
        }
        MutationPlan::Sequence(children) => {
            for (index, child) in children.iter().enumerate() {
                compile_mutation_plan(
                    child,
                    &format!("{path}/sequence[{index}]"),
                    expected_shape,
                    instructions,
                )?;
            }
        }
        MutationPlan::Repeat { child, count } => {
            let start = instructions.len();
            instructions.push(MutationInstruction {
                path: format!("{path}/repeat"),
                kind: MutationInstructionKind::RepeatStart { count: *count },
            });
            compile_mutation_plan(
                child,
                &format!("{path}/repeat-child"),
                expected_shape,
                instructions,
            )?;
            instructions.push(MutationInstruction {
                path: format!("{path}/repeat-end"),
                kind: MutationInstructionKind::RepeatEnd { start },
            });
        }
        MutationPlan::Delay { child, delay } => {
            if *delay > 0.0 {
                instructions.push(MutationInstruction {
                    path: format!("{path}/delay"),
                    kind: MutationInstructionKind::Wait { duration: *delay },
                });
            }
            compile_mutation_plan(
                child,
                &format!("{path}/delayed-child"),
                expected_shape,
                instructions,
            )?;
        }
    }
    Ok(())
}

fn ensure_mutation_shape(expected: &NumericValue, actual: &NumericValue) -> anyhow::Result<()> {
    if !expected.has_same_shape(actual) {
        anyhow::bail!("Mutation plan target shape changed");
    }
    Ok(())
}

fn advance_spring_exact(driver: &mut SpringDriver, dt: f64) -> f64 {
    let before = driver.clone();
    driver.step(dt);
    if !driver.completed {
        return dt;
    }
    let mut low = 0.0;
    let mut high = dt;
    for _ in 0..24 {
        let middle = (low + high) * 0.5;
        let mut candidate = before.clone();
        candidate.step(middle);
        if candidate.completed {
            high = middle;
        } else {
            low = middle;
        }
    }
    let mut completed = before;
    completed.step(high);
    if completed.completed {
        *driver = completed;
        high
    } else {
        dt
    }
}

#[derive(Debug, Clone)]
enum MutationTarget {
    Direct(Driver),
    Plan(MutationPlanExecutor),
}

impl MutationTarget {
    fn sample(&self) -> DriverSample {
        match self {
            Self::Direct(driver) => driver.sample(),
            Self::Plan(plan) => plan.sample(),
        }
    }

    fn debug(&self) -> MutationPlanDebug {
        match self {
            Self::Direct(_) => MutationPlanDebug::default(),
            Self::Plan(plan) => plan.debug(),
        }
    }
}

#[derive(Debug, Clone)]
enum TransitionDriver {
    Legacy {
        timing_node_id: Option<String>,
        driver: Driver,
    },
    Plan(PlanExecutor),
}

#[derive(Debug, Clone)]
struct Channel {
    target: MutationTarget,
    transition: TransitionDriver,
}

impl Channel {
    fn hold(value: serde_json::Value) -> Self {
        let target = hold_driver(value);
        let zero = zero_driver_like(&target.sample().value);
        Self {
            target: MutationTarget::Direct(target),
            transition: TransitionDriver::Legacy {
                timing_node_id: None,
                driver: zero,
            },
        }
    }

    fn set_static_target(&mut self, value: serde_json::Value) {
        self.target = MutationTarget::Direct(hold_driver(value));
        if !self.transition_active() {
            self.transition = TransitionDriver::Legacy {
                timing_node_id: None,
                driver: zero_driver_like(&self.target.sample().value),
            };
        }
    }

    fn start_error(&mut self, current: DriverSample, plan: impl Into<PlanTemplate>) {
        let plan = plan.into();
        let target = self.target.sample();
        if let Some(segment) = plan.direct_state_out() {
            let driver = match (&current.value, &current.velocity) {
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
                        segment.motion.clone(),
                    )
                    .with_initial_velocity(error_velocity)
                }
                _ => zero_driver_like(&target.value),
            };
            self.transition = TransitionDriver::Legacy {
                timing_node_id: Some(segment.timing_node_id.clone()),
                driver,
            };
        } else {
            self.transition = TransitionDriver::Plan(PlanExecutor::new(plan, current, &target));
        }
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
        self.target = MutationTarget::Direct(Driver::Hold(NumericSample {
            value: target,
            velocity,
        }));
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
            MutationTarget::Direct(Driver::Spring(spring))
                if spring.value.has_same_shape(&target)
                    && spring.duration == f64::from(duration as f32)
                    && spring.bounce == f64::from(bounce as f32) =>
            {
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
                self.target = MutationTarget::Direct(Driver::Spring(spring));
            }
        }
        Ok(())
    }

    fn apply_mutation_plan(&mut self, plan: MutationPlan, dt: f64) -> anyhow::Result<()> {
        match plan {
            MutationPlan::SetTo { target, velocity } => self.set_to(target, velocity, dt),
            MutationPlan::To {
                target,
                timing:
                    MutationTiming::Spring {
                        duration,
                        bounce,
                        delay,
                    },
            } if delay == 0.0 => self.to(target, duration, bounce, dt),
            plan => {
                if let MutationTarget::Plan(active) = &mut self.target
                    && active.matches(&plan)
                {
                    return active.advance(dt);
                }
                let initial = self.target.sample();
                let mut executor = MutationPlanExecutor::new(plan, initial)?;
                executor.advance(dt)?;
                self.target = MutationTarget::Plan(executor);
                Ok(())
            }
        }
    }

    fn step(&mut self, dt: f64) {
        let target = self.target.sample();
        match &mut self.transition {
            TransitionDriver::Legacy { driver, .. } => driver.step(dt),
            TransitionDriver::Plan(plan) => plan.step(dt, &target),
        }
    }

    fn target_sample(&self) -> DriverSample {
        self.target.sample()
    }

    fn error_sample(&self) -> DriverSample {
        let target = self.target.sample();
        match &self.transition {
            TransitionDriver::Legacy { driver, .. } => driver.sample(),
            TransitionDriver::Plan(plan) => plan.error_sample(&target),
        }
    }

    fn sample(&self) -> DriverSample {
        let target = self.target.sample();
        match &self.transition {
            TransitionDriver::Legacy { driver, .. } => {
                let error = driver.sample();
                DriverSample {
                    value: subtract_values(&target.value, &error.value),
                    velocity: subtract_values(&target.velocity, &error.velocity),
                    driver: error.driver,
                    completed: error.completed,
                    persistent: false,
                    timeline_progress: error.timeline_progress,
                }
            }
            TransitionDriver::Plan(plan) => plan.physical_sample(&target),
        }
    }

    fn transition_active(&self) -> bool {
        !self.error_sample().completed
    }

    fn finish_transition(&mut self) {
        self.transition = TransitionDriver::Legacy {
            timing_node_id: None,
            driver: zero_driver_like(&self.target.sample().value),
        };
    }

    fn current_timing_node_id(&self) -> Option<String> {
        match &self.transition {
            TransitionDriver::Legacy {
                timing_node_id,
                driver,
            } if !driver.sample().completed => timing_node_id.clone(),
            TransitionDriver::Legacy { .. } => None,
            TransitionDriver::Plan(plan) => plan.current_timing_node_id(),
        }
    }

    fn pending_timing_node_ids(&self) -> Vec<String> {
        match &self.transition {
            TransitionDriver::Legacy { .. } => vec![],
            TransitionDriver::Plan(plan) => plan.pending_timing_node_ids(),
        }
    }

    fn canceled_timing_node_ids(&self) -> Vec<String> {
        match &self.transition {
            TransitionDriver::Legacy { .. } => vec![],
            TransitionDriver::Plan(plan) => plan.canceled_timing_node_ids.clone(),
        }
    }

    fn mutation_debug(&self) -> MutationPlanDebug {
        self.target.debug()
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

fn finite_numeric_value(value: &serde_json::Value, label: &str) -> anyhow::Result<NumericValue> {
    let value =
        NumericValue::from_json(value).ok_or_else(|| anyhow::anyhow!("{label} must be numeric"))?;
    if value
        .components()
        .iter()
        .any(|component| !component.is_finite())
    {
        anyhow::bail!("{label} must contain only finite numbers");
    }
    Ok(value)
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
            } => {
                let timeline =
                    Driver::Timeline(TimelineDriver::new(source, target, duration, curve));
                if delay > 0.0 {
                    Driver::Delayed(DelayedDriver::new(outgoing, timeline, delay, false))
                } else {
                    timeline
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
            Self::Delayed(driver) => driver.step(dt),
            Self::Discrete(driver) => driver.step(dt),
        }
    }

    fn sample(&self) -> DriverSample {
        match self {
            Self::Hold(sample) => DriverSample::numeric(sample.clone(), "hold", true, None),
            Self::Spring(driver) => driver.sample(),
            Self::Timeline(driver) => driver.sample(),
            Self::Delayed(driver) => driver.sample(),
            Self::Discrete(driver) => driver.sample(),
        }
    }

    fn remaining_duration(&self) -> Option<f64> {
        match self {
            Self::Hold(_) => Some(0.0),
            Self::Spring(_) => None,
            Self::Timeline(driver) => Some((driver.duration - driver.elapsed).max(0.0)),
            Self::Delayed(driver) => {
                let delay = (driver.delay - driver.elapsed).max(0.0);
                driver
                    .incoming
                    .remaining_duration()
                    .map(|incoming| delay + incoming)
            }
            Self::Discrete(driver) => driver.timing.remaining_duration(),
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
}

impl DriverSample {
    fn numeric(
        sample: NumericSample,
        driver: &'static str,
        completed: bool,
        timeline_progress: Option<f64>,
    ) -> Self {
        Self {
            value: sample.value,
            velocity: sample.velocity,
            driver,
            completed,
            persistent: false,
            timeline_progress,
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
        )
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
        // condition and Timelines wait for their authored duration.
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
            },
        );

        channel.step(0.5);

        assert_eq!(
            channel.sample().value.to_json(),
            serde_json::json!([[1.0, 3.0], [5.0, 7.0]])
        );
    }

    #[test]
    fn hdr_color_mutation_and_interrupted_transition_preserve_components() {
        let key = StateParamKey::new("Light:tint");
        let mut engine = MotionEngine::with_initial_values(HashMap::from([(
            key.clone(),
            serde_json::json!([1.25, 0.5, 0.1, 1.0]),
        )]));
        engine.begin_mutation_frame();
        engine
            .set_to(&key, serde_json::json!([4.0, 1.5, 0.25, 0.6]), None, 0.0)
            .unwrap();
        assert_eq!(
            engine.physical_value(&key),
            Some(serde_json::json!([4.0, 1.5, 0.25, 0.6]))
        );

        for _ in 0..60 {
            engine.begin_mutation_frame();
            engine
                .to(
                    &key,
                    serde_json::json!([8.0, 3.0, 0.5, 0.8]),
                    0.4,
                    0.1,
                    1.0 / 60.0,
                )
                .unwrap();
        }
        let mutated = engine.physical_value(&key).expect("mutated HDR color");
        assert!(mutated[0].as_f64().unwrap() > 1.0, "{mutated}");
        assert!(mutated[1].as_f64().unwrap() > 1.0, "{mutated}");

        let mut channel = Channel::hold(serde_json::json!([4.0, 1.5, 0.25, 0.6]));
        let outgoing = Channel::hold(serde_json::json!([1.25, 0.5, 0.1, 1.0])).sample();
        channel.start_error(
            outgoing,
            MotionPlan::Timeline {
                duration: 1.0,
                delay: 0.0,
                curve: TimelinePreset::Linear,
            },
        );
        channel.step(0.35);
        let before_interrupt = channel.sample();
        channel.set_static_target(serde_json::json!([6.0, 2.0, 0.4, 0.7]));
        channel.start_error(
            before_interrupt.clone(),
            MotionPlan::Spring {
                duration: 0.4,
                bounce: 0.1,
                delay: 0.0,
            },
        );
        let interrupted = channel.sample();
        assert!(
            interrupted
                .value
                .components()
                .iter()
                .zip(before_interrupt.value.components())
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-12)
        );
        channel.step(1.0);
        let resumed = channel.sample().value.to_json();
        assert!(resumed[0].as_f64().unwrap() > 1.0, "{resumed}");
        assert!(
            resumed
                .as_array()
                .unwrap()
                .iter()
                .all(|value| value.as_f64().unwrap().is_finite())
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
        assert_eq!(drivers.get("Node:y").map(String::as_str), Some("timeline"));
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

    fn timeline_node(id: &str, duration: f64, delay: f64) -> TransitionMotionNode {
        TransitionMotionNode::Linear {
            timeline: super::super::types::TimelineMotionNode {
                id: id.into(),
                position: Default::default(),
                label: None,
                duration,
                delay,
            },
        }
    }

    fn endpoint(node_id: &str, port_id: &str) -> super::super::types::GraphEndpoint {
        super::super::types::GraphEndpoint {
            node_id: node_id.into(),
            port_id: port_id.into(),
        }
    }

    fn serial_waypoint_graph() -> TransitionMotionGraph {
        TransitionMotionGraph {
            id: "serial".into(),
            name: "Serial waypoint".into(),
            inputs: vec![],
            outputs: vec![],
            nodes: vec![
                timeline_node("to_waypoint", 0.1, 0.0),
                TransitionMotionNode::Waypoint {
                    id: "peak".into(),
                    position: Default::default(),
                    label: None,
                    port_type: "float".into(),
                    value: serde_json::json!(64.0),
                    array_length: None,
                },
                timeline_node("to_target", 0.1, 0.0),
            ],
            connections: vec![
                super::super::types::GraphConnection {
                    id: "to_peak".into(),
                    from: endpoint("to_waypoint", "value"),
                    to: endpoint("peak", "in"),
                },
                super::super::types::GraphConnection {
                    id: "from_peak".into(),
                    from: endpoint("peak", "value"),
                    to: endpoint("to_target", "value"),
                },
            ],
            input_bindings: vec![super::super::types::TransitionMotionInputBinding {
                source: StateValueSource::StateParam {
                    state_param_id: "Blur:value".into(),
                },
                to: endpoint("to_waypoint", "value"),
            }],
            output_bindings: vec![super::super::types::TransitionMotionOutputBinding {
                state_param_id: "Blur:value".into(),
                from: endpoint("to_target", "value"),
            }],
            passthrough_bindings: vec![],
            condition_binding: None,
            layout: None,
            viewport: None,
        }
    }

    #[test]
    fn serial_waypoint_reaches_absolute_value_then_final_target() {
        let graph = serial_waypoint_graph();
        let key = StateParamKey::new("Blur:value");
        let source = HashMap::from([(key.clone(), serde_json::json!(0.0))]);
        let target = HashMap::from([(key.clone(), serde_json::json!(0.0))]);
        let mut engine = MotionEngine::new();
        engine.start_transition("transition", &graph, &source, &target, &HashMap::new());

        let peak = engine.step(0.1);
        let peak_channel = peak
            .channels
            .iter()
            .find(|channel| channel.key == "Blur:value")
            .unwrap();
        assert!(
            (peak_channel.value[0] - 64.0).abs() <= 1.0e-6,
            "{peak_channel:?}"
        );
        assert_eq!(
            peak_channel.current_timing_node_id.as_deref(),
            Some("to_target")
        );

        let final_step = engine.step(0.1);
        let final_channel = final_step
            .channels
            .iter()
            .find(|channel| channel.key == "Blur:value")
            .unwrap();
        assert!(final_channel.value[0].abs() <= 1.0e-6, "{final_channel:?}");
        assert!(!final_step.active);
    }

    #[test]
    fn waypoint_is_absolute_while_mutation_target_moves() {
        let graph = serial_waypoint_graph();
        let plan = compile_channel_plans(&graph)
            .specific
            .get("Blur:value")
            .cloned()
            .expect("compiled serial plan");
        let previous = Channel::hold(serde_json::json!(0.0)).sample();
        let mut channel = Channel::hold(serde_json::json!(0.0));
        channel.start_error(previous, plan);
        channel.set_static_target(serde_json::json!(100.0));
        channel.step(0.1);
        assert!((channel.sample().value.components()[0] - 64.0).abs() <= 1.0e-6);
        assert!((channel.error_sample().value.components()[0] - 36.0).abs() <= 1.0e-6);
    }

    fn parallel_takeover_graph(second_delay: f64) -> TransitionMotionGraph {
        TransitionMotionGraph {
            id: "parallel".into(),
            name: "Parallel takeover".into(),
            inputs: vec![],
            outputs: vec![],
            nodes: vec![
                timeline_node("early", 1.0, 0.0),
                timeline_node("late", 0.1, second_delay),
            ],
            connections: vec![],
            input_bindings: vec![
                super::super::types::TransitionMotionInputBinding {
                    source: StateValueSource::StateParam {
                        state_param_id: "Value:x".into(),
                    },
                    to: endpoint("early", "value"),
                },
                super::super::types::TransitionMotionInputBinding {
                    source: StateValueSource::StateParam {
                        state_param_id: "Value:x".into(),
                    },
                    to: endpoint("late", "value"),
                },
            ],
            output_bindings: vec![
                super::super::types::TransitionMotionOutputBinding {
                    state_param_id: "Value:x".into(),
                    from: endpoint("early", "value"),
                },
                super::super::types::TransitionMotionOutputBinding {
                    state_param_id: "Value:x".into(),
                    from: endpoint("late", "value"),
                },
            ],
            passthrough_bindings: vec![],
            condition_binding: None,
            layout: None,
            viewport: None,
        }
    }

    #[test]
    fn delayed_parallel_branch_takes_over_and_cancels_earlier_owner() {
        let graph = parallel_takeover_graph(0.2);
        let key = StateParamKey::new("Value:x");
        let source = HashMap::from([(key.clone(), serde_json::json!(0.0))]);
        let target = HashMap::from([(key.clone(), serde_json::json!(10.0))]);
        let mut engine = MotionEngine::new();
        engine.start_transition("transition", &graph, &source, &target, &HashMap::new());

        let before = engine.step(0.1);
        let before_channel = before
            .channels
            .iter()
            .find(|channel| channel.key == "Value:x")
            .unwrap();
        assert_eq!(
            before_channel.current_timing_node_id.as_deref(),
            Some("early")
        );
        assert_eq!(before_channel.pending_timing_node_ids, vec!["late"]);

        let takeover = engine.step(0.1);
        let takeover_channel = takeover
            .channels
            .iter()
            .find(|channel| channel.key == "Value:x")
            .unwrap();
        assert_eq!(
            takeover_channel.current_timing_node_id.as_deref(),
            Some("late")
        );
        assert_eq!(takeover_channel.canceled_timing_node_ids, vec!["early"]);
        assert!(
            (takeover_channel.value[0] - 2.0).abs() <= 1.0e-6,
            "{takeover_channel:?}"
        );

        let completed = engine.step(0.1);
        let completed_channel = completed
            .channels
            .iter()
            .find(|channel| channel.key == "Value:x")
            .unwrap();
        assert!((completed_channel.value[0] - 10.0).abs() <= 1.0e-6);
        assert!(!completed.active);
    }

    #[test]
    fn same_start_parallel_order_uses_later_persisted_binding() {
        let graph = parallel_takeover_graph(0.0);
        let plan = compile_channel_plans(&graph)
            .specific
            .get("Value:x")
            .cloned()
            .expect("compiled parallel plan");
        let previous = Channel::hold(serde_json::json!(0.0)).sample();
        let mut channel = Channel::hold(serde_json::json!(10.0));
        channel.start_error(previous, plan);
        assert_eq!(channel.current_timing_node_id().as_deref(), Some("late"));
        assert_eq!(channel.canceled_timing_node_ids(), vec!["early"]);
    }

    fn timeline_segment(id: &str, duration: f64, delay: f64, target: PlanTarget) -> PlanTemplate {
        PlanTemplate::Segment(SegmentTemplate {
            timing_node_id: id.into(),
            motion: MotionPlan::Timeline {
                duration,
                delay,
                curve: TimelinePreset::Linear,
            },
            target,
        })
    }

    fn spring_segment(id: &str, target: PlanTarget) -> PlanTemplate {
        PlanTemplate::Segment(SegmentTemplate {
            timing_node_id: id.into(),
            motion: MotionPlan::Spring {
                duration: 0.4,
                bounce: 0.0,
                delay: 0.0,
            },
            target,
        })
    }

    fn instant_segment(id: &str, target: PlanTarget) -> PlanTemplate {
        PlanTemplate::Segment(SegmentTemplate {
            timing_node_id: id.into(),
            motion: MotionPlan::Instant,
            target,
        })
    }

    #[test]
    fn mixed_instant_timeline_and_spring_sequence_hands_off_in_order() {
        let plan = PlanTemplate::Sequence(vec![
            instant_segment(
                "instant_peak",
                PlanTarget::Waypoint(NumericValue::from_json(&serde_json::json!(64.0)).unwrap()),
            ),
            timeline_segment(
                "timeline_mid",
                0.1,
                0.0,
                PlanTarget::Waypoint(NumericValue::from_json(&serde_json::json!(32.0)).unwrap()),
            ),
            spring_segment("spring_target", PlanTarget::StateOut),
        ]);
        let previous = Channel::hold(serde_json::json!(0.0)).sample();
        let mut channel = Channel::hold(serde_json::json!(0.0));
        channel.start_error(previous, plan);

        assert_eq!(
            channel.current_timing_node_id().as_deref(),
            Some("timeline_mid")
        );
        assert!((channel.sample().value.components()[0] - 64.0).abs() <= 1.0e-9);

        channel.step(0.1);
        assert_eq!(
            channel.current_timing_node_id().as_deref(),
            Some("spring_target")
        );
        assert!((channel.sample().value.components()[0] - 32.0).abs() <= 1.0e-9);

        for _ in 0..240 {
            if !channel.transition_active() {
                break;
            }
            channel.step(1.0 / 60.0);
        }
        assert!(!channel.transition_active());
        assert!(channel.sample().value.components()[0].abs() <= 1.0e-6);
    }

    #[test]
    fn large_dt_crosses_parallel_takeover_and_continues_outer_sequence() {
        let waypoint = NumericValue::from_json(&serde_json::json!(64.0)).unwrap();
        let plan = PlanTemplate::Sequence(vec![
            PlanTemplate::Parallel(vec![
                timeline_segment("early", 1.0, 0.0, PlanTarget::Waypoint(waypoint.clone())),
                timeline_segment("late", 0.1, 0.1, PlanTarget::Waypoint(waypoint)),
            ]),
            timeline_segment("final", 0.1, 0.0, PlanTarget::StateOut),
        ]);
        let previous = Channel::hold(serde_json::json!(0.0)).sample();
        let mut channel = Channel::hold(serde_json::json!(0.0));
        channel.start_error(previous, plan);

        channel.step(0.25);
        let midway = channel.sample();
        assert!(
            (midway.value.components()[0] - 32.0).abs() <= 1.0e-6,
            "{midway:?}"
        );
        assert_eq!(channel.current_timing_node_id().as_deref(), Some("final"));
        assert_eq!(channel.canceled_timing_node_ids(), vec!["early"]);

        channel.step(0.06);
        assert!(channel.sample().value.components()[0].abs() <= 1.0e-6);
        assert!(!channel.transition_active());
    }

    #[test]
    fn zero_distance_final_spring_completes_in_the_waypoint_frame() {
        let mut graph = serial_waypoint_graph();
        graph.nodes[2] = TransitionMotionNode::Spring {
            id: "to_target".into(),
            position: Default::default(),
            label: None,
            duration: 0.1,
            bounce: 0.0,
            delay: 0.0,
        };
        let key = StateParamKey::new("Blur:value");
        let source = HashMap::from([(key.clone(), serde_json::json!(0.0))]);
        let target = HashMap::from([(key.clone(), serde_json::json!(64.0))]);
        let mut engine = MotionEngine::new();
        engine.start_transition("transition", &graph, &source, &target, &HashMap::new());

        let step = engine.step(0.11);
        let channel = step
            .channels
            .iter()
            .find(|channel| channel.key == "Blur:value")
            .unwrap();
        assert!((channel.value[0] - 64.0).abs() <= 1.0e-6);
        assert!(!step.active);
    }

    #[test]
    fn replacing_a_plan_snapshots_physical_value_and_velocity_and_drops_old_nodes() {
        let old_plan = PlanTemplate::Sequence(vec![
            spring_segment(
                "old_first",
                PlanTarget::Waypoint(NumericValue::from_json(&serde_json::json!(64.0)).unwrap()),
            ),
            spring_segment("old_second", PlanTarget::StateOut),
        ]);
        let new_plan = PlanTemplate::Sequence(vec![
            spring_segment(
                "new_first",
                PlanTarget::Waypoint(NumericValue::from_json(&serde_json::json!(32.0)).unwrap()),
            ),
            spring_segment("new_second", PlanTarget::StateOut),
        ]);
        let previous = Channel::hold(serde_json::json!(0.0)).sample();
        let mut channel = Channel::hold(serde_json::json!(0.0));
        channel.start_error(previous, old_plan);
        channel.step(0.05);
        let outgoing = channel.sample();
        assert!(outgoing.velocity.components()[0].abs() > 1.0e-6);

        channel.set_static_target(serde_json::json!(10.0));
        channel.start_error(outgoing.clone(), new_plan);
        let incoming = channel.sample();
        assert!((incoming.value.components()[0] - outgoing.value.components()[0]).abs() <= 1.0e-9);
        assert!(
            (incoming.velocity.components()[0] - outgoing.velocity.components()[0]).abs() <= 1.0e-9
        );
        assert_eq!(
            channel.current_timing_node_id().as_deref(),
            Some("new_first")
        );
        assert!(
            channel
                .pending_timing_node_ids()
                .iter()
                .all(|id| !id.starts_with("old_"))
        );
        assert!(channel.canceled_timing_node_ids().is_empty());
    }

    #[test]
    fn replacing_a_plan_is_continuous_during_delay_both_segments_and_waypoint_handoff() {
        for elapsed in [0.1, 0.3, 0.4, 0.55] {
            let old_plan = PlanTemplate::Sequence(vec![
                timeline_segment(
                    "old_first",
                    0.2,
                    0.2,
                    PlanTarget::Waypoint(
                        NumericValue::from_json(&serde_json::json!(64.0)).unwrap(),
                    ),
                ),
                timeline_segment("old_second", 0.4, 0.0, PlanTarget::StateOut),
            ]);
            let new_plan = PlanTemplate::Sequence(vec![
                spring_segment(
                    "new_first",
                    PlanTarget::Waypoint(
                        NumericValue::from_json(&serde_json::json!(32.0)).unwrap(),
                    ),
                ),
                spring_segment("new_second", PlanTarget::StateOut),
            ]);
            let previous = Channel::hold(serde_json::json!(0.0)).sample();
            let mut channel = Channel::hold(serde_json::json!(0.0));
            channel.start_error(previous, old_plan);
            channel.step(elapsed);
            let outgoing = channel.sample();

            channel.set_static_target(serde_json::json!(10.0));
            channel.start_error(outgoing.clone(), new_plan);
            let incoming = channel.sample();
            assert!(
                (incoming.value.components()[0] - outgoing.value.components()[0]).abs() <= 1.0e-9,
                "value discontinuity at t={elapsed}: {outgoing:?} -> {incoming:?}"
            );
            assert!(
                (incoming.velocity.components()[0] - outgoing.velocity.components()[0]).abs()
                    <= 1.0e-9,
                "velocity discontinuity at t={elapsed}: {outgoing:?} -> {incoming:?}"
            );
            assert_eq!(
                channel.current_timing_node_id().as_deref(),
                Some("new_first")
            );
            assert!(
                channel
                    .pending_timing_node_ids()
                    .iter()
                    .all(|id| !id.starts_with("old_"))
            );
        }
    }

    fn set_plan(value: f64) -> MutationPlan {
        MutationPlan::SetTo {
            target: serde_json::json!(value),
            velocity: None,
        }
    }

    fn linear_plan(value: f64, duration: f64, delay: f64) -> MutationPlan {
        MutationPlan::To {
            target: serde_json::json!(value),
            timing: MutationTiming::Linear { duration, delay },
        }
    }

    fn phase_cycle_plan(start_delay: f64) -> MutationPlan {
        let cycle = MutationPlan::Repeat {
            child: Box::new(MutationPlan::Sequence(vec![
                set_plan(0.0),
                linear_plan(1.0, 2.5, 0.0),
            ])),
            count: -1,
        };
        MutationPlan::Sequence(vec![
            set_plan(0.0),
            MutationPlan::Delay {
                child: Box::new(cycle),
                delay: start_delay,
            },
        ])
    }

    #[test]
    fn mutation_sequence_holds_then_repeats_with_exact_leftover_time() {
        let initial = hold_driver(serde_json::json!(0.0)).sample();
        let mut plan = MutationPlanExecutor::new(phase_cycle_plan(1.0), initial).unwrap();

        plan.advance(0.75).unwrap();
        assert_eq!(plan.sample().value.components(), &[0.0]);
        assert_eq!(plan.debug().delay_remaining, Some(0.25));

        plan.advance(0.25).unwrap();
        assert_eq!(plan.sample().value.components(), &[0.0]);
        assert!(matches!(
            plan.active,
            Some(ActiveMutationSegment::Linear { .. })
        ));

        plan.advance(0.625).unwrap();
        assert!((plan.sample().value.components()[0] - 0.25).abs() <= 1.0e-6);

        // The large step finishes the current cycle, executes the zero-time
        // reset, and consumes the exact remainder in the next iteration.
        plan.advance(2.5).unwrap();
        assert!((plan.sample().value.components()[0] - 0.25).abs() <= 1.0e-6);
        assert_eq!(plan.debug().repeat_iteration, Some(2));
        assert_eq!(plan.debug().repeat_count, Some(-1));
    }

    #[test]
    fn finite_repeat_count_is_total_plays_and_completes() {
        let plan = MutationPlan::Sequence(vec![
            set_plan(0.0),
            MutationPlan::Repeat {
                child: Box::new(MutationPlan::Sequence(vec![
                    linear_plan(1.0, 0.2, 0.2),
                    linear_plan(0.0, 0.2, 0.0),
                ])),
                count: 3,
            },
        ]);
        let initial = hold_driver(serde_json::json!(0.0)).sample();
        let mut executor = MutationPlanExecutor::new(plan, initial).unwrap();
        executor.advance(1.8).unwrap();
        let sample = executor.sample();
        assert!(sample.completed, "{sample:?}");
        assert!(sample.value.components()[0].abs() <= 1.0e-6);
    }

    #[test]
    fn delay_wrapper_order_distinguishes_once_from_every_repeat() {
        let cycle = MutationPlan::Sequence(vec![set_plan(0.0), linear_plan(1.0, 1.0, 0.0)]);
        let delay_once = MutationPlan::Delay {
            child: Box::new(MutationPlan::Repeat {
                child: Box::new(cycle.clone()),
                count: 2,
            }),
            delay: 0.5,
        };
        let delay_each_time = MutationPlan::Repeat {
            child: Box::new(MutationPlan::Delay {
                child: Box::new(cycle),
                delay: 0.5,
            }),
            count: 2,
        };
        let initial = hold_driver(serde_json::json!(0.0)).sample();
        let mut once = MutationPlanExecutor::new(delay_once, initial.clone()).unwrap();
        let mut each = MutationPlanExecutor::new(delay_each_time, initial).unwrap();

        once.advance(1.75).unwrap();
        each.advance(1.75).unwrap();

        assert!((once.sample().value.components()[0] - 0.25).abs() <= 1.0e-6);
        assert!((each.sample().value.components()[0] - 1.0).abs() <= 1.0e-6);
        assert!((each.debug().delay_remaining.unwrap() - 0.25).abs() <= 1.0e-6);
        assert_eq!(each.debug().repeat_iteration, Some(2));
    }

    #[test]
    fn segment_delay_and_coarse_frames_consume_all_exact_boundaries() {
        let initial = hold_driver(serde_json::json!(0.0)).sample();
        let mut delayed_segment =
            MutationPlanExecutor::new(linear_plan(1.0, 0.5, 0.25), initial.clone()).unwrap();
        delayed_segment.advance(0.375).unwrap();
        assert!((delayed_segment.sample().value.components()[0] - 0.25).abs() <= 1.0e-6);
        assert_eq!(delayed_segment.debug().path.as_deref(), Some("root"));

        let mut repeated = MutationPlanExecutor::new(phase_cycle_plan(1.0), initial).unwrap();
        repeated.advance(9.75).unwrap();
        assert!((repeated.sample().value.components()[0] - 0.5).abs() <= 1.0e-6);
        assert_eq!(repeated.debug().repeat_iteration, Some(4));
    }

    #[test]
    fn spring_segment_inherits_the_current_plan_velocity() {
        let plan = MutationPlan::Sequence(vec![
            MutationPlan::SetTo {
                target: serde_json::json!(0.0),
                velocity: Some(serde_json::json!(2.0)),
            },
            MutationPlan::To {
                target: serde_json::json!(1.0),
                timing: MutationTiming::Spring {
                    duration: 0.8,
                    bounce: 0.1,
                    delay: 0.0,
                },
            },
        ]);
        let initial = hold_driver(serde_json::json!(0.0)).sample();
        let executor = MutationPlanExecutor::new(plan, initial).unwrap();

        assert_eq!(executor.sample().driver, "mutation-spring");
        assert!((executor.sample().velocity.components()[0] - 2.0).abs() <= 1.0e-6);
    }

    #[test]
    fn same_mutation_plan_descriptor_continues_instead_of_restarting() {
        let key = StateParamKey::new("phase");
        let mut engine = MotionEngine::with_initial_values(HashMap::from([(
            key.clone(),
            serde_json::json!(0.0),
        )]));
        let plan = phase_cycle_plan(1.0);

        engine.begin_mutation_frame();
        engine.apply_mutation_plan(&key, plan.clone(), 0.0).unwrap();
        engine.begin_mutation_frame();
        engine.apply_mutation_plan(&key, plan.clone(), 1.5).unwrap();
        let first = engine.target_value(&key).unwrap().as_f64().unwrap();
        assert!((first - 0.2).abs() <= 1.0e-6, "{first}");

        engine.begin_mutation_frame();
        engine.apply_mutation_plan(&key, plan, 0.5).unwrap();
        let second = engine.target_value(&key).unwrap().as_f64().unwrap();
        assert!((second - 0.4).abs() <= 1.0e-6, "{second}");
    }

    #[test]
    fn changed_descriptor_and_state_reentry_restart_the_plan() {
        let key = StateParamKey::new("phase");
        let mut engine = MotionEngine::with_initial_values(HashMap::from([(
            key.clone(),
            serde_json::json!(0.0),
        )]));

        engine.begin_mutation_frame();
        engine
            .apply_mutation_plan(&key, phase_cycle_plan(1.0), 1.5)
            .unwrap();
        assert!((engine.target_value(&key).unwrap().as_f64().unwrap() - 0.2).abs() <= 1.0e-6);

        engine.begin_mutation_frame();
        engine
            .apply_mutation_plan(&key, phase_cycle_plan(0.5), 0.0)
            .unwrap();
        assert_eq!(engine.target_value(&key).unwrap(), serde_json::json!(0.0));
        assert_eq!(
            engine
                .channels
                .get(&key)
                .unwrap()
                .mutation_debug()
                .delay_remaining,
            Some(0.5)
        );

        engine.begin_mutation_frame();
        engine
            .apply_mutation_plan(&key, phase_cycle_plan(0.5), 0.75)
            .unwrap();
        assert!((engine.target_value(&key).unwrap().as_f64().unwrap() - 0.1).abs() <= 1.0e-6);

        engine.seed_targets_from_state([&key]);
        engine.begin_mutation_frame();
        engine
            .apply_mutation_plan(&key, phase_cycle_plan(0.5), 0.0)
            .unwrap();
        assert_eq!(engine.target_value(&key).unwrap(), serde_json::json!(0.0));
        assert_eq!(
            engine
                .channels
                .get(&key)
                .unwrap()
                .mutation_debug()
                .delay_remaining,
            Some(0.5)
        );
    }

    #[test]
    fn infinite_mutation_repeat_does_not_block_transition_completion() {
        let key = StateParamKey::new("phase");
        let mut engine = MotionEngine::with_initial_values(HashMap::from([(
            key.clone(),
            serde_json::json!(0.0),
        )]));
        engine.begin_mutation_frame();
        engine
            .apply_mutation_plan(&key, phase_cycle_plan(1.0), 0.0)
            .unwrap();
        let previous = engine.clone();
        engine.begin_transition_from(
            "visual-transition",
            &wildcard_spring_graph(0.4, 0.0),
            &previous,
            &HashSet::new(),
        );

        let step = engine.step(0.0);
        assert!(!step.active);
        assert_eq!(engine.active_transition_id(), None);
        assert_eq!(
            step.channels[0].mutation_plan_completed,
            Some(false),
            "infinite target plan must remain active independently"
        );
    }

    #[test]
    fn mutation_plan_rejects_invalid_repeat_and_timing_values() {
        assert!(
            MutationPlan::Repeat {
                child: Box::new(set_plan(0.0)),
                count: -1,
            }
            .validate()
            .unwrap_err()
            .to_string()
            .contains("consumes time")
        );
        assert!(
            linear_plan(1.0, f64::NAN, 0.0)
                .validate()
                .unwrap_err()
                .to_string()
                .contains("linear duration")
        );
        for count in [0, -2] {
            assert!(
                MutationPlan::Repeat {
                    child: Box::new(linear_plan(1.0, 1.0, 0.0)),
                    count,
                }
                .validate()
                .is_err()
            );
        }
        assert!(MutationPlan::Sequence(vec![]).validate().is_err());
        assert!(linear_plan(1.0, -0.1, 0.0).validate().is_err());
        assert!(linear_plan(1.0, 0.1, -0.1).validate().is_err());
        assert!(
            MutationPlan::Delay {
                child: Box::new(linear_plan(1.0, 0.1, 0.0)),
                delay: f64::INFINITY,
            }
            .validate()
            .is_err()
        );
        for bounce in [-1.0, 1.0] {
            assert!(
                MutationPlan::To {
                    target: serde_json::json!(1.0),
                    timing: MutationTiming::Spring {
                        duration: 0.8,
                        bounce,
                        delay: 0.0,
                    },
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            MutationPlan::To {
                target: serde_json::json!(1.0),
                timing: MutationTiming::Spring {
                    duration: 0.0,
                    bounce: 0.0,
                    delay: 0.0,
                },
            }
            .validate()
            .is_err()
        );
    }
}
