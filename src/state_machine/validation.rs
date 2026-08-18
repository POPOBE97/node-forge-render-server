//! Structural and semantic validation for `StateMachine` definitions.
//!
//! All checks are intentionally fail-fast: on the first error encountered
//! an `Err` is returned with a human-readable diagnostic that includes
//! relevant IDs (stateId / transitionId / derivationId).

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use crate::dsl::SceneDSL;
use crate::schema;

use super::types::*;

/// Validate a `StateMachine` definition.
///
/// Returns `Ok(())` when the definition is structurally sound, or an `Err`
/// with an actionable diagnostic on the first violation encountered.
pub fn validate(sm: &StateMachine) -> Result<()> {
    validate_state_params(sm)?;
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
    for derivation in &sm.derivations {
        for binding in &derivation.output_bindings {
            let target_id = binding.uniform.id();
            let target = declarations.get(&target_id).ok_or_else(|| {
                anyhow::anyhow!(
                    "state_machine validation: Derivation '{}' output references missing GPU uniform '{}'",
                    derivation.id,
                    target_id
                )
            })?;
            let source = graph_output_port(&derivation.nodes, &binding.from, &derivation.id)?;
            validate_port_type(
                source.port_type.as_deref(),
                source.array_length,
                target,
                &format!("Derivation '{}' GPU uniform '{}'", derivation.id, target_id),
            )?;
        }
        for binding in &derivation.passthrough_bindings {
            let target_id = binding.uniform.id();
            let target = declarations.get(&target_id).ok_or_else(|| {
                anyhow::anyhow!(
                    "state_machine validation: Derivation '{}' passthrough references missing GPU uniform '{}'",
                    derivation.id,
                    target_id
                )
            })?;
            let source = source_port(sm, &binding.source)?;
            validate_port_type(
                source.port_type.as_deref(),
                source.array_length,
                target,
                &format!("Derivation '{}' GPU uniform '{}'", derivation.id, target_id),
            )?;
        }
    }

    Ok(())
}

fn validate_state_params(sm: &StateMachine) -> Result<()> {
    let scheme = schema::load_default_scheme()?;
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for declaration in &sm.state_params {
        if !ids.insert(declaration.id.as_str()) {
            bail!(
                "state_machine validation: duplicate State Param id '{}'",
                declaration.id
            );
        }
        if !names.insert(declaration.name.as_str()) {
            bail!(
                "state_machine validation: duplicate State Param name '{}'",
                declaration.name
            );
        }
        if !sm
            .state_param_graph
            .declaration_positions
            .contains_key(&declaration.id)
        {
            bail!(
                "state_machine validation: State Param '{}' is missing its Declaration Graph position",
                declaration.id
            );
        }
        let packed = declaration.param_type.starts_with("packed<");
        if packed != declaration.array_length.is_some() {
            bail!(
                "state_machine validation: State Param '{}' must declare arrayLength exactly when packed",
                declaration.id
            );
        }
        let contract = scheme
            .port_type_definitions
            .get(&declaration.param_type)
            .filter(|contract| contract.state_param)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "state_machine validation: State Param '{}' has unsupported type '{}'",
                    declaration.id,
                    declaration.param_type
                )
            })?;
        if contract.requires_array_length != declaration.array_length.is_some() {
            bail!(
                "state_machine validation: State Param '{}' arrayLength does not match the '{}' value contract",
                declaration.id,
                declaration.param_type
            );
        }
        validate_canonical_state_value(
            &declaration.param_type,
            &declaration.default_value,
            declaration.array_length,
            &format!("State Param '{}' defaultValue", declaration.id),
        )?;
    }
    for declaration_id in sm.state_param_graph.declaration_positions.keys() {
        if !ids.contains(declaration_id.as_str()) {
            bail!(
                "state_machine validation: Declaration Graph position references missing State Param '{}'",
                declaration_id
            );
        }
    }
    for state in &sm.states {
        for (id, value) in &state.state_param_overrides {
            if !ids.contains(id.as_str()) {
                bail!(
                    "state_machine validation: State '{}' overrides missing State Param '{}'",
                    state.id,
                    id
                );
            }
            let declaration = sm
                .state_params
                .iter()
                .find(|declaration| declaration.id == *id)
                .expect("override declaration checked above");
            validate_canonical_state_value(
                &declaration.param_type,
                value,
                declaration.array_length,
                &format!("State '{}' override '{}'", state.id, id),
            )?;
        }
    }
    Ok(())
}

fn validate_canonical_state_value(
    port_type: &str,
    value: &serde_json::Value,
    array_length: Option<usize>,
    label: &str,
) -> Result<()> {
    if let Some(element_type) = port_type
        .strip_prefix("packed<")
        .and_then(|value| value.strip_suffix('>'))
    {
        let values = value
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("{label} must be an array"))?;
        let expected =
            array_length.ok_or_else(|| anyhow::anyhow!("{label} requires arrayLength"))?;
        if values.len() != expected {
            bail!("{label} must contain exactly {expected} values");
        }
        for item in values {
            validate_canonical_state_value(element_type, item, None, label)?;
        }
        return Ok(());
    }

    let numeric_tuple = |length: usize| -> bool {
        value.as_array().is_some_and(|values| {
            values.len() == length && values.iter().all(|value| value.as_f64().is_some())
        })
    };
    let valid = match port_type {
        "float" => value.as_f64().is_some(),
        "int" => {
            value.as_i64().is_some() || value.as_u64().is_some_and(|value| value <= i64::MAX as u64)
        }
        "bool" => value.is_boolean(),
        "vector2" => numeric_tuple(2),
        "vector3" => numeric_tuple(3),
        "vector4" | "color" | "normalizedBezierCurve" => numeric_tuple(4),
        "bezierCurve" => value.as_array().is_some_and(|points| {
            points.len() == 4
                && points.iter().all(|point| {
                    point.as_array().is_some_and(|values| {
                        values.len() == 2 && values.iter().all(|value| value.as_f64().is_some())
                    })
                })
        }),
        _ => false,
    };
    if !valid {
        bail!("{label} is not a canonical '{port_type}' value");
    }
    Ok(())
}

fn validate_source(sm: &StateMachine, source: &StateValueSource) -> Result<()> {
    match source {
        StateValueSource::StateParam { state_param_id } => {
            if state_param_id == "*"
                || sm
                    .state_params
                    .iter()
                    .any(|declaration| declaration.id == *state_param_id)
            {
                Ok(())
            } else {
                bail!(
                    "state_machine validation: missing State Param '{}'",
                    state_param_id
                )
            }
        }
        StateValueSource::FrameInput { frame_input_id } => {
            if matches!(
                frame_input_id.as_str(),
                "sceneElapsedTime"
                    | "localElapsedTime"
                    | "scene.size"
                    | "mouse.position"
                    | "mouse.position.x"
                    | "mouse.position.y"
            ) {
                Ok(())
            } else {
                bail!(
                    "state_machine validation: unknown frame input '{}'",
                    frame_input_id
                )
            }
        }
    }
}

fn state_param_port(sm: &StateMachine, state_param_id: &str) -> Result<GraphPort> {
    let declaration = sm
        .state_params
        .iter()
        .find(|declaration| declaration.id == state_param_id)
        .ok_or_else(|| {
            anyhow::anyhow!("state_machine validation: missing State Param '{state_param_id}'")
        })?;
    Ok(GraphPort {
        id: declaration.id.clone(),
        name: Some(declaration.name.clone()),
        port_type: Some(declaration.param_type.clone()),
        array_length: declaration.array_length,
        motion: None,
    })
}

fn source_port(sm: &StateMachine, source: &StateValueSource) -> Result<GraphPort> {
    validate_source(sm, source)?;
    match source {
        StateValueSource::StateParam { state_param_id } => state_param_port(sm, state_param_id),
        StateValueSource::FrameInput { frame_input_id } => Ok(GraphPort {
            id: frame_input_id.clone(),
            name: None,
            port_type: Some(
                if matches!(frame_input_id.as_str(), "scene.size" | "mouse.position") {
                    "vector2"
                } else {
                    "float"
                }
                .into(),
            ),
            array_length: None,
            motion: None,
        }),
    }
}

fn graph_input_port<'a>(
    nodes: &'a [GraphInnerNode],
    endpoint: &GraphEndpoint,
    label: &str,
) -> Result<&'a GraphPort> {
    let node = nodes
        .iter()
        .find(|node| node.id == endpoint.node_id)
        .ok_or_else(|| anyhow::anyhow!("{label} references missing node '{}'", endpoint.node_id))?;
    node.inputs
        .iter()
        .find(|port| port.id == endpoint.port_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{label} references missing input '{}.{}'",
                endpoint.node_id,
                endpoint.port_id
            )
        })
}

fn graph_output_port<'a>(
    nodes: &'a [GraphInnerNode],
    endpoint: &GraphEndpoint,
    label: &str,
) -> Result<&'a GraphPort> {
    let node = nodes
        .iter()
        .find(|node| node.id == endpoint.node_id)
        .ok_or_else(|| anyhow::anyhow!("{label} references missing node '{}'", endpoint.node_id))?;
    node.outputs
        .iter()
        .find(|port| port.id == endpoint.port_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{label} references missing output '{}.{}'",
                endpoint.node_id,
                endpoint.port_id
            )
        })
}

fn validate_port_type(
    source_type: Option<&str>,
    source_array_length: Option<usize>,
    target: &FormalRenderDeclaration,
    label: &str,
) -> Result<()> {
    if source_type != Some(target.port_type.as_str()) || source_array_length != target.array_length
    {
        bail!(
            "state_machine validation: {label} must exactly match type '{}' and fixed length {:?}",
            target.port_type,
            target.array_length
        );
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
        "BezierCurveInput" => &[("value", "bezierCurve")],
        "NormalizedBezierCurveInput" => &[("value", "normalizedBezierCurve")],
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
        if !matches!(
            state.state_type,
            AnimationStateType::AnimationState | AnimationStateType::AnyState
        ) {
            bail!(
                "state_machine validation: Derivation binding '{}' endpoint '{}' is neither an animationState nor the anyState fallback",
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
        let state_param_ids = sm
            .state_params
            .iter()
            .map(|declaration| declaration.id.as_str())
            .collect::<HashSet<_>>();
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

        #[derive(Clone)]
        struct TimingEdge {
            source: String,
            target: String,
            timing_node_id: String,
        }

        fn reducible(source: &str, target: &str, active: &[TimingEdge]) -> bool {
            #[derive(Clone)]
            struct Edge {
                source: String,
                target: String,
            }
            let mut edges = active
                .iter()
                .map(|edge| Edge {
                    source: edge.source.clone(),
                    target: edge.target.clone(),
                })
                .collect::<Vec<_>>();
            while edges.len() > 1 {
                let mut parallel = None;
                for left in 0..edges.len() {
                    let group = (left..edges.len())
                        .filter(|right| {
                            edges[*right].source == edges[left].source
                                && edges[*right].target == edges[left].target
                        })
                        .collect::<Vec<_>>();
                    if group.len() > 1 {
                        parallel = Some(group);
                        break;
                    }
                }
                if let Some(indices) = parallel {
                    let replacement = edges[indices[0]].clone();
                    for index in indices.into_iter().rev() {
                        edges.remove(index);
                    }
                    edges.push(replacement);
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
                    (incoming.len() == 1 && outgoing.len() == 1)
                        .then_some((incoming[0], outgoing[0]))
                });
                let Some((incoming, outgoing)) = serial else {
                    break;
                };
                let replacement = Edge {
                    source: edges[incoming].source.clone(),
                    target: edges[outgoing].target.clone(),
                };
                let mut remove = [incoming, outgoing];
                remove.sort_unstable();
                for index in remove.into_iter().rev() {
                    edges.remove(index);
                }
                edges.push(replacement);
            }
            edges.len() == 1 && edges[0].source == source && edges[0].target == target
        }

        let mut timing_sources: HashMap<&str, Vec<String>> = HashMap::new();
        let mut timing_targets: HashMap<&str, Vec<String>> = HashMap::new();
        for binding in &graph.input_bindings {
            validate_source(sm, &binding.source)?;
            if !node_ids.contains(binding.to.node_id.as_str()) {
                bail!(
                    "state_machine validation: transition motion graph '{}' has invalid input binding '{}'",
                    graph.id,
                    binding.source.id()
                );
            }
            if !node_by_id
                .get(binding.to.node_id.as_str())
                .is_some_and(|node| node.is_timing())
            {
                if matches!(
                    node_by_id.get(binding.to.node_id.as_str()),
                    Some(TransitionMotionNode::Waypoint { .. })
                ) {
                    bail!(
                        "state_machine validation: transition motion graph '{}' Waypoint '{}' must be reached from a Timing node",
                        graph.id,
                        binding.to.node_id
                    );
                }
                continue;
            }
            let Some(property) = binding.source.state_param_id() else {
                bail!(
                    "state_machine validation: transition motion graph '{}' timing node '{}' requires a State Param input",
                    graph.id,
                    binding.to.node_id
                );
            };
            if binding.to.port_id != "value" {
                bail!(
                    "state_machine validation: transition motion graph '{}' timing node '{}' input must use value",
                    graph.id,
                    binding.to.node_id
                );
            }
            timing_sources
                .entry(binding.to.node_id.as_str())
                .or_default()
                .push(format!("input:{property}"));
        }
        for connection in &graph.connections {
            let from = node_by_id.get(connection.from.node_id.as_str()).copied();
            let to = node_by_id.get(connection.to.node_id.as_str()).copied();
            if from.is_some_and(TransitionMotionNode::is_timing)
                && matches!(to, Some(TransitionMotionNode::Waypoint { .. }))
                && connection.from.port_id == "value"
                && connection.to.port_id == "in"
            {
                timing_targets
                    .entry(connection.from.node_id.as_str())
                    .or_default()
                    .push(format!("waypoint:{}", connection.to.node_id));
            } else if matches!(from, Some(TransitionMotionNode::Waypoint { .. }))
                && to.is_some_and(TransitionMotionNode::is_timing)
                && connection.from.port_id == "value"
                && connection.to.port_id == "value"
            {
                timing_sources
                    .entry(connection.to.node_id.as_str())
                    .or_default()
                    .push(format!("waypoint:{}", connection.from.node_id));
            } else if from.is_some_and(TransitionMotionNode::is_timing)
                || to.is_some_and(TransitionMotionNode::is_timing)
                || matches!(from, Some(TransitionMotionNode::Waypoint { .. }))
                || matches!(to, Some(TransitionMotionNode::Waypoint { .. }))
            {
                bail!(
                    "state_machine validation: transition motion graph '{}' connection '{}' must alternate Anchor -> Timing -> Anchor",
                    graph.id,
                    connection.id
                );
            }
        }
        for binding in &graph.output_bindings {
            if (binding.state_param_id != "*"
                && !state_param_ids.contains(binding.state_param_id.as_str()))
                || !node_ids.contains(binding.from.node_id.as_str())
            {
                bail!(
                    "state_machine validation: transition motion graph '{}' has invalid output binding '{}'",
                    graph.id,
                    binding.state_param_id
                );
            }
            if !node_by_id
                .get(binding.from.node_id.as_str())
                .is_some_and(|node| node.is_timing())
            {
                bail!(
                    "state_machine validation: transition motion graph '{}' output '{}' must be driven by a timing node",
                    graph.id,
                    binding.state_param_id
                );
            }
            if binding.from.port_id != "value" {
                bail!(
                    "state_machine validation: transition motion graph '{}' output '{}' must use Timing value",
                    graph.id,
                    binding.state_param_id
                );
            }
            timing_targets
                .entry(binding.from.node_id.as_str())
                .or_default()
                .push(format!("output:{}", binding.state_param_id));
        }

        let mut timing_edges = Vec::new();
        for node in graph.nodes.iter().filter(|node| node.is_timing()) {
            let sources = timing_sources.get(node.id()).cloned().unwrap_or_default();
            let targets = timing_targets.get(node.id()).cloned().unwrap_or_default();
            if sources.len() > 1 || targets.len() > 1 {
                bail!(
                    "state_machine validation: transition motion graph '{}' timing node '{}' must have one input anchor and one output anchor",
                    graph.id,
                    node.id()
                );
            }
            if sources.len() == 1 && targets.len() == 1 {
                timing_edges.push(TimingEdge {
                    source: sources[0].clone(),
                    target: targets[0].clone(),
                    timing_node_id: node.id().to_string(),
                });
            }
        }

        let mut properties = timing_edges
            .iter()
            .filter_map(|edge| edge.source.strip_prefix("input:"))
            .map(str::to_string)
            .collect::<Vec<_>>();
        properties.sort();
        properties.dedup();
        let mut covered_outputs = HashSet::new();
        let mut timing_memberships: HashMap<String, HashSet<String>> = HashMap::new();
        let mut waypoint_memberships: HashMap<String, HashSet<String>> = HashMap::new();
        for property in properties {
            let source = format!("input:{property}");
            let target = format!("output:{property}");
            let mut forward = HashSet::from([source.clone()]);
            loop {
                let before = forward.len();
                for edge in &timing_edges {
                    if forward.contains(&edge.source) {
                        forward.insert(edge.target.clone());
                    }
                }
                if forward.len() == before {
                    break;
                }
            }
            if let Some(other) = forward.iter().find_map(|anchor| {
                anchor
                    .strip_prefix("output:")
                    .filter(|output| *output != property)
            }) {
                bail!(
                    "state_machine validation: transition motion graph '{}' crosses property '{}' to '{}'",
                    graph.id,
                    property,
                    other
                );
            }
            let mut backward = HashSet::from([target.clone()]);
            loop {
                let before = backward.len();
                for edge in &timing_edges {
                    if backward.contains(&edge.target) {
                        backward.insert(edge.source.clone());
                    }
                }
                if backward.len() == before {
                    break;
                }
            }
            let active = timing_edges
                .iter()
                .filter(|edge| forward.contains(&edge.source) && backward.contains(&edge.target))
                .cloned()
                .collect::<Vec<_>>();
            if active.is_empty() {
                continue;
            }
            if !reducible(&source, &target, &active) {
                bail!(
                    "state_machine validation: transition motion graph '{}' property '{}' is not a two-terminal series-parallel DAG",
                    graph.id,
                    property
                );
            }
            covered_outputs.insert(property.clone());
            for edge in active {
                timing_memberships
                    .entry(edge.timing_node_id)
                    .or_default()
                    .insert(property.clone());
                for anchor in [&edge.source, &edge.target] {
                    if let Some(node_id) = anchor.strip_prefix("waypoint:") {
                        waypoint_memberships
                            .entry(node_id.to_string())
                            .or_default()
                            .insert(property.clone());
                    }
                }
            }
        }
        for (node_id, memberships) in timing_memberships.iter().chain(&waypoint_memberships) {
            if memberships.len() > 1 {
                bail!(
                    "state_machine validation: transition motion graph '{}' node '{}' crosses properties",
                    graph.id,
                    node_id
                );
            }
        }
        for (node_id, memberships) in &waypoint_memberships {
            let Some(TransitionMotionNode::Waypoint {
                port_type,
                value,
                array_length,
                ..
            }) = node_by_id.get(node_id.as_str()).copied()
            else {
                continue;
            };
            if !matches!(
                port_type.as_str(),
                "float"
                    | "int"
                    | "vector2"
                    | "vector3"
                    | "vector4"
                    | "color"
                    | "packed<float>"
                    | "packed<int>"
                    | "packed<vector2>"
                    | "packed<vector3>"
                    | "packed<vector4>"
                    | "packed<color>"
                    | "bezierCurve"
                    | "normalizedBezierCurve"
            ) {
                bail!(
                    "state_machine validation: transition motion graph '{}' Waypoint '{}' has non-numeric type '{}'",
                    graph.id,
                    node_id,
                    port_type
                );
            }
            validate_canonical_state_value(
                port_type,
                value,
                *array_length,
                &format!(
                    "Transition motion graph '{}' Waypoint '{}' value",
                    graph.id, node_id
                ),
            )?;
            for property in memberships {
                if property == "*" {
                    bail!(
                        "state_machine validation: transition motion graph '{}' wildcard path cannot contain Waypoint '{}'",
                        graph.id,
                        node_id
                    );
                }
                let declaration = sm
                    .state_params
                    .iter()
                    .find(|declaration| declaration.id == *property)
                    .ok_or_else(|| anyhow::anyhow!("missing State Param '{property}'"))?;
                if declaration.param_type != *port_type || declaration.array_length != *array_length
                {
                    bail!(
                        "state_machine validation: transition motion graph '{}' Waypoint '{}' does not exactly match State Param '{}'",
                        graph.id,
                        node_id,
                        property
                    );
                }
            }
        }
        for passthrough in &graph.passthrough_bindings {
            if passthrough.state_param_id != "*"
                && !state_param_ids.contains(passthrough.state_param_id.as_str())
            {
                bail!(
                    "state_machine validation: transition motion graph '{}' has invalid passthrough",
                    graph.id
                );
            }
            if !covered_outputs.insert(passthrough.state_param_id.clone()) {
                bail!(
                    "state_machine validation: transition motion graph '{}' has conflicting outputs for '{}'",
                    graph.id,
                    passthrough.state_param_id
                );
            }
        }

        match graph.condition_binding.as_ref() {
            None => {}
            Some(TransitionConditionBinding::Input { input }) => {
                validate_source(sm, input)?;
                let valid = input.state_param_id().is_some_and(|id| {
                    sm.state_params
                        .iter()
                        .any(|declaration| declaration.id == id && declaration.param_type == "bool")
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
                TransitionMotionNode::Waypoint {
                    id,
                    port_type,
                    value,
                    array_length,
                    ..
                } => {
                    if !matches!(
                        port_type.as_str(),
                        "float"
                            | "int"
                            | "vector2"
                            | "vector3"
                            | "vector4"
                            | "color"
                            | "packed<float>"
                            | "packed<int>"
                            | "packed<vector2>"
                            | "packed<vector3>"
                            | "packed<vector4>"
                            | "packed<color>"
                            | "bezierCurve"
                            | "normalizedBezierCurve"
                    ) {
                        bail!(
                            "state_machine validation: transition motion graph '{}' Waypoint '{}' has non-numeric type '{}'",
                            graph.id,
                            id,
                            port_type
                        );
                    }
                    validate_canonical_state_value(
                        port_type,
                        value,
                        *array_length,
                        &format!(
                            "Transition motion graph '{}' Waypoint '{}' value",
                            graph.id, id
                        ),
                    )?;
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
            let label = format!("State '{}' Mutation", state.id);
            validate_graph_core(&label, &graph.nodes, &graph.connections, true)?;
            let mut input_targets = HashSet::new();
            for binding in &graph.input_bindings {
                let source = source_port(sm, &binding.source)?;
                let target = graph_input_port(&graph.nodes, &binding.to, &label)?;
                validate_port_compatibility(&label, &source, target)?;
                if !input_targets.insert((&binding.to.node_id, &binding.to.port_id)) {
                    bail!(
                        "{label} input '{}.{}' has multiple writers",
                        binding.to.node_id,
                        binding.to.port_id
                    );
                }
            }
            let mut output_ids = HashSet::new();
            let mut motion_sources = HashMap::new();
            for binding in &graph.output_bindings {
                let target = state_param_port(sm, &binding.state_param_id)?;
                let source = graph_output_port(&graph.nodes, &binding.from, &label)?;
                validate_port_compatibility(&label, source, &target)?;
                if source.motion != Some(true) {
                    bail!(
                        "{label} State Param '{}' must be driven by Motion<T>",
                        binding.state_param_id
                    );
                }
                if !output_ids.insert(binding.state_param_id.as_str()) {
                    bail!(
                        "{label} State Param '{}' has multiple writers",
                        binding.state_param_id
                    );
                }
                *motion_sources
                    .entry((binding.from.node_id.as_str(), binding.from.port_id.as_str()))
                    .or_insert(0usize) += 1;
            }
            for node in &graph.nodes {
                for output in node.outputs.iter().filter(|port| port.motion == Some(true)) {
                    let count = motion_sources
                        .get(&(node.id.as_str(), output.id.as_str()))
                        .copied()
                        .unwrap_or_default();
                    if count != 1 {
                        bail!(
                            "{label} Motion output '{}.{}' must bind exactly once to a State Param",
                            node.id,
                            output.id
                        );
                    }
                }
            }
        }
    }
    for derivation in &sm.derivations {
        let label = format!("Derivation '{}'", derivation.id);
        validate_graph_core(&label, &derivation.nodes, &derivation.connections, false)?;
        let mut input_targets = HashSet::new();
        for binding in &derivation.input_bindings {
            let source = source_port(sm, &binding.source)?;
            let target = graph_input_port(&derivation.nodes, &binding.to, &label)?;
            validate_port_compatibility(&label, &source, target)?;
            if !input_targets.insert((&binding.to.node_id, &binding.to.port_id)) {
                bail!(
                    "{label} input '{}.{}' has multiple writers",
                    binding.to.node_id,
                    binding.to.port_id
                );
            }
        }
        let mut uniform_ids = HashSet::new();
        for binding in &derivation.output_bindings {
            let source = graph_output_port(&derivation.nodes, &binding.from, &label)?;
            if source.motion == Some(true) {
                bail!("{label} cannot bind Motion<T> to a GPU uniform");
            }
            let uniform_id = binding.uniform.id();
            if !uniform_ids.insert(uniform_id.clone()) {
                bail!("{label} GPU uniform '{uniform_id}' has multiple writers");
            }
        }
        for binding in &derivation.passthrough_bindings {
            validate_source(sm, &binding.source)?;
            let uniform_id = binding.uniform.id();
            if !uniform_ids.insert(uniform_id.clone()) {
                bail!("{label} GPU uniform '{uniform_id}' has multiple writers");
            }
        }
    }
    Ok(())
}

fn validate_graph_core(
    label: &str,
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

fn validate_port_compatibility(label: &str, source: &GraphPort, target: &GraphPort) -> Result<()> {
    let source_type = source.port_type.as_deref().unwrap_or("any");
    let target_type = target.port_type.as_deref().unwrap_or("any");
    let scheme = schema::load_default_scheme()?;
    let compatible = schema::port_types_compatible(
        &scheme,
        &schema::PortTypeSpec::One(source_type.to_string()),
        &schema::PortTypeSpec::One(target_type.to_string()),
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
            version: "6.0".into(),
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
            state_params: vec![],
            state_param_graph: Default::default(),
            node_widths: Default::default(),
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
            motion_graphs: vec![instant_motion_graph()],
            initial_state_id: Some("entry".into()),
            viewport: None,
        }
    }

    fn float_state_param(id: &str, name: &str) -> StateParamDeclaration {
        StateParamDeclaration {
            id: id.into(),
            name: name.into(),
            param_type: "float".into(),
            default_value: serde_json::json!(0.0),
            array_length: None,
        }
    }

    #[test]
    fn state_param_graph_requires_a_position_for_every_declaration() {
        let mut sm = minimal_sm();
        sm.state_params
            .push(float_state_param("pointer_x", "Pointer X"));

        let error = validate_state_params(&sm)
            .expect_err("a declaration without graph position must fail")
            .to_string();
        assert!(error.contains("missing its Declaration Graph position"));
    }

    #[test]
    fn state_param_graph_rejects_unknown_declaration_positions() {
        let mut sm = minimal_sm();
        sm.state_param_graph
            .declaration_positions
            .insert("removed".into(), Position::default());

        let error = validate_state_params(&sm)
            .expect_err("an unknown declaration graph position must fail")
            .to_string();
        assert!(error.contains("references missing State Param 'removed'"));
    }

    #[test]
    fn state_param_graph_rejects_duplicate_paramkey_names() {
        let mut sm = minimal_sm();
        sm.state_params.extend([
            float_state_param("pointer_x", "Pointer"),
            float_state_param("pointer_y", "Pointer"),
        ]);
        sm.state_param_graph
            .declaration_positions
            .insert("pointer_x".into(), Position::default());
        sm.state_param_graph
            .declaration_positions
            .insert("pointer_y".into(), Position { x: 280.0, y: 0.0 });

        let error = validate_state_params(&sm)
            .expect_err("duplicate ParamKey names must fail")
            .to_string();
        assert!(error.contains("duplicate State Param name 'Pointer'"));
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
            state_param_overrides: Default::default(),
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
            state_param_overrides: Default::default(),
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
                node_widths: HashMap::new(),
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
        derivation.nodes.push(GraphInnerNode {
            id: "value".into(),
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
        });
        derivation.output_bindings.push(DerivationOutputBinding {
            uniform: GpuUniformRef {
                node_id: "consumer".into(),
                param_id: "opacity".into(),
            },
            from: GraphEndpoint {
                node_id: "value".into(),
                port_id: "value".into(),
            },
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
        assert!(error.contains("missing GPU uniform 'consumer:opacity'"));
    }

    #[test]
    fn scene_declarations_enforce_packed_array_length() {
        let mut sm = minimal_sm();
        sm.states.push(regular_state("state"));
        sm.states.push(derivation_node("derive_node", "derive"));
        let mut derivation = empty_derivation("derive");
        derivation.nodes.push(GraphInnerNode {
            id: "pack".into(),
            node_type: GraphInnerNodeType::PackArray,
            params: HashMap::new(),
            inputs: vec![],
            outputs: vec![GraphPort {
                id: "packed".into(),
                name: Some("Packed".into()),
                port_type: Some("packed<color>".into()),
                array_length: Some(1),
                motion: None,
            }],
        });
        derivation.output_bindings.push(DerivationOutputBinding {
            uniform: GpuUniformRef {
                node_id: "colors".into(),
                param_id: "value".into(),
            },
            from: GraphEndpoint {
                node_id: "pack".into(),
                port_id: "packed".into(),
            },
        });
        sm.derivations.push(derivation);
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
        sm.state_params.extend([
            StateParamDeclaration {
                id: "Node:x".into(),
                name: "X".into(),
                param_type: "float".into(),
                default_value: serde_json::json!(0.0),
                array_length: None,
            },
            StateParamDeclaration {
                id: "Node:y".into(),
                name: "Y".into(),
                param_type: "float".into(),
                default_value: serde_json::json!(0.0),
                array_length: None,
            },
        ]);
        sm.state_param_graph
            .declaration_positions
            .insert("Node:x".into(), Position::default());
        sm.state_param_graph
            .declaration_positions
            .insert("Node:y".into(), Position { x: 280.0, y: 0.0 });
        let graph = &mut sm.motion_graphs[0];
        graph.input_bindings[0].source = StateValueSource::StateParam {
            state_param_id: "Node:x".into(),
        };
        graph.output_bindings[0].state_param_id = "Node:y".into();

        let err = validate(&sm).unwrap_err().to_string();
        assert!(err.contains("crosses property"), "{err}");
    }

    fn serial_waypoint_sm() -> StateMachine {
        let mut sm = minimal_sm();
        sm.state_params
            .push(float_state_param("Blur:value", "Blur"));
        sm.state_param_graph
            .declaration_positions
            .insert("Blur:value".into(), Position::default());
        let graph = &mut sm.motion_graphs[0];
        graph.nodes = vec![
            TransitionMotionNode::Instant {
                id: "first".into(),
                position: Position::default(),
                label: None,
            },
            TransitionMotionNode::Waypoint {
                id: "peak".into(),
                position: Position::default(),
                label: None,
                port_type: "float".into(),
                value: serde_json::json!(64.0),
                array_length: None,
            },
            TransitionMotionNode::Instant {
                id: "second".into(),
                position: Position::default(),
                label: None,
            },
        ];
        graph.connections = vec![
            GraphConnection {
                id: "to_peak".into(),
                from: GraphEndpoint {
                    node_id: "first".into(),
                    port_id: "value".into(),
                },
                to: GraphEndpoint {
                    node_id: "peak".into(),
                    port_id: "in".into(),
                },
            },
            GraphConnection {
                id: "from_peak".into(),
                from: GraphEndpoint {
                    node_id: "peak".into(),
                    port_id: "value".into(),
                },
                to: GraphEndpoint {
                    node_id: "second".into(),
                    port_id: "value".into(),
                },
            },
        ];
        graph.input_bindings = vec![TransitionMotionInputBinding {
            source: StateValueSource::StateParam {
                state_param_id: "Blur:value".into(),
            },
            to: GraphEndpoint {
                node_id: "first".into(),
                port_id: "value".into(),
            },
        }];
        graph.output_bindings = vec![TransitionMotionOutputBinding {
            state_param_id: "Blur:value".into(),
            from: GraphEndpoint {
                node_id: "second".into(),
                port_id: "value".into(),
            },
        }];
        sm
    }

    #[test]
    fn motion_graph_accepts_a_typed_serial_waypoint_plan() {
        validate_motion_graphs(&serial_waypoint_sm()).expect("serial plan should validate");
    }

    #[test]
    fn motion_graph_rejects_wildcard_and_boolean_waypoints() {
        let mut wildcard = serial_waypoint_sm();
        let graph = &mut wildcard.motion_graphs[0];
        graph.input_bindings[0].source = StateValueSource::StateParam {
            state_param_id: "*".into(),
        };
        graph.output_bindings[0].state_param_id = "*".into();
        let error = validate_motion_graphs(&wildcard).unwrap_err().to_string();
        assert!(
            error.contains("wildcard path cannot contain Waypoint"),
            "{error}"
        );

        let mut boolean = serial_waypoint_sm();
        let TransitionMotionNode::Waypoint {
            port_type, value, ..
        } = &mut boolean.motion_graphs[0].nodes[1]
        else {
            unreachable!()
        };
        *port_type = "bool".into();
        *value = serde_json::json!(false);
        let error = validate_motion_graphs(&boolean).unwrap_err().to_string();
        assert!(error.contains("non-numeric type 'bool'"), "{error}");
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
            state_param_overrides: Default::default(),
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
            state_param_overrides: Default::default(),
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
            state_param_overrides: Default::default(),
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
            state_param_overrides: Default::default(),
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
    fn any_state_can_provide_the_fallback_derivation() {
        let mut sm = minimal_sm();
        sm.states
            .push(derivation_node("derivation_node", "derivation"));
        sm.derivations.push(empty_derivation("derivation"));
        sm.derivation_bindings.push(DerivationStateBinding {
            id: "binding_fallback".into(),
            state_id: "any".into(),
            derivation_node_id: "derivation_node".into(),
        });

        validate(&sm).expect("the Any State may own the fallback Derivation binding");
    }

    #[test]
    fn entry_and_exit_states_cannot_bind_derivations() {
        for state_id in ["entry", "exit"] {
            let mut sm = minimal_sm();
            sm.states
                .push(derivation_node("derivation_node", "derivation"));
            sm.derivations.push(empty_derivation("derivation"));
            sm.derivation_bindings.push(DerivationStateBinding {
                id: format!("binding_{state_id}"),
                state_id: state_id.into(),
                derivation_node_id: "derivation_node".into(),
            });

            let error = validate(&sm).unwrap_err().to_string();
            assert!(error.contains("anyState fallback"), "{error}");
        }
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
                uniform: GpuUniformRef {
                    node_id: "X".into(),
                    param_id: "value".into(),
                },
                from: GraphEndpoint {
                    node_id: "n".into(),
                    port_id: "value".into(),
                },
            }],
            passthrough_bindings: vec![DerivationPassthroughBinding {
                source: StateValueSource::FrameInput {
                    frame_input_id: "sceneElapsedTime".into(),
                },
                uniform: GpuUniformRef {
                    node_id: "X".into(),
                    param_id: "value".into(),
                },
            }],
            layout: None,
            viewport: None,
        };
        sm.states.push(AnimationState {
            id: "s1".into(),
            name: "S1".into(),
            position: None,
            state_param_overrides: Default::default(),
            state_type: AnimationStateType::DerivationNode,
            mutation_graph: None,
            derivation_id: Some("m1".into()),
        });
        sm.derivations.push(derivation);
        let err = validate(&sm).unwrap_err().to_string();
        assert!(
            err.contains("GPU uniform 'X:value' has multiple writers"),
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
            source: StateValueSource::StateParam {
                state_param_id: "FloatInput:value".into(),
            },
            to: GraphEndpoint {
                node_id: "function".into(),
                port_id: "value".into(),
            },
        });

        let mut sm = minimal_sm();
        sm.state_params.push(StateParamDeclaration {
            id: "FloatInput:value".into(),
            name: "Value".into(),
            param_type: "float".into(),
            default_value: serde_json::json!(0.0),
            array_length: None,
        });
        sm.state_param_graph
            .declaration_positions
            .insert("FloatInput:value".into(), Position::default());
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
            state_param_id: "FloatInput:value".into(),
            from: GraphEndpoint {
                node_id: "first".into(),
                port_id: "value".into(),
            },
        });

        let mut sm = minimal_sm();
        sm.state_params.push(StateParamDeclaration {
            id: "FloatInput:value".into(),
            name: "Value".into(),
            param_type: "float".into(),
            default_value: serde_json::json!(0.0),
            array_length: None,
        });
        sm.state_param_graph
            .declaration_positions
            .insert("FloatInput:value".into(), Position::default());
        let mut state = regular_state("state");
        state.mutation_graph = Some(mutation);
        sm.states.push(state);
        validate(&sm).expect("Q may flow downstream and bind to one State Output identity");
    }
}
