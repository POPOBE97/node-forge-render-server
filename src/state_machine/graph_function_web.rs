use std::{cell::RefCell, collections::HashMap, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use js_sys::{Array, Function, Object, Reflect};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wasm_bindgen::{JsCast, JsValue};

use super::{
    graph::GraphValue,
    motion::{MutationPlan, MutationTiming},
    types::{GraphInnerNodeType, StateMachine},
};
use crate::dsl::SceneDSL;

const GRAPH_FUNCTION_ABI_VERSION: u32 = 10;

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
            _ => bail!("unsupported Graph Function scope '{}'", self.scope),
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

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionOutput {
    Value(GraphValue),
    Motion(MutationPlan),
}

struct PreparedFunction {
    entry: Function,
    motion_kind: JsValue,
    resource: FunctionResource,
}

thread_local! {
    static RESOURCES: RefCell<HashMap<String, FunctionResource>> = RefCell::new(HashMap::new());
    static PREPARED: RefCell<HashMap<String, PreparedFunction>> = RefCell::new(HashMap::new());
}

fn resource_key(scope: &str, node_id: &str) -> String {
    format!("{scope}/{node_id}")
}

pub fn install_document_functions(
    functions: impl IntoIterator<Item = FunctionResource>,
) -> Result<()> {
    let mut next = HashMap::new();
    for function in functions {
        function.validate_stored_artifact()?;
        let key = resource_key(&function.scope, &function.node_id);
        if next.insert(key.clone(), function).is_some() {
            bail!("duplicate Graph Function resource '{key}'");
        }
    }
    RESOURCES.with(|resources| *resources.borrow_mut() = next);
    PREPARED.with(|prepared| prepared.borrow_mut().clear());
    Ok(())
}

pub fn clear_document_functions() {
    RESOURCES.with(|resources| resources.borrow_mut().clear());
    PREPARED.with(|prepared| prepared.borrow_mut().clear());
}

pub fn installed_document_functions() -> Vec<FunctionResource> {
    RESOURCES.with(|resources| resources.borrow().values().cloned().collect())
}

pub fn validate_function_resources(scene: &SceneDSL, resources: &[FunctionResource]) -> Result<()> {
    let mut expected = HashMap::<String, GraphInnerNodeType>::new();
    if let Some(machine) = &scene.state_machine {
        for state in &machine.states {
            if let Some(graph) = &state.mutation_graph {
                for node in &graph.nodes {
                    if node.node_type == GraphInnerNodeType::MutationFunction {
                        expected.insert(
                            resource_key(&format!("state:{}", state.id), &node.id),
                            node.node_type,
                        );
                    }
                }
            }
        }
        for graph in &machine.derivations {
            for node in &graph.nodes {
                if node.node_type == GraphInnerNodeType::DerivationFunction {
                    expected.insert(
                        resource_key(&format!("derivation:{}", graph.id), &node.id),
                        node.node_type,
                    );
                }
            }
        }
    }
    let mut actual = HashMap::new();
    for resource in resources {
        resource.validate_stored_artifact()?;
        let key = resource_key(&resource.scope, &resource.node_id);
        if actual.insert(key.clone(), resource).is_some() {
            bail!("duplicate Graph Function resource '{key}'");
        }
        if !expected.contains_key(&key) {
            bail!("Graph Function resource '{key}' has no matching graph node");
        }
    }
    for key in expected.keys() {
        if !actual.contains_key(key) {
            bail!("Graph Function node '{key}' has no Function resource");
        }
    }
    Ok(())
}

fn js_error(operation: &str, value: JsValue) -> anyhow::Error {
    let message = value
        .as_string()
        .or_else(|| {
            js_sys::JSON::stringify(&value)
                .ok()
                .and_then(|text| text.as_string())
        })
        .unwrap_or_else(|| format!("{value:?}"));
    anyhow!("Graph Function {operation}: {message}")
}

pub fn prepare(graph_scope: &str, node_id: &str) -> Result<()> {
    let key = resource_key(graph_scope, node_id);
    if PREPARED.with(|prepared| prepared.borrow().contains_key(&key)) {
        return Ok(());
    }
    let resource = RESOURCES
        .with(|resources| resources.borrow().get(&key).cloned())
        .ok_or_else(|| anyhow!("Graph Function resource '{key}' is not installed"))?;
    let installed = js_sys::eval(&resource.compiled_java_script)
        .map_err(|error| js_error("installation failed", error))?;
    let entry = Reflect::get(&installed, &JsValue::from_str("entry"))
        .map_err(|error| js_error("entry lookup failed", error))?
        .dyn_into::<Function>()
        .map_err(|_| anyhow!("Graph Function ABI v10 installer returned no entry"))?;
    let motion_kind = Reflect::get(&installed, &JsValue::from_str("motionKind"))
        .map_err(|error| js_error("motion symbol lookup failed", error))?;
    if resource
        .outputs
        .iter()
        .any(|port| port.motion == Some(true))
        && motion_kind.is_undefined()
    {
        bail!("Graph Function ABI v10 installer returned no motion symbol");
    }
    PREPARED.with(|prepared| {
        prepared.borrow_mut().insert(
            key,
            PreparedFunction {
                entry,
                motion_kind,
                resource,
            },
        );
    });
    Ok(())
}

pub fn prepare_state_machine(state_machine: &StateMachine) -> Result<()> {
    for graph in &state_machine.derivations {
        for node in &graph.nodes {
            if node.node_type == GraphInnerNodeType::DerivationFunction {
                prepare(&format!("derivation:{}", graph.id), &node.id)?;
            }
        }
    }
    for state in &state_machine.states {
        if let Some(graph) = &state.mutation_graph {
            for node in &graph.nodes {
                if node.node_type == GraphInnerNodeType::MutationFunction {
                    prepare(&format!("state:{}", state.id), &node.id)?;
                }
            }
        }
    }
    Ok(())
}

pub fn evaluate_function(
    graph_scope: &str,
    node_id: &str,
    inputs: &[GraphValue],
    remaining_budget: Duration,
) -> Result<Vec<FunctionOutput>> {
    if remaining_budget.is_zero() {
        bail!("Graph Function exceeded the graph frame budget");
    }
    let key = resource_key(graph_scope, node_id);
    PREPARED.with(|prepared| {
        let prepared = prepared.borrow();
        let function = prepared.get(&key).ok_or_else(|| {
            anyhow!("Graph Function '{key}' was not prepared before the frame")
        })?;
        if inputs.len() != function.resource.inputs.len() {
            bail!(
                "Graph Function '{node_id}' expected {} inputs, got {}",
                function.resource.inputs.len(),
                inputs.len()
            );
        }
        let input = Object::new();
        for (port, value) in function.resource.inputs.iter().zip(inputs) {
            let value = serde_wasm_bindgen::to_value(&value.to_json())
                .map_err(|error| anyhow!("failed to encode Graph Function input: {error}"))?;
            Reflect::set(&input, &JsValue::from_str(&port.id), &value)
                .map_err(|error| js_error("input assignment failed", error))?;
        }
        let started = js_sys::Date::now();
        let returned = function
            .entry
            .call1(&JsValue::UNDEFINED, &input)
            .map_err(|error| js_error("execution failed", error))?;
        if js_sys::Date::now() - started > remaining_budget.as_secs_f64() * 1000.0 {
            bail!("Graph Function exceeded the graph frame budget");
        }
        if function.resource.outputs.is_empty() {
            return Ok(Vec::new());
        }
        if !returned.is_object() {
            bail!("Graph Function '{node_id}' must return an object");
        }
        function
            .resource
            .outputs
            .iter()
            .map(|port| {
                let value = Reflect::get(&returned, &JsValue::from_str(&port.id))
                    .map_err(|error| js_error("output lookup failed", error))?;
                if value.is_undefined() {
                    bail!("Graph Function '{node_id}' omitted output '{}'", port.id);
                }
                if port.motion == Some(true) {
                    let mut nodes = 0;
                    let plan = motion_plan_from_js(
                        &value,
                        port,
                        &function.motion_kind,
                        0,
                        &mut nodes,
                    )?;
                    plan.validate()?;
                    Ok(FunctionOutput::Motion(plan))
                } else {
                    graph_value_from_js(&value, port)
                        .map(FunctionOutput::Value)
                        .with_context(|| {
                            format!(
                                "Graph Function '{node_id}.{}' returned a value incompatible with '{}'",
                                port.id, port.port_type
                            )
                        })
                }
            })
            .collect()
    })
}

fn property(value: &JsValue, name: &str) -> Result<JsValue> {
    Reflect::get(value, &JsValue::from_str(name))
        .map_err(|error| js_error(&format!("property '{name}' lookup failed"), error))
}

fn number_property(value: &JsValue, name: &str) -> Result<f64> {
    property(value, name)?
        .as_f64()
        .filter(|number| number.is_finite())
        .ok_or_else(|| anyhow!("Motion property '{name}' must be a finite number"))
}

fn motion_plan_from_js(
    value: &JsValue,
    port: &ReflectedPort,
    motion_kind: &JsValue,
    depth: usize,
    nodes: &mut usize,
) -> Result<MutationPlan> {
    if depth > 64 {
        bail!("Motion plan exceeds maximum nesting depth");
    }
    *nodes += 1;
    if *nodes > 4096 {
        bail!("Motion plan exceeds maximum node count");
    }
    if !value.is_object() {
        bail!("Motion output is not an object");
    }
    let kind = Reflect::get(value, motion_kind)
        .map_err(|error| js_error("motion kind lookup failed", error))?
        .as_string()
        .ok_or_else(|| anyhow!("Motion output was not created by a Motion helper"))?;
    match kind.as_str() {
        "setTo" => {
            let target = graph_value_from_js(&property(value, "target")?, port)?.to_json();
            let velocity = property(value, "velocity")?;
            let velocity = if velocity.is_undefined() {
                None
            } else {
                Some(graph_value_from_js(&velocity, port)?.to_json())
            };
            Ok(MutationPlan::SetTo { target, velocity })
        }
        "to" => {
            let target = graph_value_from_js(&property(value, "target")?, port)?.to_json();
            let duration = number_property(value, "duration")?;
            let delay = number_property(value, "timingDelay")?;
            let timing = match property(value, "timing")?.as_string().as_deref() {
                Some("linear") => MutationTiming::Linear { duration, delay },
                Some("spring") => MutationTiming::Spring {
                    duration,
                    bounce: number_property(value, "bounce")?,
                    delay,
                },
                _ => bail!("to(...) timing must come from linear(...) or spring(...)"),
            };
            Ok(MutationPlan::To { target, timing })
        }
        "sequence" => {
            let children = Array::from(&property(value, "children")?);
            if children.length() == 0 {
                bail!("sequence(...) requires at least one child");
            }
            let plans = children
                .iter()
                .map(|child| motion_plan_from_js(&child, port, motion_kind, depth + 1, nodes))
                .collect::<Result<Vec<_>>>()?;
            Ok(MutationPlan::Sequence(plans))
        }
        "repeat" => {
            let count = number_property(value, "count")?;
            if count.fract() != 0.0 || count < i64::MIN as f64 || count > i64::MAX as f64 {
                bail!("repeat(...) count must be an integer");
            }
            Ok(MutationPlan::Repeat {
                child: Box::new(motion_plan_from_js(
                    &property(value, "child")?,
                    port,
                    motion_kind,
                    depth + 1,
                    nodes,
                )?),
                count: count as i64,
            })
        }
        "delay" => Ok(MutationPlan::Delay {
            child: Box::new(motion_plan_from_js(
                &property(value, "child")?,
                port,
                motion_kind,
                depth + 1,
                nodes,
            )?),
            delay: number_property(value, "seconds")?,
        }),
        _ => bail!("unknown Motion output kind '{kind}'"),
    }
}

fn graph_value_from_js(value: &JsValue, port: &ReflectedPort) -> Result<GraphValue> {
    let json: serde_json::Value = serde_wasm_bindgen::from_value(value.clone())
        .map_err(|error| anyhow!("failed to decode Graph Function value: {error}"))?;
    let parsed = GraphValue::from_json_typed(&json, Some(&port.port_type))
        .ok_or_else(|| anyhow!("value does not match '{}'", port.port_type))?;
    if let (Some(expected), GraphValue::Packed(values)) = (port.array_length, &parsed)
        && values.len() != expected
    {
        bail!("expected exactly {expected} packed elements");
    }
    Ok(parsed)
}
