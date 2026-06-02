//! IntelligentLight pass assembler.
//!
//! Fullscreen procedural fragment shader simulating a physical spring-powered
//! intelligent lighting fixture with 11 color zones.

use anyhow::{Context, Result};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color},
};

use crate::{
    dsl::{Node, incoming_connection},
    renderer::{
        camera::legacy_projection_camera_matrix,
        types::{GraphBinding, GraphBindingKind, GraphSchema, PassExtension, PassOutputSpec},
        utils::{cpu_num_f32, cpu_num_u32_min_1},
        wgsl::{build_fullscreen_textured_bundle, build_upsample_bilinear_bundle},
    },
};

use super::super::pass_spec::{
    PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl, make_params,
};
use super::args::{BuilderState, SceneContext};

/// Per-frame update configuration for IntelligentLight.
///
/// Stored inside `PassExtension::IntelligentLight` and used by the runtime
/// to recompute the uniform buffer each frame (positions, params, colors).
#[derive(Clone, Debug)]
pub struct ILightUpdateConfig {
    pub layer_id: String,
    pub driver_node_id: Option<String>,
    pub driver_fallback: f64,
    pub power_fallback: f32,
    pub lightness_fallback: f32,
}

impl ILightUpdateConfig {
    pub fn pack_buffer(&self, scene: &crate::dsl::SceneDSL) -> Vec<u8> {
        let nodes_by_id: std::collections::HashMap<&str, &crate::dsl::Node> =
            scene.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        let layer_node = nodes_by_id.get(self.layer_id.as_str());

        let driver = self
            .driver_node_id
            .as_ref()
            .and_then(|nid| nodes_by_id.get(nid.as_str()))
            .and_then(|n| n.params.get("value"))
            .and_then(|v| v.as_f64())
            .unwrap_or(self.driver_fallback);

        let power = layer_node
            .and_then(|n| n.params.get("power"))
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(self.power_fallback);

        let lightness = layer_node
            .and_then(|n| n.params.get("lightness"))
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(self.lightness_fallback);

        let mut colors = [[1.0f32; 3]; 11];
        for i in 0..11 {
            let port_id = format!("color{i}");
            colors[i] = resolve_color_runtime(scene, &nodes_by_id, &self.layer_id, &port_id)
                .unwrap_or([1.0, 1.0, 1.0]);
        }

        pack_ilight_buffer(driver, power, lightness, &colors)
    }
}

fn resolve_color_runtime(
    scene: &crate::dsl::SceneDSL,
    nodes_by_id: &std::collections::HashMap<&str, &crate::dsl::Node>,
    layer_id: &str,
    port_id: &str,
) -> Option<[f32; 3]> {
    if let Some(conn) = incoming_connection(scene, layer_id, port_id) {
        if let Some(upstream) = nodes_by_id.get(conn.from.node_id.as_str()) {
            if let Some(c) = parse_color_from_params(&upstream.params, "value")
                .or_else(|| parse_color_from_params(&upstream.params, &conn.from.port_id))
            {
                return Some(c);
            }
        }
    }
    if let Some(layer_node) = nodes_by_id.get(layer_id) {
        if let Some(c) = parse_color_from_params(&layer_node.params, port_id) {
            return Some(c);
        }
    }
    None
}

/// Assemble an `"IntelligentLight"` layer.
pub(crate) fn assemble_intelligent_light(
    sc: &SceneContext<'_>,
    bs: &mut BuilderState<'_>,
    layer_id: &str,
    layer_node: &Node,
) -> Result<()> {
    let scene = sc.scene();
    let nodes_by_id = sc.nodes_by_id();
    let tgt_w = bs.tgt_size[0];
    let tgt_h = bs.tgt_size[1];
    let tgt_w_u = bs.tgt_size_u[0];
    let tgt_h_u = bs.tgt_size_u[1];

    let pass_blend_state =
        crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
            .with_context(|| format!("invalid blend params for IntelligentLight {}", layer_id))?;

    // ── Resolve inputs ──────────────────────────────────────────────────

    let driver = cpu_num_f32(scene, &nodes_by_id, layer_node, "driver", 0.0)?;
    let power = cpu_num_f32(scene, &nodes_by_id, layer_node, "power", 0.0)?;
    let lightness = cpu_num_f32(scene, &nodes_by_id, layer_node, "lightness", 0.75)?;

    let driver_node_id =
        incoming_connection(scene, layer_id, "driver").map(|conn| conn.from.node_id.clone());

    let mut colors: [[f32; 3]; 11] = [[1.0, 1.0, 1.0]; 11];
    for i in 0..11 {
        let port_id = format!("color{i}");
        colors[i] = resolve_color_input(sc, layer_node, layer_id, &port_id);
    }

    // ── Intermediate texture (low-res render) ─────────────────────────

    let inter_w = cpu_num_u32_min_1(scene, &nodes_by_id, layer_node, "width", 60)?;
    let inter_h = cpu_num_u32_min_1(scene, &nodes_by_id, layer_node, "height", 37)?;
    let inter_w_f = inter_w as f32;
    let inter_h_f = inter_h as f32;

    let inter_tex: ResourceName = format!("sys.ilight.{layer_id}.inter").into();
    bs.textures.push(TextureDecl {
        name: inter_tex.clone(),
        size: [inter_w, inter_h],
        format: bs.sampled_pass_format,
        sample_count: 1,
        needs_sampling: false,
    });

    // ── Pass 1: Render intelliLight shader to small intermediate ─────

    let geo: ResourceName = format!("sys.ilight.{layer_id}.geo").into();
    bs.push_fullscreen_geometry(geo.clone(), inter_w_f, inter_h_f);

    let params_name: ResourceName = format!("params.sys.ilight.{layer_id}").into();
    let camera = legacy_projection_camera_matrix([inter_w_f, inter_h_f]);
    let params_val = make_params(
        [inter_w_f, inter_h_f],
        [inter_w_f, inter_h_f],
        [inter_w_f * 0.5, inter_h_f * 0.5],
        camera,
        [0.0, 0.0, 0.0, 0.0],
    );

    let pass_name: ResourceName = format!("sys.ilight.{layer_id}.pass").into();

    // Build ilight uniform buffer (CPU-computed light positions + params).
    let ilight_buffer_name: ResourceName = format!("params.sys.ilight.{layer_id}.graph").into();
    let ilight_values = pack_ilight_buffer(driver as f64, power, lightness, &colors);
    let ilight_config = ILightUpdateConfig {
        layer_id: layer_id.to_string(),
        driver_node_id,
        driver_fallback: driver as f64,
        power_fallback: power,
        lightness_fallback: lightness,
    };
    let graph_binding = GraphBinding {
        buffer_name: ilight_buffer_name,
        kind: GraphBindingKind::Uniform,
        schema: GraphSchema {
            fields: Vec::new(),
            size_bytes: ILIGHT_BUFFER_SIZE,
        },
    };

    let shader_wgsl = build_intelligent_light_wgsl();

    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: pass_name.as_str().to_string(),
        name: pass_name.clone(),
        geometry_buffer: geo.clone(),
        instance_buffer: None,
        normals_buffer: None,
        target_texture: inter_tex.clone(),
        resolve_target: None,
        params_buffer: params_name,
        baked_data_parse_buffer: None,
        params: params_val,
        graph_binding: Some(graph_binding),
        graph_values: Some(ilight_values),
        shader_wgsl,
        texture_bindings: Vec::new(),
        sampler_kinds: Vec::new(),
        blend_state: BlendState::REPLACE,
        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
        sample_count: 1,
    });
    bs.pass_extensions.insert(
        pass_name.as_str().to_string(),
        PassExtension::IntelligentLight(ilight_config),
    );
    bs.composite_passes.push(pass_name);

    // ── Output texture (upsampled) ──────────────────────────────────────

    let is_sampled_output = bs.sampled_pass_ids.contains(layer_id);
    let writes_scene_output_target = !is_sampled_output;

    let output_tex: ResourceName = if is_sampled_output {
        let tex: ResourceName = format!("sys.ilight.{layer_id}.out").into();
        bs.textures.push(TextureDecl {
            name: tex.clone(),
            size: [tgt_w_u, tgt_h_u],
            format: bs.sampled_pass_format,
            sample_count: 1,
            needs_sampling: false,
        });
        tex
    } else {
        bs.target_texture_name.clone()
    };

    // ── Pass 2: Upsample blit from intermediate to target ───────────

    let upsample_pass_name: ResourceName = format!("sys.ilight.{layer_id}.upsample.pass").into();
    let upsample_geo: ResourceName = format!("sys.ilight.{layer_id}.upsample.geo").into();
    bs.push_fullscreen_geometry(upsample_geo.clone(), tgt_w, tgt_h);

    let upsample_params_name: ResourceName =
        format!("params.sys.ilight.{layer_id}.upsample").into();
    let upsample_params = make_params(
        [tgt_w, tgt_h],
        [tgt_w, tgt_h],
        [tgt_w * 0.5, tgt_h * 0.5],
        legacy_projection_camera_matrix([tgt_w, tgt_h]),
        [0.0, 0.0, 0.0, 0.0],
    );

    let upsample_bundle = build_upsample_bilinear_bundle();

    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: upsample_pass_name.as_str().to_string(),
        name: upsample_pass_name.clone(),
        geometry_buffer: upsample_geo,
        instance_buffer: None,
        normals_buffer: None,
        target_texture: output_tex.clone(),
        resolve_target: None,
        params_buffer: upsample_params_name,
        baked_data_parse_buffer: None,
        params: upsample_params,
        graph_binding: None,
        graph_values: None,
        shader_wgsl: upsample_bundle.module,
        texture_bindings: vec![PassTextureBinding {
            texture: inter_tex.clone(),
            image_node_id: None,
        }],
        sampler_kinds: vec![SamplerKind::LinearClamp],
        blend_state: pass_blend_state,
        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
        sample_count: 1,
    });
    bs.composite_passes.push(upsample_pass_name);

    // ── Register output ─────────────────────────────────────────────────

    bs.pass_output_registry.register(PassOutputSpec {
        node_id: layer_id.to_string(),
        texture_name: output_tex.clone(),
        resolution: [tgt_w_u, tgt_h_u],
        format: if is_sampled_output {
            bs.sampled_pass_format
        } else {
            bs.target_format
        },
    });

    // ── Composition consumers ───────────────────────────────────────────

    let composition_consumers = sc
        .composition_consumers_by_source
        .get(layer_id)
        .cloned()
        .unwrap_or_default();

    if !composition_consumers.is_empty() {
        let compose_blend_state = pass_blend_state;
        for composition_id in composition_consumers {
            let Some(comp_ctx) = sc.composition_contexts.get(&composition_id) else {
                continue;
            };
            if output_tex == comp_ctx.target_texture_name {
                continue;
            }
            if writes_scene_output_target && comp_ctx.target_texture_name == *bs.target_texture_name
            {
                continue;
            }

            let comp_w = comp_ctx.target_size_px[0];
            let comp_h = comp_ctx.target_size_px[1];

            let compose_geo: ResourceName =
                format!("sys.ilight.{layer_id}.to.{composition_id}.compose.geo").into();
            bs.push_fullscreen_geometry(compose_geo.clone(), comp_w, comp_h);

            let compose_pass_name: ResourceName =
                format!("sys.ilight.{layer_id}.to.{composition_id}.compose.pass").into();
            let compose_params_name: ResourceName =
                format!("params.sys.ilight.{layer_id}.to.{composition_id}.compose").into();
            let compose_params = make_params(
                [comp_w, comp_h],
                [comp_w, comp_h],
                [comp_w * 0.5, comp_h * 0.5],
                legacy_projection_camera_matrix([comp_w, comp_h]),
                [0.0, 0.0, 0.0, 0.0],
            );

            bs.render_pass_specs.push(RenderPassSpec {
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
                    texture: output_tex.clone(),
                    image_node_id: None,
                }],
                sampler_kinds: vec![SamplerKind::LinearClamp],
                blend_state: compose_blend_state,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                sample_count: 1,
            });

            bs.composite_passes.push(compose_pass_name);
        }
    }

    Ok(())
}

// ── Color input resolution ──────────────────────────────────────────────

/// Resolve a color input for the IntelligentLight node.
///
/// Precedence: incoming connection → node params → white default.
fn resolve_color_input(
    sc: &SceneContext<'_>,
    layer_node: &Node,
    layer_id: &str,
    port_id: &str,
) -> [f32; 3] {
    let scene = sc.scene();
    let nodes_by_id = sc.nodes_by_id();

    // 1. Check incoming connection.
    if let Some(conn) = incoming_connection(scene, layer_id, port_id) {
        if let Some(upstream_node) = nodes_by_id.get(&conn.from.node_id) {
            // Try to read color from upstream node's params ("value" or the port name).
            if let Some(c) = parse_color_from_params(&upstream_node.params, "value")
                .or_else(|| parse_color_from_params(&upstream_node.params, &conn.from.port_id))
            {
                return c;
            }
        }
    }

    // 2. Check inline params on the IntelligentLight node itself.
    if let Some(c) = parse_color_from_params(&layer_node.params, port_id) {
        return c;
    }

    // 3. Default: white.
    [1.0, 1.0, 1.0]
}

/// Parse a color value from a params map. Supports:
/// - Hex string: "#RRGGBB" or "#RGB"
/// - Object: { "r": f, "g": f, "b": f }
/// - Array: [r, g, b]
fn parse_color_from_params(
    params: &std::collections::HashMap<String, serde_json::Value>,
    key: &str,
) -> Option<[f32; 3]> {
    let value = params.get(key)?;

    // Hex string
    if let Some(s) = value.as_str() {
        return parse_hex_color(s);
    }

    // Object { r, g, b }
    if let Some(obj) = value.as_object() {
        let r = obj.get("r").and_then(json_f32).unwrap_or(1.0);
        let g = obj.get("g").and_then(json_f32).unwrap_or(1.0);
        let b = obj.get("b").and_then(json_f32).unwrap_or(1.0);
        return Some([r, g, b]);
    }

    // Array [r, g, b]
    if let Some(arr) = value.as_array() {
        if arr.len() >= 3 {
            let r = json_f32(&arr[0]).unwrap_or(1.0);
            let g = json_f32(&arr[1]).unwrap_or(1.0);
            let b = json_f32(&arr[2]).unwrap_or(1.0);
            return Some([r, g, b]);
        }
    }

    None
}

fn json_f32(v: &serde_json::Value) -> Option<f32> {
    v.as_f64()
        .map(|x| x as f32)
        .or_else(|| v.as_i64().map(|x| x as f32))
        .or_else(|| v.as_u64().map(|x| x as f32))
}

/// Parse "#RRGGBB" or "#RGB" hex color to [f32; 3] in 0..1 range.
fn parse_hex_color(s: &str) -> Option<[f32; 3]> {
    let s = s.trim().strip_prefix('#')?;
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()? as f32 / 255.0;
            let g = u8::from_str_radix(&s[2..4], 16).ok()? as f32 / 255.0;
            let b = u8::from_str_radix(&s[4..6], 16).ok()? as f32 / 255.0;
            Some([r, g, b])
        }
        3 => {
            let r = u8::from_str_radix(&s[0..1], 16).ok()? as f32 / 15.0;
            let g = u8::from_str_radix(&s[1..2], 16).ok()? as f32 / 15.0;
            let b = u8::from_str_radix(&s[2..3], 16).ok()? as f32 / 15.0;
            Some([r, g, b])
        }
        _ => None,
    }
}

// ── WGSL shader generation ──────────────────────────────────────────────

pub(crate) fn build_intelligent_light_wgsl() -> String {
    r#"//── IntelligentLight pass (CPU-driven uniforms) ─────────────────────

struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,
    geo_translate: vec2f,
    geo_scale: vec2f,
    time: f32,
    _pad0: f32,
    color: vec4f,
    camera: mat4x4f,
    camera_position: vec4f,
};

struct ILightData {
    lights: array<vec4f, 11>,
    params: vec4f,
    colors: array<vec4f, 11>,
};

@group(0) @binding(0)
var<uniform> params: Params;
@group(0) @binding(2)
var<uniform> ilight_data: ILightData;

struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    @location(1) frag_coord_gl: vec2f,
    @location(2) local_px: vec3f,
    @location(3) geo_size_px: vec2f,
};

@vertex
fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {
    var out: VSOut;

    let _unused_geo_size = params.geo_size;
    let _unused_geo_translate = params.geo_translate;
    let _unused_geo_scale = params.geo_scale;

    out.uv = uv;
    out.geo_size_px = params.geo_size;
    out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, position.z);

    let p_px = params.center + position.xy;
    out.position = params.camera * vec4f(p_px, position.z, 1.0);
    out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
    return out;
}

// ── Constants ────────────────────────────────────────────────────────

const NUM_LIGHTS: u32 = 11u;
const BASE_COLOR: vec3f = vec3f(0.0, 0.5884, 1.0);

// ── Fragment shader ──────────────────────────────────────────────────

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    let aspect = params.target_size.x / params.target_size.y;
    var uv = in.uv * 2.0 - 1.0;
    uv.x *= aspect;

    var current_color = BASE_COLOR;

    for (var i = 0u; i < NUM_LIGHTS; i = i + 1u) {
        let lpos = ilight_data.lights[i].xy;
        let d = distance(uv, lpos);
        let factor = clamp(1.0 - d, 0.0, 1.0);
        let s = smoothstep(0.0, 1.0, factor);
        let light_color = ilight_data.colors[i].xyz * s;
        current_color = current_color * (1.0 - s) + light_color;
    }

    current_color = min(vec3f(1.0), current_color);

    let power = ilight_data.params.x;
    let lightness = ilight_data.params.y;
    let brightness = 1.0 + power * 0.2;
    let luminance = dot(current_color, vec3f(0.2126, 0.7153, 0.0722));
    let scale = mix(0.75, 0.775, lightness);
    let result = mix(vec3f(luminance), current_color, vec3f(brightness)) * scale;

    return vec4f(clamp(result, vec3f(0.0), vec3f(1.0)), 1.0);
}
"#
    .to_string()
}

// ── CPU-side noise and light position computation ──────────────────────

const PERM: [u16; 512] = [
    0x97, 0xa0, 0x89, 0x5b, 0x5a, 0x0f, 0x83, 0x0d, 0xc9, 0x5f, 0x60, 0x35, 0xc2, 0xe9, 0x07, 0xe1,
    0x8c, 0x24, 0x67, 0x1e, 0x45, 0x8e, 0x08, 0x63, 0x25, 0xf0, 0x15, 0x0a, 0x17, 0xbe, 0x06, 0x94,
    0xf7, 0x78, 0xea, 0x4b, 0x00, 0x1a, 0xc5, 0x3e, 0x5e, 0xfc, 0xdb, 0xcb, 0x75, 0x23, 0x0b, 0x20,
    0x39, 0xb1, 0x21, 0x58, 0xed, 0x95, 0x38, 0x57, 0xae, 0x14, 0x7d, 0x88, 0xab, 0xa8, 0x44, 0xaf,
    0x4a, 0xa5, 0x47, 0x86, 0x8b, 0x30, 0x1b, 0xa6, 0x4d, 0x92, 0x9e, 0xe7, 0x53, 0x6f, 0xe5, 0x7a,
    0x3c, 0xd3, 0x85, 0xe6, 0xdc, 0x69, 0x5c, 0x29, 0x37, 0x2e, 0xf5, 0x28, 0xf4, 0x66, 0x8f, 0x36,
    0x41, 0x19, 0x3f, 0xa1, 0x01, 0xd8, 0x50, 0x49, 0xd1, 0x4c, 0x84, 0xbb, 0xd0, 0x59, 0x12, 0xa9,
    0xc8, 0xc4, 0x87, 0x82, 0x74, 0xbc, 0x9f, 0x56, 0xa4, 0x64, 0x6d, 0xc6, 0xad, 0xba, 0x03, 0x40,
    0x34, 0xd9, 0xe2, 0xfa, 0x7c, 0x7b, 0x05, 0xca, 0x26, 0x93, 0x76, 0x7e, 0xff, 0x52, 0x55, 0xd4,
    0xcf, 0xce, 0x3b, 0xe3, 0x2f, 0x10, 0x3a, 0x11, 0xb6, 0xbd, 0x1c, 0x2a, 0xdf, 0xb7, 0xaa, 0xd5,
    0x77, 0xf8, 0x98, 0x02, 0x2c, 0x9a, 0xa3, 0x46, 0xdd, 0x99, 0x65, 0x9b, 0xa7, 0x2b, 0xac, 0x09,
    0x81, 0x16, 0x27, 0xfd, 0x13, 0x62, 0x6c, 0x6e, 0x4f, 0x71, 0xe0, 0xe8, 0xb2, 0xb9, 0x70, 0x68,
    0xda, 0xf6, 0x61, 0xe4, 0xfb, 0x22, 0xf2, 0xc1, 0xee, 0xd2, 0x90, 0x0c, 0xbf, 0xb3, 0xa2, 0xf1,
    0x51, 0x33, 0x91, 0xeb, 0xf9, 0x0e, 0xef, 0x6b, 0x31, 0xc0, 0xd6, 0x1f, 0xb5, 0xc7, 0x6a, 0x9d,
    0xb8, 0x54, 0xcc, 0xb0, 0x73, 0x79, 0x32, 0x2d, 0x7f, 0x04, 0x96, 0xfe, 0x8a, 0xec, 0xcd, 0x5d,
    0xde, 0x72, 0x43, 0x1d, 0x18, 0x48, 0xf3, 0x8d, 0x80, 0xc3, 0x4e, 0x42, 0xd7, 0x3d, 0x9c, 0xb4,
    0x97, 0xa0, 0x89, 0x5b, 0x5a, 0x0f, 0x83, 0x0d, 0xc9, 0x5f, 0x60, 0x35, 0xc2, 0xe9, 0x07, 0xe1,
    0x8c, 0x24, 0x67, 0x1e, 0x45, 0x8e, 0x08, 0x63, 0x25, 0xf0, 0x15, 0x0a, 0x17, 0xbe, 0x06, 0x94,
    0xf7, 0x78, 0xea, 0x4b, 0x00, 0x1a, 0xc5, 0x3e, 0x5e, 0xfc, 0xdb, 0xcb, 0x75, 0x23, 0x0b, 0x20,
    0x39, 0xb1, 0x21, 0x58, 0xed, 0x95, 0x38, 0x57, 0xae, 0x14, 0x7d, 0x88, 0xab, 0xa8, 0x44, 0xaf,
    0x4a, 0xa5, 0x47, 0x86, 0x8b, 0x30, 0x1b, 0xa6, 0x4d, 0x92, 0x9e, 0xe7, 0x53, 0x6f, 0xe5, 0x7a,
    0x3c, 0xd3, 0x85, 0xe6, 0xdc, 0x69, 0x5c, 0x29, 0x37, 0x2e, 0xf5, 0x28, 0xf4, 0x66, 0x8f, 0x36,
    0x41, 0x19, 0x3f, 0xa1, 0x01, 0xd8, 0x50, 0x49, 0xd1, 0x4c, 0x84, 0xbb, 0xd0, 0x59, 0x12, 0xa9,
    0xc8, 0xc4, 0x87, 0x82, 0x74, 0xbc, 0x9f, 0x56, 0xa4, 0x64, 0x6d, 0xc6, 0xad, 0xba, 0x03, 0x40,
    0x34, 0xd9, 0xe2, 0xfa, 0x7c, 0x7b, 0x05, 0xca, 0x26, 0x93, 0x76, 0x7e, 0xff, 0x52, 0x55, 0xd4,
    0xcf, 0xce, 0x3b, 0xe3, 0x2f, 0x10, 0x3a, 0x11, 0xb6, 0xbd, 0x1c, 0x2a, 0xdf, 0xb7, 0xaa, 0xd5,
    0x77, 0xf8, 0x98, 0x02, 0x2c, 0x9a, 0xa3, 0x46, 0xdd, 0x99, 0x65, 0x9b, 0xa7, 0x2b, 0xac, 0x09,
    0x81, 0x16, 0x27, 0xfd, 0x13, 0x62, 0x6c, 0x6e, 0x4f, 0x71, 0xe0, 0xe8, 0xb2, 0xb9, 0x70, 0x68,
    0xda, 0xf6, 0x61, 0xe4, 0xfb, 0x22, 0xf2, 0xc1, 0xee, 0xd2, 0x90, 0x0c, 0xbf, 0xb3, 0xa2, 0xf1,
    0x51, 0x33, 0x91, 0xeb, 0xf9, 0x0e, 0xef, 0x6b, 0x31, 0xc0, 0xd6, 0x1f, 0xb5, 0xc7, 0x6a, 0x9d,
    0xb8, 0x54, 0xcc, 0xb0, 0x73, 0x79, 0x32, 0x2d, 0x7f, 0x04, 0x96, 0xfe, 0x8a, 0xec, 0xcd, 0x5d,
    0xde, 0x72, 0x43, 0x1d, 0x18, 0x48, 0xf3, 0x8d, 0x80, 0xc3, 0x4e, 0x42, 0xd7, 0x3d, 0x9c, 0xb4,
];

const CHIP_RANDOM_OFFSETS: [f64; 33] = [
    -80.79679107666015,
    -151.77667236328125,
    289.5301513671875,
    133.59625244140625,
    152.01351928710938,
    90.911148071289070,
    -256.3884582519531,
    78.98082733154297,
    230.8242950439453,
    -136.3740234375,
    -38.15315246582031,
    159.8968505859375,
    -13.360940933227539,
    -157.33534240722655,
    -135.05589294433593,
    -84.4410171508789,
    -200.09568786621093,
    -8.089578628540039,
    238.59375,
    245.52487182617186,
    -263.66140747070315,
    242.79183959960937,
    2.713751792907715,
    9.775185585021973,
    -108.58023834228516,
    291.9852600097656,
    -3.613990545272827,
    -140.31329345703125,
    -245.5602569580078,
    268.6585693359375,
    -255.75054931640624,
    0.4242539405822754,
    -69.51470184326172,
];

fn noise2_final(param1: f64, param2: f64) -> f64 {
    let ip1 = if param1 <= 0.0 {
        (param1 as i32) - 1
    } else {
        param1 as i32
    };
    let ip2 = if param2 <= 0.0 {
        (param2 as i32) - 1
    } else {
        param2 as i32
    };
    let fp1 = param1 - ip1 as f64;
    let fp2 = param2 - ip2 as f64;
    let nfp1 = fp1 - 1.0;
    let nfp2 = fp2 - 1.0;

    let u = fp1 * fp1 * fp1 * (fp1 * (fp1 * 6.0 - 15.0) + 10.0);
    let v = fp2 * fp2 * fp2 * (fp2 * (fp2 * 6.0 - 15.0) + 10.0);

    let ix = (ip1 & 0xff) as usize;
    let ix1 = ((ip1 + 1) & 0xff) as usize;
    let iy = (ip2 & 0xff) as usize;
    let iy1 = ((ip2 + 1) & 0xff) as usize;

    let h00 = PERM[PERM[iy] as usize + ix] as u32;
    let h10 = PERM[PERM[iy] as usize + ix1] as u32;
    let h01 = PERM[PERM[iy1] as usize + ix] as u32;
    let h11 = PERM[PERM[iy1] as usize + ix1] as u32;

    fn grad(hash: u32, ox: f64, oy: f64) -> f64 {
        let b4 = (hash & 4) != 0;
        let b2 = (hash & 2) != 0;
        let b1 = (hash & 1) != 0;
        let f4 = if b4 { ox } else { oy };
        let f41 = if b4 { oy } else { ox };
        let f2 = if b2 { -2.0 } else { 2.0 };
        let f1 = if b1 { -f41 } else { f41 };
        f2 * f4 + f1
    }

    let g00 = grad(h00, fp1, fp2);
    let g10 = grad(h10, nfp1, fp2);
    let g01 = grad(h01, fp1, nfp2);
    let g11 = grad(h11, nfp1, nfp2);

    let high_final = g00 + (g01 - g00) * v;
    let low_final = g10 + (g11 - g10) * v;
    let result = high_final + u * (low_final - high_final);

    result * 0.507
}

fn compute_light_positions(chip_rotation: f64) -> [(f64, f64); 11] {
    let mut lights = [(0.0, 0.0); 11];
    let mut offset_index: usize = 0;

    for index in 0..11 {
        let rotation_factor = chip_rotation * 0.05;
        let oi = offset_index as f64;

        let noise1 = noise2_final(
            oi - 8.5,
            CHIP_RANDOM_OFFSETS[offset_index] + rotation_factor,
        );
        let noise2 = noise2_final(
            oi + 1.0 - 8.5,
            CHIP_RANDOM_OFFSETS[offset_index + 1] + rotation_factor,
        );
        let rot = noise2_final(
            oi + 2.0 - 8.5,
            CHIP_RANDOM_OFFSETS[offset_index + 2] + rotation_factor,
        );

        let angle1 = noise1 * 12.56637;
        let rot_angle = rot * 12.56637;
        let angle2_val = noise2 * 12.56637;

        let s1 = angle1.sin();
        let c1 = angle1.cos();
        let s2 = angle2_val.sin();
        let c2 = angle2_val.cos();
        let sr = rot_angle.sin();
        let cr = rot_angle.cos();

        let v12 = sr * (-s2) * s1 + cr * c2;
        let ly = -c1 * sr;

        lights[index] = (v12 * 0.9, ly * 0.9);
        offset_index += 3;
    }
    lights
}

// lights: array<vec4f, 11> (176) + params: vec4f (16) + colors: array<vec4f, 11> (176) = 368
pub(crate) const ILIGHT_BUFFER_SIZE: u64 = 368;

pub(crate) fn pack_ilight_buffer(
    driver: f64,
    power: f32,
    lightness: f32,
    colors: &[[f32; 3]; 11],
) -> Vec<u8> {
    let positions = compute_light_positions(driver);
    let mut bytes = vec![0u8; ILIGHT_BUFFER_SIZE as usize];
    // lights: offset 0, 11 × vec4f
    for (i, &(x, y)) in positions.iter().enumerate() {
        let base = i * 16;
        bytes[base..base + 4].copy_from_slice(&(x as f32).to_ne_bytes());
        bytes[base + 4..base + 8].copy_from_slice(&(y as f32).to_ne_bytes());
    }
    // params: offset 176, vec4f
    let params_base = 11 * 16;
    bytes[params_base..params_base + 4].copy_from_slice(&power.to_ne_bytes());
    bytes[params_base + 4..params_base + 8].copy_from_slice(&lightness.to_ne_bytes());
    // colors: offset 192, 11 × vec4f
    let colors_base = 12 * 16;
    for (i, c) in colors.iter().enumerate() {
        let base = colors_base + i * 16;
        bytes[base..base + 4].copy_from_slice(&c[0].to_ne_bytes());
        bytes[base + 4..base + 8].copy_from_slice(&c[1].to_ne_bytes());
        bytes[base + 8..base + 12].copy_from_slice(&c[2].to_ne_bytes());
        bytes[base + 12..base + 16].copy_from_slice(&1.0f32.to_ne_bytes()); // w = 1.0
    }
    bytes
}

// ── Unit tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Spring physics ─────────────────────────────────────────────

    #[derive(Clone)]
    struct Spring {
        value: f64,
        cur_velocity: f64,
        max_acceleration: f64,
        spring_amount: f64,
    }

    fn update_spring(spring: &mut Spring, target_value: f64) {
        let current_value = spring.value;
        let mut spring_value =
            current_value * (1.0 - spring.spring_amount) + target_value * spring.spring_amount;
        spring_value -= current_value;

        if spring_value != 0.0 {
            let mut velocity = spring.cur_velocity;
            let max_accel = spring.max_acceleration;
            let velocity_diff = spring_value - velocity;

            if velocity_diff <= max_accel {
                if -max_accel <= velocity_diff {
                    velocity += velocity_diff;
                } else {
                    velocity -= max_accel;
                }
            } else {
                velocity += max_accel;
            }

            spring.cur_velocity = velocity;

            if spring_value >= 0.0 {
                if spring_value < velocity {
                    spring.cur_velocity = spring_value;
                    velocity = spring_value;
                }
            } else if velocity < spring_value {
                spring.cur_velocity = spring_value;
                velocity = spring_value;
            }

            spring.value = current_value + velocity;
        }
    }

    struct Physics {
        flame_spring: Spring,
        on_spring: Spring,
        volume_spring: Spring,
        glow_spring: Spring,
        energy_spring: Spring,
        lightness_spring: Spring,
        chip_rotation: f64,
        physics_tick_delta: f64,
        framerate_energy_modifier: f64,
        is_buddy: bool,
        flame_drawn_size: f64,
    }

    fn init_physics() -> Physics {
        Physics {
            flame_spring: Spring {
                value: 0.0,
                cur_velocity: 0.0,
                max_acceleration: 0.017999999225139618,
                spring_amount: 0.19000005722045898,
            },
            on_spring: Spring {
                value: 0.0,
                cur_velocity: 0.0,
                max_acceleration: 0.017999999225139618,
                spring_amount: 0.19000005722045898,
            },
            volume_spring: Spring {
                value: 0.0,
                cur_velocity: 0.0,
                max_acceleration: 0.026999998837709427,
                spring_amount: 0.31937503814697266,
            },
            glow_spring: Spring {
                value: 0.0,
                cur_velocity: 0.0,
                max_acceleration: 0.026999998837709427,
                spring_amount: 0.31937503814697266,
            },
            energy_spring: Spring {
                value: 0.0,
                cur_velocity: 0.0,
                max_acceleration: 0.012000000104308128,
                spring_amount: 0.0975000262260437,
            },
            lightness_spring: Spring {
                value: 5.0,
                cur_velocity: 0.0,
                max_acceleration: 0.008999999612569809,
                spring_amount: 0.05909997224807739,
            },
            chip_rotation: 0.0,
            physics_tick_delta: 0.01666666753590107,
            framerate_energy_modifier: 0.30000001192092896,
            is_buddy: false,
            flame_drawn_size: 0.0,
        }
    }

    fn update_physics_tick(p: &mut Physics) {
        let mic_power_level: f64 = 0.0;
        let reduce_motion = false;
        let mut target_value = mic_power_level * mic_power_level * 0.7 + 0.7;
        if reduce_motion {
            target_value = 0.7;
        }

        update_spring(&mut p.flame_spring, target_value);
        update_spring(&mut p.on_spring, 1.0);

        let physics_delta = p.physics_tick_delta;
        p.flame_drawn_size = p.flame_spring.value * 0.07;

        update_spring(&mut p.volume_spring, mic_power_level);

        if p.is_buddy {
            update_spring(&mut p.energy_spring, 0.0);
            update_spring(&mut p.lightness_spring, p.energy_spring.value);
            return;
        }

        let mut mic_pl = 2.5_f64;
        if reduce_motion {
            mic_pl = 0.3;
        }
        let tv = p.on_spring.cur_velocity;
        mic_pl = physics_delta * mic_pl * p.volume_spring.value;
        let mut energy = tv * 20.0;
        if tv < 0.0 {
            energy = 0.0;
        }
        target_value = physics_delta * 25.0;
        if mic_pl <= physics_delta * 25.0 {
            target_value = mic_pl;
        }
        energy = energy * p.framerate_energy_modifier + p.volume_spring.value;
        mic_pl = energy.min(1.3);

        update_spring(&mut p.glow_spring, mic_pl);

        target_value = target_value * 0.5 + physics_delta * 0.7;
        if reduce_motion {
            mic_pl = target_value * 0.4;
            target_value = physics_delta * 0.6;
            if mic_pl <= physics_delta * 0.6 {
                target_value = mic_pl;
            }
        }

        p.chip_rotation += target_value;
    }

    fn update_physics(p: &mut Physics) {
        update_physics_tick(p);
    }

    fn assert_near(actual: f64, expected: f64, tol: f64, label: &str) {
        let diff = (actual - expected).abs();
        assert!(
            diff < tol,
            "{label}: expected {expected}, got {actual} (diff {diff})"
        );
    }

    #[test]
    fn test_light_positions_frame1() {
        let mut p = init_physics();
        update_physics(&mut p);
        let lights = compute_light_positions(p.chip_rotation);

        let expected = [
            (-0.2919275650470243, -0.4486050784264397),
            (0.898848354232521, 0.04532887927271878),
            (0.8752541305293285, 0.13678240081213974),
            (0.8852328430456297, 0.0),
            (0.019847327431506686, -0.05116867006272884),
            (-0.47634631730450033, 0.6549601430462291),
            (-0.7476473511300574, 0.4851090368675278),
            (-0.6432178072207827, -0.04591841476865834),
            (-0.614120224639545, -0.6423043628773332),
            (-0.19981256104335915, -0.8767988172899388),
            (-0.02418890604770534, -0.8459559146135601),
        ];

        for (i, &(ex, ey)) in expected.iter().enumerate() {
            assert_near(lights[i].0, ex, 1e-4, &format!("light[{i}].x frame1"));
            assert_near(lights[i].1, ey, 1e-4, &format!("light[{i}].y frame1"));
        }
    }

    #[test]
    fn test_light_positions_frame60() {
        let mut p = init_physics();
        for _ in 0..60 {
            update_physics(&mut p);
        }

        assert_near(p.chip_rotation, 0.7000000365078457, 1e-6, "chipRotation");

        let lights = compute_light_positions(p.chip_rotation);

        let expected = [
            (-0.5087176156915038, -0.4941985989903448),
            (0.8999109816561935, 0.01143782030781697),
            (0.8988271615979994, -0.010689945216479908),
            (0.8960521798711923, 0.0),
            (0.3466893107119007, 0.006794413567230739),
            (-0.09778919194654541, 0.8807210653856001),
            (-0.4712201110731609, 0.6871260937133429),
            (-0.8048004222954347, -0.05884622657382343),
            (-0.06744793309390501, -0.8072108627689244),
            (-0.1292813840760012, -0.871219374375766),
            (-0.4098698634370342, -0.789759907376821),
        ];

        for (i, &(ex, ey)) in expected.iter().enumerate() {
            assert_near(lights[i].0, ex, 1e-4, &format!("light[{i}].x frame60"));
            assert_near(lights[i].1, ey, 1e-4, &format!("light[{i}].y frame60"));
        }
    }

    #[test]
    fn test_spring_physics_60_frames() {
        let mut p = init_physics();

        for f in 1..=60 {
            update_physics(&mut p);

            if f == 10 {
                assert_near(p.chip_rotation, 0.11666667275130746, 1e-6, "chipRot@10");
                assert_near(p.flame_spring.value, 0.5500683196621298, 1e-6, "flame@10");
                assert_near(p.on_spring.value, 0.7322494640337356, 1e-6, "onSpring@10");
            }
            if f == 30 {
                assert_near(p.chip_rotation, 0.3500000182539226, 1e-6, "chipRot@30");
                assert_near(p.flame_spring.value, 0.697783880514762, 1e-6, "flame@30");
            }
        }

        assert_near(p.chip_rotation, 0.7000000365078457, 1e-6, "chipRot@60");
        assert_near(p.flame_spring.value, 0.6999960176188991, 1e-6, "flame@60");
        assert_near(p.on_spring.value, 0.9999928881963317, 1e-6, "onSpring@60");
        assert_near(p.lightness_spring.value, 5.0, 1e-6, "lightness@60");
    }
}
