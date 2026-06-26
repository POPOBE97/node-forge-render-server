//! MeshGradient pass assembler.
//!
//! Mesh control points are resolved in target pixel space, then CPU-tessellated
//! into ordinary triangles carrying position, uv, and color vertex attributes.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use rust_wgpu_fiber::{
    ResourceName,
    eframe::wgpu::{self, Color},
};
use serde_json::Value;

use crate::{
    dsl::{Node, incoming_connection},
    renderer::{
        camera::{legacy_projection_camera_matrix, resolve_effective_camera_for_pass_node},
        node_compiler::template_loader,
        types::PassOutputSpec,
        utils::{as_bytes_slice, cpu_num_f32, cpu_num_u32_min_1},
        wgsl::build_fullscreen_textured_bundle,
    },
};

use super::super::pass_spec::{
    PassTextureBinding, RenderPassSpec, SamplerKind, TextureDecl, VertexLayoutKind, make_params,
};
use super::args::{BuilderState, SceneContext};

const GRID_ROWS: usize = 3;
const GRID_COLS: usize = 3;
const POINT_COUNT: usize = GRID_ROWS * GRID_COLS;
const DEFAULT_PATCH_DIV_COUNT: u32 = 20;
const MAX_PATCH_DIV_COUNT: u32 = 96;
const FLOATS_PER_VERTEX: usize = 9;

const DEFAULT_COLOR_HEX: [&str; POINT_COUNT] = [
    "#ff6b6b", "#ffa94d", "#ffd43b", "#69db7c", "#ffffff", "#74c0fc", "#b197fc", "#f783ac",
    "#da77f2",
];

const HM: [[f32; 4]; 4] = [
    [2.0, -2.0, 1.0, 1.0],
    [-3.0, 3.0, -2.0, -1.0],
    [0.0, 0.0, 1.0, 0.0],
    [1.0, 0.0, 0.0, 0.0],
];

#[derive(Clone, Copy, Debug)]
struct MeshPoint {
    position: [f32; 2],
    color: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct MeshSample {
    position: [f32; 2],
    color: [f32; 4],
}

/// Assemble a `"MeshGradient"` layer.
pub(crate) fn assemble_mesh_gradient(
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
    let target_size = [tgt_w, tgt_h];
    let center = [tgt_w * 0.5, tgt_h * 0.5];

    let pass_blend_state =
        crate::renderer::render_plan::parse_render_pass_blend_state(&layer_node.params)
            .with_context(|| format!("invalid blend params for MeshGradient {layer_id}"))?;
    let camera =
        resolve_effective_camera_for_pass_node(scene, nodes_by_id, layer_node, target_size)
            .with_context(|| format!("failed to resolve camera for MeshGradient {layer_id}"))?;
    let background =
        resolve_color_input(sc, layer_node, layer_id, "background", [0.0, 0.0, 0.0, 1.0])?;

    let patch_div_count = cpu_num_u32_min_1(
        scene,
        nodes_by_id,
        layer_node,
        "patchDivCount",
        DEFAULT_PATCH_DIV_COUNT,
    )?
    .clamp(2, MAX_PATCH_DIV_COUNT) as usize;

    let points = resolve_mesh_points(sc, layer_node, layer_id, target_size)?;
    let vertices = build_mesh_vertices(&points, patch_div_count, target_size, center);
    let geo: ResourceName = format!("sys.mesh_gradient.{layer_id}.geo").into();
    bs.geometry_buffers
        .push((geo.clone(), Arc::from(as_bytes_slice(&vertices).to_vec())));

    let is_sampled_output = bs.sampled_pass_ids.contains(layer_id);
    let writes_scene_output_target = !is_sampled_output;

    let output_tex: ResourceName = if is_sampled_output {
        let tex: ResourceName = format!("sys.mesh_gradient.{layer_id}.out").into();
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

    let pass_name: ResourceName = format!("sys.mesh_gradient.{layer_id}.pass").into();
    let params_name: ResourceName = format!("params.sys.mesh_gradient.{layer_id}").into();
    let params_val = make_params(target_size, target_size, center, camera, background);

    bs.render_pass_specs.push(RenderPassSpec {
        pass_id: pass_name.as_str().to_string(),
        name: pass_name.clone(),
        geometry_buffer: geo,
        instance_buffer: None,
        normals_buffer: None,
        vertex_layout: VertexLayoutKind::PositionUvColor,
        target_texture: output_tex.clone(),
        resolve_target: None,
        params_buffer: params_name,
        baked_data_parse_buffer: None,
        params: params_val,
        graph_binding: None,
        graph_values: None,
        shader_wgsl: build_mesh_gradient_wgsl(),
        texture_bindings: Vec::new(),
        sampler_kinds: Vec::new(),
        blend_state: pass_blend_state,
        color_load_op: wgpu::LoadOp::Clear(wgpu_color(background)),
        sample_count: 1,
    });
    bs.composite_passes.push(pass_name);

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
                format!("sys.mesh_gradient.{layer_id}.to.{composition_id}.compose.geo").into();
            bs.push_fullscreen_geometry(compose_geo.clone(), comp_w, comp_h);

            let compose_pass_name: ResourceName =
                format!("sys.mesh_gradient.{layer_id}.to.{composition_id}.compose.pass").into();
            let compose_params_name: ResourceName =
                format!("params.sys.mesh_gradient.{layer_id}.to.{composition_id}.compose").into();
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

pub(crate) fn build_mesh_gradient_wgsl() -> String {
    let vertex = template_loader::load_template("mesh_gradient_vertex.wgsl");
    let fragment = template_loader::load_template("mesh_gradient_fragment.wgsl");
    format!("{vertex}\n\n{fragment}")
}

fn resolve_mesh_points(
    sc: &SceneContext<'_>,
    layer_node: &Node,
    layer_id: &str,
    target_size: [f32; 2],
) -> Result<[MeshPoint; POINT_COUNT]> {
    let mut points = [MeshPoint {
        position: [0.0, 0.0],
        color: [1.0, 1.0, 1.0, 1.0],
    }; POINT_COUNT];

    for index in 0..POINT_COUNT {
        let pos_port = format!("pos{index}");
        let color_port = format!("color{index}");
        points[index] = MeshPoint {
            position: resolve_position_input(
                sc,
                layer_node,
                layer_id,
                &pos_port,
                default_position(index, target_size),
            )?,
            color: resolve_color_input(
                sc,
                layer_node,
                layer_id,
                &color_port,
                parse_hex_color(DEFAULT_COLOR_HEX[index])
                    .expect("bundled MeshGradient default color must be valid"),
            )?,
        };
    }

    Ok(points)
}

fn resolve_position_input(
    sc: &SceneContext<'_>,
    layer_node: &Node,
    layer_id: &str,
    port_id: &str,
    default: [f32; 2],
) -> Result<[f32; 2]> {
    let scene = sc.scene();
    let nodes_by_id = sc.nodes_by_id();

    if let Some(conn) = incoming_connection(scene, layer_id, port_id) {
        let Some(upstream_node) = nodes_by_id.get(&conn.from.node_id) else {
            return Ok(default);
        };
        if upstream_node.node_type != "Vector2Input" {
            bail!(
                "MeshGradient.{port_id} only supports Vector2Input connection, got {} ({})",
                upstream_node.node_type,
                conn.from.node_id
            );
        }
        if conn.from.port_id != "vector" {
            bail!(
                "MeshGradient.{port_id} must be connected from Vector2Input.vector, got {}.{}",
                conn.from.node_id,
                conn.from.port_id
            );
        }
        return Ok([
            cpu_num_f32(scene, nodes_by_id, upstream_node, "x", default[0])?,
            cpu_num_f32(scene, nodes_by_id, upstream_node, "y", default[1])?,
        ]);
    }

    parse_vec2_param(layer_node, port_id)?.map_or(Ok(default), Ok)
}

fn resolve_color_input(
    sc: &SceneContext<'_>,
    layer_node: &Node,
    layer_id: &str,
    port_id: &str,
    default: [f32; 4],
) -> Result<[f32; 4]> {
    let scene = sc.scene();
    let nodes_by_id = sc.nodes_by_id();

    if let Some(conn) = incoming_connection(scene, layer_id, port_id) {
        let Some(upstream_node) = nodes_by_id.get(&conn.from.node_id) else {
            return Ok(default);
        };
        if upstream_node.node_type == "ColorInput" && conn.from.port_id != "color" {
            bail!(
                "MeshGradient.{port_id} must be connected from ColorInput.color, got {}.{}",
                conn.from.node_id,
                conn.from.port_id
            );
        }
        if let Some(color) = parse_color_from_params(&upstream_node.params, "value")
            .or_else(|| parse_color_from_params(&upstream_node.params, &conn.from.port_id))
        {
            return Ok(color);
        }
        bail!(
            "MeshGradient.{port_id} connection must be a CPU-resolvable color, got {} ({})",
            upstream_node.node_type,
            conn.from.node_id
        );
    }

    Ok(parse_color_from_params(&layer_node.params, port_id).unwrap_or(default))
}

fn build_mesh_vertices(
    points: &[MeshPoint; POINT_COUNT],
    patch_div_count: usize,
    target_size: [f32; 2],
    center: [f32; 2],
) -> Vec<f32> {
    let cells_per_patch = patch_div_count.saturating_sub(1);
    let vertex_count = (GRID_ROWS - 1) * (GRID_COLS - 1) * cells_per_patch * cells_per_patch * 6;
    let mut out = Vec::with_capacity(vertex_count * FLOATS_PER_VERTEX);

    for patch_row in 0..(GRID_ROWS - 1) {
        for patch_col in 0..(GRID_COLS - 1) {
            for row in 0..cells_per_patch {
                let r0 = row as f32 / cells_per_patch as f32;
                let r1 = (row + 1) as f32 / cells_per_patch as f32;
                for col in 0..cells_per_patch {
                    let c0 = col as f32 / cells_per_patch as f32;
                    let c1 = (col + 1) as f32 / cells_per_patch as f32;

                    let p00 = sample_patch(points, patch_row, patch_col, r0, c0);
                    let p01 = sample_patch(points, patch_row, patch_col, r0, c1);
                    let p11 = sample_patch(points, patch_row, patch_col, r1, c1);
                    let p10 = sample_patch(points, patch_row, patch_col, r1, c0);

                    push_vertex(&mut out, p00, target_size, center);
                    push_vertex(&mut out, p01, target_size, center);
                    push_vertex(&mut out, p11, target_size, center);

                    push_vertex(&mut out, p00, target_size, center);
                    push_vertex(&mut out, p11, target_size, center);
                    push_vertex(&mut out, p10, target_size, center);
                }
            }
        }
    }

    out
}

fn sample_patch(
    points: &[MeshPoint; POINT_COUNT],
    patch_row: usize,
    patch_col: usize,
    row_t: f32,
    col_t: f32,
) -> MeshSample {
    MeshSample {
        position: [
            sample_scalar(
                points,
                patch_row,
                patch_col,
                row_t,
                col_t,
                ScalarField::PositionX,
            ),
            sample_scalar(
                points,
                patch_row,
                patch_col,
                row_t,
                col_t,
                ScalarField::PositionY,
            ),
        ],
        color: [
            sample_scalar(
                points,
                patch_row,
                patch_col,
                row_t,
                col_t,
                ScalarField::ColorR,
            ),
            sample_scalar(
                points,
                patch_row,
                patch_col,
                row_t,
                col_t,
                ScalarField::ColorG,
            ),
            sample_scalar(
                points,
                patch_row,
                patch_col,
                row_t,
                col_t,
                ScalarField::ColorB,
            ),
            sample_scalar(
                points,
                patch_row,
                patch_col,
                row_t,
                col_t,
                ScalarField::ColorA,
            ),
        ],
    }
}

#[derive(Clone, Copy)]
enum ScalarField {
    PositionX,
    PositionY,
    ColorR,
    ColorG,
    ColorB,
    ColorA,
}

fn sample_scalar(
    points: &[MeshPoint; POINT_COUNT],
    patch_row: usize,
    patch_col: usize,
    row_t: f32,
    col_t: f32,
    field: ScalarField,
) -> f32 {
    let p00 = point_value(points, patch_row, patch_col, field);
    let p10 = point_value(points, patch_row + 1, patch_col, field);
    let p01 = point_value(points, patch_row, patch_col + 1, field);
    let p11 = point_value(points, patch_row + 1, patch_col + 1, field);

    let du00 = row_derivative(points, patch_row, patch_col, field);
    let du10 = row_derivative(points, patch_row + 1, patch_col, field);
    let du01 = row_derivative(points, patch_row, patch_col + 1, field);
    let du11 = row_derivative(points, patch_row + 1, patch_col + 1, field);

    let dv00 = col_derivative(points, patch_row, patch_col, field);
    let dv10 = col_derivative(points, patch_row + 1, patch_col, field);
    let dv01 = col_derivative(points, patch_row, patch_col + 1, field);
    let dv11 = col_derivative(points, patch_row + 1, patch_col + 1, field);

    let patch = [
        [p00, p10, du00, du10],
        [p01, p11, du01, du11],
        [dv00, dv10, 0.0, 0.0],
        [dv01, dv11, 0.0, 0.0],
    ];

    hermite_scalar(&patch, row_t, col_t)
}

fn hermite_scalar(patch: &[[f32; 4]; 4], u: f32, v: f32) -> f32 {
    let bu = hermite_basis(u);
    let bv = hermite_basis(v);
    let mut out = 0.0;
    for row in 0..4 {
        for col in 0..4 {
            out += bu[row] * patch[row][col] * bv[col];
        }
    }
    out
}

fn hermite_basis(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let tv = [t2 * t, t2, t, 1.0];
    let mut out = [0.0; 4];
    for col in 0..4 {
        for row in 0..4 {
            out[col] += tv[row] * HM[row][col];
        }
    }
    out
}

fn row_derivative(
    points: &[MeshPoint; POINT_COUNT],
    row: usize,
    col: usize,
    field: ScalarField,
) -> f32 {
    if row == 0 {
        point_value(points, 1, col, field) - point_value(points, 0, col, field)
    } else if row + 1 == GRID_ROWS {
        point_value(points, row, col, field) - point_value(points, row - 1, col, field)
    } else {
        (point_value(points, row + 1, col, field) - point_value(points, row - 1, col, field)) * 0.5
    }
}

fn col_derivative(
    points: &[MeshPoint; POINT_COUNT],
    row: usize,
    col: usize,
    field: ScalarField,
) -> f32 {
    if col == 0 {
        point_value(points, row, 1, field) - point_value(points, row, 0, field)
    } else if col + 1 == GRID_COLS {
        point_value(points, row, col, field) - point_value(points, row, col - 1, field)
    } else {
        (point_value(points, row, col + 1, field) - point_value(points, row, col - 1, field)) * 0.5
    }
}

fn point_value(
    points: &[MeshPoint; POINT_COUNT],
    row: usize,
    col: usize,
    field: ScalarField,
) -> f32 {
    let p = points[row * GRID_COLS + col];
    match field {
        ScalarField::PositionX => p.position[0],
        ScalarField::PositionY => p.position[1],
        ScalarField::ColorR => p.color[0],
        ScalarField::ColorG => p.color[1],
        ScalarField::ColorB => p.color[2],
        ScalarField::ColorA => p.color[3],
    }
}

fn push_vertex(out: &mut Vec<f32>, sample: MeshSample, target_size: [f32; 2], center: [f32; 2]) {
    let target_w = target_size[0].max(1.0);
    let target_h = target_size[1].max(1.0);
    let local_x = sample.position[0] - center[0];
    let local_y = sample.position[1] - center[1];
    let uv_x = sample.position[0] / target_w;
    let uv_y = 1.0 - sample.position[1] / target_h;
    out.extend_from_slice(&[
        local_x,
        local_y,
        0.0,
        uv_x,
        uv_y,
        sample.color[0],
        sample.color[1],
        sample.color[2],
        sample.color[3],
    ]);
}

fn default_position(index: usize, target_size: [f32; 2]) -> [f32; 2] {
    let row = index / GRID_COLS;
    let col = index % GRID_COLS;
    [
        col as f32 / (GRID_COLS - 1) as f32 * target_size[0],
        row as f32 / (GRID_ROWS - 1) as f32 * target_size[1],
    ]
}

fn parse_vec2_param(node: &Node, key: &str) -> Result<Option<[f32; 2]>> {
    let Some(value) = node.params.get(key) else {
        return Ok(None);
    };
    parse_vec2_value(value).map(Some).ok_or_else(|| {
        anyhow!(
            "{}.{} must be vec2 object {{x,y}} or array [x,y]",
            node.id,
            key
        )
    })
}

fn parse_vec2_value(value: &Value) -> Option<[f32; 2]> {
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

fn parse_color_from_params(
    params: &std::collections::HashMap<String, Value>,
    key: &str,
) -> Option<[f32; 4]> {
    let value = params.get(key)?;
    parse_color_value(value)
}

fn parse_color_value(value: &Value) -> Option<[f32; 4]> {
    if let Some(s) = value.as_str() {
        return parse_hex_color(s);
    }

    if let Some(arr) = value.as_array() {
        return Some([
            arr.first().and_then(json_f32).unwrap_or(0.0),
            arr.get(1).and_then(json_f32).unwrap_or(0.0),
            arr.get(2).and_then(json_f32).unwrap_or(0.0),
            arr.get(3).and_then(json_f32).unwrap_or(1.0),
        ]);
    }

    if let Some(obj) = value.as_object() {
        let has_rgba = obj.contains_key("r") || obj.contains_key("g") || obj.contains_key("b");
        if has_rgba {
            return Some([
                obj.get("r").and_then(json_f32).unwrap_or(0.0),
                obj.get("g").and_then(json_f32).unwrap_or(0.0),
                obj.get("b").and_then(json_f32).unwrap_or(0.0),
                obj.get("a").and_then(json_f32).unwrap_or(1.0),
            ]);
        }
        let has_xyzw = obj.contains_key("x") || obj.contains_key("y") || obj.contains_key("z");
        if has_xyzw {
            return Some([
                obj.get("x").and_then(json_f32).unwrap_or(0.0),
                obj.get("y").and_then(json_f32).unwrap_or(0.0),
                obj.get("z").and_then(json_f32).unwrap_or(0.0),
                obj.get("w").and_then(json_f32).unwrap_or(1.0),
            ]);
        }
    }

    None
}

fn json_f32(v: &Value) -> Option<f32> {
    let value = v
        .as_f64()
        .or_else(|| v.as_i64().map(|x| x as f64))
        .or_else(|| v.as_u64().map(|x| x as f64))?;
    value.is_finite().then_some(value as f32)
}

fn parse_hex_color(s: &str) -> Option<[f32; 4]> {
    let raw = s.trim().strip_prefix('#')?;
    match raw.len() {
        6 => Some([
            u8::from_str_radix(&raw[0..2], 16).ok()? as f32 / 255.0,
            u8::from_str_radix(&raw[2..4], 16).ok()? as f32 / 255.0,
            u8::from_str_radix(&raw[4..6], 16).ok()? as f32 / 255.0,
            1.0,
        ]),
        8 => Some([
            u8::from_str_radix(&raw[0..2], 16).ok()? as f32 / 255.0,
            u8::from_str_radix(&raw[2..4], 16).ok()? as f32 / 255.0,
            u8::from_str_radix(&raw[4..6], 16).ok()? as f32 / 255.0,
            u8::from_str_radix(&raw[6..8], 16).ok()? as f32 / 255.0,
        ]),
        3 => Some([
            u8::from_str_radix(&raw[0..1], 16).ok()? as f32 / 15.0,
            u8::from_str_radix(&raw[1..2], 16).ok()? as f32 / 15.0,
            u8::from_str_radix(&raw[2..3], 16).ok()? as f32 / 15.0,
            1.0,
        ]),
        4 => Some([
            u8::from_str_radix(&raw[0..1], 16).ok()? as f32 / 15.0,
            u8::from_str_radix(&raw[1..2], 16).ok()? as f32 / 15.0,
            u8::from_str_radix(&raw[2..3], 16).ok()? as f32 / 15.0,
            u8::from_str_radix(&raw[3..4], 16).ok()? as f32 / 15.0,
        ]),
        _ => None,
    }
}

fn wgpu_color(color: [f32; 4]) -> Color {
    Color {
        r: color[0] as f64,
        g: color[1] as f64,
        b: color[2] as f64,
        a: color[3] as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_points(target_size: [f32; 2]) -> [MeshPoint; POINT_COUNT] {
        let mut points = [MeshPoint {
            position: [0.0, 0.0],
            color: [1.0, 1.0, 1.0, 1.0],
        }; POINT_COUNT];
        for index in 0..POINT_COUNT {
            points[index].position = default_position(index, target_size);
        }
        points
    }

    #[test]
    fn default_positions_are_target_pixel_space() {
        let size = [300.0, 150.0];
        assert_eq!(default_position(0, size), [0.0, 0.0]);
        assert_eq!(default_position(4, size), [150.0, 75.0]);
        assert_eq!(default_position(8, size), [300.0, 150.0]);
    }

    #[test]
    fn tessellation_expands_to_triangle_vertices() {
        let size = [300.0, 150.0];
        let vertices = build_mesh_vertices(&default_points(size), 2, size, [150.0, 75.0]);
        let expected_vertices = (GRID_ROWS - 1) * (GRID_COLS - 1) * 6;
        assert_eq!(vertices.len(), expected_vertices * FLOATS_PER_VERTEX);
    }

    #[test]
    fn hermite_patch_hits_control_point_corners() {
        let points = default_points([300.0, 150.0]);
        let p00 = sample_patch(&points, 0, 0, 0.0, 0.0);
        let p11 = sample_patch(&points, 0, 0, 1.0, 1.0);
        assert!((p00.position[0] - 0.0).abs() < 1e-4);
        assert!((p00.position[1] - 0.0).abs() < 1e-4);
        assert!((p11.position[0] - 150.0).abs() < 1e-4);
        assert!((p11.position[1] - 75.0).abs() < 1e-4);
    }
}
