//! IntelligentLight pass assembler.
//!
//! Fullscreen shader for an 11-zone intelligent lighting fixture. Geometry is
//! supplied as packed positions or through the explicit manual zone ports.

use anyhow::{Context, Result, bail};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, BlendState, Color},
};

use crate::{
    dsl::{Node, incoming_connection},
    renderer::{
        camera::{legacy_projection_camera_matrix, resolve_effective_camera_for_pass_node},
        types::{GraphBinding, GraphBindingKind, GraphSchema, PassExtension, PassOutputSpec},
        utils::{cpu_num_f32, cpu_num_u32_min_1},
        wgsl::build_fullscreen_textured_bundle,
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
    pub power_fallback: f32,
    pub lightness_fallback: f32,
}

pub(crate) const INTELLIGENT_LIGHT_ZONE_COUNT: usize = 11;
const INTELLIGENT_LIGHT_TEMPLATE_NAME: &str = "intelligent_light.wgsl";

pub(crate) const DEFAULT_INTELLIGENT_LIGHT_LAYOUT: [[f32; 2]; INTELLIGENT_LIGHT_ZONE_COUNT] = [
    [0.217379, 0.225445],
    [0.999951, 0.506354],
    [0.999348, 0.494061],
    [0.997807, 0.5],
    [0.692605, 0.503775],
    [0.445673, 0.989289],
    [0.238211, 0.881737],
    [0.052889, 0.467308],
    [0.462529, 0.05155],
    [0.428177, 0.015989],
    [0.272295, 0.061244],
];

const DEFAULT_INTELLIGENT_LIGHT_TARGET_SIZE: [f32; 2] = [60.0, 37.0];

pub(crate) const DEFAULT_INTELLIGENT_LIGHT_COLORS: [[f32; 3]; INTELLIGENT_LIGHT_ZONE_COUNT] = [
    [0.5019608, 0.5254902, 1.0],
    [1.0, 0.827451, 0.7019608],
    [1.0, 0.5254902, 0.20784314],
    [0.5176471, 0.49411765, 1.0],
    [0.07058824, 0.4117647, 0.9490196],
    [0.5019608, 0.5254902, 1.0],
    [1.0, 0.827451, 0.7019608],
    [1.0, 0.5254902, 0.20784314],
    [1.0, 0.5254902, 0.20784314],
    [0.07058824, 0.4117647, 0.9490196],
    [0.5176471, 0.49411765, 1.0],
];

impl ILightUpdateConfig {
    pub fn pack_buffer(&self, scene: &crate::dsl::SceneDSL) -> Vec<u8> {
        let nodes_by_id: std::collections::HashMap<&str, &crate::dsl::Node> =
            scene.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        let layer_node = nodes_by_id.get(self.layer_id.as_str()).copied();

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

        let packed = layer_node.and_then(|node| match resolve_packed_pair(scene, node) {
            Ok(value) => value,
            Err(error) => {
                eprintln!(
                    "[IntelligentLight] rejected packed inputs for '{}': {error:#}",
                    self.layer_id
                );
                None
            }
        });
        let (positions, colors) = if let Some(packed) = packed {
            packed
        } else {
            let mut colors = DEFAULT_INTELLIGENT_LIGHT_COLORS;
            for i in 0..INTELLIGENT_LIGHT_ZONE_COUNT {
                let port_id = format!("color{i}");
                colors[i] = resolve_color_runtime(scene, &nodes_by_id, &self.layer_id, &port_id)
                    .unwrap_or(DEFAULT_INTELLIGENT_LIGHT_COLORS[i]);
            }
            let positions = layer_node
                .map(|node| {
                    let target_size = [
                        node.params
                            .get("width")
                            .and_then(json_f32)
                            .unwrap_or(DEFAULT_INTELLIGENT_LIGHT_TARGET_SIZE[0])
                            .max(1.0),
                        node.params
                            .get("height")
                            .and_then(json_f32)
                            .unwrap_or(DEFAULT_INTELLIGENT_LIGHT_TARGET_SIZE[1])
                            .max(1.0),
                    ];
                    resolve_light_positions(node, target_size, |port_id| {
                        incoming_connection(scene, &self.layer_id, port_id).and_then(|conn| {
                            nodes_by_id
                                .get(conn.from.node_id.as_str())
                                .and_then(|upstream| {
                                    resolve_connected_vec2_source(upstream, &conn.from.port_id)
                                })
                        })
                    })
                })
                .unwrap_or_else(|| {
                    std::array::from_fn(|index| {
                        let [x, y] =
                            default_light_position(index, DEFAULT_INTELLIGENT_LIGHT_TARGET_SIZE);
                        (x, y)
                    })
                });
            (positions, colors)
        };

        pack_ilight_buffer(&positions, power, lightness, &colors)
    }
}

fn clamp_pixel_position(position: [f32; 2], target_size: [f32; 2]) -> [f32; 2] {
    [
        position[0].clamp(0.0, target_size[0].max(1.0)),
        position[1].clamp(0.0, target_size[1].max(1.0)),
    ]
}

fn is_legacy_normalized_position(position: [f32; 2]) -> bool {
    (0.0..=1.0).contains(&position[0]) && (0.0..=1.0).contains(&position[1])
}

fn normalized_position_to_pixel_space(position: [f32; 2], target_size: [f32; 2]) -> [f32; 2] {
    [
        position[0].clamp(0.0, 1.0) * target_size[0].max(1.0),
        (1.0 - position[1].clamp(0.0, 1.0)) * target_size[1].max(1.0),
    ]
}

fn resolve_pixel_position(position: [f32; 2], target_size: [f32; 2]) -> [f32; 2] {
    let pixel = if is_legacy_normalized_position(position) {
        normalized_position_to_pixel_space(position, target_size)
    } else {
        position
    };
    clamp_pixel_position(pixel, target_size)
}

fn parse_packed_positions(
    value: &serde_json::Value,
) -> Result<[(f32, f32); INTELLIGENT_LIGHT_ZONE_COUNT]> {
    let values = value
        .as_array()
        .context("positions must be packed<vector2>")?;
    if values.len() != INTELLIGENT_LIGHT_ZONE_COUNT {
        bail!("positions must contain exactly {INTELLIGENT_LIGHT_ZONE_COUNT} values");
    }
    let positions = values
        .iter()
        .enumerate()
        .map(|(index, value)| -> Result<(f32, f32)> {
            let value = value
                .as_array()
                .with_context(|| format!("positions[{index}] must be vector2"))?;
            if value.len() != 2 {
                bail!("positions[{index}] must contain exactly 2 components");
            }
            let pixel = [
                json_f32(&value[0]).with_context(|| format!("positions[{index}].x is invalid"))?,
                json_f32(&value[1]).with_context(|| format!("positions[{index}].y is invalid"))?,
            ];
            if !pixel.iter().all(|component| component.is_finite()) {
                bail!("positions[{index}] contains a non-finite component");
            }
            Ok((pixel[0], pixel[1]))
        })
        .collect::<Result<Vec<_>>>()?;
    positions
        .try_into()
        .map_err(|_| anyhow::anyhow!("positions length changed during validation"))
}

fn parse_packed_colors(
    value: &serde_json::Value,
) -> Result<[[f32; 3]; INTELLIGENT_LIGHT_ZONE_COUNT]> {
    let values = value.as_array().context("colors must be packed<color>")?;
    if values.len() != INTELLIGENT_LIGHT_ZONE_COUNT {
        bail!("colors must contain exactly {INTELLIGENT_LIGHT_ZONE_COUNT} values");
    }
    let colors = values
        .iter()
        .enumerate()
        .map(|(index, value)| -> Result<[f32; 3]> {
            let value = value
                .as_array()
                .with_context(|| format!("colors[{index}] must be color"))?;
            if value.len() != 3 && value.len() != 4 {
                bail!("colors[{index}] must contain 3 or 4 components");
            }
            let color = [
                json_f32(&value[0]).with_context(|| format!("colors[{index}].r is invalid"))?,
                json_f32(&value[1]).with_context(|| format!("colors[{index}].g is invalid"))?,
                json_f32(&value[2]).with_context(|| format!("colors[{index}].b is invalid"))?,
            ];
            if !color.iter().all(|component| component.is_finite()) {
                bail!("colors[{index}] contains a non-finite component");
            }
            Ok(color)
        })
        .collect::<Result<Vec<_>>>()?;
    colors
        .try_into()
        .map_err(|_| anyhow::anyhow!("colors length changed during validation"))
}

fn packed_input_value(scene: &crate::dsl::SceneDSL, node: &Node) -> serde_json::Value {
    if let Some(value) = node.params.get("value") {
        return value.clone();
    }
    let nodes_by_id = scene
        .nodes
        .iter()
        .map(|candidate| (candidate.id.as_str(), candidate))
        .collect::<std::collections::HashMap<_, _>>();
    serde_json::Value::Array(
        node.inputs
            .iter()
            .map(|input| {
                let source = incoming_connection(scene, &node.id, &input.id)
                    .and_then(|connection| nodes_by_id.get(connection.from.node_id.as_str()))
                    .copied();
                let Some(source) = source else {
                    return serde_json::Value::Null;
                };
                match source.node_type.as_str() {
                    "Vector2Input" => serde_json::json!([
                        source.params.get("x").and_then(json_f32).unwrap_or(0.0),
                        source.params.get("y").and_then(json_f32).unwrap_or(0.0)
                    ]),
                    "ColorInput" => source
                        .params
                        .get("value")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([1.0, 0.0, 1.0, 1.0])),
                    _ => source
                        .params
                        .get("value")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                }
            })
            .collect(),
    )
}

fn resolve_packed_pair(
    scene: &crate::dsl::SceneDSL,
    node: &Node,
) -> Result<
    Option<(
        [(f32, f32); INTELLIGENT_LIGHT_ZONE_COUNT],
        [[f32; 3]; INTELLIGENT_LIGHT_ZONE_COUNT],
    )>,
> {
    let positions_connection = incoming_connection(scene, &node.id, "positions");
    let colors_connection = incoming_connection(scene, &node.id, "colors");
    match (positions_connection, colors_connection) {
        (None, None) => Ok(None),
        (Some(positions_connection), Some(colors_connection)) => {
            let positions_node = scene
                .nodes
                .iter()
                .find(|candidate| candidate.id == positions_connection.from.node_id)
                .context("positions packed source node is missing")?;
            let colors_node = scene
                .nodes
                .iter()
                .find(|candidate| candidate.id == colors_connection.from.node_id)
                .context("colors packed source node is missing")?;
            if positions_node.node_type != "PackedInput"
                || colors_node.node_type != "PackedInput"
                || positions_connection.from.port_id != "value"
                || colors_connection.from.port_id != "value"
            {
                bail!("packed mode requires PackedInput.value sources");
            }
            let positions_type = positions_node
                .params
                .get("elementType")
                .and_then(serde_json::Value::as_str);
            let colors_type = colors_node
                .params
                .get("elementType")
                .and_then(serde_json::Value::as_str);
            if positions_type != Some("vector2") || colors_type != Some("color") {
                bail!("positions requires packed<vector2> and colors requires packed<color>");
            }
            Ok(Some((
                parse_packed_positions(&packed_input_value(scene, positions_node))?,
                parse_packed_colors(&packed_input_value(scene, colors_node))?,
            )))
        }
        _ => bail!("packed mode requires both positions and colors"),
    }
}

pub(crate) fn default_light_position(index: usize, target_size: [f32; 2]) -> [f32; 2] {
    let source = DEFAULT_INTELLIGENT_LIGHT_LAYOUT
        .get(index)
        .copied()
        .unwrap_or([0.5, 0.5]);
    normalized_position_to_pixel_space(source, target_size)
}

fn resolve_connected_vec2_source(node: &Node, output_port_id: &str) -> Option<[f32; 2]> {
    if node.node_type == "Vector2Input" {
        return Some([
            node.params.get("x").and_then(json_f32).unwrap_or(0.0),
            node.params.get("y").and_then(json_f32).unwrap_or(0.0),
        ]);
    }

    parse_vec2_from_params(&node.params, "value")
        .or_else(|| parse_vec2_from_params(&node.params, output_port_id))
}

fn resolve_light_positions<F>(
    layer_node: &Node,
    target_size: [f32; 2],
    mut resolve_connected_position: F,
) -> [(f32, f32); INTELLIGENT_LIGHT_ZONE_COUNT]
where
    F: FnMut(&str) -> Option<[f32; 2]>,
{
    let mut positions = [(0.0f32, 0.0f32); INTELLIGENT_LIGHT_ZONE_COUNT];
    for index in 0..INTELLIGENT_LIGHT_ZONE_COUNT {
        let port_id = format!("pos{index}");
        let pixel = resolve_connected_position(port_id.as_str())
            .or_else(|| parse_vec2_from_params(&layer_node.params, port_id.as_str()))
            .map(|position| resolve_pixel_position(position, target_size))
            .unwrap_or_else(|| default_light_position(index, target_size));
        positions[index] = (pixel[0], pixel[1]);
    }
    positions
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
    port_id
        .strip_prefix("color")
        .and_then(|suffix| suffix.parse::<usize>().ok())
        .and_then(|index| DEFAULT_INTELLIGENT_LIGHT_COLORS.get(index).copied())
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
    let pass_blend_state =
        crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
            .with_context(|| format!("invalid blend params for IntelligentLight {}", layer_id))?;

    // ── Resolve inputs ──────────────────────────────────────────────────

    let power = cpu_num_f32(scene, &nodes_by_id, layer_node, "power", 0.0)?;
    let lightness = cpu_num_f32(scene, &nodes_by_id, layer_node, "lightness", 0.75)?;

    let inter_w = cpu_num_u32_min_1(scene, &nodes_by_id, layer_node, "width", 60)?;
    let inter_h = cpu_num_u32_min_1(scene, &nodes_by_id, layer_node, "height", 37)?;
    let inter_w_f = inter_w as f32;
    let inter_h_f = inter_h as f32;

    let (positions, colors) = if let Some(packed) = resolve_packed_pair(scene, layer_node)
        .with_context(|| format!("invalid packed inputs for IntelligentLight {layer_id}"))?
    {
        packed
    } else {
        let mut colors = DEFAULT_INTELLIGENT_LIGHT_COLORS;
        for i in 0..INTELLIGENT_LIGHT_ZONE_COUNT {
            let port_id = format!("color{i}");
            colors[i] = resolve_color_input(sc, layer_node, layer_id, &port_id);
        }
        let positions = resolve_light_positions(layer_node, [inter_w_f, inter_h_f], |port_id| {
            incoming_connection(scene, layer_id, port_id).and_then(|conn| {
                nodes_by_id
                    .get(conn.from.node_id.as_str())
                    .and_then(|upstream| {
                        resolve_connected_vec2_source(upstream, &conn.from.port_id)
                    })
            })
        });
        (positions, colors)
    };

    // ── Intermediate texture (low-res render) ─────────────────────────

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
    let camera = resolve_effective_camera_for_pass_node(
        scene,
        &nodes_by_id,
        layer_node,
        [inter_w_f, inter_h_f],
    )
    .with_context(|| format!("failed to resolve camera for IntelligentLight {layer_id}"))?;
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
    let ilight_values = pack_ilight_buffer(&positions, power, lightness, &colors);
    let ilight_config = ILightUpdateConfig {
        layer_id: layer_id.to_string(),
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

    let shader_wgsl = build_intelligent_light_wgsl(layer_node);

    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: pass_name.as_str().to_string(),
        name: pass_name.clone(),
        geometry_buffer: geo.clone(),
        instance_buffer: None,
        normals_buffer: None,
        vertex_layout: Default::default(),
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

    // ── Canonical output ────────────────────────────────────────────────
    //
    // IntelligentLight's natural output is its low-resolution computation
    // texture. Presentation scaling belongs to each explicit Composite edge.

    bs.pass_output_registry.register(PassOutputSpec {
        endpoint: crate::renderer::types::OutputEndpoint::new(layer_id, "pass"),
        texture_name: inter_tex.clone(),
        resolution: [inter_w, inter_h],
        format: bs.sampled_pass_format,
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
                vertex_layout: Default::default(),
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
                    texture: inter_tex.clone(),
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
/// Precedence: incoming connection → node params → node default.
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

    port_id
        .strip_prefix("color")
        .and_then(|suffix| suffix.parse::<usize>().ok())
        .and_then(|index| DEFAULT_INTELLIGENT_LIGHT_COLORS.get(index).copied())
        .unwrap_or([1.0, 1.0, 1.0])
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

fn parse_vec2_from_params(
    params: &std::collections::HashMap<String, serde_json::Value>,
    key: &str,
) -> Option<[f32; 2]> {
    let value = params.get(key)?;
    if let Some(arr) = value.as_array() {
        return Some([
            arr.first().and_then(json_f32).unwrap_or(0.0),
            arr.get(1).and_then(json_f32).unwrap_or(0.0),
        ]);
    }
    if let Some(obj) = value.as_object() {
        return Some([
            obj.get("x").and_then(json_f32).unwrap_or(0.0),
            obj.get("y").and_then(json_f32).unwrap_or(0.0),
        ]);
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

fn intelligent_light_override_path(node: &Node) -> Option<std::path::PathBuf> {
    node.wgsl_override
        .as_deref()
        .and_then(crate::renderer::node_compiler::template_loader::resolve_override_path)
}

pub(crate) fn build_intelligent_light_wgsl(node: &Node) -> String {
    let path = intelligent_light_override_path(node);
    crate::renderer::node_compiler::template_loader::load_template_with_override(
        path.as_deref(),
        INTELLIGENT_LIGHT_TEMPLATE_NAME,
    )
}

// lights: array<vec4f, 11> (176) + params: vec4f (16) + colors: array<vec4f, 11> (176) = 368
pub(crate) const ILIGHT_BUFFER_SIZE: u64 = 368;

pub(crate) fn pack_ilight_buffer(
    positions: &[(f32, f32); INTELLIGENT_LIGHT_ZONE_COUNT],
    power: f32,
    lightness: f32,
    colors: &[[f32; 3]; INTELLIGENT_LIGHT_ZONE_COUNT],
) -> Vec<u8> {
    let mut bytes = vec![0u8; ILIGHT_BUFFER_SIZE as usize];
    // lights: offset 0, 11 × vec4f
    for (i, &(x, y)) in positions.iter().enumerate() {
        let base = i * 16;
        bytes[base..base + 4].copy_from_slice(&x.to_ne_bytes());
        bytes[base + 4..base + 8].copy_from_slice(&y.to_ne_bytes());
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
    use std::collections::HashMap;

    use crate::dsl::{Connection, Endpoint, Metadata, Node, SceneDSL};

    fn test_node(
        node_id: &str,
        node_type: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> Node {
        Node {
            id: node_id.to_string(),
            node_type: node_type.to_string(),
            params,
            inputs: vec![],
            outputs: vec![],
            input_bindings: vec![],
            wgsl_override: None,
        }
    }

    #[test]
    fn intelligent_light_shader_uses_template_file() {
        let shader =
            build_intelligent_light_wgsl(&test_node("ilight", "IntelligentLight", HashMap::new()));
        let template = crate::renderer::node_compiler::template_loader::load_template(
            INTELLIGENT_LIGHT_TEMPLATE_NAME,
        );

        assert_eq!(shader, template);
    }

    fn assert_near(actual: f64, expected: f64, tol: f64, label: &str) {
        let diff = (actual - expected).abs();
        assert!(
            diff < tol,
            "{label}: expected {expected}, got {actual} (diff {diff})"
        );
    }

    fn read_light_xy(bytes: &[u8], index: usize) -> (f32, f32) {
        let base = index * 16;
        let x = f32::from_ne_bytes(bytes[base..base + 4].try_into().unwrap());
        let y = f32::from_ne_bytes(bytes[base + 4..base + 8].try_into().unwrap());
        (x, y)
    }

    fn read_light_color(bytes: &[u8], index: usize) -> (f32, f32, f32) {
        let base = (12 * 16) + index * 16;
        let r = f32::from_ne_bytes(bytes[base..base + 4].try_into().unwrap());
        let g = f32::from_ne_bytes(bytes[base + 4..base + 8].try_into().unwrap());
        let b = f32::from_ne_bytes(bytes[base + 8..base + 12].try_into().unwrap());
        (r, g, b)
    }

    fn make_test_scene(
        layer_params: serde_json::Map<String, serde_json::Value>,
        extra_nodes: Vec<Node>,
        connections: Vec<Connection>,
    ) -> SceneDSL {
        let mut nodes = vec![Node {
            id: "ilight".to_string(),
            node_type: "IntelligentLight".to_string(),
            params: layer_params.into_iter().collect(),
            inputs: vec![],
            outputs: vec![],
            input_bindings: vec![],
            wgsl_override: None,
        }];
        nodes.extend(extra_nodes);

        SceneDSL {
            version: "1".to_string(),
            metadata: Metadata {
                name: "ilight".to_string(),
                created: None,
                modified: None,
            },
            nodes,
            connections,
            outputs: None,
            groups: vec![],
            assets: HashMap::new(),
            state_machine: None,
            debug_artifacts: None,
        }
    }

    #[test]
    fn pack_buffer_uses_manual_layout_positions() {
        let scene = make_test_scene(
            serde_json::json!({
                "pos0": [0.25, 0.75],
            })
            .as_object()
            .unwrap()
            .clone(),
            Vec::new(),
            Vec::new(),
        );

        let cfg = ILightUpdateConfig {
            layer_id: "ilight".to_string(),
            power_fallback: 0.0,
            lightness_fallback: 0.75,
        };

        let bytes = cfg.pack_buffer(&scene);
        let (x0, y0) = read_light_xy(&bytes, 0);
        let (x1, y1) = read_light_xy(&bytes, 1);

        assert_near(x0 as f64, 15.0, 1e-6, "manual light[0].x");
        assert_near(y0 as f64, 9.25, 1e-6, "manual light[0].y");

        let [expected_x1, expected_y1] = default_light_position(1, [60.0, 37.0]);
        assert_near(x1 as f64, expected_x1 as f64, 1e-6, "default light[1].x");
        assert_near(y1 as f64, expected_y1 as f64, 1e-6, "default light[1].y");
    }

    #[test]
    fn pack_buffer_manual_layout_uses_connected_vector2_input() {
        let vector_node = Node {
            id: "vec".to_string(),
            node_type: "Vector2Input".to_string(),
            params: serde_json::json!({
                "x": 0.1,
                "y": 0.9,
            })
            .as_object()
            .unwrap()
            .clone()
            .into_iter()
            .collect(),
            inputs: vec![],
            outputs: vec![],
            input_bindings: vec![],
            wgsl_override: None,
        };
        let scene = make_test_scene(
            serde_json::json!({
                "pos0": [0.25, 0.75],
            })
            .as_object()
            .unwrap()
            .clone(),
            vec![vector_node],
            vec![Connection {
                id: "vec-to-ilight".to_string(),
                from: Endpoint {
                    node_id: "vec".to_string(),
                    port_id: "vector".to_string(),
                },
                to: Endpoint {
                    node_id: "ilight".to_string(),
                    port_id: "pos0".to_string(),
                },
            }],
        );

        let cfg = ILightUpdateConfig {
            layer_id: "ilight".to_string(),
            power_fallback: 0.0,
            lightness_fallback: 0.75,
        };

        let bytes = cfg.pack_buffer(&scene);
        let (x0, y0) = read_light_xy(&bytes, 0);

        assert_near(x0 as f64, 6.0, 1e-6, "connected manual light[0].x");
        assert_near(y0 as f64, 3.7, 1e-6, "connected manual light[0].y");
    }

    #[test]
    fn pack_buffer_uses_local_color_params() {
        let scene = make_test_scene(
            serde_json::json!({
                "color0": "#44cc88",
            })
            .as_object()
            .unwrap()
            .clone(),
            Vec::new(),
            Vec::new(),
        );

        let cfg = ILightUpdateConfig {
            layer_id: "ilight".to_string(),
            power_fallback: 0.0,
            lightness_fallback: 0.75,
        };

        let bytes = cfg.pack_buffer(&scene);
        let (r0, g0, b0) = read_light_color(&bytes, 0);

        assert_near(r0 as f64, 0x44 as f64 / 255.0, 1e-6, "local color0.r");
        assert_near(g0 as f64, 0xcc as f64 / 255.0, 1e-6, "local color0.g");
        assert_near(b0 as f64, 0x88 as f64 / 255.0, 1e-6, "local color0.b");
    }

    #[test]
    fn pack_buffer_uses_connected_color_input() {
        let color_node = Node {
            id: "color".to_string(),
            node_type: "ColorInput".to_string(),
            params: serde_json::json!({
                "value": "#abcdef",
            })
            .as_object()
            .unwrap()
            .clone()
            .into_iter()
            .collect(),
            inputs: vec![],
            outputs: vec![],
            input_bindings: vec![],
            wgsl_override: None,
        };
        let scene = make_test_scene(
            serde_json::json!({
                "color0": "#44cc88",
            })
            .as_object()
            .unwrap()
            .clone(),
            vec![color_node],
            vec![Connection {
                id: "color-to-ilight".to_string(),
                from: Endpoint {
                    node_id: "color".to_string(),
                    port_id: "color".to_string(),
                },
                to: Endpoint {
                    node_id: "ilight".to_string(),
                    port_id: "color0".to_string(),
                },
            }],
        );

        let cfg = ILightUpdateConfig {
            layer_id: "ilight".to_string(),
            power_fallback: 0.0,
            lightness_fallback: 0.75,
        };

        let bytes = cfg.pack_buffer(&scene);
        let (r0, g0, b0) = read_light_color(&bytes, 0);

        assert_near(r0 as f64, 0xab as f64 / 255.0, 1e-6, "connected color0.r");
        assert_near(g0 as f64, 0xcd as f64 / 255.0, 1e-6, "connected color0.g");
        assert_near(b0 as f64, 0xef as f64 / 255.0, 1e-6, "connected color0.b");
    }

    #[test]
    fn packed_mode_requires_complete_exact_sized_position_and_color_arrays() {
        let positions = test_node(
            "positions",
            "PackedInput",
            HashMap::from([
                ("elementType".to_string(), serde_json::json!("vector2")),
                (
                    "value".to_string(),
                    serde_json::json!(vec![
                        serde_json::json!([1.0, 2.0]);
                        INTELLIGENT_LIGHT_ZONE_COUNT
                    ]),
                ),
            ]),
        );
        let colors = test_node(
            "colors",
            "PackedInput",
            HashMap::from([
                ("elementType".to_string(), serde_json::json!("color")),
                (
                    "value".to_string(),
                    serde_json::json!(vec![
                        serde_json::json!([1.0, 0.5, 0.25, 1.0]);
                        INTELLIGENT_LIGHT_ZONE_COUNT
                    ]),
                ),
            ]),
        );
        let positions_edge = Connection {
            id: "positions-edge".to_string(),
            from: Endpoint {
                node_id: "positions".to_string(),
                port_id: "value".to_string(),
            },
            to: Endpoint {
                node_id: "ilight".to_string(),
                port_id: "positions".to_string(),
            },
        };
        let colors_edge = Connection {
            id: "colors-edge".to_string(),
            from: Endpoint {
                node_id: "colors".to_string(),
                port_id: "value".to_string(),
            },
            to: Endpoint {
                node_id: "ilight".to_string(),
                port_id: "colors".to_string(),
            },
        };
        let complete = make_test_scene(
            serde_json::Map::new(),
            vec![positions.clone(), colors.clone()],
            vec![positions_edge.clone(), colors_edge.clone()],
        );
        assert!(
            resolve_packed_pair(&complete, &complete.nodes[0])
                .unwrap()
                .is_some()
        );

        let positions_only = make_test_scene(
            serde_json::Map::new(),
            vec![positions.clone()],
            vec![positions_edge],
        );
        assert!(resolve_packed_pair(&positions_only, &positions_only.nodes[0]).is_err());

        let mut wrong_positions = positions;
        wrong_positions.params.insert(
            "value".to_string(),
            serde_json::json!(vec![
                serde_json::json!([1.0, 2.0]);
                INTELLIGENT_LIGHT_ZONE_COUNT - 1
            ]),
        );
        let wrong_length = make_test_scene(
            serde_json::Map::new(),
            vec![wrong_positions, colors],
            vec![
                Connection {
                    id: "positions-edge".to_string(),
                    from: Endpoint {
                        node_id: "positions".to_string(),
                        port_id: "value".to_string(),
                    },
                    to: Endpoint {
                        node_id: "ilight".to_string(),
                        port_id: "positions".to_string(),
                    },
                },
                colors_edge,
            ],
        );
        assert!(resolve_packed_pair(&wrong_length, &wrong_length.nodes[0]).is_err());
    }
}
