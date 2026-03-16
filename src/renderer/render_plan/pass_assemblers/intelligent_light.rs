//! IntelligentLight pass assembler.
//!
//! Fullscreen procedural fragment shader simulating a physical spring-powered
//! intelligent lighting fixture with 11 color zones. All inputs (11 colors +
//! 1 driver float) are resolved on the CPU and baked into WGSL constants.

use anyhow::{Context, Result};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color},
};

use crate::{
    dsl::{incoming_connection, Node},
    renderer::{
        camera::legacy_projection_camera_matrix,
        types::PassOutputSpec,
        utils::{cpu_num_f32, fmt_f32},
        wgsl::build_fullscreen_textured_bundle,
    },
};

use super::super::pass_spec::{
    PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl, make_params,
};
use super::args::{BuilderState, SceneContext};

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
            .with_context(|| {
                format!(
                    "invalid blend params for IntelligentLight {}",
                    layer_id
                )
            })?;

    // ── Resolve inputs ──────────────────────────────────────────────────

    let driver = cpu_num_f32(scene, &nodes_by_id, layer_node, "driver", 0.0)?;

    let mut colors: [[f32; 3]; 11] = [[1.0, 1.0, 1.0]; 11];
    for i in 0..11 {
        let port_id = format!("color{i}");
        colors[i] = resolve_color_input(sc, layer_node, layer_id, &port_id);
    }

    // ── Output texture ──────────────────────────────────────────────────

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

    let output_blend = if output_tex == *bs.target_texture_name {
        pass_blend_state
    } else {
        BlendState::REPLACE
    };

    // ── Fullscreen geometry ─────────────────────────────────────────────

    let geo: ResourceName = format!("sys.ilight.{layer_id}.geo").into();
    bs.push_fullscreen_geometry(geo.clone(), tgt_w, tgt_h);

    // ── Params ──────────────────────────────────────────────────────────

    let params_name: ResourceName = format!("params.sys.ilight.{layer_id}").into();
    let camera = legacy_projection_camera_matrix([tgt_w, tgt_h]);
    let params_val = make_params(
        [tgt_w, tgt_h],
        [tgt_w, tgt_h],
        [tgt_w * 0.5, tgt_h * 0.5],
        camera,
        [0.0, 0.0, 0.0, 0.0],
    );

    // ── WGSL shader ─────────────────────────────────────────────────────

    let pass_name: ResourceName = format!("sys.ilight.{layer_id}.pass").into();
    let shader_wgsl = build_intelligent_light_wgsl(&colors, driver);

    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: pass_name.as_str().to_string(),
        name: pass_name.clone(),
        geometry_buffer: geo.clone(),
        instance_buffer: None,
        normals_buffer: None,
        target_texture: output_tex.clone(),
        resolve_target: None,
        params_buffer: params_name,
        baked_data_parse_buffer: None,
        params: params_val,
        graph_binding: None,
        graph_values: None,
        shader_wgsl,
        texture_bindings: Vec::new(),
        sampler_kinds: Vec::new(),
        blend_state: output_blend,
        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
        sample_count: 1,
    });
    bs.composite_passes.push(pass_name);

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
            if writes_scene_output_target
                && comp_ctx.target_texture_name == *bs.target_texture_name
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

fn build_intelligent_light_wgsl(colors: &[[f32; 3]; 11], driver: f32) -> String {
    let mut color_consts = String::new();
    for (i, c) in colors.iter().enumerate() {
        color_consts.push_str(&format!(
            "const LIGHT_COLOR_{i}: vec3f = vec3f({}, {}, {});\n",
            fmt_f32(c[0]),
            fmt_f32(c[1]),
            fmt_f32(c[2])
        ));
    }

    format!(
        r#"// ── IntelligentLight pass ───────────────────────────────────────────

struct Params {{
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
}};

@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {{
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    @location(1) frag_coord_gl: vec2f,
    @location(2) local_px: vec3f,
    @location(3) geo_size_px: vec2f,
}};

@vertex
fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {{
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
}}

// ── Constants ────────────────────────────────────────────────────────

const DRIVER: f32 = {driver};
const NUM_LIGHTS: u32 = 11u;
const TAU: f32 = 6.283185307;
const BASE_COLOR: vec3f = vec3f(0.0, 0.5884, 1.0);

{color_consts}
// 33 random offsets (3 per light) from the original fixture definition.
const CHIP_OFFSETS: array<f32, 33> = array<f32, 33>(
    -256.0, 291.0, -128.0,
     64.0, -192.0, 256.0,
    128.0,  -64.0, 192.0,
   -256.0,   64.0, -128.0,
    256.0, -192.0, 128.0,
    -64.0,  192.0, -256.0,
     64.0, -128.0, 256.0,
   -192.0,  128.0,  -64.0,
    192.0, -256.0,   64.0,
   -128.0,  256.0, -192.0,
    128.0,  -64.0, 192.0
);

// ── Noise ────────────────────────────────────────────────────────────

fn hash21(p: vec2f) -> f32 {{
    var h = dot(p, vec2f(127.1, 311.7));
    return fract(sin(h) * 43758.5453123);
}}

fn value_noise(x: f32, y: f32) -> f32 {{
    let p = vec2f(x, y);
    let i = floor(p);
    let f = fract(p);

    let a = hash21(i);
    let b = hash21(i + vec2f(1.0, 0.0));
    let c = hash21(i + vec2f(0.0, 1.0));
    let d = hash21(i + vec2f(1.0, 1.0));

    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}}

// ── Light position from noise ────────────────────────────────────────

fn light_position(light_index: u32) -> vec2f {{
    let base = light_index * 3u;
    let idx_f = f32(light_index);

    let offset0 = CHIP_OFFSETS[base];
    let offset1 = CHIP_OFFSETS[base + 1u];
    let offset2 = CHIP_OFFSETS[base + 2u];

    let speed = DRIVER * 0.05;

    let angle1 = value_noise(idx_f - 8.5, offset0 + speed) * TAU;
    let angle2 = value_noise(idx_f + 1.0 - 8.5, offset1 + speed) * TAU;
    let rot    = value_noise(idx_f + 2.0 - 8.5, offset2 + speed);

    let rot_angle = rot * TAU;
    let sr = sin(rot_angle);
    let cr = cos(rot_angle);

    let lx = sr * (-cos(angle2)) * sin(angle1) + cr * sin(angle2);
    let ly = -sin(angle1) * sr;

    return vec2f(lx, ly) * 0.8;
}}

// ── Get light color by index ─────────────────────────────────────────

fn get_light_color(i: u32) -> vec3f {{
    switch i {{
        case 0u  {{ return LIGHT_COLOR_0; }}
        case 1u  {{ return LIGHT_COLOR_1; }}
        case 2u  {{ return LIGHT_COLOR_2; }}
        case 3u  {{ return LIGHT_COLOR_3; }}
        case 4u  {{ return LIGHT_COLOR_4; }}
        case 5u  {{ return LIGHT_COLOR_5; }}
        case 6u  {{ return LIGHT_COLOR_6; }}
        case 7u  {{ return LIGHT_COLOR_7; }}
        case 8u  {{ return LIGHT_COLOR_8; }}
        case 9u  {{ return LIGHT_COLOR_9; }}
        case 10u {{ return LIGHT_COLOR_10; }}
        default  {{ return vec3f(1.0); }}
    }}
}}

// ── Fragment shader ──────────────────────────────────────────────────

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {{
    let aspect = params.target_size.x / params.target_size.y;
    var uv = in.uv * 2.0 - 1.0;
    uv.x *= aspect;

    var accum = vec3f(0.0);

    for (var i = 0u; i < NUM_LIGHTS; i = i + 1u) {{
        let lpos = light_position(i);
        let d = distance(uv, lpos);
        let falloff = smoothstep(0.0, 1.0, clamp(1.0 - d, 0.0, 1.0));
        let col = get_light_color(i);
        accum += col * falloff;
    }}

    var result = BASE_COLOR + accum;

    let power = clamp(DRIVER, 0.0, 1.0);
    let brightness = 1.0 + power * 0.2;
    let lum = dot(result, vec3f(0.2126, 0.7152, 0.0722));
    result = mix(result, result * brightness, clamp(lum, 0.0, 1.0));

    let lightness = clamp(DRIVER, 0.0, 1.0);
    let scale = mix(0.75, 0.775, lightness);
    result *= scale;

    return vec4f(clamp(result, vec3f(0.0), vec3f(1.0)), 1.0);
}}
"#,
        driver = fmt_f32(driver),
        color_consts = color_consts,
    )
}
