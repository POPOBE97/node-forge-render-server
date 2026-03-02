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
        camera::{
            legacy_projection_camera_matrix, pass_node_uses_custom_camera,
            resolve_effective_camera_for_pass_node,
        },
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
        wgsl_bloom::{
            BLOOM_MAX_MIPS, build_bloom_additive_combine_bundle, build_bloom_extract_bundle,
        },
    },
};

// Re-exports from extracted modules.
use super::image_utils::{ensure_rgba8, image_node_dimensions, load_image_from_path};
use super::pass_spec::{
    DepthResolvePass, ImagePrepass, PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl,
    TextureCapabilityRequirement,
    IDENTITY_MAT4, build_depth_resolve_wgsl, make_params,
};
use super::resource_naming::{
    UI_PRESENT_HDR_GAMMA_SUFFIX, UI_PRESENT_SDR_SRGB_SUFFIX,
    bloom_downsample_level_count, blur_downsample_steps_for_factor,
    build_hdr_gamma_encode_wgsl, build_srgb_display_encode_wgsl,
    fullscreen_processing_camera, gaussian_blur_extend_upsample_geo_size,
    infer_uniform_resolution_from_pass_deps, parse_render_pass_cull_mode,
    parse_render_pass_depth_test, parse_tint_from_node_or_default,
    readable_pass_name_for_node, resolve_chain_camera_for_first_pass,
    resolve_pass_texture_bindings, sampled_render_pass_output_size,
    sanitize_resource_segment, select_effective_msaa_sample_count,
    should_skip_blur_downsample_pass, should_skip_blur_upsample_pass,
    stable_short_id_suffix, validate_render_pass_msaa_request,
};
use super::sampler::{
    build_image_premultiply_wgsl, sampler_kind_for_pass_texture, sampler_kind_from_node_params,
};
use super::texture_caps::{
    collect_texture_capability_requirements, effective_texture_format_features,
    image_texture_wgpu_format, validate_texture_capability_requirements,
    validate_texture_capability_requirements_with_resolver,
};

// `image_node_dimensions` is now in super::image_utils (re-exported via mod.rs).
// `update_pass_params` is now in super::sampler (re-exported via mod.rs).

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
        "BloomNode" => {
            let source_conn = incoming_connection(scene, pass_node_id, "pass")
                .ok_or_else(|| anyhow!("BloomNode.pass missing for {pass_node_id}"))?;
            Ok(vec![source_conn.from.node_id.clone()])
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

pub(crate) fn build_shader_space_from_scene_internal(
    scene: &SceneDSL,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    adapter: Option<&wgpu::Adapter>,
    enable_display_encode: bool,
    debug_dump_wgsl_dir: Option<PathBuf>,
    asset_store: Option<&crate::asset_store::AssetStore>,
    presentation_mode: super::api::ShaderSpacePresentationMode,
) -> Result<(
    ShaderSpace,
    [u32; 2],
    ResourceName,
    Vec<PassBindings>,
    [u8; 32],
    Option<ResourceName>,
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
        resolve_scene_draw_contexts(&prepared.scene, nodes_by_id, ids, resolution, asset_store)?;
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
    let mut pass_cull_mode_by_name: HashMap<ResourceName, Option<wgpu::Face>> = HashMap::new();
    let mut pass_depth_attachment_by_name: HashMap<ResourceName, ResourceName> = HashMap::new();
    let mut depth_resolve_passes: Vec<DepthResolvePass> = Vec::new();
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
    let mut sampled_pass_ids = crate::renderer::render_plan::sampled_pass_node_ids_from_roots(
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
                    needs_sampling: false,
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

    // Create present textures for display-encode passes.
    //
    // Two separate textures may be created:
    //
    //  • HDR gamma-encoded (UiHdrNative + Rgba16Float only):
    //    Unclamped gamma-encoded values in a Rgba16Float texture for on-screen
    //    display on the Rgba16Float surface.  Values > 1.0 survive the
    //    egui round-trip (gamma → linear_from_gamma_rgb → original linear).
    //
    //  • SDR sRGB-encoded (for clipboard copy / headless PNG export):
    //    Clamped sRGB bytes in a Rgba8Unorm texture.  read_texture_rgba8
    //    returns gamma-encoded bytes suitable for PNG and clipboard.
    //    Also used for on-screen display in UiSdrDisplayEncode mode.
    //
    // For UiHdrNative + 8-bit targets: on-screen uses scene_output directly
    // (egui handles sRGB hardware decode); only the SDR export texture is
    // created when the raw storage bytes are linear (Rgba8Unorm/Bgra8Unorm).
    // sRGB-format targets already have gamma-encoded storage so no export
    // pass is needed either.
    let is_hdr_native = presentation_mode == super::api::ShaderSpacePresentationMode::UiHdrNative;

    // HDR gamma-encoded texture for on-screen display (UiHdrNative + Rgba16Float only).
    // egui's linear_from_gamma_rgb round-trips the unclamped gamma values back to linear
    // on the Rgba16Float surface, preserving EDR values > 1.0.
    let hdr_gamma_texture: Option<ResourceName> =
        if enable_display_encode && is_hdr_native && target_format == TextureFormat::Rgba16Float {
            let name: ResourceName = format!(
                "{}{}",
                target_texture_name.as_str(),
                UI_PRESENT_HDR_GAMMA_SUFFIX
            )
            .into();
            textures.push(TextureDecl {
                name: name.clone(),
                size: [tgt_w_u, tgt_h_u],
                format: TextureFormat::Rgba16Float,
                sample_count: 1,
                needs_sampling: false,
            });
            Some(name)
        } else {
            None
        };

    // SDR sRGB-encoded texture for clipboard copy and headless PNG export.
    // Also used for on-screen display in UiSdrDisplayEncode mode.
    //
    // NOT created for sRGB-format targets in UiHdrNative mode because their
    // storage bytes are already gamma-encoded — read_texture_rgba8 returns
    // correct sRGB bytes without an extra encode pass.
    let sdr_srgb_texture: Option<ResourceName> = if enable_display_encode {
        let needs_sdr = if is_hdr_native {
            // Export-only: needed when raw storage bytes are linear.
            matches!(
                target_format,
                TextureFormat::Rgba8Unorm
                    | TextureFormat::Bgra8Unorm
                    | TextureFormat::Rgba16Float
            )
        } else {
            // UiSdrDisplayEncode: always create for on-screen + export.
            matches!(
                target_format,
                TextureFormat::Rgba8UnormSrgb
                    | TextureFormat::Bgra8UnormSrgb
                    | TextureFormat::Rgba8Unorm
                    | TextureFormat::Bgra8Unorm
            )
        };
        if needs_sdr {
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
                needs_sampling: false,
            });
            Some(name)
        } else {
            None
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
    let warmup_root_ids = crate::renderer::render_plan::forward_root_dependencies_from_roots(
        &prepared.scene,
        nodes_by_id,
        composite_layers_in_order,
    )?;
    sampled_pass_ids.extend(warmup_root_ids.iter().cloned());

    let warmup_items: Vec<(String, bool)> = composite_layers_in_order
        .iter()
        .filter(|id| warmup_root_ids.contains(*id))
        .cloned()
        .map(|id| (id, true))
        .collect();
    let normal_items: Vec<(String, bool)> = pass_nodes_in_order
        .iter()
        .cloned()
        .map(|id| (id, false))
        .collect();
    let mut execution_items: Vec<(String, bool)> =
        Vec::with_capacity(warmup_items.len() + normal_items.len());
    execution_items.extend(warmup_items);
    execution_items.extend(normal_items);
    let mut deferred_target_compose_passes_by_layer: HashMap<String, Vec<ResourceName>> =
        HashMap::new();

    // Pass nodes used as resample/filter sources keep special dynamic-geometry fullscreen handling.
    let mut downsample_source_pass_ids: HashSet<String> = HashSet::new();
    let mut upsample_source_pass_ids: HashSet<String> = HashSet::new();
    let mut gaussian_source_pass_ids: HashSet<String> = HashSet::new();
    let mut bloom_source_pass_ids: HashSet<String> = HashSet::new();
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
        if node.node_type == "BloomNode" {
            if let Some(conn) = incoming_connection(&prepared.scene, node_id, "pass") {
                let src_is_pass_like = nodes_by_id
                    .get(&conn.from.node_id)
                    .is_some_and(|n| is_pass_like_node_type(&n.node_type));
                if src_is_pass_like {
                    bloom_source_pass_ids.insert(conn.from.node_id.clone());
                }
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

    for (layer_id, is_warmup_pass) in &execution_items {
        let layer_id = layer_id;
        if !*is_warmup_pass && warmup_root_ids.contains(layer_id) {
            if let Some(deferred_passes) = deferred_target_compose_passes_by_layer.get(layer_id) {
                composite_passes.extend(deferred_passes.iter().cloned());
                continue;
            }
        }

        let branch_spec_start = render_pass_specs.len();
        let branch_composite_start = composite_passes.len();
        let layer_node = find_node(&nodes_by_id, layer_id)?;
        match layer_node.node_type.as_str() {
            "RenderPass" => {
                let sc = super::pass_assemblers::args::SceneContext {
                    prepared: &prepared,
                    composition_contexts: &composition_contexts,
                    composition_consumers_by_source: &composition_consumers_by_source,
                    draw_coord_size_by_pass: &draw_coord_size_by_pass,
                    asset_store,
                    device: &device,
                    adapter,
                };
                let mut bs = super::pass_assemblers::args::BuilderState {
                    target_texture_name: &target_texture_name,
                    target_format,
                    sampled_pass_format,
                    tgt_size: [tgt_w, tgt_h],
                    tgt_size_u: [tgt_w_u, tgt_h_u],
                    geometry_buffers: &mut geometry_buffers,
                    instance_buffers: &mut instance_buffers,
                    textures: &mut textures,
                    render_pass_specs: &mut render_pass_specs,
                    composite_passes: &mut composite_passes,
                    depth_resolve_passes: &mut depth_resolve_passes,
                    pass_cull_mode_by_name: &mut pass_cull_mode_by_name,
                    pass_depth_attachment_by_name: &mut pass_depth_attachment_by_name,
                    pass_output_registry: &mut pass_output_registry,
                    sampled_pass_ids: &sampled_pass_ids,
                    baked_data_parse_meta_by_pass: &mut baked_data_parse_meta_by_pass,
                    baked_data_parse_bytes_by_pass: &mut baked_data_parse_bytes_by_pass,
                    baked_data_parse_buffer_to_pass_id: &mut baked_data_parse_buffer_to_pass_id,
                    downsample_source_pass_ids: &mut downsample_source_pass_ids,
                    upsample_source_pass_ids: &mut upsample_source_pass_ids,
                    gaussian_source_pass_ids: &mut gaussian_source_pass_ids,
                    bloom_source_pass_ids: &mut bloom_source_pass_ids,
                    gradient_source_pass_ids: &mut gradient_source_pass_ids,
                };
                super::pass_assemblers::render_pass::assemble_render_pass(
                    &sc, &mut bs, layer_id, layer_node,
                )?;
            }
            "BloomNode" => {
                let sc = super::pass_assemblers::args::SceneContext {
                    prepared: &prepared,
                    composition_contexts: &composition_contexts,
                    composition_consumers_by_source: &composition_consumers_by_source,
                    draw_coord_size_by_pass: &draw_coord_size_by_pass,
                    asset_store,
                    device: &device,
                    adapter,
                };
                let mut bs = super::pass_assemblers::args::BuilderState {
                    target_texture_name: &target_texture_name,
                    target_format,
                    sampled_pass_format,
                    tgt_size: [tgt_w, tgt_h],
                    tgt_size_u: [tgt_w_u, tgt_h_u],
                    geometry_buffers: &mut geometry_buffers,
                    instance_buffers: &mut instance_buffers,
                    textures: &mut textures,
                    render_pass_specs: &mut render_pass_specs,
                    composite_passes: &mut composite_passes,
                    depth_resolve_passes: &mut depth_resolve_passes,
                    pass_cull_mode_by_name: &mut pass_cull_mode_by_name,
                    pass_depth_attachment_by_name: &mut pass_depth_attachment_by_name,
                    pass_output_registry: &mut pass_output_registry,
                    sampled_pass_ids: &sampled_pass_ids,
                    baked_data_parse_meta_by_pass: &mut baked_data_parse_meta_by_pass,
                    baked_data_parse_bytes_by_pass: &mut baked_data_parse_bytes_by_pass,
                    baked_data_parse_buffer_to_pass_id: &mut baked_data_parse_buffer_to_pass_id,
                    downsample_source_pass_ids: &mut downsample_source_pass_ids,
                    upsample_source_pass_ids: &mut upsample_source_pass_ids,
                    gaussian_source_pass_ids: &mut gaussian_source_pass_ids,
                    bloom_source_pass_ids: &mut bloom_source_pass_ids,
                    gradient_source_pass_ids: &mut gradient_source_pass_ids,
                };
                super::pass_assemblers::bloom::assemble_bloom(
                    &sc, &mut bs, layer_id, layer_node,
                )?;
            }
            "GuassianBlurPass" => {
                let sc = super::pass_assemblers::args::SceneContext {
                    prepared: &prepared,
                    composition_contexts: &composition_contexts,
                    composition_consumers_by_source: &composition_consumers_by_source,
                    draw_coord_size_by_pass: &draw_coord_size_by_pass,
                    asset_store,
                    device: &device,
                    adapter: adapter.as_deref(),
                };
                let mut bs = super::pass_assemblers::args::BuilderState {
                    target_texture_name: &target_texture_name,
                    target_format,
                    sampled_pass_format,
                    tgt_size: [tgt_w, tgt_h],
                    tgt_size_u: [tgt_w_u, tgt_h_u],
                    geometry_buffers: &mut geometry_buffers,
                    instance_buffers: &mut instance_buffers,
                    textures: &mut textures,
                    render_pass_specs: &mut render_pass_specs,
                    composite_passes: &mut composite_passes,
                    depth_resolve_passes: &mut depth_resolve_passes,
                    pass_cull_mode_by_name: &mut pass_cull_mode_by_name,
                    pass_depth_attachment_by_name: &mut pass_depth_attachment_by_name,
                    pass_output_registry: &mut pass_output_registry,
                    sampled_pass_ids: &sampled_pass_ids,
                    baked_data_parse_meta_by_pass: &mut baked_data_parse_meta_by_pass,
                    baked_data_parse_bytes_by_pass: &mut baked_data_parse_bytes_by_pass,
                    baked_data_parse_buffer_to_pass_id: &mut baked_data_parse_buffer_to_pass_id,
                    downsample_source_pass_ids: &mut downsample_source_pass_ids,
                    upsample_source_pass_ids: &mut upsample_source_pass_ids,
                    gaussian_source_pass_ids: &mut gaussian_source_pass_ids,
                    bloom_source_pass_ids: &mut bloom_source_pass_ids,
                    gradient_source_pass_ids: &mut gradient_source_pass_ids,
                };
                super::pass_assemblers::gaussian_blur::assemble_gaussian_blur(
                    &sc, &mut bs, layer_id, layer_node,
                )?;
            }
            "GradientBlur" => {
                let sc = super::pass_assemblers::args::SceneContext {
                    prepared: &prepared,
                    composition_contexts: &composition_contexts,
                    composition_consumers_by_source: &composition_consumers_by_source,
                    draw_coord_size_by_pass: &draw_coord_size_by_pass,
                    asset_store,
                    device: &device,
                    adapter: adapter.as_deref(),
                };
                let mut bs = super::pass_assemblers::args::BuilderState {
                    target_texture_name: &target_texture_name,
                    target_format,
                    sampled_pass_format,
                    tgt_size: [tgt_w, tgt_h],
                    tgt_size_u: [tgt_w_u, tgt_h_u],
                    geometry_buffers: &mut geometry_buffers,
                    instance_buffers: &mut instance_buffers,
                    textures: &mut textures,
                    render_pass_specs: &mut render_pass_specs,
                    composite_passes: &mut composite_passes,
                    depth_resolve_passes: &mut depth_resolve_passes,
                    pass_cull_mode_by_name: &mut pass_cull_mode_by_name,
                    pass_depth_attachment_by_name: &mut pass_depth_attachment_by_name,
                    pass_output_registry: &mut pass_output_registry,
                    sampled_pass_ids: &sampled_pass_ids,
                    baked_data_parse_meta_by_pass: &mut baked_data_parse_meta_by_pass,
                    baked_data_parse_bytes_by_pass: &mut baked_data_parse_bytes_by_pass,
                    baked_data_parse_buffer_to_pass_id: &mut baked_data_parse_buffer_to_pass_id,
                    downsample_source_pass_ids: &mut downsample_source_pass_ids,
                    upsample_source_pass_ids: &mut upsample_source_pass_ids,
                    gaussian_source_pass_ids: &mut gaussian_source_pass_ids,
                    bloom_source_pass_ids: &mut bloom_source_pass_ids,
                    gradient_source_pass_ids: &mut gradient_source_pass_ids,
                };
                super::pass_assemblers::gradient_blur::assemble_gradient_blur(
                    &sc, &mut bs, layer_id, layer_node,
                )?;
            }
            "Downsample" => {
                let sc = super::pass_assemblers::args::SceneContext {
                    prepared: &prepared,
                    composition_contexts: &composition_contexts,
                    composition_consumers_by_source: &composition_consumers_by_source,
                    draw_coord_size_by_pass: &draw_coord_size_by_pass,
                    asset_store,
                    device: &device,
                    adapter,
                };
                let mut bs = super::pass_assemblers::args::BuilderState {
                    target_texture_name: &target_texture_name,
                    target_format,
                    sampled_pass_format,
                    tgt_size: [tgt_w, tgt_h],
                    tgt_size_u: [tgt_w_u, tgt_h_u],
                    geometry_buffers: &mut geometry_buffers,
                    instance_buffers: &mut instance_buffers,
                    textures: &mut textures,
                    render_pass_specs: &mut render_pass_specs,
                    composite_passes: &mut composite_passes,
                    depth_resolve_passes: &mut depth_resolve_passes,
                    pass_cull_mode_by_name: &mut pass_cull_mode_by_name,
                    pass_depth_attachment_by_name: &mut pass_depth_attachment_by_name,
                    pass_output_registry: &mut pass_output_registry,
                    sampled_pass_ids: &sampled_pass_ids,
                    baked_data_parse_meta_by_pass: &mut baked_data_parse_meta_by_pass,
                    baked_data_parse_bytes_by_pass: &mut baked_data_parse_bytes_by_pass,
                    baked_data_parse_buffer_to_pass_id: &mut baked_data_parse_buffer_to_pass_id,
                    downsample_source_pass_ids: &mut downsample_source_pass_ids,
                    upsample_source_pass_ids: &mut upsample_source_pass_ids,
                    gaussian_source_pass_ids: &mut gaussian_source_pass_ids,
                    bloom_source_pass_ids: &mut bloom_source_pass_ids,
                    gradient_source_pass_ids: &mut gradient_source_pass_ids,
                };
                super::pass_assemblers::downsample::assemble_downsample(
                    &sc, &mut bs, layer_id, layer_node,
                )?;
            }
            "Upsample" => {
                let sc = super::pass_assemblers::args::SceneContext {
                    prepared: &prepared,
                    composition_contexts: &composition_contexts,
                    composition_consumers_by_source: &composition_consumers_by_source,
                    draw_coord_size_by_pass: &draw_coord_size_by_pass,
                    asset_store,
                    device: &device,
                    adapter,
                };
                let mut bs = super::pass_assemblers::args::BuilderState {
                    target_texture_name: &target_texture_name,
                    target_format,
                    sampled_pass_format,
                    tgt_size: [tgt_w, tgt_h],
                    tgt_size_u: [tgt_w_u, tgt_h_u],
                    geometry_buffers: &mut geometry_buffers,
                    instance_buffers: &mut instance_buffers,
                    textures: &mut textures,
                    render_pass_specs: &mut render_pass_specs,
                    composite_passes: &mut composite_passes,
                    depth_resolve_passes: &mut depth_resolve_passes,
                    pass_cull_mode_by_name: &mut pass_cull_mode_by_name,
                    pass_depth_attachment_by_name: &mut pass_depth_attachment_by_name,
                    pass_output_registry: &mut pass_output_registry,
                    sampled_pass_ids: &sampled_pass_ids,
                    baked_data_parse_meta_by_pass: &mut baked_data_parse_meta_by_pass,
                    baked_data_parse_bytes_by_pass: &mut baked_data_parse_bytes_by_pass,
                    baked_data_parse_buffer_to_pass_id: &mut baked_data_parse_buffer_to_pass_id,
                    downsample_source_pass_ids: &mut downsample_source_pass_ids,
                    upsample_source_pass_ids: &mut upsample_source_pass_ids,
                    gaussian_source_pass_ids: &mut gaussian_source_pass_ids,
                    bloom_source_pass_ids: &mut bloom_source_pass_ids,
                    gradient_source_pass_ids: &mut gradient_source_pass_ids,
                };
                super::pass_assemblers::upsample::assemble_upsample(
                    &sc, &mut bs, layer_id, layer_node,
                )?;
            }
            "Composite" => {
                let sc = super::pass_assemblers::args::SceneContext {
                    prepared: &prepared,
                    composition_contexts: &composition_contexts,
                    composition_consumers_by_source: &composition_consumers_by_source,
                    draw_coord_size_by_pass: &draw_coord_size_by_pass,
                    asset_store,
                    device: &device,
                    adapter,
                };
                let mut bs = super::pass_assemblers::args::BuilderState {
                    target_texture_name: &target_texture_name,
                    target_format,
                    sampled_pass_format,
                    tgt_size: [tgt_w, tgt_h],
                    tgt_size_u: [tgt_w_u, tgt_h_u],
                    geometry_buffers: &mut geometry_buffers,
                    instance_buffers: &mut instance_buffers,
                    textures: &mut textures,
                    render_pass_specs: &mut render_pass_specs,
                    composite_passes: &mut composite_passes,
                    depth_resolve_passes: &mut depth_resolve_passes,
                    pass_cull_mode_by_name: &mut pass_cull_mode_by_name,
                    pass_depth_attachment_by_name: &mut pass_depth_attachment_by_name,
                    pass_output_registry: &mut pass_output_registry,
                    sampled_pass_ids: &sampled_pass_ids,
                    baked_data_parse_meta_by_pass: &mut baked_data_parse_meta_by_pass,
                    baked_data_parse_bytes_by_pass: &mut baked_data_parse_bytes_by_pass,
                    baked_data_parse_buffer_to_pass_id: &mut baked_data_parse_buffer_to_pass_id,
                    downsample_source_pass_ids: &mut downsample_source_pass_ids,
                    upsample_source_pass_ids: &mut upsample_source_pass_ids,
                    gaussian_source_pass_ids: &mut gaussian_source_pass_ids,
                    bloom_source_pass_ids: &mut bloom_source_pass_ids,
                    gradient_source_pass_ids: &mut gradient_source_pass_ids,
                };
                super::pass_assemblers::composite::assemble_composite(
                    &sc, &mut bs, layer_id, layer_node,
                )?;
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

        if *is_warmup_pass {
            let target_writer_names: HashSet<ResourceName> = render_pass_specs[branch_spec_start..]
                .iter()
                .filter(|spec| spec.target_texture == target_texture_name)
                .map(|spec| spec.name.clone())
                .collect();
            if !target_writer_names.is_empty() {
                let prefix: Vec<ResourceName> = composite_passes[..branch_composite_start].to_vec();
                let mut deferred: Vec<ResourceName> = Vec::new();
                let mut suffix: Vec<ResourceName> = Vec::new();
                for name in composite_passes[branch_composite_start..].iter().cloned() {
                    if target_writer_names.contains(&name) {
                        deferred.push(name);
                    } else {
                        suffix.push(name);
                    }
                }
                if !deferred.is_empty() {
                    deferred_target_compose_passes_by_layer
                        .entry(layer_id.clone())
                        .or_default()
                        .extend(deferred);
                }
                composite_passes = prefix.into_iter().chain(suffix.into_iter()).collect();
            }
        }
    }

    // Final display-encode passes: gamma-encode linear scene output.
    //
    // Up to two passes may be created:
    //  1. SDR sRGB encode  → clamped [0,1] sRGB bytes in Rgba8Unorm
    //     (for clipboard / headless PNG, and for on-screen in UiSdrDisplayEncode).
    //  2. HDR gamma encode → unclamped sRGB gamma in Rgba16Float
    //     (for on-screen in UiHdrNative, preserving EDR values > 1.0).
    //
    // The SDR pass runs first so both passes read the same linear source.
    // The HDR pass (if any) runs last — it is the on-screen presentation texture.
    //
    // In UiHdrNative mode the SDR encode pass is registered (pipeline compiled)
    // but excluded from the per-frame composition.  It is executed on-demand
    // only when the user copies to clipboard or exports.  This avoids the
    // encode accidentally affecting the on-screen presentation path.
    let mut sdr_encode_pass_name: Option<ResourceName> = None;
    {
        let encode_passes: Vec<(&ResourceName, &str, String)> = [
            sdr_srgb_texture.as_ref().map(|tex| {
                (
                    tex,
                    UI_PRESENT_SDR_SRGB_SUFFIX,
                    build_srgb_display_encode_wgsl("src_tex", "src_samp"),
                )
            }),
            hdr_gamma_texture.as_ref().map(|tex| {
                (
                    tex,
                    UI_PRESENT_HDR_GAMMA_SUFFIX,
                    build_hdr_gamma_encode_wgsl("src_tex", "src_samp"),
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect();

        for (encode_tex, suffix, shader_wgsl) in encode_passes {
            let pass_name: ResourceName =
                format!("{}{}.pass", target_texture_name.as_str(), suffix).into();
            let geo: ResourceName =
                format!("{}{}.geo", target_texture_name.as_str(), suffix).into();
            geometry_buffers.push((geo.clone(), make_fullscreen_geometry(tgt_w, tgt_h)));

            let params_name: ResourceName =
                format!("params.{}{}", target_texture_name.as_str(), suffix).into();
            let params = make_params(
                [tgt_w, tgt_h],
                [tgt_w, tgt_h],
                [tgt_w * 0.5, tgt_h * 0.5],
                legacy_projection_camera_matrix([tgt_w, tgt_h]),
                [0.0, 0.0, 0.0, 0.0],
            );

            render_pass_specs.push(RenderPassSpec {
                pass_id: pass_name.as_str().to_string(),
                name: pass_name.clone(),
                geometry_buffer: geo,
                instance_buffer: None,
                normals_buffer: None,
                target_texture: encode_tex.clone(),
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

            // In UiHdrNative mode ALL encode passes are kept out of the
            // per-frame composition.  The SDR pass is triggered on-demand for
            // clipboard / export.  The HDR gamma pass is not needed because
            // macOS already applies sRGB on the Rgba16Float surface — running
            // an additional gamma encode would cause double-gamma.
            if is_hdr_native {
                if suffix == UI_PRESENT_SDR_SRGB_SUFFIX {
                    sdr_encode_pass_name = Some(pass_name);
                }
                // HDR gamma pass: simply skip adding to composite_passes.
            } else {
                composite_passes.push(pass_name);
            }
        }
    }

    // Clear each render texture only on its first write in actual execution order.
    // If multiple passes target the same texture, subsequent passes must Load so
    // alpha blending can accumulate.
    //
    // Important: execution order is driven by `composite_passes` (including warmup/deferred
    // rewrites), not by `render_pass_specs` insertion order.
    {
        let pass_order: HashMap<ResourceName, usize> = composite_passes
            .iter()
            .enumerate()
            .map(|(idx, name)| (name.clone(), idx))
            .collect();

        let mut spec_indices_by_exec_order: Vec<usize> = (0..render_pass_specs.len()).collect();
        spec_indices_by_exec_order.sort_by_key(|idx| {
            let name = &render_pass_specs[*idx].name;
            pass_order.get(name).copied().unwrap_or(usize::MAX)
        });

        let mut seen_targets: HashSet<ResourceName> = HashSet::new();
        for idx in spec_indices_by_exec_order {
            let spec = &mut render_pass_specs[idx];
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
                let base = TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC;
                if t.needs_sampling {
                    base | TextureUsages::TEXTURE_BINDING
                } else {
                    base
                }
            } else {
                TextureUsages::RENDER_ATTACHMENT
                    | TextureUsages::TEXTURE_BINDING
                    | TextureUsages::COPY_SRC
            },
            sample_count: t.sample_count,
        })
        .collect();

    let mut image_prepasses: Vec<ImagePrepass> = Vec::new();
    let mut prepass_buffer_specs: Vec<BufferSpec> = Vec::new();
    let mut prepass_names: Vec<ResourceName> = Vec::new();
    let mut prepass_texture_samples: Vec<(String, ResourceName)> = Vec::new();

    // ImageTexture resources (sampled textures) referenced by any reachable RenderPass.
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

                let params = make_params(
                    [w, h],
                    [w, h],
                    [w * 0.5, h * 0.5],
                    legacy_projection_camera_matrix([w, h]),
                    [0.0, 0.0, 0.0, 0.0],
                );

                let pass_name: ResourceName =
                    format!("sys.image.{node_id}.premultiply.pass").into();
                let tex_var = MaterialCompileContext::tex_var_name(src_name.as_str());
                let samp_var = MaterialCompileContext::sampler_var_name(src_name.as_str());
                let shader_wgsl = build_image_premultiply_wgsl(&tex_var, &samp_var);
                let prepass_src_name = src_name.clone();

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
                prepass_texture_samples.push((
                    format!("sys.image.{node_id}.premultiply.pass"),
                    prepass_src_name,
                ));
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

    // Depth-resolve pass buffers (geometry + params).
    if !depth_resolve_passes.is_empty() {
        let mut dr_buffer_specs: Vec<BufferSpec> = Vec::new();
        for drp in &depth_resolve_passes {
            dr_buffer_specs.push(BufferSpec::Init {
                name: drp.geometry_buffer.clone(),
                contents: make_fullscreen_geometry(
                    drp.params.target_size[0],
                    drp.params.target_size[1],
                ),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            dr_buffer_specs.push(BufferSpec::Sized {
                name: drp.params_buffer.clone(),
                size: core::mem::size_of::<Params>(),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        }
        shader_space.declare_buffers(dr_buffer_specs);
    }

    let texture_capability_requirements = collect_texture_capability_requirements(
        &texture_specs,
        &render_pass_specs,
        &prepass_texture_samples,
    )?;
    validate_texture_capability_requirements(
        &texture_capability_requirements,
        shader_space.device.features(),
        adapter,
    )?;

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
        let cull_mode = pass_cull_mode_by_name
            .get(&spec.name)
            .copied()
            .unwrap_or(None);
        let depth_stencil_attachment = pass_depth_attachment_by_name.get(&spec.name).cloned();
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
            if let Some(depth_tex) = depth_stencil_attachment.clone() {
                b = b.bind_depth_stencil_attachment(depth_tex);
            }
            if let Some(resolve_target) = resolve_target.clone() {
                b = b.resolve_target(resolve_target);
            }
            b.cull_mode(cull_mode)
                .blending(blend_state)
                .load_op(color_load_op)
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

    // Register depth-resolve passes.
    for spec in &depth_resolve_passes {
        let pass_name = spec.pass_name.clone();
        let geometry_buffer = spec.geometry_buffer.clone();
        let params_buffer = spec.params_buffer.clone();
        let depth_texture = spec.depth_texture.clone();
        let dst_texture = spec.dst_texture.clone();
        let shader_wgsl = spec.shader_wgsl.clone();
        let is_multisampled = spec.is_multisampled;

        let shader_desc: wgpu::ShaderModuleDescriptor<'static> = wgpu::ShaderModuleDescriptor {
            label: Some("node-forge-depth-resolve"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(shader_wgsl)),
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
                .bind_depth_texture(1, 0, depth_texture, ShaderStages::FRAGMENT, is_multisampled)
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

    for spec in &depth_resolve_passes {
        shader_space.write_buffer(spec.params_buffer.as_str(), 0, as_bytes(&spec.params))?;
    }

    Ok((
        shader_space,
        resolution,
        output_texture_name,
        pass_bindings,
        pipeline_signature,
        sdr_encode_pass_name,
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
    fn infer_blur_source_resolution_from_uniform_pass_deps() {
        let mut reg = PassOutputRegistry::new();
        reg.register(PassOutputSpec {
            node_id: "p0".to_string(),
            texture_name: "tex.p0".into(),
            resolution: [33, 75],
            format: TextureFormat::Rgba8Unorm,
        });
        reg.register(PassOutputSpec {
            node_id: "p1".to_string(),
            texture_name: "tex.p1".into(),
            resolution: [33, 75],
            format: TextureFormat::Rgba8Unorm,
        });

        let got = infer_uniform_resolution_from_pass_deps(
            "blur_1",
            &["p0".to_string(), "p1".to_string()],
            &reg,
        )
        .expect("resolution inference should succeed");
        assert_eq!(got, Some([33, 75]));
    }

    #[test]
    fn infer_blur_source_resolution_errors_on_mixed_sizes() {
        let mut reg = PassOutputRegistry::new();
        reg.register(PassOutputSpec {
            node_id: "p0".to_string(),
            texture_name: "tex.p0".into(),
            resolution: [33, 75],
            format: TextureFormat::Rgba8Unorm,
        });
        reg.register(PassOutputSpec {
            node_id: "p1".to_string(),
            texture_name: "tex.p1".into(),
            resolution: [67, 150],
            format: TextureFormat::Rgba8Unorm,
        });

        let err = infer_uniform_resolution_from_pass_deps(
            "blur_1",
            &["p0".to_string(), "p1".to_string()],
            &reg,
        )
        .expect_err("mismatched sizes must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("GuassianBlurPass blur_1"));
        assert!(msg.contains("p0=33x75"));
        assert!(msg.contains("p1=67x150"));
    }

    #[test]
    fn infer_blur_source_resolution_returns_none_without_pass_deps() {
        let reg = PassOutputRegistry::new();
        let got = infer_uniform_resolution_from_pass_deps("blur_1", &[], &reg)
            .expect("empty deps should not fail");
        assert_eq!(got, None);
    }

    #[test]
    fn back_pin_pin_blur_27_infers_33x75_from_mathclosure_pass_deps() -> Result<()> {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let scene_path = manifest_dir.join("tests/cases/back-pin-pin/scene.json");
        let scene = crate::dsl::load_scene_from_path(scene_path)?;
        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();

        let bundle = build_blur_image_wgsl_bundle(&scene, &nodes_by_id, "GuassianBlurPass_27")?;
        assert_eq!(
            bundle.pass_textures,
            vec!["Downsample_18".to_string(), "Upsample_24".to_string()]
        );

        let mut reg = PassOutputRegistry::new();
        reg.register(PassOutputSpec {
            node_id: "Downsample_18".to_string(),
            texture_name: "sys.downsample.Downsample_18.out".into(),
            resolution: [33, 75],
            format: TextureFormat::Rgba8Unorm,
        });
        reg.register(PassOutputSpec {
            node_id: "Upsample_24".to_string(),
            texture_name: "sys.upsample.Upsample_24.out".into(),
            resolution: [33, 75],
            format: TextureFormat::Rgba8Unorm,
        });

        let inferred = infer_uniform_resolution_from_pass_deps(
            "GuassianBlurPass_27",
            &bundle.pass_textures,
            &reg,
        )?;
        assert_eq!(inferred, Some([33, 75]));

        Ok(())
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

    fn make_format_features(
        allowed_usages: TextureUsages,
        flags: wgpu::TextureFormatFeatureFlags,
    ) -> wgpu::TextureFormatFeatures {
        wgpu::TextureFormatFeatures {
            allowed_usages,
            flags,
        }
    }

    #[test]
    fn texture_capability_validation_fails_when_required_usage_is_missing() {
        let req = TextureCapabilityRequirement {
            name: "rt".into(),
            format: TextureFormat::Rgba16Float,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            sample_count: 1,
            sampled_by_passes: Vec::new(),
            blend_target_passes: Vec::new(),
        };

        let err = validate_texture_capability_requirements_with_resolver(&[req], |_| {
            make_format_features(
                TextureUsages::RENDER_ATTACHMENT,
                wgpu::TextureFormatFeatureFlags::empty(),
            )
        })
        .expect_err("missing usage should fail");

        let msg = err.to_string();
        assert!(msg.contains("missing required usages"));
        assert!(msg.contains("rt"));
    }

    #[test]
    fn texture_capability_validation_fails_when_filterable_flag_is_missing() {
        let req = TextureCapabilityRequirement {
            name: "rt".into(),
            format: TextureFormat::Rgba16Float,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC,
            sample_count: 1,
            sampled_by_passes: vec!["pass_a".to_string()],
            blend_target_passes: Vec::new(),
        };

        let err = validate_texture_capability_requirements_with_resolver(&[req], |_| {
            make_format_features(
                TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC,
                wgpu::TextureFormatFeatureFlags::empty(),
            )
        })
        .expect_err("missing filterable should fail");

        let msg = err.to_string();
        assert!(msg.contains("FILTERABLE"));
        assert!(msg.contains("pass_a"));
    }

    #[test]
    fn texture_capability_validation_fails_when_blendable_flag_is_missing() {
        let req = TextureCapabilityRequirement {
            name: "rt".into(),
            format: TextureFormat::Rgba16Float,
            usage: TextureUsages::RENDER_ATTACHMENT,
            sample_count: 1,
            sampled_by_passes: Vec::new(),
            blend_target_passes: vec!["pass_b".to_string()],
        };

        let err = validate_texture_capability_requirements_with_resolver(&[req], |_| {
            make_format_features(
                TextureUsages::RENDER_ATTACHMENT,
                wgpu::TextureFormatFeatureFlags::FILTERABLE,
            )
        })
        .expect_err("missing blendable should fail");

        let msg = err.to_string();
        assert!(msg.contains("BLENDABLE"));
        assert!(msg.contains("pass_b"));
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
        label: Some("node-forge-error-fallback"),
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
