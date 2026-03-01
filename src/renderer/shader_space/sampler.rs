//! Sampler configuration helpers.
//!
//! Resolves which sampler kind (nearest/linear × clamp/repeat/mirror) to use
//! for image-texture and pass-texture bindings.

use std::collections::HashMap;

use rust_wgpu_fiber::shader_space::{ShaderSpace, ShaderSpaceResult};

use crate::{
    dsl::{SceneDSL, incoming_connection},
    renderer::types::Params,
    renderer::utils::as_bytes,
};

use super::pass_spec::SamplerKind;
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
    upstream_pass_id: &str,
) -> SamplerKind {
    // We bind pass textures by upstream pass id (see MaterialCompileContext::pass_sampler_var_name).
    // Therefore, if multiple PassTexture nodes reference the same upstream pass, we can only pick
    // one sampler behavior. We choose deterministically by smallest node id.
    let mut pass_texture_nodes: Vec<&crate::dsl::Node> = Vec::new();
    for node in scene.nodes.iter() {
        if node.node_type != "PassTexture" {
            continue;
        }
        let Some(conn) = incoming_connection(scene, &node.id, "pass") else {
            continue;
        };
        if conn.from.node_id == upstream_pass_id {
            pass_texture_nodes.push(node);
        }
    }

    pass_texture_nodes.sort_by(|a, b| a.id.cmp(&b.id));
    if let Some(node) = pass_texture_nodes.first() {
        sampler_kind_from_node_params(&node.params)
    } else {
        // Fallback when a material references a pass output without a PassTexture node.
        SamplerKind::LinearClamp
    }
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
