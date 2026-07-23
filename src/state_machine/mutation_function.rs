use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc, OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use deno_core::{JsRuntime, RuntimeOptions, ascii_str};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MUTATION_FUNCTION_ABI_VERSION: u32 = 1;
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
    FUNCTION_RUNTIMES.with(|runtimes| runtimes.borrow_mut().clear());
    Ok(())
}

pub fn clear_document_functions() {
    if let Ok(mut functions) = registry().write() {
        functions.clear();
    }
    FUNCTION_RUNTIMES.with(|runtimes| runtimes.borrow_mut().clear());
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

struct FunctionJsRuntime {
    initializer: String,
}

impl FunctionJsRuntime {
    fn new(resource: &FunctionResource) -> Result<Self> {
        let initializer = format!(
            r#"
            globalThis.__nodeForgeDeepFreeze = function deepFreeze(value) {{
              if (value && typeof value === 'object' && !Object.isFrozen(value)) {{
                Object.freeze(value);
                for (const key of Object.keys(value)) deepFreeze(value[key]);
              }}
              return value;
            }};
            globalThis.__nodeForgeMutationFactory = () => {{
              const module = {{ exports: {{}} }};
              const exports = module.exports;
              {compiled}
              if (typeof module.exports.default !== 'function') {{
                throw new TypeError('compiled Mutation Function has no default function');
              }}
              return module.exports.default;
            }};
            "#,
            compiled = resource.compiled_java_script
        );
        let mut runtime = JsRuntime::new(RuntimeOptions::default());
        runtime
            .execute_script(ascii_str!("<mutation-function-init>"), initializer.clone())
            .with_context(|| {
                format!(
                    "failed to initialize Mutation Function '{}/{}'",
                    resource.scope, resource.node_id
                )
            })?;
        Ok(Self { initializer })
    }

    fn evaluate(
        &mut self,
        input: &serde_json::Value,
        remaining_budget: Duration,
    ) -> Result<serde_json::Value> {
        if remaining_budget.is_zero() {
            bail!("Mutation Function exceeded the Mutation graph frame budget");
        }
        // A fresh JavaScript realm is created for every frame. This prevents
        // module locals, closures, and globalThis writes from becoming hidden
        // cross-frame Mutation state.
        let mut runtime = JsRuntime::new(RuntimeOptions::default());
        runtime
            .execute_script(
                ascii_str!("<mutation-function-init>"),
                self.initializer.clone(),
            )
            .context("failed to initialize Mutation Function frame")?;
        let input = serde_json::to_string(input)?;
        let script = format!(
            "globalThis.__nodeForgeMutationFactory()(globalThis.__nodeForgeDeepFreeze({input}))"
        );
        let isolate_handle = runtime.v8_isolate().thread_safe_handle();
        let (cancel_tx, cancel_rx) = mpsc::channel();
        let timed_out = Arc::new(AtomicBool::new(false));
        let timed_out_in_thread = Arc::clone(&timed_out);
        let terminator = std::thread::spawn(move || {
            if cancel_rx.recv_timeout(remaining_budget).is_err() {
                timed_out_in_thread.store(true, Ordering::Release);
                isolate_handle.terminate_execution();
            }
        });
        let result = runtime.execute_script(ascii_str!("<mutation-function-frame>"), script);
        let _ = cancel_tx.send(());
        let _ = terminator.join();

        let value = match result {
            Ok(value) => value,
            Err(error) => {
                if timed_out.load(Ordering::Acquire) {
                    runtime.v8_isolate().cancel_terminate_execution();
                    bail!("Mutation Function exceeded the Mutation graph frame budget");
                }
                return Err(anyhow!("Mutation Function execution failed: {error:?}"));
            }
        };
        deno_core::scope!(scope, runtime);
        let local = deno_core::v8::Local::new(scope, value);
        deno_core::serde_v8::from_v8(scope, local)
            .map_err(|error| anyhow!("Mutation Function returned an invalid value: {error:?}"))
    }
}

thread_local! {
    static FUNCTION_RUNTIMES: RefCell<HashMap<String, FunctionJsRuntime>> = RefCell::new(HashMap::new());
}

pub fn prepare(mutation_id: &str, node_id: &str) -> Result<()> {
    let resource = function_for(mutation_id, node_id)?;
    FUNCTION_RUNTIMES.with(|runtimes| {
        let mut runtimes = runtimes.borrow_mut();
        if !runtimes.contains_key(&resource.source_hash) {
            runtimes.insert(
                resource.source_hash.clone(),
                FunctionJsRuntime::new(&resource)?,
            );
        }
        Ok(())
    })
}

pub fn evaluate(
    mutation_id: &str,
    node_id: &str,
    input: &serde_json::Value,
    remaining_budget: Duration,
) -> Result<serde_json::Value> {
    let resource = function_for(mutation_id, node_id)?;
    prepare(mutation_id, node_id)?;
    FUNCTION_RUNTIMES.with(|runtimes| {
        let mut runtimes = runtimes.borrow_mut();
        runtimes
            .get_mut(&resource.source_hash)
            .expect("runtime inserted above")
            .evaluate(input, remaining_budget)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(source: &str, compiled_java_script: &str) -> FunctionResource {
        FunctionResource {
            scope: "mutation:test".to_string(),
            node_id: "function".to_string(),
            language: "typescript".to_string(),
            source: source.to_string(),
            compiled_java_script: compiled_java_script.to_string(),
            source_hash: format!("{:x}", Sha256::digest(source.as_bytes())),
            abi_version: 1,
            inputs: vec![],
            outputs: vec![],
        }
    }

    #[test]
    fn executes_compiled_function_and_recovers_after_timeout() {
        install_document_functions([resource(
            "export default function mutation(input: { value: number }): { value: number } { return { value: input.value * 2 }; }",
            "Object.defineProperty(exports, \"__esModule\", { value: true }); exports.default = mutation; function mutation(input) { return { value: input.value * 2 }; }",
        )])
        .expect("install function");
        assert_eq!(
            evaluate(
                "test",
                "function",
                &serde_json::json!({"value": 3.0}),
                Duration::from_millis(4),
            )
            .expect("evaluate function"),
            serde_json::json!({"value": 6})
        );

        install_document_functions([resource(
            "export default function mutation(): { value: number } { while (true) {} }",
            "Object.defineProperty(exports, \"__esModule\", { value: true }); exports.default = mutation; function mutation() { while (true) {} }",
        )])
        .expect("install timeout function");
        let error = evaluate(
            "test",
            "function",
            &serde_json::json!({}),
            Duration::from_millis(4),
        )
        .expect_err("infinite loop must time out");
        assert!(
            error.to_string().contains("Mutation graph frame budget"),
            "unexpected timeout error: {error:#}"
        );

        install_document_functions([resource(
            "export default function mutation(): { value: number } { return { value: 7 }; }",
            "Object.defineProperty(exports, \"__esModule\", { value: true }); exports.default = mutation; function mutation() { return { value: 7 }; }",
        )])
        .expect("install recovery function");
        assert_eq!(
            evaluate(
                "test",
                "function",
                &serde_json::json!({}),
                Duration::from_millis(4),
            )
            .expect("runtime recovers"),
            serde_json::json!({"value": 7})
        );

        install_document_functions([resource(
            "export default function mutation(): { value: number } { return { value: 1 }; }",
            "globalThis.hiddenCounter = (globalThis.hiddenCounter ?? 0) + 1; Object.defineProperty(exports, \"__esModule\", { value: true }); exports.default = mutation; function mutation() { return { value: globalThis.hiddenCounter }; }",
        )])
        .expect("install function");

        for _ in 0..2 {
            assert_eq!(
                evaluate(
                    "test",
                    "function",
                    &serde_json::json!({}),
                    Duration::from_millis(20),
                )
                .expect("evaluate stateless frame"),
                serde_json::json!({"value": 1})
            );
        }
    }
}
