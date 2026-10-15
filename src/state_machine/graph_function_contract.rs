use std::collections::HashMap;
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use super::types::{GraphInnerNode, GraphInnerNodeType, GraphPort};
use crate::dsl::SceneDSL;
pub(crate) const GRAPH_FUNCTION_ABI_VERSION: u32 = 10;
pub(crate) fn resource_key(scope: &str, node_id: &str) -> String { format!("{scope}/{node_id}") }

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReflectedPort {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub port_type: String,
    #[serde(default)]
    pub array_length: Option<usize>,
    #[serde(default)]
    pub motion: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FunctionResource {
    pub scope: String,
    pub node_id: String,
    pub kind: String,
    pub language: String,
    pub source: String,
    pub compiled_java_script: String,
    pub source_hash: String,
    pub abi_version: u32,
    #[serde(default)]
    pub inputs: Vec<ReflectedPort>,
    #[serde(default)]
    pub outputs: Vec<ReflectedPort>,
}

impl FunctionResource {
    pub fn validate_stored_artifact(&self) -> Result<()> {
        let (scope_kind, scope_id) = self
            .scope
            .split_once(':')
            .filter(|(_, id)| !id.is_empty())
            .ok_or_else(|| anyhow!("invalid Graph Function scope '{}'", self.scope))?;
        let expected_kind = match scope_kind {
            "state" => "mutation",
            "derivation" => "derivation",
            _ => {
                bail!("unsupported Graph Function scope '{}'", self.scope);
            }
        };
        if self.kind != expected_kind {
            bail!(
                "Graph Function kind '{}' does not match scope '{}'",
                self.kind,
                self.scope
            );
        }
        let _ = scope_id;
        if self.language != "typescript" {
            bail!("unsupported Graph Function language '{}'", self.language);
        }
        if self.abi_version != GRAPH_FUNCTION_ABI_VERSION {
            bail!(
                "unsupported Graph Function ABI {} (expected {})",
                self.abi_version,
                GRAPH_FUNCTION_ABI_VERSION
            );
        }
        let actual_hash = format!("{:x}", Sha256::digest(self.source.as_bytes()));
        if actual_hash != self.source_hash {
            bail!(
                "Graph Function '{}/{}' source hash is stale",
                self.scope,
                self.node_id
            );
        }
        if self.compiled_java_script.trim().is_empty() {
            bail!("Graph Function compiled JavaScript is empty");
        }
        Ok(())
    }
}

pub fn validate_function_resources(scene: &SceneDSL, resources: &[FunctionResource]) -> Result<()> {
    struct ExpectedFunction<'a> {
        node: &'a GraphInnerNode,
        node_type: GraphInnerNodeType,
    }

    let mut expected = HashMap::<String, ExpectedFunction<'_>>::new();
    if let Some(state_machine) = &scene.state_machine {
        for state in &state_machine.states {
            let Some(graph) = &state.mutation_graph else {
                continue;
            };
            let scope = format!("state:{}", state.id);
            for node in &graph.nodes {
                if node.node_type != GraphInnerNodeType::MutationFunction {
                    continue;
                }
                let key = resource_key(&scope, &node.id);
                if expected
                    .insert(
                        key.clone(),
                        ExpectedFunction {
                            node,
                            node_type: GraphInnerNodeType::MutationFunction,
                        },
                    )
                    .is_some()
                {
                    bail!("duplicate Graph Function node '{key}'");
                }
            }
        }
        for derivation in &state_machine.derivations {
            let scope = format!("derivation:{}", derivation.id);
            for node in &derivation.nodes {
                if node.node_type != GraphInnerNodeType::DerivationFunction {
                    continue;
                }
                let key = resource_key(&scope, &node.id);
                if expected
                    .insert(
                        key.clone(),
                        ExpectedFunction {
                            node,
                            node_type: GraphInnerNodeType::DerivationFunction,
                        },
                    )
                    .is_some()
                {
                    bail!("duplicate Graph Function node '{key}'");
                }
            }
        }
    }

    let mut seen = std::collections::HashSet::new();
    for resource in resources {
        resource.validate_stored_artifact()?;
        let key = resource_key(&resource.scope, &resource.node_id);
        if !seen.insert(key.clone()) {
            bail!("duplicate Graph Function resource '{key}'");
        }
        let function = expected
            .remove(&key)
            .ok_or_else(|| anyhow!("Graph Function resource '{key}' has no matching graph node"))?;
        let expected_kind = match function.node_type {
            GraphInnerNodeType::MutationFunction => "mutation",
            GraphInnerNodeType::DerivationFunction => "derivation",
            _ => unreachable!("expected map contains only Function nodes"),
        };
        if resource.kind != expected_kind {
            bail!(
                "Graph Function resource '{key}' kind '{}' does not match node type",
                resource.kind
            );
        }
        validate_reflected_ports(&key, "input", &function.node.inputs, &resource.inputs)?;
        validate_reflected_ports(&key, "output", &function.node.outputs, &resource.outputs)?;
    }

    if let Some((key, _)) = expected.into_iter().next() {
        bail!("Graph Function node '{key}' has no stored resource");
    }
    Ok(())
}

fn validate_reflected_ports(
    function_key: &str,
    direction: &str,
    graph_ports: &[GraphPort],
    reflected_ports: &[ReflectedPort],
) -> Result<()> {
    if graph_ports.len() != reflected_ports.len() {
        bail!(
            "Graph Function '{function_key}' {direction} reflection count does not match its node"
        );
    }
    for (index, (graph, reflected)) in graph_ports.iter().zip(reflected_ports).enumerate() {
        if graph.id != reflected.id
            || graph.name.as_deref() != Some(reflected.name.as_str())
            || graph.port_type.as_deref() != Some(reflected.port_type.as_str())
            || graph.array_length != reflected.array_length
            || graph.motion != reflected.motion
        {
            bail!(
                "Graph Function '{function_key}' {direction} reflection at index {index} does not match its node"
            );
        }
    }
    Ok(())
}

