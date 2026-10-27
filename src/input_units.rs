//! Authoring units belong to inline values. Edges and boundary bindings carry evaluated values.
use std::collections::HashMap;

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::dsl::{Node, SceneDSL};

pub fn convert_value(value: &Value, unit: &Value, dpi: f64, integer: bool) -> Result<Value> {
    if matches!(unit.as_str(), Some("none" | "px")) {
        return Ok(value.clone());
    }
    if let Some(values) = value.as_array() {
        if let Some(units) = unit.as_array() {
            ensure!(units.len() == values.len(), "vector unit count must match its components");
        }
        return values.iter().enumerate().map(|(index, value)| {
            convert_value(value, unit.as_array().map_or(unit, |units| &units[index]), dpi, integer)
        }).collect::<Result<Vec<_>>>().map(Value::Array);
    }
    if let Some(object) = value.as_object() {
        let keys: Vec<_> = ["x", "y", "z", "w"].into_iter().filter(|key| object.contains_key(*key)).collect();
        ensure!(keys.len() == object.len() && (2..=4).contains(&keys.len()), "units require a numeric vector");
        if let Some(units) = unit.as_array() {
            ensure!(units.len() == keys.len(), "vector unit count must match its components");
        }
        let mut result = object.clone();
        for (index, key) in keys.iter().enumerate() {
            result.insert((*key).into(), convert_value(&object[*key], unit.as_array().map_or(unit, |units| &units[index]), dpi, false)?);
        }
        return Ok(Value::Object(result));
    }
    ensure!(unit.as_str() == Some("dp"), "input unit must be none, px or dp");
    let value = value.as_f64().context("dp requires a finite numeric value")?;
    let pixels = crate::dsl::dp_to_px(value, dpi, false)?;
    if integer {
        let pixels = pixels.round();
        ensure!(pixels >= i32::MIN as f64 && pixels <= i32::MAX as f64, "integer dp result exceeds i32");
        Ok(serde_json::json!(pixels as i32))
    } else {
        Ok(serde_json::json!(pixels))
    }
}

pub fn inline_value(node: &Node, key: &str, value: &Value, dpi: f64) -> Result<Value> {
    let Some(units) = node.params.get("inputUnits") else { return Ok(value.clone()) };
    let units = units.as_object().context("inputUnits must be an object")?;
    let Some(unit) = units.get(key) else { return Ok(value.clone()) };
    if node.node_type == "IntInput" && key == "value" && unit.as_str() == Some("dp") {
        crate::dsl::dp_to_px(value.as_f64().context("integer dp requires a number")?, dpi, true)?;
    }
    let integer = node.node_type == "IntInput" && key == "value"
        || node.inputs.iter().any(|port| port.id == key && port.port_type.as_deref() == Some("int"))
        || crate::schema::load_default_scheme()?.nodes.get(&node.node_type)
            .and_then(|definition| definition.inputs.get(key))
            .is_some_and(|ty| matches!(ty, crate::schema::PortTypeSpec::One(ty) if ty == "int"));
    convert_value(value, unit, dpi, integer)
}

/// Resolve a runtime copy, leaving the authored document and its unit selections intact.
pub fn materialize_node(node: &mut Node, dpi: f64, connected: impl Fn(&str) -> bool) -> Result<()> {
    let Some(units) = node.params.get("inputUnits").cloned() else { return Ok(()) };
    let units = units.as_object().context("inputUnits must be an object")?;
    let scheme = crate::schema::load_default_scheme()?;
    for key in units.keys() {
        if connected(key) { continue; }
        let value = node.params.get(key).cloned().or_else(||
            crate::schema::input_port_default_value(&scheme, &node.node_type, key).cloned());
        let Some(value) = value else { bail!("missing inline value {}.{key}", node.id) };
        node.params.insert(key.clone(), inline_value(node, key, &value, dpi)?);
    }
    node.params.remove("inputUnits");
    Ok(())
}

pub fn materialize_scene(scene: &mut SceneDSL) -> Result<()> {
    let connections = &scene.connections;
    for node in &mut scene.nodes {
        let id = node.id.clone();
        materialize_node(node, scene.dpi, |key| connections.iter().any(|edge| edge.to.node_id == id && edge.to.port_id == key))?;
    }
    Ok(())
}

pub fn inner_params(params: &HashMap<String, Value>, key: &str, dpi: f64, integer: bool) -> Result<Option<Value>> {
    let Some(value) = params.get(key) else { return Ok(None) };
    let value = match params.get("inputUnits").and_then(|units| units.get(key)) {
        Some(unit) => convert_value(value, unit, dpi, integer)?,
        None => value.clone(),
    };
    Ok(Some(value))
}
