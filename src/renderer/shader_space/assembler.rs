//! ShaderSpace construction module.
//!
//! This module contains logic for building ShaderSpace instances from DSL scenes,
//! including texture creation, geometry buffers, uniform buffers, pipelines, and
//! composite layer handling.
//!
//! ## Chain Pass Support
//!
//! This module supports chaining pass nodes together (e.g., GuassianBlurPass -> GuassianBlurPass).
//! Each pass that outputs to `pass` type gets an intermediate texture allocated automatically.
//! Resolution inheritance: downstream passes inherit upstream resolution by default, but can override.
#![allow(dead_code)]

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    io::Cursor,
    path::PathBuf,
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, bail};
use image::DynamicImage;
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{
        self, BlendState, Color, ShaderStages, TextureFormat, TextureUsages, vertex_attr_array,
    },
    pool::{
        buffer_pool::BufferSpec, sampler_pool::SamplerSpec,
        texture_pool::TextureSpec as FiberTextureSpec,
    },
    shader_space::{ShaderSpace, ShaderSpaceResult},
};

use crate::{
    dsl::{SceneDSL, find_node, incoming_connection, parse_str, parse_texture_format},
    renderer::{
        geometry_resolver::{
            is_draw_pass_node_type, is_pass_like_node_type, resolve_scene_draw_contexts,
        },
        graph_uniforms::{
            choose_graph_binding_kind, compute_pipeline_signature_for_pass_bindings,
            graph_field_name, hash_bytes, pack_graph_values,
        },
        node_compiler::geometry_nodes::{rect2d_geometry_vertices, rect2d_unit_geometry_vertices},
        scene_prep::{bake_data_parse_nodes, prepare_scene},
        types::ValueType,
        types::{
            BakedDataParseMeta, BakedValue, GraphBinding, GraphBindingKind, Kernel2D,
            MaterialCompileContext, Params, PassBindings, PassOutputRegistry, PassOutputSpec,
            TypedExpr,
        },
        utils::{as_bytes, as_bytes_slice, decode_data_url, load_image_from_data_url},
        utils::{
            coerce_to_type, cpu_num_f32, cpu_num_f32_min_0, cpu_num_u32_floor, cpu_num_u32_min_1,
        },
        wgsl::{
            ERROR_SHADER_WGSL, build_blur_image_wgsl_bundle,
            build_blur_image_wgsl_bundle_with_graph_binding, build_downsample_bundle,
            build_downsample_pass_wgsl_bundle, build_dynamic_rect_compose_bundle,
            build_fullscreen_textured_bundle, build_horizontal_blur_bundle_with_tap_count,
            build_pass_wgsl_bundle, build_pass_wgsl_bundle_with_graph_binding,
            build_upsample_bilinear_bundle, build_vertical_blur_bundle_with_tap_count, clamp_min_1,
            gaussian_kernel_8, gaussian_mip_level_and_sigma_p,
        },
    },
};

pub(crate) fn image_node_dimensions(
    node: &crate::dsl::Node,
    asset_store: Option<&crate::asset_store::AssetStore>,
) -> Option<[u32; 2]> {
    // Prefer assetId → asset_store lookup.
    if let Some(asset_id) = node
        .params
        .get("assetId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        if let Some(store) = asset_store {
            if let Some(data) = store.get(asset_id) {
                let reader = image::ImageReader::new(Cursor::new(&data.bytes))
                    .with_guessed_format()
                    .ok()?;
                return reader.into_dimensions().ok().map(|(w, h)| [w, h]);
            }
        }
    }

    // Legacy fallback: dataUrl.
    let data_url = node
        .params
        .get("dataUrl")
        .and_then(|v| v.as_str())
        .or_else(|| node.params.get("data_url").and_then(|v| v.as_str()));

    if let Some(s) = data_url.filter(|s| !s.trim().is_empty()) {
        let bytes = decode_data_url(s).ok()?;
        let reader = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        return reader.into_dimensions().ok().map(|(w, h)| [w, h]);
    }

    // Legacy fallback: file path.
    let rel_base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = node.params.get("path").and_then(|v| v.as_str());
    let p = path.filter(|s| !s.trim().is_empty())?;

    let candidates: Vec<std::path::PathBuf> = {
        let pb = std::path::PathBuf::from(p);
        if pb.is_absolute() {
            vec![pb]
        } else {
            vec![
                pb.clone(),
                rel_base.join(&pb),
                rel_base.join("assets").join(&pb),
            ]
        }
    };

    for cand in &candidates {
        if let Ok((w, h)) = image::image_dimensions(cand) {
            return Some([w, h]);
        }
    }

    None
}

pub(crate) fn parse_kernel_source_js_like(source: &str) -> Result<Kernel2D> {
    // Strip JS comments so we don't accidentally match docstrings like "width/height: number".
    fn strip_js_comments(src: &str) -> String {
        // Minimal, non-string-aware comment stripper:
        // - removes // line comments
        // - removes /* block comments */
        let mut out = String::with_capacity(src.len());
        let mut i = 0;
        let bytes = src.as_bytes();
        let mut in_block = false;
        while i < bytes.len() {
            if in_block {
                if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    in_block = false;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }

            // Block comment start
            if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                in_block = true;
                i += 2;
                continue;
            }
            // Line comment start
            if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
                // Skip until newline
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }

            out.push(bytes[i] as char);
            i += 1;
        }
        out
    }

    let source = strip_js_comments(source);

    // Minimal parser for the editor-authored Kernel node `params.source`.
    // Expected form (JavaScript-like):
    // return { width: 3, height: 3, value: [ ... ] };
    // or: return { width: 3, height: 3, values: [ ... ] };

    fn find_field_after_colon<'a>(src: &'a str, key: &str) -> Result<&'a str> {
        // Find `key` as an identifier (not inside comments like `width/height`) and return the
        // substring after its ':' (trimmed).
        let bytes = src.as_bytes();
        let key_bytes = key.as_bytes();
        'outer: for i in 0..=bytes.len().saturating_sub(key_bytes.len()) {
            if &bytes[i..i + key_bytes.len()] != key_bytes {
                continue;
            }
            // Word boundary before key.
            if i > 0 {
                let prev = bytes[i - 1];
                if prev.is_ascii_alphanumeric() || prev == b'_' {
                    continue;
                }
            }
            // After key: skip whitespace then require ':'
            let mut j = i + key_bytes.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= bytes.len() || bytes[j] != b':' {
                continue;
            }
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            // Ensure this isn't a prefix of a longer identifier.
            if i + key_bytes.len() < bytes.len() {
                let next = bytes[i + key_bytes.len()];
                if next.is_ascii_alphanumeric() || next == b'_' {
                    continue 'outer;
                }
            }
            return Ok(&src[j..]);
        }
        bail!("Kernel.source missing {key}")
    }

    fn parse_u32_field(src: &str, key: &str) -> Result<u32> {
        let after_colon = find_field_after_colon(src, key)?;
        let mut num = String::new();
        for ch in after_colon.chars() {
            if ch.is_ascii_digit() {
                num.push(ch);
            } else {
                break;
            }
        }
        if num.is_empty() {
            bail!("Kernel.source field {key} missing numeric value");
        }
        Ok(num.parse::<u32>()?)
    }

    fn parse_f32_array_field(src: &str, key: &str) -> Result<Vec<f32>> {
        let after_colon = find_field_after_colon(src, key)?;
        let lb = after_colon
            .find('[')
            .ok_or_else(|| anyhow!("Kernel.source missing '[' for {key}"))?;
        let after_lb = &after_colon[lb + 1..];
        let rb = after_lb
            .find(']')
            .ok_or_else(|| anyhow!("Kernel.source missing ']' for {key}"))?;
        let inside = &after_lb[..rb];

        let mut values: Vec<f32> = Vec::new();
        let mut token = String::new();
        for ch in inside.chars() {
            if ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+' || ch == 'e' || ch == 'E'
            {
                token.push(ch);
            } else if !token.trim().is_empty() {
                values.push(token.trim().parse::<f32>()?);
                token.clear();
            } else {
                token.clear();
            }
        }
        if !token.trim().is_empty() {
            values.push(token.trim().parse::<f32>()?);
        }
        Ok(values)
    }

    let w = parse_u32_field(source.as_str(), "width")?;
    let h = parse_u32_field(source.as_str(), "height")?;
    // Prefer `values` when present; otherwise fallback to `value`.
    let values = match parse_f32_array_field(source.as_str(), "values") {
        Ok(v) => v,
        Err(_) => parse_f32_array_field(source.as_str(), "value")?,
    };

    let expected = (w as usize).saturating_mul(h as usize);
    if expected == 0 {
        bail!("Kernel.source invalid size: {w}x{h}");
    }
    if values.len() != expected {
        bail!(
            "Kernel.source values length mismatch: expected {expected} for {w}x{h}, got {}",
            values.len()
        );
    }

    Ok(Kernel2D {
        width: w,
        height: h,
        values,
    })
}

const IDENTITY_MAT4: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

fn sampled_pass_node_ids(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
) -> Result<HashSet<String>> {
    // A pass must render into a dedicated intermediate texture if it will be sampled later.
    //
    // Originally we only treated passes referenced by explicit PassTexture nodes as "sampled".
    // But some material nodes (e.g. GlassMaterial) can directly depend on upstream pass textures
    // without a PassTexture node in the graph. Those dependencies show up in WGSL bundle
    // `pass_textures`, so we detect sampling by scanning all pass nodes and collecting their
    // referenced pass textures.
    let mut out: HashSet<String> = HashSet::new();

    for (node_id, node) in nodes_by_id {
        if !is_pass_like_node_type(&node.node_type) {
            continue;
        }
        let deps = deps_for_pass_node(scene, nodes_by_id, node_id.as_str())?;
        out.extend(deps);
    }

    Ok(out)
}

fn sampled_pass_node_ids_from_roots(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    roots_in_draw_order: &[String],
) -> Result<HashSet<String>> {
    let reachable = compute_pass_render_order(scene, nodes_by_id, roots_in_draw_order)?;
    let reachable_set: HashSet<String> = reachable.iter().cloned().collect();

    let mut out: HashSet<String> = HashSet::new();
    for node_id in reachable {
        let Some(node) = nodes_by_id.get(&node_id) else {
            continue;
        };
        if !is_pass_like_node_type(&node.node_type) {
            continue;
        }

        // Composite being reachable should not by itself force its input layers
        // to be treated as sampled outputs. We only mark passes sampled when a
        // non-Composite pass node actually samples them as textures.
        if node.node_type == "Composite" {
            continue;
        }

        let deps = deps_for_pass_node(scene, nodes_by_id, node_id.as_str())?;
        for dep in deps {
            if reachable_set.contains(&dep) {
                out.insert(dep);
            }
        }
    }

    Ok(out)
}

fn resolve_pass_texture_bindings(
    pass_output_registry: &PassOutputRegistry,
    pass_node_ids: &[String],
) -> Result<Vec<PassTextureBinding>> {
    let mut out: Vec<PassTextureBinding> = Vec::with_capacity(pass_node_ids.len());
    for upstream_pass_id in pass_node_ids {
        let Some(tex) = pass_output_registry.get_texture(upstream_pass_id) else {
            bail!(
                "PassTexture references upstream pass {upstream_pass_id}, but its output texture is not registered yet. \
Ensure the upstream pass is rendered earlier in Composite draw order."
            );
        };
        out.push(PassTextureBinding {
            texture: tex.clone(),
            image_node_id: None,
        });
    }
    Ok(out)
}

fn deps_for_pass_node(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    pass_node_id: &str,
) -> Result<Vec<String>> {
    let Some(node) = nodes_by_id.get(pass_node_id) else {
        bail!("missing node for pass id: {pass_node_id}");
    };

    match node.node_type.as_str() {
        "RenderPass" => {
            let bundle = build_pass_wgsl_bundle(
                scene,
                nodes_by_id,
                None,
                None,
                pass_node_id,
                false,
                None,
                Vec::new(),
                String::new(),
                false,
            )?;
            Ok(bundle.pass_textures)
        }
        "GuassianBlurPass" => {
            let bundle = build_blur_image_wgsl_bundle(scene, nodes_by_id, pass_node_id)?;
            Ok(bundle.pass_textures)
        }
        "Downsample" => {
            // Downsample depends on the upstream pass provided on its `source` input.
            let source_conn = incoming_connection(scene, pass_node_id, "source")
                .ok_or_else(|| anyhow!("Downsample.source missing for {pass_node_id}"))?;
            Ok(vec![source_conn.from.node_id.clone()])
        }
        "Upsample" => {
            // Upsample depends on the upstream pass provided on its `source` input.
            let source_conn = incoming_connection(scene, pass_node_id, "source")
                .ok_or_else(|| anyhow!("Upsample.source missing for {pass_node_id}"))?;
            Ok(vec![source_conn.from.node_id.clone()])
        }
        "Composite" => crate::renderer::scene_prep::composite_layers_in_draw_order(
            scene,
            nodes_by_id,
            pass_node_id,
        ),
        "GradientBlur" => {
            // GradientBlur reads "source" input (not "pass").
            let Some(conn) = incoming_connection(scene, pass_node_id, "source") else {
                return Ok(Vec::new());
            };
            let source_is_pass = nodes_by_id
                .get(&conn.from.node_id)
                .is_some_and(|n| is_pass_like_node_type(&n.node_type));
            if source_is_pass {
                Ok(vec![conn.from.node_id.clone()])
            } else {
                // Non-pass source: compile the material expression to find transitive pass deps.
                let mut ctx = crate::renderer::types::MaterialCompileContext::default();
                let mut cache = std::collections::HashMap::new();
                crate::renderer::node_compiler::compile_material_expr(
                    scene,
                    nodes_by_id,
                    &conn.from.node_id,
                    Some(&conn.from.port_id),
                    &mut ctx,
                    &mut cache,
                )?;
                Ok(ctx.pass_textures)
            }
        }
        other => bail!("expected a pass node id, got node type {other} for {pass_node_id}"),
    }
}

fn visit_pass_node(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    pass_node_id: &str,
    deps_cache: &mut HashMap<String, Vec<String>>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    out: &mut Vec<String>,
) -> Result<()> {
    if visited.contains(pass_node_id) {
        return Ok(());
    }
    if !visiting.insert(pass_node_id.to_string()) {
        bail!("cycle detected in pass dependencies at: {pass_node_id}");
    }

    let deps = if let Some(existing) = deps_cache.get(pass_node_id) {
        existing.clone()
    } else {
        let deps = deps_for_pass_node(scene, nodes_by_id, pass_node_id)?;
        deps_cache.insert(pass_node_id.to_string(), deps.clone());
        deps
    };

    for dep in deps {
        visit_pass_node(
            scene,
            nodes_by_id,
            dep.as_str(),
            deps_cache,
            visiting,
            visited,
            out,
        )?;
    }

    visiting.remove(pass_node_id);
    visited.insert(pass_node_id.to_string());
    out.push(pass_node_id.to_string());
    Ok(())
}

fn compute_pass_render_order(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    roots_in_draw_order: &[String],
) -> Result<Vec<String>> {
    let mut deps_cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut visiting: HashSet<String> = HashSet::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();

    for root in roots_in_draw_order {
        visit_pass_node(
            scene,
            nodes_by_id,
            root.as_str(),
            &mut deps_cache,
            &mut visiting,
            &mut visited,
            &mut out,
        )?;
    }

    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SamplerKind {
    NearestClamp,
    NearestMirror,
    NearestRepeat,
    LinearMirror,
    LinearRepeat,
    LinearClamp,
}

fn sampler_kind_from_node_params(
    params: &std::collections::HashMap<String, serde_json::Value>,
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

fn sampler_kind_for_pass_texture(scene: &SceneDSL, upstream_pass_id: &str) -> SamplerKind {
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

type PassTextureBinding = crate::renderer::render_plan::types::PassTextureBinding;

fn build_image_premultiply_wgsl(tex_var: &str, samp_var: &str) -> String {
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

#[derive(Clone)]
struct TextureDecl {
    name: ResourceName,
    size: [u32; 2],
    format: TextureFormat,
    sample_count: u32,
}

#[derive(Clone)]
struct RenderPassSpec {
    pass_id: String,
    name: ResourceName,
    geometry_buffer: ResourceName,
    instance_buffer: Option<ResourceName>,
    normals_buffer: Option<ResourceName>,
    target_texture: ResourceName,
    resolve_target: Option<ResourceName>,
    params_buffer: ResourceName,
    baked_data_parse_buffer: Option<ResourceName>,
    params: Params,
    graph_binding: Option<GraphBinding>,
    graph_values: Option<Vec<u8>>,
    shader_wgsl: String,
    texture_bindings: Vec<PassTextureBinding>,
    sampler_kinds: Vec<SamplerKind>,
    blend_state: BlendState,
    color_load_op: wgpu::LoadOp<Color>,
    sample_count: u32,
}

fn validate_render_pass_msaa_request(pass_id: &str, requested: u32) -> Result<()> {
    if matches!(requested, 1 | 2 | 4 | 8) {
        Ok(())
    } else {
        bail!("RenderPass.msaaSampleCount for {pass_id} must be one of 1,2,4,8, got {requested}");
    }
}

fn select_effective_msaa_sample_count(
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

fn build_srgb_display_encode_wgsl(tex_var: &str, samp_var: &str) -> String {
    crate::renderer::wgsl_templates::build_srgb_display_encode_wgsl(tex_var, samp_var)
}

// UI presentation helper: encode linear output to SDR sRGB for egui-wgpu.
// We use dot-separated segments (no `__`) so the names read well and extend naturally to HDR.
const UI_PRESENT_SDR_SRGB_SUFFIX: &str = ".present.sdr.srgb";

fn sanitize_resource_segment(value: &str) -> String {
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

fn stable_short_id_suffix(node_id: &str, max_len: usize) -> String {
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

fn readable_pass_name_for_node(node: &crate::dsl::Node) -> ResourceName {
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

fn sampled_render_pass_output_size(
    _has_processing_consumer: bool,
    _is_downsample_source: bool,
    _coord_size_u: [u32; 2],
    geo_size: [f32; 2],
) -> [u32; 2] {
    [
        geo_size[0].max(1.0).round() as u32,
        geo_size[1].max(1.0).round() as u32,
    ]
}

fn gaussian_blur_extend_upsample_geo_size(
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

fn blur_downsample_steps_for_factor(downsample_factor: u32) -> Result<Vec<u32>> {
    match downsample_factor {
        1 | 2 | 4 | 8 => Ok(vec![downsample_factor]),
        16 => Ok(vec![8, 2]),
        other => bail!("GuassianBlurPass: unsupported downsample factor {other}"),
    }
}

fn should_skip_blur_downsample_pass(downsample_factor: u32) -> bool {
    downsample_factor == 1
}

fn should_skip_blur_upsample_pass(
    downsample_factor: u32,
    extend_enabled: bool,
    is_sampled_output: bool,
) -> bool {
    downsample_factor == 1 && !extend_enabled && is_sampled_output
}

pub(crate) fn build_shader_space_from_scene_internal(
    scene: &SceneDSL,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    adapter: Option<&wgpu::Adapter>,
    enable_display_encode: bool,
    debug_dump_wgsl_dir: Option<PathBuf>,
    asset_store: Option<&crate::asset_store::AssetStore>,
) -> Result<(
    ShaderSpace,
    [u32; 2],
    ResourceName,
    Vec<PassBindings>,
    [u8; 32],
)> {
    let prepared = prepare_scene(scene)?;
    let resolution = prepared.resolution;
    let nodes_by_id = &prepared.nodes_by_id;
    let ids = &prepared.ids;
    let output_texture_node_id = &prepared.output_texture_node_id;
    let output_texture_name = prepared.output_texture_name.clone();
    let composite_layers_in_order = &prepared.composite_layers_in_draw_order;
    let order = &prepared.topo_order;

    let resolved_contexts =
        resolve_scene_draw_contexts(&prepared.scene, nodes_by_id, ids, resolution)?;
    let composition_contexts = resolved_contexts.composition_contexts.clone();
    let composition_consumers_by_source = resolved_contexts.composition_consumers_by_source;
    let mut draw_coord_size_by_pass: HashMap<String, [f32; 2]> = HashMap::new();
    for ctx in &resolved_contexts.draw_contexts {
        // If a pass appears in multiple draw contexts, keep the most recent inferred domain.
        draw_coord_size_by_pass.insert(ctx.pass_node_id.clone(), ctx.coord_domain.size_px);
    }

    let mut geometry_buffers: Vec<(ResourceName, Arc<[u8]>)> = Vec::new();
    let mut instance_buffers: Vec<(ResourceName, Arc<[u8]>)> = Vec::new();
    let mut textures: Vec<TextureDecl> = Vec::new();
    let mut render_pass_specs: Vec<RenderPassSpec> = Vec::new();
    let mut baked_data_parse_meta_by_pass: HashMap<String, Arc<BakedDataParseMeta>> =
        HashMap::new();
    let mut baked_data_parse_bytes_by_pass: HashMap<String, Arc<[u8]>> = HashMap::new();
    let mut baked_data_parse_buffer_to_pass_id: HashMap<ResourceName, String> = HashMap::new();
    let mut composite_passes: Vec<ResourceName> = Vec::new();

    // Output target texture is always Composite.target.
    let target_texture_id = output_texture_node_id.clone();
    let target_node = find_node(&nodes_by_id, &target_texture_id)?;
    if target_node.node_type != "RenderTexture" {
        bail!(
            "Composite.target must come from RenderTexture, got {}",
            target_node.node_type
        );
    }
    let tgt_w_u = cpu_num_u32_min_1(
        &prepared.scene,
        nodes_by_id,
        target_node,
        "width",
        resolution[0],
    )?;
    let tgt_h_u = cpu_num_u32_min_1(
        &prepared.scene,
        nodes_by_id,
        target_node,
        "height",
        resolution[1],
    )?;
    let tgt_w = tgt_w_u as f32;
    let tgt_h = tgt_h_u as f32;
    let target_texture_name = ids
        .get(&target_texture_id)
        .cloned()
        .ok_or_else(|| anyhow!("missing name for node: {}", target_texture_id))?;

    // Pass nodes sampled by reachable downstream pass nodes must have dedicated
    // intermediate outputs. Restrict to currently reachable pass roots to avoid
    // dead branches forcing sampled-output paths.
    let sampled_pass_ids = crate::renderer::render_plan::sampled_pass_node_ids_from_roots(
        &prepared.scene,
        nodes_by_id,
        composite_layers_in_order,
    )?;

    for id in order {
        let node = match nodes_by_id.get(id) {
            Some(n) => n,
            None => continue,
        };
        let name = ids
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("missing name for node: {id}"))?;

        match node.node_type.as_str() {
            "Rect2DGeometry" => {
                let (
                    _geo_buf,
                    geo_w,
                    geo_h,
                    _geo_x,
                    _geo_y,
                    _instances,
                    _base_m,
                    _instance_mats,
                    _translate_expr,
                    _vtx_inline_stmts,
                    _vtx_wgsl_decls,
                    _vtx_graph_input_kinds,
                    _uses_instance_index,
                    rect_dyn,
                    _normals_bytes,
                ) = crate::renderer::render_plan::resolve_geometry_for_render_pass(
                    &prepared.scene,
                    nodes_by_id,
                    ids,
                    &node.id,
                    [tgt_w, tgt_h],
                    None,
                    asset_store,
                )?;
                let verts = if rect_dyn.is_some() {
                    rect2d_unit_geometry_vertices()
                } else {
                    rect2d_geometry_vertices(geo_w, geo_h)
                };
                let bytes: Arc<[u8]> = Arc::from(as_bytes_slice(&verts).to_vec());
                geometry_buffers.push((name, bytes));
            }
            "GLTFGeometry" => {
                let store = asset_store.ok_or_else(|| anyhow!("GLTFGeometry: no asset store"))?;
                let loaded = crate::renderer::render_plan::load_gltf_geometry_pixel_space(
                    &prepared.scene,
                    &node.id,
                    node,
                    [tgt_w, tgt_h],
                    store,
                )?;
                let verts = loaded.vertices;

                let vert_bytes: Arc<[u8]> =
                    Arc::from(bytemuck::cast_slice::<[f32; 5], u8>(&verts).to_vec());
                geometry_buffers.push((name.clone(), vert_bytes));
                if let Some(nb) = loaded.normals_bytes {
                    let normals_name: ResourceName = format!("{name}.normals").into();
                    geometry_buffers.push((normals_name, nb));
                }
            }
            "RenderTexture" => {
                let w =
                    cpu_num_u32_min_1(&prepared.scene, nodes_by_id, node, "width", resolution[0])?;
                let h =
                    cpu_num_u32_min_1(&prepared.scene, nodes_by_id, node, "height", resolution[1])?;
                let format = parse_texture_format(&node.params)?;
                textures.push(TextureDecl {
                    name,
                    size: [w, h],
                    format,
                    sample_count: 1,
                });
            }
            _ => {}
        }
    }

    // Helper: create a fullscreen geometry buffer.
    let make_fullscreen_geometry = |w: f32, h: f32| -> Arc<[u8]> {
        let verts = rect2d_geometry_vertices(w, h);
        Arc::from(as_bytes_slice(&verts).to_vec())
    };

    // Track pass outputs for chain resolution.
    let mut pass_output_registry = PassOutputRegistry::new();
    let target_format = parse_texture_format(&target_node.params)?;
    // Sampled pass outputs are typically intermediate textures (used by PassTexture / blur chains).
    // Keep them in a linear UNORM format even when the Composite target is sRGB.
    // This matches existing test baselines and avoids relying on sRGB attachment readback paths.
    let sampled_pass_format = match target_format {
        TextureFormat::Rgba8UnormSrgb => TextureFormat::Rgba8Unorm,
        TextureFormat::Bgra8UnormSrgb => TextureFormat::Bgra8Unorm,
        other => other,
    };

    // If the output target is sRGB, create an extra linear UNORM texture that contains
    // *sRGB-encoded bytes* for UI presentation (egui/eframe commonly presents into a linear
    // swapchain format).
    let display_texture_name: Option<ResourceName> = if enable_display_encode {
        match target_format {
            TextureFormat::Rgba8UnormSrgb | TextureFormat::Bgra8UnormSrgb => {
                let name: ResourceName = format!(
                    "{}{}",
                    target_texture_name.as_str(),
                    UI_PRESENT_SDR_SRGB_SUFFIX
                )
                .into();
                textures.push(TextureDecl {
                    name: name.clone(),
                    size: [tgt_w_u, tgt_h_u],
                    format: TextureFormat::Rgba8Unorm,
                    sample_count: 1,
                });
                Some(name)
            }
            _ => None,
        }
    } else {
        None
    };

    // Composite draw order only contains direct inputs. For chained passes, we must render
    // upstream pass dependencies first so PassTexture can resolve them.
    let pass_nodes_in_order = crate::renderer::render_plan::compute_pass_render_order(
        &prepared.scene,
        nodes_by_id,
        composite_layers_in_order,
    )?;

    // Pass nodes used as resample/filter sources keep special dynamic-geometry fullscreen handling.
    let mut downsample_source_pass_ids: HashSet<String> = HashSet::new();
    let mut upsample_source_pass_ids: HashSet<String> = HashSet::new();
    let mut gaussian_source_pass_ids: HashSet<String> = HashSet::new();
    let mut gradient_source_pass_ids: HashSet<String> = HashSet::new();
    for (node_id, node) in nodes_by_id {
        if node.node_type == "Downsample" {
            if let Some(conn) = incoming_connection(&prepared.scene, node_id, "source") {
                downsample_source_pass_ids.insert(conn.from.node_id.clone());
            }
            continue;
        }
        if node.node_type == "Upsample" {
            if let Some(conn) = incoming_connection(&prepared.scene, node_id, "source") {
                upsample_source_pass_ids.insert(conn.from.node_id.clone());
            }
            continue;
        }
        if node.node_type == "GuassianBlurPass" {
            if let Some(conn) = incoming_connection(&prepared.scene, node_id, "pass") {
                gaussian_source_pass_ids.insert(conn.from.node_id.clone());
            }
            continue;
        }
        if node.node_type == "GradientBlur" {
            if let Some(conn) = incoming_connection(&prepared.scene, node_id, "source") {
                let src_is_pass_like = nodes_by_id
                    .get(&conn.from.node_id)
                    .is_some_and(|n| is_pass_like_node_type(&n.node_type));
                if src_is_pass_like {
                    gradient_source_pass_ids.insert(conn.from.node_id.clone());
                }
            }
        }
    }

    for layer_id in &pass_nodes_in_order {
        let layer_node = find_node(&nodes_by_id, layer_id)?;
        match layer_node.node_type.as_str() {
            "RenderPass" => {
                let pass_name = readable_pass_name_for_node(layer_node);

                let requested_msaa = cpu_num_u32_floor(
                    &prepared.scene,
                    nodes_by_id,
                    layer_node,
                    "msaaSampleCount",
                    1,
                )?;
                let msaa_sample_count = select_effective_msaa_sample_count(
                    layer_id,
                    requested_msaa,
                    sampled_pass_format,
                    device.features(),
                    adapter,
                )?;
                // Only passes sampled downstream use geometry-sized intermediate outputs.
                // MSAA alone must not force this path, otherwise vertex-space behavior diverges from 1x.
                let is_sampled_output = sampled_pass_ids.contains(layer_id);
                let composition_consumers = composition_consumers_by_source
                    .get(layer_id)
                    .cloned()
                    .unwrap_or_default();
                let has_composition_consumer = !composition_consumers.is_empty();
                let has_processing_consumer = prepared.scene.connections.iter().any(|conn| {
                    conn.from.node_id == *layer_id
                        && conn.from.port_id == "pass"
                        && nodes_by_id
                            .get(&conn.to.node_id)
                            .is_some_and(|n| is_draw_pass_node_type(&n.node_type))
                });
                let has_extend_blur_consumer = prepared.scene.connections.iter().any(|conn| {
                    if conn.from.node_id != *layer_id || conn.from.port_id != "pass" {
                        return false;
                    }
                    let Some(dst_node) = nodes_by_id.get(&conn.to.node_id) else {
                        return false;
                    };
                    if dst_node.node_type != "GuassianBlurPass" {
                        return false;
                    }
                    dst_node
                        .params
                        .get("extend")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                });
                let is_downsample_source = downsample_source_pass_ids.contains(layer_id);
                let is_upsample_source = upsample_source_pass_ids.contains(layer_id);
                let is_blur_source = gaussian_source_pass_ids.contains(layer_id)
                    || gradient_source_pass_ids.contains(layer_id);
                let pass_coord_size = draw_coord_size_by_pass
                    .get(layer_id)
                    .copied()
                    .unwrap_or([tgt_w, tgt_h]);
                let pass_coord_w_u = pass_coord_size[0].max(1.0).round() as u32;
                let pass_coord_h_u = pass_coord_size[1].max(1.0).round() as u32;

                let blend_state =
                    crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
                        .with_context(|| {
                            format!(
                                "invalid blend params for {}",
                                crate::dsl::node_display_label_with_id(layer_node)
                            )
                        })?;

                let render_geo_node_id = incoming_connection(&prepared.scene, layer_id, "geometry")
                    .map(|c| c.from.node_id.clone())
                    .ok_or_else(|| anyhow!("RenderPass.geometry missing for {layer_id}"))?;
                let render_geo_is_rect =
                    find_node(nodes_by_id, &render_geo_node_id)?.node_type == "Rect2DGeometry";

                let (
                    geometry_buffer,
                    geo_w,
                    geo_h,
                    geo_x,
                    geo_y,
                    instance_count,
                    _base_m,
                    _instance_mats,
                    _translate_expr,
                    _vertex_inline_stmts,
                    _vertex_wgsl_decls,
                    _vertex_graph_input_kinds,
                    _vertex_uses_instance_index,
                    _rect_dyn,
                    normals_bytes,
                ) = crate::renderer::render_plan::resolve_geometry_for_render_pass(
                    &prepared.scene,
                    nodes_by_id,
                    ids,
                    &render_geo_node_id,
                    pass_coord_size,
                    Some(&MaterialCompileContext {
                        baked_data_parse: Some(std::sync::Arc::new(
                            prepared.baked_data_parse.clone(),
                        )),
                        baked_data_parse_meta: None,
                        ..Default::default()
                    }),
                    asset_store,
                )?;

                // Determine the single-sample output target for this pass.
                // - If sampled by downstream processing passes: keep coord-domain sizing so all
                //   processing passes share the same inferred render-target space.
                // - If sampled only by non-processing consumers (e.g. PassTexture/material paths):
                //   keep geometry-sized intermediates.
                // - Otherwise: render directly into the Composite target texture.
                let (pass_target_w_u, pass_target_h_u, pass_output_texture): (
                    u32,
                    u32,
                    ResourceName,
                ) = if is_sampled_output {
                    let out_tex: ResourceName = format!("sys.pass.{layer_id}.out").into();
                    let [w_u, h_u] = sampled_render_pass_output_size(
                        has_processing_consumer,
                        is_downsample_source || is_upsample_source || is_blur_source,
                        [pass_coord_w_u, pass_coord_h_u],
                        [geo_w, geo_h],
                    );
                    textures.push(TextureDecl {
                        name: out_tex.clone(),
                        size: [w_u, h_u],
                        format: sampled_pass_format,
                        sample_count: 1,
                    });
                    (w_u, h_u, out_tex)
                } else {
                    (tgt_w_u, tgt_h_u, target_texture_name.clone())
                };
                let pass_output_format = if is_sampled_output {
                    sampled_pass_format
                } else {
                    target_format
                };
                let (pass_render_target_texture, pass_resolve_target) = if msaa_sample_count > 1 {
                    // Key MSAA color attachments by resolve target so passes that resolve into the
                    // same output can share load/clear behavior.
                    let msaa_tex: ResourceName = format!(
                        "sys.msaa.{}.{}.color",
                        pass_output_texture.as_str(),
                        msaa_sample_count
                    )
                    .into();
                    textures.push(TextureDecl {
                        name: msaa_tex.clone(),
                        size: [pass_target_w_u, pass_target_h_u],
                        format: pass_output_format,
                        sample_count: msaa_sample_count,
                    });
                    (msaa_tex, Some(pass_output_texture.clone()))
                } else {
                    (pass_output_texture.clone(), None)
                };
                let pass_target_w = pass_target_w_u as f32;
                let pass_target_h = pass_target_h_u as f32;

                let mut baked = prepared.baked_data_parse.clone();
                baked.extend(bake_data_parse_nodes(
                    nodes_by_id,
                    layer_id,
                    instance_count,
                )?);

                let mut slot_by_output: HashMap<(String, String, String), u32> = HashMap::new();
                let mut keys: Vec<(String, String, String)> = baked
                    .keys()
                    .filter(|(pass_id, _, _)| pass_id == layer_id)
                    .cloned()
                    .collect();
                keys.sort();

                for (i, k) in keys.iter().enumerate() {
                    slot_by_output.insert(k.clone(), i as u32);
                }

                let meta = Arc::new(BakedDataParseMeta {
                    pass_id: layer_id.clone(),
                    outputs_per_instance: keys.len() as u32,
                    slot_by_output,
                });

                let mut packed: Vec<f32> = Vec::new();
                let instances = instance_count.min(1024) as usize;
                packed.resize(instances * meta.outputs_per_instance as usize * 4, 0.0);

                for (slot, (pass_id, node_id, port_id)) in keys.iter().enumerate() {
                    let vs = baked
                        .get(&(pass_id.clone(), node_id.clone(), port_id.clone()))
                        .cloned()
                        .unwrap_or_default();
                    for i in 0..instances {
                        let v = vs.get(i).cloned().unwrap_or(BakedValue::F32(0.0));
                        let base = (i * meta.outputs_per_instance as usize + slot) * 4;
                        match v {
                            BakedValue::F32(x) => {
                                packed[base] = x;
                            }
                            BakedValue::I32(x) => {
                                packed[base] = x as f32;
                            }
                            BakedValue::U32(x) => {
                                packed[base] = x as f32;
                            }
                            BakedValue::Bool(x) => {
                                packed[base] = if x { 1.0 } else { 0.0 };
                            }
                            BakedValue::Vec2([x, y]) => {
                                packed[base] = x;
                                packed[base + 1] = y;
                            }
                            BakedValue::Vec3([x, y, z]) => {
                                packed[base] = x;
                                packed[base + 1] = y;
                                packed[base + 2] = z;
                            }
                            BakedValue::Vec4([x, y, z, w]) => {
                                packed[base] = x;
                                packed[base + 1] = y;
                                packed[base + 2] = z;
                                packed[base + 3] = w;
                            }
                        }
                    }
                }

                let bytes: Arc<[u8]> = Arc::from(as_bytes_slice(&packed).to_vec());
                baked_data_parse_meta_by_pass.insert(layer_id.clone(), meta);
                baked_data_parse_bytes_by_pass.insert(layer_id.clone(), bytes.clone());

                let (
                    _geometry_buffer_2,
                    _geo_w_2,
                    _geo_h_2,
                    _geo_x_2,
                    _geo_y_2,
                    _instance_count_2,
                    base_m_2,
                    instance_mats_2,
                    translate_expr,
                    vertex_inline_stmts,
                    vertex_wgsl_decls,
                    vertex_graph_input_kinds,
                    vertex_uses_instance_index,
                    rect_dyn_2,
                    _normals_bytes_2,
                ) = crate::renderer::render_plan::resolve_geometry_for_render_pass(
                    &prepared.scene,
                    nodes_by_id,
                    ids,
                    &render_geo_node_id,
                    pass_coord_size,
                    Some(&MaterialCompileContext {
                        baked_data_parse: Some(std::sync::Arc::new(baked.clone())),
                        baked_data_parse_meta: baked_data_parse_meta_by_pass.get(layer_id).cloned(),
                        ..Default::default()
                    }),
                    asset_store,
                )?;

                // For intermediate pass outputs that will be blitted into a final Composition target,
                // render the main pass in local texture space (fullscreen in its own output), then
                // apply scene placement at compose time. This preserves size/position when blitting.
                let use_fullscreen_for_downsample_source =
                    is_downsample_source && rect_dyn_2.is_some();
                let use_fullscreen_for_upsample_source = is_upsample_source && rect_dyn_2.is_some();
                let use_fullscreen_for_extend_blur_source =
                    is_sampled_output && has_extend_blur_consumer;
                let use_fullscreen_for_local_blit =
                    is_sampled_output && !has_processing_consumer && has_composition_consumer;
                let use_fullscreen_main_pass = use_fullscreen_for_downsample_source
                    || use_fullscreen_for_upsample_source
                    || use_fullscreen_for_extend_blur_source
                    || use_fullscreen_for_local_blit;

                let (main_pass_geometry_buffer, main_pass_params, main_pass_rect_dyn) =
                    if use_fullscreen_main_pass {
                        // Create a dedicated fullscreen geometry for this pass.
                        let fs_geo: ResourceName =
                            format!("sys.pass.{layer_id}.fullscreen.geo").into();
                        geometry_buffers.push((
                            fs_geo.clone(),
                            make_fullscreen_geometry(pass_target_w, pass_target_h),
                        ));
                        (
                            fs_geo,
                            Params {
                                target_size: [pass_target_w, pass_target_h],
                                geo_size: [pass_target_w, pass_target_h],
                                // Center the fullscreen geometry in the intermediate texture.
                                center: [pass_target_w * 0.5, pass_target_h * 0.5],
                                geo_translate: [0.0, 0.0],
                                geo_scale: [1.0, 1.0],
                                time: 0.0,
                                _pad0: 0.0,
                                color: [0.9, 0.2, 0.2, 1.0],
                            },
                            // Keep rect_dyn_2 so shader can access dynamic size for GeoFragcoord/GeoSize.
                            // Vertex positioning is controlled by fullscreen_vertex_positioning flag.
                            rect_dyn_2.clone(),
                        )
                    } else {
                        let resolved_geometry_buffer: ResourceName = if render_geo_is_rect {
                            let geo_name: ResourceName =
                                format!("sys.pass.{layer_id}.resolved.geo").into();
                            let geo_bytes: Arc<[u8]> = if rect_dyn_2.is_some() {
                                Arc::from(as_bytes_slice(&rect2d_unit_geometry_vertices()).to_vec())
                            } else {
                                Arc::from(
                                    as_bytes_slice(&rect2d_geometry_vertices(
                                        geo_w.max(1.0),
                                        geo_h.max(1.0),
                                    ))
                                    .to_vec(),
                                )
                            };
                            geometry_buffers.push((geo_name.clone(), geo_bytes));
                            geo_name
                        } else {
                            geometry_buffer.clone()
                        };
                        (
                            resolved_geometry_buffer,
                            Params {
                                target_size: [pass_target_w, pass_target_h],
                                geo_size: [geo_w.max(1.0), geo_h.max(1.0)],
                                center: [geo_x, geo_y],
                                geo_translate: [0.0, 0.0],
                                geo_scale: [1.0, 1.0],
                                time: 0.0,
                                _pad0: 0.0,
                                color: [0.9, 0.2, 0.2, 1.0],
                            },
                            rect_dyn_2.clone(),
                        )
                    };

                let params_name: ResourceName = format!("params.{layer_id}").into();
                let params = main_pass_params;

                let has_non_identity_base_m = base_m_2 != IDENTITY_MAT4;
                let has_instance_mats = instance_mats_2.as_ref().is_some_and(|m| !m.is_empty());
                let is_instanced =
                    instance_count > 1 || has_non_identity_base_m || has_instance_mats;

                // Internal resource naming helpers for this pass node.
                let baked_buf_name: ResourceName =
                    format!("sys.pass.{layer_id}.baked_data_parse").into();

                let baked_arc = std::sync::Arc::new(baked);
                let translate_expr_wgsl = translate_expr.map(|e| e.expr);
                let vertex_inline_stmts_for_bundle = vertex_inline_stmts.clone();
                let vertex_wgsl_decls_for_bundle = vertex_wgsl_decls.clone();
                let vertex_graph_input_kinds_for_bundle = vertex_graph_input_kinds.clone();

                let has_normals = normals_bytes.is_some();
                let fullscreen_vertex_positioning =
                    use_fullscreen_main_pass && rect_dyn_2.is_some();

                let mut bundle = build_pass_wgsl_bundle_with_graph_binding(
                    &prepared.scene,
                    nodes_by_id,
                    Some(baked_arc.clone()),
                    baked_data_parse_meta_by_pass.get(layer_id).cloned(),
                    layer_id,
                    is_instanced,
                    translate_expr_wgsl.clone(),
                    vertex_inline_stmts_for_bundle.clone(),
                    vertex_wgsl_decls_for_bundle.clone(),
                    vertex_uses_instance_index,
                    main_pass_rect_dyn.clone(),
                    vertex_graph_input_kinds_for_bundle.clone(),
                    None,
                    fullscreen_vertex_positioning,
                    has_normals,
                )?;

                let mut graph_binding: Option<GraphBinding> = None;
                let mut graph_values: Option<Vec<u8>> = None;
                if let Some(schema) = bundle.graph_schema.clone() {
                    let limits = device.limits();
                    let kind = choose_graph_binding_kind(
                        schema.size_bytes,
                        limits.max_uniform_buffer_binding_size as u64,
                        limits.max_storage_buffer_binding_size as u64,
                    )?;

                    if bundle.graph_binding_kind != Some(kind) {
                        bundle = build_pass_wgsl_bundle_with_graph_binding(
                            &prepared.scene,
                            nodes_by_id,
                            Some(baked_arc.clone()),
                            baked_data_parse_meta_by_pass.get(layer_id).cloned(),
                            layer_id,
                            is_instanced,
                            translate_expr_wgsl.clone(),
                            vertex_inline_stmts_for_bundle.clone(),
                            vertex_wgsl_decls_for_bundle.clone(),
                            vertex_uses_instance_index,
                            main_pass_rect_dyn.clone(),
                            vertex_graph_input_kinds_for_bundle.clone(),
                            Some(kind),
                            fullscreen_vertex_positioning,
                            has_normals,
                        )?;
                    }

                    let schema = bundle.graph_schema.clone().ok_or_else(|| {
                        anyhow!("missing graph schema after graph binding selection")
                    })?;
                    let graph_buffer_name: ResourceName = format!("params.{layer_id}.graph").into();
                    let values = pack_graph_values(&prepared.scene, &schema)?;
                    graph_values = Some(values);
                    graph_binding = Some(GraphBinding {
                        buffer_name: graph_buffer_name,
                        kind,
                        schema,
                    });
                }

                let shader_wgsl = bundle.module;

                let mut texture_bindings: Vec<PassTextureBinding> = Vec::new();
                let mut sampler_kinds: Vec<SamplerKind> = Vec::new();

                // ImageTexture bindings first (must match MaterialCompileContext::wgsl_decls order).
                for id in bundle.image_textures.iter() {
                    let Some(tex) = ids.get(id).cloned() else {
                        continue;
                    };
                    texture_bindings.push(PassTextureBinding {
                        texture: tex,
                        image_node_id: Some(id.clone()),
                    });

                    // Default to LinearClamp if node is missing or params are absent.
                    let kind = nodes_by_id
                        .get(id)
                        .map(|n| sampler_kind_from_node_params(&n.params))
                        .unwrap_or(SamplerKind::LinearClamp);
                    sampler_kinds.push(kind);
                }

                // PassTexture bindings next (also must match MaterialCompileContext::wgsl_decls order).
                let pass_bindings = crate::renderer::render_plan::resolve_pass_texture_bindings(
                    &pass_output_registry,
                    &bundle.pass_textures,
                )?;
                for (upstream_pass_id, binding) in bundle.pass_textures.iter().zip(pass_bindings) {
                    texture_bindings.push(binding);
                    sampler_kinds.push(sampler_kind_for_pass_texture(
                        &prepared.scene,
                        upstream_pass_id,
                    ));
                }

                let instance_buffer = if is_instanced {
                    let b: ResourceName = format!("sys.pass.{layer_id}.instances").into();

                    // Per-instance mat4 (column-major) as 4 vec4<f32> (16 floats).
                    // If SetTransform provides per-instance CPU-baked matrices, prefer them.
                    let mats: Vec<[f32; 16]> = if let Some(mats) = instance_mats_2 {
                        mats
                    } else {
                        let mut mats: Vec<[f32; 16]> = Vec::with_capacity(instance_count as usize);
                        for _ in 0..instance_count {
                            mats.push(base_m_2);
                        }
                        mats
                    };

                    let bytes: Arc<[u8]> = Arc::from(as_bytes_slice(&mats).to_vec());

                    debug_assert_eq!(bytes.len(), (instance_count as usize) * 16 * 4);

                    instance_buffers.push((b.clone(), bytes));

                    Some(b)
                } else {
                    None
                };

                let baked_data_parse_buffer: Option<ResourceName> = if keys.is_empty() {
                    None
                } else {
                    baked_data_parse_buffer_to_pass_id
                        .insert(baked_buf_name.clone(), layer_id.clone());
                    Some(baked_buf_name.clone())
                };

                render_pass_specs.push(RenderPassSpec {
                    pass_id: pass_name.as_str().to_string(),
                    name: pass_name.clone(),
                    geometry_buffer: main_pass_geometry_buffer.clone(),
                    instance_buffer,
                    normals_buffer: normals_bytes.as_ref().map(|_| {
                        let nb_name: ResourceName =
                            format!("{}.normals", main_pass_geometry_buffer).into();
                        nb_name
                    }),
                    target_texture: pass_render_target_texture.clone(),
                    resolve_target: pass_resolve_target,
                    params_buffer: params_name,
                    baked_data_parse_buffer,
                    params,
                    graph_binding: graph_binding.clone(),
                    graph_values: graph_values.clone(),
                    shader_wgsl,
                    texture_bindings,
                    sampler_kinds,
                    blend_state,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                    sample_count: msaa_sample_count,
                });
                composite_passes.push(pass_name);

                // If a pass is sampled (so it renders to sys.pass.<id>.out) and consumed by one
                // or more Composition nodes, synthesize compose passes per Composition consumer.
                if is_sampled_output && has_composition_consumer {
                    for composition_id in composition_consumers {
                        let Some(comp_ctx) = composition_contexts.get(&composition_id) else {
                            continue;
                        };
                        let comp_tgt_w = comp_ctx.target_size_px[0];
                        let comp_tgt_h = comp_ctx.target_size_px[1];
                        let comp_tgt_w_u = comp_tgt_w.max(1.0).round() as u32;
                        let comp_tgt_h_u = comp_tgt_h.max(1.0).round() as u32;

                        let compose_pass_name: ResourceName =
                            format!("sys.pass.{layer_id}.to.{composition_id}.compose.pass").into();
                        let compose_params_name: ResourceName =
                            format!("params.sys.pass.{layer_id}.to.{composition_id}.compose")
                                .into();

                        // If the sampled output is target-sized, compose with a fullscreen quad.
                        // If it is intermediate/local-sized, compose with original placement (size/position).
                        let (
                            compose_geometry_buffer,
                            compose_params_val,
                            compose_bundle,
                            compose_graph_binding,
                            compose_graph_values,
                        ) = if pass_target_w_u == comp_tgt_w_u && pass_target_h_u == comp_tgt_h_u {
                            let compose_geo: ResourceName =
                                format!("sys.pass.{layer_id}.to.{composition_id}.compose.geo")
                                    .into();
                            geometry_buffers.push((
                                compose_geo.clone(),
                                make_fullscreen_geometry(comp_tgt_w, comp_tgt_h),
                            ));
                            let fragment_body =
                                "return textureSample(src_tex, src_samp, in.uv);".to_string();
                            (
                                compose_geo,
                                Params {
                                    target_size: [comp_tgt_w, comp_tgt_h],
                                    geo_size: [comp_tgt_w, comp_tgt_h],
                                    center: [comp_tgt_w * 0.5, comp_tgt_h * 0.5],
                                    geo_translate: [0.0, 0.0],
                                    geo_scale: [1.0, 1.0],
                                    time: 0.0,
                                    _pad0: 0.0,
                                    color: [0.0, 0.0, 0.0, 0.0],
                                },
                                build_fullscreen_textured_bundle(fragment_body),
                                None,
                                None,
                            )
                        } else if use_fullscreen_main_pass && rect_dyn_2.is_some() {
                            let compose_geo: ResourceName =
                                format!("sys.pass.{layer_id}.to.{composition_id}.compose.geo")
                                    .into();
                            geometry_buffers.push((
                                compose_geo.clone(),
                                Arc::from(
                                    as_bytes_slice(&rect2d_unit_geometry_vertices()).to_vec(),
                                ),
                            ));

                            let rect_dyn = rect_dyn_2.as_ref().expect(
                                "fullscreen-main-pass dynamic compose implies rect_dyn_2.is_some()",
                            );
                            let position_expr = rect_dyn
                                .position_expr
                                .as_ref()
                                .map(|e| e.expr.as_str())
                                .unwrap_or("params.center");
                            let size_expr = rect_dyn
                                .size_expr
                                .as_ref()
                                .map(|e| e.expr.as_str())
                                .unwrap_or("params.geo_size");

                            let graph_inputs_wgsl = if let Some(gb) = graph_binding.as_ref() {
                                let mut decl = String::new();
                                decl.push_str("\nstruct GraphInputs {\n");
                                for field in &gb.schema.fields {
                                    decl.push_str(&format!(
                                        "    // Node: {}\n    {}: {},\n",
                                        field.node_id,
                                        field.field_name,
                                        field.kind.wgsl_slot_type()
                                    ));
                                }
                                decl.push_str("};\n\n");
                                decl.push_str("@group(0) @binding(2)\n");
                                match gb.kind {
                                    GraphBindingKind::Uniform => {
                                        decl.push_str("var<uniform> graph_inputs: GraphInputs;\n")
                                    }
                                    GraphBindingKind::StorageRead => decl.push_str(
                                        "var<storage, read> graph_inputs: GraphInputs;\n",
                                    ),
                                }
                                decl
                            } else {
                                String::new()
                            };

                            let bundle = build_dynamic_rect_compose_bundle(
                                &graph_inputs_wgsl,
                                position_expr,
                                size_expr,
                            );

                            (
                                compose_geo,
                                Params {
                                    target_size: [comp_tgt_w, comp_tgt_h],
                                    geo_size: [pass_target_w, pass_target_h],
                                    center: [geo_x, geo_y],
                                    geo_translate: [0.0, 0.0],
                                    geo_scale: [1.0, 1.0],
                                    time: 0.0,
                                    _pad0: 0.0,
                                    color: [0.0, 0.0, 0.0, 0.0],
                                },
                                bundle,
                                graph_binding.clone(),
                                graph_values.clone(),
                            )
                        } else if use_fullscreen_for_local_blit {
                            let compose_geo: ResourceName =
                                format!("sys.pass.{layer_id}.to.{composition_id}.compose.geo")
                                    .into();
                            geometry_buffers.push((
                                compose_geo.clone(),
                                Arc::from(
                                    as_bytes_slice(&rect2d_geometry_vertices(
                                        geo_w.max(1.0),
                                        geo_h.max(1.0),
                                    ))
                                    .to_vec(),
                                ),
                            ));
                            let fragment_body =
                                "return textureSample(src_tex, src_samp, in.uv);".to_string();
                            (
                                compose_geo,
                                Params {
                                    target_size: [comp_tgt_w, comp_tgt_h],
                                    geo_size: [geo_w.max(1.0), geo_h.max(1.0)],
                                    center: [geo_x, geo_y],
                                    geo_translate: [0.0, 0.0],
                                    geo_scale: [1.0, 1.0],
                                    time: 0.0,
                                    _pad0: 0.0,
                                    color: [0.0, 0.0, 0.0, 0.0],
                                },
                                build_fullscreen_textured_bundle(fragment_body),
                                None,
                                None,
                            )
                        } else {
                            let fragment_body =
                                "return textureSample(src_tex, src_samp, in.uv);".to_string();
                            (
                                main_pass_geometry_buffer.clone(),
                                Params {
                                    target_size: [comp_tgt_w, comp_tgt_h],
                                    geo_size: [geo_w.max(1.0), geo_h.max(1.0)],
                                    center: [geo_x, geo_y],
                                    geo_translate: [0.0, 0.0],
                                    geo_scale: [1.0, 1.0],
                                    time: 0.0,
                                    _pad0: 0.0,
                                    color: [0.0, 0.0, 0.0, 0.0],
                                },
                                build_fullscreen_textured_bundle(fragment_body),
                                None,
                                None,
                            )
                        };

                        render_pass_specs.push(RenderPassSpec {
                            pass_id: compose_pass_name.as_str().to_string(),
                            name: compose_pass_name.clone(),
                            geometry_buffer: compose_geometry_buffer,
                            instance_buffer: None,
                            normals_buffer: None,
                            target_texture: comp_ctx.target_texture_name.clone(),
                            resolve_target: None,
                            params_buffer: compose_params_name,
                            baked_data_parse_buffer: None,
                            params: compose_params_val,
                            graph_binding: compose_graph_binding,
                            graph_values: compose_graph_values,
                            shader_wgsl: compose_bundle.module,
                            texture_bindings: vec![PassTextureBinding {
                                texture: pass_output_texture.clone(),
                                image_node_id: None,
                            }],
                            sampler_kinds: vec![SamplerKind::NearestClamp],
                            blend_state,
                            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                            sample_count: 1,
                        });
                        composite_passes.push(compose_pass_name);
                    }
                }

                // Register output so downstream PassTexture nodes can resolve it.
                pass_output_registry.register(PassOutputSpec {
                    node_id: layer_id.clone(),
                    texture_name: pass_output_texture,
                    resolution: [pass_target_w_u, pass_target_h_u],
                    format: if is_sampled_output {
                        sampled_pass_format
                    } else {
                        target_format
                    },
                });
            }
            "GuassianBlurPass" => {
                // GuassianBlurPass takes its source from `pass` input.
                // Scene prep may auto-wrap compatible non-pass sources into a fullscreen RenderPass.
                // We first sample that source pass into an intermediate texture, then apply the blur chain.

                // Determine the base resolution for this blur pass.
                // Blur operates in input source texture space by default.
                let mut base_resolution: [u32; 2] = [tgt_w_u, tgt_h_u];
                let mut blur_output_center: Option<[f32; 2]> = None;

                let radius_px =
                    cpu_num_f32_min_0(&prepared.scene, nodes_by_id, layer_node, "radius", 0.0)?;
                let extend_enabled = layer_node
                    .params
                    .get("extend")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let extend_pad_px: u32 = if extend_enabled {
                    radius_px.ceil().max(0.0) as u32
                } else {
                    0
                };
                let can_direct_bypass = extend_pad_px == 0;

                // Optimization: skip the intermediate `sys.blur.<id>.src` pass when we can
                // directly consume an existing texture resource as the blur source.
                //
                // Safe bypass cases:
                // - Upstream is a pass node output (RenderPass/Blur/Downsample) with sampled format.
                // - Upstream is an ImageTexture sampled as-is (default UV).
                let mut initial_blur_source_texture: Option<ResourceName> = None;
                let mut initial_blur_source_image_node_id: Option<String> = None;
                let mut initial_blur_source_sampler_kind: Option<SamplerKind> = None;
                if let Some(src_conn) = incoming_connection(&prepared.scene, layer_id, "pass") {
                    if let Some(src_node) = nodes_by_id.get(&src_conn.from.node_id) {
                        if src_node.node_type == "RenderPass" {
                            if let Some(geo_conn) = incoming_connection(
                                &prepared.scene,
                                &src_conn.from.node_id,
                                "geometry",
                            ) {
                                if let Ok((
                                    _,
                                    src_geo_w,
                                    src_geo_h,
                                    src_geo_x,
                                    src_geo_y,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                )) =
                                    crate::renderer::render_plan::resolve_geometry_for_render_pass(
                                        &prepared.scene,
                                        nodes_by_id,
                                        ids,
                                        &geo_conn.from.node_id,
                                        [tgt_w, tgt_h],
                                        None,
                                        asset_store,
                                    )
                                {
                                    base_resolution = [
                                        src_geo_w.max(1.0).round() as u32,
                                        src_geo_h.max(1.0).round() as u32,
                                    ];
                                    blur_output_center = Some([src_geo_x, src_geo_y]);
                                }
                            }
                        }
                    }

                    // (A) Upstream pass output bypass.
                    // When the blur input is already a pass output texture with sampled format,
                    // skip generating an extra fullscreen
                    // `sys.blur.<id>.src.pass` sampling pass.
                    if let Some(src_spec) = pass_output_registry.get(&src_conn.from.node_id) {
                        base_resolution = src_spec.resolution;
                        if can_direct_bypass && src_spec.format == sampled_pass_format {
                            initial_blur_source_texture = Some(src_spec.texture_name.clone());
                            // Match the sampler behavior the old `.src.pass` would have used.
                            initial_blur_source_sampler_kind = Some(sampler_kind_for_pass_texture(
                                &prepared.scene,
                                &src_conn.from.node_id,
                            ));
                        }
                    }

                    // (B) ImageTexture direct bypass (only when UV is default).
                    if initial_blur_source_texture.is_none() {
                        if let Some(src_node) = nodes_by_id.get(&src_conn.from.node_id) {
                            if src_node.node_type == "ImageTexture"
                                && src_conn.from.port_id == "color"
                                && incoming_connection(
                                    &prepared.scene,
                                    &src_conn.from.node_id,
                                    "uv",
                                )
                                .is_none()
                            {
                                if let Some(dims) = image_node_dimensions(src_node, asset_store) {
                                    base_resolution = dims;
                                }
                                if let Some(tex) = ids.get(&src_conn.from.node_id).cloned() {
                                    initial_blur_source_texture = Some(tex);
                                    initial_blur_source_image_node_id =
                                        Some(src_conn.from.node_id.clone());
                                    initial_blur_source_sampler_kind =
                                        Some(sampler_kind_from_node_params(&src_node.params));
                                }
                            }
                        }
                    }
                }
                let src_content_resolution = base_resolution;
                let src_resolution = if extend_pad_px > 0 {
                    [
                        src_content_resolution[0].saturating_add(extend_pad_px.saturating_mul(2)),
                        src_content_resolution[1].saturating_add(extend_pad_px.saturating_mul(2)),
                    ]
                } else {
                    src_content_resolution
                };
                let src_w = src_resolution[0] as f32;
                let src_h = src_resolution[1] as f32;
                let src_content_w = src_content_resolution[0] as f32;
                let src_content_h = src_content_resolution[1] as f32;
                let initial_blur_source_texture: ResourceName = if let Some(existing_tex) =
                    initial_blur_source_texture
                {
                    existing_tex
                } else {
                    // Create source texture for the pass input.
                    let src_tex: ResourceName = format!("sys.blur.{layer_id}.src").into();
                    textures.push(TextureDecl {
                        name: src_tex.clone(),
                        size: src_resolution,
                        format: sampled_pass_format,
                        sample_count: 1,
                    });

                    // Build source pass geometry. In Extend mode, draw source content with its
                    // original size centered in the padded target; otherwise fullscreen.
                    let src_geo_w = if extend_pad_px > 0 {
                        src_content_w
                    } else {
                        src_w
                    };
                    let src_geo_h = if extend_pad_px > 0 {
                        src_content_h
                    } else {
                        src_h
                    };

                    let geo_src: ResourceName = format!("sys.blur.{layer_id}.src.geo").into();
                    geometry_buffers.push((
                        geo_src.clone(),
                        make_fullscreen_geometry(src_geo_w, src_geo_h),
                    ));

                    let params_src: ResourceName = format!("params.sys.blur.{layer_id}.src").into();
                    let params_src_val = Params {
                        target_size: [src_w, src_h],
                        geo_size: [src_geo_w, src_geo_h],
                        // Center source content in the padded texture (or fullscreen if no extend).
                        center: [src_w * 0.5, src_h * 0.5],
                        geo_translate: [0.0, 0.0],
                        geo_scale: [1.0, 1.0],
                        time: 0.0,
                        _pad0: 0.0,
                        color: [0.0, 0.0, 0.0, 0.0],
                    };

                    // Build WGSL for sampling the `pass` input source.
                    let mut src_bundle =
                        build_blur_image_wgsl_bundle(&prepared.scene, nodes_by_id, layer_id)?;
                    let mut src_graph_binding: Option<GraphBinding> = None;
                    let mut src_graph_values: Option<Vec<u8>> = None;
                    if let Some(schema) = src_bundle.graph_schema.clone() {
                        let limits = device.limits();
                        let kind = choose_graph_binding_kind(
                            schema.size_bytes,
                            limits.max_uniform_buffer_binding_size as u64,
                            limits.max_storage_buffer_binding_size as u64,
                        )?;
                        if src_bundle.graph_binding_kind != Some(kind) {
                            src_bundle = build_blur_image_wgsl_bundle_with_graph_binding(
                                &prepared.scene,
                                nodes_by_id,
                                layer_id,
                                Some(kind),
                            )?;
                        }
                        let schema = src_bundle
                            .graph_schema
                            .clone()
                            .ok_or_else(|| anyhow!("missing blur source graph schema"))?;
                        let values = pack_graph_values(&prepared.scene, &schema)?;
                        src_graph_values = Some(values);
                        src_graph_binding = Some(GraphBinding {
                            buffer_name: format!("params.sys.blur.{layer_id}.src.graph").into(),
                            kind,
                            schema,
                        });
                    }
                    let mut src_texture_bindings: Vec<PassTextureBinding> = Vec::new();
                    let mut src_sampler_kinds: Vec<SamplerKind> = Vec::new();

                    for id in src_bundle.image_textures.iter() {
                        let Some(tex) = ids.get(id).cloned() else {
                            continue;
                        };
                        src_texture_bindings.push(PassTextureBinding {
                            texture: tex,
                            image_node_id: Some(id.clone()),
                        });
                        let kind = nodes_by_id
                            .get(id)
                            .map(|n| sampler_kind_from_node_params(&n.params))
                            .unwrap_or(SamplerKind::LinearClamp);
                        src_sampler_kinds.push(kind);
                    }

                    let src_pass_bindings =
                        crate::renderer::render_plan::resolve_pass_texture_bindings(
                            &pass_output_registry,
                            &src_bundle.pass_textures,
                        )?;
                    for (upstream_pass_id, binding) in
                        src_bundle.pass_textures.iter().zip(src_pass_bindings)
                    {
                        src_texture_bindings.push(binding);
                        src_sampler_kinds.push(sampler_kind_for_pass_texture(
                            &prepared.scene,
                            upstream_pass_id,
                        ));
                    }

                    let src_pass_name: ResourceName =
                        format!("sys.blur.{layer_id}.src.pass").into();
                    render_pass_specs.push(RenderPassSpec {
                        pass_id: src_pass_name.as_str().to_string(),
                        name: src_pass_name.clone(),
                        geometry_buffer: geo_src,
                        instance_buffer: None,
                        normals_buffer: None,
                        target_texture: src_tex.clone(),
                        resolve_target: None,
                        params_buffer: params_src.clone(),
                        baked_data_parse_buffer: None,
                        params: params_src_val,
                        graph_binding: src_graph_binding,
                        graph_values: src_graph_values,
                        shader_wgsl: src_bundle.module,
                        texture_bindings: src_texture_bindings,
                        sampler_kinds: src_sampler_kinds,
                        blend_state: BlendState::REPLACE,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                        sample_count: 1,
                    });
                    composite_passes.push(src_pass_name);
                    src_tex
                };

                // Resolution: use source resolution (possibly extended), but allow override via params.
                let blur_w = cpu_num_u32_min_1(
                    &prepared.scene,
                    nodes_by_id,
                    layer_node,
                    "width",
                    src_resolution[0],
                )?;
                let blur_h = cpu_num_u32_min_1(
                    &prepared.scene,
                    nodes_by_id,
                    layer_node,
                    "height",
                    src_resolution[1],
                )?;

                // SceneDSL `radius` is authored as an analytic 1D cutoff radius in full-res pixels,
                // not as Gaussian sigma.
                //
                // We map radius -> sigma using the same cutoff epsilon (~0.002) that our packed
                // 27-wide Gaussian kernel effectively uses when pruning tiny weights
                // (see `gaussian_kernel_8`).
                //
                // k = sqrt(2*ln(1/eps)) with eps=0.002 -> k≈3.525494, so sigma = radius/k.
                let sigma = radius_px / 3.525_494;
                let (mip_level, sigma_p) = gaussian_mip_level_and_sigma_p(sigma);
                let downsample_factor: u32 = 1 << mip_level;
                let (kernel, offset, num) = gaussian_kernel_8(sigma_p.max(1e-6));
                let tap_count = num.clamp(1, 8);
                let is_sampled_output = sampled_pass_ids.contains(layer_id);
                let skip_factor1_downsample = should_skip_blur_downsample_pass(downsample_factor);
                let skip_factor1_upsample = should_skip_blur_upsample_pass(
                    downsample_factor,
                    extend_enabled,
                    is_sampled_output,
                );
                let emit_upsample_pass = !skip_factor1_upsample;

                let downsample_steps: Vec<u32> = if skip_factor1_downsample {
                    Vec::new()
                } else {
                    blur_downsample_steps_for_factor(downsample_factor)?
                };

                // Allocate textures (and matching fullscreen geometry) for each downsample step.
                // Use blur_w/blur_h as the base resolution (inherited from upstream or overridden).
                let mut step_textures: Vec<(u32, ResourceName, u32, u32, ResourceName)> =
                    Vec::new();
                let mut cur_w: u32 = blur_w;
                let mut cur_h: u32 = blur_h;
                for step in &downsample_steps {
                    let shift = match *step {
                        1 => 0,
                        2 => 1,
                        4 => 2,
                        8 => 3,
                        other => bail!("GuassianBlurPass: unsupported downsample factor {other}"),
                    };
                    let next_w = clamp_min_1(cur_w >> shift);
                    let next_h = clamp_min_1(cur_h >> shift);
                    let tex: ResourceName = format!("sys.blur.{layer_id}.ds.{step}").into();
                    textures.push(TextureDecl {
                        name: tex.clone(),
                        size: [next_w, next_h],
                        format: sampled_pass_format,
                        sample_count: 1,
                    });
                    let geo: ResourceName = format!("sys.blur.{layer_id}.ds.{step}.geo").into();
                    geometry_buffers.push((
                        geo.clone(),
                        make_fullscreen_geometry(next_w as f32, next_h as f32),
                    ));
                    step_textures.push((*step, tex, next_w, next_h, geo));
                    cur_w = next_w;
                    cur_h = next_h;
                }

                let ds_w = cur_w;
                let ds_h = cur_h;

                let h_tex: ResourceName = format!("sys.blur.{layer_id}.h").into();
                let v_tex: ResourceName = format!("sys.blur.{layer_id}.v").into();

                textures.push(TextureDecl {
                    name: h_tex.clone(),
                    size: [ds_w, ds_h],
                    format: sampled_pass_format,
                    sample_count: 1,
                });
                textures.push(TextureDecl {
                    name: v_tex.clone(),
                    size: [ds_w, ds_h],
                    format: sampled_pass_format,
                    sample_count: 1,
                });

                // If this blur pass is sampled downstream (PassTexture), render into an intermediate output.
                // Otherwise, render to the final Composite.target texture.
                let output_tex: ResourceName = if is_sampled_output {
                    if emit_upsample_pass {
                        let out_tex: ResourceName = format!("sys.blur.{layer_id}.out").into();
                        textures.push(TextureDecl {
                            name: out_tex.clone(),
                            size: [blur_w, blur_h],
                            format: sampled_pass_format,
                            sample_count: 1,
                        });
                        out_tex
                    } else {
                        // Factor=1 sampled blur can directly publish v_tex.
                        v_tex.clone()
                    }
                } else {
                    target_texture_name.clone()
                };
                let upsample_geo_size: Option<[f32; 2]> = if emit_upsample_pass {
                    Some(if extend_pad_px > 0 && output_tex == target_texture_name {
                        gaussian_blur_extend_upsample_geo_size(
                            src_content_resolution,
                            [blur_w, blur_h],
                        )
                    } else {
                        [blur_w as f32, blur_h as f32]
                    })
                } else {
                    None
                };

                let pass_blend_state =
                    crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
                        .with_context(|| {
                            format!(
                                "invalid blend params for {}",
                                crate::dsl::node_display_label_with_id(layer_node)
                            )
                        })?;
                let blur_output_blend_state: BlendState = if output_tex == target_texture_name {
                    pass_blend_state
                } else {
                    BlendState::REPLACE
                };

                // Fullscreen geometry buffers for blur + upsample.
                let geo_ds: ResourceName = format!("sys.blur.{layer_id}.ds.geo").into();
                geometry_buffers.push((
                    geo_ds.clone(),
                    make_fullscreen_geometry(ds_w as f32, ds_h as f32),
                ));
                let geo_out: Option<ResourceName> =
                    if let Some(upsample_geo_size) = upsample_geo_size {
                        let geo_out: ResourceName = format!("sys.blur.{layer_id}.out.geo").into();
                        geometry_buffers.push((
                            geo_out.clone(),
                            make_fullscreen_geometry(upsample_geo_size[0], upsample_geo_size[1]),
                        ));
                        Some(geo_out)
                    } else {
                        None
                    };

                // Downsample chain
                let mut prev_tex: Option<ResourceName> = None;
                for (step, tex, step_w, step_h, step_geo) in &step_textures {
                    let params_name: ResourceName =
                        format!("params.sys.blur.{layer_id}.ds.{step}").into();
                    let bundle = build_downsample_bundle(*step)?;

                    let sampler_kind = if prev_tex.is_none() {
                        initial_blur_source_sampler_kind.unwrap_or(SamplerKind::LinearMirror)
                    } else {
                        SamplerKind::LinearMirror
                    };

                    let params_val = Params {
                        target_size: [*step_w as f32, *step_h as f32],
                        geo_size: [*step_w as f32, *step_h as f32],
                        center: [*step_w as f32 * 0.5, *step_h as f32 * 0.5],
                        geo_translate: [0.0, 0.0],
                        geo_scale: [1.0, 1.0],
                        time: 0.0,
                        _pad0: 0.0,
                        color: [0.0, 0.0, 0.0, 0.0],
                    };

                    let (src_tex, src_image_node_id) = match &prev_tex {
                        None => (
                            initial_blur_source_texture.clone(),
                            initial_blur_source_image_node_id.clone(),
                        ),
                        Some(t) => (t.clone(), None),
                    };

                    let baked_buf: ResourceName =
                        format!("sys.pass.{layer_id}.baked_data_parse").into();
                    baked_data_parse_buffer_to_pass_id
                        .entry(baked_buf.clone())
                        .or_insert_with(|| layer_id.clone());

                    let pass_name: ResourceName =
                        format!("sys.blur.{layer_id}.ds.{step}.pass").into();
                    render_pass_specs.push(RenderPassSpec {
                        pass_id: pass_name.as_str().to_string(),
                        name: pass_name.clone(),
                        geometry_buffer: step_geo.clone(),
                        instance_buffer: None,
                        normals_buffer: None,
                        target_texture: tex.clone(),
                        resolve_target: None,
                        params_buffer: params_name,
                        baked_data_parse_buffer: Some(baked_buf),
                        params: params_val,
                        graph_binding: None,
                        graph_values: None,
                        shader_wgsl: bundle.module,
                        texture_bindings: vec![PassTextureBinding {
                            texture: src_tex,
                            image_node_id: src_image_node_id,
                        }],
                        sampler_kinds: vec![sampler_kind],
                        blend_state: BlendState::REPLACE,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                        sample_count: 1,
                    });
                    composite_passes.push(pass_name);
                    prev_tex = Some(tex.clone());
                }

                let (ds_src_tex, ds_src_image_node_id): (ResourceName, Option<String>) =
                    if let Some(prev_tex) = prev_tex {
                        (prev_tex, None)
                    } else {
                        (
                            initial_blur_source_texture.clone(),
                            initial_blur_source_image_node_id.clone(),
                        )
                    };

                // 2) Horizontal blur: ds_src_tex -> h_tex
                let params_h: ResourceName =
                    format!("params.sys.blur.{layer_id}.h.ds{downsample_factor}").into();
                let bundle_h =
                    build_horizontal_blur_bundle_with_tap_count(kernel, offset, tap_count);
                let params_h_val = Params {
                    target_size: [ds_w as f32, ds_h as f32],
                    geo_size: [ds_w as f32, ds_h as f32],
                    center: [ds_w as f32 * 0.5, ds_h as f32 * 0.5],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                };

                let pass_name_h: ResourceName =
                    format!("sys.blur.{layer_id}.h.ds{downsample_factor}.pass").into();
                render_pass_specs.push(RenderPassSpec {
                    pass_id: pass_name_h.as_str().to_string(),
                    name: pass_name_h.clone(),
                    geometry_buffer: geo_ds.clone(),
                    instance_buffer: None,
                    normals_buffer: None,
                    target_texture: h_tex.clone(),
                    resolve_target: None,
                    params_buffer: params_h.clone(),
                    baked_data_parse_buffer: None,
                    params: params_h_val,
                    graph_binding: None,
                    graph_values: None,
                    shader_wgsl: bundle_h.module,
                    texture_bindings: vec![PassTextureBinding {
                        texture: ds_src_tex.clone(),
                        image_node_id: ds_src_image_node_id,
                    }],
                    sampler_kinds: vec![SamplerKind::LinearMirror],
                    blend_state: BlendState::REPLACE,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                    sample_count: 1,
                });
                composite_passes.push(pass_name_h);

                // 3) Vertical blur: h_tex -> v_tex (still downsampled resolution)
                let params_v: ResourceName =
                    format!("params.sys.blur.{layer_id}.v.ds{downsample_factor}").into();
                let bundle_v = build_vertical_blur_bundle_with_tap_count(kernel, offset, tap_count);
                let pass_name_v: ResourceName =
                    format!("sys.blur.{layer_id}.v.ds{downsample_factor}.pass").into();
                let params_v_val = Params {
                    target_size: [ds_w as f32, ds_h as f32],
                    geo_size: [ds_w as f32, ds_h as f32],
                    center: [ds_w as f32 * 0.5, ds_h as f32 * 0.5],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                };
                render_pass_specs.push(RenderPassSpec {
                    pass_id: pass_name_v.as_str().to_string(),
                    name: pass_name_v.clone(),
                    geometry_buffer: geo_ds.clone(),
                    instance_buffer: None,
                    normals_buffer: None,
                    target_texture: v_tex.clone(),
                    resolve_target: None,
                    params_buffer: params_v.clone(),
                    baked_data_parse_buffer: None,
                    params: params_v_val,
                    graph_binding: None,
                    graph_values: None,
                    shader_wgsl: bundle_v.module,
                    texture_bindings: vec![PassTextureBinding {
                        texture: h_tex.clone(),
                        image_node_id: None,
                    }],
                    sampler_kinds: vec![SamplerKind::LinearMirror],
                    blend_state: BlendState::REPLACE,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                    sample_count: 1,
                });

                composite_passes.push(pass_name_v);

                // 4) Upsample bilinear back to output: v_tex -> output_tex
                if emit_upsample_pass {
                    let upsample_geo_size = upsample_geo_size
                        .ok_or_else(|| anyhow!("GuassianBlurPass: missing upsample geo size"))?;
                    let geo_out = geo_out
                        .clone()
                        .ok_or_else(|| anyhow!("GuassianBlurPass: missing upsample geometry"))?;
                    let params_u: ResourceName = format!(
                        "params.sys.blur.{layer_id}.upsample_bilinear.ds{downsample_factor}"
                    )
                    .into();
                    let bundle_u = build_upsample_bilinear_bundle();
                    let upsample_target_size: [f32; 2] = if output_tex == target_texture_name {
                        [tgt_w, tgt_h]
                    } else {
                        [blur_w as f32, blur_h as f32]
                    };
                    let upsample_center: [f32; 2] = if output_tex == target_texture_name {
                        blur_output_center
                            .unwrap_or([upsample_geo_size[0] * 0.5, upsample_geo_size[1] * 0.5])
                    } else {
                        [upsample_geo_size[0] * 0.5, upsample_geo_size[1] * 0.5]
                    };
                    let params_u_val = Params {
                        target_size: upsample_target_size,
                        geo_size: upsample_geo_size,
                        center: upsample_center,
                        geo_translate: [0.0, 0.0],
                        geo_scale: [1.0, 1.0],
                        time: 0.0,
                        _pad0: 0.0,
                        color: [0.0, 0.0, 0.0, 0.0],
                    };
                    let pass_name_u: ResourceName =
                        format!("sys.blur.{layer_id}.upsample_bilinear.ds{downsample_factor}.pass")
                            .into();
                    render_pass_specs.push(RenderPassSpec {
                        pass_id: pass_name_u.as_str().to_string(),
                        name: pass_name_u.clone(),
                        geometry_buffer: geo_out,
                        instance_buffer: None,
                        normals_buffer: None,
                        target_texture: output_tex.clone(),
                        resolve_target: None,
                        params_buffer: params_u.clone(),
                        baked_data_parse_buffer: None,
                        params: params_u_val,
                        graph_binding: None,
                        graph_values: None,
                        shader_wgsl: bundle_u.module,
                        texture_bindings: vec![PassTextureBinding {
                            texture: v_tex.clone(),
                            image_node_id: None,
                        }],
                        sampler_kinds: vec![SamplerKind::LinearMirror],
                        blend_state: blur_output_blend_state,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                        sample_count: 1,
                    });

                    composite_passes.push(pass_name_u);
                }

                // Register this GuassianBlurPass output for potential downstream chaining.
                let blur_output_tex = output_tex.clone();
                pass_output_registry.register(PassOutputSpec {
                    node_id: layer_id.clone(),
                    texture_name: blur_output_tex.clone(),
                    resolution: [blur_w, blur_h],
                    format: if is_sampled_output {
                        sampled_pass_format
                    } else {
                        target_format
                    },
                });

                let composition_consumers = composition_consumers_by_source
                    .get(layer_id)
                    .cloned()
                    .unwrap_or_default();
                for composition_id in composition_consumers {
                    let Some(comp_ctx) = composition_contexts.get(&composition_id) else {
                        continue;
                    };
                    if blur_output_tex == comp_ctx.target_texture_name {
                        continue;
                    }

                    let comp_w = comp_ctx.target_size_px[0];
                    let comp_h = comp_ctx.target_size_px[1];
                    let compose_geo: ResourceName =
                        format!("sys.blur.{layer_id}.to.{composition_id}.compose.geo").into();
                    geometry_buffers.push((
                        compose_geo.clone(),
                        make_fullscreen_geometry(blur_w as f32, blur_h as f32),
                    ));
                    let compose_pass_name: ResourceName =
                        format!("sys.blur.{layer_id}.to.{composition_id}.compose.pass").into();
                    let compose_params_name: ResourceName =
                        format!("params.sys.blur.{layer_id}.to.{composition_id}.compose").into();
                    let compose_params = Params {
                        target_size: [comp_w, comp_h],
                        geo_size: [blur_w as f32, blur_h as f32],
                        center: blur_output_center.unwrap_or([comp_w * 0.5, comp_h * 0.5]),
                        geo_translate: [0.0, 0.0],
                        geo_scale: [1.0, 1.0],
                        time: 0.0,
                        _pad0: 0.0,
                        color: [0.0, 0.0, 0.0, 0.0],
                    };

                    render_pass_specs.push(RenderPassSpec {
                        pass_id: compose_pass_name.as_str().to_string(),
                        name: compose_pass_name.clone(),
                        geometry_buffer: compose_geo,
                        instance_buffer: None,
                        normals_buffer: None,
                        target_texture: comp_ctx.target_texture_name.clone(),
                        resolve_target: None,
                        params_buffer: compose_params_name,
                        baked_data_parse_buffer: None,
                        params: compose_params,
                        graph_binding: None,
                        graph_values: None,
                        shader_wgsl: build_fullscreen_textured_bundle(
                            "return textureSample(src_tex, src_samp, in.uv);".to_string(),
                        )
                        .module,
                        texture_bindings: vec![PassTextureBinding {
                            texture: blur_output_tex.clone(),
                            image_node_id: None,
                        }],
                        sampler_kinds: vec![SamplerKind::LinearClamp],
                        blend_state: pass_blend_state,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                        sample_count: 1,
                    });
                    composite_passes.push(compose_pass_name);
                }
            }
            "GradientBlur" => {
                use crate::renderer::wgsl_gradient_blur::*;

                // ---------- resolve source dimensions ----------
                let mut gb_src_resolution: [u32; 2] = [tgt_w_u, tgt_h_u];
                let mut gb_output_center: Option<[f32; 2]> = None;

                if let Some(src_conn) = incoming_connection(&prepared.scene, layer_id, "source") {
                    if let Some(src_node) = nodes_by_id.get(&src_conn.from.node_id) {
                        if src_node.node_type == "RenderPass" {
                            if let Some(geo_conn) = incoming_connection(
                                &prepared.scene,
                                &src_conn.from.node_id,
                                "geometry",
                            ) {
                                if let Ok((
                                    _,
                                    src_geo_w,
                                    src_geo_h,
                                    src_geo_x,
                                    src_geo_y,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                )) =
                                    crate::renderer::render_plan::resolve_geometry_for_render_pass(
                                        &prepared.scene,
                                        nodes_by_id,
                                        ids,
                                        &geo_conn.from.node_id,
                                        [tgt_w, tgt_h],
                                        None,
                                        asset_store,
                                    )
                                {
                                    gb_src_resolution = [
                                        src_geo_w.max(1.0).round() as u32,
                                        src_geo_h.max(1.0).round() as u32,
                                    ];
                                    gb_output_center = Some([src_geo_x, src_geo_y]);
                                }
                            }
                        }
                    }

                    // (A) Upstream pass output.
                    if let Some(src_spec) = pass_output_registry.get(&src_conn.from.node_id) {
                        gb_src_resolution = src_spec.resolution;
                    }
                    // (B) Direct ImageTexture.
                    if let Some(src_node) = nodes_by_id.get(&src_conn.from.node_id) {
                        if src_node.node_type == "ImageTexture" {
                            if let Some(dims) = image_node_dimensions(src_node, asset_store) {
                                gb_src_resolution = dims;
                            }
                        }
                    }
                }

                let [padded_w, padded_h] =
                    gradient_blur_padded_size(gb_src_resolution[0], gb_src_resolution[1]);
                let src_w = gb_src_resolution[0] as f32;
                let src_h = gb_src_resolution[1] as f32;
                let pad_w = padded_w as f32;
                let pad_h = padded_h as f32;
                let pad_offset_x = (pad_w - src_w) * 0.5;
                let pad_offset_y = (pad_h - src_h) * 0.5;

                let is_sampled_output = sampled_pass_ids.contains(layer_id);

                // ---------- source pass ----------
                // Attempt direct bypass (ImageTexture or upstream pass output).
                let mut initial_source_texture: Option<ResourceName> = None;
                let mut initial_source_image_node_id: Option<String> = None;
                let mut initial_source_sampler_kind: Option<SamplerKind> = None;

                if let Some(src_conn) = incoming_connection(&prepared.scene, layer_id, "source") {
                    // (A) upstream pass output bypass
                    if let Some(spec) = pass_output_registry.get(&src_conn.from.node_id) {
                        if spec.format == sampled_pass_format {
                            initial_source_texture = Some(spec.texture_name.clone());
                            initial_source_sampler_kind = Some(sampler_kind_for_pass_texture(
                                &prepared.scene,
                                &src_conn.from.node_id,
                            ));
                        }
                    }
                    // (B) direct ImageTexture bypass
                    if initial_source_texture.is_none() {
                        if let Some(src_node) = nodes_by_id.get(&src_conn.from.node_id) {
                            if src_node.node_type == "ImageTexture"
                                && src_conn.from.port_id == "color"
                                && incoming_connection(
                                    &prepared.scene,
                                    &src_conn.from.node_id,
                                    "uv",
                                )
                                .is_none()
                            {
                                if let Some(tex) = ids.get(&src_conn.from.node_id).cloned() {
                                    initial_source_texture = Some(tex);
                                    initial_source_image_node_id =
                                        Some(src_conn.from.node_id.clone());
                                    initial_source_sampler_kind =
                                        Some(sampler_kind_from_node_params(&src_node.params));
                                }
                            }
                        }
                    }
                }

                let source_texture: ResourceName = if let Some(existing_tex) =
                    initial_source_texture
                {
                    existing_tex
                } else {
                    // Create intermediate source texture.
                    let src_tex: ResourceName = format!("sys.gb.{layer_id}.src").into();
                    textures.push(TextureDecl {
                        name: src_tex.clone(),
                        size: gb_src_resolution,
                        format: sampled_pass_format,
                        sample_count: 1,
                    });

                    let geo_src: ResourceName = format!("sys.gb.{layer_id}.src.geo").into();
                    geometry_buffers
                        .push((geo_src.clone(), make_fullscreen_geometry(src_w, src_h)));

                    let params_src: ResourceName = format!("params.sys.gb.{layer_id}.src").into();
                    let params_src_val = Params {
                        target_size: [src_w, src_h],
                        geo_size: [src_w, src_h],
                        center: [src_w * 0.5, src_h * 0.5],
                        geo_translate: [0.0, 0.0],
                        geo_scale: [1.0, 1.0],
                        time: 0.0,
                        _pad0: 0.0,
                        color: [0.0, 0.0, 0.0, 0.0],
                    };

                    let mut src_bundle = build_gradient_blur_source_wgsl_bundle(
                        &prepared.scene,
                        nodes_by_id,
                        layer_id,
                    )?;
                    let mut src_graph_binding: Option<GraphBinding> = None;
                    let mut src_graph_values: Option<Vec<u8>> = None;
                    if let Some(schema) = src_bundle.graph_schema.clone() {
                        let limits = device.limits();
                        let kind = choose_graph_binding_kind(
                            schema.size_bytes,
                            limits.max_uniform_buffer_binding_size as u64,
                            limits.max_storage_buffer_binding_size as u64,
                        )?;
                        if src_bundle.graph_binding_kind != Some(kind) {
                            src_bundle = build_gradient_blur_source_wgsl_bundle_with_graph_binding(
                                &prepared.scene,
                                nodes_by_id,
                                layer_id,
                                Some(kind),
                            )?;
                        }
                        let schema = src_bundle
                            .graph_schema
                            .clone()
                            .ok_or_else(|| anyhow!("missing gb source graph schema"))?;
                        let values = pack_graph_values(&prepared.scene, &schema)?;
                        src_graph_values = Some(values);
                        src_graph_binding = Some(GraphBinding {
                            buffer_name: format!("params.sys.gb.{layer_id}.src.graph").into(),
                            kind,
                            schema,
                        });
                    }

                    let mut src_texture_bindings: Vec<PassTextureBinding> = Vec::new();
                    let mut src_sampler_kinds: Vec<SamplerKind> = Vec::new();

                    for id in src_bundle.image_textures.iter() {
                        let Some(tex) = ids.get(id).cloned() else {
                            continue;
                        };
                        src_texture_bindings.push(PassTextureBinding {
                            texture: tex,
                            image_node_id: Some(id.clone()),
                        });
                        let kind = nodes_by_id
                            .get(id)
                            .map(|n| sampler_kind_from_node_params(&n.params))
                            .unwrap_or(SamplerKind::LinearClamp);
                        src_sampler_kinds.push(kind);
                    }

                    let src_pass_bindings = resolve_pass_texture_bindings(
                        &pass_output_registry,
                        &src_bundle.pass_textures,
                    )?;
                    for (upstream_pass_id, binding) in
                        src_bundle.pass_textures.iter().zip(src_pass_bindings)
                    {
                        src_texture_bindings.push(binding);
                        src_sampler_kinds.push(sampler_kind_for_pass_texture(
                            &prepared.scene,
                            upstream_pass_id,
                        ));
                    }

                    let src_pass_name: ResourceName = format!("sys.gb.{layer_id}.src.pass").into();
                    render_pass_specs.push(RenderPassSpec {
                        pass_id: src_pass_name.as_str().to_string(),
                        name: src_pass_name.clone(),
                        geometry_buffer: geo_src,
                        instance_buffer: None,
                        normals_buffer: None,
                        target_texture: src_tex.clone(),
                        resolve_target: None,
                        params_buffer: params_src.clone(),
                        baked_data_parse_buffer: None,
                        params: params_src_val,
                        graph_binding: src_graph_binding,
                        graph_values: src_graph_values,
                        shader_wgsl: src_bundle.module,
                        texture_bindings: src_texture_bindings,
                        sampler_kinds: src_sampler_kinds,
                        blend_state: BlendState::REPLACE,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                        sample_count: 1,
                    });
                    composite_passes.push(src_pass_name);
                    src_tex
                };

                // ---------- pad pass ----------
                let pad_tex: ResourceName = format!("sys.gb.{layer_id}.pad").into();
                textures.push(TextureDecl {
                    name: pad_tex.clone(),
                    size: [padded_w, padded_h],
                    format: sampled_pass_format,
                    sample_count: 1,
                });

                let pad_geo: ResourceName = format!("sys.gb.{layer_id}.pad.geo").into();
                geometry_buffers.push((pad_geo.clone(), make_fullscreen_geometry(pad_w, pad_h)));

                let params_pad: ResourceName = format!("params.sys.gb.{layer_id}.pad").into();
                let params_pad_val = Params {
                    target_size: [pad_w, pad_h],
                    geo_size: [pad_w, pad_h],
                    center: [pad_w * 0.5, pad_h * 0.5],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                };

                let pad_bundle = build_gradient_blur_pad_wgsl_bundle(src_w, src_h, pad_w, pad_h);

                let pad_pass_name: ResourceName = format!("sys.gb.{layer_id}.pad.pass").into();
                render_pass_specs.push(RenderPassSpec {
                    pass_id: pad_pass_name.as_str().to_string(),
                    name: pad_pass_name.clone(),
                    geometry_buffer: pad_geo,
                    instance_buffer: None,
                    normals_buffer: None,
                    target_texture: pad_tex.clone(),
                    resolve_target: None,
                    params_buffer: params_pad,
                    baked_data_parse_buffer: None,
                    params: params_pad_val,
                    graph_binding: None,
                    graph_values: None,
                    shader_wgsl: pad_bundle.module,
                    texture_bindings: vec![PassTextureBinding {
                        texture: source_texture.clone(),
                        image_node_id: initial_source_image_node_id.clone(),
                    }],
                    // Always use mirror-repeat for the pad pass so the
                    // padding region reflects the source content.
                    sampler_kinds: vec![SamplerKind::LinearMirror],
                    blend_state: BlendState::REPLACE,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                    sample_count: 1,
                });
                composite_passes.push(pad_pass_name);

                // ---------- mip chain ----------
                let mip_pass_ids: Vec<String> = (0..GB_MIP_LEVELS)
                    .map(|i| {
                        if i == 0 {
                            format!("sys.gb.{layer_id}.pad")
                        } else {
                            format!("sys.gb.{layer_id}.mip{i}")
                        }
                    })
                    .collect();

                let mut prev_mip_tex = pad_tex.clone();
                let mut cur_mip_w = padded_w;
                let mut cur_mip_h = padded_h;

                for i in 1..GB_MIP_LEVELS {
                    cur_mip_w = clamp_min_1(cur_mip_w / 2);
                    cur_mip_h = clamp_min_1(cur_mip_h / 2);
                    let mip_tex: ResourceName = format!("sys.gb.{layer_id}.mip{i}").into();
                    textures.push(TextureDecl {
                        name: mip_tex.clone(),
                        size: [cur_mip_w, cur_mip_h],
                        format: sampled_pass_format,
                        sample_count: 1,
                    });

                    let mip_geo: ResourceName = format!("sys.gb.{layer_id}.mip{i}.geo").into();
                    geometry_buffers.push((
                        mip_geo.clone(),
                        make_fullscreen_geometry(cur_mip_w as f32, cur_mip_h as f32),
                    ));

                    let params_mip: ResourceName =
                        format!("params.sys.gb.{layer_id}.mip{i}").into();
                    let params_mip_val = Params {
                        target_size: [cur_mip_w as f32, cur_mip_h as f32],
                        geo_size: [cur_mip_w as f32, cur_mip_h as f32],
                        center: [cur_mip_w as f32 * 0.5, cur_mip_h as f32 * 0.5],
                        geo_translate: [0.0, 0.0],
                        geo_scale: [1.0, 1.0],
                        time: 0.0,
                        _pad0: 0.0,
                        color: [0.0, 0.0, 0.0, 0.0],
                    };

                    let ds_bundle = crate::renderer::wgsl::build_downsample_pass_wgsl_bundle(
                        &gradient_blur_cross_kernel(),
                    )?;

                    let mip_pass_name: ResourceName =
                        format!("sys.gb.{layer_id}.mip{i}.pass").into();
                    render_pass_specs.push(RenderPassSpec {
                        pass_id: mip_pass_name.as_str().to_string(),
                        name: mip_pass_name.clone(),
                        geometry_buffer: mip_geo,
                        instance_buffer: None,
                        normals_buffer: None,
                        target_texture: mip_tex.clone(),
                        resolve_target: None,
                        params_buffer: params_mip,
                        baked_data_parse_buffer: None,
                        params: params_mip_val,
                        graph_binding: None,
                        graph_values: None,
                        shader_wgsl: ds_bundle.module,
                        texture_bindings: vec![PassTextureBinding {
                            texture: prev_mip_tex.clone(),
                            image_node_id: None,
                        }],
                        sampler_kinds: vec![SamplerKind::LinearMirror],
                        blend_state: BlendState::REPLACE,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                        sample_count: 1,
                    });
                    composite_passes.push(mip_pass_name);
                    prev_mip_tex = mip_tex;
                }

                // ---------- register mip pass outputs ----------
                // Register each mip texture so the composite pass can resolve them.
                for (i, mip_id) in mip_pass_ids.iter().enumerate() {
                    let mip_w = clamp_min_1(padded_w >> i);
                    let mip_h = clamp_min_1(padded_h >> i);
                    let tex_name: ResourceName = mip_id.clone().into();
                    pass_output_registry.register(PassOutputSpec {
                        node_id: mip_id.clone(),
                        texture_name: tex_name,
                        resolution: [mip_w, mip_h],
                        format: sampled_pass_format,
                    });
                }

                // ---------- composite/final pass ----------
                let output_tex: ResourceName = if is_sampled_output {
                    let out: ResourceName = format!("sys.gb.{layer_id}.out").into();
                    textures.push(TextureDecl {
                        name: out.clone(),
                        size: gb_src_resolution,
                        format: sampled_pass_format,
                        sample_count: 1,
                    });
                    out
                } else {
                    target_texture_name.clone()
                };

                let final_geo: ResourceName = format!("sys.gb.{layer_id}.final.geo").into();
                geometry_buffers.push((final_geo.clone(), make_fullscreen_geometry(src_w, src_h)));

                let params_final: ResourceName = format!("params.sys.gb.{layer_id}.final").into();
                let final_target_size = if output_tex == target_texture_name {
                    [tgt_w, tgt_h]
                } else {
                    [src_w, src_h]
                };
                let final_center = if output_tex == target_texture_name {
                    gb_output_center.unwrap_or([src_w * 0.5, src_h * 0.5])
                } else {
                    [src_w * 0.5, src_h * 0.5]
                };
                let params_final_val = Params {
                    target_size: final_target_size,
                    geo_size: [src_w, src_h],
                    center: final_center,
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                };

                let mut composite_bundle = build_gradient_blur_composite_wgsl_bundle(
                    &prepared.scene,
                    nodes_by_id,
                    layer_id,
                    &mip_pass_ids,
                    [pad_w, pad_h],
                    [pad_offset_x, pad_offset_y],
                )?;

                let mut final_graph_binding: Option<GraphBinding> = None;
                let mut final_graph_values: Option<Vec<u8>> = None;
                if let Some(schema) = composite_bundle.graph_schema.clone() {
                    let limits = device.limits();
                    let kind = choose_graph_binding_kind(
                        schema.size_bytes,
                        limits.max_uniform_buffer_binding_size as u64,
                        limits.max_storage_buffer_binding_size as u64,
                    )?;
                    if composite_bundle.graph_binding_kind != Some(kind) {
                        composite_bundle =
                            build_gradient_blur_composite_wgsl_bundle_with_graph_binding(
                                &prepared.scene,
                                nodes_by_id,
                                layer_id,
                                &mip_pass_ids,
                                [pad_w, pad_h],
                                [pad_offset_x, pad_offset_y],
                                Some(kind),
                            )?;
                    }
                    let schema = composite_bundle
                        .graph_schema
                        .clone()
                        .ok_or_else(|| anyhow!("missing gb composite graph schema"))?;
                    let values = pack_graph_values(&prepared.scene, &schema)?;
                    final_graph_values = Some(values);
                    final_graph_binding = Some(GraphBinding {
                        buffer_name: format!("params.sys.gb.{layer_id}.final.graph").into(),
                        kind,
                        schema,
                    });
                }

                // Build texture bindings for the composite pass.
                let mut final_texture_bindings: Vec<PassTextureBinding> = Vec::new();
                let mut final_sampler_kinds: Vec<SamplerKind> = Vec::new();

                // Image textures from mask expression.
                for id in composite_bundle.image_textures.iter() {
                    let Some(tex) = ids.get(id).cloned() else {
                        continue;
                    };
                    final_texture_bindings.push(PassTextureBinding {
                        texture: tex,
                        image_node_id: Some(id.clone()),
                    });
                    let kind = nodes_by_id
                        .get(id)
                        .map(|n| sampler_kind_from_node_params(&n.params))
                        .unwrap_or(SamplerKind::LinearClamp);
                    final_sampler_kinds.push(kind);
                }

                // Pass textures (mip textures + any from mask expression).
                let final_pass_bindings = resolve_pass_texture_bindings(
                    &pass_output_registry,
                    &composite_bundle.pass_textures,
                )?;
                for (upstream_pass_id, binding) in composite_bundle
                    .pass_textures
                    .iter()
                    .zip(final_pass_bindings)
                {
                    final_texture_bindings.push(binding);
                    // Use LinearClamp for mip textures (hardware bilinear in shader).
                    if upstream_pass_id.contains("sys.gb.") {
                        final_sampler_kinds.push(SamplerKind::LinearClamp);
                    } else {
                        final_sampler_kinds.push(sampler_kind_for_pass_texture(
                            &prepared.scene,
                            upstream_pass_id,
                        ));
                    }
                }

                let pass_blend_state =
                    crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
                        .with_context(|| {
                            format!(
                                "invalid blend params for {}",
                                crate::dsl::node_display_label_with_id(layer_node)
                            )
                        })?;
                let final_blend_state: BlendState = if output_tex == target_texture_name {
                    pass_blend_state
                } else {
                    BlendState::REPLACE
                };

                let final_pass_name: ResourceName = format!("sys.gb.{layer_id}.final.pass").into();
                render_pass_specs.push(RenderPassSpec {
                    pass_id: final_pass_name.as_str().to_string(),
                    name: final_pass_name.clone(),
                    geometry_buffer: final_geo,
                    instance_buffer: None,
                    normals_buffer: None,
                    target_texture: output_tex.clone(),
                    resolve_target: None,
                    params_buffer: params_final,
                    baked_data_parse_buffer: None,
                    params: params_final_val,
                    graph_binding: final_graph_binding,
                    graph_values: final_graph_values,
                    shader_wgsl: composite_bundle.module,
                    texture_bindings: final_texture_bindings,
                    sampler_kinds: final_sampler_kinds,
                    blend_state: final_blend_state,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                    sample_count: 1,
                });
                composite_passes.push(final_pass_name);

                // Register GradientBlur output for downstream chaining.
                let gradient_output_tex = output_tex.clone();
                pass_output_registry.register(PassOutputSpec {
                    node_id: layer_id.clone(),
                    texture_name: gradient_output_tex.clone(),
                    resolution: gb_src_resolution,
                    format: if is_sampled_output {
                        sampled_pass_format
                    } else {
                        target_format
                    },
                });

                let composition_consumers = composition_consumers_by_source
                    .get(layer_id)
                    .cloned()
                    .unwrap_or_default();
                for composition_id in composition_consumers {
                    let Some(comp_ctx) = composition_contexts.get(&composition_id) else {
                        continue;
                    };
                    if gradient_output_tex == comp_ctx.target_texture_name {
                        continue;
                    }

                    let comp_w = comp_ctx.target_size_px[0];
                    let comp_h = comp_ctx.target_size_px[1];
                    let compose_geo: ResourceName =
                        format!("sys.gb.{layer_id}.to.{composition_id}.compose.geo").into();
                    geometry_buffers
                        .push((compose_geo.clone(), make_fullscreen_geometry(src_w, src_h)));
                    let compose_pass_name: ResourceName =
                        format!("sys.gb.{layer_id}.to.{composition_id}.compose.pass").into();
                    let compose_params_name: ResourceName =
                        format!("params.sys.gb.{layer_id}.to.{composition_id}.compose").into();
                    let compose_params = Params {
                        target_size: [comp_w, comp_h],
                        geo_size: [src_w, src_h],
                        center: gb_output_center.unwrap_or([comp_w * 0.5, comp_h * 0.5]),
                        geo_translate: [0.0, 0.0],
                        geo_scale: [1.0, 1.0],
                        time: 0.0,
                        _pad0: 0.0,
                        color: [0.0, 0.0, 0.0, 0.0],
                    };

                    render_pass_specs.push(RenderPassSpec {
                        pass_id: compose_pass_name.as_str().to_string(),
                        name: compose_pass_name.clone(),
                        geometry_buffer: compose_geo,
                        instance_buffer: None,
                        normals_buffer: None,
                        target_texture: comp_ctx.target_texture_name.clone(),
                        resolve_target: None,
                        params_buffer: compose_params_name,
                        baked_data_parse_buffer: None,
                        params: compose_params,
                        graph_binding: None,
                        graph_values: None,
                        shader_wgsl: build_fullscreen_textured_bundle(
                            "return textureSample(src_tex, src_samp, in.uv);".to_string(),
                        )
                        .module,
                        texture_bindings: vec![PassTextureBinding {
                            texture: gradient_output_tex.clone(),
                            image_node_id: None,
                        }],
                        sampler_kinds: vec![SamplerKind::LinearClamp],
                        blend_state: pass_blend_state,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                        sample_count: 1,
                    });
                    composite_passes.push(compose_pass_name);
                }
            }
            "Downsample" => {
                // Downsample takes its source from `source` (pass), and downsamples into `targetSize`.
                // If sampled downstream (PassTexture), render into an intermediate texture;
                // otherwise render to the Composite target.

                let pass_name: ResourceName = format!("sys.downsample.{layer_id}.pass").into();
                let pass_blend_state =
                    crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
                        .with_context(|| {
                            format!(
                                "invalid blend params for {}",
                                crate::dsl::node_display_label_with_id(layer_node)
                            )
                        })?;

                // Resolve inputs.
                let src_conn = incoming_connection(&prepared.scene, layer_id, "source")
                    .ok_or_else(|| anyhow!("Downsample.source missing for {layer_id}"))?;
                let src_pass_id = src_conn.from.node_id.clone();
                let src_tex = pass_output_registry
                    .get_texture(&src_pass_id)
                    .cloned()
                    .ok_or_else(|| anyhow!(
                        "Downsample.source references upstream pass {src_pass_id}, but its output texture is not registered yet"
                    ))?;

                let kernel_conn = incoming_connection(&prepared.scene, layer_id, "kernel")
                    .ok_or_else(|| anyhow!("Downsample.kernel missing for {layer_id}"))?;
                let kernel_node = find_node(nodes_by_id, &kernel_conn.from.node_id)?;
                let kernel_src = kernel_node
                    .params
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let kernel = crate::renderer::render_plan::parse_kernel_source_js_like(kernel_src)?;

                fn parse_json_number_f32(v: &serde_json::Value) -> Option<f32> {
                    v.as_f64()
                        .map(|x| x as f32)
                        .or_else(|| v.as_i64().map(|x| x as f32))
                        .or_else(|| v.as_u64().map(|x| x as f32))
                }

                // Resolve targetSize:
                // - Prefer incoming connection (material graph)
                // - Otherwise fall back to inline params (Downsample.params.targetSize)
                let target_size_expr = if let Some(target_size_conn) =
                    incoming_connection(&prepared.scene, layer_id, "targetSize")
                {
                    let target_size_expr = {
                        let mut ctx = MaterialCompileContext::default();
                        let mut cache: HashMap<(String, String), TypedExpr> = HashMap::new();
                        crate::renderer::node_compiler::compile_material_expr(
                            &prepared.scene,
                            nodes_by_id,
                            &target_size_conn.from.node_id,
                            Some(&target_size_conn.from.port_id),
                            &mut ctx,
                            &mut cache,
                        )?
                    };
                    coerce_to_type(target_size_expr, ValueType::Vec2)?
                } else if let Some(v) = layer_node.params.get("targetSize") {
                    let (x, y) = if let Some(arr) = v.as_array() {
                        (
                            arr.get(0).and_then(parse_json_number_f32).unwrap_or(0.0),
                            arr.get(1).and_then(parse_json_number_f32).unwrap_or(0.0),
                        )
                    } else if let Some(obj) = v.as_object() {
                        (
                            obj.get("x").and_then(parse_json_number_f32).unwrap_or(0.0),
                            obj.get("y").and_then(parse_json_number_f32).unwrap_or(0.0),
                        )
                    } else {
                        bail!(
                            "Downsample.targetSize must be an object {{x,y}} or array [x,y] in params for {layer_id}"
                        );
                    };
                    TypedExpr::new(format!("vec2f({x}, {y})"), ValueType::Vec2)
                } else {
                    bail!("missing input '{layer_id}.targetSize' (no connection and no param)");
                };

                // Require CPU-known size for texture allocation.
                // (Vector2Input is used by tests; other graphs are not supported yet.)
                let (out_w, out_h) = {
                    let s = target_size_expr.expr.replace([' ', '\n', '\t', '\r'], "");
                    // Vector2Input compiles to (graph_inputs.<field>).xy; if we see that shape,
                    // try to fold the actual values from the node params.
                    if let Some(inner) = s
                        .strip_prefix("(graph_inputs.")
                        .and_then(|x| x.strip_suffix(").xy"))
                    {
                        // Find the Vector2Input node that owns this field.
                        if let Some((_node_id, node)) = nodes_by_id.iter().find(|(_, n)| {
                            n.node_type == "Vector2Input" && graph_field_name(&n.id) == inner
                        }) {
                            let w = cpu_num_u32_min_1(&prepared.scene, nodes_by_id, node, "x", 1)?;
                            let h = cpu_num_u32_min_1(&prepared.scene, nodes_by_id, node, "y", 1)?;
                            (w, h)
                        } else {
                            bail!(
                                "Downsample.targetSize must be a CPU-constant vec2f(w,h) for now, got {}",
                                target_size_expr.expr
                            );
                        }
                    } else if let Some(inner) =
                        s.strip_prefix("vec2f(").and_then(|x| x.strip_suffix(')'))
                    {
                        let parts: Vec<&str> = inner.split(',').collect();
                        if parts.len() == 2 {
                            let w = parts[0].parse::<f32>().unwrap_or(0.0).max(1.0).floor() as u32;
                            let h = parts[1].parse::<f32>().unwrap_or(0.0).max(1.0).floor() as u32;
                            (w, h)
                        } else {
                            bail!(
                                "Downsample.targetSize must be vec2f(w,h), got {}",
                                target_size_expr.expr
                            );
                        }
                    } else {
                        bail!(
                            "Downsample.targetSize must be a CPU-constant vec2f(w,h) for now, got {}",
                            target_size_expr.expr
                        );
                    }
                };

                let is_sampled_output = sampled_pass_ids.contains(layer_id);

                // Determine if we need to scale to Composite target size.
                let needs_upsample = !is_sampled_output && (out_w != tgt_w_u || out_h != tgt_h_u);

                // Non-sampled Downsample always reaches the scene output target
                // (either directly or through the synthesized upsample pass).
                let writes_scene_output_target = !is_sampled_output;

                // Allocate intermediate texture only when:
                // 1. Output is sampled by downstream passes, OR
                // 2. Output needs upsampling (different size from Composite target)
                // Otherwise render directly to the Composite target texture.
                let needs_intermediate = is_sampled_output || needs_upsample;

                let downsample_out_tex: ResourceName = if needs_intermediate {
                    let tex: ResourceName = format!("sys.downsample.{layer_id}.out").into();
                    textures.push(TextureDecl {
                        name: tex.clone(),
                        size: [out_w, out_h],
                        format: if is_sampled_output {
                            sampled_pass_format
                        } else {
                            target_format
                        },
                        sample_count: 1,
                    });
                    tex
                } else {
                    target_texture_name.clone()
                };

                // Fullscreen geometry for Downsample output size.
                let geo: ResourceName = format!("sys.downsample.{layer_id}.geo").into();
                geometry_buffers.push((
                    geo.clone(),
                    make_fullscreen_geometry(out_w as f32, out_h as f32),
                ));

                // Params for Downsample pass.
                let params_name: ResourceName = format!("params.sys.downsample.{layer_id}").into();
                let params_val = Params {
                    target_size: [out_w as f32, out_h as f32],
                    geo_size: [out_w as f32, out_h as f32],
                    center: [out_w as f32 * 0.5, out_h as f32 * 0.5],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                };

                // Sampling mode -> sampler kind.
                let sampling = parse_str(&layer_node.params, "sampling").unwrap_or("Mirror");
                let sampler_kind = match sampling {
                    "Mirror" => SamplerKind::LinearMirror,
                    "Repeat" => SamplerKind::LinearRepeat,
                    "Clamp" => SamplerKind::LinearClamp,
                    // ClampToBorder is not available in the current sampler set; treat as Clamp.
                    "ClampToBorder" => SamplerKind::LinearClamp,
                    other => bail!("Downsample.sampling unsupported: {other}"),
                };

                let bundle = build_downsample_pass_wgsl_bundle(&kernel)?;

                render_pass_specs.push(RenderPassSpec {
                    pass_id: pass_name.as_str().to_string(),
                    name: pass_name.clone(),
                    geometry_buffer: geo.clone(),
                    instance_buffer: None,
                    normals_buffer: None,
                    target_texture: downsample_out_tex.clone(),
                    resolve_target: None,
                    params_buffer: params_name,
                    baked_data_parse_buffer: None,
                    params: params_val,
                    graph_binding: None,
                    graph_values: None,
                    shader_wgsl: bundle.module,
                    texture_bindings: vec![PassTextureBinding {
                        texture: src_tex.clone(),
                        image_node_id: None,
                    }],
                    sampler_kinds: vec![sampler_kind],
                    blend_state: if downsample_out_tex == target_texture_name {
                        pass_blend_state
                    } else {
                        BlendState::REPLACE
                    },
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                    sample_count: 1,
                });
                composite_passes.push(pass_name);

                // If Downsample is the final layer and targetSize != Composite target,
                // add an upsample bilinear pass to scale to Composite target size.
                if needs_upsample {
                    let upsample_pass_name: ResourceName =
                        format!("sys.downsample.{layer_id}.upsample.pass").into();
                    let upsample_geo: ResourceName =
                        format!("sys.downsample.{layer_id}.upsample.geo").into();
                    geometry_buffers
                        .push((upsample_geo.clone(), make_fullscreen_geometry(tgt_w, tgt_h)));

                    let upsample_params_name: ResourceName =
                        format!("params.sys.downsample.{layer_id}.upsample").into();
                    let upsample_params_val = Params {
                        target_size: [tgt_w, tgt_h],
                        geo_size: [tgt_w, tgt_h],
                        center: [tgt_w * 0.5, tgt_h * 0.5],
                        geo_translate: [0.0, 0.0],
                        geo_scale: [1.0, 1.0],
                        time: 0.0,
                        _pad0: 0.0,
                        color: [0.0, 0.0, 0.0, 0.0],
                    };

                    let upsample_bundle = build_upsample_bilinear_bundle();

                    render_pass_specs.push(RenderPassSpec {
                        pass_id: upsample_pass_name.as_str().to_string(),
                        name: upsample_pass_name.clone(),
                        geometry_buffer: upsample_geo,
                        instance_buffer: None,
                        normals_buffer: None,
                        target_texture: target_texture_name.clone(),
                        resolve_target: None,
                        params_buffer: upsample_params_name,
                        baked_data_parse_buffer: None,
                        params: upsample_params_val,
                        graph_binding: None,
                        graph_values: None,
                        shader_wgsl: upsample_bundle.module,
                        texture_bindings: vec![PassTextureBinding {
                            texture: downsample_out_tex.clone(),
                            image_node_id: None,
                        }],
                        sampler_kinds: vec![SamplerKind::LinearClamp],
                        blend_state: pass_blend_state,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                        sample_count: 1,
                    });
                    composite_passes.push(upsample_pass_name);
                }

                // Register Downsample output for chaining.
                let downsample_output_tex = downsample_out_tex.clone();
                if is_sampled_output {
                    pass_output_registry.register(PassOutputSpec {
                        node_id: layer_id.clone(),
                        texture_name: downsample_output_tex.clone(),
                        resolution: [out_w, out_h],
                        format: sampled_pass_format,
                    });
                }

                let composition_consumers = composition_consumers_by_source
                    .get(layer_id)
                    .cloned()
                    .unwrap_or_default();
                if !composition_consumers.is_empty() {
                    let compose_blend_state = pass_blend_state;
                    for composition_id in composition_consumers {
                        let Some(comp_ctx) = composition_contexts.get(&composition_id) else {
                            continue;
                        };
                        if downsample_output_tex == comp_ctx.target_texture_name {
                            continue;
                        }
                        // Skip duplicate compose-to-target when this Downsample branch already
                        // writes to the scene output target directly or via upsample.
                        if writes_scene_output_target
                            && comp_ctx.target_texture_name == target_texture_name
                        {
                            continue;
                        }
                        let comp_w = comp_ctx.target_size_px[0];
                        let comp_h = comp_ctx.target_size_px[1];
                        let compose_geo: ResourceName =
                            format!("sys.downsample.{layer_id}.to.{composition_id}.compose.geo")
                                .into();
                        geometry_buffers.push((
                            compose_geo.clone(),
                            make_fullscreen_geometry(comp_w, comp_h),
                        ));
                        let compose_pass_name: ResourceName =
                            format!("sys.downsample.{layer_id}.to.{composition_id}.compose.pass")
                                .into();
                        let compose_params_name: ResourceName =
                            format!("params.sys.downsample.{layer_id}.to.{composition_id}.compose")
                                .into();
                        let compose_params = Params {
                            target_size: [comp_w, comp_h],
                            geo_size: [comp_w, comp_h],
                            center: [comp_w * 0.5, comp_h * 0.5],
                            geo_translate: [0.0, 0.0],
                            geo_scale: [1.0, 1.0],
                            time: 0.0,
                            _pad0: 0.0,
                            color: [0.0, 0.0, 0.0, 0.0],
                        };

                        render_pass_specs.push(RenderPassSpec {
                            pass_id: compose_pass_name.as_str().to_string(),
                            name: compose_pass_name.clone(),
                            geometry_buffer: compose_geo,
                            instance_buffer: None,
                            normals_buffer: None,
                            target_texture: comp_ctx.target_texture_name.clone(),
                            resolve_target: None,
                            params_buffer: compose_params_name,
                            baked_data_parse_buffer: None,
                            params: compose_params,
                            graph_binding: None,
                            graph_values: None,
                            shader_wgsl: build_fullscreen_textured_bundle(
                                "return textureSample(src_tex, src_samp, in.uv);".to_string(),
                            )
                            .module,
                            texture_bindings: vec![PassTextureBinding {
                                texture: downsample_output_tex.clone(),
                                image_node_id: None,
                            }],
                            sampler_kinds: vec![SamplerKind::LinearClamp],
                            blend_state: compose_blend_state,
                            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                            sample_count: 1,
                        });
                        composite_passes.push(compose_pass_name);
                    }
                }
            }
            "Upsample" => {
                let pass_name: ResourceName = format!("sys.upsample.{layer_id}.pass").into();
                let pass_blend_state =
                    crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
                        .with_context(|| {
                            format!(
                                "invalid blend params for {}",
                                crate::dsl::node_display_label_with_id(layer_node)
                            )
                        })?;

                let src_conn = incoming_connection(&prepared.scene, layer_id, "source")
                    .ok_or_else(|| anyhow!("Upsample.source missing for {layer_id}"))?;
                let src_pass_id = src_conn.from.node_id.clone();
                let src_tex = pass_output_registry
                    .get_texture(&src_pass_id)
                    .cloned()
                    .ok_or_else(|| anyhow!(
                        "Upsample.source references upstream pass {src_pass_id}, but its output texture is not registered yet"
                    ))?;

                fn parse_json_number_f32(v: &serde_json::Value) -> Option<f32> {
                    v.as_f64()
                        .map(|x| x as f32)
                        .or_else(|| v.as_i64().map(|x| x as f32))
                        .or_else(|| v.as_u64().map(|x| x as f32))
                }

                fn to_positive_target_size(layer_id: &str, x: f32, y: f32) -> Result<(u32, u32)> {
                    if !x.is_finite() || !y.is_finite() {
                        bail!(
                            "Upsample.targetSize must have finite components for {layer_id}, got ({x}, {y})"
                        );
                    }
                    if x <= 0.0 || y <= 0.0 {
                        bail!(
                            "Upsample.targetSize must have positive components for {layer_id}, got ({x}, {y})"
                        );
                    }
                    Ok((x.round().max(1.0) as u32, y.round().max(1.0) as u32))
                }

                let target_size_expr = if let Some(target_size_conn) =
                    incoming_connection(&prepared.scene, layer_id, "targetSize")
                {
                    let target_size_expr = {
                        let mut ctx = MaterialCompileContext::default();
                        let mut cache: HashMap<(String, String), TypedExpr> = HashMap::new();
                        crate::renderer::node_compiler::compile_material_expr(
                            &prepared.scene,
                            nodes_by_id,
                            &target_size_conn.from.node_id,
                            Some(&target_size_conn.from.port_id),
                            &mut ctx,
                            &mut cache,
                        )?
                    };
                    coerce_to_type(target_size_expr, ValueType::Vec2)?
                } else if let Some(v) = layer_node.params.get("targetSize") {
                    let (x, y) = if let Some(arr) = v.as_array() {
                        let x = arr
                            .first()
                            .and_then(parse_json_number_f32)
                            .ok_or_else(|| {
                                anyhow!(
                                    "Upsample.targetSize must be an object {{x,y}} or array [x,y] in params for {layer_id}"
                                )
                            })?;
                        let y = arr
                            .get(1)
                            .and_then(parse_json_number_f32)
                            .ok_or_else(|| {
                                anyhow!(
                                    "Upsample.targetSize must be an object {{x,y}} or array [x,y] in params for {layer_id}"
                                )
                            })?;
                        (x, y)
                    } else if let Some(obj) = v.as_object() {
                        let x = obj
                            .get("x")
                            .and_then(parse_json_number_f32)
                            .ok_or_else(|| {
                                anyhow!(
                                    "Upsample.targetSize must be an object {{x,y}} or array [x,y] in params for {layer_id}"
                                )
                            })?;
                        let y = obj
                            .get("y")
                            .and_then(parse_json_number_f32)
                            .ok_or_else(|| {
                                anyhow!(
                                    "Upsample.targetSize must be an object {{x,y}} or array [x,y] in params for {layer_id}"
                                )
                            })?;
                        (x, y)
                    } else {
                        bail!(
                            "Upsample.targetSize must be an object {{x,y}} or array [x,y] in params for {layer_id}"
                        );
                    };
                    TypedExpr::new(format!("vec2f({x}, {y})"), ValueType::Vec2)
                } else {
                    bail!("missing input '{layer_id}.targetSize' (no connection and no param)");
                };

                let (out_w, out_h) = {
                    let s = target_size_expr.expr.replace([' ', '\n', '\t', '\r'], "");
                    if let Some(inner) = s
                        .strip_prefix("(graph_inputs.")
                        .and_then(|x| x.strip_suffix(").xy"))
                    {
                        if let Some((_node_id, node)) = nodes_by_id.iter().find(|(_, n)| {
                            n.node_type == "Vector2Input" && graph_field_name(&n.id) == inner
                        }) {
                            let w = cpu_num_f32(&prepared.scene, nodes_by_id, node, "x", 0.0)?;
                            let h = cpu_num_f32(&prepared.scene, nodes_by_id, node, "y", 0.0)?;
                            to_positive_target_size(layer_id, w, h)?
                        } else {
                            bail!(
                                "Upsample.targetSize must be a CPU-constant vec2f(w,h) for now, got {}",
                                target_size_expr.expr
                            );
                        }
                    } else if let Some(inner) =
                        s.strip_prefix("vec2f(").and_then(|x| x.strip_suffix(')'))
                    {
                        let parts: Vec<&str> = inner.split(',').collect();
                        if parts.len() == 2 {
                            let w = parts[0].parse::<f32>().map_err(|_| {
                                anyhow!(
                                    "Upsample.targetSize must be vec2f(w,h), got {}",
                                    target_size_expr.expr
                                )
                            })?;
                            let h = parts[1].parse::<f32>().map_err(|_| {
                                anyhow!(
                                    "Upsample.targetSize must be vec2f(w,h), got {}",
                                    target_size_expr.expr
                                )
                            })?;
                            to_positive_target_size(layer_id, w, h)?
                        } else {
                            bail!(
                                "Upsample.targetSize must be vec2f(w,h), got {}",
                                target_size_expr.expr
                            );
                        }
                    } else {
                        bail!(
                            "Upsample.targetSize must be a CPU-constant vec2f(w,h) for now, got {}",
                            target_size_expr.expr
                        );
                    }
                };

                let is_sampled_output = sampled_pass_ids.contains(layer_id);
                let needs_intermediate =
                    is_sampled_output || (out_w != tgt_w_u || out_h != tgt_h_u);
                // Non-sampled Upsample always reaches the scene output target
                // (either directly or through the synthesized fit pass).
                let writes_scene_output_target = !is_sampled_output;

                let upsample_out_tex: ResourceName = if needs_intermediate {
                    let tex: ResourceName = format!("sys.upsample.{layer_id}.out").into();
                    textures.push(TextureDecl {
                        name: tex.clone(),
                        size: [out_w, out_h],
                        format: if is_sampled_output {
                            sampled_pass_format
                        } else {
                            target_format
                        },
                        sample_count: 1,
                    });
                    tex
                } else {
                    target_texture_name.clone()
                };

                let geo: ResourceName = format!("sys.upsample.{layer_id}.geo").into();
                geometry_buffers.push((
                    geo.clone(),
                    make_fullscreen_geometry(out_w as f32, out_h as f32),
                ));

                let params_name: ResourceName = format!("params.sys.upsample.{layer_id}").into();
                let params_val = Params {
                    target_size: [out_w as f32, out_h as f32],
                    geo_size: [out_w as f32, out_h as f32],
                    center: [out_w as f32 * 0.5, out_h as f32 * 0.5],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                };

                let address_mode = parse_str(&layer_node.params, "address_mode")
                    .unwrap_or("clamp-to-edge")
                    .trim()
                    .to_ascii_lowercase();
                let filter = parse_str(&layer_node.params, "filter")
                    .unwrap_or("linear")
                    .trim()
                    .to_ascii_lowercase();

                let address = match address_mode.as_str() {
                    "clamp-to-edge" => "clamp",
                    "repeat" => "repeat",
                    "mirror-repeat" => "mirror",
                    other => bail!("Upsample: invalid address_mode: {other}"),
                };
                let nearest = match filter.as_str() {
                    "nearest" => true,
                    "linear" => false,
                    other => bail!("Upsample: invalid filter: {other}"),
                };
                let sampler_kind = match (nearest, address) {
                    (true, "mirror") => SamplerKind::NearestMirror,
                    (true, "repeat") => SamplerKind::NearestRepeat,
                    (true, _) => SamplerKind::NearestClamp,
                    (false, "mirror") => SamplerKind::LinearMirror,
                    (false, "repeat") => SamplerKind::LinearRepeat,
                    (false, _) => SamplerKind::LinearClamp,
                };

                let bundle = build_upsample_bilinear_bundle();

                render_pass_specs.push(RenderPassSpec {
                    pass_id: pass_name.as_str().to_string(),
                    name: pass_name.clone(),
                    geometry_buffer: geo,
                    instance_buffer: None,
                    normals_buffer: None,
                    target_texture: upsample_out_tex.clone(),
                    resolve_target: None,
                    params_buffer: params_name,
                    baked_data_parse_buffer: None,
                    params: params_val,
                    graph_binding: None,
                    graph_values: None,
                    shader_wgsl: bundle.module,
                    texture_bindings: vec![PassTextureBinding {
                        texture: src_tex.clone(),
                        image_node_id: None,
                    }],
                    sampler_kinds: vec![sampler_kind],
                    blend_state: if upsample_out_tex == target_texture_name {
                        pass_blend_state
                    } else {
                        BlendState::REPLACE
                    },
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                    sample_count: 1,
                });
                composite_passes.push(pass_name);

                if !is_sampled_output && upsample_out_tex != target_texture_name {
                    let fit_pass_name: ResourceName =
                        format!("sys.upsample.{layer_id}.fit.pass").into();
                    let fit_geo: ResourceName = format!("sys.upsample.{layer_id}.fit.geo").into();
                    geometry_buffers
                        .push((fit_geo.clone(), make_fullscreen_geometry(tgt_w, tgt_h)));

                    let fit_params_name: ResourceName =
                        format!("params.sys.upsample.{layer_id}.fit").into();
                    let fit_params = Params {
                        target_size: [tgt_w, tgt_h],
                        geo_size: [tgt_w, tgt_h],
                        center: [tgt_w * 0.5, tgt_h * 0.5],
                        geo_translate: [0.0, 0.0],
                        geo_scale: [1.0, 1.0],
                        time: 0.0,
                        _pad0: 0.0,
                        color: [0.0, 0.0, 0.0, 0.0],
                    };
                    let fit_bundle = build_upsample_bilinear_bundle();

                    render_pass_specs.push(RenderPassSpec {
                        pass_id: fit_pass_name.as_str().to_string(),
                        name: fit_pass_name.clone(),
                        geometry_buffer: fit_geo,
                        instance_buffer: None,
                        normals_buffer: None,
                        target_texture: target_texture_name.clone(),
                        resolve_target: None,
                        params_buffer: fit_params_name,
                        baked_data_parse_buffer: None,
                        params: fit_params,
                        graph_binding: None,
                        graph_values: None,
                        shader_wgsl: fit_bundle.module,
                        texture_bindings: vec![PassTextureBinding {
                            texture: upsample_out_tex.clone(),
                            image_node_id: None,
                        }],
                        sampler_kinds: vec![SamplerKind::LinearClamp],
                        blend_state: pass_blend_state,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                        sample_count: 1,
                    });
                    composite_passes.push(fit_pass_name);
                }

                let upsample_output_tex = upsample_out_tex.clone();
                if is_sampled_output {
                    pass_output_registry.register(PassOutputSpec {
                        node_id: layer_id.clone(),
                        texture_name: upsample_output_tex.clone(),
                        resolution: [out_w, out_h],
                        format: sampled_pass_format,
                    });
                }

                let composition_consumers = composition_consumers_by_source
                    .get(layer_id)
                    .cloned()
                    .unwrap_or_default();
                if !composition_consumers.is_empty() {
                    let compose_blend_state = pass_blend_state;
                    for composition_id in composition_consumers {
                        let Some(comp_ctx) = composition_contexts.get(&composition_id) else {
                            continue;
                        };
                        if upsample_output_tex == comp_ctx.target_texture_name {
                            continue;
                        }
                        // Skip duplicate compose-to-target when this Upsample branch already
                        // writes to the scene output target directly or via fit pass.
                        if writes_scene_output_target
                            && comp_ctx.target_texture_name == target_texture_name
                        {
                            continue;
                        }
                        let comp_w = comp_ctx.target_size_px[0];
                        let comp_h = comp_ctx.target_size_px[1];
                        let compose_geo: ResourceName =
                            format!("sys.upsample.{layer_id}.to.{composition_id}.compose.geo")
                                .into();
                        geometry_buffers.push((
                            compose_geo.clone(),
                            make_fullscreen_geometry(comp_w, comp_h),
                        ));
                        let compose_pass_name: ResourceName =
                            format!("sys.upsample.{layer_id}.to.{composition_id}.compose.pass")
                                .into();
                        let compose_params_name: ResourceName =
                            format!("params.sys.upsample.{layer_id}.to.{composition_id}.compose")
                                .into();
                        let compose_params = Params {
                            target_size: [comp_w, comp_h],
                            geo_size: [comp_w, comp_h],
                            center: [comp_w * 0.5, comp_h * 0.5],
                            geo_translate: [0.0, 0.0],
                            geo_scale: [1.0, 1.0],
                            time: 0.0,
                            _pad0: 0.0,
                            color: [0.0, 0.0, 0.0, 0.0],
                        };

                        render_pass_specs.push(RenderPassSpec {
                            pass_id: compose_pass_name.as_str().to_string(),
                            name: compose_pass_name.clone(),
                            geometry_buffer: compose_geo,
                            instance_buffer: None,
                            normals_buffer: None,
                            target_texture: comp_ctx.target_texture_name.clone(),
                            resolve_target: None,
                            params_buffer: compose_params_name,
                            baked_data_parse_buffer: None,
                            params: compose_params,
                            graph_binding: None,
                            graph_values: None,
                            shader_wgsl: build_fullscreen_textured_bundle(
                                "return textureSample(src_tex, src_samp, in.uv);".to_string(),
                            )
                            .module,
                            texture_bindings: vec![PassTextureBinding {
                                texture: upsample_output_tex.clone(),
                                image_node_id: None,
                            }],
                            sampler_kinds: vec![SamplerKind::LinearClamp],
                            blend_state: compose_blend_state,
                            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                            sample_count: 1,
                        });
                        composite_passes.push(compose_pass_name);
                    }
                }
            }
            "Composite" => {
                let Some(comp_ctx) = composition_contexts.get(layer_id) else {
                    bail!("missing resolved Composition context for '{layer_id}'");
                };
                let comp_target_node = find_node(nodes_by_id, &comp_ctx.target_texture_node_id)?;
                let comp_target_format = parse_texture_format(&comp_target_node.params)?;
                let pass_blend_state =
                    crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
                        .with_context(|| {
                            format!(
                                "invalid blend params for {}",
                                crate::dsl::node_display_label_with_id(layer_node)
                            )
                        })?;
                pass_output_registry.register(PassOutputSpec {
                    node_id: layer_id.clone(),
                    texture_name: comp_ctx.target_texture_name.clone(),
                    resolution: [
                        comp_ctx.target_size_px[0].max(1.0).round() as u32,
                        comp_ctx.target_size_px[1].max(1.0).round() as u32,
                    ],
                    format: comp_target_format,
                });

                // Implicit Composition -> Composition fullscreen blit.
                let composition_consumers = composition_consumers_by_source
                    .get(layer_id)
                    .cloned()
                    .unwrap_or_default();
                if !composition_consumers.is_empty() {
                    let compose_blend_state = pass_blend_state;
                    for downstream_comp_id in composition_consumers {
                        let Some(dst_ctx) = composition_contexts.get(&downstream_comp_id) else {
                            continue;
                        };
                        if dst_ctx.composition_node_id == comp_ctx.composition_node_id {
                            continue;
                        }
                        if dst_ctx.target_texture_name == comp_ctx.target_texture_name {
                            continue;
                        }
                        let dst_w = dst_ctx.target_size_px[0];
                        let dst_h = dst_ctx.target_size_px[1];
                        let geo: ResourceName =
                            format!("sys.comp.{layer_id}.to.{downstream_comp_id}.compose.geo")
                                .into();
                        geometry_buffers
                            .push((geo.clone(), make_fullscreen_geometry(dst_w, dst_h)));
                        let pass_name: ResourceName =
                            format!("sys.comp.{layer_id}.to.{downstream_comp_id}.compose.pass")
                                .into();
                        let params_name: ResourceName =
                            format!("params.sys.comp.{layer_id}.to.{downstream_comp_id}.compose")
                                .into();
                        let params = Params {
                            target_size: [dst_w, dst_h],
                            geo_size: [dst_w, dst_h],
                            center: [dst_w * 0.5, dst_h * 0.5],
                            geo_translate: [0.0, 0.0],
                            geo_scale: [1.0, 1.0],
                            time: 0.0,
                            _pad0: 0.0,
                            color: [0.0, 0.0, 0.0, 0.0],
                        };
                        render_pass_specs.push(RenderPassSpec {
                            pass_id: pass_name.as_str().to_string(),
                            name: pass_name.clone(),
                            geometry_buffer: geo,
                            instance_buffer: None,
                            normals_buffer: None,
                            target_texture: dst_ctx.target_texture_name.clone(),
                            resolve_target: None,
                            params_buffer: params_name,
                            baked_data_parse_buffer: None,
                            params,
                            graph_binding: None,
                            graph_values: None,
                            shader_wgsl: build_fullscreen_textured_bundle(
                                "return textureSample(src_tex, src_samp, in.uv);".to_string(),
                            )
                            .module,
                            texture_bindings: vec![PassTextureBinding {
                                texture: comp_ctx.target_texture_name.clone(),
                                image_node_id: None,
                            }],
                            sampler_kinds: vec![SamplerKind::LinearClamp],
                            blend_state: compose_blend_state,
                            color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                            sample_count: 1,
                        });
                        composite_passes.push(pass_name);
                    }
                }
            }
            other => {
                // To add support for new pass types:
                // 1. Add the type to is_pass_node() function
                // 2. Add a match arm here with the rendering logic
                // 3. Register the output in pass_output_registry for chain support
                bail!(
                    "Composite layer must be a pass node (RenderPass/GuassianBlurPass/Downsample/Upsample/GradientBlur/Composite), got {other} for {layer_id}. \
                     To enable chain support for new pass types, update is_pass_node() and add handling here."
                )
            }
        }
    }

    // Final display encode pass (sRGB output -> linear texture with sRGB bytes).
    if enable_display_encode {
        if let Some(display_tex) = display_texture_name.clone() {
            let pass_name: ResourceName = format!(
                "{}{}.pass",
                target_texture_name.as_str(),
                UI_PRESENT_SDR_SRGB_SUFFIX
            )
            .into();
            let geo: ResourceName = format!(
                "{}{}.geo",
                target_texture_name.as_str(),
                UI_PRESENT_SDR_SRGB_SUFFIX
            )
            .into();
            geometry_buffers.push((geo.clone(), make_fullscreen_geometry(tgt_w, tgt_h)));

            let params_name: ResourceName = format!(
                "params.{}{}",
                target_texture_name.as_str(),
                UI_PRESENT_SDR_SRGB_SUFFIX
            )
            .into();
            let params = Params {
                target_size: [tgt_w, tgt_h],
                geo_size: [tgt_w, tgt_h],
                center: [tgt_w * 0.5, tgt_h * 0.5],
                geo_translate: [0.0, 0.0],
                geo_scale: [1.0, 1.0],
                time: 0.0,
                _pad0: 0.0,
                color: [0.0, 0.0, 0.0, 0.0],
            };

            let shader_wgsl = build_srgb_display_encode_wgsl("src_tex", "src_samp");
            render_pass_specs.push(RenderPassSpec {
                pass_id: pass_name.as_str().to_string(),
                name: pass_name.clone(),
                geometry_buffer: geo,
                instance_buffer: None,
                normals_buffer: None,
                target_texture: display_tex.clone(),
                resolve_target: None,
                params_buffer: params_name,
                baked_data_parse_buffer: None,
                params,
                graph_binding: None,
                graph_values: None,
                shader_wgsl,
                texture_bindings: vec![PassTextureBinding {
                    texture: target_texture_name.clone(),
                    image_node_id: None,
                }],
                sampler_kinds: vec![SamplerKind::NearestClamp],
                blend_state: BlendState::REPLACE,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                sample_count: 1,
            });

            // Make sure it runs last.
            composite_passes.push(pass_name);
        }
    }

    // Clear each render texture only on its first write per frame.
    // If multiple RenderPass nodes target the same RenderTexture, subsequent passes should Load so
    // alpha blending can accumulate.
    {
        let mut seen_targets: HashSet<ResourceName> = HashSet::new();
        for spec in &mut render_pass_specs {
            if seen_targets.insert(spec.target_texture.clone()) {
                spec.color_load_op = wgpu::LoadOp::Clear(Color::TRANSPARENT);
            } else {
                spec.color_load_op = wgpu::LoadOp::Load;
            }
        }
    }

    let mut shader_space = ShaderSpace::new(device, queue);

    let pass_bindings: Vec<PassBindings> = render_pass_specs
        .iter()
        .map(|s| PassBindings {
            pass_id: s.pass_id.clone(),
            params_buffer: s.params_buffer.clone(),
            base_params: s.params,
            graph_binding: s.graph_binding.clone(),
            last_graph_hash: s.graph_values.as_ref().map(|v| hash_bytes(v.as_slice())),
        })
        .collect();
    let pipeline_signature =
        compute_pipeline_signature_for_pass_bindings(&prepared.scene, &pass_bindings);

    // ---------------- data-driven declarations ----------------
    // 1) Buffers
    let mut buffer_specs: Vec<BufferSpec> = Vec::new();

    for (name, bytes) in &geometry_buffers {
        buffer_specs.push(BufferSpec::Init {
            name: name.clone(),
            contents: bytes.clone(),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
    }

    for (name, bytes) in &instance_buffers {
        buffer_specs.push(BufferSpec::Init {
            name: name.clone(),
            contents: bytes.clone(),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
    }

    for pass in &pass_bindings {
        buffer_specs.push(BufferSpec::Sized {
            name: pass.params_buffer.clone(),
            size: core::mem::size_of::<Params>(),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        if let Some(graph_binding) = pass.graph_binding.as_ref() {
            let usage = match graph_binding.kind {
                GraphBindingKind::Uniform => {
                    wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST
                }
                GraphBindingKind::StorageRead => {
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST
                }
            };
            buffer_specs.push(BufferSpec::Sized {
                name: graph_binding.buffer_name.clone(),
                size: graph_binding.schema.size_bytes as usize,
                usage,
            });
        }
    }

    for spec in &render_pass_specs {
        let Some(name) = spec.baked_data_parse_buffer.clone() else {
            continue;
        };

        // BakedDataParse buffers are owned by a logical pass id (the DSL pass node id).
        // Keep the mapping explicit so renaming the buffer doesn't require parsing strings.
        let pass_id: Option<&String> = baked_data_parse_buffer_to_pass_id.get(&name);
        let contents = pass_id
            .and_then(|id| baked_data_parse_bytes_by_pass.get(id))
            .cloned()
            .unwrap_or_else(|| Arc::from(vec![0u8; 16]));

        buffer_specs.push(BufferSpec::Init {
            name,
            contents,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
    }

    shader_space.declare_buffers(buffer_specs);

    // 2) Textures
    let mut texture_specs: Vec<FiberTextureSpec> = textures
        .iter()
        .map(|t| FiberTextureSpec::Texture {
            name: t.name.clone(),
            resolution: t.size,
            format: t.format,
            usage: if t.sample_count > 1 {
                TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC
            } else {
                TextureUsages::RENDER_ATTACHMENT
                    | TextureUsages::TEXTURE_BINDING
                    | TextureUsages::COPY_SRC
            },
            sample_count: t.sample_count,
        })
        .collect();

    #[derive(Clone)]
    struct ImagePrepass {
        pass_name: ResourceName,
        geometry_buffer: ResourceName,
        params_buffer: ResourceName,
        params: Params,
        src_texture: ResourceName,
        dst_texture: ResourceName,
        shader_wgsl: String,
    }

    let mut image_prepasses: Vec<ImagePrepass> = Vec::new();
    let mut prepass_buffer_specs: Vec<BufferSpec> = Vec::new();
    let mut prepass_names: Vec<ResourceName> = Vec::new();

    // ImageTexture resources (sampled textures) referenced by any reachable RenderPass.
    fn load_image_from_path(
        rel_base: &PathBuf,
        path: Option<&str>,
        node_id: &str,
    ) -> Result<Arc<DynamicImage>> {
        let Some(p) = path.filter(|s| !s.trim().is_empty()) else {
            bail!("ImageTexture node '{node_id}' has no path specified");
        };

        let candidates: Vec<PathBuf> = {
            let pb = PathBuf::from(p);
            if pb.is_absolute() {
                vec![pb]
            } else {
                vec![
                    pb.clone(),
                    rel_base.join(&pb),
                    rel_base.join("assets").join(&pb),
                ]
            }
        };

        for cand in &candidates {
            if let Ok(img) = image::open(cand) {
                return Ok(Arc::new(img));
            }
        }

        bail!(
            "ImageTexture node '{node_id}': failed to load image from path '{}'. Tried: {:?}",
            p,
            candidates
        );
    }

    fn ensure_rgba8(image: Arc<DynamicImage>) -> Arc<DynamicImage> {
        // rust-wgpu-fiber's image texture path selects wgpu texture format based on image.color().
        // For RGB images it maps to RGBA formats (because wgpu has no RGB8), so we must ensure
        // the pixel buffer is actually RGBA to keep bytes_per_row consistent.
        if image.color() == image::ColorType::Rgba8 {
            return image;
        }
        Arc::new(DynamicImage::ImageRgba8(image.as_ref().to_rgba8()))
    }

    let rel_base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut seen_image_nodes: HashSet<String> = HashSet::new();
    for pass in &render_pass_specs {
        for binding in &pass.texture_bindings {
            let Some(node_id) = binding.image_node_id.as_ref() else {
                continue;
            };
            if !seen_image_nodes.insert(node_id.clone()) {
                continue;
            }
            let node = find_node(&nodes_by_id, node_id)?;
            if node.node_type != "ImageTexture" {
                bail!(
                    "expected ImageTexture node for {node_id}, got {}",
                    node.node_type
                );
            }

            // Load image: prefer assetId → asset_store, then legacy dataUrl, then legacy path.
            let asset_id = node
                .params
                .get("assetId")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty());

            let encoder_space = node
                .params
                .get("encoderSpace")
                .and_then(|v| v.as_str())
                .unwrap_or("srgb")
                .trim()
                .to_ascii_lowercase();
            let is_srgb = match encoder_space.as_str() {
                "srgb" => true,
                "linear" => false,
                other => bail!("unsupported ImageTexture.encoderSpace: {other}"),
            };

            let image = if let Some(aid) = asset_id {
                if let Some(store) = asset_store {
                    match store.load_image(aid)? {
                        Some(img) => ensure_rgba8(Arc::new(img)),
                        None => bail!(
                            "ImageTexture node '{node_id}': asset '{aid}' not found in asset store"
                        ),
                    }
                } else {
                    bail!(
                        "ImageTexture node '{node_id}': has assetId '{aid}' but no asset store provided"
                    )
                }
            } else {
                // Legacy fallback: dataUrl, then path.
                let data_url = node
                    .params
                    .get("dataUrl")
                    .and_then(|v| v.as_str())
                    .or_else(|| node.params.get("data_url").and_then(|v| v.as_str()));
                match data_url {
                    Some(s) if !s.trim().is_empty() => match load_image_from_data_url(s) {
                        Ok(img) => ensure_rgba8(Arc::new(img)),
                        Err(e) => bail!(
                            "ImageTexture node '{node_id}': failed to load image from dataUrl: {e}"
                        ),
                    },
                    _ => {
                        let path = node.params.get("path").and_then(|v| v.as_str());
                        ensure_rgba8(load_image_from_path(&rel_base, path, node_id)?)
                    }
                }
            };

            // GPU prepass for straight-alpha: upload source straight, render a 1:1
            // premultiply pass to a destination texture. This ensures all GPU bilinear
            // sampling operates on premultiplied data (avoiding dark-fringe artifacts).
            let alpha_mode = node
                .params
                .get("alphaMode")
                .and_then(|v| v.as_str())
                .unwrap_or("straight")
                .trim()
                .to_ascii_lowercase();
            let needs_premultiply = match alpha_mode.as_str() {
                "straight" => true,
                "premultiplied" => false,
                other => bail!("unsupported ImageTexture.alphaMode: {other}"),
            };

            let img_w = image.width();
            let img_h = image.height();

            let name = ids
                .get(node_id)
                .cloned()
                .ok_or_else(|| anyhow!("missing name for node: {node_id}"))?;

            if needs_premultiply {
                let src_name: ResourceName = format!("sys.image.{node_id}.src").into();

                // Upload source as straight-alpha.
                texture_specs.push(FiberTextureSpec::Image {
                    name: src_name.clone(),
                    image,
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                    srgb: is_srgb,
                });

                // Allocate destination texture (ALWAYS linear). This avoids an early
                // linear->sRGB encode at the premultiply stage which would later be
                // decoded again on sampling and can cause darkening.
                // When the source is sRGB-encoded, use Rgba16Float to preserve
                // precision in the linear domain and avoid banding in dark tones.
                let dst_format = if is_srgb {
                    TextureFormat::Rgba16Float
                } else {
                    TextureFormat::Rgba8Unorm
                };
                texture_specs.push(FiberTextureSpec::Texture {
                    name: name.clone(),
                    resolution: [img_w, img_h],
                    format: dst_format,
                    usage: TextureUsages::RENDER_ATTACHMENT
                        | TextureUsages::TEXTURE_BINDING
                        | TextureUsages::COPY_SRC,
                    sample_count: 1,
                });

                let w = img_w as f32;
                let h = img_h as f32;

                let geo: ResourceName = format!("sys.image.{node_id}.premultiply.geo").into();
                prepass_buffer_specs.push(BufferSpec::Init {
                    name: geo.clone(),
                    contents: make_fullscreen_geometry(w, h),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                });

                let params_name: ResourceName =
                    format!("params.sys.image.{node_id}.premultiply").into();
                prepass_buffer_specs.push(BufferSpec::Sized {
                    name: params_name.clone(),
                    size: core::mem::size_of::<Params>(),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });

                let params = Params {
                    target_size: [w, h],
                    geo_size: [w, h],
                    center: [w * 0.5, h * 0.5],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                };

                let pass_name: ResourceName =
                    format!("sys.image.{node_id}.premultiply.pass").into();
                let tex_var = MaterialCompileContext::tex_var_name(src_name.as_str());
                let samp_var = MaterialCompileContext::sampler_var_name(src_name.as_str());
                let shader_wgsl = build_image_premultiply_wgsl(&tex_var, &samp_var);

                prepass_names.push(pass_name.clone());
                image_prepasses.push(ImagePrepass {
                    pass_name,
                    geometry_buffer: geo,
                    params_buffer: params_name,
                    params,
                    src_texture: src_name,
                    dst_texture: name,
                    shader_wgsl,
                });
            } else {
                texture_specs.push(FiberTextureSpec::Image {
                    name,
                    image,
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                    srgb: is_srgb,
                });
            }
        }
    }

    if !prepass_buffer_specs.is_empty() {
        shader_space.declare_buffers(prepass_buffer_specs);
    }

    shader_space.declare_textures(texture_specs);

    // 3) Samplers
    let nearest_sampler: ResourceName = "sampler_nearest".into();
    let nearest_mirror_sampler: ResourceName = "sampler_nearest_mirror".into();
    let nearest_repeat_sampler: ResourceName = "sampler_nearest_repeat".into();
    let linear_mirror_sampler: ResourceName = "sampler_linear_mirror".into();
    let linear_repeat_sampler: ResourceName = "sampler_linear_repeat".into();
    let linear_clamp_sampler: ResourceName = "sampler_linear_clamp".into();
    shader_space.declare_samplers(vec![
        SamplerSpec {
            name: nearest_sampler.clone(),
            desc: wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                mipmap_filter: wgpu::FilterMode::Nearest,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            },
        },
        SamplerSpec {
            name: nearest_mirror_sampler.clone(),
            desc: wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                mipmap_filter: wgpu::FilterMode::Nearest,
                address_mode_u: wgpu::AddressMode::MirrorRepeat,
                address_mode_v: wgpu::AddressMode::MirrorRepeat,
                address_mode_w: wgpu::AddressMode::MirrorRepeat,
                ..Default::default()
            },
        },
        SamplerSpec {
            name: nearest_repeat_sampler.clone(),
            desc: wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                mipmap_filter: wgpu::FilterMode::Nearest,
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                address_mode_w: wgpu::AddressMode::Repeat,
                ..Default::default()
            },
        },
        SamplerSpec {
            name: linear_mirror_sampler.clone(),
            desc: wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Nearest,
                address_mode_u: wgpu::AddressMode::MirrorRepeat,
                address_mode_v: wgpu::AddressMode::MirrorRepeat,
                address_mode_w: wgpu::AddressMode::MirrorRepeat,
                ..Default::default()
            },
        },
        SamplerSpec {
            name: linear_repeat_sampler.clone(),
            desc: wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Nearest,
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                address_mode_w: wgpu::AddressMode::Repeat,
                ..Default::default()
            },
        },
        SamplerSpec {
            name: linear_clamp_sampler.clone(),
            desc: wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Nearest,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            },
        },
    ]);

    for spec in &render_pass_specs {
        let geometry_buffer = spec.geometry_buffer.clone();
        let target_texture = spec.target_texture.clone();
        let resolve_target = spec.resolve_target.clone();
        let sample_count = spec.sample_count;
        let params_buffer = spec.params_buffer.clone();
        let shader_wgsl = spec.shader_wgsl.clone();
        let blend_state = spec.blend_state;
        let color_load_op = spec.color_load_op;
        let graph_binding = spec.graph_binding.clone();

        let texture_names: Vec<ResourceName> = spec
            .texture_bindings
            .iter()
            .map(|b| b.texture.clone())
            .collect();

        let sampler_names: Vec<ResourceName> = spec
            .sampler_kinds
            .iter()
            .map(|k| match k {
                SamplerKind::NearestClamp => nearest_sampler.clone(),
                SamplerKind::NearestMirror => nearest_mirror_sampler.clone(),
                SamplerKind::NearestRepeat => nearest_repeat_sampler.clone(),
                SamplerKind::LinearMirror => linear_mirror_sampler.clone(),
                SamplerKind::LinearRepeat => linear_repeat_sampler.clone(),
                SamplerKind::LinearClamp => linear_clamp_sampler.clone(),
            })
            .collect();
        let fallback_sampler = linear_clamp_sampler.clone();

        // When shader compilation fails (wgpu create_shader_module), the error message can be
        // hard to correlate back to the generated WGSL. Dump it to a predictable temp location
        // so tests can inspect the exact module wgpu validated.
        let debug_dump_path = debug_dump_wgsl_dir
            .as_ref()
            .map(|dir| dir.join(format!("node-forge-pass.{}.wgsl", spec.name.as_str())));
        let shader_desc: wgpu::ShaderModuleDescriptor<'static> = wgpu::ShaderModuleDescriptor {
            label: Some("node-forge-pass"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(shader_wgsl.clone())),
        };
        if let Some(debug_dump_path) = debug_dump_path {
            if let Some(parent) = debug_dump_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&debug_dump_path, &shader_wgsl);
        }
        shader_space.render_pass(spec.name.clone(), move |builder| {
            let mut b = builder.shader(shader_desc).bind_uniform_buffer(
                0,
                0,
                params_buffer,
                ShaderStages::VERTEX_FRAGMENT,
            );

            if let Some(baked_data_parse_buffer) = spec.baked_data_parse_buffer.clone() {
                b = b.bind_storage_buffer(
                    0,
                    1,
                    baked_data_parse_buffer.as_str(),
                    ShaderStages::VERTEX_FRAGMENT,
                    true,
                );
            }

            if let Some(graph_binding) = graph_binding.clone() {
                b = match graph_binding.kind {
                    GraphBindingKind::Uniform => b.bind_uniform_buffer(
                        0,
                        2,
                        graph_binding.buffer_name.clone(),
                        ShaderStages::VERTEX_FRAGMENT,
                    ),
                    GraphBindingKind::StorageRead => b.bind_storage_buffer(
                        0,
                        2,
                        graph_binding.buffer_name.clone(),
                        ShaderStages::VERTEX_FRAGMENT,
                        true,
                    ),
                };
            }

            b = b.bind_attribute_buffer(
                0,
                geometry_buffer,
                wgpu::VertexStepMode::Vertex,
                vertex_attr_array![0 => Float32x3, 1 => Float32x2].to_vec(),
            );

            if let Some(instance_buffer) = spec.instance_buffer.clone() {
                b = b.bind_attribute_buffer(
                    1,
                    instance_buffer,
                    wgpu::VertexStepMode::Instance,
                    vertex_attr_array![
                        2 => Float32x4,
                        3 => Float32x4,
                        4 => Float32x4,
                        5 => Float32x4
                    ]
                    .to_vec(),
                );
            }

            if let Some(normals_buffer) = spec.normals_buffer.clone() {
                let normals_slot = if spec.instance_buffer.is_some() { 2 } else { 1 };
                b = b.bind_attribute_buffer(
                    normals_slot,
                    normals_buffer,
                    wgpu::VertexStepMode::Vertex,
                    vertex_attr_array![6 => Float32x3].to_vec(),
                );
            }

            debug_assert_eq!(texture_names.len(), sampler_names.len());
            for (i, tex_name) in texture_names.iter().enumerate() {
                let tex_binding = (i as u32) * 2;
                let samp_binding = tex_binding + 1;
                b = b
                    .bind_texture(1, tex_binding, tex_name.clone(), ShaderStages::FRAGMENT)
                    .bind_sampler(
                        1,
                        samp_binding,
                        sampler_names
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| fallback_sampler.clone()),
                        ShaderStages::FRAGMENT,
                    );
            }

            b = b
                .bind_color_attachment(target_texture)
                .sample_count(sample_count);
            if let Some(resolve_target) = resolve_target.clone() {
                b = b.resolve_target(resolve_target);
            }
            b.blending(blend_state).load_op(color_load_op)
        });
    }

    // Register image premultiply prepasses.
    for spec in &image_prepasses {
        let pass_name = spec.pass_name.clone();
        let geometry_buffer = spec.geometry_buffer.clone();
        let params_buffer = spec.params_buffer.clone();
        let src_texture = spec.src_texture.clone();
        let dst_texture = spec.dst_texture.clone();
        let shader_wgsl = spec.shader_wgsl.clone();
        let nearest_sampler_for_pass = nearest_sampler.clone();

        let shader_desc: wgpu::ShaderModuleDescriptor<'static> = wgpu::ShaderModuleDescriptor {
            label: Some("node-forge-imgpm"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(shader_wgsl.clone())),
        };

        shader_space.render_pass(pass_name, move |builder| {
            builder
                .shader(shader_desc)
                .bind_uniform_buffer(0, 0, params_buffer, ShaderStages::VERTEX_FRAGMENT)
                .bind_attribute_buffer(
                    0,
                    geometry_buffer,
                    wgpu::VertexStepMode::Vertex,
                    vertex_attr_array![0 => Float32x3, 1 => Float32x2].to_vec(),
                )
                .bind_texture(1, 0, src_texture, ShaderStages::FRAGMENT)
                .bind_sampler(1, 1, nearest_sampler_for_pass, ShaderStages::FRAGMENT)
                .bind_color_attachment(dst_texture)
                .blending(BlendState::REPLACE)
                .load_op(wgpu::LoadOp::Clear(Color::TRANSPARENT))
        });
    }

    if !prepass_names.is_empty() {
        let mut ordered: Vec<ResourceName> =
            Vec::with_capacity(prepass_names.len() + composite_passes.len());
        ordered.extend(prepass_names);
        ordered.extend(composite_passes);
        composite_passes = ordered;
    }

    fn compose_in_strict_order(
        composer: rust_wgpu_fiber::composition::CompositionBuilder,
        ordered_passes: &[ResourceName],
    ) -> rust_wgpu_fiber::composition::CompositionBuilder {
        match ordered_passes {
            [] => composer,
            [only] => composer.pass(only.clone()),
            _ => {
                let (deps, last) = ordered_passes.split_at(ordered_passes.len() - 1);
                let last = last[0].clone();
                composer.pass_with_deps(last, move |c| compose_in_strict_order(c, deps))
            }
        }
    }

    shader_space.composite(move |composer| compose_in_strict_order(composer, &composite_passes));

    shader_space.prepare();

    for spec in &render_pass_specs {
        shader_space.write_buffer(spec.params_buffer.as_str(), 0, as_bytes(&spec.params))?;
        if let (Some(graph_binding), Some(values)) = (&spec.graph_binding, &spec.graph_values) {
            shader_space.write_buffer(graph_binding.buffer_name.as_str(), 0, values)?;
        }
    }

    for spec in &image_prepasses {
        shader_space.write_buffer(spec.params_buffer.as_str(), 0, as_bytes(&spec.params))?;
    }

    Ok((
        shader_space,
        resolution,
        output_texture_name,
        pass_bindings,
        pipeline_signature,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::Node;
    use crate::renderer::scene_prep::composite_layers_in_draw_order;
    use image::{Rgba, RgbaImage};
    use serde_json::json;

    #[test]
    fn pass_textures_are_included_in_texture_bindings() {
        // Regression: previously we only bound `bundle.image_textures`, so shaders that used PassTexture
        // would declare @group(1) bindings that were missing from the pipeline layout.
        let mut reg = PassOutputRegistry::new();
        reg.register(PassOutputSpec {
            node_id: "upstream_pass".to_string(),
            texture_name: "up_tex".into(),
            resolution: [64, 64],
            format: TextureFormat::Rgba8Unorm,
        });

        let bindings = resolve_pass_texture_bindings(&reg, &["upstream_pass".to_string()]).unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].texture, ResourceName::from("up_tex"));
    }

    #[test]
    fn render_pass_blend_state_from_explicit_params() {
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        params.insert("blendfunc".to_string(), json!("add"));
        params.insert("src_factor".to_string(), json!("one"));
        params.insert("dst_factor".to_string(), json!("one-minus-src-alpha"));
        params.insert("src_alpha_factor".to_string(), json!("one"));
        params.insert("dst_alpha_factor".to_string(), json!("one-minus-src-alpha"));

        let got = crate::renderer::render_plan::parse_render_pass_blend_state(&params).unwrap();
        let expected = BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        assert_eq!(format!("{got:?}"), format!("{expected:?}"));
    }

    #[test]
    fn render_pass_blend_state_from_preset_alpha() {
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        params.insert("blend_preset".to_string(), json!("alpha"));
        let got = crate::renderer::render_plan::parse_render_pass_blend_state(&params).unwrap();
        let expected =
            crate::renderer::render_plan::blend::default_blend_state_for_preset("alpha").unwrap();
        assert_eq!(format!("{got:?}"), format!("{expected:?}"));
    }

    #[test]
    fn render_pass_blend_state_from_preset_premul_alpha() {
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        params.insert("blend_preset".to_string(), json!("premul-alpha"));

        let got = crate::renderer::render_plan::parse_render_pass_blend_state(&params).unwrap();
        let expected =
            crate::renderer::render_plan::blend::default_blend_state_for_preset("premul_alpha")
                .unwrap();
        assert_eq!(format!("{got:?}"), format!("{expected:?}"));
    }

    #[test]
    fn render_pass_blend_state_defaults_to_replace() {
        let params: HashMap<String, serde_json::Value> = HashMap::new();
        let got = crate::renderer::render_plan::parse_render_pass_blend_state(&params).unwrap();
        assert_eq!(format!("{got:?}"), format!("{:?}", BlendState::REPLACE));
    }

    #[test]
    fn sampled_render_pass_output_size_uses_coord_domain_for_processing_consumers() {
        let got = sampled_render_pass_output_size(true, false, [1080, 2400], [200.0, 200.0]);
        assert_eq!(got, [1080, 2400]);
    }

    #[test]
    fn sampled_render_pass_output_size_keeps_geometry_extent_without_processing_consumers() {
        let got = sampled_render_pass_output_size(false, false, [1080, 2400], [200.0, 200.0]);
        assert_eq!(got, [200, 200]);
    }

    #[test]
    fn sampled_render_pass_output_size_uses_geometry_extent_for_downsample_sources() {
        let got = sampled_render_pass_output_size(true, true, [2160, 2400], [1080.0, 2400.0]);
        assert_eq!(got, [1080, 2400]);
    }

    #[test]
    fn gaussian_blur_extend_upsample_geo_size_cancels_shrink() {
        let geo_size = gaussian_blur_extend_upsample_geo_size([200, 120], [240, 160]);
        assert!((geo_size[0] - 240.0).abs() < 1e-6);
        assert!((geo_size[1] - 160.0).abs() < 1e-6);
    }

    #[test]
    fn blur_downsample_steps_for_factor_matches_expected_chain() {
        assert_eq!(blur_downsample_steps_for_factor(1).unwrap(), vec![1]);
        assert_eq!(blur_downsample_steps_for_factor(2).unwrap(), vec![2]);
        assert_eq!(blur_downsample_steps_for_factor(4).unwrap(), vec![4]);
        assert_eq!(blur_downsample_steps_for_factor(8).unwrap(), vec![8]);
        assert_eq!(blur_downsample_steps_for_factor(16).unwrap(), vec![8, 2]);
    }

    #[test]
    fn blur_factor1_downsample_elision_only_triggers_for_factor1() {
        assert!(should_skip_blur_downsample_pass(1));
        assert!(!should_skip_blur_downsample_pass(2));
        assert!(!should_skip_blur_downsample_pass(4));
    }

    #[test]
    fn blur_factor1_upsample_elision_requires_sampled_and_non_extend() {
        assert!(should_skip_blur_upsample_pass(1, false, true));
        assert!(!should_skip_blur_upsample_pass(1, true, true));
        assert!(!should_skip_blur_upsample_pass(1, false, false));
        assert!(!should_skip_blur_upsample_pass(2, false, true));
    }

    #[test]
    fn msaa_requested_one_kept_as_single_sample() {
        let got = select_effective_msaa_sample_count(
            "rp",
            1,
            TextureFormat::Rgba8Unorm,
            wgpu::Features::empty(),
            None,
        )
        .expect("msaa selection");
        assert_eq!(got, 1);
    }

    #[test]
    fn msaa_unsupported_downgrades_to_single_sample_when_adapter_unavailable() {
        let got = select_effective_msaa_sample_count(
            "rp",
            2,
            TextureFormat::Rgba8Unorm,
            wgpu::Features::empty(),
            None,
        )
        .expect("msaa selection");
        assert_eq!(got, 1);
    }

    #[test]
    fn msaa_invalid_value_is_rejected() {
        let err = select_effective_msaa_sample_count(
            "rp",
            0,
            TextureFormat::Rgba8Unorm,
            wgpu::Features::empty(),
            None,
        )
        .expect_err("invalid value must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("must be one of 1,2,4,8"));
    }

    #[test]
    fn msaa_guaranteed_4x_is_kept_without_adapter_specific_feature() {
        let got = select_effective_msaa_sample_count(
            "rp",
            4,
            TextureFormat::Rgba8Unorm,
            wgpu::Features::empty(),
            None,
        )
        .expect("msaa selection");
        assert_eq!(got, 4);
    }

    #[test]
    fn msaa_8x_downgrades_to_4x_when_8x_unsupported() {
        let got = select_effective_msaa_sample_count(
            "rp",
            8,
            TextureFormat::Rgba8Unorm,
            wgpu::Features::empty(),
            None,
        )
        .expect("msaa selection");
        assert_eq!(got, 4);
    }

    #[test]
    fn data_url_decodes_png_bytes() {
        use base64::{Engine as _, engine::general_purpose};
        use image::codecs::png::PngEncoder;
        use image::{ExtendedColorType, ImageEncoder};

        // Build a valid 1x1 PNG in memory, then wrap it as a data URL.
        let src = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 0]));
        let mut png_bytes: Vec<u8> = Vec::new();
        PngEncoder::new(&mut png_bytes)
            .write_image(src.as_raw(), 1, 1, ExtendedColorType::Rgba8)
            .unwrap();

        let b64 = general_purpose::STANDARD.encode(&png_bytes);
        let data_url = format!("data:image/png;base64,{b64}");

        let img = load_image_from_data_url(&data_url).unwrap();
        assert_eq!(img.width(), 1);
        assert_eq!(img.height(), 1);
    }

    #[test]
    fn composite_draw_order_is_pass_then_dynamic_indices() {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: crate::dsl::Metadata {
                name: "test".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                crate::dsl::Node {
                    id: "out".to_string(),
                    node_type: "Composite".to_string(),
                    params: HashMap::new(),
                    inputs: vec![
                        crate::dsl::NodePort {
                            id: "dynamic_1".to_string(),
                            name: Some("image2".to_string()),
                            port_type: Some("color".to_string()),
                        },
                        crate::dsl::NodePort {
                            id: "dynamic_0".to_string(),
                            name: Some("image1".to_string()),
                            port_type: Some("color".to_string()),
                        },
                    ],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                crate::dsl::Node {
                    id: "p_img".to_string(),
                    node_type: "RenderPass".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                crate::dsl::Node {
                    id: "p0".to_string(),
                    node_type: "RenderPass".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                crate::dsl::Node {
                    id: "p1".to_string(),
                    node_type: "RenderPass".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
            ],
            connections: vec![
                crate::dsl::Connection {
                    id: "c_img".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "p_img".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "out".to_string(),
                        port_id: "pass".to_string(),
                    },
                },
                crate::dsl::Connection {
                    id: "c_dyn1".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "p1".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "out".to_string(),
                        port_id: "dynamic_1".to_string(),
                    },
                },
                crate::dsl::Connection {
                    id: "c_dyn0".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "p0".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "out".to_string(),
                        port_id: "dynamic_0".to_string(),
                    },
                },
            ],
            outputs: Some(HashMap::from([(
                String::from("composite"),
                String::from("out"),
            )])),
            groups: Vec::new(),
            assets: Default::default(),
        };

        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();

        let got = composite_layers_in_draw_order(&scene, &nodes_by_id, "out").unwrap();
        // inputs array order: dynamic_1 then dynamic_0
        assert_eq!(got, vec!["p_img", "p1", "p0"]);
    }

    #[test]
    fn sampled_pass_ids_detect_renderpass_used_by_pass_texture() -> Result<()> {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let scene_path =
            manifest_dir.join("tests/fixtures/render_cases/pass-texture-alpha/scene.json");
        if !scene_path.exists() {
            return Ok(());
        }
        let scene = crate::dsl::load_scene_from_path(&scene_path)?;
        let prepared = prepare_scene(&scene)?;

        let sampled = sampled_pass_node_ids(&prepared.scene, &prepared.nodes_by_id)?;
        assert!(
            sampled.contains("pass_up"),
            "expected sampled passes to include pass_up, got: {sampled:?}"
        );

        Ok(())
    }

    #[test]
    fn sampled_pass_ids_from_roots_ignores_dead_branch_sampling() -> Result<()> {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: crate::dsl::Metadata {
                name: "dead-branch-sampling".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                crate::dsl::Node {
                    id: "out".to_string(),
                    node_type: "Composite".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                crate::dsl::Node {
                    id: "rt".to_string(),
                    node_type: "RenderTexture".to_string(),
                    params: HashMap::from([
                        ("width".to_string(), json!(100)),
                        ("height".to_string(), json!(100)),
                    ]),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                crate::dsl::Node {
                    id: "p_live".to_string(),
                    node_type: "RenderPass".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                crate::dsl::Node {
                    id: "ds_dead".to_string(),
                    node_type: "Downsample".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
            ],
            connections: vec![
                crate::dsl::Connection {
                    id: "c_target".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "rt".to_string(),
                        port_id: "texture".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "out".to_string(),
                        port_id: "target".to_string(),
                    },
                },
                crate::dsl::Connection {
                    id: "c_out".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "p_live".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "out".to_string(),
                        port_id: "pass".to_string(),
                    },
                },
                // Dead branch: Downsample consumes p_live but is not reachable from Composite output.
                crate::dsl::Connection {
                    id: "c_dead_source".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "p_live".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "ds_dead".to_string(),
                        port_id: "source".to_string(),
                    },
                },
            ],
            outputs: Some(HashMap::from([(
                String::from("composite"),
                String::from("out"),
            )])),
            groups: Vec::new(),
            assets: HashMap::new(),
        };

        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();

        let roots = vec![String::from("p_live")];
        let sampled = sampled_pass_node_ids_from_roots(&scene, &nodes_by_id, &roots)?;
        assert!(
            !sampled.contains("p_live"),
            "dead branch should not force p_live to sampled output, got: {sampled:?}"
        );

        Ok(())
    }

    #[test]
    fn pass_order_supports_nested_composition_routing_nodes() -> Result<()> {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: crate::dsl::Metadata {
                name: "nested-comp".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                crate::dsl::Node {
                    id: "comp_a".to_string(),
                    node_type: "Composite".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                crate::dsl::Node {
                    id: "comp_b".to_string(),
                    node_type: "Composite".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                crate::dsl::Node {
                    id: "rt_a".to_string(),
                    node_type: "RenderTexture".to_string(),
                    params: HashMap::from([
                        ("width".to_string(), json!(100)),
                        ("height".to_string(), json!(100)),
                    ]),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                crate::dsl::Node {
                    id: "rt_b".to_string(),
                    node_type: "RenderTexture".to_string(),
                    params: HashMap::from([
                        ("width".to_string(), json!(200)),
                        ("height".to_string(), json!(200)),
                    ]),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
                crate::dsl::Node {
                    id: "ds".to_string(),
                    node_type: "Downsample".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                    input_bindings: Vec::new(),
                    outputs: Vec::new(),
                },
            ],
            connections: vec![
                crate::dsl::Connection {
                    id: "c_ta".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "rt_a".to_string(),
                        port_id: "texture".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "comp_a".to_string(),
                        port_id: "target".to_string(),
                    },
                },
                crate::dsl::Connection {
                    id: "c_tb".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "rt_b".to_string(),
                        port_id: "texture".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "comp_b".to_string(),
                        port_id: "target".to_string(),
                    },
                },
                crate::dsl::Connection {
                    id: "c_source".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "comp_a".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "ds".to_string(),
                        port_id: "source".to_string(),
                    },
                },
                crate::dsl::Connection {
                    id: "c_out".to_string(),
                    from: crate::dsl::Endpoint {
                        node_id: "ds".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: crate::dsl::Endpoint {
                        node_id: "comp_b".to_string(),
                        port_id: "pass".to_string(),
                    },
                },
            ],
            outputs: None,
            groups: Vec::new(),
            assets: HashMap::new(),
        };
        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();

        let order = compute_pass_render_order(&scene, &nodes_by_id, &[String::from("comp_b")])?;
        assert_eq!(order, vec!["comp_a", "ds", "comp_b"]);

        let sampled = sampled_pass_node_ids(&scene, &nodes_by_id)?;
        assert!(sampled.contains("comp_a"), "sampled={sampled:?}");
        assert!(sampled.contains("ds"), "sampled={sampled:?}");

        Ok(())
    }
}

pub(crate) fn build_error_shader_space_internal(
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    resolution: [u32; 2],
) -> Result<(
    ShaderSpace,
    [u32; 2],
    ResourceName,
    Vec<PassBindings>,
    [u8; 32],
)> {
    let mut shader_space = ShaderSpace::new(device, queue);

    let output_texture_name: ResourceName = "error_output".into();
    let pass_name: ResourceName = "error_pass".into();
    let geometry_buffer: ResourceName = "error_plane".into();

    let plane: [[f32; 3]; 6] = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ];
    let plane_bytes: Arc<[u8]> = Arc::from(as_bytes_slice(&plane).to_vec());

    shader_space.declare_buffers(vec![BufferSpec::Init {
        name: geometry_buffer.clone(),
        contents: plane_bytes,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
    }]);

    shader_space.declare_textures(vec![FiberTextureSpec::Texture {
        name: output_texture_name.clone(),
        resolution,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::RENDER_ATTACHMENT
            | TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_SRC,
        sample_count: 1,
    }]);

    let shader_desc: wgpu::ShaderModuleDescriptor<'static> = wgpu::ShaderModuleDescriptor {
        label: Some("node-forge-error-purple"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(ERROR_SHADER_WGSL)),
    };

    let output_texture_for_pass = output_texture_name.clone();
    shader_space.render_pass(pass_name.clone(), move |builder| {
        builder
            .shader(shader_desc)
            .bind_attribute_buffer(
                0,
                geometry_buffer,
                wgpu::VertexStepMode::Vertex,
                vertex_attr_array![0 => Float32x3].to_vec(),
            )
            .bind_color_attachment(output_texture_for_pass)
            .blending(BlendState::REPLACE)
            .load_op(wgpu::LoadOp::Clear(Color::BLACK))
    });

    shader_space.composite(move |composer| composer.pass(pass_name));
    shader_space.prepare();

    Ok((
        shader_space,
        resolution,
        output_texture_name,
        Vec::new(),
        [0_u8; 32],
    ))
}
