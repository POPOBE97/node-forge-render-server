use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};
use rust_wgpu_fiber::ResourceName;

use crate::{
    dsl::{SceneDSL, find_node, incoming_connection},
    renderer::{
        node_compiler::compile_vertex_expr,
        types::{BakedValue, GraphFieldKind, MaterialCompileContext, TypedExpr, ValueType},
        utils::{coerce_to_type, cpu_num_u32_min_1},
    },
};

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

fn parse_json_number_f32(v: &serde_json::Value) -> Option<f32> {
    v.as_f64()
        .map(|x| x as f32)
        .or_else(|| v.as_i64().map(|x| x as f32))
        .or_else(|| v.as_u64().map(|x| x as f32))
}

fn parse_inline_vec2(node: &crate::dsl::Node, key: &str) -> Result<Option<[f32; 2]>> {
    let Some(v) = node.params.get(key) else {
        return Ok(None);
    };

    if let Some(arr) = v.as_array() {
        let x = arr.first().and_then(parse_json_number_f32).unwrap_or(0.0);
        let y = arr.get(1).and_then(parse_json_number_f32).unwrap_or(0.0);
        return Ok(Some([x, y]));
    }

    if let Some(obj) = v.as_object() {
        let x = obj.get("x").and_then(parse_json_number_f32).unwrap_or(0.0);
        let y = obj.get("y").and_then(parse_json_number_f32).unwrap_or(0.0);
        return Ok(Some([x, y]));
    }

    bail!(
        "{}.{} must be vec2 object {{x,y}} or array [x,y]",
        node.id,
        key
    );
}

fn parse_vec2_literal_expr(expr: &str) -> Option<[f32; 2]> {
    let compact = expr.replace([' ', '\n', '\t', '\r'], "");
    let inner = compact.strip_prefix("vec2f(")?.strip_suffix(')')?;
    let mut parts = inner.split(',');
    let x = parts.next()?.parse::<f32>().ok()?;
    let y = parts.next()?.parse::<f32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some([x, y])
}

fn resolve_rect_vec2_input(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    node: &crate::dsl::Node,
    port_id: &str,
    material_ctx: Option<&MaterialCompileContext>,
) -> Result<Option<[f32; 2]>> {
    if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
        let mut ctx = MaterialCompileContext {
            baked_data_parse: material_ctx.and_then(|c| c.baked_data_parse.clone()),
            baked_data_parse_meta: material_ctx.and_then(|c| c.baked_data_parse_meta.clone()),
            ..Default::default()
        };
        let mut cache: HashMap<(String, String), TypedExpr> = HashMap::new();

        let expr = compile_vertex_expr(
            scene,
            nodes_by_id,
            &conn.from.node_id,
            Some(&conn.from.port_id),
            &mut ctx,
            &mut cache,
        )?;
        let expr = coerce_to_type(expr, ValueType::Vec2)?;
        let parsed = parse_vec2_literal_expr(&expr.expr).ok_or_else(|| {
            anyhow!(
                "{}.{} must resolve to literal vec2 for CPU geometry allocation, got {}",
                node.id,
                port_id,
                expr.expr
            )
        })?;
        return Ok(Some(parsed));
    }

    parse_inline_vec2(node, port_id)
}

fn resolve_rect2d_geometry_metrics(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    node: &crate::dsl::Node,
    render_target_size: [f32; 2],
    material_ctx: Option<&MaterialCompileContext>,
) -> Result<(f32, f32, f32, f32)> {
    let size = resolve_rect_vec2_input(scene, nodes_by_id, node, "size", material_ctx)?;
    let position = resolve_rect_vec2_input(scene, nodes_by_id, node, "position", material_ctx)?;

    let default_w = render_target_size[0].max(1.0);
    let default_h = render_target_size[1].max(1.0);
    let default_size = [default_w, default_h];
    let default_position = [default_w * 0.5, default_h * 0.5];

    let size = size.unwrap_or(default_size);
    let position = position.unwrap_or(default_position);

    Ok((size[0].max(1.0), size[1].max(1.0), position[0], position[1]))
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
    std::collections::BTreeMap<String, GraphFieldKind>,
    bool,
)> {
    let geometry_node = find_node(nodes_by_id, geometry_node_id)?;

    match geometry_node.node_type.as_str() {
        "Rect2DGeometry" => {
            let geometry_buffer = ids
                .get(geometry_node_id)
                .cloned()
                .ok_or_else(|| anyhow!("missing name for node: {}", geometry_node_id))?;

            let (geo_w, geo_h, geo_x, geo_y) = resolve_rect2d_geometry_metrics(
                scene,
                nodes_by_id,
                geometry_node,
                render_target_size,
                material_ctx,
            )?;
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
                None,
                Vec::new(),
                String::new(),
                std::collections::BTreeMap::new(),
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
                instance_mats,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                graph_input_kinds,
                uses_instance_index,
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
                graph_input_kinds,
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
                instance_mats,
                translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                graph_input_kinds,
                uses_instance_index,
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
                graph_input_kinds,
                uses_instance_index,
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
                _graph_input_kinds,
                uses_instance_index,
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
                std::collections::BTreeMap::new(),
                uses_instance_index,
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
                mut graph_input_kinds,
                mut uses_instance_index,
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
            let mut local_graph_input_kinds: std::collections::BTreeMap<String, GraphFieldKind> =
                std::collections::BTreeMap::new();

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
                local_graph_input_kinds = vtx_ctx.graph_input_kinds.clone();
                translate_expr = Some(expr);
            }

            if !local_inline_stmts.is_empty() {
                vtx_inline_stmts = local_inline_stmts;
                vtx_wgsl_decls = local_wgsl_decls;
            }
            if local_uses_instance_index {
                uses_instance_index = true;
            }
            if !local_graph_input_kinds.is_empty() {
                graph_input_kinds = local_graph_input_kinds;
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
                graph_input_kinds,
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
