//! Resource naming, sizing, and miscellaneous helpers.
//!
//! Centralises deterministic resource-name generation, render-pass size
//! calculations, and small utility functions used across pass assemblers.

use std::collections::HashMap;

use anyhow::{Result, bail};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, TextureFormat},
};

use crate::{
    dsl::{SceneDSL, incoming_connection},
    renderer::{
        camera::legacy_projection_camera_matrix,
        types::PassOutputRegistry,
        wgsl::clamp_min_1,
        wgsl_bloom::BLOOM_MAX_MIPS,
    },
};

// ── Display encode constants ─────────────────────────────────────────────

/// UI presentation helper: encode linear output to SDR sRGB for egui-wgpu.
/// We use dot-separated segments (no `__`) so the names read well and extend naturally to HDR.
pub(crate) const UI_PRESENT_SDR_SRGB_SUFFIX: &str = ".present.sdr.srgb";

/// HDR presentation: unclamped gamma-encode into Rgba16Float so egui's linear_from_gamma_rgb
/// can round-trip it back to linear on the Rgba16Float surface. Values > 1.0 survive.
pub(crate) const UI_PRESENT_HDR_GAMMA_SUFFIX: &str = ".present.hdr.gamma";

pub(crate) fn build_srgb_display_encode_wgsl(tex_var: &str, samp_var: &str) -> String {
    crate::renderer::wgsl_templates::build_srgb_display_encode_wgsl(tex_var, samp_var)
}

pub(crate) fn build_hdr_gamma_encode_wgsl(tex_var: &str, samp_var: &str) -> String {
    crate::renderer::wgsl_templates::build_hdr_gamma_encode_wgsl(tex_var, samp_var)
}

// ── Resource naming helpers ──────────────────────────────────────────────

pub(crate) fn sanitize_resource_segment(value: &str) -> String {
    let mut out = String::new();
    let mut last_was_dot = false;
    for ch in value.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '.'
        };
        if mapped == '.' {
            if !last_was_dot && !out.is_empty() {
                out.push('.');
            }
            last_was_dot = true;
        } else {
            out.push(mapped);
            last_was_dot = false;
        }
    }
    while out.ends_with('.') {
        out.pop();
    }
    out
}

pub(crate) fn stable_short_id_suffix(node_id: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }
    let sanitized = sanitize_resource_segment(node_id);
    let compact: String = sanitized.chars().filter(|c| *c != '.').collect();
    if compact.is_empty() {
        return String::new();
    }
    let keep = compact.len().min(max_len);
    compact[compact.len() - keep..].to_string()
}

pub(crate) fn readable_pass_name_for_node(node: &crate::dsl::Node) -> ResourceName {
    let id = node.id.as_str();
    let (id_base, budget_base_len) = if let Some(base) = id.strip_suffix(".pass") {
        (base, base.len())
    } else {
        (id, id.len())
    };

    let label_hint = ["label", "name", "title", "headerLabel"]
        .iter()
        .find_map(|k| node.params.get(*k).and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(sanitize_resource_segment)
        .filter(|s| !s.is_empty());

    let base = if let Some(label_hint) = label_hint {
        if budget_base_len >= 6 {
            let suffix = stable_short_id_suffix(id_base, 6);
            if suffix.is_empty() {
                id_base.to_string()
            } else {
                let reserved = suffix.len() + 1;
                if reserved >= budget_base_len {
                    id_base.to_string()
                } else {
                    let hint_budget = budget_base_len - reserved;
                    let hint: String = label_hint.chars().take(hint_budget).collect();
                    if hint.is_empty() {
                        id_base.to_string()
                    } else {
                        format!("{hint}.{suffix}")
                    }
                }
            }
        } else {
            id_base.to_string()
        }
    } else {
        id_base.to_string()
    };

    format!("{base}.pass").into()
}

// ── Size/resolution helpers ──────────────────────────────────────────────

pub(crate) fn sampled_render_pass_output_size(
    has_processing_consumer: bool,
    is_downsample_source: bool,
    coord_size_u: [u32; 2],
    geo_size: [f32; 2],
) -> [u32; 2] {
    if has_processing_consumer && !is_downsample_source {
        coord_size_u
    } else {
        [
            geo_size[0].max(1.0).round() as u32,
            geo_size[1].max(1.0).round() as u32,
        ]
    }
}

pub(crate) fn gaussian_blur_extend_upsample_geo_size(
    src_content_resolution: [u32; 2],
    padded_blur_resolution: [u32; 2],
) -> [f32; 2] {
    let src_w = src_content_resolution[0].max(1) as f32;
    let src_h = src_content_resolution[1].max(1) as f32;
    let padded_w = padded_blur_resolution[0].max(1) as f32;
    let padded_h = padded_blur_resolution[1].max(1) as f32;
    // Extend grows blur texture from `src` -> `padded`.
    // For the final upsample pass we scale geometry by that same factor so
    // the original content footprint is preserved (no apparent shrink).
    let extend_scale_x = padded_w / src_w;
    let extend_scale_y = padded_h / src_h;
    let scaled_w = src_w * extend_scale_x;
    let scaled_h = src_h * extend_scale_y;
    [scaled_w.max(1.0).round(), scaled_h.max(1.0).round()]
}

pub(crate) fn blur_downsample_steps_for_factor(downsample_factor: u32) -> Result<Vec<u32>> {
    match downsample_factor {
        1 | 2 | 4 | 8 => Ok(vec![downsample_factor]),
        16 => Ok(vec![8, 2]),
        other => bail!("GuassianBlurPass: unsupported downsample factor {other}"),
    }
}

pub(crate) fn should_skip_blur_downsample_pass(downsample_factor: u32) -> bool {
    downsample_factor == 1
}

pub(crate) fn should_skip_blur_upsample_pass(
    downsample_factor: u32,
    extend_enabled: bool,
    is_sampled_output: bool,
) -> bool {
    downsample_factor == 1 && !extend_enabled && is_sampled_output
}

pub(crate) fn bloom_downsample_level_count(mut size: [u32; 2]) -> u32 {
    let mut levels = 0u32;
    while levels < BLOOM_MAX_MIPS && size[0] > 2 && size[1] > 2 {
        size = [clamp_min_1(size[0] / 2), clamp_min_1(size[1] / 2)];
        levels += 1;
    }
    levels
}

pub(crate) fn parse_tint_from_node_or_default(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    bloom_node: &crate::dsl::Node,
) -> Result<[f32; 4]> {
    fn json_num(v: &serde_json::Value) -> Option<f32> {
        v.as_f64()
            .map(|x| x as f32)
            .or_else(|| v.as_i64().map(|x| x as f32))
            .or_else(|| v.as_u64().map(|x| x as f32))
    }

    fn parse_color(v: &serde_json::Value) -> Option<[f32; 4]> {
        if let Some(arr) = v.as_array() {
            let r = arr.first().and_then(json_num)?;
            let g = arr.get(1).and_then(json_num)?;
            let b = arr.get(2).and_then(json_num)?;
            let a = arr.get(3).and_then(json_num).unwrap_or(1.0);
            return Some([r, g, b, a]);
        }
        if let Some(obj) = v.as_object() {
            let r = obj.get("r").and_then(json_num).or_else(|| obj.get("x").and_then(json_num))?;
            let g = obj.get("g").and_then(json_num).or_else(|| obj.get("y").and_then(json_num))?;
            let b = obj.get("b").and_then(json_num).or_else(|| obj.get("z").and_then(json_num))?;
            let a = obj
                .get("a")
                .and_then(json_num)
                .or_else(|| obj.get("w").and_then(json_num))
                .unwrap_or(1.0);
            return Some([r, g, b, a]);
        }
        None
    }

    if let Some(conn) = incoming_connection(scene, &bloom_node.id, "tint") {
        let Some(src_node) = nodes_by_id.get(&conn.from.node_id) else {
            bail!("BloomNode {}.tint upstream node missing", bloom_node.id);
        };
        if src_node.node_type == "ColorInput" {
            if let Some(v) = src_node.params.get("value").and_then(parse_color) {
                return Ok(v.map(|c| c.clamp(0.0, 1.0)));
            }
            bail!(
                "BloomNode {}.tint expected ColorInput.value as vec3/vec4",
                bloom_node.id
            );
        }
        bail!(
            "BloomNode {}.tint expects color-compatible input, got {}",
            bloom_node.id,
            src_node.node_type
        );
    }

    if let Some(v) = bloom_node.params.get("tint").and_then(parse_color) {
        return Ok(v.map(|c| c.clamp(0.0, 1.0)));
    }

    Ok([1.0, 1.0, 1.0, 1.0])
}

// ── Camera helpers ───────────────────────────────────────────────────────

/// Processing chains (blur/mipmap/gradient internal passes) should run in fullscreen
/// texture space to avoid accumulating user-camera transforms across steps.
pub(crate) fn fullscreen_processing_camera(target_size: [f32; 2]) -> [f32; 16] {
    legacy_projection_camera_matrix(target_size)
}

pub(crate) fn resolve_chain_camera_for_first_pass(
    first_camera_consumed: &mut bool,
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    layer_node: &crate::dsl::Node,
    target_size: [f32; 2],
) -> Result<[f32; 16]> {
    if !*first_camera_consumed {
        *first_camera_consumed = true;
        crate::renderer::camera::resolve_effective_camera_for_pass_node(
            scene, nodes_by_id, layer_node, target_size,
        )
    } else {
        Ok(fullscreen_processing_camera(target_size))
    }
}

// ── Pass dependency graph resolution ─────────────────────────────────────

pub(crate) fn infer_uniform_resolution_from_pass_deps(
    blur_node_id: &str,
    pass_node_ids: &[String],
    pass_output_registry: &PassOutputRegistry,
) -> Result<Option<[u32; 2]>> {
    if pass_node_ids.is_empty() {
        return Ok(None);
    }

    let mut resolved: Vec<(String, [u32; 2])> = Vec::with_capacity(pass_node_ids.len());
    for upstream_pass_id in pass_node_ids {
        let Some(spec) = pass_output_registry.get(upstream_pass_id) else {
            bail!(
                "GuassianBlurPass {blur_node_id} non-pass source depends on upstream pass \
{upstream_pass_id}, but its output is not registered yet. Ensure upstream dependencies \
render earlier in Composite draw order."
            );
        };
        resolved.push((upstream_pass_id.clone(), spec.resolution));
    }

    let first_resolution = resolved[0].1;
    if resolved.iter().all(|(_, res)| *res == first_resolution) {
        return Ok(Some(first_resolution));
    }

    let details = resolved
        .iter()
        .map(|(node_id, [w, h])| format!("{node_id}={w}x{h}"))
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "GuassianBlurPass {blur_node_id} non-pass source samples pass textures with mismatched \
resolutions: {details}"
    );
}

// ── MSAA helpers ─────────────────────────────────────────────────────────

pub(crate) fn validate_render_pass_msaa_request(pass_id: &str, requested: u32) -> Result<()> {
    if matches!(requested, 1 | 2 | 4 | 8) {
        Ok(())
    } else {
        bail!("RenderPass.msaaSampleCount for {pass_id} must be one of 1,2,4,8, got {requested}");
    }
}

pub(crate) fn select_effective_msaa_sample_count(
    pass_id: &str,
    requested: u32,
    target_format: TextureFormat,
    device_features: wgpu::Features,
    adapter: Option<&wgpu::Adapter>,
) -> Result<u32> {
    validate_render_pass_msaa_request(pass_id, requested)?;
    let mut supported = target_format
        .guaranteed_format_features(device_features)
        .flags
        .supported_sample_counts();

    if device_features.contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES) {
        if let Some(adapter) = adapter {
            supported = adapter
                .get_texture_format_features(target_format)
                .flags
                .supported_sample_counts();
        }
    }

    let mut effective = 1u32;
    for candidate in [8u32, 4u32, 2u32, 1u32] {
        if candidate <= requested && supported.contains(&candidate) {
            effective = candidate;
            break;
        }
    }

    if effective != requested {
        eprintln!(
            "[msaa] RenderPass {pass_id}: {requested}x unsupported for {target_format:?}; supported={supported:?}; downgraded to {effective}x"
        );
    }
    Ok(effective)
}

// ── Pass texture binding resolution ──────────────────────────────────────

pub(crate) fn resolve_pass_texture_bindings(
    pass_output_registry: &PassOutputRegistry,
    pass_node_ids: &[String],
) -> Result<Vec<super::pass_spec::PassTextureBinding>> {
    let mut out: Vec<super::pass_spec::PassTextureBinding> =
        Vec::with_capacity(pass_node_ids.len());
    for upstream_pass_id in pass_node_ids {
        let Some(tex) = pass_output_registry.get_texture(upstream_pass_id) else {
            bail!(
                "PassTexture references upstream pass {upstream_pass_id}, but its output texture is not registered yet. \
Ensure the upstream pass is rendered earlier in Composite draw order."
            );
        };
        out.push(super::pass_spec::PassTextureBinding {
            texture: tex.clone(),
            image_node_id: None,
        });
    }
    Ok(out)
}

// ── Render-pass param helpers ────────────────────────────────────────────

pub(crate) fn parse_render_pass_cull_mode(
    params: &HashMap<String, serde_json::Value>,
) -> Result<Option<wgpu::Face>> {
    match params
        .get("culling")
        .and_then(|v| v.as_str())
        .unwrap_or("none")
    {
        "none" => Ok(None),
        "front" => Ok(Some(wgpu::Face::Front)),
        "back" => Ok(Some(wgpu::Face::Back)),
        other => bail!("RenderPass.culling must be one of none|front|back, got {other}"),
    }
}

pub(crate) fn parse_render_pass_depth_test(
    params: &HashMap<String, serde_json::Value>,
) -> Result<bool> {
    match params.get("depthTest") {
        Some(v) => v
            .as_bool()
            .ok_or_else(|| anyhow::anyhow!("RenderPass.depthTest must be a boolean, got {v}")),
        None => Ok(false),
    }
}
