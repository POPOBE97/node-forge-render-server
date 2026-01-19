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

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use anyhow::{Result, anyhow, bail};
use image::{DynamicImage, Rgba, RgbaImage};
use rust_wgpu_fiber::{
    HeadlessRenderer, HeadlessRendererConfig, ResourceName,
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
        node_compiler::{compile_vertex_expr, geometry_nodes::rect2d_geometry_vertices},
        scene_prep::{bake_data_parse_nodes, prepare_scene},
        types::ValueType,
        types::{
            BakedDataParseMeta, BakedValue, MaterialCompileContext, Params, PassBindings,
            PassOutputRegistry, PassOutputSpec, TypedExpr,
        },
        utils::{as_bytes, as_bytes_slice, load_image_from_data_url},
        utils::{coerce_to_type, cpu_num_f32, cpu_num_f32_min_0, cpu_num_u32_min_1},
        wgsl::{
            ERROR_SHADER_WGSL, build_blur_image_wgsl_bundle, build_downsample_bundle,
            build_horizontal_blur_bundle, build_pass_wgsl_bundle, build_upsample_bilinear_bundle,
            build_vertical_blur_bundle, clamp_min_1, gaussian_kernel_8,
            gaussian_mip_level_and_sigma_p,
        },
    },
};

fn mat4_mul(a: [f32; 16], b: [f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for r in 0..4 {
        for c in 0..4 {
            out[r * 4 + c] = a[r * 4 + 0] * b[0 * 4 + c]
                + a[r * 4 + 1] * b[1 * 4 + c]
                + a[r * 4 + 2] * b[2 * 4 + c]
                + a[r * 4 + 3] * b[3 * 4 + c];
        }
    }
    out
}

fn mat4_translate(tx: f32, ty: f32, tz: f32) -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, tx, ty, tz, 1.0,
    ]
}

fn mat4_scale(sx: f32, sy: f32, sz: f32) -> [f32; 16] {
    [
        sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, sz, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn mat4_rotate_z(rad: f32) -> [f32; 16] {
    let c = rad.cos();
    let s = rad.sin();
    [
        c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn compute_transform_geometry_matrix(
    _scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, crate::dsl::Node>,
    node: &crate::dsl::Node,
) -> Result<[f32; 16]> {
    // T * Rz * S for now.
    let mut tx = 0.0f32;
    let mut ty = 0.0f32;
    let mut tz = 0.0f32;

    // Inline params (Components mode)
    if let Some(t) = node.params.get("translate") {
        if let Some(obj) = t.as_object() {
            tx = obj.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            ty = obj.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            tz = obj.get("z").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        }
    }

    let mut sx = 1.0f32;
    let mut sy = 1.0f32;
    let mut sz = 1.0f32;
    if let Some(s) = node.params.get("scale") {
        if let Some(obj) = s.as_object() {
            sx = obj.get("x").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
            sy = obj.get("y").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
            sz = obj.get("z").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
        }
    }

    let mut rz = 0.0f32;
    if let Some(r) = node.params.get("rotate") {
        if let Some(obj) = r.as_object() {
            rz = obj.get("z").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        }
    }

    Ok(mat4_mul(
        mat4_translate(tx, ty, tz),
        mat4_mul(mat4_rotate_z(rz), mat4_scale(sx, sy, sz)),
    ))
}

pub(crate) fn resolve_geometry_for_render_pass(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    ids: &HashMap<String, ResourceName>,
    geometry_node_id: &str,
    material_ctx: Option<&MaterialCompileContext>,
) -> Result<(
    ResourceName,
    f32,
    f32,
    f32,
    f32,
    u32,
    [f32; 16],
    Option<TypedExpr>,
    Vec<String>,
    String,
    bool,
)> {
    let geometry_node = find_node(nodes_by_id, geometry_node_id)?;

    match geometry_node.node_type.as_str() {
        "Rect2DGeometry" => {
            let geometry_buffer = ids
                .get(geometry_node_id)
                .cloned()
                .ok_or_else(|| anyhow!("missing name for node: {}", geometry_node_id))?;

            let geo_w_u = cpu_num_u32_min_1(scene, nodes_by_id, geometry_node, "width", 100)?;
            let geo_h_u = cpu_num_u32_min_1(scene, nodes_by_id, geometry_node, "height", geo_w_u)?;
            let geo_w = geo_w_u as f32;
            let geo_h = geo_h_u as f32;
            let geo_x = cpu_num_f32(scene, nodes_by_id, geometry_node, "x", 0.0)?;
            let geo_y = cpu_num_f32(scene, nodes_by_id, geometry_node, "y", 0.0)?;
            Ok((
                geometry_buffer,
                geo_w,
                geo_h,
                geo_x,
                geo_y,
                1,
                [
                    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                ],
                None,
                Vec::new(),
                String::new(),
                false,
            ))
        }
        "InstancedGeometryStart" => {
            // Treat start as a passthrough wrapper for geometry resolution.
            // The instancing count is finalized at InstancedGeometryEnd.
            let upstream_geo_id = incoming_connection(scene, geometry_node_id, "base")
                .or_else(|| incoming_connection(scene, geometry_node_id, "geometry"))
                .map(|c| c.from.node_id.clone())
                .ok_or_else(|| {
                    anyhow!("InstancedGeometryStart.base missing for {geometry_node_id}")
                })?;

            let (
                buf,
                w,
                h,
                x,
                y,
                _instances,
                base_m,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                uses_instance_index,
            ) = resolve_geometry_for_render_pass(
                scene,
                nodes_by_id,
                ids,
                &upstream_geo_id,
                material_ctx,
            )?;

            let count_u = cpu_num_u32_min_1(scene, nodes_by_id, geometry_node, "count", 1)?;
            Ok((
                buf,
                w,
                h,
                x,
                y,
                count_u,
                base_m,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                uses_instance_index,
            ))
        }
        "InstancedGeometryEnd" => {
            let upstream_geo_id = incoming_connection(scene, geometry_node_id, "geometry")
                .map(|c| c.from.node_id.clone())
                .ok_or_else(|| {
                    anyhow!("InstancedGeometryEnd.geometry missing for {geometry_node_id}")
                })?;

            let (
                buf,
                w,
                h,
                x,
                y,
                _instances,
                base_m,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                uses_instance_index,
            ) = resolve_geometry_for_render_pass(
                scene,
                nodes_by_id,
                ids,
                &upstream_geo_id,
                material_ctx,
            )?;

            // Find InstancedGeometryStart by matching zoneId.
            let zone_id = geometry_node
                .params
                .get("zoneId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if zone_id.trim().is_empty() {
                bail!("InstancedGeometryEnd.zoneId missing for {geometry_node_id}");
            }

            let start = nodes_by_id
                .values()
                .find(|n| {
                    n.node_type == "InstancedGeometryStart"
                        && n.params.get("zoneId").and_then(|v| v.as_str()) == Some(zone_id)
                })
                .ok_or_else(|| {
                    anyhow!("InstancedGeometryStart with zoneId '{zone_id}' not found")
                })?;

            let count_u = cpu_num_u32_min_1(scene, nodes_by_id, start, "count", 1)?;

            Ok((
                buf,
                w,
                h,
                x,
                y,
                count_u,
                base_m,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                uses_instance_index,
            ))
        }
        "TransformGeometry" => {
            // Geometry chain: TransformGeometry.geometry -> base geometry buffer.
            // NOTE: GPU-side transforms are applied in the vertex shader (Params.geo_translate / Params.geo_scale).
            // Here we only adjust the *logical* geo_size for UV/GeoFragCoord correctness (per user request).
            let upstream_geo_id = incoming_connection(scene, geometry_node_id, "geometry")
                .map(|c| c.from.node_id.clone())
                .ok_or_else(|| {
                    anyhow!("TransformGeometry.geometry missing for {geometry_node_id}")
                })?;

            let (
                buf,
                mut w,
                mut h,
                x,
                y,
                instances,
                base_m,
                upstream_translate_expr,
                mut vtx_inline_stmts,
                mut vtx_wgsl_decls,
                mut uses_instance_index,
            ) = resolve_geometry_for_render_pass(
                scene,
                nodes_by_id,
                ids,
                &upstream_geo_id,
                material_ctx,
            )?;

            // Scale affects geo_size for correct UV + GeoFragcoord mapping.
            // We only support inline scale params here (object {x,y,z}); connected scale inputs are GPU-side.
            if let Some(s) = geometry_node.params.get("scale") {
                if let Some(obj) = s.as_object() {
                    if let Some(vx) = obj.get("x").and_then(|v| v.as_f64()) {
                        w *= vx as f32;
                    }
                    if let Some(vy) = obj.get("y").and_then(|v| v.as_f64()) {
                        h *= vy as f32;
                    }
                }
            }

            // For now, CPU-side matrix composition only supports the inline params.
            // Connected inputs (e.g. MathClosure driven by Index) are compiled in the vertex stage.
            let delta_m = compute_transform_geometry_matrix(scene, nodes_by_id, geometry_node)?;

            // Vertex-stage translate overrides upstream translate (for now).
            // Later we can support composition by emitting `upstream + local`.
            let mut translate_expr = upstream_translate_expr;
            let mut local_inline_stmts: Vec<String> = Vec::new();
            let mut local_wgsl_decls = String::new();
            let mut local_uses_instance_index = false;

            if let Some(conn) = incoming_connection(scene, &geometry_node.id, "translate") {
                // Compile upstream expression; it must evaluate to vec3.
                let src_node = find_node(nodes_by_id, &conn.from.node_id)?;
                match src_node.node_type.as_str() {
                    // Allow any node that the vertex compiler supports (MathClosure, Index, math nodes, inputs).
                    _ => {
                        let mut vtx_ctx = MaterialCompileContext {
                            baked_data_parse: material_ctx.and_then(|c| c.baked_data_parse.clone()),
                            baked_data_parse_meta: material_ctx
                                .and_then(|c| c.baked_data_parse_meta.clone()),
                            ..Default::default()
                        };
                        let mut cache: HashMap<(String, String), TypedExpr> = HashMap::new();

                        let expr = compile_vertex_expr(
                            scene,
                            nodes_by_id,
                            &conn.from.node_id,
                            Some(&conn.from.port_id),
                            &mut vtx_ctx,
                            &mut cache,
                        )?;
                        let expr = coerce_to_type(expr, ValueType::Vec3)?;
                        // Ensure any needed inline statements (e.g. MathClosure output var) are
                        // emitted in the vertex shader by forcing the expression through the
                        // compiler's inline statement machinery.
                        // NOTE: `expr.expr` may reference compiler-emitted locals (e.g. MathClosure
                        // output var). We must also propagate the corresponding inline statements
                        // into the render pass shader generation.
                        local_inline_stmts = vtx_ctx.inline_stmts.clone();
                        local_wgsl_decls = vtx_ctx.wgsl_decls();
                        local_uses_instance_index = vtx_ctx.uses_instance_index;
                        translate_expr = Some(expr.clone());
                    }
                }
            }

            if !local_inline_stmts.is_empty() {
                vtx_inline_stmts = local_inline_stmts;
                vtx_wgsl_decls = local_wgsl_decls;
            }
            if local_uses_instance_index {
                uses_instance_index = true;
            }

            Ok((
                buf,
                w,
                h,
                x,
                y,
                instances,
                mat4_mul(base_m, delta_m),
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                uses_instance_index,
            ))
        }
        other => {
            bail!(
                "RenderPass.geometry must resolve to Rect2DGeometry/TransformGeometry/InstancedGeometryEnd, got {other}"
            )
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn render_scene_to_png_headless(
    scene: &SceneDSL,
    output_path: impl AsRef<std::path::Path>,
) -> Result<()> {
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default())
        .map_err(|e| anyhow!("failed to create headless renderer: {e}"))?;

    let (shader_space, _resolution, output_texture_name, _passes) =
        build_shader_space_from_scene(scene, renderer.device.clone(), renderer.queue.clone())?;

    shader_space.render();
    shader_space
        .save_texture_png(output_texture_name.as_str(), output_path)
        .map_err(|e| anyhow!("failed to save png: {e}"))?;
    Ok(())
}

fn sampled_pass_node_ids(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
) -> HashSet<String> {
    // Any pass connected into PassTexture.pass is considered "sampled" and must have a resolvable output texture.
    let mut out: HashSet<String> = HashSet::new();
    for n in nodes_by_id.values() {
        if n.node_type != "PassTexture" {
            continue;
        }
        if let Some(conn) = incoming_connection(scene, &n.id, "pass") {
            out.insert(conn.from.node_id.clone());
        }
    }
    out
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
    LinearMirror,
}

#[derive(Clone, Debug)]
struct PassTextureBinding {
    /// ResourceName of the texture to bind.
    texture: ResourceName,
    /// If this binding refers to an ImageTexture node id, keep it here so the loader knows
    /// it must provide CPU image bytes.
    image_node_id: Option<String>,
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
}

#[derive(Clone)]
struct RenderPassSpec {
    name: ResourceName,
    geometry_buffer: ResourceName,
    instance_buffer: Option<ResourceName>,
    target_texture: ResourceName,
    params_buffer: ResourceName,
    baked_data_parse_buffer: Option<ResourceName>,
    params: Params,
    shader_wgsl: String,
    texture_bindings: Vec<PassTextureBinding>,
    sampler_kind: SamplerKind,
    blend_state: BlendState,
    color_load_op: wgpu::LoadOp<Color>,
}

fn normalize_blend_token(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace('_', "-")
}

fn parse_blend_operation(op: &str) -> Result<wgpu::BlendOperation> {
    let op = normalize_blend_token(op);
    Ok(match op.as_str() {
        "add" => wgpu::BlendOperation::Add,
        "subtract" => wgpu::BlendOperation::Subtract,
        "reverse-subtract" | "rev-subtract" => wgpu::BlendOperation::ReverseSubtract,
        "min" => wgpu::BlendOperation::Min,
        "max" => wgpu::BlendOperation::Max,
        other => bail!("unsupported blendfunc/blend operation: {other}"),
    })
}

fn parse_blend_factor(f: &str) -> Result<wgpu::BlendFactor> {
    let f = normalize_blend_token(f);
    Ok(match f.as_str() {
        "zero" => wgpu::BlendFactor::Zero,
        "one" => wgpu::BlendFactor::One,

        "src" | "src-color" => wgpu::BlendFactor::Src,
        "one-minus-src" | "one-minus-src-color" => wgpu::BlendFactor::OneMinusSrc,

        "src-alpha" => wgpu::BlendFactor::SrcAlpha,
        "one-minus-src-alpha" => wgpu::BlendFactor::OneMinusSrcAlpha,

        "dst" | "dst-color" => wgpu::BlendFactor::Dst,
        "one-minus-dst" | "one-minus-dst-color" => wgpu::BlendFactor::OneMinusDst,

        "dst-alpha" => wgpu::BlendFactor::DstAlpha,
        "one-minus-dst-alpha" => wgpu::BlendFactor::OneMinusDstAlpha,

        "src-alpha-saturated" => wgpu::BlendFactor::SrcAlphaSaturated,
        "constant" | "blend-color" => wgpu::BlendFactor::Constant,
        "one-minus-constant" | "one-minus-blend-color" => wgpu::BlendFactor::OneMinusConstant,
        other => bail!("unsupported blend factor: {other}"),
    })
}

fn default_blend_state_for_preset(preset: &str) -> Result<BlendState> {
    let preset = normalize_blend_token(preset);
    Ok(match preset.as_str() {
        "alpha" => BlendState {
            color: wgpu::BlendComponent {
                // Premultiplied alpha: RGB is assumed multiplied by A.
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        },
        "add" | "additive" => BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        },
        "opaque" | "none" | "off" | "replace" => BlendState::REPLACE,
        // "custom" means: start from a neutral blend state and let explicit
        // blendfunc/src/dst overrides drive the final state.
        "custom" => BlendState::REPLACE,
        other => bail!("unsupported blend_preset: {other}"),
    })
}

fn parse_render_pass_blend_state(
    params: &HashMap<String, serde_json::Value>,
) -> Result<BlendState> {
    // Start with preset if present; otherwise default to REPLACE.
    // Note: RenderPass has scheme defaults for blendfunc/factors. If a user sets only
    // `blend_preset=replace` (common intent: disable blending), those default factor keys will
    // still exist in params after default-merging. We must treat replace/off/none/opaque as
    // authoritative and ignore factor overrides.
    if let Some(preset) = parse_str(params, "blend_preset") {
        let preset_norm = normalize_blend_token(preset);
        if matches!(preset_norm.as_str(), "opaque" | "none" | "off" | "replace") {
            return Ok(BlendState::REPLACE);
        }
    }

    let mut state = if let Some(preset) = parse_str(params, "blend_preset") {
        default_blend_state_for_preset(preset)?
    } else {
        BlendState::REPLACE
    };

    // Override with explicit params if present.
    if let Some(op) = parse_str(params, "blendfunc") {
        let op = parse_blend_operation(op)?;
        state.color.operation = op;
        state.alpha.operation = op;
    }
    if let Some(src) = parse_str(params, "src_factor") {
        state.color.src_factor = parse_blend_factor(src)?;
    }
    if let Some(dst) = parse_str(params, "dst_factor") {
        state.color.dst_factor = parse_blend_factor(dst)?;
    }
    if let Some(src) = parse_str(params, "src_alpha_factor") {
        state.alpha.src_factor = parse_blend_factor(src)?;
    }
    if let Some(dst) = parse_str(params, "dst_alpha_factor") {
        state.alpha.dst_factor = parse_blend_factor(dst)?;
    }

    Ok(state)
}

fn premultiply_rgba8(image: Arc<DynamicImage>) -> Arc<DynamicImage> {
    // Convert to premultiplied alpha in-place (RGBA8).
    let mut rgba = image.as_ref().to_rgba8();
    for p in rgba.pixels_mut() {
        let a = p.0[3] as u16;
        p.0[0] = ((p.0[0] as u16 * a) / 255) as u8;
        p.0[1] = ((p.0[1] as u16 * a) / 255) as u8;
        p.0[2] = ((p.0[2] as u16 * a) / 255) as u8;
    }
    Arc::new(DynamicImage::ImageRgba8(rgba))
}

fn flip_image_y_rgba8(image: Arc<DynamicImage>) -> Arc<DynamicImage> {
    // The renderer's UV convention is bottom-left origin (GL-like).
    // Most image sources are top-left origin, so we flip pixels once on upload.
    let mut rgba = image.as_ref().to_rgba8();
    image::imageops::flip_vertical_in_place(&mut rgba);
    Arc::new(DynamicImage::ImageRgba8(rgba))
}

pub fn build_shader_space_from_scene(
    scene: &SceneDSL,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
) -> Result<(ShaderSpace, [u32; 2], ResourceName, Vec<PassBindings>)> {
    let prepared = prepare_scene(scene)?;
    let resolution = prepared.resolution;
    let nodes_by_id = &prepared.nodes_by_id;
    let ids = &prepared.ids;
    let output_texture_node_id = &prepared.output_texture_node_id;
    let output_texture_name = prepared.output_texture_name.clone();
    let composite_layers_in_order = &prepared.composite_layers_in_draw_order;
    let order = &prepared.topo_order;

    let mut geometry_buffers: Vec<(ResourceName, Arc<[u8]>)> = Vec::new();
    let mut instance_buffers: Vec<(ResourceName, Arc<[u8]>)> = Vec::new();
    let mut textures: Vec<TextureDecl> = Vec::new();
    let mut render_pass_specs: Vec<RenderPassSpec> = Vec::new();
    let mut baked_data_parse_meta_by_pass: HashMap<String, Arc<BakedDataParseMeta>> =
        HashMap::new();
    let mut baked_data_parse_bytes_by_pass: HashMap<String, Arc<[u8]>> = HashMap::new();
    let mut composite_passes: Vec<ResourceName> = Vec::new();

    // Pass nodes that are sampled via PassTexture must have a dedicated output texture.
    let sampled_pass_ids = sampled_pass_node_ids(&prepared.scene, nodes_by_id);

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
                let geo_w_u = cpu_num_u32_min_1(&prepared.scene, nodes_by_id, node, "width", 100)?;
                let geo_h_u =
                    cpu_num_u32_min_1(&prepared.scene, nodes_by_id, node, "height", geo_w_u)?;
                let geo_w = geo_w_u as f32;
                let geo_h = geo_h_u as f32;
                let verts = rect2d_geometry_vertices(geo_w, geo_h);
                let bytes: Arc<[u8]> = Arc::from(as_bytes_slice(&verts).to_vec());
                geometry_buffers.push((name, bytes));
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

    // Track pass outputs for chain resolution.
    let mut pass_output_registry = PassOutputRegistry::new();
    let format = parse_texture_format(&target_node.params)?;

    // Composite draw order only contains direct inputs. For chained passes, we must render
    // upstream pass dependencies first so PassTexture can resolve them.
    let pass_nodes_in_order =
        compute_pass_render_order(&prepared.scene, nodes_by_id, composite_layers_in_order)?;

    for layer_id in &pass_nodes_in_order {
        let layer_node = find_node(&nodes_by_id, layer_id)?;
        match layer_node.node_type.as_str() {
            "RenderPass" => {
                let pass_name = ids
                    .get(layer_id)
                    .cloned()
                    .ok_or_else(|| anyhow!("missing name for node: {layer_id}"))?;

                // If this pass is sampled downstream (PassTexture), render into a dedicated intermediate texture.
                // This avoids aliasing the final output and gives PassTexture a stable source.
                let is_sampled_output = sampled_pass_ids.contains(layer_id);
                let pass_output_texture: ResourceName = if is_sampled_output {
                    let out_tex: ResourceName = format!("{layer_id}__out").into();
                    textures.push(TextureDecl {
                        name: out_tex.clone(),
                        size: [tgt_w as u32, tgt_h as u32],
                        format,
                    });
                    out_tex
                } else {
                    target_texture_name.clone()
                };

                let blend_state = parse_render_pass_blend_state(&layer_node.params)?;

                let render_geo_node_id = incoming_connection(&prepared.scene, layer_id, "geometry")
                    .map(|c| c.from.node_id.clone())
                    .ok_or_else(|| anyhow!("RenderPass.geometry missing for {layer_id}"))?;

                let (
                    geometry_buffer,
                    geo_w,
                    geo_h,
                    geo_x,
                    geo_y,
                    instance_count,
                    base_m,
                    translate_expr,
                    vertex_inline_stmts,
                    vertex_wgsl_decls,
                    vertex_uses_instance_index,
                ) = resolve_geometry_for_render_pass(
                    &prepared.scene,
                    nodes_by_id,
                    ids,
                    &render_geo_node_id,
                    None,
                )?;

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
                    _base_m_2,
                    _translate_expr,
                    _vertex_inline_stmts,
                    _vertex_wgsl_decls,
                    _vertex_uses_instance_index,
                ) = resolve_geometry_for_render_pass(
                    &prepared.scene,
                    nodes_by_id,
                    ids,
                    &render_geo_node_id,
                    Some(&MaterialCompileContext {
                        baked_data_parse: Some(std::sync::Arc::new(baked.clone())),
                        baked_data_parse_meta: baked_data_parse_meta_by_pass.get(layer_id).cloned(),
                        ..Default::default()
                    }),
                )?;

                let params_name: ResourceName = format!("params_{layer_id}").into();
                let params = Params {
                    target_size: [tgt_w, tgt_h],
                    geo_size: [geo_w.max(1.0), geo_h.max(1.0)],
                    center: [geo_x, geo_y],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.9, 0.2, 0.2, 1.0],
                };

                let is_instanced = instance_count > 1;

                let bundle = build_pass_wgsl_bundle(
                    &prepared.scene,
                    nodes_by_id,
                    Some(std::sync::Arc::new(baked)),
                    baked_data_parse_meta_by_pass.get(layer_id).cloned(),
                    layer_id,
                    is_instanced,
                    translate_expr.map(|e| e.expr),
                    vertex_inline_stmts,
                    vertex_wgsl_decls,
                    vertex_uses_instance_index,
                )?;
                let shader_wgsl = bundle.module;

                let mut texture_bindings: Vec<PassTextureBinding> = bundle
                    .image_textures
                    .iter()
                    .filter_map(|id| {
                        ids.get(id).cloned().map(|tex| PassTextureBinding {
                            texture: tex,
                            image_node_id: Some(id.clone()),
                        })
                    })
                    .collect();

                texture_bindings.extend(resolve_pass_texture_bindings(
                    &pass_output_registry,
                    &bundle.pass_textures,
                )?);

                let instance_buffer = if is_instanced {
                    let b: ResourceName = format!("{layer_id}__instances").into();

                    // Per-instance mat4 (column-major) as 4 vec4<f32> (16 floats).
                    // This is the accumulated base transform through the geometry chain.
                    // Any per-instance variation is computed in the vertex shader.
                    let mut mats: Vec<[f32; 16]> = Vec::with_capacity(instance_count as usize);
                    for _ in 0..instance_count {
                        mats.push(base_m);
                    }
                    let bytes: Arc<[u8]> = Arc::from(as_bytes_slice(&mats).to_vec());
                    instance_buffers.push((b.clone(), bytes));

                    Some(b)
                } else {
                    None
                };

                let baked_data_parse_buffer: Option<ResourceName> = if keys.is_empty() {
                    None
                } else {
                    Some(format!("{layer_id}__baked_data_parse").into())
                };

                render_pass_specs.push(RenderPassSpec {
                    name: pass_name.clone(),
                    geometry_buffer,
                    instance_buffer,
                    target_texture: pass_output_texture.clone(),
                    params_buffer: params_name,
                    baked_data_parse_buffer,
                    params,
                    shader_wgsl,
                    texture_bindings,
                    sampler_kind: SamplerKind::NearestClamp,
                    blend_state,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                });
                composite_passes.push(pass_name);

                // Register output so downstream PassTexture nodes can resolve it.
                pass_output_registry.register(PassOutputSpec {
                    node_id: layer_id.clone(),
                    texture_name: pass_output_texture,
                    resolution: [tgt_w as u32, tgt_h as u32],
                    format,
                });
            }
            "GuassianBlurPass" => {
                // GuassianBlurPass takes its source from `image` input (color type).
                // This can be from PassTexture (sampling another pass), ImageTexture, or any color expression.
                // We first render the image expression to an intermediate texture, then apply the blur chain.

                // Create source texture for the image input.
                let src_tex: ResourceName = format!("{layer_id}__src").into();
                let src_resolution = [tgt_w as u32, tgt_h as u32];
                textures.push(TextureDecl {
                    name: src_tex.clone(),
                    size: src_resolution,
                    format,
                });

                // Build a fullscreen pass to render the `image` input expression.
                let geo_src: ResourceName = format!("{layer_id}__geo_src").into();
                geometry_buffers.push((geo_src.clone(), make_fullscreen_geometry(tgt_w, tgt_h)));

                let params_src: ResourceName = format!("params_{layer_id}__src").into();
                let params_src_val = Params {
                    target_size: [tgt_w, tgt_h],
                    geo_size: [tgt_w, tgt_h],
                    center: [0.0, 0.0],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                };

                // Build WGSL for the image input expression (similar to RenderPass material).
                let src_bundle =
                    build_blur_image_wgsl_bundle(&prepared.scene, nodes_by_id, layer_id)?;
                let mut src_texture_bindings: Vec<PassTextureBinding> = src_bundle
                    .image_textures
                    .iter()
                    .filter_map(|id| {
                        ids.get(id).cloned().map(|tex| PassTextureBinding {
                            texture: tex,
                            image_node_id: Some(id.clone()),
                        })
                    })
                    .collect();

                src_texture_bindings.extend(resolve_pass_texture_bindings(
                    &pass_output_registry,
                    &src_bundle.pass_textures,
                )?);

                render_pass_specs.push(RenderPassSpec {
                    name: format!("{layer_id}__src_pass").into(),
                    geometry_buffer: geo_src,
                    instance_buffer: None,
                    target_texture: src_tex.clone(),
                    params_buffer: params_src.clone(),
                    baked_data_parse_buffer: None,
                    params: params_src_val,
                    shader_wgsl: src_bundle.module,
                    texture_bindings: src_texture_bindings,
                    sampler_kind: SamplerKind::NearestClamp,
                    blend_state: BlendState::REPLACE,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                });
                composite_passes.push(format!("{layer_id}__src_pass").into());

                // Resolution: use target resolution, but allow override via params.
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

                let sigma =
                    cpu_num_f32_min_0(&prepared.scene, nodes_by_id, layer_node, "radius", 0.0)?;
                let (mip_level, sigma_p) = gaussian_mip_level_and_sigma_p(sigma);
                let downsample_factor: u32 = 1 << mip_level;
                let (kernel, offset, _num) = gaussian_kernel_8(sigma_p.max(1e-6));

                let downsample_steps: Vec<u32> = if downsample_factor == 16 {
                    vec![8, 2]
                } else {
                    vec![downsample_factor]
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
                    let tex: ResourceName = format!("{layer_id}__ds_{step}").into();
                    textures.push(TextureDecl {
                        name: tex.clone(),
                        size: [next_w, next_h],
                        format,
                    });
                    let geo: ResourceName = format!("{layer_id}__geo_ds_{step}").into();
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

                let h_tex: ResourceName = format!("{layer_id}__h_tex").into();
                let v_tex: ResourceName = format!("{layer_id}__v_tex").into();

                textures.push(TextureDecl {
                    name: h_tex.clone(),
                    size: [ds_w, ds_h],
                    format,
                });
                textures.push(TextureDecl {
                    name: v_tex.clone(),
                    size: [ds_w, ds_h],
                    format,
                });

                // If this blur pass is sampled downstream (PassTexture), render into an intermediate output.
                // Otherwise, render to the final Composite.target texture.
                let output_tex: ResourceName = if sampled_pass_ids.contains(layer_id) {
                    let out_tex: ResourceName = format!("{layer_id}__out").into();
                    textures.push(TextureDecl {
                        name: out_tex.clone(),
                        size: [blur_w, blur_h],
                        format,
                    });
                    out_tex
                } else {
                    target_texture_name.clone()
                };

                // When multiple layers render to the same Composite.target, we must blend the later
                // layers over the earlier result (otherwise the later layer overwrites and it looks
                // like only one draw contributed).
                //
                // - For sampled outputs (PassTexture), keep REPLACE for determinism.
                // - For final output, default to alpha blending, but allow explicit overrides via
                //   RenderPass-style blend params if present.
                let blur_output_blend_state: BlendState = if output_tex == target_texture_name {
                    let has_explicit_blend_params = [
                        "blend_preset",
                        "blendfunc",
                        "src_factor",
                        "dst_factor",
                        "src_alpha_factor",
                        "dst_alpha_factor",
                    ]
                    .into_iter()
                    .any(|k| layer_node.params.contains_key(k));

                    if has_explicit_blend_params {
                        parse_render_pass_blend_state(&layer_node.params)?
                    } else {
                        default_blend_state_for_preset("alpha")?
                    }
                } else {
                    BlendState::REPLACE
                };

                // Fullscreen geometry buffers for blur + upsample.
                let geo_ds: ResourceName = format!("{layer_id}__geo_ds").into();
                geometry_buffers.push((
                    geo_ds.clone(),
                    make_fullscreen_geometry(ds_w as f32, ds_h as f32),
                ));
                let geo_out: ResourceName = format!("{layer_id}__geo_out").into();
                geometry_buffers.push((
                    geo_out.clone(),
                    make_fullscreen_geometry(blur_w as f32, blur_h as f32),
                ));

                // Downsample chain
                let mut prev_tex: Option<ResourceName> = None;
                for (step, tex, step_w, step_h, step_geo) in &step_textures {
                    let params_name: ResourceName =
                        format!("params_{layer_id}__downsample_{step}").into();
                    let bundle = build_downsample_bundle(*step)?;

                    let params_val = Params {
                        target_size: [*step_w as f32, *step_h as f32],
                        geo_size: [*step_w as f32, *step_h as f32],
                        center: [0.0, 0.0],
                        geo_translate: [0.0, 0.0],
                        geo_scale: [1.0, 1.0],
                        time: 0.0,
                        _pad0: 0.0,
                        color: [0.0, 0.0, 0.0, 0.0],
                    };

                    let src_tex = match &prev_tex {
                        None => src_tex.clone(),
                        Some(t) => t.clone(),
                    };

                    render_pass_specs.push(RenderPassSpec {
                        name: format!("{layer_id}__downsample_{step}").into(),
                        geometry_buffer: step_geo.clone(),
                        instance_buffer: None,
                        target_texture: tex.clone(),
                        params_buffer: params_name,
                        baked_data_parse_buffer: Some(
                            format!("{layer_id}__baked_data_parse").into(),
                        ),
                        params: params_val,
                        shader_wgsl: bundle.module,
                        texture_bindings: vec![PassTextureBinding {
                            texture: src_tex,
                            image_node_id: None,
                        }],
                        sampler_kind: SamplerKind::NearestMirror,
                        blend_state: BlendState::REPLACE,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                    });
                    composite_passes.push(format!("{layer_id}__downsample_{step}").into());
                    prev_tex = Some(tex.clone());
                }

                let ds_src_tex: ResourceName = prev_tex
                    .ok_or_else(|| anyhow!("GuassianBlurPass: missing downsample output"))?;

                // 2) Horizontal blur: ds_src_tex -> h_tex
                let params_h: ResourceName =
                    format!("params_{layer_id}__hblur_ds{downsample_factor}").into();
                let bundle_h = build_horizontal_blur_bundle(kernel, offset);
                let params_h_val = Params {
                    target_size: [ds_w as f32, ds_h as f32],
                    geo_size: [ds_w as f32, ds_h as f32],
                    center: [0.0, 0.0],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                };
                render_pass_specs.push(RenderPassSpec {
                    name: format!("{layer_id}__hblur_ds{downsample_factor}").into(),
                    geometry_buffer: geo_ds.clone(),
                    instance_buffer: None,
                    target_texture: h_tex.clone(),
                    params_buffer: params_h.clone(),
                    baked_data_parse_buffer: None,
                    params: params_h_val,
                    shader_wgsl: bundle_h.module,
                    texture_bindings: vec![PassTextureBinding {
                        texture: ds_src_tex.clone(),
                        image_node_id: None,
                    }],
                    sampler_kind: SamplerKind::LinearMirror,
                    blend_state: BlendState::REPLACE,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                });
                composite_passes.push(format!("{layer_id}__hblur_ds{downsample_factor}").into());

                // 3) Vertical blur: h_tex -> v_tex (still downsampled resolution)
                let params_v: ResourceName =
                    format!("params_{layer_id}__vblur_ds{downsample_factor}").into();
                let bundle_v = build_vertical_blur_bundle(kernel, offset);
                let params_v_val = Params {
                    target_size: [ds_w as f32, ds_h as f32],
                    geo_size: [ds_w as f32, ds_h as f32],
                    center: [0.0, 0.0],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                };
                render_pass_specs.push(RenderPassSpec {
                    name: format!("{layer_id}__vblur_ds{downsample_factor}").into(),
                    geometry_buffer: geo_ds.clone(),
                    instance_buffer: None,
                    target_texture: v_tex.clone(),
                    params_buffer: params_v.clone(),
                    baked_data_parse_buffer: None,
                    params: params_v_val,
                    shader_wgsl: bundle_v.module,
                    texture_bindings: vec![PassTextureBinding {
                        texture: h_tex.clone(),
                        image_node_id: None,
                    }],
                    sampler_kind: SamplerKind::LinearMirror,
                    blend_state: BlendState::REPLACE,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                });

                composite_passes.push(format!("{layer_id}__vblur_ds{downsample_factor}").into());

                // 4) Upsample bilinear back to output: v_tex -> output_tex
                let params_u: ResourceName =
                    format!("params_{layer_id}__upsample_bilinear_ds{downsample_factor}").into();
                let bundle_u = build_upsample_bilinear_bundle();
                let params_u_val = Params {
                    target_size: [blur_w as f32, blur_h as f32],
                    geo_size: [blur_w as f32, blur_h as f32],
                    center: [0.0, 0.0],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                };
                render_pass_specs.push(RenderPassSpec {
                    name: format!("{layer_id}__upsample_bilinear_ds{downsample_factor}").into(),
                    geometry_buffer: geo_out.clone(),
                    instance_buffer: None,
                    target_texture: output_tex.clone(),
                    params_buffer: params_u.clone(),
                    baked_data_parse_buffer: None,
                    params: params_u_val,
                    shader_wgsl: bundle_u.module,
                    texture_bindings: vec![PassTextureBinding {
                        texture: v_tex.clone(),
                        image_node_id: None,
                    }],
                    sampler_kind: SamplerKind::LinearMirror,
                    blend_state: blur_output_blend_state,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                });

                composite_passes
                    .push(format!("{layer_id}__upsample_bilinear_ds{downsample_factor}").into());

                // Register this GuassianBlurPass output for potential downstream chaining.
                pass_output_registry.register(PassOutputSpec {
                    node_id: layer_id.clone(),
                    texture_name: output_tex,
                    resolution: [blur_w, blur_h],
                    format,
                });
            }
            other => {
                // To add support for new pass types:
                // 1. Add the type to is_pass_node() function
                // 2. Add a match arm here with the rendering logic
                // 3. Register the output in pass_output_registry for chain support
                bail!(
                    "Composite layer must be a pass node (RenderPass/GuassianBlurPass), got {other} for {layer_id}. \
                     To enable chain support for new pass types, update is_pass_node() and add handling here."
                )
            }
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
            params_buffer: s.params_buffer.clone(),
            base_params: s.params,
        })
        .collect();

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
    }

    for spec in &render_pass_specs {
        let Some(name) = spec.baked_data_parse_buffer.clone() else {
            continue;
        };
        let pass_id = name
            .as_str()
            .trim_end_matches("__baked_data_parse")
            .to_string();
        let contents = baked_data_parse_bytes_by_pass
            .get(&pass_id)
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
            usage: TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_SRC,
        })
        .collect();

    // ImageTexture resources (sampled textures) referenced by any reachable RenderPass.
    fn placeholder_image() -> Arc<DynamicImage> {
        let img = RgbaImage::from_pixel(1, 1, Rgba([255, 0, 255, 255]));
        Arc::new(DynamicImage::ImageRgba8(img))
    }

    fn load_image_with_fallback(rel_base: &PathBuf, path: Option<&str>) -> Arc<DynamicImage> {
        let Some(p) = path.filter(|s| !s.trim().is_empty()) else {
            return placeholder_image();
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

        for cand in candidates {
            if let Ok(img) = image::open(&cand) {
                return Arc::new(img);
            }
        }
        placeholder_image()
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

            // Prefer inlined data URL (data:image/...;base64,...) if present.
            // Fallback to file path lookup.
            let data_url = node
                .params
                .get("dataUrl")
                .and_then(|v| v.as_str())
                .or_else(|| node.params.get("data_url").and_then(|v| v.as_str()));

            let image = match data_url {
                Some(s) if !s.trim().is_empty() => match load_image_from_data_url(s) {
                    Ok(img) => premultiply_rgba8(flip_image_y_rgba8(ensure_rgba8(Arc::new(img)))),
                    Err(_e) => placeholder_image(),
                },
                _ => {
                    let path = node.params.get("path").and_then(|v| v.as_str());
                    premultiply_rgba8(flip_image_y_rgba8(ensure_rgba8(load_image_with_fallback(
                        &rel_base, path,
                    ))))
                }
            };

            let name = ids
                .get(node_id)
                .cloned()
                .ok_or_else(|| anyhow!("missing name for node: {node_id}"))?;

            texture_specs.push(FiberTextureSpec::Image {
                name,
                image,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            });
        }
    }

    shader_space.declare_textures(texture_specs);

    // 3) Samplers
    let nearest_sampler: ResourceName = "sampler_nearest".into();
    let nearest_mirror_sampler: ResourceName = "sampler_nearest_mirror".into();
    let linear_mirror_sampler: ResourceName = "sampler_linear_mirror".into();
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
    ]);

    for spec in &render_pass_specs {
        let geometry_buffer = spec.geometry_buffer.clone();
        let target_texture = spec.target_texture.clone();
        let params_buffer = spec.params_buffer.clone();
        let shader_wgsl = spec.shader_wgsl.clone();
        let blend_state = spec.blend_state;
        let color_load_op = spec.color_load_op;

        let texture_names: Vec<ResourceName> = spec
            .texture_bindings
            .iter()
            .map(|b| b.texture.clone())
            .collect();
        let sampler_name = match spec.sampler_kind {
            SamplerKind::NearestClamp => nearest_sampler.clone(),
            SamplerKind::NearestMirror => nearest_mirror_sampler.clone(),
            SamplerKind::LinearMirror => linear_mirror_sampler.clone(),
        };

        // When shader compilation fails (wgpu create_shader_module), the error message can be
        // hard to correlate back to the generated WGSL. Dump it to a predictable temp location
        // so tests can inspect the exact module wgpu validated.
        let debug_dump_path = format!("/tmp/node-forge-pass__{}.wgsl", spec.name.as_str());
        let shader_desc: wgpu::ShaderModuleDescriptor<'static> = wgpu::ShaderModuleDescriptor {
            label: Some("node-forge-pass"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(shader_wgsl.clone())),
        };
        let _ = std::fs::write(&debug_dump_path, &shader_wgsl);
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

            for (i, tex_name) in texture_names.iter().enumerate() {
                let tex_binding = (i as u32) * 2;
                let samp_binding = tex_binding + 1;
                b = b
                    .bind_texture(1, tex_binding, tex_name.clone(), ShaderStages::FRAGMENT)
                    .bind_sampler(
                        1,
                        samp_binding,
                        sampler_name.clone(),
                        ShaderStages::FRAGMENT,
                    );
            }

            b.bind_color_attachment(target_texture)
                .blending(blend_state)
                .load_op(color_load_op)
        });
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
    }

    Ok((shader_space, resolution, output_texture_name, pass_bindings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::Node;
    use crate::renderer::scene_prep::composite_layers_in_draw_order;
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

        let got = parse_render_pass_blend_state(&params).unwrap();
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
        let got = parse_render_pass_blend_state(&params).unwrap();
        let expected = default_blend_state_for_preset("alpha").unwrap();
        assert_eq!(format!("{got:?}"), format!("{expected:?}"));
    }

    #[test]
    fn render_pass_blend_state_defaults_to_replace() {
        let params: HashMap<String, serde_json::Value> = HashMap::new();
        let got = parse_render_pass_blend_state(&params).unwrap();
        assert_eq!(format!("{got:?}"), format!("{:?}", BlendState::REPLACE));
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
    fn sampled_pass_ids_detect_renderpass_used_by_pass_texture() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let scene_path = manifest_dir.join("tests/cases/pass-texture-alpha/scene.json");
        let scene = crate::dsl::load_scene_from_path(&scene_path).expect("load scene");
        let prepared = prepare_scene(&scene).expect("prepare scene");

        let sampled = sampled_pass_node_ids(&prepared.scene, &prepared.nodes_by_id);
        assert!(
            sampled.contains("pass_up"),
            "expected sampled passes to include pass_up, got: {sampled:?}"
        );
    }
}

pub fn build_error_shader_space(
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    resolution: [u32; 2],
) -> Result<(ShaderSpace, [u32; 2], ResourceName, Vec<PassBindings>)> {
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

    Ok((shader_space, resolution, output_texture_name, Vec::new()))
}
