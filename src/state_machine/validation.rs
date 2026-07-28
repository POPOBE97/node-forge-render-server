//! Structural and semantic validation for `StateMachine` definitions.
//!
//! All checks are intentionally fail-fast: on the first error encountered
//! an `Err` is returned with a human-readable diagnostic that includes
//! relevant IDs (stateId / transitionId / derivationId).

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use crate::dsl::SceneDSL;

use super::types::*;

/// Validate a `StateMachine` definition.
///
/// Returns `Ok(())` when the definition is structurally sound, or an `Err`
/// with an actionable diagnostic on the first violation encountered.
pub fn validate(sm: &StateMachine) -> Result<()> {
    validate_state_ids(sm)?;
    validate_builtin_states(sm)?;
    validate_graph_ownership(sm)?;
    validate_transition_endpoints(sm)?;
    validate_transition_direction_constraints(sm)?;
    validate_graphs(sm)?;
    validate_motion_graphs(sm)?;
    Ok(())
}

/// Validate graph boundary identities against the writable declarations in the
/// complete Render Graph. This is scene-level because declarations may live in
/// the root graph or inside a reusable Group.
pub fn validate_scene_declarations(scene: &SceneDSL, sm: &StateMachine) -> Result<()> {
    let declarations = collect_formal_render_declarations(scene)?;
    let runtime_inputs = [
        "sceneElapsedTime",
        "localElapsedTime",
        "mouse.position.x",
        "mouse.position.y",
    ]
    .into_iter()
    .collect::<HashSet<_>>();

    for state in sm
        .states
        .iter()
        .filter(|state| state.state_type == AnimationStateType::AnimationState)
    {
        let graph = state
            .mutation_graph
            .as_ref()
            .expect("graph ownership validation requires a regular State mutationGraph");
        for input in &graph.inputs {
            if runtime_inputs.contains(input.id.as_str()) {
                validate_runtime_input_port(
                    input,
                    &format!("State '{}' Mutation input", state.id),
                )?;
            } else {
                validate_formal_declaration_port(
                    input,
                    &declarations,
                    &format!("State '{}' Mutation input", state.id),
                )?;
            }
        }
        for output in &graph.outputs {
            validate_formal_declaration_port(
                output,
                &declarations,
                &format!("State '{}' output", state.id),
            )?;
        }
        let output_ids = graph
            .outputs
            .iter()
            .map(|port| port.id.as_str())
            .collect::<HashSet<_>>();
        for declaration_id in declarations.keys() {
            if !output_ids.contains(declaration_id.as_str()) {
                bail!(
                    "state_machine validation: State '{}' State Outputs is missing formal declaration '{}'",
                    state.id,
                    declaration_id
                );
            }
        }
    }

    for derivation in &sm.derivations {
        for input in &derivation.inputs {
            if runtime_inputs.contains(input.id.as_str()) {
                validate_runtime_input_port(
                    input,
                    &format!("Derivation '{}' input", derivation.id),
                )?;
            } else {
                validate_formal_declaration_port(
                    input,
                    &declarations,
                    &format!("Derivation '{}' input", derivation.id),
                )?;
            }
        }
        for output in &derivation.outputs {
            validate_formal_declaration_port(
                output,
                &declarations,
                &format!("Derivation '{}' output", derivation.id),
            )?;
        }
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FormalRenderDeclaration {
    port_type: String,
    array_length: Option<usize>,
}

fn collect_formal_render_declarations(
    scene: &SceneDSL,
) -> Result<HashMap<String, FormalRenderDeclaration>> {
    let mut declarations = HashMap::new();
    for node in &scene.nodes {
        for (param_id, port_type) in referenceable_params(node.node_type.as_str()) {
            let array_length = port_type
                .starts_with("packed<")
                .then(|| {
                    node.params
                        .get(*param_id)
                        .and_then(serde_json::Value::as_array)
                        .map(Vec::len)
                })
                .flatten();
            insert_formal_declaration(
                &mut declarations,
                format!("{}:{param_id}", node.id),
                FormalRenderDeclaration {
                    port_type: (*port_type).to_string(),
                    array_length,
                },
            )?;
        }

        if node.node_type == "PackedInput" {
            let output = node.outputs.iter().find(|port| port.id == "value");
            let element_type = node
                .params
                .get("elementType")
                .and_then(serde_json::Value::as_str)
                .filter(|value| {
                    matches!(
                        *value,
                        "float" | "int" | "bool" | "vector2" | "vector3" | "vector4" | "color"
                    )
                })
                .unwrap_or("float");
            let array_length = output
                .and_then(|port| port.array_length)
                .or_else(|| {
                    node.params
                        .get("value")
                        .and_then(serde_json::Value::as_array)
                        .map(Vec::len)
                })
                .or_else(|| (!node.inputs.is_empty()).then_some(node.inputs.len()));
            insert_formal_declaration(
                &mut declarations,
                format!("{}:value", node.id),
                FormalRenderDeclaration {
                    port_type: output
                        .and_then(|port| port.port_type.clone())
                        .unwrap_or_else(|| format!("packed<{element_type}>")),
                    array_length,
                },
            )?;
        }
    }
    Ok(declarations)
}

fn referenceable_params(node_type: &str) -> &'static [(&'static str, &'static str)] {
    match node_type {
        "ColorInput" => &[("value", "color")],
        "FloatInput" => &[("value", "float")],
        "IntInput" => &[("value", "int")],
        "BoolInput" => &[("value", "bool")],
        "Vector2Input" => &[("x", "float"), ("y", "float")],
        "Vector3Input" => &[("x", "float"), ("y", "float"), ("z", "float")],
        "Vector4Input" => &[
            ("x", "float"),
            ("y", "float"),
            ("z", "float"),
            ("w", "float"),
        ],
        "ColorArrayInput" => &[("value", "packed<color>")],
        "Vector2ArrayInput" => &[("value", "packed<vector2>")],
        _ => &[],
    }
}

fn insert_formal_declaration(
    declarations: &mut HashMap<String, FormalRenderDeclaration>,
    id: String,
    declaration: FormalRenderDeclaration,
) -> Result<()> {
    if let Some(existing) = declarations.insert(id.clone(), declaration.clone())
        && existing != declaration
    {
        bail!(
            "state_machine validation: formal render declaration '{id}' has conflicting definitions"
        );
    }
    Ok(())
}

fn validate_runtime_input_port(port: &GraphPort, owner: &str) -> Result<()> {
    if port.port_type.as_deref() != Some("float") || port.array_length.is_some() {
        bail!(
            "state_machine validation: {owner} '{}' must be a scalar float runtime input",
            port.id
        );
    }
    Ok(())
}

fn validate_formal_declaration_port(
    port: &GraphPort,
    declarations: &HashMap<String, FormalRenderDeclaration>,
    owner: &str,
) -> Result<()> {
    let declaration = declarations.get(&port.id).ok_or_else(|| {
        anyhow::anyhow!(
            "state_machine validation: {owner} '{}' is not a formal writable Render Graph declaration",
            port.id
        )
    })?;
    if port.port_type.as_deref() != Some(declaration.port_type.as_str())
        || port.array_length != declaration.array_length
    {
        bail!(
            "state_machine validation: {owner} '{}' must exactly match type '{}' and fixed length {:?}",
            port.id,
            declaration.port_type,
            declaration.array_length
        );
    }
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
    if let Some(initial_state_id) = sm.initial_state_id.as_deref()
        && sm.states.iter().any(|state| {
            state.id == initial_state_id && state.state_type == AnimationStateType::DerivationNode
        })
    {
        bail!(
            "state_machine validation: initialStateId '{}' cannot reference a derivationNode",
            initial_state_id
        );
    }

    Ok(())
}

// ── State-private Mutation and shared Derivation ownership ─────────────────

fn validate_graph_ownership(sm: &StateMachine) -> Result<()> {
    let mut derivation_ids = HashSet::new();
    for derivation in &sm.derivations {
        if !derivation_ids.insert(derivation.id.as_str()) {
            bail!(
                "state_machine validation: duplicate Derivation id '{}'",
                derivation.id
            );
        }
    }
    let mut owner_by_derivation: HashMap<&str, &str> = HashMap::new();
    let state_by_id: HashMap<&str, &AnimationState> = sm
        .states
        .iter()
        .map(|state| (state.id.as_str(), state))
        .collect();

    for state in &sm.states {
        match state.state_type {
            AnimationStateType::AnimationState => {
                if state.mutation_graph.is_none() {
                    bail!(
                        "state_machine validation: animationState '{}' is missing its private mutationGraph",
                        state.id
                    );
                }
                if state.derivation_id.is_some() {
                    bail!(
                        "state_machine validation: animationState '{}' cannot store derivationId",
                        state.id
                    );
                }
            }
            AnimationStateType::DerivationNode => {
                if state.mutation_graph.is_some() {
                    bail!(
                        "state_machine validation: derivationNode '{}' cannot own mutationGraph",
                        state.id
                    );
                }
                let derivation_id = state.derivation_id.as_deref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "state_machine validation: derivationNode '{}' is missing derivationId",
                        state.id
                    )
                })?;
                if !derivation_ids.contains(derivation_id) {
                    bail!(
                        "state_machine validation: derivationNode '{}' references missing Derivation '{}'",
                        state.id,
                        derivation_id
                    );
                }
                if let Some(existing) = owner_by_derivation.insert(derivation_id, state.id.as_str())
                {
                    bail!(
                        "state_machine validation: Derivation '{}' is owned by nodes '{}' and '{}'",
                        derivation_id,
                        existing,
                        state.id
                    );
                }
            }
            AnimationStateType::EntryState
            | AnimationStateType::AnyState
            | AnimationStateType::ExitState => {
                if state.mutation_graph.is_some() || state.derivation_id.is_some() {
                    bail!(
                        "state_machine validation: built-in state '{}' cannot own a graph",
                        state.id
                    );
                }
            }
        }
    }
    for derivation in &sm.derivations {
        if !owner_by_derivation.contains_key(derivation.id.as_str()) {
            bail!(
                "state_machine validation: Derivation '{}' has no derivationNode owner",
                derivation.id
            );
        }
    }

    let mut binding_ids = HashSet::new();
    let mut bound_state_ids: HashMap<&str, &str> = HashMap::new();
    for binding in &sm.derivation_bindings {
        if !binding_ids.insert(binding.id.as_str()) {
            bail!(
                "state_machine validation: duplicate Derivation binding id '{}'",
                binding.id
            );
        }
        let state = state_by_id.get(binding.state_id.as_str()).ok_or_else(|| {
            anyhow::anyhow!(
                "state_machine validation: Derivation binding '{}' references missing State '{}'",
                binding.id,
                binding.state_id
            )
        })?;
        if state.state_type != AnimationStateType::AnimationState {
            bail!(
                "state_machine validation: Derivation binding '{}' endpoint '{}' is not an animationState",
                binding.id,
                binding.state_id
            );
        }
        let derivation_node = state_by_id
            .get(binding.derivation_node_id.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "state_machine validation: Derivation binding '{}' references missing derivationNode '{}'",
                    binding.id,
                    binding.derivation_node_id
                )
            })?;
        if derivation_node.state_type != AnimationStateType::DerivationNode {
            bail!(
                "state_machine validation: Derivation binding '{}' endpoint '{}' is not a derivationNode",
                binding.id,
                binding.derivation_node_id
            );
        }
        if let Some(existing) =
            bound_state_ids.insert(binding.state_id.as_str(), binding.id.as_str())
        {
            bail!(
                "state_machine validation: State '{}' has multiple Derivation bindings '{}' and '{}'",
                binding.state_id,
                existing,
                binding.id
            );
        }
    }

    Ok(())
}

// ── Transition endpoint references ─────────────────────────────────────────

fn validate_transition_endpoints(sm: &StateMachine) -> Result<()> {
    let state_by_id: HashMap<&str, &AnimationState> =
        sm.states.iter().map(|s| (s.id.as_str(), s)).collect();
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
        if !state_by_id.contains_key(t.source.as_str()) {
            bail!(
                "state_machine validation: transition '{}' source '{}' references missing state",
                t.id,
                t.source
            );
        }
        if !state_by_id.contains_key(t.target.as_str()) {
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
        if state_by_id
            .get(t.source.as_str())
            .is_some_and(|state| state.state_type == AnimationStateType::DerivationNode)
            || state_by_id
                .get(t.target.as_str())
                .is_some_and(|state| state.state_type == AnimationStateType::DerivationNode)
        {
            bail!(
                "state_machine validation: transition '{}' cannot use a derivationNode as source or target",
                t.id
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
            if from.is_some_and(TransitionMotionNode::is_timing)
                || to.is_some_and(TransitionMotionNode::is_timing)
            {
                bail!(
                    "state_machine validation: transition motion graph '{}' timing nodes bind directly to boundary channels",
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

// ── State Mutation and Render Derivation graphs ────────────────────────────

fn validate_graphs(sm: &StateMachine) -> Result<()> {
    for state in &sm.states {
        if let Some(graph) = &state.mutation_graph {
            validate_graph_core(
                &format!("State '{}' Mutation", state.id),
                &graph.inputs,
                &graph.outputs,
                &graph.nodes,
                &graph.connections,
                true,
            )?;
            validate_boundary_bindings(
                &format!("State '{}' Mutation", state.id),
                &graph.inputs,
                &graph.outputs,
                &graph.nodes,
                graph
                    .input_bindings
                    .iter()
                    .map(|binding| (binding.state_port_id.as_str(), &binding.to)),
                graph
                    .output_bindings
                    .iter()
                    .map(|binding| (binding.state_port_id.as_str(), &binding.from)),
                std::iter::empty(),
                true,
            )?;
        }
    }
    for derivation in &sm.derivations {
        validate_graph_core(
            &format!("Derivation '{}'", derivation.id),
            &derivation.inputs,
            &derivation.outputs,
            &derivation.nodes,
            &derivation.connections,
            false,
        )?;
        validate_boundary_bindings(
            &format!("Derivation '{}'", derivation.id),
            &derivation.inputs,
            &derivation.outputs,
            &derivation.nodes,
            derivation
                .input_bindings
                .iter()
                .map(|binding| (binding.port_id.as_str(), &binding.to)),
            derivation
                .output_bindings
                .iter()
                .map(|binding| (binding.port_id.as_str(), &binding.from)),
            derivation
                .passthrough_bindings
                .iter()
                .map(|binding| (binding.from_port_id.as_str(), binding.to_port_id.as_str())),
            false,
        )?;
    }
    Ok(())
}

fn validate_graph_core(
    label: &str,
    _inputs: &[GraphPort],
    _outputs: &[GraphPort],
    nodes: &[GraphInnerNode],
    connections: &[GraphConnection],
    is_mutation: bool,
) -> Result<()> {
    let mut ids = HashSet::new();
    for node in nodes {
        if !ids.insert(node.id.as_str()) {
            bail!("{label} has duplicate node id '{}'", node.id);
        }
        if node.inputs.iter().any(|port| port.motion == Some(true)) {
            bail!(
                "{label} node '{}' has a Motion input; Motion is return-only",
                node.id
            );
        }
        match (is_mutation, node.node_type) {
            (true, GraphInnerNodeType::DerivationFunction) => {
                bail!("{label} cannot contain a DerivationFunction")
            }
            (false, GraphInnerNodeType::MutationFunction) => {
                bail!("{label} cannot contain a MutationFunction")
            }
            _ => {}
        }
        if !is_mutation && node.outputs.iter().any(|port| port.motion == Some(true)) {
            bail!("{label} cannot declare a Motion output");
        }
    }

    let node_by_id = nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut incoming = HashSet::new();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut indegree = nodes
        .iter()
        .map(|node| (node.id.as_str(), 0usize))
        .collect::<HashMap<_, _>>();
    for connection in connections {
        let source = node_by_id
            .get(connection.from.node_id.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{label} connection '{}' has missing source node",
                    connection.id
                )
            })?;
        let target = node_by_id
            .get(connection.to.node_id.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{label} connection '{}' has missing target node",
                    connection.id
                )
            })?;
        let source_port = source
            .outputs
            .iter()
            .find(|port| port.id == connection.from.port_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{label} connection '{}' has missing source port",
                    connection.id
                )
            })?;
        let target_port = target
            .inputs
            .iter()
            .find(|port| port.id == connection.to.port_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{label} connection '{}' has missing target port",
                    connection.id
                )
            })?;
        validate_port_compatibility(label, source_port, target_port)?;
        if !incoming.insert((
            connection.to.node_id.as_str(),
            connection.to.port_id.as_str(),
        )) {
            bail!(
                "{label} input '{}.{}' has multiple writers",
                connection.to.node_id,
                connection.to.port_id
            );
        }
        adjacency
            .entry(connection.from.node_id.as_str())
            .or_default()
            .push(connection.to.node_id.as_str());
        *indegree
            .get_mut(connection.to.node_id.as_str())
            .expect("validated target exists") += 1;
    }
    let mut ready = indegree
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(*id))
        .collect::<Vec<_>>();
    let mut visited = 0usize;
    while let Some(id) = ready.pop() {
        visited += 1;
        for target in adjacency.get(id).into_iter().flatten() {
            let count = indegree
                .get_mut(target)
                .expect("validated graph node exists");
            *count -= 1;
            if *count == 0 {
                ready.push(target);
            }
        }
    }
    if visited != nodes.len() {
        bail!("{label} contains a cycle");
    }
    Ok(())
}

fn validate_boundary_bindings<'a>(
    label: &str,
    inputs: &[GraphPort],
    outputs: &[GraphPort],
    nodes: &'a [GraphInnerNode],
    input_bindings: impl Iterator<Item = (&'a str, &'a GraphEndpoint)>,
    output_bindings: impl Iterator<Item = (&'a str, &'a GraphEndpoint)>,
    passthrough_bindings: impl Iterator<Item = (&'a str, &'a str)>,
    is_mutation: bool,
) -> Result<()> {
    let node_by_id = nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut written_inputs = HashSet::new();
    for (boundary_id, endpoint) in input_bindings {
        let boundary = inputs
            .iter()
            .find(|port| port.id == boundary_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{label} input binding references missing boundary port '{boundary_id}'"
                )
            })?;
        let node = node_by_id.get(endpoint.node_id.as_str()).ok_or_else(|| {
            anyhow::anyhow!(
                "{label} input binding references missing node '{}'",
                endpoint.node_id
            )
        })?;
        let port = node
            .inputs
            .iter()
            .find(|port| port.id == endpoint.port_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{label} input binding references missing port '{}.{}'",
                    endpoint.node_id,
                    endpoint.port_id
                )
            })?;
        validate_port_compatibility(label, boundary, port)?;
        if !written_inputs.insert((endpoint.node_id.as_str(), endpoint.port_id.as_str())) {
            bail!(
                "{label} input '{}.{}' has multiple writers",
                endpoint.node_id,
                endpoint.port_id
            );
        }
    }

    let mut written_outputs = HashSet::new();
    let mut bound_motion_sources = HashMap::<(&str, &str), usize>::new();
    for (boundary_id, endpoint) in output_bindings {
        let boundary = outputs
            .iter()
            .find(|port| port.id == boundary_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{label} output binding references missing boundary port '{boundary_id}'"
                )
            })?;
        if OverrideKey::parse(boundary_id).is_none() {
            bail!("{label} output '{boundary_id}' is not a formal render declaration");
        }
        let node = node_by_id.get(endpoint.node_id.as_str()).ok_or_else(|| {
            anyhow::anyhow!(
                "{label} output binding references missing node '{}'",
                endpoint.node_id
            )
        })?;
        let port = node
            .outputs
            .iter()
            .find(|port| port.id == endpoint.port_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{label} output binding references missing port '{}.{}'",
                    endpoint.node_id,
                    endpoint.port_id
                )
            })?;
        validate_port_compatibility(label, port, boundary)?;
        if is_mutation && port.motion != Some(true) {
            bail!("{label} State Output '{boundary_id}' must be driven by Motion<T>");
        }
        if !is_mutation && port.motion == Some(true) {
            bail!("{label} cannot bind Motion<T> to a render output");
        }
        if !written_outputs.insert(boundary_id) {
            bail!("{label} output '{boundary_id}' has multiple writers");
        }
        *bound_motion_sources
            .entry((endpoint.node_id.as_str(), endpoint.port_id.as_str()))
            .or_default() += 1;
    }
    if is_mutation {
        for node in nodes {
            for port in node.outputs.iter().filter(|port| port.motion == Some(true)) {
                let count = bound_motion_sources
                    .get(&(node.id.as_str(), port.id.as_str()))
                    .copied()
                    .unwrap_or(0);
                if count != 1 {
                    bail!(
                        "{label} Motion output '{}.{}' must bind exactly once to State Outputs",
                        node.id,
                        port.id
                    );
                }
            }
        }
    }

    for (from_id, to_id) in passthrough_bindings {
        let from = inputs
            .iter()
            .find(|port| port.id == from_id)
            .ok_or_else(|| {
                anyhow::anyhow!("{label} passthrough references missing input '{from_id}'")
            })?;
        let to = outputs
            .iter()
            .find(|port| port.id == to_id)
            .ok_or_else(|| {
                anyhow::anyhow!("{label} passthrough references missing output '{to_id}'")
            })?;
        validate_port_compatibility(label, from, to)?;
        if !written_outputs.insert(to_id) {
            bail!("{label} output '{to_id}' has multiple writers");
        }
    }
    Ok(())
}

fn validate_port_compatibility(label: &str, source: &GraphPort, target: &GraphPort) -> Result<()> {
    let source_type = source.port_type.as_deref().unwrap_or("any");
    let target_type = target.port_type.as_deref().unwrap_or("any");
    let compatible = target_type == "any"
        || source_type == target_type
        || matches!(
            (source_type, target_type),
            ("float" | "int" | "bool", "float" | "int")
                | (
                    "float" | "int" | "bool",
                    "vector2" | "vector3" | "vector4" | "color"
                )
                | (
                    "vector2" | "vector3" | "vector4" | "color",
                    "vector2" | "vector3" | "vector4" | "color"
                )
        );
    if !compatible || (source.array_length.is_some() && source.array_length != target.array_length)
    {
        bail!(
            "{label} has incompatible ports '{}' ({source_type}, {:?}) -> '{}' ({target_type}, {:?})",
            source.id,
            source.array_length,
            target.id,
            target.array_length
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::Node;

    fn scene_with_state_machine(sm: StateMachine, nodes: Vec<Node>) -> SceneDSL {
        SceneDSL {
            version: "3.0".into(),
            metadata: crate::dsl::Metadata {
                name: "Validation test".into(),
                created: None,
                modified: None,
            },
            nodes,
            connections: vec![],
            outputs: None,
            groups: vec![],
            assets: HashMap::new(),
            state_machine: Some(sm),
            debug_artifacts: None,
        }
    }

    fn render_node(id: &str, node_type: &str, params: HashMap<String, serde_json::Value>) -> Node {
        Node {
            id: id.into(),
            node_type: node_type.into(),
            params,
            inputs: vec![],
            outputs: vec![],
            input_bindings: vec![],
            wgsl_override: None,
        }
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
                    state_type: AnimationStateType::EntryState,
                    mutation_graph: None,
                    derivation_id: None,
                },
                AnimationState {
                    id: "any".into(),
                    name: "Any".into(),
                    position: None,
                    parameter_overrides: Default::default(),
                    state_type: AnimationStateType::AnyState,
                    mutation_graph: None,
                    derivation_id: None,
                },
                AnimationState {
                    id: "exit".into(),
                    name: "Exit".into(),
                    position: None,
                    parameter_overrides: Default::default(),
                    state_type: AnimationStateType::ExitState,
                    mutation_graph: None,
                    derivation_id: None,
                },
            ],
            transitions: vec![],
            derivation_bindings: vec![],
            derivations: vec![],
            motion_graphs: vec![instant_motion_graph()],
            initial_state_id: Some("entry".into()),
            viewport: None,
        }
    }

    fn instant_motion_graph() -> TransitionMotionGraph {
        let port = GraphPort {
            id: "*".into(),
            name: Some("Any".into()),
            port_type: Some("any".into()),
            array_length: None,
            motion: None,
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
                to: GraphEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
            }],
            output_bindings: vec![TransitionMotionOutputBinding {
                port_id: "*".into(),
                from: GraphEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
            }],
            passthrough_bindings: vec![],
            condition_binding: None,
            viewport: None,
        }
    }

    fn empty_derivation(id: &str) -> DerivationDefinition {
        DerivationDefinition {
            id: id.into(),
            name: id.into(),
            inputs: vec![],
            outputs: vec![],
            nodes: vec![],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![],
            passthrough_bindings: vec![],
            layout: None,
            viewport: None,
        }
    }

    fn regular_state(id: &str) -> AnimationState {
        AnimationState {
            id: id.into(),
            name: id.into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: AnimationStateType::AnimationState,
            mutation_graph: Some(empty_state_mutation()),
            derivation_id: None,
        }
    }

    fn derivation_node(id: &str, derivation_id: &str) -> AnimationState {
        AnimationState {
            id: id.into(),
            name: id.into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: AnimationStateType::DerivationNode,
            mutation_graph: None,
            derivation_id: Some(derivation_id.into()),
        }
    }

    fn empty_state_mutation() -> StateMutationGraph {
        StateMutationGraph {
            inputs: vec![],
            outputs: vec![],
            nodes: vec![],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![],
            layout: StateMutationGraphLayout {
                parameter_positions: HashMap::new(),
                runtime_input_position: Position::default(),
                output_position: Position::default(),
                runtime_input_collapsed: false,
                output_collapsed: false,
            },
            viewport: None,
        }
    }

    #[test]
    fn minimal_valid() {
        assert!(validate(&minimal_sm()).is_ok());
    }

    #[test]
    fn scene_declarations_reject_derivation_output_targeting_a_consumer_port() {
        let mut sm = minimal_sm();
        sm.states.push(regular_state("state"));
        sm.states.push(derivation_node("derive_node", "derive"));
        let mut derivation = empty_derivation("derive");
        derivation.outputs.push(GraphPort {
            id: "consumer:opacity".into(),
            name: Some("Consumer opacity".into()),
            port_type: Some("float".into()),
            array_length: None,
            motion: None,
        });
        sm.derivations.push(derivation);
        validate(&sm).expect("the graph is structurally valid before scene identity validation");

        let scene = scene_with_state_machine(
            sm.clone(),
            vec![render_node("consumer", "Composite", HashMap::new())],
        );
        let error = validate_scene_declarations(&scene, &sm)
            .expect_err("consumer inputs cannot establish Derivation output identity")
            .to_string();
        assert!(error.contains("not a formal writable Render Graph declaration"));
    }

    #[test]
    fn scene_declarations_enforce_packed_array_length() {
        let mut sm = minimal_sm();
        let mut state = regular_state("state");
        state
            .mutation_graph
            .as_mut()
            .expect("regular State owns Mutation graph")
            .outputs
            .push(GraphPort {
                id: "colors:value".into(),
                name: Some("Colors".into()),
                port_type: Some("packed<color>".into()),
                array_length: Some(1),
                motion: None,
            });
        sm.states.push(state);
        validate(&sm).expect("the graph is structurally valid before scene identity validation");

        let scene = scene_with_state_machine(
            sm.clone(),
            vec![render_node(
                "colors",
                "ColorArrayInput",
                HashMap::from([(
                    "value".into(),
                    serde_json::json!([[1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]]),
                )]),
            )],
        );
        let error = validate_scene_declarations(&scene, &sm)
            .expect_err("State Outputs must keep the declaration's fixed length")
            .to_string();
        assert!(error.contains("fixed length Some(2)"));
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
            GraphConnection {
                id: "a".into(),
                from: GraphEndpoint {
                    node_id: "motion".into(),
                    port_id: "value".into(),
                },
                to: GraphEndpoint {
                    node_id: "motion2".into(),
                    port_id: "value".into(),
                },
            },
            GraphConnection {
                id: "b".into(),
                from: GraphEndpoint {
                    node_id: "motion2".into(),
                    port_id: "value".into(),
                },
                to: GraphEndpoint {
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
            mutation_graph: None,
            derivation_id: None,
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
    fn derivation_node_missing_ref() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "mut1".into(),
            name: "M1".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: AnimationStateType::DerivationNode,
            mutation_graph: None,
            derivation_id: Some("nonexistent".into()),
        });
        let err = validate(&sm).unwrap_err().to_string();
        assert!(err.contains("missing Derivation"), "{err}");
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
            mutation_graph: Some(empty_state_mutation()),
            derivation_id: None,
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
            mutation_graph: Some(empty_state_mutation()),
            derivation_id: None,
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
    fn derivation_node_can_feed_multiple_states() {
        let mut sm = minimal_sm();
        sm.states.extend([
            regular_state("state_a"),
            regular_state("state_b"),
            derivation_node("derivation_node", "derivation"),
        ]);
        sm.derivations.push(empty_derivation("derivation"));
        sm.derivation_bindings.extend([
            DerivationStateBinding {
                id: "binding_a".into(),
                state_id: "state_a".into(),
                derivation_node_id: "derivation_node".into(),
            },
            DerivationStateBinding {
                id: "binding_b".into(),
                state_id: "state_b".into(),
                derivation_node_id: "derivation_node".into(),
            },
        ]);

        validate(&sm).expect("a Derivation node output may fan out to multiple States");
    }

    #[test]
    fn state_cannot_receive_multiple_derivations() {
        let mut sm = minimal_sm();
        sm.states.extend([
            regular_state("state"),
            derivation_node("derivation_a_node", "derivation_a"),
            derivation_node("derivation_b_node", "derivation_b"),
        ]);
        sm.derivations.extend([
            empty_derivation("derivation_a"),
            empty_derivation("derivation_b"),
        ]);
        sm.derivation_bindings.extend([
            DerivationStateBinding {
                id: "binding_a".into(),
                state_id: "state".into(),
                derivation_node_id: "derivation_a_node".into(),
            },
            DerivationStateBinding {
                id: "binding_b".into(),
                state_id: "state".into(),
                derivation_node_id: "derivation_b_node".into(),
            },
        ]);

        let error = validate(&sm).unwrap_err().to_string();
        assert!(error.contains("multiple Derivation bindings"), "{error}");
    }

    #[test]
    fn passthrough_duplicate_output_rejected() {
        let mut sm = minimal_sm();
        let derivation = DerivationDefinition {
            id: "m1".into(),
            name: "M1".into(),
            inputs: vec![GraphPort {
                id: "sceneElapsedTime".into(),
                name: Some("Scene Elapsed Time".into()),
                port_type: Some("float".into()),
                array_length: None,
                motion: None,
            }],
            outputs: vec![GraphPort {
                id: "X:value".into(),
                name: Some("X".into()),
                port_type: Some("float".into()),
                array_length: None,
                motion: None,
            }],
            nodes: vec![GraphInnerNode {
                id: "n".into(),
                node_type: GraphInnerNodeType::FloatInput,
                params: [("value".into(), serde_json::json!(42.0))]
                    .into_iter()
                    .collect(),
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
                port_id: "X:value".into(),
                from: GraphEndpoint {
                    node_id: "n".into(),
                    port_id: "value".into(),
                },
            }],
            passthrough_bindings: vec![DerivationPassthroughBinding {
                from_port_id: "sceneElapsedTime".into(),
                to_port_id: "X:value".into(),
            }],
            layout: None,
            viewport: None,
        };
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: AnimationStateType::DerivationNode,
            mutation_graph: None,
            derivation_id: Some("m1".into()),
        });
        sm.derivations.push(derivation);
        let err = validate(&sm).unwrap_err().to_string();
        assert!(
            err.contains("output 'X:value' has multiple writers"),
            "{err}"
        );
    }

    #[test]
    fn declaration_binding_is_an_ordinary_state_value_input() {
        let mut mutation = empty_state_mutation();
        mutation.inputs.push(GraphPort {
            id: "FloatInput:value".into(),
            name: Some("FloatInput.value".into()),
            port_type: Some("float".into()),
            array_length: None,
            motion: None,
        });
        mutation.nodes.push(GraphInnerNode {
            id: "function".into(),
            node_type: GraphInnerNodeType::MutationFunction,
            params: HashMap::new(),
            inputs: vec![GraphPort {
                id: "value".into(),
                name: Some("value".into()),
                port_type: Some("float".into()),
                array_length: None,
                motion: None,
            }],
            outputs: vec![],
        });
        mutation.input_bindings.push(StateMutationInputBinding {
            state_port_id: "FloatInput:value".into(),
            to: GraphEndpoint {
                node_id: "function".into(),
                port_id: "value".into(),
            },
        });

        let mut sm = minimal_sm();
        let mut state = regular_state("state");
        state.mutation_graph = Some(mutation);
        sm.states.push(state);
        validate(&sm).expect("Mutation Inputs expose S as ordinary values");
    }

    #[test]
    fn motion_is_return_only() {
        let mut mutation = empty_state_mutation();
        mutation.nodes.push(GraphInnerNode {
            id: "function".into(),
            node_type: GraphInnerNodeType::MutationFunction,
            params: HashMap::new(),
            inputs: vec![GraphPort {
                id: "value".into(),
                name: Some("value".into()),
                port_type: Some("float".into()),
                array_length: None,
                motion: Some(true),
            }],
            outputs: vec![],
        });

        let mut sm = minimal_sm();
        let mut state = regular_state("state");
        state.mutation_graph = Some(mutation);
        sm.states.push(state);
        let error = validate(&sm)
            .expect_err("Motion inputs must be rejected")
            .to_string();
        assert!(error.contains("return-only"), "{error}");
    }

    #[test]
    fn motion_output_must_bind_to_one_declaration_and_may_feed_downstream() {
        let mut mutation = empty_state_mutation();
        mutation.outputs.push(GraphPort {
            id: "FloatInput:value".into(),
            name: Some("FloatInput.value".into()),
            port_type: Some("float".into()),
            array_length: None,
            motion: None,
        });
        mutation.nodes.extend([
            GraphInnerNode {
                id: "first".into(),
                node_type: GraphInnerNodeType::MutationFunction,
                params: HashMap::new(),
                inputs: vec![],
                outputs: vec![GraphPort {
                    id: "value".into(),
                    name: Some("physical".into()),
                    port_type: Some("float".into()),
                    array_length: None,
                    motion: Some(true),
                }],
            },
            GraphInnerNode {
                id: "second".into(),
                node_type: GraphInnerNodeType::MutationFunction,
                params: HashMap::new(),
                inputs: vec![GraphPort {
                    id: "value".into(),
                    name: Some("value".into()),
                    port_type: Some("float".into()),
                    array_length: None,
                    motion: None,
                }],
                outputs: vec![],
            },
        ]);
        mutation.connections.push(GraphConnection {
            id: "first_to_second".into(),
            from: GraphEndpoint {
                node_id: "first".into(),
                port_id: "value".into(),
            },
            to: GraphEndpoint {
                node_id: "second".into(),
                port_id: "value".into(),
            },
        });
        mutation.output_bindings.push(StateMutationOutputBinding {
            state_port_id: "FloatInput:value".into(),
            from: GraphEndpoint {
                node_id: "first".into(),
                port_id: "value".into(),
            },
        });

        let mut sm = minimal_sm();
        let mut state = regular_state("state");
        state.mutation_graph = Some(mutation);
        sm.states.push(state);
        validate(&sm).expect("Q may flow downstream and bind to one State Output identity");
    }
}
