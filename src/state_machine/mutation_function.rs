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
    mutation::MutationValue,
    types::{MutationInnerNodeType, StateMachine},
};

const MUTATION_FUNCTION_ABI_VERSION: u32 = 2;
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

macro_rules! javascript_error {
    ($scope:expr, $operation:expr) => {{
        let message = $scope
            .exception()
            .map(|value| value.to_rust_string_lossy($scope))
            .unwrap_or_else(|| "unknown JavaScript exception".to_string());
        anyhow!("Mutation Function {}: {}", $operation, message)
    }};
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReflectedPort {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub port_type: String,
    #[serde(default)]
    pub array_length: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionResource {
    pub scope: String,
    pub node_id: String,
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
        if self.language != "typescript" {
            bail!("unsupported Mutation Function language '{}'", self.language);
        }
        if self.abi_version != MUTATION_FUNCTION_ABI_VERSION {
            bail!(
                "unsupported Mutation Function ABI {} (expected {})",
                self.abi_version,
                MUTATION_FUNCTION_ABI_VERSION
            );
        }
        let actual_hash = format!("{:x}", Sha256::digest(self.source.as_bytes()));
        if actual_hash != self.source_hash {
            bail!(
                "Mutation Function '{}/{}' source hash is stale",
                self.scope,
                self.node_id
            );
        }
        if self.compiled_java_script.trim().is_empty() {
            bail!("Mutation Function compiled JavaScript is empty");
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
        next.insert(resource_key(&function.scope, &function.node_id), function);
    }
    *registry()
        .write()
        .map_err(|_| anyhow!("Mutation Function registry lock poisoned"))? = next;
    registry_generation().fetch_add(1, Ordering::AcqRel);
    FUNCTION_RUNTIME.with(|runtime| *runtime.borrow_mut() = None);
    Ok(())
}

pub fn clear_document_functions() {
    if let Ok(mut functions) = registry().write() {
        functions.clear();
    }
    registry_generation().fetch_add(1, Ordering::AcqRel);
    FUNCTION_RUNTIME.with(|runtime| *runtime.borrow_mut() = None);
}

pub fn installed_document_functions() -> Vec<FunctionResource> {
    registry()
        .read()
        .map(|functions| functions.values().cloned().collect())
        .unwrap_or_default()
}

fn function_for(mutation_id: &str, node_id: &str) -> Result<FunctionResource> {
    let key = resource_key(&format!("mutation:{mutation_id}"), node_id);
    registry()
        .read()
        .map_err(|_| anyhow!("Mutation Function registry lock poisoned"))?
        .get(&key)
        .cloned()
        .ok_or_else(|| anyhow!("Mutation Function resource '{key}' is not installed"))
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
                .name("mutation-function-watchdog".into())
                .spawn(move || watchdog_loop(worker_inner))
                .expect("failed to spawn shared Mutation Function watchdog");
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
            .expect("Mutation Function watchdog lock poisoned")
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
                _ => unreachable!("invalid Mutation Function watchdog state"),
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
                .expect("Mutation Function watchdog lock poisoned");
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
    input_keys: Vec<v8::Global<v8::String>>,
    output_keys: Vec<v8::Global<v8::String>>,
    outputs: Vec<ReflectedPort>,
    source_hash: String,
}

struct MutationJsRuntime {
    // Global handles must be dropped before their owning isolate.
    functions: HashMap<String, HashMap<String, PreparedFunction>>,
    runtime: JsRuntime,
    watchdog_slot: Arc<WatchdogSlot>,
    generation: u64,
}

impl MutationJsRuntime {
    fn new(generation: u64) -> Self {
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
        let mutation_id = resource.scope.strip_prefix("mutation:").unwrap_or_default();
        self.functions
            .get(mutation_id)
            .and_then(|functions| functions.get(&resource.node_id))
            .is_some_and(|function| function.source_hash == resource.source_hash)
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
                .ok_or_else(|| anyhow!("Mutation Function compiled source is too large"))?;
            #[cfg(test)]
            TEST_SCRIPT_COMPILATIONS.fetch_add(1, Ordering::Relaxed);
            let script = v8::Script::compile(scope, source, None)
                .ok_or_else(|| javascript_error!(scope, "compilation failed"))?;
            let installed = script
                .run(scope)
                .ok_or_else(|| javascript_error!(scope, "installation failed"))?;
            let installed = installed
                .to_object(scope)
                .ok_or_else(|| anyhow!("Mutation Function ABI v2 installer returned no object"))?;
            let entry_key = v8::String::new(scope, "entry").unwrap();
            let bindings_key = v8::String::new(scope, "bindings").unwrap();
            let entry = installed
                .get(scope, entry_key.into())
                .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
                .ok_or_else(|| anyhow!("Mutation Function ABI v2 installer returned no entry"))?;
            let bindings = installed
                .get(scope, bindings_key.into())
                .and_then(|value| v8::Local::<v8::Array>::try_from(value).ok())
                .ok_or_else(|| {
                    anyhow!("Mutation Function ABI v2 installer returned no bindings array")
                })?;
            for index in 0..bindings.length() {
                let binding = bindings
                    .get_index(scope, index)
                    .ok_or_else(|| anyhow!("Mutation Function binding {index} is missing"))?;
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
                input_keys,
                output_keys,
                outputs: resource.outputs.clone(),
                source_hash: resource.source_hash.clone(),
            }
        };
        let mutation_id = resource
            .scope
            .strip_prefix("mutation:")
            .ok_or_else(|| anyhow!("invalid Mutation Function scope '{}'", resource.scope))?
            .to_string();
        self.functions
            .entry(mutation_id)
            .or_default()
            .insert(resource.node_id.clone(), prepared);
        Ok(())
    }

    fn evaluate(
        &mut self,
        mutation_id: &str,
        node_id: &str,
        inputs: &[MutationValue],
        remaining_budget: Duration,
    ) -> Result<Vec<MutationValue>> {
        if remaining_budget.is_zero() {
            bail!("Mutation Function exceeded the Mutation graph frame budget");
        }
        if self.generation != registry_generation().load(Ordering::Acquire) {
            bail!("Mutation Function runtime is stale and must be prepared before evaluation");
        }
        let (runtime, functions) = (&mut self.runtime, &self.functions);
        let prepared = functions
            .get(mutation_id)
            .and_then(|functions| functions.get(node_id))
            .ok_or_else(|| {
                anyhow!(
                    "Mutation Function 'mutation:{mutation_id}/{node_id}' was not prepared before the frame"
                )
            })?;
        if inputs.len() != prepared.input_keys.len() {
            bail!(
                "Mutation Function '{node_id}' expected {} inputs, got {}",
                prepared.input_keys.len(),
                inputs.len()
            );
        }

        SharedWatchdog::global().arm(&self.watchdog_slot, remaining_budget);
        let result = (|| -> Result<Vec<MutationValue>> {
            let isolate = runtime.v8_isolate();
            v8::scope!(handle_scope, isolate);
            let context = v8::Local::new(handle_scope, &prepared.context);
            let scope = &mut v8::ContextScope::new(handle_scope, context);
            v8::tc_scope!(let scope, scope);

            let input = v8::Object::new(scope);
            for ((value, key), index) in inputs.iter().zip(&prepared.input_keys).zip(0usize..) {
                let key = v8::Local::new(scope, key);
                let value = mutation_value_to_v8(scope, value).with_context(|| {
                    format!("failed to encode Mutation Function input at index {index}")
                })?;
                if input.set(scope, key.into(), value) != Some(true) {
                    bail!("failed to set Mutation Function input at index {index}");
                }
            }
            deep_freeze(scope, input.into())?;

            let function = v8::Local::new(scope, &prepared.function);
            let receiver = v8::undefined(scope).into();
            let returned = function
                .call(scope, receiver, &[input.into()])
                .ok_or_else(|| javascript_error!(scope, "execution failed"))?;
            let returned = returned
                .to_object(scope)
                .ok_or_else(|| anyhow!("Mutation Function '{node_id}' must return an object"))?;
            prepared
                .outputs
                .iter()
                .zip(&prepared.output_keys)
                .map(|(port, key)| {
                    let key = v8::Local::new(scope, key);
                    let value = returned.get(scope, key.into()).ok_or_else(|| {
                        anyhow!("Mutation Function '{node_id}' omitted output '{}'", port.id)
                    })?;
                    mutation_value_from_v8(scope, value, port).with_context(|| {
                        format!(
                            "Mutation Function '{node_id}.{}' returned a value incompatible with '{}'",
                            port.id, port.port_type
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()
        })();
        let timed_out = SharedWatchdog::global().disarm(&self.watchdog_slot);
        let terminating = self.runtime.v8_isolate().is_execution_terminating();
        if timed_out || terminating {
            self.runtime.v8_isolate().cancel_terminate_execution();
            bail!("Mutation Function exceeded the Mutation graph frame budget");
        }
        result
    }
}

fn persistent_string(
    scope: &mut v8::PinScope<'_, '_>,
    value: &str,
) -> Result<v8::Global<v8::String>> {
    v8::String::new(scope, value)
        .map(|value| v8::Global::new(scope, value))
        .ok_or_else(|| anyhow!("Mutation Function port name is too large"))
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
        .ok_or_else(|| anyhow!("failed to inspect Mutation Function object"))?;
    if !value.is_function() {
        let keys = object
            .get_own_property_names(scope, Default::default())
            .ok_or_else(|| anyhow!("failed to enumerate Mutation Function object"))?;
        for index in 0..keys.length() {
            let key = keys
                .get_index(scope, index)
                .ok_or_else(|| anyhow!("failed to read Mutation Function object key"))?;
            let child = object
                .get(scope, key)
                .ok_or_else(|| anyhow!("failed to read Mutation Function object value"))?;
            deep_freeze(scope, child)?;
        }
    }
    if object.set_integrity_level(scope, v8::IntegrityLevel::Frozen) != Some(true) {
        bail!("failed to freeze Mutation Function object");
    }
    Ok(())
}

fn mutation_value_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: &MutationValue,
) -> Result<v8::Local<'s, v8::Value>> {
    let value = match value {
        MutationValue::Float(value) => v8::Number::new(scope, *value).into(),
        MutationValue::Int(value) => v8::Number::new(scope, *value as f64).into(),
        MutationValue::Bool(value) => v8::Boolean::new(scope, *value).into(),
        MutationValue::Vec2(values) => numeric_array(scope, values)?.into(),
        MutationValue::Vec3(values) => numeric_array(scope, values)?.into(),
        MutationValue::Vec4(values) | MutationValue::Color(values) => {
            numeric_array(scope, values)?.into()
        }
        MutationValue::Packed(values) => {
            let array = v8::Array::new(scope, values.len() as i32);
            for (index, value) in values.iter().enumerate() {
                let value = mutation_value_to_v8(scope, value)?;
                if array.set_index(scope, index as u32, value) != Some(true) {
                    bail!("failed to construct packed Mutation Function input");
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
            bail!("failed to construct vector Mutation Function input");
        }
    }
    Ok(array)
}

fn mutation_value_from_v8(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<'_, v8::Value>,
    port: &ReflectedPort,
) -> Result<MutationValue> {
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
            values.push(mutation_value_from_type(scope, item, element_type)?);
        }
        return Ok(MutationValue::Packed(values));
    }
    mutation_value_from_type(scope, value, &port.port_type)
}

fn mutation_value_from_type(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<'_, v8::Value>,
    port_type: &str,
) -> Result<MutationValue> {
    match port_type {
        "float" => finite_number(scope, value).map(MutationValue::Float),
        "int" => {
            let value = finite_number(scope, value)?;
            if value.fract() != 0.0 || value < i64::MIN as f64 || value > i64::MAX as f64 {
                bail!("expected integer");
            }
            Ok(MutationValue::Int(value as i64))
        }
        "bool" => {
            if !value.is_boolean() {
                bail!("expected boolean");
            }
            Ok(MutationValue::Bool(value.boolean_value(scope)))
        }
        "vector2" => Ok(MutationValue::Vec2(numeric_tuple(scope, value)?)),
        "vector3" => Ok(MutationValue::Vec3(numeric_tuple(scope, value)?)),
        "vector4" => Ok(MutationValue::Vec4(numeric_tuple(scope, value)?)),
        "color" => Ok(MutationValue::Color(numeric_tuple(scope, value)?)),
        other => bail!("unsupported Mutation Function port type '{other}'"),
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
    static FUNCTION_RUNTIME: RefCell<Option<MutationJsRuntime>> = const { RefCell::new(None) };
}

pub fn prepare(mutation_id: &str, node_id: &str) -> Result<()> {
    let resource = function_for(mutation_id, node_id)?;
    let generation = registry_generation().load(Ordering::Acquire);
    FUNCTION_RUNTIME.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        if runtime
            .as_ref()
            .is_none_or(|runtime| runtime.generation != generation)
        {
            *runtime = Some(MutationJsRuntime::new(generation));
        }
        runtime
            .as_mut()
            .expect("runtime inserted above")
            .prepare(&resource)
    })
}

pub fn prepare_state_machine(state_machine: &StateMachine) -> Result<()> {
    for mutation in &state_machine.mutations {
        for node in &mutation.nodes {
            if node.node_type == MutationInnerNodeType::MutationFunction {
                prepare(&mutation.id, &node.id)?;
            }
        }
    }
    Ok(())
}

pub fn evaluate(
    mutation_id: &str,
    node_id: &str,
    inputs: &[MutationValue],
    remaining_budget: Duration,
) -> Result<Vec<MutationValue>> {
    FUNCTION_RUNTIME.with(|runtime| {
        runtime
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| {
                anyhow!(
                    "Mutation Function runtime was not prepared before evaluating 'mutation:{mutation_id}/{node_id}'"
                )
            })?
            .evaluate(mutation_id, node_id, inputs, remaining_budget)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn resource(
        source: &str,
        body: &str,
        inputs: Vec<ReflectedPort>,
        outputs: Vec<ReflectedPort>,
    ) -> FunctionResource {
        let compiled_java_script = format!(
            r#"(() => {{
              "use strict";
              const module = {{ exports: {{}} }};
              const exports = module.exports;
              {body}
              return {{ entry: module.exports.default, bindings: [mutation] }};
            }})()"#
        );
        FunctionResource {
            scope: "mutation:test".to_string(),
            node_id: "function".to_string(),
            language: "typescript".to_string(),
            source: source.to_string(),
            compiled_java_script,
            source_hash: format!("{:x}", Sha256::digest(source.as_bytes())),
            abi_version: MUTATION_FUNCTION_ABI_VERSION,
            inputs,
            outputs,
        }
    }

    fn port(id: &str, port_type: &str) -> ReflectedPort {
        ReflectedPort {
            id: id.into(),
            name: id.into(),
            port_type: port_type.into(),
            array_length: None,
        }
    }

    fn packed_port(id: &str, port_type: &str, length: usize) -> ReflectedPort {
        ReflectedPort {
            array_length: Some(length),
            ..port(id, port_type)
        }
    }

    #[test]
    fn reuses_context_and_recovers_after_timeout() {
        let _guard = test_lock();
        install_document_functions([resource(
            "conditional timeout",
            "Object.defineProperty(exports, \"__esModule\", { value: true }); exports.default = mutation; function mutation(input) { if (input.hang) { while (true) {} } return { value: input.value * 2 }; }",
            vec![port("hang", "bool"), port("value", "float")],
            vec![port("value", "float")],
        )])
        .expect("install function");
        prepare("test", "function").expect("prepare function");
        let lifecycle_counts = (
            TEST_CONTEXT_CREATIONS.load(Ordering::Relaxed),
            TEST_SCRIPT_COMPILATIONS.load(Ordering::Relaxed),
            TEST_WATCHDOG_THREADS.load(Ordering::Relaxed),
        );
        assert_eq!(
            evaluate(
                "test",
                "function",
                &[MutationValue::Bool(false), MutationValue::Float(3.0)],
                Duration::from_millis(20),
            )
            .expect("evaluate function"),
            vec![MutationValue::Float(6.0)]
        );

        let error = evaluate(
            "test",
            "function",
            &[MutationValue::Bool(true), MutationValue::Float(3.0)],
            Duration::from_millis(4),
        )
        .expect_err("infinite loop must time out");
        assert!(
            error.to_string().contains("Mutation graph frame budget"),
            "unexpected timeout error: {error:#}"
        );

        assert_eq!(
            evaluate(
                "test",
                "function",
                &[MutationValue::Bool(false), MutationValue::Float(4.0)],
                Duration::from_millis(20),
            )
            .expect("same context recovers"),
            vec![MutationValue::Float(8.0)]
        );
        assert_eq!(
            lifecycle_counts,
            (
                TEST_CONTEXT_CREATIONS.load(Ordering::Relaxed),
                TEST_SCRIPT_COMPILATIONS.load(Ordering::Relaxed),
                TEST_WATCHDOG_THREADS.load(Ordering::Relaxed),
            ),
            "hot calls must not create contexts, compile scripts, or spawn watchdog threads"
        );
    }

    #[test]
    fn replay_is_deterministic_and_module_bindings_are_frozen() {
        let _guard = test_lock();
        install_document_functions([resource(
            "frozen module state",
            "Object.defineProperty(exports, \"__esModule\", { value: true }); exports.default = mutation; const state = { value: 0 }; function mutation(input) { if (input.mutate) state.value += 1; return { value: input.value + state.value }; }",
            vec![port("mutate", "bool"), port("value", "float")],
            vec![port("value", "float")],
        )])
        .expect("install function");
        // Include state in the ABI binding list for this hand-authored fixture.
        {
            let mut functions = registry().write().unwrap();
            let function = functions.get_mut("mutation:test/function").unwrap();
            function.compiled_java_script = function
                .compiled_java_script
                .replace("bindings: [mutation]", "bindings: [state, mutation]");
        }
        registry_generation().fetch_add(1, Ordering::AcqRel);
        prepare("test", "function").expect("prepare function");

        let a = [MutationValue::Bool(false), MutationValue::Float(7.0)];
        let b = [MutationValue::Bool(false), MutationValue::Float(11.0)];
        let first_a = evaluate("test", "function", &a, Duration::from_millis(20)).expect("first A");
        let _ = evaluate("test", "function", &b, Duration::from_millis(20)).expect("B");
        let second_a =
            evaluate("test", "function", &a, Duration::from_millis(20)).expect("second A");
        assert_eq!(first_a, second_a);

        let error = evaluate(
            "test",
            "function",
            &[MutationValue::Bool(true), MutationValue::Float(7.0)],
            Duration::from_millis(20),
        )
        .expect_err("frozen module state must reject mutation");
        assert!(error.to_string().contains("execution failed"));
    }

    #[test]
    fn typed_bridge_preserves_semantic_values_and_freezes_inputs() {
        let _guard = test_lock();
        let ports = vec![
            port("floatValue", "float"),
            port("intValue", "int"),
            port("boolValue", "bool"),
            port("vec2Value", "vector2"),
            port("vec3Value", "vector3"),
            port("vec4Value", "vector4"),
            port("colorValue", "color"),
            packed_port("packedValue", "packed<vector2>", 2),
        ];
        install_document_functions([resource(
            "typed identity",
            "Object.defineProperty(exports, \"__esModule\", { value: true }); exports.default = mutation; function mutation(input) { return input; }",
            ports.clone(),
            ports,
        )])
        .expect("install function");
        prepare("test", "function").expect("prepare function");
        let values = vec![
            MutationValue::Float(1.25),
            MutationValue::Int(7),
            MutationValue::Bool(true),
            MutationValue::Vec2([1.0, 2.0]),
            MutationValue::Vec3([1.0, 2.0, 3.0]),
            MutationValue::Vec4([1.0, 2.0, 3.0, 4.0]),
            MutationValue::Color([0.1, 0.2, 0.3, 1.0]),
            MutationValue::Packed(vec![
                MutationValue::Vec2([5.0, 6.0]),
                MutationValue::Vec2([7.0, 8.0]),
            ]),
        ];
        assert_eq!(
            evaluate("test", "function", &values, Duration::from_millis(20))
                .expect("typed round trip"),
            values
        );

        install_document_functions([resource(
            "input mutation",
            "Object.defineProperty(exports, \"__esModule\", { value: true }); exports.default = mutation; function mutation(input) { input.value[0] = 9; return { value: input.value }; }",
            vec![port("value", "vector2")],
            vec![port("value", "vector2")],
        )])
        .expect("install mutation attempt");
        prepare("test", "function").expect("prepare mutation attempt");
        let error = evaluate(
            "test",
            "function",
            &[MutationValue::Vec2([1.0, 2.0])],
            Duration::from_millis(20),
        )
        .expect_err("input arrays must be frozen");
        assert!(error.to_string().contains("execution failed"));
    }

    #[test]
    #[ignore = "manual release-mode latency report"]
    fn reports_cached_rust_v8_rust_latency() {
        let _guard = test_lock();
        install_document_functions([resource(
            "latency report",
            "Object.defineProperty(exports, \"__esModule\", { value: true }); exports.default = mutation; function mutation(input) { return { value: input.value * 2 }; }",
            vec![port("value", "float")],
            vec![port("value", "float")],
        )])
        .expect("install function");
        prepare("test", "function").expect("prepare function");
        let input = [MutationValue::Float(3.0)];
        for _ in 0..1_000 {
            std::hint::black_box(
                evaluate("test", "function", &input, Duration::from_millis(20)).expect("warm call"),
            );
        }
        let mut samples = Vec::with_capacity(10_000);
        for _ in 0..10_000 {
            let started = Instant::now();
            std::hint::black_box(
                evaluate("test", "function", &input, Duration::from_millis(20))
                    .expect("measured call"),
            );
            samples.push(started.elapsed().as_nanos() as u64);
        }
        samples.sort_unstable();
        let percentile = |percent: usize| samples[(samples.len() - 1) * percent / 100];
        eprintln!(
            "cached Rust→V8→Rust scalar latency: p50={}ns p95={}ns p99={}ns",
            percentile(50),
            percentile(95),
            percentile(99)
        );
    }
}
