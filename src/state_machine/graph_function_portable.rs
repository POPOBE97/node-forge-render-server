//! Function ABI v10 on QuickJS, with the same Rust MotionEngine and schema validation.
use std::{cell::RefCell, collections::HashMap, time::Duration};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::{dsl::SceneDSL, portable_js::PortableJs};
use super::{graph::GraphValue, motion::{MutationPlan, MutationTiming}, types::{GraphInnerNodeType, StateMachine}};
pub use super::graph_function_contract::{FunctionResource, ReflectedPort, validate_function_resources};
use super::graph_function_contract::resource_key;

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionOutput { Value(GraphValue), Motion(MutationPlan) }

struct PreparedFunction { resource: FunctionResource, runtime: PortableJs }
thread_local! {
    static RESOURCES: RefCell<HashMap<String, FunctionResource>> = RefCell::new(HashMap::new());
    static PREPARED: RefCell<HashMap<String, PreparedFunction>> = RefCell::new(HashMap::new());
}

pub fn install_document_functions(functions: impl IntoIterator<Item = FunctionResource>) -> Result<()> {
    let mut next = HashMap::new();
    for function in functions {
        function.validate_stored_artifact()?;
        let key = resource_key(&function.scope, &function.node_id);
        if next.insert(key.clone(), function).is_some() { bail!("duplicate Graph Function resource '{key}'"); }
    }
    PREPARED.with(|prepared| prepared.borrow_mut().retain(|key, prepared| {
        next.get(key).is_some_and(|resource| serde_json::to_value(resource).ok() == serde_json::to_value(&prepared.resource).ok())
    }));
    RESOURCES.with(|resources| *resources.borrow_mut() = next);
    Ok(())
}

pub fn clear_document_functions() {
    PREPARED.with(|prepared| prepared.borrow_mut().clear());
    RESOURCES.with(|resources| resources.borrow_mut().clear());
}

pub fn installed_document_functions() -> Vec<FunctionResource> {
    RESOURCES.with(|resources| resources.borrow().values().cloned().collect())
}

pub fn prepare(graph_scope: &str, node_id: &str) -> Result<()> {
    let key = resource_key(graph_scope, node_id);
    if PREPARED.with(|prepared| prepared.borrow().contains_key(&key)) { return Ok(()); }
    let resource = RESOURCES.with(|resources| resources.borrow().get(&key).cloned())
        .ok_or_else(|| anyhow!("Graph Function resource '{key}' is not installed"))?;
    let runtime = PortableJs::new()?;
    let source = format!("{INSTALL_PREFIX}\nconst installed = {};\nconst ports = {};\n{INSTALL_SUFFIX}",
        resource.compiled_java_script, serde_json::to_string(&resource.outputs)?);
    runtime.install(&source).with_context(|| format!("Graph Function '{key}' installation"))?;
    PREPARED.with(|prepared| prepared.borrow_mut().insert(key, PreparedFunction { resource, runtime }));
    Ok(())
}

pub fn prepare_state_machine(machine: &StateMachine) -> Result<()> {
    for derivation in &machine.derivations {
        for node in &derivation.nodes {
            if node.node_type == GraphInnerNodeType::DerivationFunction { prepare(&format!("derivation:{}", derivation.id), &node.id)?; }
        }
    }
    for state in &machine.states {
        if let Some(graph) = &state.mutation_graph {
            for node in &graph.nodes {
                if node.node_type == GraphInnerNodeType::MutationFunction { prepare(&format!("state:{}", state.id), &node.id)?; }
            }
        }
    }
    Ok(())
}

pub fn evaluate_function(scope: &str, node: &str, inputs: &[GraphValue], budget: Duration) -> Result<Vec<FunctionOutput>> {
    let key = resource_key(scope, node);
    PREPARED.with(|prepared| {
        let prepared = prepared.borrow();
        let function = prepared.get(&key).ok_or_else(|| anyhow!("Graph Function '{key}' was not prepared"))?;
        if function.resource.inputs.len() != inputs.len() { bail!("Graph Function '{key}' input count mismatch"); }
        let values: serde_json::Map<String, Value> = function.resource.inputs.iter().zip(inputs)
            .map(|(port, value)| (port.id.clone(), value.to_json())).collect();
        let json = function.runtime.call(&serde_json::to_string(&values)?, budget)
            .with_context(|| format!("Graph Function '{key}'"))?;
        let outputs: Vec<Value> = serde_json::from_str(&json)?;
        if outputs.len() != function.resource.outputs.len() { bail!("Graph Function output count mismatch"); }
        outputs.iter().zip(&function.resource.outputs).map(|(value, port)| {
            if port.motion == Some(true) {
                let plan = motion_from_json(value, port, 0, &mut 0)?;
                plan.validate()?;
                Ok(FunctionOutput::Motion(plan))
            } else { typed_value(value, port).map(FunctionOutput::Value) }
        }).collect()
    })
}

fn typed_value(value: &Value, port: &ReflectedPort) -> Result<GraphValue> {
    fn check(value: &Value, kind: &str) -> Result<()> {
        if let Some(element) = kind.strip_prefix("packed<").and_then(|kind| kind.strip_suffix('>')) {
            for value in value.as_array().context("expected packed array")? { check(value, element)?; }
            return Ok(());
        }
        match kind {
            "float" | "int" => {
                let number = value.as_f64().filter(|number| number.is_finite()).context("expected finite number")?;
                if kind == "int" && (number.fract() != 0.0 || number < i64::MIN as f64 || number >= -(i64::MIN as f64)) { bail!("expected integer"); }
            }
            "bool" => { if !value.is_boolean() { bail!("expected boolean"); } }
            "vector2" | "vector3" | "vector4" | "color" | "normalizedBezierCurve" | "bezierCurve" => {
                let array = value.as_array().context("expected vector array")?;
                let length = match kind { "vector2" => 2, "vector3" => 3, _ => 4 };
                if array.len() != length { bail!("incorrect vector length"); }
                for item in array { check(item, if kind == "bezierCurve" { "vector2" } else { "float" })?; }
            }
            _ => bail!("unsupported Function port type '{kind}'"),
        }
        Ok(())
    }
    check(value, &port.port_type)?;
    if let Some(length) = port.array_length {
        if value.as_array().is_none_or(|values| values.len() != length) { bail!("packed output length mismatch"); }
    }
    // JSON numbers emitted by JavaScript may be encoded with a decimal suffix.
    fn normalize_int(value: &Value, kind: &str) -> Value {
        if kind == "int" { return Value::from(value.as_f64().unwrap() as i64); }
        if let Some(element) = kind.strip_prefix("packed<").and_then(|kind| kind.strip_suffix('>')) {
            return Value::Array(value.as_array().unwrap().iter().map(|value| normalize_int(value, element)).collect());
        }
        value.clone()
    }
    GraphValue::from_json_typed(&normalize_int(value, &port.port_type), Some(&port.port_type))
        .ok_or_else(|| anyhow!("Graph Function output '{}' type mismatch", port.id))
}

fn motion_from_json(value: &Value, port: &ReflectedPort, depth: usize, nodes: &mut usize) -> Result<MutationPlan> {
    *nodes += 1;
    if depth > 64 || *nodes > 4096 { bail!("Motion plan exceeds complexity limit"); }
    let number = |key: &str| value[key].as_f64().filter(|v| v.is_finite()).with_context(|| format!("Motion '{key}' must be finite"));
    Ok(match value["kind"].as_str().context("Motion helper identity missing")? {
        "setTo" => MutationPlan::SetTo {
            target: typed_value(&value["target"], port)?.to_json(),
            velocity: value.get("velocity").map(|value| typed_value(value, port).map(|v| v.to_json())).transpose()?,
        },
        "to" => MutationPlan::To {
            target: typed_value(&value["target"], port)?.to_json(),
            timing: match value["timing"].as_str() {
                Some("linear") => MutationTiming::Linear { duration: number("duration")?, delay: number("timingDelay")? },
                Some("spring") => MutationTiming::Spring { duration: number("duration")?, delay: number("timingDelay")?, bounce: number("bounce")? },
                _ => bail!("unknown Motion timing"),
            },
        },
        "sequence" => MutationPlan::Sequence(value["children"].as_array().context("sequence children missing")?.iter()
            .map(|child| motion_from_json(child, port, depth + 1, nodes)).collect::<Result<_>>()?),
        "delay" => MutationPlan::Delay { child: Box::new(motion_from_json(&value["child"], port, depth + 1, nodes)?), delay: number("seconds")? },
        "repeat" => {
            let count = number("count")?;
            if count.fract() != 0.0 || count < -1.0 || count >= -(i64::MIN as f64) { bail!("invalid repeat count"); }
            MutationPlan::Repeat { child: Box::new(motion_from_json(&value["child"], port, depth + 1, nodes)?), count: count as i64 }
        }
        _ => bail!("unknown Motion helper"),
    })
}

const INSTALL_PREFIX: &str = "(() => { 'use strict';";
const INSTALL_SUFFIX: &str = r#"
if (!installed || typeof installed.entry !== 'function' || !Array.isArray(installed.bindings))
    throw new Error('Invalid Function ABI v10 installer');
if (ports.some(p => p.motion) && typeof installed.motionKind !== 'symbol')
    throw new Error('Missing Motion symbol');
function freeze(value, seen = new Set()) {
    if (value === null || (typeof value !== 'object' && typeof value !== 'function') || seen.has(value)) return value;
    seen.add(value);
    if (typeof value !== 'function') for (const key of Object.getOwnPropertyNames(value)) freeze(value[key], seen);
    return Object.freeze(value);
}
for (const binding of installed.bindings) freeze(binding);
function motion(value, depth, count) {
    if (depth > 64 || ++count.value > 4096) throw new Error('Motion complexity limit');
    if (!value || typeof value !== 'object') throw new Error('Expected Motion');
    const kind = value[installed.motionKind];
    switch (kind) {
      case 'setTo': return {kind, target:value.target, velocity:value.velocity};
      case 'to': return {kind, target:value.target, timing:value.timing, duration:value.duration, timingDelay:value.timingDelay, bounce:value.bounce};
      case 'sequence': return {kind, children:value.children.map(child => motion(child, depth+1, count))};
      case 'repeat': return {kind, child:motion(value.child, depth+1, count), count:value.count};
      case 'delay': return {kind, child:motion(value.child, depth+1, count), seconds:value.seconds};
      default: throw new Error('Motion output was not created by a Motion helper');
    }
}
const stringify = JSON.stringify.bind(JSON), parse = JSON.parse.bind(JSON);
Object.defineProperty(globalThis, '__nodeForgeInvoke', {value: input => {
    const result = installed.entry(freeze(parse(input)));
    if (!result || typeof result !== 'object' || typeof result.then === 'function') throw new Error('Expected synchronous Function output object');
    return stringify(ports.map(port => {
        if (!(port.id in result)) throw new Error('Missing output '+port.id);
        return port.motion ? motion(result[port.id], 0, {value:0}) : result[port.id];
    }), (_, value) => {
        if (typeof value === 'number' && !Number.isFinite(value)) throw new Error('Non-finite Function output');
        return value;
    });
}, writable:false, configurable:false});
Object.defineProperty(Math, 'random', {value:undefined, writable:false, configurable:false});
Object.defineProperty(Function.prototype, 'constructor', {value:undefined, writable:false, configurable:false});
for (const value of [Math, JSON, Object.prototype, Array.prototype, Number.prototype, Boolean.prototype, String.prototype, Function.prototype]) Object.freeze(value);
for (const name of ['Date','eval','Function','WebAssembly','fetch','Deno','setTimeout','setInterval','queueMicrotask','crypto','performance'])
    Object.defineProperty(globalThis, name, {value:undefined, writable:false, configurable:false});
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn install(source: &str, kind: &str, port_type: &str, motion: bool) {
        let scope = if kind == "mutation" { "state:test" } else { "derivation:test" };
        let resource = FunctionResource {
            scope: scope.into(), node_id: "fn".into(), kind: kind.into(), language: "typescript".into(),
            source: source.into(), source_hash: format!("{:x}", Sha256::digest(source.as_bytes())),
            compiled_java_script: source.into(), abi_version: 10,
            inputs: vec![ReflectedPort { id:"value".into(), name:"value".into(), port_type:port_type.into(), array_length:None, motion:None }],
            outputs: vec![ReflectedPort { id:"value".into(), name:"value".into(), port_type:port_type.into(), array_length:None, motion:Some(motion) }],
        };
        install_document_functions([resource]).unwrap(); prepare(scope,"fn").unwrap();
    }

    #[test]
    fn hdr_values_and_composed_motion_survive_portable_js() {
        install("(() => ({bindings:[],entry:input=>input}))()", "derivation", "color", false);
        let color = GraphValue::Color([4.0,1.5,0.25,0.6]);
        assert_eq!(evaluate_function("derivation:test","fn",&[color.clone()],Duration::from_millis(100)).unwrap(),vec![FunctionOutput::Value(color.clone())]);
        install("(() => {const motionKind=Symbol();return {motionKind,bindings:[],entry:input=>({value:{[motionKind]:'repeat',count:3,child:{[motionKind]:'delay',seconds:0.25,child:{[motionKind]:'setTo',target:input.value}}}})}})()", "mutation", "color", true);
        assert_eq!(evaluate_function("state:test","fn",&[color.clone()],Duration::from_millis(100)).unwrap(),vec![FunctionOutput::Motion(MutationPlan::Repeat { count:3,child:Box::new(MutationPlan::Delay {delay:0.25,child:Box::new(MutationPlan::SetTo {target:color.to_json(),velocity:None})}) })]);
    }

    #[test]
    fn infinite_loop_times_out_and_next_call_recovers() {
        install("(() => ({bindings:[],entry(input){if(input.value>0)while(true){};return input;}}))()", "derivation", "float", false);
        let started=std::time::Instant::now();
        assert!(evaluate_function("derivation:test","fn",&[GraphValue::Float(1.0)],Duration::from_millis(5)).is_err());
        assert!(started.elapsed()<Duration::from_secs(1));
        assert!(evaluate_function("derivation:test","fn",&[GraphValue::Float(0.0)],Duration::from_millis(100)).is_ok());
    }

    #[test]
    fn inputs_bindings_and_output_types_are_enforced() {
        for source in [
            "(() => ({bindings:[],entry(input){input.value[0]=5;return input;}}))()",
            "(() => {const saved={n:0};return {bindings:[saved],entry(input){saved.n++;return input;}}})()",
            "(() => ({bindings:[],entry:()=>({value:[NaN,1,1,1]})}))()",
            "(() => ({bindings:[],entry:()=>({value:[1,2,3]})}))()",
        ] {
            install(source,"derivation","color",false);
            assert!(evaluate_function("derivation:test","fn",&[GraphValue::Color([4.0,1.0,0.0,1.0])],Duration::from_millis(100)).is_err(),"{source}");
        }
        install("(() => ({motionKind:Symbol(),bindings:[],entry:()=>({value:{kind:'setTo',target:1}})}))()","mutation","float",true);
        assert!(evaluate_function("state:test","fn",&[GraphValue::Float(1.0)],Duration::from_millis(100)).is_err());
    }
}
