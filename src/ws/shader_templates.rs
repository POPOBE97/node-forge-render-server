use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    protocol::{WSMessage, now_millis},
    renderer::node_compiler::template_loader,
};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct ShaderTemplateRequestPayload {
    pub template_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct ShaderTemplateResponsePayload {
    pub template_id: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UnknownShaderTemplate {
    template_id: String,
}

impl fmt::Display for UnknownShaderTemplate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown shader template id: {}",
            self.template_id
        )
    }
}

fn file_name(template_id: &str) -> Option<&'static str> {
    match template_id {
        "shader-material-default" => Some("shader_material_default.wgsl"),
        "glass-material" => Some("glass_material_fragment.wgsl"),
        "hyperos-glass-material" => Some("hyperos_glass_material_fragment.wgsl"),
        "intelligent-light" => Some("intelligent_light.wgsl"),
        "luminance-curve-lab" => Some("luminance_curve_lab.wgsl"),
        "luminance-curve-rgb" => Some("luminance_curve_rgb.wgsl"),
        _ => None,
    }
}

pub(super) fn response(
    request: ShaderTemplateRequestPayload,
    request_id: Option<String>,
) -> Result<WSMessage<ShaderTemplateResponsePayload>, UnknownShaderTemplate> {
    let Some(file_name) = file_name(&request.template_id) else {
        return Err(UnknownShaderTemplate {
            template_id: request.template_id,
        });
    };
    Ok(WSMessage {
        msg_type: "shader_template_response".to_string(),
        timestamp: now_millis(),
        request_id,
        payload: Some(ShaderTemplateResponsePayload {
            template_id: request.template_id,
            source: template_loader::load_template(file_name),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_template_id_resolves_without_exposing_a_path() {
        let message = response(
            ShaderTemplateRequestPayload {
                template_id: "shader-material-default".to_string(),
            },
            Some("request-1".to_string()),
        )
        .unwrap();

        assert_eq!(message.msg_type, "shader_template_response");
        assert_eq!(message.request_id.as_deref(), Some("request-1"));
        let payload = message.payload.unwrap();
        assert_eq!(payload.template_id, "shader-material-default");
        assert!(!payload.source.trim().is_empty());
    }

    #[test]
    fn unknown_template_id_is_rejected() {
        let error = response(
            ShaderTemplateRequestPayload {
                template_id: "../../shader.wgsl".to_string(),
            },
            None,
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "unknown shader template id: ../../shader.wgsl"
        );
    }
}
