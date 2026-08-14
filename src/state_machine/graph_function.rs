use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock, RwLock, Weak,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use deno_core::{JsRuntime, RuntimeOptions, v8};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    graph::GraphValue,
    types::{GraphInnerNode, GraphInnerNodeType, GraphPort, StateMachine},
};
use crate::dsl::SceneDSL;

const GRAPH_FUNCTION_ABI_VERSION: u32 = 9;
const WATCHDOG_IDLE: u8 = 0;
const WATCHDOG_ARMED: u8 = 1;
const WATCHDOG_FIRING: u8 = 2;
const WATCHDOG_TIMED_OUT: u8 = 3;

#[cfg(test)]
static TEST_CONTEXT_CREATIONS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static TEST_SCRIPT_COMPILATIONS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static TEST_WATCHDOG_THREADS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static TEST_RUNTIME_CREATIONS: AtomicU64 = AtomicU64::new(0);

macro_rules! javascript_error {
    ($scope:expr, $operation:expr) => {{
        let message = $scope
            .exception()
            .map(|value| value.to_rust_string_lossy($scope))
            .unwrap_or_else(|| "unknown JavaScript exception".to_string());
        anyhow!("Graph Function {}: {}", $operation, message)
    }};
}

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

fn registry() -> &'static RwLock<HashMap<String, FunctionResource>> {
    static REGISTRY: OnceLock<RwLock<HashMap<String, FunctionResource>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

fn registry_generation() -> &'static AtomicU64 {
    static GENERATION: AtomicU64 = AtomicU64::new(1);
    &GENERATION
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
    let mut installed = registry()
        .write()
        .map_err(|_| anyhow!("Graph Function registry lock poisoned"))?;
    *installed = next;
    registry_generation().fetch_add(1, Ordering::AcqRel);
    Ok(())
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

pub fn clear_document_functions() {
    if let Ok(mut functions) = registry().write() {
        functions.clear();
        registry_generation().fetch_add(1, Ordering::AcqRel);
    }
}

pub fn installed_document_functions() -> Vec<FunctionResource> {
    registry()
        .read()
        .map(|functions| functions.values().cloned().collect())
        .unwrap_or_default()
}

fn registry_snapshot() -> Result<(u64, HashMap<String, FunctionResource>)> {
    let functions = registry()
        .read()
        .map_err(|_| anyhow!("Graph Function registry lock poisoned"))?;
    let generation = registry_generation().load(Ordering::Acquire);
    Ok((generation, functions.clone()))
}

fn function_for<'a>(
    functions: &'a HashMap<String, FunctionResource>,
    graph_scope: &str,
    node_id: &str,
) -> Result<&'a FunctionResource> {
    functions
        .get(&resource_key(graph_scope, node_id))
        .ok_or_else(|| {
            anyhow!("Graph Function resource '{graph_scope}/{node_id}' is not installed")
        })
}

struct WatchdogSlot {
    isolate_handle: v8::IsolateHandle,
    deadline_ns: AtomicU64,
    state: AtomicU8,
    timed_out: AtomicBool,
}

struct WatchdogInner {
    epoch: Instant,
    slots: Mutex<Vec<Weak<WatchdogSlot>>>,
}

struct SharedWatchdog {
    inner: Arc<WatchdogInner>,
    worker: thread::Thread,
}

impl SharedWatchdog {
    fn global() -> &'static Self {
        static WATCHDOG: OnceLock<SharedWatchdog> = OnceLock::new();
        WATCHDOG.get_or_init(|| {
            let inner = Arc::new(WatchdogInner {
                epoch: Instant::now(),
                slots: Mutex::new(Vec::new()),
            });
            let worker_inner = Arc::clone(&inner);
            let handle = thread::Builder::new()
                .name("graph-function-watchdog".into())
                .spawn(move || watchdog_loop(worker_inner))
                .expect("failed to spawn shared Graph Function watchdog");
            #[cfg(test)]
            TEST_WATCHDOG_THREADS.fetch_add(1, Ordering::Relaxed);
            let worker = handle.thread().clone();
            drop(handle);
            Self { inner, worker }
        })
    }

    fn register(&self, isolate_handle: v8::IsolateHandle) -> Arc<WatchdogSlot> {
        let slot = Arc::new(WatchdogSlot {
            isolate_handle,
            deadline_ns: AtomicU64::new(0),
            state: AtomicU8::new(WATCHDOG_IDLE),
            timed_out: AtomicBool::new(false),
        });
        self.inner
            .slots
            .lock()
            .expect("Graph Function watchdog lock poisoned")
            .push(Arc::downgrade(&slot));
        self.worker.unpark();
        slot
    }

    fn arm(&self, slot: &WatchdogSlot, budget: Duration) {
        let deadline = self
            .inner
            .epoch
            .elapsed()
            .saturating_add(budget)
            .as_nanos()
            .min(u64::MAX as u128) as u64;
        slot.deadline_ns.store(deadline, Ordering::Relaxed);
        slot.timed_out.store(false, Ordering::Release);
        slot.state.store(WATCHDOG_ARMED, Ordering::Release);
        self.worker.unpark();
    }

    fn disarm(&self, slot: &WatchdogSlot) -> bool {
        loop {
            match slot.state.load(Ordering::Acquire) {
                WATCHDOG_IDLE => return slot.timed_out.swap(false, Ordering::AcqRel),
                WATCHDOG_ARMED => {
                    if slot
                        .state
                        .compare_exchange(
                            WATCHDOG_ARMED,
                            WATCHDOG_IDLE,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return slot.timed_out.swap(false, Ordering::AcqRel);
                    }
                }
                WATCHDOG_FIRING => thread::yield_now(),
                WATCHDOG_TIMED_OUT => {
                    slot.state.store(WATCHDOG_IDLE, Ordering::Release);
                    return slot.timed_out.swap(false, Ordering::AcqRel);
                }
                _ => unreachable!("invalid Graph Function watchdog state"),
            }
        }
    }
}

fn watchdog_loop(inner: Arc<WatchdogInner>) {
    loop {
        let now_ns = inner.epoch.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let mut wait = Duration::from_secs(1);
        {
            let mut slots = inner
                .slots
                .lock()
                .expect("Graph Function watchdog lock poisoned");
            slots.retain(|slot| {
                let Some(slot) = slot.upgrade() else {
                    return false;
                };
                if slot.state.load(Ordering::Acquire) == WATCHDOG_ARMED {
                    let deadline = slot.deadline_ns.load(Ordering::Relaxed);
                    if deadline <= now_ns {
                        if slot
                            .state
                            .compare_exchange(
                                WATCHDOG_ARMED,
                                WATCHDOG_FIRING,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            )
                            .is_ok()
                        {
                            slot.timed_out.store(true, Ordering::Release);
                            slot.isolate_handle.terminate_execution();
                            slot.state.store(WATCHDOG_TIMED_OUT, Ordering::Release);
                        }
                    } else {
                        wait = wait.min(Duration::from_nanos(deadline - now_ns));
                    }
                }
                true
            });
        }
        thread::park_timeout(wait);
    }
}

struct PreparedFunction {
    context: v8::Global<v8::Context>,
    function: v8::Global<v8::Function>,
    motion_kind: Option<v8::Global<v8::Symbol>>,
    input_keys: Vec<v8::Global<v8::String>>,
    output_keys: Vec<v8::Global<v8::String>>,
    outputs: Vec<ReflectedPort>,
    artifact_fingerprint: [u8; 32],
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionOutput {
    Value(GraphValue),
    SetTo {
        target: GraphValue,
        velocity: Option<GraphValue>,
    },
    To {
        target: GraphValue,
        duration: f64,
        bounce: f64,
    },
}

struct GraphJsRuntime {
    // Global handles must be dropped before their owning isolate.
    functions: HashMap<String, HashMap<String, PreparedFunction>>,
    runtime: JsRuntime,
    watchdog_slot: Arc<WatchdogSlot>,
    generation: u64,
}

impl GraphJsRuntime {
    fn new(generation: u64) -> Self {
        #[cfg(test)]
        TEST_RUNTIME_CREATIONS.fetch_add(1, Ordering::Relaxed);
        let mut runtime = JsRuntime::new(RuntimeOptions::default());
        let watchdog_slot =
            SharedWatchdog::global().register(runtime.v8_isolate().thread_safe_handle());
        Self {
            functions: HashMap::new(),
            runtime,
            watchdog_slot,
            generation,
        }
    }

    fn contains_current(&self, resource: &FunctionResource) -> bool {
        self.functions
            .get(&resource.scope)
            .and_then(|functions| functions.get(&resource.node_id))
            .is_some_and(|function| function.artifact_fingerprint == artifact_fingerprint(resource))
    }

    fn reconcile(&mut self, generation: u64, installed: &HashMap<String, FunctionResource>) {
        self.functions.retain(|graph_scope, functions| {
            functions.retain(|node_id, prepared| {
                installed.values().any(|resource| {
                    resource.node_id == *node_id
                        && resource.scope == *graph_scope
                        && prepared.artifact_fingerprint == artifact_fingerprint(resource)
                })
            });
            !functions.is_empty()
        });
        self.generation = generation;
    }

    fn prepare(&mut self, resource: &FunctionResource) -> Result<()> {
        if self.contains_current(resource) {
            return Ok(());
        }
        let prepared = {
            let isolate = self.runtime.v8_isolate();
            v8::scope!(handle_scope, isolate);
            #[cfg(test)]
            TEST_CONTEXT_CREATIONS.fetch_add(1, Ordering::Relaxed);
            let context = v8::Context::new(handle_scope, Default::default());
            let scope = &mut v8::ContextScope::new(handle_scope, context);
            v8::tc_scope!(let scope, scope);

            let source = v8::String::new(scope, &resource.compiled_java_script)
                .ok_or_else(|| anyhow!("Graph Function compiled source is too large"))?;
            #[cfg(test)]
            TEST_SCRIPT_COMPILATIONS.fetch_add(1, Ordering::Relaxed);
            let script = v8::Script::compile(scope, source, None)
                .ok_or_else(|| javascript_error!(scope, "compilation failed"))?;
            let installed = script
                .run(scope)
                .ok_or_else(|| javascript_error!(scope, "installation failed"))?;
            let installed = installed
                .to_object(scope)
                .ok_or_else(|| anyhow!("Graph Function ABI v9 installer returned no object"))?;
            let entry_key = v8::String::new(scope, "entry").unwrap();
            let bindings_key = v8::String::new(scope, "bindings").unwrap();
            let motion_kind_key = v8::String::new(scope, "motionKind").unwrap();
            let entry = installed
                .get(scope, entry_key.into())
                .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
                .ok_or_else(|| anyhow!("Graph Function ABI v9 installer returned no entry"))?;
            let bindings = installed
                .get(scope, bindings_key.into())
                .and_then(|value| v8::Local::<v8::Array>::try_from(value).ok())
                .ok_or_else(|| {
                    anyhow!("Graph Function ABI v9 installer returned no bindings array")
                })?;
            let motion_kind = installed
                .get(scope, motion_kind_key.into())
                .and_then(|value| v8::Local::<v8::Symbol>::try_from(value).ok())
                .map(|symbol| v8::Global::new(scope, symbol));
            if resource
                .outputs
                .iter()
                .any(|output| output.motion == Some(true))
                && motion_kind.is_none()
            {
                bail!("Graph Function ABI v9 installer returned no motion symbol");
            }
            for index in 0..bindings.length() {
                let binding = bindings
                    .get_index(scope, index)
                    .ok_or_else(|| anyhow!("Graph Function binding {index} is missing"))?;
                deep_freeze(scope, binding)?;
            }
            harden_context(scope)?;
            context.set_allow_generation_from_strings(false);

            let input_keys = resource
                .inputs
                .iter()
                .map(|port| persistent_string(scope, &port.id))
                .collect::<Result<Vec<_>>>()?;
            let output_keys = resource
                .outputs
                .iter()
                .map(|port| persistent_string(scope, &port.id))
                .collect::<Result<Vec<_>>>()?;

            PreparedFunction {
                context: v8::Global::new(scope, context),
                function: v8::Global::new(scope, entry),
                motion_kind,
                input_keys,
                output_keys,
                outputs: resource.outputs.clone(),
                artifact_fingerprint: artifact_fingerprint(resource),
            }
        };
        self.functions
            .entry(resource.scope.clone())
            .or_default()
            .insert(resource.node_id.clone(), prepared);
        Ok(())
    }

    fn evaluate(
        &mut self,
        graph_scope: &str,
        node_id: &str,
        inputs: &[GraphValue],
        remaining_budget: Duration,
    ) -> Result<Vec<FunctionOutput>> {
        if remaining_budget.is_zero() {
            bail!("Graph Function exceeded the graph frame budget");
        }
        // Registry changes are reconciled by `prepare` at the next frame boundary.
        // Keep evaluating the immutable context prepared for the current frame so
        // a concurrent scene refresh cannot invalidate an in-flight snapshot.
        let (runtime, functions) = (&mut self.runtime, &self.functions);
        let prepared = functions
            .get(graph_scope)
            .and_then(|functions| functions.get(node_id))
            .ok_or_else(|| {
                anyhow!(
                    "Graph Function '{graph_scope}/{node_id}' was not prepared before the frame"
                )
            })?;
        if inputs.len() != prepared.input_keys.len() {
            bail!(
                "Graph Function '{node_id}' expected {} inputs, got {}",
                prepared.input_keys.len(),
                inputs.len()
            );
        }
        SharedWatchdog::global().arm(&self.watchdog_slot, remaining_budget);
        let result = (|| -> Result<Vec<FunctionOutput>> {
            let isolate = runtime.v8_isolate();
            v8::scope!(handle_scope, isolate);
            let context = v8::Local::new(handle_scope, &prepared.context);
            let scope = &mut v8::ContextScope::new(handle_scope, context);
            v8::tc_scope!(let scope, scope);

            let input = v8::Object::new(scope);
            for (index, (value, key)) in inputs.iter().zip(&prepared.input_keys).enumerate() {
                let key = v8::Local::new(scope, key);
                let value = graph_value_to_v8(scope, value).with_context(|| {
                    format!("failed to encode Graph Function input at index {index}")
                })?;
                if input.set(scope, key.into(), value) != Some(true) {
                    bail!("failed to set Graph Function input at index {index}");
                }
            }
            deep_freeze(scope, input.into())?;

            let function = v8::Local::new(scope, &prepared.function);
            let receiver = v8::undefined(scope).into();
            let returned = function.call(scope, receiver, &[input.into()]);
            let returned = returned.ok_or_else(|| javascript_error!(scope, "execution failed"))?;
            let outputs = if prepared.outputs.is_empty() {
                Vec::new()
            } else {
                let returned = returned
                    .to_object(scope)
                    .ok_or_else(|| anyhow!("Graph Function '{node_id}' must return an object"))?;
                prepared
                    .outputs
                    .iter()
                    .zip(&prepared.output_keys)
                    .map(|(port, key)| -> Result<FunctionOutput> {
                        let key = v8::Local::new(scope, key);
                        let value = returned.get(scope, key.into()).ok_or_else(|| {
                            anyhow!("Graph Function '{node_id}' omitted output '{}'", port.id)
                        })?;
                        if port.motion == Some(true) {
                            let motion_kind = prepared.motion_kind.as_ref().ok_or_else(|| {
                                anyhow!(
                                    "Graph Function '{node_id}' has a Motion output but no motion symbol"
                                )
                            })?;
                            let motion_kind = v8::Local::new(scope, motion_kind);
                            return motion_output_from_v8(
                                scope,
                                value,
                                port,
                                motion_kind,
                            )
                            .with_context(|| {
                                format!(
                                    "Graph Function '{node_id}.{}' must return setTo(...) or to(...)",
                                    port.id
                                )
                            });
                        }
                        graph_value_from_v8(scope, value, port).map(FunctionOutput::Value).with_context(|| {
                            format!(
                                "Graph Function '{node_id}.{}' returned a value incompatible with '{}'",
                                port.id, port.port_type
                            )
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
            };
            Ok(outputs)
        })();
        let timed_out = SharedWatchdog::global().disarm(&self.watchdog_slot);
        let terminating = self.runtime.v8_isolate().is_execution_terminating();
        if timed_out || terminating {
            self.runtime.v8_isolate().cancel_terminate_execution();
            bail!("Graph Function exceeded the graph frame budget");
        }
        result
    }
}

fn motion_output_from_v8(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<'_, v8::Value>,
    port: &ReflectedPort,
    motion_kind: v8::Local<'_, v8::Symbol>,
) -> Result<FunctionOutput> {
    let object = value
        .to_object(scope)
        .ok_or_else(|| anyhow!("Motion output is not an object"))?;
    let kind = object
        .get(scope, motion_kind.into())
        .map(|value| value.to_rust_string_lossy(scope))
        .ok_or_else(|| anyhow!("Motion output was not created by setTo(...) or to(...)"))?;
    let target_key = v8::String::new(scope, "target").unwrap();
    let target = object
        .get(scope, target_key.into())
        .ok_or_else(|| anyhow!("Motion output omitted target"))
        .and_then(|value| graph_value_from_v8(scope, value, port))?;
    match kind.as_str() {
        "setTo" => {
            let velocity_key = v8::String::new(scope, "velocity").unwrap();
            let velocity = object
                .get(scope, velocity_key.into())
                .filter(|value| !value.is_undefined())
                .map(|value| graph_value_from_v8(scope, value, port))
                .transpose()?;
            Ok(FunctionOutput::SetTo { target, velocity })
        }
        "to" => {
            let duration_key = v8::String::new(scope, "duration").unwrap();
            let bounce_key = v8::String::new(scope, "bounce").unwrap();
            let duration = object
                .get(scope, duration_key.into())
                .and_then(|value| value.number_value(scope))
                .ok_or_else(|| anyhow!("to(...) duration must be numeric"))?;
            let bounce = object
                .get(scope, bounce_key.into())
                .and_then(|value| value.number_value(scope))
                .ok_or_else(|| anyhow!("to(...) bounce must be numeric"))?;
            Ok(FunctionOutput::To {
                target,
                duration,
                bounce,
            })
        }
        _ => bail!("unknown Motion output kind '{kind}'"),
    }
}

fn artifact_fingerprint(resource: &FunctionResource) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(resource.abi_version.to_le_bytes());
    for value in [
        resource.scope.as_str(),
        resource.node_id.as_str(),
        resource.source_hash.as_str(),
        resource.compiled_java_script.as_str(),
    ] {
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value.as_bytes());
    }
    for ports in [&resource.inputs, &resource.outputs] {
        digest.update((ports.len() as u64).to_le_bytes());
        for port in ports {
            for value in [
                port.id.as_str(),
                port.name.as_str(),
                port.port_type.as_str(),
            ] {
                digest.update((value.len() as u64).to_le_bytes());
                digest.update(value.as_bytes());
            }
            digest.update(port.array_length.unwrap_or(usize::MAX).to_le_bytes());
            digest.update([u8::from(port.motion == Some(true))]);
        }
    }
    digest.finalize().into()
}

fn persistent_string(
    scope: &mut v8::PinScope<'_, '_>,
    value: &str,
) -> Result<v8::Global<v8::String>> {
    v8::String::new(scope, value)
        .map(|value| v8::Global::new(scope, value))
        .ok_or_else(|| anyhow!("Graph Function port name is too large"))
}

fn harden_context(scope: &mut v8::PinScope<'_, '_>) -> Result<()> {
    v8::tc_scope!(let scope, scope);
    let hardening = r#"
      Object.defineProperty(Math, 'random', {
        value: undefined, writable: false, configurable: false
      });
      for (const value of [
        Math, Object.prototype, Array.prototype, Number.prototype,
        Boolean.prototype, String.prototype, Function.prototype
      ]) Object.freeze(value);
      for (const name of [
        'Deno', 'fetch', 'WebAssembly', 'Date', 'eval', 'Function',
        'setTimeout', 'setInterval', 'clearTimeout', 'clearInterval',
        'queueMicrotask', 'crypto', 'performance'
      ]) {
        try {
          Object.defineProperty(globalThis, name, {
            value: undefined, writable: false, configurable: false
          });
        } catch {}
      }
    "#;
    let source = v8::String::new(scope, hardening).unwrap();
    let script = v8::Script::compile(scope, source, None)
        .ok_or_else(|| javascript_error!(scope, "context hardening compilation failed"))?;
    script
        .run(scope)
        .ok_or_else(|| javascript_error!(scope, "context hardening failed"))?;
    Ok(())
}

fn deep_freeze<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<()> {
    if !value.is_object() {
        return Ok(());
    }
    let object = value
        .to_object(scope)
        .ok_or_else(|| anyhow!("failed to inspect Graph Function object"))?;
    if !value.is_function() {
        let keys = object
            .get_own_property_names(scope, Default::default())
            .ok_or_else(|| anyhow!("failed to enumerate Graph Function object"))?;
        for index in 0..keys.length() {
            let key = keys
                .get_index(scope, index)
                .ok_or_else(|| anyhow!("failed to read Graph Function object key"))?;
            let child = object
                .get(scope, key)
                .ok_or_else(|| anyhow!("failed to read Graph Function object value"))?;
            deep_freeze(scope, child)?;
        }
    }
    if object.set_integrity_level(scope, v8::IntegrityLevel::Frozen) != Some(true) {
        bail!("failed to freeze Graph Function object");
    }
    Ok(())
}

fn graph_value_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: &GraphValue,
) -> Result<v8::Local<'s, v8::Value>> {
    let value = match value {
        GraphValue::Float(value) => v8::Number::new(scope, *value).into(),
        GraphValue::Int(value) => v8::Number::new(scope, *value as f64).into(),
        GraphValue::Bool(value) => v8::Boolean::new(scope, *value).into(),
        GraphValue::Vec2(values) => numeric_array(scope, values)?.into(),
        GraphValue::Vec3(values) => numeric_array(scope, values)?.into(),
        GraphValue::Vec4(values) | GraphValue::Color(values) => {
            numeric_array(scope, values)?.into()
        }
        GraphValue::NormalizedBezierCurve(values) => numeric_array(scope, values)?.into(),
        GraphValue::BezierCurve(points) => {
            let array = v8::Array::new(scope, points.len() as i32);
            for (index, point) in points.iter().enumerate() {
                let point = numeric_array(scope, point)?;
                if array.set_index(scope, index as u32, point.into()) != Some(true) {
                    bail!("failed to construct Bezier curve Graph Function input");
                }
            }
            array.into()
        }
        GraphValue::Packed(values) => {
            let array = v8::Array::new(scope, values.len() as i32);
            for (index, value) in values.iter().enumerate() {
                let value = graph_value_to_v8(scope, value)?;
                if array.set_index(scope, index as u32, value) != Some(true) {
                    bail!("failed to construct packed Graph Function input");
                }
            }
            array.into()
        }
    };
    Ok(value)
}

fn numeric_array<'s, const N: usize>(
    scope: &mut v8::PinScope<'s, '_>,
    values: &[f64; N],
) -> Result<v8::Local<'s, v8::Array>> {
    let array = v8::Array::new(scope, N as i32);
    for (index, value) in values.iter().enumerate() {
        if array.set_index(scope, index as u32, v8::Number::new(scope, *value).into()) != Some(true)
        {
            bail!("failed to construct vector Graph Function input");
        }
    }
    Ok(array)
}

fn graph_value_from_v8(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<'_, v8::Value>,
    port: &ReflectedPort,
) -> Result<GraphValue> {
    if let Some(element_type) = port
        .port_type
        .strip_prefix("packed<")
        .and_then(|value| value.strip_suffix('>'))
    {
        let array = v8::Local::<v8::Array>::try_from(value)
            .map_err(|_| anyhow!("expected packed array"))?;
        if let Some(expected) = port.array_length
            && array.length() as usize != expected
        {
            bail!("expected exactly {expected} packed elements");
        }
        let mut values = Vec::with_capacity(array.length() as usize);
        for index in 0..array.length() {
            let item = array
                .get_index(scope, index)
                .ok_or_else(|| anyhow!("missing packed element {index}"))?;
            values.push(graph_value_from_type(scope, item, element_type)?);
        }
        return Ok(GraphValue::Packed(values));
    }
    graph_value_from_type(scope, value, &port.port_type)
}

fn graph_value_from_type(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<'_, v8::Value>,
    port_type: &str,
) -> Result<GraphValue> {
    match port_type {
        "float" => finite_number(scope, value).map(GraphValue::Float),
        "int" => {
            let value = finite_number(scope, value)?;
            if value.fract() != 0.0 || value < i64::MIN as f64 || value > i64::MAX as f64 {
                bail!("expected integer");
            }
            Ok(GraphValue::Int(value as i64))
        }
        "bool" => {
            if !value.is_boolean() {
                bail!("expected boolean");
            }
            Ok(GraphValue::Bool(value.boolean_value(scope)))
        }
        "vector2" => Ok(GraphValue::Vec2(numeric_tuple(scope, value)?)),
        "vector3" => Ok(GraphValue::Vec3(numeric_tuple(scope, value)?)),
        "vector4" => Ok(GraphValue::Vec4(numeric_tuple(scope, value)?)),
        "color" => Ok(GraphValue::Color(numeric_tuple(scope, value)?)),
        "normalizedBezierCurve" => Ok(GraphValue::NormalizedBezierCurve(numeric_tuple(
            scope, value,
        )?)),
        "bezierCurve" => {
            let array = v8::Local::<v8::Array>::try_from(value)
                .map_err(|_| anyhow!("expected Bezier curve array"))?;
            if array.length() != 4 {
                bail!("expected exactly 4 Bezier control points");
            }
            let mut points = [[0.0; 2]; 4];
            for (index, point) in points.iter_mut().enumerate() {
                let value = array
                    .get_index(scope, index as u32)
                    .ok_or_else(|| anyhow!("missing Bezier control point {index}"))?;
                *point = numeric_tuple(scope, value)?;
            }
            Ok(GraphValue::BezierCurve(points))
        }
        other => bail!("unsupported Graph Function port type '{other}'"),
    }
}

fn finite_number(scope: &mut v8::PinScope<'_, '_>, value: v8::Local<'_, v8::Value>) -> Result<f64> {
    if !value.is_number() {
        bail!("expected number");
    }
    let value = value
        .number_value(scope)
        .ok_or_else(|| anyhow!("failed to read number"))?;
    if !value.is_finite() {
        bail!("expected finite number");
    }
    Ok(value)
}

fn numeric_tuple<const N: usize>(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<'_, v8::Value>,
) -> Result<[f64; N]> {
    let array =
        v8::Local::<v8::Array>::try_from(value).map_err(|_| anyhow!("expected vector array"))?;
    if array.length() as usize != N {
        bail!("expected exactly {N} vector components");
    }
    let mut values = [0.0; N];
    for (index, output) in values.iter_mut().enumerate() {
        let value = array
            .get_index(scope, index as u32)
            .ok_or_else(|| anyhow!("missing vector component {index}"))?;
        *output = finite_number(scope, value)?;
    }
    Ok(values)
}

thread_local! {
    static FUNCTION_RUNTIME: RefCell<Option<GraphJsRuntime>> = const { RefCell::new(None) };
}

pub fn prepare(graph_scope: &str, node_id: &str) -> Result<()> {
    let (generation, installed) = registry_snapshot()?;
    let resource = function_for(&installed, graph_scope, node_id)?;
    FUNCTION_RUNTIME.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        if runtime.is_none() {
            *runtime = Some(GraphJsRuntime::new(generation));
        }
        let runtime = runtime.as_mut().expect("runtime inserted above");
        if runtime.generation != generation {
            runtime.reconcile(generation, &installed);
        }
        runtime.prepare(resource)
    })
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
    FUNCTION_RUNTIME.with(|runtime| {
        runtime
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| {
                anyhow!(
                    "Graph Function runtime was not prepared before evaluating '{graph_scope}/{node_id}'"
                )
            })?
            .evaluate(graph_scope, node_id, inputs, remaining_budget)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_with_mutation_function() -> SceneDSL {
        serde_json::from_value(serde_json::json!({
            "version": "6.0",
            "metadata": { "name": "function validation" },
            "nodes": [],
            "connections": [],
            "groups": [],
            "assets": {},
            "stateMachine": {
                "id": "machine",
                "stateParams": [],
                "stateParamGraph": {
                    "rootNodePosition": { "x": -320.0, "y": -120.0 },
                    "declarationPositions": {}
                },
                "states": [
                    { "id": "entry", "name": "Entry", "type": "entryState" },
                    { "id": "any", "name": "Any", "type": "anyState" },
                    { "id": "exit", "name": "Exit", "type": "exitState" },
                    {
                        "id": "active",
                        "name": "Active",
                        "type": "animationState",
                        "mutationGraph": {
                            "nodes": [{
                                "id": "fn",
                                "type": "MutationFunction",
                                "params": {},
                                "inputs": [{
                                    "id": "value",
                                    "name": "value",
                                    "type": "float"
                                }],
                                "outputs": [{
                                    "id": "value",
                                    "name": "value",
                                    "type": "float",
                                    "motion": true
                                }]
                            }],
                            "connections": [],
                            "inputBindings": [],
                            "outputBindings": [],
                            "layout": {
                                "parameterPositions": {},
                                "runtimeInputPosition": { "x": 0.0, "y": 0.0 },
                                "outputPosition": { "x": 0.0, "y": 0.0 }
                            }
                        }
                    }
                ],
                "transitions": [],
                "derivationBindings": [],
                "derivations": [],
                "motionGraphs": []
            }
        }))
        .expect("test SceneDSL")
    }

    fn mutation_resource() -> FunctionResource {
        let source = "export default function mutation() { return {}; }".to_string();
        FunctionResource {
            scope: "state:active".into(),
            node_id: "fn".into(),
            kind: "mutation".into(),
            language: "typescript".into(),
            source_hash: format!("{:x}", Sha256::digest(source.as_bytes())),
            source,
            compiled_java_script: "(() => ({ entry() {}, bindings: [] }))()".into(),
            abi_version: GRAPH_FUNCTION_ABI_VERSION,
            inputs: vec![ReflectedPort {
                id: "value".into(),
                name: "value".into(),
                port_type: "float".into(),
                array_length: None,
                motion: None,
            }],
            outputs: vec![ReflectedPort {
                id: "value".into(),
                name: "value".into(),
                port_type: "float".into(),
                array_length: None,
                motion: Some(true),
            }],
        }
    }

    #[test]
    fn function_resource_must_match_scope_node_type_and_reflected_ports() {
        let scene = scene_with_mutation_function();
        validate_function_resources(&scene, &[mutation_resource()])
            .expect("matching resource should validate");

        let mut stale = mutation_resource();
        stale.outputs[0].motion = None;
        let error = validate_function_resources(&scene, &[stale])
            .unwrap_err()
            .to_string();
        assert!(error.contains("reflection"), "{error}");
    }

    #[test]
    fn function_nodes_and_resources_have_a_one_to_one_identity() {
        let scene = scene_with_mutation_function();
        let missing = validate_function_resources(&scene, &[])
            .unwrap_err()
            .to_string();
        assert!(missing.contains("has no stored resource"), "{missing}");

        let mut orphan = mutation_resource();
        orphan.node_id = "other".into();
        let error = validate_function_resources(&scene, &[orphan])
            .unwrap_err()
            .to_string();
        assert!(error.contains("no matching graph node"), "{error}");
    }

    #[test]
    fn hdr_color_round_trips_through_derivation_and_mutation_motion_functions() {
        let color_port = |id: &str, motion: Option<bool>| ReflectedPort {
            id: id.into(),
            name: id.into(),
            port_type: "color".into(),
            array_length: None,
            motion,
        };
        let resource = |scope: &str,
                        node_id: &str,
                        kind: &str,
                        compiled_java_script: &str,
                        outputs: Vec<ReflectedPort>| {
            let source = format!("export default function {node_id}() {{}}");
            FunctionResource {
                scope: scope.into(),
                node_id: node_id.into(),
                kind: kind.into(),
                language: "typescript".into(),
                source_hash: format!("{:x}", Sha256::digest(source.as_bytes())),
                source,
                compiled_java_script: compiled_java_script.into(),
                abi_version: GRAPH_FUNCTION_ABI_VERSION,
                inputs: vec![color_port("color", None)],
                outputs,
            }
        };
        let derivation = resource(
            "derivation:hdr",
            "derive",
            "derivation",
            "(() => ({ bindings: [], entry(input) { return { color: input.color }; } }))()",
            vec![color_port("color", None)],
        );
        let mutation = resource(
            "state:hdr",
            "mutate",
            "mutation",
            "(() => { const motionKind = Symbol('motion'); return { motionKind, bindings: [], entry(input) { return { setColor: { [motionKind]: 'setTo', target: input.color }, springColor: { [motionKind]: 'to', target: input.color, duration: 0.4, bounce: 0.1 } }; } }; })()",
            vec![
                color_port("setColor", Some(true)),
                color_port("springColor", Some(true)),
            ],
        );

        let hdr = GraphValue::Color([4.0, 1.5, 0.25, 0.6]);
        let mut runtime = GraphJsRuntime::new(0);
        runtime.prepare(&derivation).unwrap();
        runtime.prepare(&mutation).unwrap();

        assert_eq!(
            runtime
                .evaluate(
                    "derivation:hdr",
                    "derive",
                    std::slice::from_ref(&hdr),
                    Duration::from_millis(100),
                )
                .unwrap(),
            vec![FunctionOutput::Value(hdr.clone())]
        );
        assert_eq!(
            runtime
                .evaluate(
                    "state:hdr",
                    "mutate",
                    std::slice::from_ref(&hdr),
                    Duration::from_millis(100),
                )
                .unwrap(),
            vec![
                FunctionOutput::SetTo {
                    target: hdr.clone(),
                    velocity: None,
                },
                FunctionOutput::To {
                    target: hdr,
                    duration: 0.4,
                    bounce: 0.1,
                },
            ]
        );
    }
}
