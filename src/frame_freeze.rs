use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{dsl::SceneDSL, state_machine::OverrideKey};

pub const FRAME_FREEZE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameFreezeParamValue {
    pub node_id: String,
    pub param_name: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameFreezeV1 {
    pub schema_version: u32,
    pub scene_output_texture_name: String,
    pub scene_time_seconds: f32,
    #[serde(default)]
    pub state_params: Vec<FrameFreezeParamValue>,
    pub node_params: Vec<FrameFreezeParamValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameFreezeCase {
    pub case_id: String,
    pub name: String,
    pub captured_at: String,
    pub topology_fingerprint: String,
    pub payload: FrameFreezeV1,
}

#[derive(Clone, Debug)]
pub struct LastSubmittedFrame {
    pub topology_fingerprint: String,
    pub payload: FrameFreezeV1,
}

pub fn fingerprint_hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn collect_scene_params(scene: &SceneDSL) -> Vec<FrameFreezeParamValue> {
    let mut values = scene
        .nodes
        .iter()
        .flat_map(|node| {
            node.params
                .iter()
                .map(|(param_name, value)| FrameFreezeParamValue {
                    node_id: node.id.clone(),
                    param_name: param_name.clone(),
                    value: value.clone(),
                })
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        (&left.node_id, &left.param_name).cmp(&(&right.node_id, &right.param_name))
    });
    values
}

pub fn collect_state_params(values: &HashMap<OverrideKey, Value>) -> Vec<FrameFreezeParamValue> {
    let mut values = values
        .iter()
        .map(|(key, value)| FrameFreezeParamValue {
            node_id: key.node_id.clone(),
            param_name: key.param_name.clone(),
            value: value.clone(),
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        (&left.node_id, &left.param_name).cmp(&(&right.node_id, &right.param_name))
    });
    values
}

pub fn apply_param_values(scene: &mut SceneDSL, values: &[FrameFreezeParamValue]) -> Result<()> {
    let mut nodes = scene
        .nodes
        .iter_mut()
        .map(|node| (node.id.clone(), node))
        .collect::<HashMap<_, _>>();
    for value in values {
        let node = nodes
            .get_mut(&value.node_id)
            .ok_or_else(|| anyhow!("freeze references missing prepared node {}", value.node_id))?;
        node.params
            .insert(value.param_name.clone(), value.value.clone());
    }
    Ok(())
}

pub fn apply_state_param_values(scene: &mut SceneDSL, values: &[FrameFreezeParamValue]) {
    let overrides = values
        .iter()
        .map(|value| {
            (
                OverrideKey::new(&value.node_id, &value.param_name),
                value.value.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    crate::state_machine::apply_overrides(scene, &overrides);
}

pub fn validate_case(case: &FrameFreezeCase) -> Result<()> {
    if case.case_id.trim().is_empty() {
        bail!("freeze case id must not be empty");
    }
    if case.name.trim().is_empty() {
        bail!("freeze case name must not be empty");
    }
    if case.topology_fingerprint.len() != 64
        || !case
            .topology_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        bail!("freeze topology fingerprint must be 64 hex characters");
    }
    if case.payload.schema_version != FRAME_FREEZE_SCHEMA_VERSION {
        bail!(
            "unsupported frame freeze schema version {}",
            case.payload.schema_version
        );
    }
    if case.payload.scene_output_texture_name.trim().is_empty() {
        bail!("freeze scene output texture name must not be empty");
    }
    if !case.payload.scene_time_seconds.is_finite() {
        bail!("freeze scene time must be finite");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{Metadata, Node};

    fn scene(value: f64) -> SceneDSL {
        SceneDSL {
            version: "6.1".into(),
            working_color_space: crate::color::WorkingColorSpace::Srgb,
            dpi: 3.0,
            metadata: Metadata {
                name: "freeze".into(),
                created: None,
                modified: None,
            },
            nodes: vec![Node {
                id: "value".into(),
                node_type: "FloatInput".into(),
                params: HashMap::from([("value".into(), serde_json::json!(value))]),
                inputs: Vec::new(),
                outputs: Vec::new(),
                input_bindings: Vec::new(),
                wgsl_override: None,
            }],
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        }
    }

    #[test]
    fn scene_params_round_trip_into_prepared_scene() {
        let params = collect_scene_params(&scene(2.0));
        let mut replay = scene(1.0);
        apply_param_values(&mut replay, &params).unwrap();
        assert_eq!(replay.nodes[0].params["value"], serde_json::json!(2.0));
    }

    #[test]
    fn state_params_follow_authored_node_identity_into_prepared_mirrors() {
        let mut replay = scene(1.0);
        replay.nodes[0].id = "prepared/value".into();
        replay.nodes[0].params.insert(
            "__authoredNodeId".into(),
            serde_json::json!("authored-value"),
        );
        apply_state_param_values(
            &mut replay,
            &[FrameFreezeParamValue {
                node_id: "authored-value".into(),
                param_name: "value".into(),
                value: serde_json::json!(3.0),
            }],
        );
        assert_eq!(replay.nodes[0].params["value"], serde_json::json!(3.0));
    }
}
