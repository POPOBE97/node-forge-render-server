//! Structural and semantic validation for `StateMachine` definitions.
//!
//! All checks are intentionally fail-fast: on the first error encountered
//! an `Err` is returned with a human-readable diagnostic that includes
//! relevant IDs (stateId / transitionId / mutationId).

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use super::types::*;

/// Validate a `StateMachine` definition.
///
/// Returns `Ok(())` when the definition is structurally sound, or an `Err`
/// with an actionable diagnostic on the first violation encountered.
pub fn validate(sm: &StateMachine) -> Result<()> {
    validate_state_ids(sm)?;
    validate_builtin_states(sm)?;
    validate_mutation_ids(sm)?;
    validate_mutation_ownership(sm)?;
    validate_transition_endpoints(sm)?;
    validate_transition_direction_constraints(sm)?;
    validate_mutation_internals(sm)?;
    validate_motion_graphs(sm)?;
    Ok(())
}

// ── State ID uniqueness ────────────────────────────────────────────────────

fn validate_state_ids(sm: &StateMachine) -> Result<()> {
    let mut seen: HashSet<&str> = HashSet::new();
    for s in &sm.states {
        if !seen.insert(s.id.as_str()) {
            bail!("state_machine validation: duplicate state id '{}'", s.id);
        }
    }
    Ok(())
}

// ── Built-in state invariants ──────────────────────────────────────────────

fn validate_builtin_states(sm: &StateMachine) -> Result<()> {
    let mut entry_count = 0u32;
    let mut any_count = 0u32;
    let mut exit_count = 0u32;

    for s in &sm.states {
        match s.resolved_type() {
            AnimationStateType::EntryState => entry_count += 1,
            AnimationStateType::AnyState => any_count += 1,
            AnimationStateType::ExitState => exit_count += 1,
            _ => {}
        }
    }

    if entry_count != 1 {
        bail!("state_machine validation: expected exactly 1 entryState, found {entry_count}");
    }
    if any_count != 1 {
        bail!("state_machine validation: expected exactly 1 anyState, found {any_count}");
    }
    if exit_count != 1 {
        bail!("state_machine validation: expected exactly 1 exitState, found {exit_count}");
    }

    Ok(())
}

// ── Mutation ID uniqueness ─────────────────────────────────────────────────

fn validate_mutation_ids(sm: &StateMachine) -> Result<()> {
    let mut seen: HashSet<&str> = HashSet::new();
    for m in &sm.mutations {
        if !seen.insert(m.id.as_str()) {
            bail!("state_machine validation: duplicate mutation id '{}'", m.id);
        }
    }
    Ok(())
}

// ── State-local MutationDefinition ownership ───────────────────────────────

fn validate_mutation_ownership(sm: &StateMachine) -> Result<()> {
    let mutation_ids: HashSet<&str> = sm.mutations.iter().map(|m| m.id.as_str()).collect();
    let mut owner_by_mutation: HashMap<&str, &str> = HashMap::new();

    for s in &sm.states {
        match s.mutation_id.as_deref() {
            None => continue,
            Some(mid) if !mutation_ids.contains(mid) => bail!(
                "state_machine validation: state '{}' references missing mutation '{mid}'",
                s.id
            ),
            Some(mid) => {
                if let Some(existing) = owner_by_mutation.insert(mid, s.id.as_str()) {
                    bail!(
                        "state_machine validation: mutation '{mid}' is shared by states '{existing}' and '{}'",
                        s.id
                    );
                }
            }
        }
    }
    for mutation in &sm.mutations {
        if !owner_by_mutation.contains_key(mutation.id.as_str()) {
            bail!(
                "state_machine validation: mutation '{}' is not owned by any state",
                mutation.id
            );
        }
    }

    Ok(())
}

// ── Transition endpoint references ─────────────────────────────────────────

fn validate_transition_endpoints(sm: &StateMachine) -> Result<()> {
    let state_ids: HashSet<&str> = sm.states.iter().map(|s| s.id.as_str()).collect();
    let motion_graph_ids: HashSet<&str> = sm.motion_graphs.iter().map(|g| g.id.as_str()).collect();
    let mut referenced_motion_graph_ids = HashSet::new();

    for t in &sm.transitions {
        if !referenced_motion_graph_ids.insert(t.motion_graph_id.as_str()) {
            bail!(
                "state_machine validation: transition '{}' reuses motion graph '{}'; each transition must own an independent motion graph",
                t.id,
                t.motion_graph_id
            );
        }
        if !state_ids.contains(t.source.as_str()) {
            bail!(
                "state_machine validation: transition '{}' source '{}' references missing state",
                t.id,
                t.source
            );
        }
        if !state_ids.contains(t.target.as_str()) {
            bail!(
                "state_machine validation: transition '{}' target '{}' references missing state",
                t.id,
                t.target
            );
        }
        if !motion_graph_ids.contains(t.motion_graph_id.as_str()) {
            bail!(
                "state_machine validation: transition '{}' references missing motion graph '{}'",
                t.id,
                t.motion_graph_id
            );
        }
    }

    Ok(())
}

fn validate_motion_graphs(sm: &StateMachine) -> Result<()> {
    let mut graph_ids = HashSet::new();
    for graph in &sm.motion_graphs {
        if !graph_ids.insert(graph.id.as_str()) {
            bail!(
                "state_machine validation: duplicate transition motion graph id '{}'",
                graph.id
            );
        }

        let node_ids: HashSet<&str> = graph.nodes.iter().map(TransitionMotionNode::id).collect();
        if node_ids.len() != graph.nodes.len() {
            bail!(
                "state_machine validation: transition motion graph '{}' has duplicate node ids",
                graph.id
            );
        }
        let input_ids: HashSet<&str> = graph.inputs.iter().map(|port| port.id.as_str()).collect();
        let output_ids: HashSet<&str> = graph.outputs.iter().map(|port| port.id.as_str()).collect();
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut incoming_count: HashMap<&str, usize> =
            node_ids.iter().map(|node_id| (*node_id, 0)).collect();

        for connection in &graph.connections {
            if !node_ids.contains(connection.from.node_id.as_str())
                || !node_ids.contains(connection.to.node_id.as_str())
            {
                bail!(
                    "state_machine validation: transition motion graph '{}' connection '{}' references a missing node",
                    graph.id,
                    connection.id
                );
            }
            adjacency
                .entry(connection.from.node_id.as_str())
                .or_default()
                .push(connection.to.node_id.as_str());
            *incoming_count
                .entry(connection.to.node_id.as_str())
                .or_default() += 1;
        }
        let mut ready: Vec<&str> = incoming_count
            .iter()
            .filter_map(|(node_id, count)| (*count == 0).then_some(*node_id))
            .collect();
        let mut visited = 0usize;
        while let Some(node_id) = ready.pop() {
            visited += 1;
            for target in adjacency.get(node_id).into_iter().flatten() {
                let count = incoming_count.get_mut(target).expect("motion node exists");
                *count -= 1;
                if *count == 0 {
                    ready.push(target);
                }
            }
        }
        if visited != node_ids.len() {
            bail!(
                "state_machine validation: transition motion graph '{}' contains a cycle",
                graph.id
            );
        }
        let node_by_id: HashMap<&str, &TransitionMotionNode> =
            graph.nodes.iter().map(|node| (node.id(), node)).collect();
        for connection in &graph.connections {
            let from = node_by_id.get(connection.from.node_id.as_str()).copied();
            let to = node_by_id.get(connection.to.node_id.as_str()).copied();
            let repeat_follow = matches!(from, Some(TransitionMotionNode::RepeatTimeline { .. }))
                && connection.from.port_id == "target"
                && matches!(to, Some(TransitionMotionNode::SpringFollow { .. }))
                && connection.to.port_id == "target";
            if !repeat_follow
                && (from.is_some_and(TransitionMotionNode::is_timing)
                    || to.is_some_and(TransitionMotionNode::is_timing))
            {
                bail!(
                    "state_machine validation: transition motion graph '{}' only allows RepeatTimeline.target -> SpringFollow.target timing chains",
                    graph.id
                );
            }
        }

        let mut input_channel_by_node: HashMap<&str, &str> = HashMap::new();
        for binding in &graph.input_bindings {
            if !input_ids.contains(binding.port_id.as_str())
                || !node_ids.contains(binding.to.node_id.as_str())
            {
                bail!(
                    "state_machine validation: transition motion graph '{}' has invalid input binding '{}'",
                    graph.id,
                    binding.port_id
                );
            }
            if !node_by_id
                .get(binding.to.node_id.as_str())
                .is_some_and(|node| node.is_timing())
            {
                continue;
            }
            if input_channel_by_node
                .insert(binding.to.node_id.as_str(), binding.port_id.as_str())
                .is_some()
            {
                bail!(
                    "state_machine validation: transition motion graph '{}' node '{}' has multiple property inputs",
                    graph.id,
                    binding.to.node_id
                );
            }
        }
        let mut covered_outputs = HashSet::new();
        for binding in &graph.output_bindings {
            if !output_ids.contains(binding.port_id.as_str())
                || !node_ids.contains(binding.from.node_id.as_str())
            {
                bail!(
                    "state_machine validation: transition motion graph '{}' has invalid output binding '{}'",
                    graph.id,
                    binding.port_id
                );
            }
            if !node_by_id
                .get(binding.from.node_id.as_str())
                .is_some_and(|node| node.is_timing())
            {
                bail!(
                    "state_machine validation: transition motion graph '{}' output '{}' must be driven by a timing node",
                    graph.id,
                    binding.port_id
                );
            }
            let input_channel = input_channel_by_node
                .get(binding.from.node_id.as_str())
                .copied()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "state_machine validation: transition motion graph '{}' node '{}' has an output without a State In binding",
                        graph.id,
                        binding.from.node_id
                    )
                })?;
            if input_channel != binding.port_id {
                bail!(
                    "state_machine validation: transition motion graph '{}' crosses property '{}' to '{}'",
                    graph.id,
                    input_channel,
                    binding.port_id
                );
            }
            if !covered_outputs.insert(binding.port_id.as_str()) {
                bail!(
                    "state_machine validation: transition motion graph '{}' has conflicting outputs for '{}'",
                    graph.id,
                    binding.port_id
                );
            }
        }
        for passthrough in &graph.passthrough_bindings {
            if !input_ids.contains(passthrough.from_port_id.as_str())
                || !output_ids.contains(passthrough.to_port_id.as_str())
            {
                bail!(
                    "state_machine validation: transition motion graph '{}' has invalid passthrough",
                    graph.id
                );
            }
            if passthrough.from_port_id != passthrough.to_port_id {
                bail!(
                    "state_machine validation: transition motion graph '{}' crosses passthrough properties",
                    graph.id
                );
            }
            if !covered_outputs.insert(passthrough.to_port_id.as_str()) {
                bail!(
                    "state_machine validation: transition motion graph '{}' has conflicting outputs for '{}'",
                    graph.id,
                    passthrough.to_port_id
                );
            }
        }
        let has_any_fallback = covered_outputs.contains("*");
        for output_id in &output_ids {
            if !has_any_fallback && !covered_outputs.contains(output_id) {
                bail!(
                    "state_machine validation: transition motion graph '{}' leaves property '{}' uncovered",
                    graph.id,
                    output_id
                );
            }
        }

        match graph.condition_binding.as_ref() {
            None => {}
            Some(TransitionConditionBinding::Input { input_port_id }) => {
                let valid = graph.inputs.iter().any(|port| {
                    port.id == *input_port_id && port.port_type.as_deref() == Some("bool")
                });
                if !valid {
                    bail!(
                        "state_machine validation: transition motion graph '{}' Condition Out requires a bool input",
                        graph.id
                    );
                }
            }
            Some(TransitionConditionBinding::Node { from }) => {
                let valid = node_by_id
                    .get(from.node_id.as_str())
                    .is_some_and(|node| match node {
                        TransitionMotionNode::EventTrigger { .. } => from.port_id == "fired",
                        TransitionMotionNode::Logic { .. } => from.port_id == "result",
                        TransitionMotionNode::BoolInput { .. } => from.port_id == "value",
                        _ => false,
                    });
                if !valid {
                    bail!(
                        "state_machine validation: transition motion graph '{}' Condition Out requires a bool condition-node output",
                        graph.id
                    );
                }
            }
        }

        for node in &graph.nodes {
            if let Some((_curve, timeline)) = node.timeline() {
                if !timeline.duration.is_finite() || timeline.duration < 0.0 {
                    bail!("state_machine validation: timeline duration must be >= 0");
                }
                if !timeline.delay.is_finite() || timeline.delay < 0.0 {
                    bail!("state_machine validation: timeline delay must be >= 0");
                }
                if let Some(blending) = &timeline.blending
                    && (!blending.duration.is_finite() || blending.duration < 0.0)
                {
                    bail!("state_machine validation: blending duration must be >= 0");
                }
                continue;
            }
            match node {
                TransitionMotionNode::RepeatTimeline {
                    from, to, duration, ..
                } => {
                    if !from.is_finite() || !to.is_finite() {
                        bail!("state_machine validation: repeat timeline endpoints must be finite");
                    }
                    if !duration.is_finite() || *duration <= 0.0 {
                        bail!("state_machine validation: repeat timeline duration must be > 0");
                    }
                    let valid_consumer = graph.connections.iter().any(|connection| {
                        connection.from.node_id == node.id()
                            && connection.from.port_id == "target"
                            && connection.to.port_id == "target"
                            && node_by_id.get(connection.to.node_id.as_str()).is_some_and(
                                |target| {
                                    matches!(target, TransitionMotionNode::SpringFollow { .. })
                                },
                            )
                    });
                    if !valid_consumer {
                        bail!(
                            "state_machine validation: RepeatTimeline must drive SpringFollow.target"
                        );
                    }
                }
                TransitionMotionNode::SpringFollow {
                    duration, bounce, ..
                } => {
                    if !duration.is_finite() || *duration <= 0.0 {
                        bail!("state_machine validation: spring follow duration must be > 0");
                    }
                    if !bounce.is_finite() || !(-1.0..1.0).contains(bounce) {
                        bail!("state_machine validation: spring follow bounce must be in (-1, 1)");
                    }
                    let repeat_inputs = graph
                        .connections
                        .iter()
                        .filter(|connection| {
                            connection.to.node_id == node.id()
                                && connection.to.port_id == "target"
                                && connection.from.port_id == "target"
                                && node_by_id
                                    .get(connection.from.node_id.as_str())
                                    .is_some_and(|source| {
                                        matches!(
                                            source,
                                            TransitionMotionNode::RepeatTimeline { .. }
                                        )
                                    })
                        })
                        .count();
                    if repeat_inputs != 1 {
                        bail!(
                            "state_machine validation: SpringFollow requires exactly one RepeatTimeline target"
                        );
                    }
                }
                TransitionMotionNode::Spring {
                    duration,
                    bounce,
                    delay,
                    ..
                } => {
                    if !duration.is_finite() || *duration <= 0.0 {
                        bail!("state_machine validation: spring duration must be > 0");
                    }
                    if !bounce.is_finite() || !(-1.0..1.0).contains(bounce) {
                        bail!("state_machine validation: spring bounce must be in (-1, 1)");
                    }
                    if !delay.is_finite() || *delay < 0.0 {
                        bail!("state_machine validation: spring delay must be >= 0");
                    }
                }
                TransitionMotionNode::Instant { .. }
                | TransitionMotionNode::EventTrigger { .. }
                | TransitionMotionNode::Logic { .. }
                | TransitionMotionNode::BoolInput { .. }
                | TransitionMotionNode::MathAdd { .. }
                | TransitionMotionNode::MathSubtract { .. }
                | TransitionMotionNode::MathMultiply { .. }
                | TransitionMotionNode::MathDivide { .. }
                | TransitionMotionNode::Lerp { .. } => {}
                TransitionMotionNode::FloatInput { value, .. } => {
                    if !value.is_finite() {
                        bail!("state_machine validation: FloatInput value must be finite");
                    }
                }
                _ => unreachable!("timeline motion nodes returned above"),
            }
        }
    }
    Ok(())
}

// ── Directional constraints ────────────────────────────────────────────────

fn validate_transition_direction_constraints(sm: &StateMachine) -> Result<()> {
    let state_types: HashMap<&str, AnimationStateType> = sm
        .states
        .iter()
        .map(|s| (s.id.as_str(), s.resolved_type()))
        .collect();

    for t in &sm.transitions {
        // exitState is source-forbidden
        if let Some(AnimationStateType::ExitState) = state_types.get(t.source.as_str()) {
            bail!(
                "state_machine validation: transition '{}' cannot use exitState '{}' as source",
                t.id,
                t.source
            );
        }
        // entryState and anyState are target-forbidden
        if let Some(st) = state_types.get(t.target.as_str()) {
            match st {
                AnimationStateType::EntryState => bail!(
                    "state_machine validation: transition '{}' cannot target entryState '{}'",
                    t.id,
                    t.target
                ),
                AnimationStateType::AnyState => bail!(
                    "state_machine validation: transition '{}' cannot target anyState '{}'",
                    t.id,
                    t.target
                ),
                _ => {}
            }
        }
    }

    Ok(())
}

// ── Mutation internal graph ────────────────────────────────────────────────

fn validate_mutation_internals(sm: &StateMachine) -> Result<()> {
    for m in &sm.mutations {
        validate_mutation_inner_node_ids(m)?;
        validate_mutation_connections(m)?;
        validate_mutation_bindings(m)?;
        validate_mutation_output_uniqueness(m)?;
    }
    Ok(())
}

fn validate_mutation_inner_node_ids(m: &MutationDefinition) -> Result<()> {
    let mut seen: HashSet<&str> = HashSet::new();
    for n in &m.nodes {
        if !seen.insert(n.id.as_str()) {
            bail!(
                "state_machine validation: mutation '{}' has duplicate inner-node id '{}'",
                m.id,
                n.id
            );
        }
    }
    Ok(())
}

fn validate_mutation_connections(m: &MutationDefinition) -> Result<()> {
    let node_ids: HashSet<&str> = m.nodes.iter().map(|n| n.id.as_str()).collect();

    for c in &m.connections {
        if !node_ids.contains(c.from.node_id.as_str()) {
            bail!(
                "state_machine validation: mutation '{}' connection '{}' from references missing node '{}'",
                m.id,
                c.id,
                c.from.node_id
            );
        }
        if !node_ids.contains(c.to.node_id.as_str()) {
            bail!(
                "state_machine validation: mutation '{}' connection '{}' to references missing node '{}'",
                m.id,
                c.id,
                c.to.node_id
            );
        }
    }

    Ok(())
}

fn validate_mutation_bindings(m: &MutationDefinition) -> Result<()> {
    let node_ids: HashSet<&str> = m.nodes.iter().map(|n| n.id.as_str()).collect();

    for b in &m.input_bindings {
        if !node_ids.contains(b.to.node_id.as_str()) {
            bail!(
                "state_machine validation: mutation '{}' inputBinding port '{}' targets missing node '{}'",
                m.id,
                b.port_id,
                b.to.node_id
            );
        }
    }

    for b in &m.output_bindings {
        if !node_ids.contains(b.from.node_id.as_str()) {
            bail!(
                "state_machine validation: mutation '{}' outputBinding port '{}' sources from missing node '{}'",
                m.id,
                b.port_id,
                b.from.node_id
            );
        }
    }

    Ok(())
}

/// Validate that no two binding types write to the same output port.
///
/// An output port may be written by at most one of:
/// - An `outputBinding` (via `portId`)
/// - A `passthroughBinding` (via `outputPortId`)
///
/// Duplicates are validation errors.
fn validate_mutation_output_uniqueness(m: &MutationDefinition) -> Result<()> {
    let mut seen: HashSet<&str> = HashSet::new();

    for b in &m.output_bindings {
        if !seen.insert(b.port_id.as_str()) {
            bail!(
                "state_machine validation: mutation '{}' has duplicate output for port '{}'",
                m.id,
                b.port_id
            );
        }
    }

    for pt in &m.passthrough_bindings {
        if !seen.insert(pt.to_port_id.as_str()) {
            bail!(
                "state_machine validation: mutation '{}' has duplicate output for port '{}' (passthrough conflicts with existing binding)",
                m.id,
                pt.to_port_id
            );
        }
    }

    Ok(())
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
            motion_graphs: vec![instant_motion_graph()],
            initial_state_id: Some("entry".into()),
            viewport: None,
        }
    }

    fn instant_motion_graph() -> TransitionMotionGraph {
        let port = MutationPort {
            id: "*".into(),
            name: Some("Any".into()),
            port_type: Some("any".into()),
        };
        TransitionMotionGraph {
            id: "instant".into(),
            name: "Instant".into(),
            inputs: vec![port.clone()],
            outputs: vec![port],
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

    #[test]
    fn minimal_valid() {
        assert!(validate(&minimal_sm()).is_ok());
    }

    #[test]
    fn motion_graph_rejects_cross_property_routes() {
        let mut sm = minimal_sm();
        let graph = &mut sm.motion_graphs[0];
        graph.inputs[0].id = "Node:x".into();
        graph.outputs[0].id = "Node:y".into();
        graph.input_bindings[0].port_id = "Node:x".into();
        graph.output_bindings[0].port_id = "Node:y".into();

        let err = validate(&sm).unwrap_err().to_string();
        assert!(err.contains("crosses property"), "{err}");
    }

    #[test]
    fn motion_graph_rejects_cycles() {
        let mut sm = minimal_sm();
        let graph = &mut sm.motion_graphs[0];
        graph.nodes.push(TransitionMotionNode::Instant {
            id: "motion2".into(),
            position: Position::default(),
            label: None,
        });
        graph.connections = vec![
            MutationConnection {
                id: "a".into(),
                from: MutationEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
                to: MutationEndpoint {
                    node_id: "motion2".into(),
                    port_id: "value".into(),
                },
            },
            MutationConnection {
                id: "b".into(),
                from: MutationEndpoint {
                    node_id: "motion2".into(),
                    port_id: "value".into(),
                },
                to: MutationEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
            },
        ];

        let err = validate(&sm).unwrap_err().to_string();
        assert!(err.contains("contains a cycle"), "{err}");
    }

    #[test]
    fn duplicate_state_id() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "entry".into(),
            name: "Dup".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_id: None,
        });
        let err = validate(&sm).unwrap_err().to_string();
        assert!(err.contains("duplicate state id"), "{err}");
    }

    #[test]
    fn missing_entry_state() {
        let mut sm = minimal_sm();
        sm.states
            .retain(|s| s.resolved_type() != AnimationStateType::EntryState);
        let err = validate(&sm).unwrap_err().to_string();
        assert!(err.contains("entryState"), "{err}");
    }

    #[test]
    fn mutation_node_missing_ref() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "mut1".into(),
            name: "M1".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_id: Some("nonexistent".into()),
        });
        let err = validate(&sm).unwrap_err().to_string();
        assert!(err.contains("missing mutation"), "{err}");
    }

    #[test]
    fn exit_state_as_source() {
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
            source: "exit".into(),
            target: "s1".into(),
            motion_graph_id: "instant".into(),
        });
        let err = validate(&sm).unwrap_err().to_string();
        assert!(err.contains("exitState"), "{err}");
    }

    #[test]
    fn entry_state_as_target() {
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
            source: "s1".into(),
            target: "entry".into(),
            motion_graph_id: "instant".into(),
        });
        let err = validate(&sm).unwrap_err().to_string();
        assert!(err.contains("entryState"), "{err}");
    }

    #[test]
    fn transition_motion_graph_cannot_be_shared() {
        let mut sm = minimal_sm();
        for id in ["t1", "t2"] {
            sm.transitions.push(AnimationTransition {
                id: id.into(),
                source: "entry".into(),
                target: "exit".into(),
                motion_graph_id: "instant".into(),
            });
        }

        let err = validate(&sm).unwrap_err().to_string();
        assert!(err.contains("independent motion graph"), "{err}");
    }

    #[test]
    fn passthrough_duplicate_output_rejected() {
        let mut sm = minimal_sm();
        let mutation = MutationDefinition {
            id: "m1".into(),
            name: "M1".into(),
            inputs: vec![],
            outputs: vec![MutationPort {
                id: "X:value".into(),
                name: Some("X".into()),
                port_type: Some("float".into()),
            }],
            nodes: vec![MutationInnerNode {
                id: "n".into(),
                node_type: MutationInnerNodeType::FloatInput,
                params: [("value".into(), serde_json::json!(42.0))]
                    .into_iter()
                    .collect(),
                inputs: vec![],
                outputs: vec![MutationPort {
                    id: "value".into(),
                    name: None,
                    port_type: None,
                }],
            }],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![MutationOutputBinding {
                port_id: "X:value".into(),
                from: MutationEndpoint {
                    node_id: "n".into(),
                    port_id: "value".into(),
                },
            }],
            passthrough_bindings: vec![MutationPassthroughBinding {
                from_port_id: "sceneElapsedTime".into(),
                to_port_id: "X:value".into(),
            }],
            viewport: None,
        };
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_id: Some("m1".into()),
        });
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "entry".into(),
            target: "s1".into(),
            motion_graph_id: "instant".into(),
        });
        sm.mutations.push(mutation);
        let err = validate(&sm).unwrap_err().to_string();
        assert!(
            err.contains("duplicate output") && err.contains("passthrough"),
            "expected passthrough conflict error, got: {err}"
        );
    }
}
