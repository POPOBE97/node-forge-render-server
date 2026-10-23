//! Shared CPU text inputs for TextPass and TextSdf.
use super::{
    text::{FontSource, TextRequest, format_numbers},
    utils::cpu_num_f32,
};
use crate::{
    asset_store::AssetStore,
    dsl::{Node, SceneDSL, data_parse_output, evaluate_data_parse, incoming_connection},
};
use anyhow::{Context, Result, bail, ensure};
use std::collections::HashMap;

pub(crate) struct TextInputs<'a> {
    pub scene: &'a SceneDSL,
    pub nodes: &'a HashMap<String, Node>,
    pub asset_store: Option<&'a AssetStore>,
}
impl TextInputs<'_> {
    fn scene(&self) -> &SceneDSL {
        self.scene
    }
    fn nodes_by_id(&self) -> &HashMap<String, Node> {
        self.nodes
    }
}

pub(crate) fn vec2(sc: &TextInputs<'_>, node: &Node, port: &str) -> Result<Option<[f32; 2]>> {
    let computed;
    let value = if let Some(edge) = incoming_connection(sc.scene(), &node.id, port) {
        let source = sc
            .nodes_by_id()
            .get(&edge.from.node_id)
            .context("missing Text vector source")?;
        match source.node_type.as_str() {
            "Vector2Input" if edge.from.port_id == "vector" => {
                return Ok(Some([
                    cpu_num_f32(sc.scene(), sc.nodes_by_id(), source, "x", 0.0)?,
                    cpu_num_f32(sc.scene(), sc.nodes_by_id(), source, "y", 0.0)?,
                ]));
            }
            "DataParse" => {
                computed = evaluate_data_parse(
                    sc.scene(),
                    sc.nodes_by_id(),
                    source,
                    0,
                    &mut crate::renderer::scene_prep::data_parse_runtime::TsRuntime::new(),
                )?;
                data_parse_output(source, &computed, &edge.from.port_id)?
            }
            _ => bail!(
                "Text.{port} requires a CPU Vector2Input or DataParse, got {}.{}",
                source.node_type,
                edge.from.port_id
            ),
        }
    } else {
        let Some(value) = node.params.get(port) else {
            return Ok(None);
        };
        value
    };
    let xy = if let Some(a) = value.as_array() {
        ensure!(a.len() == 2, "Text.{port} requires exactly two components");
        [
            a.first().and_then(|v| v.as_f64()),
            a.get(1).and_then(|v| v.as_f64()),
        ]
    } else {
        [
            value.get("x").and_then(|v| v.as_f64()),
            value.get("y").and_then(|v| v.as_f64()),
        ]
    };
    let [Some(x), Some(y)] = xy else {
        bail!("Text.{port} requires [x,y] or {{x,y}}");
    };
    ensure!(x.is_finite() && y.is_finite(), "Text.{port} must be finite");
    Ok(Some([x as f32, y as f32]))
}

fn font(sc: &TextInputs<'_>, node: &Node, port: &str) -> Result<Option<FontSource>> {
    let Some(edge) = incoming_connection(sc.scene(), &node.id, port) else {
        return Ok(None);
    };
    let source = sc
        .nodes_by_id()
        .get(&edge.from.node_id)
        .context("missing FontFile node")?;
    ensure!(
        source.node_type == "FontFile" && edge.from.port_id == "font",
        "Text.{port} requires FontFile.font"
    );
    let id = source
        .params
        .get("assetId")
        .and_then(|v| v.as_str())
        .context("FontFile needs assetId")?;
    let bytes =
        if let Some(bytes) = crate::renderer::scene_prep::procedural_test_texture::font_bytes(id) {
            bytes.to_vec()
        } else {
            sc.asset_store
                .and_then(|store| store.get(id))
                .with_context(|| format!("FontFile asset '{id}' is not loaded"))?
                .bytes
        };
    let face_index = source
        .params
        .get("faceIndex")
        .map(|v| {
            v.as_u64()
                .context("FontFile faceIndex must be a nonnegative integer")
        })
        .transpose()?
        .unwrap_or(0);
    Ok(Some(FontSource {
        bytes,
        face_index: u32::try_from(face_index)?,
    }))
}

fn text(sc: &TextInputs<'_>, node: &Node) -> Result<String> {
    let Some(edge) = incoming_connection(sc.scene(), &node.id, "text") else {
        return Ok(node
            .params
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("Node Forge")
            .into());
    };
    let source = sc
        .nodes_by_id()
        .get(&edge.from.node_id)
        .context("missing text source")?;
    ensure!(
        source.node_type == "StringFormat" && edge.from.port_id == "text",
        "Text.text requires StringFormat.text"
    );
    let decimals = source
        .params
        .get("decimals")
        .map(|v| {
            v.as_u64()
                .context("StringFormat decimals must be an integer")
        })
        .transpose()?
        .unwrap_or(0);
    let values = [
        crate::dsl::resolve_input_f64(sc.scene(), sc.nodes_by_id(), &source.id, "value0")?
            .unwrap_or(0.0),
        crate::dsl::resolve_input_f64(sc.scene(), sc.nodes_by_id(), &source.id, "value1")?
            .unwrap_or(0.0),
    ];
    format_numbers(
        source
            .params
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("{0} × {1}"),
        &values,
        decimals as usize,
    )
}

fn validate_static_layout_inputs(sc: &TextInputs<'_>, node: &Node) -> Result<()> {
    let id = node.id.as_str();
    // TextSdf.position shifts GPU sampling of an already-built field; it does not
    // participate in CPU glyph shaping, layout, or distance-field generation.
    let gpu_position = node.node_type == "TextSdf";
    if let Some(machine) = &sc.scene().state_machine {
        let dynamic = crate::state_machine::dynamic_render_keys(machine);
        let mut pending = vec![id.to_string()];
        let mut visited = std::collections::HashSet::new();
        while let Some(dependency) = pending.pop() {
            if !visited.insert(dependency.clone()) {
                continue;
            }
            let prefix = format!("{dependency}:");
            ensure!(
                !dynamic.iter().any(|key| key.starts_with(&prefix)
                    && !(gpu_position && dependency == id && key == &format!("{id}:position"))),
                "Text {id} depends on animated input {dependency}; per-frame text updates are not supported yet (scene edits are supported)"
            );
            pending.extend(
                sc.scene()
                    .connections
                    .iter()
                    .filter(|edge| edge.to.node_id == dependency
                        && !(gpu_position && dependency == id && edge.to.port_id == "position"))
                    .map(|edge| edge.from.node_id.clone()),
            );
        }
    }
    Ok(())
}

pub(crate) fn resolve(sc: &TextInputs<'_>, node: &Node) -> Result<(TextRequest, Vec<FontSource>)> {
    validate_static_layout_inputs(sc, node)?;
    let number = |port, default| cpu_num_f32(sc.scene(), sc.nodes_by_id(), node, port, default);
    let string = |port, default: &str| {
        node.params
            .get(port)
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .to_string()
    };
    let mut fonts = vec![
        font(sc, node, "font")?.context("Text requires an embedded FontFile on its font input")?,
    ];
    if let Some(fallback) = font(sc, node, "fallbackFont")? {
        fonts.push(fallback);
    }
    let font_size = number("fontSize", 28.0)?;
    let line_height = number("lineHeight", 0.0)?;
    let weight = number("fontWeight", 0.0)?;
    ensure!(
        weight.fract() == 0.0 && (0.0..=1000.0).contains(&weight),
        "Text fontWeight must be an integer in [0, 1000] (0 = embedded face weight)"
    );
    let request = TextRequest {
        text: text(sc, node)?,
        font_size,
        weight: weight as u16,
        line_height: if line_height == 0.0 {
            font_size * 1.2
        } else {
            line_height
        },
        letter_spacing: number("letterSpacing", 0.0)?,
        size: vec2(sc, node, "size")?,
        align: string("align", "left"),
        vertical_align: string("verticalAlign", "top"),
        wrap: string("wrap", "none"),
        hinting: node
            .params
            .get("hinting")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        density: 1.0,
        phase: [0.0; 2],
        measured_frame: None,
    };
    Ok((request, fonts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scene() -> SceneDSL {
        serde_json::from_value(json!({
            "version": "6.0", "workingColorSpace": "linear-srgb",
            "metadata": {"name": "dynamic SDF sampling"}, "outputs": null,
            "nodes": [
                {"id": "text", "type": "TextSdf", "params": {}},
                {"id": "origin", "type": "Vector2Input", "params": {"x": 0, "y": 0}},
                {"id": "value", "type": "FloatInput", "params": {"value": 0}}
            ],
            "connections": [
                {"id": "value.origin", "from": {"nodeId": "value", "portId": "value"}, "to": {"nodeId": "origin", "portId": "x"}},
                {"id": "origin.text", "from": {"nodeId": "origin", "portId": "vector"}, "to": {"nodeId": "text", "portId": "position"}}
            ],
            "stateMachine": {
                "id": "clock", "stateParamGraph": {"rootNodePosition": {"x": 0, "y": 0}, "declarationPositions": {}},
                "derivations": [{"id": "clock", "outputBindings": [{
                    "uniform": {"nodeId": "value", "paramId": "value"},
                    "from": {"nodeId": "derive", "portId": "seconds"}
                }]}]
            }
        })).unwrap()
    }

    fn check(scene: &SceneDSL) -> Result<()> {
        let nodes = scene.nodes.iter().map(|node| (node.id.clone(), node.clone())).collect();
        validate_static_layout_inputs(&TextInputs { scene, nodes: &nodes, asset_store: None }, &scene.nodes[0])
    }

    #[test]
    fn sdf_sampling_position_can_depend_on_derivation_without_animating_layout() {
        let mut scene = scene();
        assert!(check(&scene).is_ok());
        // The same transitive dependency is forbidden when it changes CPU layout.
        scene.connections[1].to.port_id = "size".into();
        assert!(check(&scene).unwrap_err().to_string().contains("animated input value"));
        scene.connections[1].to.port_id = "position".into();
        scene.nodes[0].node_type = "TextPass".into();
        assert!(check(&scene).is_err());
    }

    #[test]
    fn sdf_only_exempts_direct_position_uniform_from_static_layout_guard() {
        let mut scene = scene();
        scene.connections.clear();
        let uniform = &mut scene.state_machine.as_mut().unwrap().derivations[0].output_bindings[0].uniform;
        uniform.node_id = "text".into();
        uniform.param_id = "position".into();
        assert!(check(&scene).is_ok());
        scene.state_machine.as_mut().unwrap().derivations[0].output_bindings[0].uniform.param_id = "fontSize".into();
        assert!(check(&scene).is_err());
    }
}
