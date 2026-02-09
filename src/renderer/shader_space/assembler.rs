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

use anyhow::{anyhow, bail, Result};
use image::{DynamicImage, Rgba, RgbaImage};
use rust_wgpu_fiber::{
    eframe::wgpu::{
        self, vertex_attr_array, BlendState, Color, ShaderStages, TextureFormat, TextureUsages,
    },
    pool::{
        buffer_pool::BufferSpec, sampler_pool::SamplerSpec,
        texture_pool::TextureSpec as FiberTextureSpec,
    },
    shader_space::{ShaderSpace, ShaderSpaceResult},
    ResourceName,
};

use crate::{
    dsl::{find_node, incoming_connection, parse_str, parse_texture_format, SceneDSL},
    renderer::{
        graph_uniforms::{
            choose_graph_binding_kind, compute_pipeline_signature_for_pass_bindings,
            graph_field_name, hash_bytes, pack_graph_values,
        },
        node_compiler::{
            compile_vertex_expr,
            geometry_nodes::{rect2d_geometry_vertices, rect2d_unit_geometry_vertices},
        },
        scene_prep::{bake_data_parse_nodes, prepare_scene},
        types::ValueType,
        types::{
            BakedDataParseMeta, BakedValue, GraphBinding, GraphBindingKind, Kernel2D,
            MaterialCompileContext, Params, PassBindings, PassOutputRegistry, PassOutputSpec,
            TypedExpr,
        },
        utils::{as_bytes, as_bytes_slice, load_image_from_data_url},
        utils::{coerce_to_type, cpu_num_f32_min_0, cpu_num_u32_min_1},
        wgsl::{
            build_blur_image_wgsl_bundle, build_blur_image_wgsl_bundle_with_graph_binding,
            build_downsample_bundle, build_downsample_pass_wgsl_bundle,
            build_fullscreen_textured_bundle, build_horizontal_blur_bundle, build_pass_wgsl_bundle,
            build_pass_wgsl_bundle_with_graph_binding, build_upsample_bilinear_bundle,
            build_vertical_blur_bundle, clamp_min_1, gaussian_kernel_8,
            gaussian_mip_level_and_sigma_p, ERROR_SHADER_WGSL,
        },
    },
};

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

fn mat4_mul(a: [f32; 16], b: [f32; 16]) -> [f32; 16] {
    // Column-major mat4 multiply to match WGSL `mat4x4f` (constructed from 4 column vectors)
    // and the `inst_m * vec4f(position, 1.0)` convention in the vertex shader.
    //
    // out = a * b
    // out[r,c] = sum_k a[r,k] * b[k,c]
    // idx(r,c) = c*4 + r
    let mut out = [0.0f32; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = a[0 * 4 + r] * b[c * 4 + 0]
                + a[1 * 4 + r] * b[c * 4 + 1]
                + a[2 * 4 + r] * b[c * 4 + 2]
                + a[3 * 4 + r] * b[c * 4 + 3];
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

fn parse_inline_vec3(node: &crate::dsl::Node, key: &str, default: [f32; 3]) -> [f32; 3] {
    let mut out = default;
    if let Some(v) = node.params.get(key) {
        if let Some(obj) = v.as_object() {
            out[0] = obj
                .get("x")
                .and_then(|v| v.as_f64())
                .unwrap_or(out[0] as f64) as f32;
            out[1] = obj
                .get("y")
                .and_then(|v| v.as_f64())
                .unwrap_or(out[1] as f64) as f32;
            out[2] = obj
                .get("z")
                .and_then(|v| v.as_f64())
                .unwrap_or(out[2] as f64) as f32;
        }
    }
    out
}

fn parse_inline_mat4(node: &crate::dsl::Node, key: &str) -> Option<[f32; 16]> {
    let Some(v) = node.params.get(key) else {
        return None;
    };

    if let Some(arr) = v.as_array() {
        if arr.len() == 16 {
            let mut m = [0.0f32; 16];
            for (i, x) in arr.iter().enumerate() {
                m[i] = x.as_f64().unwrap_or(0.0) as f32;
            }
            return Some(m);
        }
    }

    // Allow object form: { m00:..., m01:..., ... } is not supported (yet).
    None
}

fn compute_trs_matrix(node: &crate::dsl::Node) -> [f32; 16] {
    // T * Rz * S for now.
    // Note: rotate is authored in degrees.
    let t = parse_inline_vec3(node, "translate", [0.0, 0.0, 0.0]);
    let s = parse_inline_vec3(node, "scale", [1.0, 1.0, 1.0]);
    let r = parse_inline_vec3(node, "rotate", [0.0, 0.0, 0.0]);
    let rz = r[2].to_radians();

    mat4_mul(
        mat4_translate(t[0], t[1], t[2]),
        mat4_mul(mat4_rotate_z(rz), mat4_scale(s[0], s[1], s[2])),
    )
}

fn compute_set_transform_matrix(
    _scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, crate::dsl::Node>,
    node: &crate::dsl::Node,
) -> Result<[f32; 16]> {
    let mode = node
        .params
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("Components");

    match mode {
        "Matrix" => {
            if let Some(m) = parse_inline_mat4(node, "matrix") {
                Ok(m)
            } else {
                // The scheme says matrix:any (usually connected). For now we only accept inline arrays.
                // If users want dynamic matrices, they'll need a dedicated CPU-side feature.
                Ok([
                    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                ])
            }
        }
        _ => Ok(compute_trs_matrix(node)),
    }
}

fn baked_to_vec3_translate(v: BakedValue) -> [f32; 3] {
    match v {
        BakedValue::Vec3(v) => v,
        BakedValue::Vec4([x, y, z, _w]) => [x, y, z],
        BakedValue::Vec2([x, y]) => [x, y, 0.0],
        BakedValue::F32(x) => [x, 0.0, 0.0],
        BakedValue::I32(x) => [x as f32, 0.0, 0.0],
        BakedValue::U32(x) => [x as f32, 0.0, 0.0],
        BakedValue::Bool(x) => {
            if x {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 0.0, 0.0]
            }
        }
    }
}

fn baked_to_vec3_scale(v: BakedValue) -> [f32; 3] {
    match v {
        BakedValue::Vec3(v) => v,
        BakedValue::Vec4([x, y, z, _w]) => [x, y, z],
        BakedValue::Vec2([x, y]) => [x, y, 1.0],
        BakedValue::F32(x) => [x, x, 1.0],
        BakedValue::I32(x) => {
            let x = x as f32;
            [x, x, 1.0]
        }
        BakedValue::U32(x) => {
            let x = x as f32;
            [x, x, 1.0]
        }
        BakedValue::Bool(x) => {
            if x {
                [1.0, 1.0, 1.0]
            } else {
                [0.0, 0.0, 1.0]
            }
        }
    }
}

fn baked_to_vec3_rotate_deg(v: BakedValue) -> [f32; 3] {
    match v {
        BakedValue::Vec3(v) => v,
        BakedValue::Vec4([x, y, z, _w]) => [x, y, z],
        BakedValue::Vec2([x, y]) => [x, y, 0.0],
        // Common authoring pattern: scalar rotation means Z rotation.
        BakedValue::F32(z) => [0.0, 0.0, z],
        BakedValue::I32(z) => [0.0, 0.0, z as f32],
        BakedValue::U32(z) => [0.0, 0.0, z as f32],
        BakedValue::Bool(x) => {
            if x {
                [0.0, 0.0, 1.0]
            } else {
                [0.0, 0.0, 0.0]
            }
        }
    }
}

pub(crate) fn resolve_geometry_for_render_pass(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    ids: &HashMap<String, ResourceName>,
    geometry_node_id: &str,
    render_target_size: [f32; 2],
    material_ctx: Option<&MaterialCompileContext>,
) -> Result<(
    ResourceName,
    f32,
    f32,
    f32,
    f32,
    u32,
    [f32; 16],
    Option<Vec<[f32; 16]>>,
    Option<TypedExpr>,
    Vec<String>,
    String,
    bool,
    Option<crate::renderer::render_plan::geometry::Rect2DDynamicInputs>,
)> {
    let geometry_node = find_node(nodes_by_id, geometry_node_id)?;

    match geometry_node.node_type.as_str() {
        "Rect2DGeometry" => {
            let (
                geometry_buffer,
                geo_w,
                geo_h,
                geo_x,
                geo_y,
                instances,
                base_m,
                instance_mats,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                _graph_input_kinds,
                uses_instance_index,
                rect_dyn,
            ) = crate::renderer::render_plan::resolve_geometry_for_render_pass(
                scene,
                nodes_by_id,
                ids,
                geometry_node_id,
                render_target_size,
                material_ctx,
            )?;
            Ok((
                geometry_buffer,
                geo_w,
                geo_h,
                geo_x,
                geo_y,
                instances,
                base_m,
                instance_mats,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                uses_instance_index,
                rect_dyn,
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
                instance_mats,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                uses_instance_index,
                rect_dyn,
            ) = resolve_geometry_for_render_pass(
                scene,
                nodes_by_id,
                ids,
                &upstream_geo_id,
                render_target_size,
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
                instance_mats,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                uses_instance_index,
                rect_dyn,
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
                instance_mats,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                uses_instance_index,
                rect_dyn,
            ) = resolve_geometry_for_render_pass(
                scene,
                nodes_by_id,
                ids,
                &upstream_geo_id,
                render_target_size,
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
                instance_mats,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                uses_instance_index,
                rect_dyn,
            ))
        }
        "SetTransform" => {
            // Geometry chain: SetTransform.geometry -> base geometry buffer.
            // Unlike TransformGeometry, this sets the base transform directly at CPU instance-buffer initialization.

            let upstream_geo_id = incoming_connection(scene, geometry_node_id, "geometry")
                .map(|c| c.from.node_id.clone())
                .ok_or_else(|| anyhow!("SetTransform.geometry missing for {geometry_node_id}"))?;

            let (
                buf,
                w,
                h,
                x,
                y,
                instances,
                _base_m,
                _upstream_instance_mats,
                _translate_expr,
                _vtx_inline_stmts,
                _vtx_wgsl_decls,
                uses_instance_index,
                _rect_dyn,
            ) = resolve_geometry_for_render_pass(
                scene,
                nodes_by_id,
                ids,
                &upstream_geo_id,
                render_target_size,
                material_ctx,
            )?;

            // SetTransform overrides the accumulated base matrix.
            let m = compute_set_transform_matrix(scene, nodes_by_id, geometry_node)?;

            // Bake per-instance base matrices if any of translate/scale/rotate are connected and
            // baked values are available.
            //
            // Semantics A: SetTransform result replaces upstream base matrix.
            // Connected components behave like "deltas" on top of inline params:
            // - translate: additive
            // - rotate: additive degrees (Z only currently)
            // - scale: multiplicative
            let mut instance_mats: Option<Vec<[f32; 16]>> = None;
            if let Some(material_ctx) = material_ctx {
                if let (Some(baked), Some(meta)) = (
                    material_ctx.baked_data_parse.as_ref(),
                    material_ctx.baked_data_parse_meta.as_ref(),
                ) {
                    let translate_key = incoming_connection(scene, &geometry_node.id, "translate")
                        .map(|conn| {
                            (
                                meta.pass_id.clone(),
                                conn.from.node_id.clone(),
                                conn.from.port_id.clone(),
                            )
                        });

                    let scale_key =
                        incoming_connection(scene, &geometry_node.id, "scale").map(|conn| {
                            (
                                meta.pass_id.clone(),
                                conn.from.node_id.clone(),
                                conn.from.port_id.clone(),
                            )
                        });

                    let rotate_key =
                        incoming_connection(scene, &geometry_node.id, "rotate").map(|conn| {
                            (
                                meta.pass_id.clone(),
                                conn.from.node_id.clone(),
                                conn.from.port_id.clone(),
                            )
                        });

                    let has_any = translate_key
                        .as_ref()
                        .is_some_and(|k| baked.contains_key(k))
                        || scale_key.as_ref().is_some_and(|k| baked.contains_key(k))
                        || rotate_key.as_ref().is_some_and(|k| baked.contains_key(k));

                    if has_any {
                        let t_inline =
                            parse_inline_vec3(geometry_node, "translate", [0.0, 0.0, 0.0]);
                        let s_inline = parse_inline_vec3(geometry_node, "scale", [1.0, 1.0, 1.0]);
                        let r_inline = parse_inline_vec3(geometry_node, "rotate", [0.0, 0.0, 0.0]);

                        let instances = instances;
                        let mut mats: Vec<[f32; 16]> = Vec::with_capacity(instances as usize);
                        for i in 0..instances as usize {
                            let t_conn = translate_key
                                .as_ref()
                                .and_then(|k| baked.get(k))
                                .and_then(|vs| vs.get(i))
                                .cloned()
                                .map(baked_to_vec3_translate)
                                .unwrap_or([0.0, 0.0, 0.0]);

                            let s_conn = scale_key
                                .as_ref()
                                .and_then(|k| baked.get(k))
                                .and_then(|vs| vs.get(i))
                                .cloned()
                                .map(baked_to_vec3_scale)
                                .unwrap_or([1.0, 1.0, 1.0]);

                            let r_conn = rotate_key
                                .as_ref()
                                .and_then(|k| baked.get(k))
                                .and_then(|vs| vs.get(i))
                                .cloned()
                                .map(baked_to_vec3_rotate_deg)
                                .unwrap_or([0.0, 0.0, 0.0]);

                            // Combine inline + connected components.
                            let t = [
                                t_inline[0] + t_conn[0],
                                t_inline[1] + t_conn[1],
                                t_inline[2] + t_conn[2],
                            ];
                            let s = [
                                s_inline[0] * s_conn[0],
                                s_inline[1] * s_conn[1],
                                s_inline[2] * s_conn[2],
                            ];
                            let r = [
                                r_inline[0] + r_conn[0],
                                r_inline[1] + r_conn[1],
                                r_inline[2] + r_conn[2],
                            ];

                            let rz = r[2].to_radians();
                            let m_i = mat4_mul(
                                mat4_translate(t[0], t[1], t[2]),
                                mat4_mul(mat4_rotate_z(rz), mat4_scale(s[0], s[1], s[2])),
                            );
                            mats.push(m_i);
                        }
                        instance_mats = Some(mats);
                    }
                }
            }

            // Important: SetTransform should NOT forward its translate input into the vertex shader;
            // it applies it into the base matrix here (CPU-side).
            // Also: any TransformGeometry nodes *before* SetTransform are skipped, meaning we
            // discard upstream vertex-side translate expressions and inline statements.
            Ok((
                buf,
                w,
                h,
                x,
                y,
                instances,
                // Replace upstream base matrix (per user semantics A).
                m,
                instance_mats,
                None,
                Vec::new(),
                String::new(),
                uses_instance_index,
                None,
            ))
        }
        "TransformGeometry" => {
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
                instance_mats,
                upstream_translate_expr,
                mut vtx_inline_stmts,
                mut vtx_wgsl_decls,
                mut uses_instance_index,
                rect_dyn,
            ) = resolve_geometry_for_render_pass(
                scene,
                nodes_by_id,
                ids,
                &upstream_geo_id,
                render_target_size,
                material_ctx,
            )?;

            // Adjust logical size by inline scale, so UV/GeoFragCoord stay correct.
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

            // Vertex-stage translate overrides upstream translate.
            let mut translate_expr = upstream_translate_expr;
            let mut local_inline_stmts: Vec<String> = Vec::new();
            let mut local_wgsl_decls = String::new();
            let mut local_uses_instance_index = false;

            if let Some(conn) = incoming_connection(scene, &geometry_node.id, "translate") {
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

                local_inline_stmts = vtx_ctx.inline_stmts.clone();
                local_wgsl_decls = vtx_ctx.wgsl_decls();
                local_uses_instance_index = vtx_ctx.uses_instance_index;
                translate_expr = Some(expr);
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
                base_m,
                instance_mats,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                uses_instance_index,
                rect_dyn,
            ))
        }
        other => {
            bail!(
                "RenderPass.geometry must resolve to Rect2DGeometry/TransformGeometry/InstancedGeometryEnd, got {other}"
            )
        }
    }
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
        if !matches!(
            node.node_type.as_str(),
            "RenderPass" | "GuassianBlurPass" | "Downsample"
        ) {
            continue;
        }
        let deps = deps_for_pass_node(scene, nodes_by_id, node_id.as_str())?;
        out.extend(deps);
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
    LinearMirror,
    LinearRepeat,
    LinearClamp,
}

type PassTextureBinding = crate::renderer::render_plan::types::PassTextureBinding;

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
    pass_id: String,
    name: ResourceName,
    geometry_buffer: ResourceName,
    instance_buffer: Option<ResourceName>,
    target_texture: ResourceName,
    params_buffer: ResourceName,
    baked_data_parse_buffer: Option<ResourceName>,
    params: Params,
    graph_binding: Option<GraphBinding>,
    graph_values: Option<Vec<u8>>,
    shader_wgsl: String,
    texture_bindings: Vec<PassTextureBinding>,
    sampler_kind: SamplerKind,
    blend_state: BlendState,
    color_load_op: wgpu::LoadOp<Color>,
}

fn build_image_premultiply_wgsl(tex_var: &str, samp_var: &str) -> String {
    crate::renderer::wgsl_templates::build_image_premultiply_wgsl(tex_var, samp_var)
}

fn build_srgb_display_encode_wgsl(tex_var: &str, samp_var: &str) -> String {
    crate::renderer::wgsl_templates::build_srgb_display_encode_wgsl(tex_var, samp_var)
}

// UI presentation helper: encode linear output to SDR sRGB for egui-wgpu.
// We use dot-separated segments (no `__`) so the names read well and extend naturally to HDR.
const UI_PRESENT_SDR_SRGB_SUFFIX: &str = ".present.sdr.srgb";

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
        // Premultiplied alpha (common in our renderer): RGB is assumed multiplied by A.
        "alpha" | "premul-alpha" | "premultiplied-alpha" => BlendState {
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

fn flip_image_y_rgba8(image: Arc<DynamicImage>) -> Arc<DynamicImage> {
    crate::renderer::render_plan::image_prepass::flip_image_y_rgba8(image)
}

pub(crate) fn build_shader_space_from_scene_internal(
    scene: &SceneDSL,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    enable_display_encode: bool,
    debug_dump_wgsl_dir: Option<PathBuf>,
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

    // Pass nodes that are sampled via PassTexture must have a dedicated output texture.
    let sampled_pass_ids =
        crate::renderer::render_plan::sampled_pass_node_ids(&prepared.scene, nodes_by_id)?;

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
                ) = crate::renderer::render_plan::resolve_geometry_for_render_pass(
                    &prepared.scene,
                    nodes_by_id,
                    ids,
                    &node.id,
                    [tgt_w, tgt_h],
                    None,
                )?;
                let verts = if rect_dyn.is_some() {
                    rect2d_unit_geometry_vertices()
                } else {
                    rect2d_geometry_vertices(geo_w, geo_h)
                };
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

    // Track which pass node ids are direct composite layers (vs. transitive dependencies).
    let composite_layer_ids: HashSet<String> = composite_layers_in_order.iter().cloned().collect();

    // Some pass nodes (RenderPass) are sampled downstream by Downsample nodes as higher-resolution
    // sources. For those, we want the pass output texture sized to its geometry extent so the
    // downstream Downsample actually has more source detail to work with.
    //
    // IMPORTANT: Existing PassTexture consumers expect legacy behavior where sampled RenderPass
    // outputs match the Composite target resolution. So we only enable geometry-sized outputs for
    // passes that are directly used as Downsample.source.
    let mut downsample_source_pass_ids: HashSet<String> = HashSet::new();
    for (node_id, node) in nodes_by_id {
        if node.node_type != "Downsample" {
            continue;
        }
        if let Some(conn) = incoming_connection(&prepared.scene, node_id, "source") {
            downsample_source_pass_ids.insert(conn.from.node_id.clone());
        }
    }

    for layer_id in &pass_nodes_in_order {
        let layer_node = find_node(&nodes_by_id, layer_id)?;
        match layer_node.node_type.as_str() {
            "RenderPass" => {
                let pass_name = ids
                    .get(layer_id)
                    .cloned()
                    .ok_or_else(|| anyhow!("missing name for node: {layer_id}"))?;

                // If this pass is sampled downstream, render into a dedicated intermediate texture.
                // IMPORTANT: sampled passes are commonly used as higher-resolution sources for downstream
                // filtering (e.g. Downsample). In that case, the pass output resolution should match the
                // pass geometry extent, not the Composite target size.
                let is_sampled_output = sampled_pass_ids.contains(layer_id);
                let is_composite_layer = composite_layer_ids.contains(layer_id);
                let is_downsample_source = downsample_source_pass_ids.contains(layer_id);

                let blend_state = crate::renderer::render_plan::parse_render_pass_blend_state(
                    &layer_node.params,
                )?;

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
                    _instance_mats,
                    _translate_expr,
                    _vertex_inline_stmts,
                    _vertex_wgsl_decls,
                    _vertex_graph_input_kinds,
                    _vertex_uses_instance_index,
                    _rect_dyn,
                ) = crate::renderer::render_plan::resolve_geometry_for_render_pass(
                    &prepared.scene,
                    nodes_by_id,
                    ids,
                    &render_geo_node_id,
                    [tgt_w, tgt_h],
                    Some(&MaterialCompileContext {
                        baked_data_parse: Some(std::sync::Arc::new(
                            prepared.baked_data_parse.clone(),
                        )),
                        baked_data_parse_meta: None,
                        ..Default::default()
                    }),
                )?;

                // Determine the render target for this pass.
                // - If sampled downstream: render into a dedicated intermediate texture sized to the
                //   geometry extent (rounded to integer pixels).
                // - Otherwise: render directly into the Composite target texture.
                let (pass_target_w_u, pass_target_h_u, pass_output_texture): (
                    u32,
                    u32,
                    ResourceName,
                ) = if is_sampled_output {
                    let out_tex: ResourceName = format!("sys.pass.{layer_id}.out").into();
                    let (w_u, h_u) = if is_downsample_source {
                        // Optimization: when a pass is used as Downsample.source, keep its output sized
                        // to its geometry extent for higher-resolution filtering downstream.
                        //
                        // NOTE: If the pass is also a direct composite layer, we still keep this
                        // geometry-sized output so Downsample chains see the expected content.
                        // Compositing is handled by a dedicated compose pass below.
                        (geo_w.max(1.0).round() as u32, geo_h.max(1.0).round() as u32)
                    } else {
                        // Legacy: sampled RenderPass outputs match the Composite target resolution.
                        (tgt_w_u, tgt_h_u)
                    };
                    textures.push(TextureDecl {
                        name: out_tex.clone(),
                        size: [w_u, h_u],
                        format: sampled_pass_format,
                    });
                    (w_u, h_u, out_tex)
                } else {
                    (tgt_w_u, tgt_h_u, target_texture_name.clone())
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
                    _base_m_2,
                    instance_mats_2,
                    translate_expr,
                    vertex_inline_stmts,
                    vertex_wgsl_decls,
                    vertex_graph_input_kinds,
                    vertex_uses_instance_index,
                    rect_dyn_2,
                ) = crate::renderer::render_plan::resolve_geometry_for_render_pass(
                    &prepared.scene,
                    nodes_by_id,
                    ids,
                    &render_geo_node_id,
                    [tgt_w, tgt_h],
                    Some(&MaterialCompileContext {
                        baked_data_parse: Some(std::sync::Arc::new(baked.clone())),
                        baked_data_parse_meta: baked_data_parse_meta_by_pass.get(layer_id).cloned(),
                        ..Default::default()
                    }),
                )?;

                let params_name: ResourceName = format!("params.{layer_id}").into();
                let params = Params {
                    target_size: [pass_target_w, pass_target_h],
                    geo_size: [geo_w.max(1.0), geo_h.max(1.0)],
                    center: [geo_x, geo_y],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.9, 0.2, 0.2, 1.0],
                };

                let is_instanced = instance_count > 1;

                // Internal resource naming helpers for this pass node.
                let baked_buf_name: ResourceName =
                    format!("sys.pass.{layer_id}.baked_data_parse").into();

                let baked_arc = std::sync::Arc::new(baked);
                let translate_expr_wgsl = translate_expr.map(|e| e.expr);
                let vertex_inline_stmts_for_bundle = vertex_inline_stmts.clone();
                let vertex_wgsl_decls_for_bundle = vertex_wgsl_decls.clone();
                let vertex_graph_input_kinds_for_bundle = vertex_graph_input_kinds.clone();

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
                    rect_dyn_2.clone(),
                    vertex_graph_input_kinds_for_bundle.clone(),
                    None,
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
                            rect_dyn_2.clone(),
                            vertex_graph_input_kinds_for_bundle.clone(),
                            Some(kind),
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

                texture_bindings.extend(
                    crate::renderer::render_plan::resolve_pass_texture_bindings(
                        &pass_output_registry,
                        &bundle.pass_textures,
                    )?,
                );

                let instance_buffer = if is_instanced {
                    let b: ResourceName = format!("sys.pass.{layer_id}.instances").into();

                    // Per-instance mat4 (column-major) as 4 vec4<f32> (16 floats).
                    // If SetTransform provides per-instance CPU-baked matrices, prefer them.
                    let mats: Vec<[f32; 16]> = if let Some(mats) = instance_mats_2 {
                        mats
                    } else {
                        let mut mats: Vec<[f32; 16]> = Vec::with_capacity(instance_count as usize);
                        for _ in 0..instance_count {
                            mats.push(base_m);
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
                    pass_id: layer_id.clone(),
                    name: pass_name.clone(),
                    geometry_buffer: geometry_buffer.clone(),
                    instance_buffer,
                    target_texture: pass_output_texture.clone(),
                    params_buffer: params_name,
                    baked_data_parse_buffer,
                    params,
                    graph_binding,
                    graph_values,
                    shader_wgsl,
                    texture_bindings,
                    sampler_kind: SamplerKind::NearestClamp,
                    blend_state,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                });
                composite_passes.push(pass_name);

                // If a pass is sampled (so it renders to sys.pass.<id>.out) but is also a direct
                // composite layer, we must still draw it into Composite.target.
                //
                // Strategy: add a dedicated compose pass that samples sys.pass.<id>.out and blends it
                // into Composite.target at the correct layer position.
                if is_sampled_output && is_composite_layer {
                    let compose_pass_name: ResourceName =
                        format!("sys.pass.{layer_id}.compose.pass").into();
                    let compose_params_name: ResourceName =
                        format!("params.sys.pass.{layer_id}.compose").into();

                    // If the sampled output is target-sized (legacy), compose with a fullscreen quad.
                    // If the sampled output is geometry-sized (Downsample optimization), compose with the
                    // original geometry so it lands at the correct screen-space position.
                    let (compose_geometry_buffer, compose_params_val) = if pass_target_w_u
                        == tgt_w_u
                        && pass_target_h_u == tgt_h_u
                    {
                        let compose_geo: ResourceName =
                            format!("sys.pass.{layer_id}.compose.geo").into();
                        geometry_buffers
                            .push((compose_geo.clone(), make_fullscreen_geometry(tgt_w, tgt_h)));
                        (
                            compose_geo,
                            Params {
                                target_size: [tgt_w, tgt_h],
                                geo_size: [tgt_w, tgt_h],
                                center: [tgt_w * 0.5, tgt_h * 0.5],
                                geo_translate: [0.0, 0.0],
                                geo_scale: [1.0, 1.0],
                                time: 0.0,
                                _pad0: 0.0,
                                color: [0.0, 0.0, 0.0, 0.0],
                            },
                        )
                    } else {
                        (
                            geometry_buffer.clone(),
                            Params {
                                target_size: [tgt_w, tgt_h],
                                geo_size: [geo_w.max(1.0), geo_h.max(1.0)],
                                center: [geo_x, geo_y],
                                geo_translate: [0.0, 0.0],
                                geo_scale: [1.0, 1.0],
                                time: 0.0,
                                _pad0: 0.0,
                                color: [0.0, 0.0, 0.0, 0.0],
                            },
                        )
                    };

                    // Sample render-target textures with a Y flip (same convention as PassTexture).
                    let fragment_body =
                        "let uv = vec2f(in.uv.x, 1.0 - in.uv.y);\n    return textureSample(src_tex, src_samp, uv);"
                            .to_string();
                    let bundle = build_fullscreen_textured_bundle(fragment_body);

                    render_pass_specs.push(RenderPassSpec {
                        pass_id: compose_pass_name.as_str().to_string(),
                        name: compose_pass_name.clone(),
                        geometry_buffer: compose_geometry_buffer,
                        instance_buffer: None,
                        target_texture: target_texture_name.clone(),
                        params_buffer: compose_params_name,
                        baked_data_parse_buffer: None,
                        params: compose_params_val,
                        graph_binding: None,
                        graph_values: None,
                        shader_wgsl: bundle.module,
                        texture_bindings: vec![PassTextureBinding {
                            texture: pass_output_texture.clone(),
                            image_node_id: None,
                        }],
                        sampler_kind: SamplerKind::NearestClamp,
                        blend_state,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                    });
                    composite_passes.push(compose_pass_name);
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
                // GuassianBlurPass takes its source from `image` input (color type).
                // This can be from PassTexture (sampling another pass), ImageTexture, or any color expression.
                // We first render the image expression to an intermediate texture, then apply the blur chain.

                // Create source texture for the image input.
                let src_tex: ResourceName = format!("sys.blur.{layer_id}.src").into();
                let src_resolution = [tgt_w as u32, tgt_h as u32];
                textures.push(TextureDecl {
                    name: src_tex.clone(),
                    size: src_resolution,
                    format: sampled_pass_format,
                });

                // Build a fullscreen pass to render the `image` input expression.
                let geo_src: ResourceName = format!("sys.blur.{layer_id}.src.geo").into();
                geometry_buffers.push((geo_src.clone(), make_fullscreen_geometry(tgt_w, tgt_h)));

                let params_src: ResourceName = format!("params.sys.blur.{layer_id}.src").into();
                let params_src_val = Params {
                    target_size: [tgt_w, tgt_h],
                    geo_size: [tgt_w, tgt_h],
                    // Bottom-left origin: center the geometry so it covers [0,0] to [w,h].
                    center: [tgt_w * 0.5, tgt_h * 0.5],
                    geo_translate: [0.0, 0.0],
                    geo_scale: [1.0, 1.0],
                    time: 0.0,
                    _pad0: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                };

                // Build WGSL for the image input expression (similar to RenderPass material).
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

                src_texture_bindings.extend(
                    crate::renderer::render_plan::resolve_pass_texture_bindings(
                        &pass_output_registry,
                        &src_bundle.pass_textures,
                    )?,
                );

                let src_pass_name: ResourceName = format!("sys.blur.{layer_id}.src.pass").into();
                render_pass_specs.push(RenderPassSpec {
                    pass_id: src_pass_name.as_str().to_string(),
                    name: src_pass_name.clone(),
                    geometry_buffer: geo_src,
                    instance_buffer: None,
                    target_texture: src_tex.clone(),
                    params_buffer: params_src.clone(),
                    baked_data_parse_buffer: None,
                    params: params_src_val,
                    graph_binding: src_graph_binding,
                    graph_values: src_graph_values,
                    shader_wgsl: src_bundle.module,
                    texture_bindings: src_texture_bindings,
                    sampler_kind: SamplerKind::NearestClamp,
                    blend_state: BlendState::REPLACE,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                });
                composite_passes.push(src_pass_name);

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

                // SceneDSL `radius` is authored as an analytic 1D cutoff radius in full-res pixels,
                // not as Gaussian sigma.
                //
                // We map radius -> sigma using the same cutoff epsilon (~0.002) that our packed
                // 27-wide Gaussian kernel effectively uses when pruning tiny weights
                // (see `gaussian_kernel_8`).
                //
                // k = sqrt(2*ln(1/eps)) with eps=0.002 -> k≈3.525494, so sigma = radius/k.
                let radius_px =
                    cpu_num_f32_min_0(&prepared.scene, nodes_by_id, layer_node, "radius", 0.0)?;
                let sigma = radius_px / 3.525_494;
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
                    let tex: ResourceName = format!("sys.blur.{layer_id}.ds.{step}").into();
                    textures.push(TextureDecl {
                        name: tex.clone(),
                        size: [next_w, next_h],
                        format: sampled_pass_format,
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
                });
                textures.push(TextureDecl {
                    name: v_tex.clone(),
                    size: [ds_w, ds_h],
                    format: sampled_pass_format,
                });

                // If this blur pass is sampled downstream (PassTexture), render into an intermediate output.
                // Otherwise, render to the final Composite.target texture.
                let output_tex: ResourceName = if sampled_pass_ids.contains(layer_id) {
                    let out_tex: ResourceName = format!("sys.blur.{layer_id}.out").into();
                    textures.push(TextureDecl {
                        name: out_tex.clone(),
                        size: [blur_w, blur_h],
                        format: sampled_pass_format,
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
                        crate::renderer::render_plan::parse_render_pass_blend_state(
                            &layer_node.params,
                        )?
                    } else {
                        crate::renderer::render_plan::default_blend_state_for_preset("alpha")?
                    }
                } else {
                    BlendState::REPLACE
                };

                // Fullscreen geometry buffers for blur + upsample.
                let geo_ds: ResourceName = format!("sys.blur.{layer_id}.ds.geo").into();
                geometry_buffers.push((
                    geo_ds.clone(),
                    make_fullscreen_geometry(ds_w as f32, ds_h as f32),
                ));
                let geo_out: ResourceName = format!("sys.blur.{layer_id}.out.geo").into();
                geometry_buffers.push((
                    geo_out.clone(),
                    make_fullscreen_geometry(blur_w as f32, blur_h as f32),
                ));

                // Downsample chain
                let mut prev_tex: Option<ResourceName> = None;
                for (step, tex, step_w, step_h, step_geo) in &step_textures {
                    let params_name: ResourceName =
                        format!("params.sys.blur.{layer_id}.ds.{step}").into();
                    let bundle = build_downsample_bundle(*step)?;

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

                    let src_tex = match &prev_tex {
                        None => src_tex.clone(),
                        Some(t) => t.clone(),
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
                        target_texture: tex.clone(),
                        params_buffer: params_name,
                        baked_data_parse_buffer: Some(baked_buf),
                        params: params_val,
                        graph_binding: None,
                        graph_values: None,
                        shader_wgsl: bundle.module,
                        texture_bindings: vec![PassTextureBinding {
                            texture: src_tex,
                            image_node_id: None,
                        }],
                        sampler_kind: SamplerKind::LinearMirror,
                        blend_state: BlendState::REPLACE,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                    });
                    composite_passes.push(pass_name);
                    prev_tex = Some(tex.clone());
                }

                let ds_src_tex: ResourceName = prev_tex
                    .ok_or_else(|| anyhow!("GuassianBlurPass: missing downsample output"))?;

                // 2) Horizontal blur: ds_src_tex -> h_tex
                let params_h: ResourceName =
                    format!("params.sys.blur.{layer_id}.h.ds{downsample_factor}").into();
                let bundle_h = build_horizontal_blur_bundle(kernel, offset);
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
                    target_texture: h_tex.clone(),
                    params_buffer: params_h.clone(),
                    baked_data_parse_buffer: None,
                    params: params_h_val,
                    graph_binding: None,
                    graph_values: None,
                    shader_wgsl: bundle_h.module,
                    texture_bindings: vec![PassTextureBinding {
                        texture: ds_src_tex.clone(),
                        image_node_id: None,
                    }],
                    sampler_kind: SamplerKind::LinearMirror,
                    blend_state: BlendState::REPLACE,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                });
                composite_passes.push(pass_name_h);

                // 3) Vertical blur: h_tex -> v_tex (still downsampled resolution)
                let params_v: ResourceName =
                    format!("params.sys.blur.{layer_id}.v.ds{downsample_factor}").into();
                let bundle_v = build_vertical_blur_bundle(kernel, offset);
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
                    target_texture: v_tex.clone(),
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
                    sampler_kind: SamplerKind::LinearMirror,
                    blend_state: BlendState::REPLACE,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                });

                composite_passes.push(pass_name_v);

                // 4) Upsample bilinear back to output: v_tex -> output_tex
                let params_u: ResourceName =
                    format!("params.sys.blur.{layer_id}.upsample_bilinear.ds{downsample_factor}")
                        .into();
                let bundle_u = build_upsample_bilinear_bundle();
                let params_u_val = Params {
                    target_size: [blur_w as f32, blur_h as f32],
                    geo_size: [blur_w as f32, blur_h as f32],
                    center: [blur_w as f32 * 0.5, blur_h as f32 * 0.5],
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
                    geometry_buffer: geo_out.clone(),
                    instance_buffer: None,
                    target_texture: output_tex.clone(),
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
                    sampler_kind: SamplerKind::LinearMirror,
                    blend_state: blur_output_blend_state,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                });

                composite_passes.push(pass_name_u);

                // Register this GuassianBlurPass output for potential downstream chaining.
                pass_output_registry.register(PassOutputSpec {
                    node_id: layer_id.clone(),
                    texture_name: output_tex,
                    resolution: [blur_w, blur_h],
                    format: if sampled_pass_ids.contains(layer_id) {
                        sampled_pass_format
                    } else {
                        target_format
                    },
                });
            }
            "Downsample" => {
                // Downsample takes its source from `source` (pass), and downsamples into `targetSize`.
                // If sampled downstream (PassTexture), render into an intermediate texture;
                // otherwise render to the Composite target.

                let pass_name: ResourceName = format!("sys.downsample.{layer_id}.pass").into();

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
                    target_texture: downsample_out_tex.clone(),
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
                    sampler_kind,
                    blend_state: BlendState::REPLACE,
                    color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
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
                        target_texture: target_texture_name.clone(),
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
                        sampler_kind: SamplerKind::LinearClamp,
                        blend_state: BlendState::REPLACE,
                        color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
                    });
                    composite_passes.push(upsample_pass_name);
                }

                // Register Downsample output for chaining.
                if is_sampled_output {
                    pass_output_registry.register(PassOutputSpec {
                        node_id: layer_id.clone(),
                        texture_name: downsample_out_tex,
                        resolution: [out_w, out_h],
                        format: sampled_pass_format,
                    });
                }
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
                target_texture: display_tex.clone(),
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
                sampler_kind: SamplerKind::NearestClamp,
                blend_state: BlendState::REPLACE,
                color_load_op: wgpu::LoadOp::Clear(Color::TRANSPARENT),
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
            usage: TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_SRC,
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

            let image = match data_url {
                Some(s) if !s.trim().is_empty() => match load_image_from_data_url(s) {
                    Ok(img) => flip_image_y_rgba8(ensure_rgba8(Arc::new(img))),
                    Err(_e) => placeholder_image(),
                },
                _ => {
                    let path = node.params.get("path").and_then(|v| v.as_str());
                    flip_image_y_rgba8(ensure_rgba8(load_image_with_fallback(&rel_base, path)))
                }
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
                let dst_format = TextureFormat::Rgba8Unorm;
                texture_specs.push(FiberTextureSpec::Texture {
                    name: name.clone(),
                    resolution: [img_w, img_h],
                    format: dst_format,
                    usage: TextureUsages::RENDER_ATTACHMENT
                        | TextureUsages::TEXTURE_BINDING
                        | TextureUsages::COPY_SRC,
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

    for spec in &render_pass_specs {
        let geometry_buffer = spec.geometry_buffer.clone();
        let target_texture = spec.target_texture.clone();
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
        let sampler_name = match spec.sampler_kind {
            SamplerKind::NearestClamp => nearest_sampler.clone(),
            SamplerKind::LinearMirror => linear_mirror_sampler.clone(),
            SamplerKind::LinearRepeat => linear_repeat_sampler.clone(),
            SamplerKind::LinearClamp => linear_clamp_sampler.clone(),
        };

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
    fn render_pass_blend_state_from_preset_premul_alpha() {
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        params.insert("blend_preset".to_string(), json!("premul-alpha"));

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
        use base64::{engine::general_purpose, Engine as _};
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
