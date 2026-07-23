//! Sampler configuration helpers.
//!
//! Resolves which sampler kind (nearest/linear × clamp/repeat/mirror) to use
//! for image-texture and pass-texture bindings.

use std::collections::HashMap;

use rust_wgpu_fiber::shader_space::{ShaderSpace, ShaderSpaceResult};

use crate::renderer::types::PassTextureRef;
use crate::{dsl::SceneDSL, renderer::types::Params, renderer::utils::as_bytes};

use crate::renderer::render_plan::pass_spec::SamplerKind;
use crate::renderer::types::PassBindings;

pub(crate) fn sampler_kind_from_node_params(
    params: &HashMap<String, serde_json::Value>,
) -> SamplerKind {
    // Scene DSL uses ImageTexture/PassTexture params like:
    // - addressModeU/V: "mirror-repeat" | "repeat" | "clamp-to-edge"
    // - magFilter/minFilter: "linear" | "nearest"
    // Legacy fields used by some scenes:
    // - interpolation: "linear" | "nearest"
    // - extension: "repeat" | "clamp" | "mirror-repeat"
    let addr_u = params
        .get("addressModeU")
        .and_then(|v| v.as_str())
        .or_else(|| params.get("extension").and_then(|v| v.as_str()))
        .unwrap_or("clamp-to-edge")
        .trim()
        .to_ascii_lowercase();
    let addr_v = params
        .get("addressModeV")
        .and_then(|v| v.as_str())
        .or_else(|| params.get("extension").and_then(|v| v.as_str()))
        .unwrap_or("clamp-to-edge")
        .trim()
        .to_ascii_lowercase();

    let address = if addr_u.contains("mirror") || addr_v.contains("mirror") {
        "mirror"
    } else if addr_u.contains("repeat") || addr_v.contains("repeat") {
        "repeat"
    } else {
        "clamp"
    };

    let mag = params.get("magFilter").and_then(|v| v.as_str());
    let min = params.get("minFilter").and_then(|v| v.as_str());
    let interpolation = params
        .get("interpolation")
        .and_then(|v| v.as_str())
        .unwrap_or("linear")
        .trim()
        .to_ascii_lowercase();

    let nearest = match (mag, min) {
        (Some(mag), Some(min)) => {
            mag.trim().eq_ignore_ascii_case("nearest") && min.trim().eq_ignore_ascii_case("nearest")
        }
        // Legacy: single toggle.
        _ => interpolation == "nearest",
    };
    match (nearest, address) {
        (true, "mirror") => SamplerKind::NearestMirror,
        (true, "repeat") => SamplerKind::NearestRepeat,
        (true, _) => SamplerKind::NearestClamp,
        (false, "mirror") => SamplerKind::LinearMirror,
        (false, "repeat") => SamplerKind::LinearRepeat,
        (false, _) => SamplerKind::LinearClamp,
    }
}

pub(crate) fn sampler_kind_for_pass_texture(
    scene: &SceneDSL,
    texture_ref: &PassTextureRef,
) -> SamplerKind {
    texture_ref
        .sampler_node_id
        .as_ref()
        .and_then(|node_id| scene.nodes.iter().find(|node| node.id == *node_id))
        .map(|node| sampler_kind_from_node_params(&node.params))
        // Direct reads are node-defined sampling sites whose contract is LinearClamp.
        .unwrap_or(SamplerKind::LinearClamp)
}

pub(crate) fn build_image_premultiply_wgsl(tex_var: &str, samp_var: &str) -> String {
    crate::renderer::wgsl_templates::build_image_premultiply_wgsl(tex_var, samp_var)
}

pub fn update_pass_params(
    shader_space: &ShaderSpace,
    pass: &PassBindings,
    params: &Params,
) -> ShaderSpaceResult<()> {
    shader_space.write_buffer(pass.params_buffer.as_str(), 0, as_bytes(params))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dsl::Node, renderer::node_compiler::test_utils::test_scene};

    fn pass_texture_node(id: &str, filter: &str) -> Node {
        Node {
            id: id.to_string(),
            node_type: "PassTexture".to_string(),
            params: HashMap::from([
                (
                    "addressModeU".to_string(),
                    serde_json::Value::String("clamp-to-edge".to_string()),
                ),
                (
                    "addressModeV".to_string(),
                    serde_json::Value::String("clamp-to-edge".to_string()),
                ),
                (
                    "magFilter".to_string(),
                    serde_json::Value::String(filter.to_string()),
                ),
                (
                    "minFilter".to_string(),
                    serde_json::Value::String(filter.to_string()),
                ),
            ]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        }
    }

    #[test]
    fn pass_texture_sampling_sites_keep_independent_samplers() {
        let scene = test_scene(
            vec![
                pass_texture_node("nearest_read", "nearest"),
                pass_texture_node("linear_read", "linear"),
            ],
            Vec::new(),
        );
        let nearest = PassTextureRef::through_pass_texture("nearest_read", "source", "pass");
        let linear = PassTextureRef::through_pass_texture("linear_read", "source", "pass");

        assert_eq!(
            sampler_kind_for_pass_texture(&scene, &nearest),
            SamplerKind::NearestClamp
        );
        assert_eq!(
            sampler_kind_for_pass_texture(&scene, &linear),
            SamplerKind::LinearClamp
        );
    }
}
