use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WSMessage<T> {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub timestamp: u64,
    #[serde(rename = "requestId", skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<T>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DesignParamPatchPayload {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "nodeType")]
    pub node_type: String,
    pub phase: String,
    pub params: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PassTargetSizesPayload {
    pub passes: Vec<PassTargetSizeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PassTargetSizeEntry {
    #[serde(rename = "passName")]
    pub pass_name: String,
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "nodeType", skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    #[serde(rename = "targetTexture", skip_serializing_if = "Option::is_none")]
    pub target_texture: Option<String>,
    #[serde(rename = "targetSize")]
    pub target_size: [u32; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InteractionEventPayload {
    #[serde(rename = "eventType")]
    pub event_type: String,
    pub seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<InteractionEventData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct InteractionEventData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<InteractionPosition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<InteractionModifiers>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<InteractionKeyData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wheel: Option<InteractionWheelData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub touch: Option<InteractionTouchData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<InteractionStateData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InteractionPosition {
    #[serde(rename = "clientX")]
    pub client_x: f32,
    #[serde(rename = "clientY")]
    pub client_y: f32,
    #[serde(rename = "canvasX")]
    pub canvas_x: f32,
    #[serde(rename = "canvasY")]
    pub canvas_y: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InteractionModifiers {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub meta: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InteractionKeyData {
    pub key: String,
    #[serde(rename = "physicalKey", skip_serializing_if = "Option::is_none")]
    pub physical_key: Option<String>,
    pub repeat: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InteractionWheelData {
    #[serde(rename = "deltaX")]
    pub delta_x: f32,
    #[serde(rename = "deltaY")]
    pub delta_y: f32,
    #[serde(rename = "deltaMode")]
    pub delta_mode: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InteractionTouchData {
    #[serde(rename = "deviceId")]
    pub device_id: u64,
    #[serde(rename = "touchId")]
    pub touch_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InteractionStateData {
    #[serde(rename = "stateId")]
    pub state_id: String,
    #[serde(rename = "transitionId", skip_serializing_if = "Option::is_none")]
    pub transition_id: Option<String>,
}

pub fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::{
        DesignParamPatchPayload, InteractionEventData, InteractionEventPayload, InteractionKeyData,
        InteractionPosition, InteractionStateData, PassTargetSizeEntry, PassTargetSizesPayload,
    };

    #[test]
    fn interaction_event_payload_serialization_omits_absent_optional_fields() {
        let payload = InteractionEventPayload {
            event_type: "keydown".to_string(),
            seq: 7,
            data: Some(InteractionEventData {
                position: Some(InteractionPosition {
                    client_x: 100.0,
                    client_y: 40.0,
                    canvas_x: 12.0,
                    canvas_y: 8.0,
                }),
                key: Some(InteractionKeyData {
                    key: "a".to_string(),
                    physical_key: None,
                    repeat: false,
                }),
                ..InteractionEventData::default()
            }),
        };

        let json = serde_json::to_value(payload).expect("serialize interaction payload");
        assert_eq!(json["eventType"], "keydown");
        assert_eq!(json["seq"], 7);
        assert!(json["data"]["position"].is_object());
        assert!(json["data"]["key"].is_object());
        assert!(json["data"]["modifiers"].is_null());
        assert!(json["data"]["button"].is_null());
        assert!(json["data"]["wheel"].is_null());
        assert!(json["data"]["touch"].is_null());
        assert!(json["data"]["state"].is_null());
        assert!(json["data"]["key"]["physicalKey"].is_null());
    }

    #[test]
    fn interaction_state_payload_serializes_with_state_and_transition_ids() {
        let payload = InteractionEventPayload {
            event_type: "stateenter".to_string(),
            seq: 9,
            data: Some(InteractionEventData {
                state: Some(InteractionStateData {
                    state_id: "idle".to_string(),
                    transition_id: Some("to_idle".to_string()),
                }),
                ..InteractionEventData::default()
            }),
        };

        let json = serde_json::to_value(payload).expect("serialize state interaction payload");
        assert_eq!(json["eventType"], "stateenter");
        assert_eq!(json["data"]["state"]["stateId"], "idle");
        assert_eq!(json["data"]["state"]["transitionId"], "to_idle");
    }

    #[test]
    fn design_param_patch_payload_serializes_with_editor_field_names() {
        let payload = DesignParamPatchPayload {
            session_id: "design:MeshGradient_12:1".to_string(),
            node_id: "MeshGradient_12".to_string(),
            node_type: "MeshGradient".to_string(),
            phase: "change".to_string(),
            params: serde_json::Map::from_iter([("pos4".to_string(), serde_json::json!([1, 2]))]),
        };

        let json = serde_json::to_value(payload).expect("serialize design patch payload");
        assert_eq!(json["sessionId"], "design:MeshGradient_12:1");
        assert_eq!(json["nodeId"], "MeshGradient_12");
        assert_eq!(json["nodeType"], "MeshGradient");
        assert_eq!(json["phase"], "change");
        assert_eq!(json["params"]["pos4"], serde_json::json!([1, 2]));
        assert!(json["session_id"].is_null());
    }

    #[test]
    fn pass_target_sizes_payload_serializes_with_editor_field_names() {
        let payload = PassTargetSizesPayload {
            passes: vec![PassTargetSizeEntry {
                pass_name: "sys.mesh_gradient.MeshGradient_12.pass".to_string(),
                node_id: "MeshGradient_12".to_string(),
                node_type: Some("MeshGradient".to_string()),
                target_texture: Some("sys.mesh_gradient.MeshGradient_12.out".to_string()),
                target_size: [1080, 2400],
            }],
        };

        let json = serde_json::to_value(payload).expect("serialize pass target sizes payload");
        let pass = &json["passes"][0];
        assert_eq!(pass["passName"], "sys.mesh_gradient.MeshGradient_12.pass");
        assert_eq!(pass["nodeId"], "MeshGradient_12");
        assert_eq!(pass["nodeType"], "MeshGradient");
        assert_eq!(pass["targetTexture"], "sys.mesh_gradient.MeshGradient_12.out");
        assert_eq!(pass["targetSize"], serde_json::json!([1080, 2400]));
        assert!(pass["pass_name"].is_null());
    }
}
