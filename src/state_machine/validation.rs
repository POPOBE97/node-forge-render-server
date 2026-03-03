//! Structural and semantic validation for `StateMachine` definitions.
//!
//! All checks are intentionally fail-fast: on the first error encountered
//! an `Err` is returned with a human-readable diagnostic that includes
//! relevant IDs (stateId / transitionId / mutationId).

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use super::types::*;

/// Maximum allowed nesting depth for compound transition conditions.
const MAX_CONDITION_DEPTH: usize = 8;

/// Validate a `StateMachine` definition.
///
/// Returns `Ok(())` when the definition is structurally sound, or an `Err`
/// with an actionable diagnostic on the first violation encountered.
pub fn validate(sm: &StateMachine) -> Result<()> {
    validate_state_ids(sm)?;
    validate_builtin_states(sm)?;
    validate_mutation_ids(sm)?;
    validate_mutation_node_refs(sm)?;
    validate_transition_endpoints(sm)?;
    validate_transition_direction_constraints(sm)?;
    validate_transition_conditions(sm)?;
    validate_mutation_internals(sm)?;
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

// ── MutationNode → MutationDefinition references ──────────────────────────

fn validate_mutation_node_refs(sm: &StateMachine) -> Result<()> {
    let mutation_ids: HashSet<&str> = sm.mutations.iter().map(|m| m.id.as_str()).collect();

    for s in &sm.states {
        if s.resolved_type() != AnimationStateType::MutationNode {
            continue;
        }
        match s.mutation_id.as_deref() {
            None => bail!(
                "state_machine validation: mutationNode '{}' is missing mutationId",
                s.id
            ),
            Some(mid) if !mutation_ids.contains(mid) => bail!(
                "state_machine validation: mutationNode '{}' references missing mutation '{mid}'",
                s.id
            ),
            _ => {}
        }
    }

    Ok(())
}

// ── Transition endpoint references ─────────────────────────────────────────

fn validate_transition_endpoints(sm: &StateMachine) -> Result<()> {
    let state_ids: HashSet<&str> = sm.states.iter().map(|s| s.id.as_str()).collect();

    for t in &sm.transitions {
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

// ── Condition shape / depth ────────────────────────────────────────────────

fn validate_transition_conditions(sm: &StateMachine) -> Result<()> {
    for t in &sm.transitions {
        if let Some(cond) = t.condition.as_ref() {
            validate_condition(cond, 1, &t.id)?;
        }
    }
    Ok(())
}

fn validate_condition(cond: &TransitionCondition, depth: usize, transition_id: &str) -> Result<()> {
    if depth > MAX_CONDITION_DEPTH {
        bail!(
            "state_machine validation: transition '{}' condition nesting exceeds max depth {MAX_CONDITION_DEPTH}",
            transition_id
        );
    }
    if let TransitionCondition::Compound { conditions, .. } = cond {
        if conditions.len() < 2 {
            bail!(
                "state_machine validation: transition '{}' compound condition must have at least 2 sub-conditions, found {}",
                transition_id,
                conditions.len()
            );
        }
        for sub in conditions {
            validate_condition(sub, depth + 1, transition_id)?;
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
    fn minimal_valid() {
        assert!(validate(&minimal_sm()).is_ok());
    }

    #[test]
    fn duplicate_state_id() {
        let mut sm = minimal_sm();
        sm.states.push(AnimationState {
            id: "entry".into(),
            name: "Dup".into(),
            position: None,
            parameter_overrides: Default::default(),
            state_type: Some(AnimationStateType::AnimationState),
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
            state_type: Some(AnimationStateType::MutationNode),
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
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "exit".into(),
            target: "s1".into(),
            condition: None,
            duration: 0.3,
            easing: EasingKind::Linear,
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
            state_type: Some(AnimationStateType::AnimationState),
            mutation_id: None,
        });
        sm.transitions.push(AnimationTransition {
            id: "t1".into(),
            source: "s1".into(),
            target: "entry".into(),
            condition: None,
            duration: 0.3,
            easing: EasingKind::Linear,
        });
        let err = validate(&sm).unwrap_err().to_string();
        assert!(err.contains("entryState"), "{err}");
    }

    #[test]
    fn compound_condition_min_children() {
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
            condition: Some(TransitionCondition::Compound {
                op: CompoundOp::And,
                conditions: vec![TransitionCondition::Trigger {
                    param_id: "p".into(),
                }],
            }),
            duration: 0.3,
            easing: EasingKind::Linear,
        });
        let err = validate(&sm).unwrap_err().to_string();
        assert!(err.contains("at least 2"), "{err}");
    }
}
