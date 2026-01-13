//! ShaderSpace construction module.
//!
//! This module contains logic for building ShaderSpace instances from DSL scenes,
//! including texture creation, geometry buffers, uniform buffers, pipelines, and
//! composite layer handling.

use std::{borrow::Cow, collections::{HashMap, HashSet}, path::PathBuf, sync::Arc};

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use image::{DynamicImage, Rgba, RgbaImage};
use rust_wgpu_fiber::{
    eframe::wgpu::{
        self, vertex_attr_array, BlendState, Color, ShaderStages, TextureFormat, TextureUsages,
    },
    pool::{
        buffer_pool::BufferSpec,
        sampler_pool::SamplerSpec,
        texture_pool::TextureSpec as FiberTextureSpec,
    },
    shader_space::{ShaderSpace, ShaderSpaceResult},
    ResourceName,
};

use crate::{
    dsl::{
        find_node, incoming_connection, parse_f32, parse_str, parse_texture_format, parse_u32,
        Connection, Endpoint, Node, SceneDSL,
    },
    graph::{topo_sort, upstream_reachable},
    schema,
    renderer::{
        node_compiler::compile_material_expr,
        scene_prep::{PreparedScene, prepare_scene, composite_layers_in_draw_order},
        types::{ValueType, TypedExpr, MaterialCompileContext, Params, PassBindings, WgslShaderBundle},
        utils::to_vec4_color,
        wgsl::{
            array8_f32_wgsl,
            build_all_pass_wgsl_bundles_from_scene,
            build_fullscreen_textured_bundle,
            build_pass_wgsl_bundle,
            clamp_min_1,
            gaussian_kernel_8,
            gaussian_mip_level_and_sigma_p,
        },
    },
};

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



fn as_bytes<T>(v: &T) -> &[u8] {
    unsafe { core::slice::from_raw_parts((v as *const T) as *const u8, core::mem::size_of::<T>()) }
}

fn as_bytes_slice<T>(v: &[T]) -> &[u8] {
    unsafe {
        core::slice::from_raw_parts(v.as_ptr() as *const u8, core::mem::size_of::<T>() * v.len())
    }
}

fn percent_decode_to_bytes(s: &str) -> Result<Vec<u8>> {
    // Minimal percent-decoder for data URLs with non-base64 payloads.
    // (We keep it strict: invalid percent sequences error.)
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    bail!("invalid percent-encoding: truncated");
                }
                let hi = bytes[i + 1];
                let lo = bytes[i + 2];
                let hex = |b: u8| -> Option<u8> {
                    match b {
                        b'0'..=b'9' => Some(b - b'0'),
                        b'a'..=b'f' => Some(b - b'a' + 10),
                        b'A'..=b'F' => Some(b - b'A' + 10),
                        _ => None,
                    }
                };

                let hi = hex(hi).ok_or_else(|| anyhow!("invalid percent-encoding"))?;
                let lo = hex(lo).ok_or_else(|| anyhow!("invalid percent-encoding"))?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }

    Ok(out)
}

fn decode_data_url(data_url: &str) -> Result<Vec<u8>> {
    let s = data_url.trim();
    if !s.starts_with("data:") {
        bail!("not a data URL");
    }

    let (_, rest) = s.split_at("data:".len());
    let (meta, data) = rest
        .split_once(',')
        .ok_or_else(|| anyhow!("invalid data URL: missing comma"))?;

    let is_base64 = meta
        .split(';')
        .any(|t| t.trim().eq_ignore_ascii_case("base64"));

    if is_base64 {
        // Some producers use URL-safe base64; try both.
        general_purpose::STANDARD
            .decode(data.trim())
            .or_else(|_| general_purpose::URL_SAFE.decode(data.trim()))
            .map_err(|e| anyhow!("invalid base64 in data URL: {e}"))
    } else {
        percent_decode_to_bytes(data)
    }
}

fn load_image_from_data_url(data_url: &str) -> Result<DynamicImage> {
    let bytes = decode_data_url(data_url)?;
    image::load_from_memory(&bytes).map_err(|e| anyhow!("failed to decode image bytes: {e}"))
}

pub fn update_pass_params(
    shader_space: &ShaderSpace,
    pass: &PassBindings,
    params: &Params,
) -> ShaderSpaceResult<()> {
    shader_space.write_buffer(pass.params_buffer.as_str(), 0, as_bytes(params))?;
    Ok(())
}

fn rect2d_geometry_vertices(width: f32, height: f32) -> [[f32; 3]; 6] {
    let w = width.max(1.0);
    let h = height.max(1.0);
    let hw = w * 0.5;
    let hh = h * 0.5;
    [
        [-hw, -hh, 0.0],
        [hw, -hh, 0.0],
        [hw, hh, 0.0],
        [-hw, -hh, 0.0],
        [hw, hh, 0.0],
        [-hw, hh, 0.0],
    ]
}

fn composite_layers_in_draw_order(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    output_node_id: &str,
) -> Result<Vec<String>> {
    let output_node = find_node(nodes_by_id, output_node_id)?;
    if output_node.node_type != "Composite" {
        bail!("output node must be Composite, got {}", output_node.node_type);
    }

    // 1) base pass is always the base layer.
    let base_pass_id: String = incoming_connection(scene, output_node_id, "pass")
        .map(|c| c.from.node_id.clone())
        .ok_or_else(|| anyhow!("Composite.pass has no incoming connection"))?;

    // 2) dynamic layers follow Composite.inputs array order (only dynamic_* ports).
    // Note: the server does not infer ordering from port ids; it trusts the JSON ordering.
    let mut ordered: Vec<String> = Vec::new();
    ordered.push(base_pass_id);

    for port in &output_node.inputs {
        if !port.id.starts_with("dynamic_") {
            continue;
        }
        if let Some(conn) = incoming_connection(scene, output_node_id, &port.id) {
            let pass_id = conn.from.node_id.clone();
            if !ordered.contains(&pass_id) {
                ordered.push(pass_id);
            }
        }
    }

    for layer_id in &ordered {
        let node = find_node(nodes_by_id, layer_id)?;
        if node.node_type != "RenderPass" && node.node_type != "GuassianBlurPass" {
            bail!(
                "Composite inputs must come from RenderPass or GuassianBlurPass nodes, got {} for {layer_id}",
                node.node_type
            );
        }
    }

    Ok(ordered)
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
    target_texture: ResourceName,
    params_buffer: ResourceName,
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
                src_factor: wgpu::BlendFactor::SrcAlpha,
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
        other => bail!("unsupported blend_preset: {other}"),
    })
}

fn parse_render_pass_blend_state(params: &HashMap<String, serde_json::Value>) -> Result<BlendState> {
    // Start with preset if present; otherwise default to REPLACE.
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
    let mut textures: Vec<TextureDecl> = Vec::new();
    let mut render_pass_specs: Vec<RenderPassSpec> = Vec::new();
    let mut composite_passes: Vec<ResourceName> = Vec::new();

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
                let geo_w = parse_f32(&node.params, "width").unwrap_or(100.0).max(1.0);
                let geo_h = parse_f32(&node.params, "height").unwrap_or(geo_w).max(1.0);
                let verts = rect2d_geometry_vertices(geo_w, geo_h);
                let bytes: Arc<[u8]> = Arc::from(as_bytes_slice(&verts).to_vec());
                geometry_buffers.push((name, bytes));
            }
            "RenderTexture" => {
                let w = parse_u32(&node.params, "width").unwrap_or(resolution[0]);
                let h = parse_u32(&node.params, "height").unwrap_or(resolution[1]);
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
    let tgt_w = parse_f32(&target_node.params, "width")
        .unwrap_or(resolution[0] as f32)
        .max(1.0);
    let tgt_h = parse_f32(&target_node.params, "height")
        .unwrap_or(resolution[1] as f32)
        .max(1.0);
    let target_texture_name = ids
        .get(&target_texture_id)
        .cloned()
        .ok_or_else(|| anyhow!("missing name for node: {}", target_texture_id))?;

    for layer_id in composite_layers_in_order {
        let layer_node = find_node(&nodes_by_id, layer_id)?;
        match layer_node.node_type.as_str() {
            "RenderPass" => {
                let pass_name = ids
                    .get(layer_id)
                    .cloned()
                    .ok_or_else(|| anyhow!("missing name for node: {layer_id}"))?;

                let blend_state = parse_render_pass_blend_state(&layer_node.params)?;

                let geometry_node_id = incoming_connection(&prepared.scene, layer_id, "geometry")
                    .map(|c| c.from.node_id.clone())
                    .ok_or_else(|| anyhow!("RenderPass.geometry missing for {layer_id}"))?;

                let geometry_node = find_node(&nodes_by_id, &geometry_node_id)?;
                if geometry_node.node_type != "Rect2DGeometry" {
                    bail!(
                        "RenderPass.geometry must come from Rect2DGeometry, got {}",
                        geometry_node.node_type
                    );
                }

                let geometry_buffer = ids
                    .get(&geometry_node_id)
                    .cloned()
                    .ok_or_else(|| anyhow!("missing name for node: {}", geometry_node_id))?;

                let geo_w = parse_f32(&geometry_node.params, "width").unwrap_or(100.0);
                let geo_h = parse_f32(&geometry_node.params, "height").unwrap_or(geo_w);
                let geo_x = parse_f32(&geometry_node.params, "x").unwrap_or(0.0);
                let geo_y = parse_f32(&geometry_node.params, "y").unwrap_or(0.0);

                let params_name: ResourceName = format!("params_{layer_id}").into();
                let params = Params {
                    target_size: [tgt_w, tgt_h],
                    geo_size: [geo_w.max(1.0), geo_h.max(1.0)],
                    center: [geo_x, geo_y],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.9, 0.2, 0.2, 1.0],
                };

                let bundle = build_pass_wgsl_bundle(&prepared.scene, nodes_by_id, layer_id)?;
                let shader_wgsl = bundle.module;

                let texture_bindings: Vec<PassTextureBinding> = bundle
                    .image_textures
                    .iter()
                    .filter_map(|id| ids.get(id).cloned().map(|tex| PassTextureBinding {
                        texture: tex,
                        image_node_id: Some(id.clone()),
                    }))
                    .collect();

                render_pass_specs.push(RenderPassSpec {
                    name: pass_name.clone(),
                    geometry_buffer,
                    target_texture: target_texture_name.clone(),
                    params_buffer: params_name,
                    params,
                    shader_wgsl,
                    texture_bindings,
                    sampler_kind: SamplerKind::NearestClamp,
                    blend_state,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                });
                composite_passes.push(pass_name);
            }
            "GuassianBlurPass" => {
                // For now: GuassianBlurPass must take its image input from ImageTexture.
                let img_conn = incoming_connection(&prepared.scene, layer_id, "image")
                    .ok_or_else(|| anyhow!("GuassianBlurPass.image missing for {layer_id}"))?;
                let img_node = find_node(&nodes_by_id, &img_conn.from.node_id)?;
                if img_node.node_type != "ImageTexture" {
                    bail!(
                        "GuassianBlurPass.image must come from ImageTexture, got {}",
                        img_node.node_type
                    );
                }

                let sigma = parse_f32(&layer_node.params, "radius").unwrap_or(0.0).max(0.0);
                let (mip_level, sigma_p) = gaussian_mip_level_and_sigma_p(sigma);
                let downsample_factor: u32 = 1 << mip_level;
                let (kernel, offset, _num) = gaussian_kernel_8(sigma_p.max(1e-6));

                let downsample_steps: Vec<u32> = if downsample_factor == 16 {
                    vec![8, 2]
                } else {
                    vec![downsample_factor]
                };

                let format = parse_texture_format(&target_node.params)?;

                // Allocate textures (and matching fullscreen geometry) for each downsample step.
                // step 8 -> size >> 3; step 2 after 8 -> additional >> 1.
                let mut step_textures: Vec<(u32, ResourceName, u32, u32, ResourceName)> = Vec::new();
                let mut cur_w: u32 = tgt_w as u32;
                let mut cur_h: u32 = tgt_h as u32;
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

                // Fullscreen geometry buffers for blur + upsample.
                let geo_ds: ResourceName = format!("{layer_id}__geo_ds").into();
                geometry_buffers
                    .push((geo_ds.clone(), make_fullscreen_geometry(ds_w as f32, ds_h as f32)));
                let geo_out: ResourceName = format!("{layer_id}__geo_out").into();
                geometry_buffers.push((geo_out.clone(), make_fullscreen_geometry(tgt_w, tgt_h)));

                // Downsample chain
                let mut prev_tex: Option<ResourceName> = None;
                for (step, tex, step_w, step_h, step_geo) in &step_textures {
                    let params_name: ResourceName = format!("params_{layer_id}__downsample_{step}").into();
                    let bundle = {
                        let body = match *step {
                            1 => {
                                r#"
let src_resolution = vec2f(textureDimensions(src_tex));
let dst_xy = vec2f(in.position.xy);
let uv = dst_xy / src_resolution;
return textureSampleLevel(src_tex, src_samp, uv, 0.0);
"#
                                .to_string()
                            }
                            2 => {
                                r#"
let src_resolution = vec2f(textureDimensions(src_tex));
let dst_xy = vec2f(in.position.xy);
let base = dst_xy * 2.0 - vec2f(0.5);

var sum = vec4f(0.0);
for (var y: i32 = 0; y < 2; y = y + 1) {
    for (var x: i32 = 0; x < 2; x = x + 1) {
        let uv = (base + vec2f(f32(x), f32(y))) / src_resolution;
        sum = sum + textureSampleLevel(src_tex, src_samp, uv, 0.0);
    }
}

return sum * 0.25;
"#
                                .to_string()
                            }
                            4 => {
                                r#"
let src_resolution = vec2f(textureDimensions(src_tex));
let dst_xy = vec2f(in.position.xy);
let base = dst_xy * 4.0 - vec2f(1.5);

var sum = vec4f(0.0);
for (var y: i32 = 0; y < 4; y = y + 1) {
    for (var x: i32 = 0; x < 4; x = x + 1) {
        let uv = (base + vec2f(f32(x), f32(y))) / src_resolution;
        sum = sum + textureSampleLevel(src_tex, src_samp, uv, 0.0);
    }
}

return sum * (1.0 / 16.0);
"#
                                .to_string()
                            }
                            8 => {
                                r#"
let src_resolution = vec2f(textureDimensions(src_tex));
let dst_xy = vec2f(in.position.xy);
let base = dst_xy * 8.0 - vec2f(3.5);

var sum = vec4f(0.0);
for (var y: i32 = 0; y < 8; y = y + 1) {
    for (var x: i32 = 0; x < 8; x = x + 1) {
        let uv = (base + vec2f(f32(x), f32(y))) / src_resolution;
        sum = sum + textureSampleLevel(src_tex, src_samp, uv, 0.0);
    }
}

return sum * (1.0 / 64.0);
"#
                                .to_string()
                            }
                            other => bail!("GuassianBlurPass: unsupported downsample factor {other}"),
                        };
                        build_fullscreen_textured_bundle(body)
                    };

                    let params_val = Params {
                        target_size: [*step_w as f32, *step_h as f32],
                        geo_size: [*step_w as f32, *step_h as f32],
                        center: [0.0, 0.0],
                        time: 0.0,
                        _pad0: 0.0,
                        color: [0.0, 0.0, 0.0, 1.0],
                    };

                    let (src_tex, src_img_node) = match &prev_tex {
                        None => (
                            ids.get(&img_conn.from.node_id)
                                .cloned()
                                .ok_or_else(|| anyhow!("missing name for node: {}", img_conn.from.node_id))?,
                            Some(img_conn.from.node_id.clone()),
                        ),
                        Some(t) => (t.clone(), None),
                    };

                    render_pass_specs.push(RenderPassSpec {
                        name: format!("{layer_id}__downsample_{step}").into(),
                        geometry_buffer: step_geo.clone(),
                        target_texture: tex.clone(),
                        params_buffer: params_name,
                        params: params_val,
                        shader_wgsl: bundle.module,
                        texture_bindings: vec![PassTextureBinding {
                            texture: src_tex,
                            image_node_id: src_img_node,
                        }],
                        sampler_kind: SamplerKind::NearestMirror,
                        blend_state: BlendState::REPLACE,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                    });
                    composite_passes.push(format!("{layer_id}__downsample_{step}").into());
                    prev_tex = Some(tex.clone());
                }

                let ds_src_tex: ResourceName = prev_tex.ok_or_else(|| anyhow!("GuassianBlurPass: missing downsample output"))?;

                // 2) Horizontal blur: ds_src_tex -> h_tex
                let params_h: ResourceName = format!("params_{layer_id}__hblur_ds{downsample_factor}").into();
                let bundle_h = {
                    let kernel_wgsl = array8_f32_wgsl(kernel);
                    let offset_wgsl = array8_f32_wgsl(offset);
                    let body = format!(
                        r#"
let original = vec2f(textureDimensions(src_tex));
let xy = vec2f(in.position.xy);
let k = {kernel_wgsl};
let o = {offset_wgsl};
var color = vec4f(0.0);
for (var i: u32 = 0u; i < 8u; i = i + 1u) {{
    let uv_pos = (xy + vec2f(o[i], 0.0)) / original;
    let uv_neg = (xy - vec2f(o[i], 0.0)) / original;
    color = color + textureSampleLevel(src_tex, src_samp, uv_pos, 0.0) * k[i];
    color = color + textureSampleLevel(src_tex, src_samp, uv_neg, 0.0) * k[i];
}}
return color;
"#
                    );
                    build_fullscreen_textured_bundle(body)
                };
                let params_h_val = Params {
                    target_size: [ds_w as f32, ds_h as f32],
                    geo_size: [ds_w as f32, ds_h as f32],
                    center: [0.0, 0.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                };
                render_pass_specs.push(RenderPassSpec {
                    name: format!("{layer_id}__hblur_ds{downsample_factor}").into(),
                    geometry_buffer: geo_ds.clone(),
                    target_texture: h_tex.clone(),
                    params_buffer: params_h.clone(),
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
                let params_v: ResourceName = format!("params_{layer_id}__vblur_ds{downsample_factor}").into();
                let bundle_v = {
                    let kernel_wgsl = array8_f32_wgsl(kernel);
                    let offset_wgsl = array8_f32_wgsl(offset);
                    let body = format!(
                        r#"
let original = vec2f(textureDimensions(src_tex));
let xy = vec2f(in.position.xy);
let k = {kernel_wgsl};
let o = {offset_wgsl};
var color = vec4f(0.0);
for (var i: u32 = 0u; i < 8u; i = i + 1u) {{
    let uv_pos = (xy + vec2f(0.0, o[i])) / original;
    let uv_neg = (xy - vec2f(0.0, o[i])) / original;
    color = color + textureSampleLevel(src_tex, src_samp, uv_pos, 0.0) * k[i];
    color = color + textureSampleLevel(src_tex, src_samp, uv_neg, 0.0) * k[i];
}}
return color;
"#
                    );
                    build_fullscreen_textured_bundle(body)
                };
                let params_v_val = Params {
                    target_size: [ds_w as f32, ds_h as f32],
                    geo_size: [ds_w as f32, ds_h as f32],
                    center: [0.0, 0.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                };
                render_pass_specs.push(RenderPassSpec {
                    name: format!("{layer_id}__vblur_ds{downsample_factor}").into(),
                    geometry_buffer: geo_ds.clone(),
                    target_texture: v_tex.clone(),
                    params_buffer: params_v.clone(),
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

                // 4) Upsample bilinear back to target: v_tex -> output target
                let params_u: ResourceName = format!("params_{layer_id}__upsample_bilinear_ds{downsample_factor}").into();
                let bundle_u = {
                    let body = format!(
                        r#"
let dst_xy = vec2f(in.position.xy);
let dst_resolution = params.target_size;
let uv = dst_xy / dst_resolution;
return textureSampleLevel(src_tex, src_samp, uv, 0.0);
"#
                    );
                    build_fullscreen_textured_bundle(body)
                };
                let params_u_val = Params {
                    target_size: [tgt_w, tgt_h],
                    geo_size: [tgt_w, tgt_h],
                    center: [0.0, 0.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                };
                render_pass_specs.push(RenderPassSpec {
                    name: format!("{layer_id}__upsample_bilinear_ds{downsample_factor}").into(),
                    geometry_buffer: geo_out.clone(),
                    target_texture: target_texture_name.clone(),
                    params_buffer: params_u.clone(),
                    params: params_u_val,
                    shader_wgsl: bundle_u.module,
                    texture_bindings: vec![PassTextureBinding {
                        texture: v_tex.clone(),
                        image_node_id: None,
                    }],
                    sampler_kind: SamplerKind::LinearMirror,
                    blend_state: BlendState::REPLACE,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                });

                composite_passes.push(
                    format!("{layer_id}__upsample_bilinear_ds{downsample_factor}").into(),
                );
            }
            other => bail!(
                "Composite layer must be RenderPass or GuassianBlurPass, got {other} for {layer_id}"
            ),
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

    for pass in &pass_bindings {
        buffer_specs.push(BufferSpec::Sized {
            name: pass.params_buffer.clone(),
            size: core::mem::size_of::<Params>(),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
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
                vec![pb.clone(), rel_base.join(&pb), rel_base.join("assets").join(&pb)]
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
                bail!("expected ImageTexture node for {node_id}, got {}", node.node_type);
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
                    Ok(img) => ensure_rgba8(Arc::new(img)),
                    Err(_e) => placeholder_image(),
                },
                _ => {
                    let path = node.params.get("path").and_then(|v| v.as_str());
                    ensure_rgba8(load_image_with_fallback(&rel_base, path))
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
    shader_space.declare_samplers(vec![SamplerSpec {
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
    }]);

    for spec in &render_pass_specs {
        let geometry_buffer = spec.geometry_buffer.clone();
        let target_texture = spec.target_texture.clone();
        let params_buffer = spec.params_buffer.clone();
        let shader_wgsl = spec.shader_wgsl.clone();
        let blend_state = spec.blend_state;
        let color_load_op = spec.color_load_op;

        let texture_names: Vec<ResourceName> = spec.texture_bindings.iter().map(|b| b.texture.clone()).collect();
        let sampler_name = match spec.sampler_kind {
            SamplerKind::NearestClamp => nearest_sampler.clone(),
            SamplerKind::NearestMirror => nearest_mirror_sampler.clone(),
            SamplerKind::LinearMirror => linear_mirror_sampler.clone(),
        };

        let shader_desc: wgpu::ShaderModuleDescriptor<'static> = wgpu::ShaderModuleDescriptor {
            label: Some("node-forge-pass"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(shader_wgsl)),
        };
        shader_space.render_pass(spec.name.clone(), move |builder| {
            let mut b = builder
                .shader(shader_desc)
                .bind_uniform_buffer(0, 0, params_buffer, ShaderStages::VERTEX_FRAGMENT)
                .bind_attribute_buffer(
                    0,
                    geometry_buffer,
                    wgpu::VertexStepMode::Vertex,
                    vertex_attr_array![0 => Float32x3].to_vec(),
                )
                ;

            for (i, tex_name) in texture_names.iter().enumerate() {
                let tex_binding = (i as u32) * 2;
                let samp_binding = tex_binding + 1;
                b = b
                    .bind_texture(1, tex_binding, tex_name.clone(), ShaderStages::FRAGMENT)
                    .bind_sampler(1, samp_binding, sampler_name.clone(), ShaderStages::FRAGMENT);
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
    use serde_json::json;

    #[test]
    fn render_pass_blend_state_from_explicit_params() {
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        params.insert("blendfunc".to_string(), json!("add"));
        params.insert("src_factor".to_string(), json!("src-alpha"));
        params.insert("dst_factor".to_string(), json!("one-minus-src-alpha"));
        params.insert("src_alpha_factor".to_string(), json!("one"));
        params.insert("dst_alpha_factor".to_string(), json!("one-minus-src-alpha"));

        let got = parse_render_pass_blend_state(&params).unwrap();
        let expected = BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
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
                },
                crate::dsl::Node {
                    id: "p_img".to_string(),
                    node_type: "RenderPass".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                },
                crate::dsl::Node {
                    id: "p0".to_string(),
                    node_type: "RenderPass".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
                },
                crate::dsl::Node {
                    id: "p1".to_string(),
                    node_type: "RenderPass".to_string(),
                    params: HashMap::new(),
                    inputs: vec![],
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
            outputs: Some(HashMap::from([(String::from("composite"), String::from("out"))])),
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
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC,
    }]);

    let shader_desc: wgpu::ShaderModuleDescriptor<'static> = wgpu::ShaderModuleDescriptor {
        label: Some("node-forge-error-purple"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(
            r#"
struct VSOut {
    @builtin(position) position: vec4f,
};

@vertex
fn vs_main(@location(0) position: vec3f) -> VSOut {
    var out: VSOut;
    out.position = vec4f(position, 1.0);
    return out;
}

@fragment
fn fs_main(_in: VSOut) -> @location(0) vec4f {
    // Purple error screen.
    return vec4f(1.0, 0.0, 1.0, 1.0);
}
"#,
        )),
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
