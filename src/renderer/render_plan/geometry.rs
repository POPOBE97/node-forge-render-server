use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use rust_wgpu_fiber::ResourceName;

use crate::{
    asset_store::AssetStore,
    dsl::{SceneDSL, find_node, incoming_connection},
    renderer::{
        camera::resolve_mat4_output_column_major,
        graph_uniforms::graph_field_name,
        node_compiler::compile_vertex_expr,
        node_compiler::geometry_nodes::load_geometry_from_asset,
        types::{BakedValue, GraphFieldKind, MaterialCompileContext, TypedExpr, ValueType},
        utils::{
            IDENTITY_MAT4, coerce_to_type, cpu_num_f32, cpu_num_u32_min_1,
            fmt_f32 as fmt_f32_utils, parse_strict_mat4_param_column_major,
        },
    },
};

#[derive(Debug, Clone)]
pub(crate) struct Rect2DDynamicInputs {
    pub(crate) position_expr: Option<TypedExpr>,
    pub(crate) size_expr: Option<TypedExpr>,
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedGltfGeometry {
    pub(crate) vertices: Vec<[f32; 5]>,
    pub(crate) normals_bytes: Option<Arc<[u8]>>,
    pub(crate) size_px: [f32; 2],
}

pub(crate) fn load_gltf_geometry_pixel_space(
    scene: &SceneDSL,
    geometry_node_id: &str,
    geometry_node: &crate::dsl::Node,
    render_target_size: [f32; 2],
    asset_store: &AssetStore,
) -> Result<LoadedGltfGeometry> {
    let asset_id = geometry_node
        .params
        .get("assetId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            anyhow!(
                "GLTFGeometry.params.assetId must be a non-empty string (node {})",
                geometry_node_id
            )
        })?;

    let entry = scene
        .assets
        .get(asset_id)
        .ok_or_else(|| anyhow!("GLTFGeometry asset not found: {asset_id}"))?;

    let file_path = if entry.original_name.is_empty() {
        &entry.path
    } else {
        &entry.original_name
    };
    let lower = file_path.to_ascii_lowercase();
    if !lower.ends_with(".gltf") && !lower.ends_with(".glb") && !lower.ends_with(".obj") {
        bail!("GLTFGeometry only supports .gltf/.glb/.obj assets, got: {file_path}");
    }

    let data = asset_store.get(asset_id).ok_or_else(|| {
        anyhow!(
            "GLTFGeometry node '{geometry_node_id}': asset '{asset_id}' not found in asset store"
        )
    })?;
    let (verts, normals) = load_geometry_from_asset(&data.bytes, file_path)?;

    // Model is in normalized coordinates (roughly -1..1 from DDC).
    // Scale to pixel space by multiplying by half the render target size.
    let [tgt_w, tgt_h] = render_target_size;
    let half_w = tgt_w * 0.5;
    // Intentionally use isotropic XY scaling in pixel space to preserve source geometry aspect.
    let vertices: Vec<[f32; 5]> = verts
        .into_iter()
        .map(|v| [v[0] * half_w, v[1] * half_w, v[2] * half_w, v[3], v[4]])
        .collect();

    let normals_bytes =
        normals.map(|n| Arc::from(bytemuck::cast_slice::<[f32; 3], u8>(&n).to_vec()));

    Ok(LoadedGltfGeometry {
        vertices,
        normals_bytes,
        size_px: [tgt_w, tgt_h],
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

fn mat4_rotate_x(rad: f32) -> [f32; 16] {
    let c = rad.cos();
    let s = rad.sin();
    [
        1.0, 0.0, 0.0, 0.0, 0.0, c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn mat4_rotate_y(rad: f32) -> [f32; 16] {
    let c = rad.cos();
    let s = rad.sin();
    [
        c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn compose_trs_matrix(translate: [f32; 3], rotate_deg: [f32; 3], scale: [f32; 3]) -> [f32; 16] {
    // Column-vector convention:
    // local = T * Rz * Ry * Rx * S * p
    // so authored XYZ rotation means apply X, then Y, then Z.
    let rx = rotate_deg[0].to_radians();
    let ry = rotate_deg[1].to_radians();
    let rz = rotate_deg[2].to_radians();

    mat4_mul(
        mat4_translate(translate[0], translate[1], translate[2]),
        mat4_mul(
            mat4_rotate_z(rz),
            mat4_mul(
                mat4_rotate_y(ry),
                mat4_mul(mat4_rotate_x(rx), mat4_scale(scale[0], scale[1], scale[2])),
            ),
        ),
    )
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

/// Read CPU values from a connected Vector2Input node.
/// Returns the (x, y) values if the port is connected to a Vector2Input node.
/// Returns Ok(None) for dangling connections (target node not found) or non-Vector2Input sources.
fn get_vec2_from_connected_vector2input(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    node: &crate::dsl::Node,
    port_id: &str,
) -> Result<Option<[f32; 2]>> {
    let Some(conn) = incoming_connection(scene, &node.id, port_id) else {
        return Ok(None);
    };

    // Handle dangling connection: target node not found.
    let Ok(from_node) = find_node(nodes_by_id, &conn.from.node_id) else {
        return Ok(None);
    };

    if from_node.node_type != "Vector2Input" {
        // Not a Vector2Input; caller should fallback to other resolution methods.
        return Ok(None);
    }

    // Read the x/y values from the Vector2Input node's params.
    let x = cpu_num_f32(scene, nodes_by_id, from_node, "x", 0.0)?;
    let y = cpu_num_f32(scene, nodes_by_id, from_node, "y", 0.0)?;
    Ok(Some([x, y]))
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

fn resolve_rect2d_geometry_metrics(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    node: &crate::dsl::Node,
    render_target_size: [f32; 2],
    _material_ctx: Option<&MaterialCompileContext>,
) -> Result<(
    f32,
    f32,
    f32,
    f32,
    Option<Rect2DDynamicInputs>,
    Vec<String>,
    String,
    std::collections::BTreeMap<String, GraphFieldKind>,
    bool,
)> {
    let default_w = render_target_size[0].max(1.0);
    let default_h = render_target_size[1].max(1.0);
    let default_position = [default_w * 0.5, default_h * 0.5];

    let mut dyn_inputs: Option<Rect2DDynamicInputs> = None;
    let vertex_inline_stmts: Vec<String> = Vec::new();
    let vertex_wgsl_decls = String::new();
    let mut vertex_graph_input_kinds: std::collections::BTreeMap<String, GraphFieldKind> =
        std::collections::BTreeMap::new();
    let vertex_uses_instance_index = false;

    // If Rect2DGeometry.position/size are connected to Vector2Input nodes, route them via
    // the GraphInputs buffer mechanism (graph_inputs.<field>.xy).
    // If the connection is dangling (target node not found), fall back to fullscreen geometry.
    // If connected to wrong node type, bail with an error.
    let mut maybe_dyn_inputs = Rect2DDynamicInputs {
        position_expr: None,
        size_expr: None,
    };

    let mut has_any_dyn = false;
    for (port_id, out_expr) in [
        ("position", &mut maybe_dyn_inputs.position_expr),
        ("size", &mut maybe_dyn_inputs.size_expr),
    ] {
        let Some(conn) = incoming_connection(scene, &node.id, port_id) else {
            continue;
        };

        // Check if the target node exists; if not, treat as dangling and use fullscreen fallback.
        let Ok(from_node) = find_node(nodes_by_id, &conn.from.node_id) else {
            // Dangling connection: target node not found. Fall back to fullscreen geometry.
            continue;
        };

        if from_node.node_type != "Vector2Input" {
            bail!(
                "{}.{} only supports Vector2Input connection; got {} ({})",
                node.id,
                port_id,
                from_node.node_type,
                conn.from.node_id
            );
        }
        if conn.from.port_id != "vector" {
            bail!(
                "{}.{} must be connected from Vector2Input.vector; got {}.{}",
                node.id,
                port_id,
                conn.from.node_id,
                conn.from.port_id
            );
        }

        // Valid Vector2Input connection.
        has_any_dyn = true;
        vertex_graph_input_kinds
            .entry(conn.from.node_id.clone())
            .or_insert(GraphFieldKind::Vec2);
        let field = graph_field_name(&conn.from.node_id);
        *out_expr = Some(TypedExpr::new(
            format!("(graph_inputs.{field}).xy"),
            ValueType::Vec2,
        ));
    }

    if has_any_dyn {
        dyn_inputs = Some(maybe_dyn_inputs);
    }

    // CPU metrics for texture allocation:
    // - Connected Vector2Input wins.
    // - If unconnected, inline params win.
    // - Otherwise, fall back to coord-domain fullscreen defaults.
    let size_connected = get_vec2_from_connected_vector2input(scene, nodes_by_id, node, "size")?;
    let position_connected =
        get_vec2_from_connected_vector2input(scene, nodes_by_id, node, "position")?;
    let has_size_conn = incoming_connection(scene, &node.id, "size").is_some();
    let has_position_conn = incoming_connection(scene, &node.id, "position").is_some();

    let size_inline = if has_size_conn {
        None
    } else {
        parse_inline_vec2(node, "size")?
    };
    let position_inline = if has_position_conn {
        None
    } else {
        parse_inline_vec2(node, "position")?
    };

    let size = size_connected
        .or(size_inline)
        .unwrap_or([default_w, default_h]);
    let position = position_connected
        .or(position_inline)
        .unwrap_or(default_position);

    let size = [size[0].max(1.0), size[1].max(1.0)];

    Ok((
        size[0],
        size[1],
        position[0],
        position[1],
        dyn_inputs,
        vertex_inline_stmts,
        vertex_wgsl_decls,
        vertex_graph_input_kinds,
        vertex_uses_instance_index,
    ))
}

fn parse_inline_mat4_column_major(node: &crate::dsl::Node, key: &str) -> Result<Option<[f32; 16]>> {
    parse_strict_mat4_param_column_major(&node.params, key, &format!("{}.{}", node.id, key))
}

fn compute_trs_matrix(node: &crate::dsl::Node) -> [f32; 16] {
    // Note: rotate is authored in degrees.
    let t = parse_inline_vec3(node, "translate", [0.0, 0.0, 0.0]);
    let s = parse_inline_vec3(node, "scale", [1.0, 1.0, 1.0]);
    let r = parse_inline_vec3(node, "rotate", [0.0, 0.0, 0.0]);
    compose_trs_matrix(t, r, s)
}

fn compute_set_transform_matrix(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    node: &crate::dsl::Node,
) -> Result<[f32; 16]> {
    let mode = node
        .params
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("Components");

    match mode {
        "Matrix" => {
            if let Some(conn) = incoming_connection(scene, &node.id, "matrix") {
                resolve_mat4_output_column_major(
                    scene,
                    nodes_by_id,
                    &conn.from.node_id,
                    &conn.from.port_id,
                )
                .map_err(|e| {
                    anyhow!(
                        "SetTransform {}.matrix failed to resolve connected mat4 from {}.{}: {e:#}",
                        node.id,
                        conn.from.node_id,
                        conn.from.port_id
                    )
                })
            } else if let Some(m) = parse_inline_mat4_column_major(node, "matrix")? {
                Ok(m)
            } else {
                Ok(IDENTITY_MAT4)
            }
        }
        _ => Ok(compute_trs_matrix(node)),
    }
}

fn vec3_literal(v: [f32; 3]) -> String {
    format!(
        "vec3f({}, {}, {})",
        fmt_f32_utils(v[0]),
        fmt_f32_utils(v[1]),
        fmt_f32_utils(v[2])
    )
}

const SYS_APPLY_TRS_XYZ_WGSL: &str = r#"
fn sys_apply_trs_xyz(p: vec3f, t: vec3f, r_deg: vec3f, s: vec3f) -> vec3f {
    let rad = r_deg * 0.017453292519943295;

    let cx = cos(rad.x);
    let sx = sin(rad.x);
    let cy = cos(rad.y);
    let sy = sin(rad.y);
    let cz = cos(rad.z);
    let sz = sin(rad.z);

    let p0 = p * s;
    let p1 = vec3f(p0.x, p0.y * cx - p0.z * sx, p0.y * sx + p0.z * cx);
    let p2 = vec3f(p1.x * cy + p1.z * sy, p1.y, -p1.x * sy + p1.z * cy);
    let p3 = vec3f(p2.x * cz - p2.y * sz, p2.x * sz + p2.y * cz, p2.z);
    return p3 + t;
}
"#;

fn merge_vertex_wgsl_decls(mut base: String, extra: String) -> String {
    if extra.trim().is_empty() {
        return base;
    }
    if base.trim().is_empty() {
        return extra;
    }
    if !base.ends_with('\n') {
        base.push('\n');
    }
    base.push_str(&extra);
    base
}

fn ensure_trs_helper_decl(mut decls: String) -> String {
    if !decls.contains("fn sys_apply_trs_xyz(") {
        if !decls.is_empty() && !decls.ends_with('\n') {
            decls.push('\n');
        }
        decls.push_str(SYS_APPLY_TRS_XYZ_WGSL);
        if !decls.ends_with('\n') {
            decls.push('\n');
        }
    }
    decls
}

fn merge_graph_input_kinds(
    mut base: std::collections::BTreeMap<String, GraphFieldKind>,
    extra: std::collections::BTreeMap<String, GraphFieldKind>,
) -> std::collections::BTreeMap<String, GraphFieldKind> {
    for (node_id, kind) in extra {
        base.entry(node_id).or_insert(kind);
    }
    base
}

#[derive(Debug)]
struct DynamicTransformExprData {
    translate_expr: TypedExpr,
    vertex_inline_stmts: Vec<String>,
    vertex_wgsl_decls: String,
    vertex_graph_input_kinds: std::collections::BTreeMap<String, GraphFieldKind>,
    vertex_uses_instance_index: bool,
}

fn compile_vector3_input_with_component_links(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    vector3_node: &crate::dsl::Node,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    let mut field_name: Option<String> = None;
    let mut uses_time = false;
    let mut components: Vec<String> = Vec::with_capacity(3);

    for axis in ["x", "y", "z"] {
        let comp_expr = if let Some(comp_conn) = incoming_connection(scene, &vector3_node.id, axis)
        {
            let raw = compile_vertex_expr(
                scene,
                nodes_by_id,
                &comp_conn.from.node_id,
                Some(&comp_conn.from.port_id),
                ctx,
                cache,
            )?;
            coerce_to_type(raw, ValueType::F32)?
        } else {
            if field_name.is_none() {
                ctx.register_graph_input(&vector3_node.id, GraphFieldKind::Vec3);
                field_name = Some(graph_field_name(&vector3_node.id));
            }
            let field = field_name
                .as_ref()
                .expect("field_name must be initialized above");
            TypedExpr::new(format!("(graph_inputs.{field}).{axis}"), ValueType::F32)
        };
        uses_time |= comp_expr.uses_time;
        components.push(comp_expr.expr);
    }

    Ok(TypedExpr::with_time(
        format!(
            "vec3f({}, {}, {})",
            components[0], components[1], components[2]
        ),
        ValueType::Vec3,
        uses_time,
    ))
}

fn compile_transform_connection_vec3_expr(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    conn: &crate::dsl::Connection,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    let from_node = find_node(nodes_by_id, &conn.from.node_id)?;
    let raw_expr = if from_node.node_type == "Vector3Input" && conn.from.port_id == "vector" {
        compile_vector3_input_with_component_links(scene, nodes_by_id, from_node, ctx, cache)?
    } else {
        compile_vertex_expr(
            scene,
            nodes_by_id,
            &conn.from.node_id,
            Some(&conn.from.port_id),
            ctx,
            cache,
        )?
    };

    coerce_to_type(raw_expr, ValueType::Vec3)
}

fn compile_dynamic_trs_delta_expr(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    material_ctx: Option<&MaterialCompileContext>,
    translate_conn: Option<&crate::dsl::Connection>,
    rotate_conn: Option<&crate::dsl::Connection>,
    scale_conn: Option<&crate::dsl::Connection>,
    inline_components_for_set_transform: Option<([f32; 3], [f32; 3], [f32; 3])>,
    base_point_expr: &str,
) -> Result<DynamicTransformExprData> {
    let mut ctx = MaterialCompileContext {
        baked_data_parse: material_ctx.and_then(|m| m.baked_data_parse.clone()),
        baked_data_parse_meta: material_ctx.and_then(|m| m.baked_data_parse_meta.clone()),
        ..Default::default()
    };
    let mut cache: HashMap<(String, String), TypedExpr> = HashMap::new();

    let translate_conn_expr = if let Some(conn) = translate_conn {
        compile_transform_connection_vec3_expr(scene, nodes_by_id, conn, &mut ctx, &mut cache)?
    } else {
        TypedExpr::new("vec3f(0.0, 0.0, 0.0)", ValueType::Vec3)
    };
    let rotate_conn_expr = if let Some(conn) = rotate_conn {
        compile_transform_connection_vec3_expr(scene, nodes_by_id, conn, &mut ctx, &mut cache)?
    } else {
        TypedExpr::new("vec3f(0.0, 0.0, 0.0)", ValueType::Vec3)
    };
    let scale_conn_expr = if let Some(conn) = scale_conn {
        compile_transform_connection_vec3_expr(scene, nodes_by_id, conn, &mut ctx, &mut cache)?
    } else {
        TypedExpr::new("vec3f(1.0, 1.0, 1.0)", ValueType::Vec3)
    };

    let (translate_expr, rotate_expr, scale_expr) =
        if let Some((t_inline, r_inline, s_inline)) = inline_components_for_set_transform {
            let t = TypedExpr::with_time(
                format!(
                    "(({}) + ({}))",
                    vec3_literal(t_inline),
                    translate_conn_expr.expr
                ),
                ValueType::Vec3,
                translate_conn_expr.uses_time,
            );
            let r = TypedExpr::with_time(
                format!(
                    "(({}) + ({}))",
                    vec3_literal(r_inline),
                    rotate_conn_expr.expr
                ),
                ValueType::Vec3,
                rotate_conn_expr.uses_time,
            );
            let s = TypedExpr::with_time(
                format!(
                    "(({}) * ({}))",
                    vec3_literal(s_inline),
                    scale_conn_expr.expr
                ),
                ValueType::Vec3,
                scale_conn_expr.uses_time,
            );
            (t, r, s)
        } else {
            (translate_conn_expr, rotate_conn_expr, scale_conn_expr)
        };

    let uses_time = translate_expr.uses_time || rotate_expr.uses_time || scale_expr.uses_time;
    let delta_expr = TypedExpr::with_time(
        format!(
            "(sys_apply_trs_xyz({}, {}, {}, {}) - p_local)",
            base_point_expr, translate_expr.expr, rotate_expr.expr, scale_expr.expr
        ),
        ValueType::Vec3,
        uses_time,
    );

    let vertex_wgsl_decls = ensure_trs_helper_decl(ctx.wgsl_decls());
    let vertex_inline_stmts = ctx.inline_stmts;
    let vertex_graph_input_kinds = ctx.graph_input_kinds;
    let vertex_uses_instance_index = ctx.uses_instance_index;

    Ok(DynamicTransformExprData {
        translate_expr: delta_expr,
        vertex_inline_stmts,
        vertex_wgsl_decls,
        vertex_graph_input_kinds,
        vertex_uses_instance_index,
    })
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
    asset_store: Option<&AssetStore>,
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
    Option<Rect2DDynamicInputs>,
    Option<Arc<[u8]>>,
)> {
    let geometry_node = find_node(nodes_by_id, geometry_node_id)?;

    match geometry_node.node_type.as_str() {
        "Rect2DGeometry" => {
            let geometry_buffer = ids
                .get(geometry_node_id)
                .cloned()
                .ok_or_else(|| anyhow!("missing name for node: {}", geometry_node_id))?;

            let (
                geo_w,
                geo_h,
                geo_x,
                geo_y,
                rect_dyn,
                vertex_inline_stmts,
                vertex_wgsl_decls,
                vertex_graph_input_kinds,
                vertex_uses_instance_index,
            ) = resolve_rect2d_geometry_metrics(
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
                vertex_inline_stmts,
                vertex_wgsl_decls,
                vertex_graph_input_kinds,
                vertex_uses_instance_index,
                rect_dyn,
                None,
            ))
        }
        "GLTFGeometry" => {
            let geometry_buffer = ids
                .get(geometry_node_id)
                .cloned()
                .ok_or_else(|| anyhow!("missing name for node: {}", geometry_node_id))?;

            let store = asset_store.ok_or_else(|| {
                anyhow!("GLTFGeometry node '{geometry_node_id}': no asset store provided")
            })?;
            let loaded = load_gltf_geometry_pixel_space(
                scene,
                geometry_node_id,
                geometry_node,
                render_target_size,
                store,
            )?;
            let geo_w = loaded.size_px[0];
            let geo_h = loaded.size_px[1];
            let _verts = loaded.vertices;
            let normals_bytes = loaded.normals_bytes;

            Ok((
                geometry_buffer,
                geo_w,
                geo_h,
                0.0,
                0.0,
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
                None,
                normals_bytes,
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
                rect_dyn,
                normals_bytes,
            ) = resolve_geometry_for_render_pass(
                scene,
                nodes_by_id,
                ids,
                &upstream_geo_id,
                render_target_size,
                material_ctx,
                asset_store,
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
                rect_dyn,
                normals_bytes,
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
                rect_dyn,
                normals_bytes,
            ) = resolve_geometry_for_render_pass(
                scene,
                nodes_by_id,
                ids,
                &upstream_geo_id,
                render_target_size,
                material_ctx,
                asset_store,
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
                rect_dyn,
                normals_bytes,
            ))
        }
        "SetTransform" => {
            // Geometry chain: SetTransform.geometry -> base geometry buffer.
            // Unlike TransformGeometry, this node replaces upstream base transform.

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
                _upstream_base_m,
                _upstream_instance_mats,
                _translate_expr,
                upstream_vtx_inline_stmts,
                upstream_vtx_wgsl_decls,
                upstream_graph_input_kinds,
                upstream_uses_instance_index,
                upstream_rect_dyn,
                upstream_normals_bytes,
            ) = resolve_geometry_for_render_pass(
                scene,
                nodes_by_id,
                ids,
                &upstream_geo_id,
                render_target_size,
                material_ctx,
                asset_store,
            )?;

            let mode = geometry_node
                .params
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("Components");
            let is_matrix_mode = mode == "Matrix";

            let translate_conn = incoming_connection(scene, &geometry_node.id, "translate");
            let scale_conn = incoming_connection(scene, &geometry_node.id, "scale");
            let rotate_conn = incoming_connection(scene, &geometry_node.id, "rotate");
            let has_any_connected =
                translate_conn.is_some() || scale_conn.is_some() || rotate_conn.is_some();

            let mut has_any_baked = false;
            let mut translate_key: Option<(String, String, String)> = None;
            let mut scale_key: Option<(String, String, String)> = None;
            let mut rotate_key: Option<(String, String, String)> = None;
            let mut baked_values: Option<
                &std::collections::HashMap<(String, String, String), Vec<BakedValue>>,
            > = None;

            if let Some(material_ctx) = material_ctx {
                if let (Some(baked), Some(meta)) = (
                    material_ctx.baked_data_parse.as_ref(),
                    material_ctx.baked_data_parse_meta.as_ref(),
                ) {
                    translate_key = translate_conn.map(|conn| {
                        (
                            meta.pass_id.clone(),
                            conn.from.node_id.clone(),
                            conn.from.port_id.clone(),
                        )
                    });
                    scale_key = scale_conn.map(|conn| {
                        (
                            meta.pass_id.clone(),
                            conn.from.node_id.clone(),
                            conn.from.port_id.clone(),
                        )
                    });
                    rotate_key = rotate_conn.map(|conn| {
                        (
                            meta.pass_id.clone(),
                            conn.from.node_id.clone(),
                            conn.from.port_id.clone(),
                        )
                    });
                    has_any_baked = translate_key
                        .as_ref()
                        .is_some_and(|k| baked.contains_key(k))
                        || scale_key.as_ref().is_some_and(|k| baked.contains_key(k))
                        || rotate_key.as_ref().is_some_and(|k| baked.contains_key(k));
                    baked_values = Some(baked);
                }
            }

            // Components mode chooses between:
            // - CPU baked matrix upload when any transform connection is baked (DataParse path)
            // - GPU runtime TRS when there are connected transform ports and none are baked
            //   (e.g. TimeInput-driven graphs).
            if !is_matrix_mode && has_any_connected && !has_any_baked {
                let inline_t = parse_inline_vec3(geometry_node, "translate", [0.0, 0.0, 0.0]);
                let inline_r = parse_inline_vec3(geometry_node, "rotate", [0.0, 0.0, 0.0]);
                let inline_s = parse_inline_vec3(geometry_node, "scale", [1.0, 1.0, 1.0]);

                let dyn_trs = compile_dynamic_trs_delta_expr(
                    scene,
                    nodes_by_id,
                    material_ctx,
                    translate_conn,
                    rotate_conn,
                    scale_conn,
                    Some((inline_t, inline_r, inline_s)),
                    // SetTransform semantics: evaluate from source-space `position`.
                    "position",
                )?;

                // Keep upstream dynamic Rect2D context, but continue to drop upstream transform
                // expressions because SetTransform replaces transform state.
                let (mut vtx_inline_stmts, mut vtx_wgsl_decls, mut graph_input_kinds, rect_dyn) =
                    if upstream_rect_dyn.is_some() {
                        (
                            upstream_vtx_inline_stmts,
                            upstream_vtx_wgsl_decls,
                            upstream_graph_input_kinds,
                            upstream_rect_dyn,
                        )
                    } else {
                        (
                            Vec::new(),
                            String::new(),
                            std::collections::BTreeMap::new(),
                            None,
                        )
                    };

                vtx_inline_stmts.extend(dyn_trs.vertex_inline_stmts);
                vtx_wgsl_decls = merge_vertex_wgsl_decls(vtx_wgsl_decls, dyn_trs.vertex_wgsl_decls);
                graph_input_kinds =
                    merge_graph_input_kinds(graph_input_kinds, dyn_trs.vertex_graph_input_kinds);
                let uses_instance_index =
                    upstream_uses_instance_index || dyn_trs.vertex_uses_instance_index;

                return Ok((
                    buf,
                    w,
                    h,
                    x,
                    y,
                    instances,
                    // SetTransform replaces upstream matrix path; runtime TRS runs in vertex shader.
                    [
                        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0,
                        1.0,
                    ],
                    None,
                    Some(dyn_trs.translate_expr),
                    vtx_inline_stmts,
                    vtx_wgsl_decls,
                    graph_input_kinds,
                    uses_instance_index,
                    rect_dyn,
                    upstream_normals_bytes,
                ));
            }

            // CPU path: static components/matrix mode and DataParse-baked component paths.
            // SetTransform overrides the accumulated base matrix.
            let m = compute_set_transform_matrix(scene, nodes_by_id, geometry_node)?;

            let mut instance_mats: Option<Vec<[f32; 16]>> = None;
            if has_any_baked {
                let t_inline = parse_inline_vec3(geometry_node, "translate", [0.0, 0.0, 0.0]);
                let s_inline = parse_inline_vec3(geometry_node, "scale", [1.0, 1.0, 1.0]);
                let r_inline = parse_inline_vec3(geometry_node, "rotate", [0.0, 0.0, 0.0]);

                let baked = baked_values.expect("baked_values must exist when has_any_baked");
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

                    // SetTransform connected components are deltas on top of inline params.
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

                    mats.push(compose_trs_matrix(t, r, s));
                }
                instance_mats = Some(mats);
            }

            // CPU path keeps prior behavior: no runtime translate expression forwarding.
            let (vtx_inline_stmts, vtx_wgsl_decls, graph_input_kinds, rect_dyn) =
                if upstream_rect_dyn.is_some() {
                    (
                        upstream_vtx_inline_stmts,
                        upstream_vtx_wgsl_decls,
                        upstream_graph_input_kinds,
                        upstream_rect_dyn,
                    )
                } else {
                    (
                        Vec::new(),
                        String::new(),
                        std::collections::BTreeMap::new(),
                        None,
                    )
                };
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
                vtx_inline_stmts,
                vtx_wgsl_decls,
                graph_input_kinds,
                upstream_uses_instance_index,
                rect_dyn,
                upstream_normals_bytes,
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
                w,
                h,
                x,
                y,
                instances,
                upstream_base_m,
                upstream_instance_mats,
                _upstream_translate_expr,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                graph_input_kinds,
                uses_instance_index,
                rect_dyn,
                normals_bytes,
            ) = resolve_geometry_for_render_pass(
                scene,
                nodes_by_id,
                ids,
                &upstream_geo_id,
                render_target_size,
                material_ctx,
                asset_store,
            )?;

            let mode = geometry_node
                .params
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("Components");
            let is_matrix_mode = mode == "Matrix";

            let translate_conn = incoming_connection(scene, &geometry_node.id, "translate");
            let scale_conn = incoming_connection(scene, &geometry_node.id, "scale");
            let rotate_conn = incoming_connection(scene, &geometry_node.id, "rotate");
            let has_any_connected =
                translate_conn.is_some() || scale_conn.is_some() || rotate_conn.is_some();

            let mut base_m = upstream_base_m;
            let mut instance_mats = upstream_instance_mats;
            let inline_local_m = if is_matrix_mode {
                if let Some(conn) = incoming_connection(scene, &geometry_node.id, "matrix") {
                    resolve_mat4_output_column_major(
                        scene,
                        nodes_by_id,
                        &conn.from.node_id,
                        &conn.from.port_id,
                    )
                    .map_err(|e| {
                        anyhow!(
                            "TransformGeometry {}.matrix failed to resolve connected mat4 from {}.{}: {e:#}",
                            geometry_node.id,
                            conn.from.node_id,
                            conn.from.port_id
                        )
                    })?
                } else {
                    parse_inline_mat4_column_major(geometry_node, "matrix")?
                        .unwrap_or(IDENTITY_MAT4)
                }
            } else {
                compute_trs_matrix(geometry_node)
            };

            // TransformGeometry and SetTransform now share the same matrix upload path:
            // local transform is composed CPU-side and applied via instance/base matrices.
            base_m = mat4_mul(inline_local_m, base_m);
            if let Some(mats) = instance_mats.as_mut() {
                for m in mats.iter_mut() {
                    *m = mat4_mul(inline_local_m, *m);
                }
            }

            let mut has_any_baked = false;
            let mut translate_key: Option<(String, String, String)> = None;
            let mut scale_key: Option<(String, String, String)> = None;
            let mut rotate_key: Option<(String, String, String)> = None;
            let mut baked_values: Option<
                &std::collections::HashMap<(String, String, String), Vec<BakedValue>>,
            > = None;

            if !is_matrix_mode {
                if let Some(material_ctx) = material_ctx {
                    if let (Some(baked), Some(meta)) = (
                        material_ctx.baked_data_parse.as_ref(),
                        material_ctx.baked_data_parse_meta.as_ref(),
                    ) {
                        translate_key = translate_conn.map(|conn| {
                            (
                                meta.pass_id.clone(),
                                conn.from.node_id.clone(),
                                conn.from.port_id.clone(),
                            )
                        });
                        scale_key = scale_conn.map(|conn| {
                            (
                                meta.pass_id.clone(),
                                conn.from.node_id.clone(),
                                conn.from.port_id.clone(),
                            )
                        });
                        rotate_key = rotate_conn.map(|conn| {
                            (
                                meta.pass_id.clone(),
                                conn.from.node_id.clone(),
                                conn.from.port_id.clone(),
                            )
                        });
                        has_any_baked = translate_key
                            .as_ref()
                            .is_some_and(|k| baked.contains_key(k))
                            || scale_key.as_ref().is_some_and(|k| baked.contains_key(k))
                            || rotate_key.as_ref().is_some_and(|k| baked.contains_key(k));
                        baked_values = Some(baked);
                    }
                }

                // Components mode chooses between CPU baked upload and runtime TRS.
                if has_any_connected && !has_any_baked {
                    let dyn_trs = compile_dynamic_trs_delta_expr(
                        scene,
                        nodes_by_id,
                        material_ctx,
                        translate_conn,
                        rotate_conn,
                        scale_conn,
                        None,
                        // TransformGeometry composes on current local position.
                        "p_local",
                    )?;

                    let mut merged_inline = vtx_inline_stmts;
                    merged_inline.extend(dyn_trs.vertex_inline_stmts);
                    let merged_decls = ensure_trs_helper_decl(merge_vertex_wgsl_decls(
                        vtx_wgsl_decls,
                        dyn_trs.vertex_wgsl_decls,
                    ));
                    let merged_graph_kinds = merge_graph_input_kinds(
                        graph_input_kinds,
                        dyn_trs.vertex_graph_input_kinds,
                    );
                    let merged_uses_instance_index =
                        uses_instance_index || dyn_trs.vertex_uses_instance_index;

                    return Ok((
                        buf,
                        w,
                        h,
                        x,
                        y,
                        instances,
                        base_m,
                        instance_mats,
                        Some(dyn_trs.translate_expr),
                        merged_inline,
                        merged_decls,
                        merged_graph_kinds,
                        merged_uses_instance_index,
                        rect_dyn,
                        normals_bytes,
                    ));
                }

                if has_any_baked {
                    let baked = baked_values.expect("baked_values must exist when has_any_baked");
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

                        let conn_m = compose_trs_matrix(t_conn, r_conn, s_conn);

                        let upstream_m = instance_mats
                            .as_ref()
                            .and_then(|um| um.get(i).copied())
                            .unwrap_or(base_m);
                        mats.push(mat4_mul(conn_m, upstream_m));
                    }
                    instance_mats = Some(mats);
                }
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
                None,
                vtx_inline_stmts,
                vtx_wgsl_decls,
                graph_input_kinds,
                uses_instance_index,
                rect_dyn,
                normals_bytes,
            ))
        }
        other => {
            bail!(
                "RenderPass.geometry must resolve to Rect2DGeometry/GLTFGeometry/TransformGeometry/InstancedGeometryEnd, got {other}"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rust_wgpu_fiber::ResourceName;
    use serde_json::json;

    use crate::dsl::{Connection, Endpoint, Metadata, Node, SceneDSL};

    use super::{compute_trs_matrix, resolve_geometry_for_render_pass};

    fn node(id: &str, node_type: &str, params: serde_json::Value) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params: params
                .as_object()
                .cloned()
                .map(|m| m.into_iter().collect())
                .unwrap_or_default(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_bindings: Vec::new(),
        }
    }

    fn conn(
        id: &str,
        from_node: &str,
        from_port: &str,
        to_node: &str,
        to_port: &str,
    ) -> Connection {
        Connection {
            id: id.to_string(),
            from: Endpoint {
                node_id: from_node.to_string(),
                port_id: from_port.to_string(),
            },
            to: Endpoint {
                node_id: to_node.to_string(),
                port_id: to_port.to_string(),
            },
        }
    }

    fn scene(nodes: Vec<Node>, connections: Vec<Connection>) -> SceneDSL {
        SceneDSL {
            version: "1.0.0".to_string(),
            metadata: Metadata {
                name: "test".to_string(),
                created: None,
                modified: None,
            },
            nodes,
            connections,
            outputs: None,
            groups: Vec::new(),
            assets: HashMap::new(),
        }
    }

    fn ids_for(nodes: &[Node]) -> HashMap<String, ResourceName> {
        nodes
            .iter()
            .map(|n| (n.id.clone(), n.id.clone().into()))
            .collect()
    }

    fn approx_eq(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4, "expected {a} ~= {b}");
    }

    fn apply_mat4_to_point(m: [f32; 16], p: [f32; 3]) -> [f32; 3] {
        [
            m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
            m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
            m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
        ]
    }

    fn col_major_to_row_major(m: [f32; 16]) -> [f32; 16] {
        let mut out = [0.0f32; 16];
        for row in 0..4 {
            for col in 0..4 {
                out[row * 4 + col] = m[col * 4 + row];
            }
        }
        out
    }

    #[test]
    fn rect2d_inline_size_and_position_are_used_when_unconnected() {
        let nodes = vec![node(
            "rect",
            "Rect2DGeometry",
            json!({"size": {"x": 108.0, "y": 240.0}, "position": {"x": 54.0, "y": 120.0}}),
        )];
        let scene = scene(nodes.clone(), vec![]);
        let nodes_by_id: HashMap<String, Node> =
            nodes.iter().cloned().map(|n| (n.id.clone(), n)).collect();
        let ids = ids_for(&nodes);

        let (_buf, w, h, x, y, ..) = resolve_geometry_for_render_pass(
            &scene,
            &nodes_by_id,
            &ids,
            "rect",
            [400.0, 400.0],
            None,
            None,
        )
        .unwrap();

        assert_eq!(w, 108.0);
        assert_eq!(h, 240.0);
        assert_eq!(x, 54.0);
        assert_eq!(y, 120.0);
    }

    #[test]
    fn set_transform_preserves_upstream_dynamic_rect_context() {
        let nodes = vec![
            node("size_in", "Vector2Input", json!({"x": 108.0, "y": 240.0})),
            node("pos_in", "Vector2Input", json!({"x": 54.0, "y": 120.0})),
            node("rect", "Rect2DGeometry", json!({})),
            node("set", "SetTransform", json!({})),
        ];
        let scene = scene(
            nodes.clone(),
            vec![
                conn("c1", "size_in", "vector", "rect", "size"),
                conn("c2", "pos_in", "vector", "rect", "position"),
                conn("c3", "rect", "geometry", "set", "geometry"),
            ],
        );
        let nodes_by_id: HashMap<String, Node> =
            nodes.iter().cloned().map(|n| (n.id.clone(), n)).collect();
        let ids = ids_for(&nodes);

        let (
            _buf,
            _w,
            _h,
            _x,
            _y,
            _inst,
            _m,
            _mats,
            _translate,
            _inline,
            _decls,
            graph_inputs,
            _ii,
            rect_dyn,
            normals,
        ) = resolve_geometry_for_render_pass(
            &scene,
            &nodes_by_id,
            &ids,
            "set",
            [400.0, 400.0],
            None,
            None,
        )
        .unwrap();

        assert!(rect_dyn.is_some());
        assert!(graph_inputs.contains_key("size_in"));
        assert!(graph_inputs.contains_key("pos_in"));
        assert!(normals.is_none());
    }

    #[test]
    fn transform_geometry_applies_inline_rotation_to_base_matrix() {
        let nodes = vec![
            node("rect", "Rect2DGeometry", json!({})),
            node(
                "xf",
                "TransformGeometry",
                json!({"mode": "Components", "rotate": {"x": 0.0, "y": 0.0, "z": 90.0}}),
            ),
        ];
        let scene = scene(
            nodes.clone(),
            vec![conn("c1", "rect", "geometry", "xf", "geometry")],
        );
        let nodes_by_id: HashMap<String, Node> =
            nodes.iter().cloned().map(|n| (n.id.clone(), n)).collect();
        let ids = ids_for(&nodes);

        let (_buf, _w, _h, _x, _y, _inst, base_m, _mats, translate, ..) =
            resolve_geometry_for_render_pass(
                &scene,
                &nodes_by_id,
                &ids,
                "xf",
                [400.0, 400.0],
                None,
                None,
            )
            .unwrap();

        assert!(translate.is_none());
        approx_eq(base_m[0], 0.0);
        approx_eq(base_m[1], 1.0);
        approx_eq(base_m[4], -1.0);
        approx_eq(base_m[5], 0.0);
    }

    #[test]
    fn transform_geometry_matrix_and_components_modes_share_matrix_path() {
        let comp_params = json!({
            "mode": "Components",
            "translate": {"x": 10.0, "y": 20.0, "z": 0.0},
            "rotate": {"x": 0.0, "y": 0.0, "z": 30.0},
            "scale": {"x": 2.0, "y": 3.0, "z": 1.0}
        });
        let comp_node = node("xf_comp", "TransformGeometry", comp_params.clone());
        let matrix = col_major_to_row_major(compute_trs_matrix(&comp_node));

        let nodes = vec![
            node("rect", "Rect2DGeometry", json!({})),
            comp_node,
            node(
                "xf_mat",
                "TransformGeometry",
                json!({"mode": "Matrix", "matrix": matrix}),
            ),
        ];
        let scene = scene(
            nodes.clone(),
            vec![
                conn("c1", "rect", "geometry", "xf_comp", "geometry"),
                conn("c2", "rect", "geometry", "xf_mat", "geometry"),
            ],
        );
        let nodes_by_id: HashMap<String, Node> =
            nodes.iter().cloned().map(|n| (n.id.clone(), n)).collect();
        let ids = ids_for(&nodes);

        let (_buf, _w, _h, _x, _y, _inst, comp_m, _mats, comp_translate, ..) =
            resolve_geometry_for_render_pass(
                &scene,
                &nodes_by_id,
                &ids,
                "xf_comp",
                [400.0, 400.0],
                None,
                None,
            )
            .unwrap();
        let (_buf, _w, _h, _x, _y, _inst, mat_m, _mats, mat_translate, ..) =
            resolve_geometry_for_render_pass(
                &scene,
                &nodes_by_id,
                &ids,
                "xf_mat",
                [400.0, 400.0],
                None,
                None,
            )
            .unwrap();

        assert!(comp_translate.is_none());
        assert!(mat_translate.is_none());
        for i in 0..16 {
            approx_eq(comp_m[i], mat_m[i]);
        }
    }

    #[test]
    fn compute_trs_matrix_uses_xyz_rotation_order() {
        let xf = node(
            "xf_xyz",
            "TransformGeometry",
            json!({
                "mode": "Components",
                "translate": {"x": 10.0, "y": 20.0, "z": 30.0},
                "rotate": {"x": 90.0, "y": 90.0, "z": 0.0},
                "scale": {"x": 2.0, "y": 3.0, "z": 4.0}
            }),
        );

        let m = compute_trs_matrix(&xf);
        let out = apply_mat4_to_point(m, [1.0, 0.0, 0.0]);
        approx_eq(out[0], 10.0);
        approx_eq(out[1], 20.0);
        approx_eq(out[2], 28.0);
    }

    #[test]
    fn set_transform_matrix_mode_accepts_connected_perspective_camera() {
        let nodes = vec![
            node("rect", "Rect2DGeometry", json!({})),
            node(
                "cam",
                "PerspectiveCamera",
                json!({
                    "position": {"x": 0.0, "y": 0.0, "z": 10.0},
                    "target": {"x": 0.0, "y": 0.0, "z": 0.0},
                    "up": {"x": 0.0, "y": 1.0, "z": 0.0},
                    "fovY": 60.0,
                    "aspect": 1.0,
                    "near": 0.1,
                    "far": 100.0
                }),
            ),
            node("set", "SetTransform", json!({"mode": "Matrix"})),
        ];

        let scene = scene(
            nodes.clone(),
            vec![
                conn("c1", "rect", "geometry", "set", "geometry"),
                conn("c2", "cam", "camera", "set", "matrix"),
            ],
        );
        let nodes_by_id: HashMap<String, Node> =
            nodes.iter().cloned().map(|n| (n.id.clone(), n)).collect();
        let ids = ids_for(&nodes);

        let (_buf, _w, _h, _x, _y, _inst, base_m, _mats, _translate, ..) =
            resolve_geometry_for_render_pass(
                &scene,
                &nodes_by_id,
                &ids,
                "set",
                [400.0, 400.0],
                None,
                None,
            )
            .unwrap();

        assert_ne!(
            base_m,
            [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0
            ]
        );
    }

    #[test]
    fn set_transform_components_can_emit_time_driven_runtime_delta_expr() {
        let nodes = vec![
            node("rect", "Rect2DGeometry", json!({})),
            node("set", "SetTransform", json!({"mode": "Components"})),
            node("v3", "Vector3Input", json!({"x": 0.0, "y": 0.0, "z": 0.0})),
            node("time", "TimeInput", json!({})),
        ];
        let scene = scene(
            nodes.clone(),
            vec![
                conn("c1", "rect", "geometry", "set", "geometry"),
                conn("c2", "v3", "vector", "set", "rotate"),
                conn("c3", "time", "time", "v3", "y"),
            ],
        );
        let nodes_by_id: HashMap<String, Node> =
            nodes.iter().cloned().map(|n| (n.id.clone(), n)).collect();
        let ids = ids_for(&nodes);

        let (
            _buf,
            _w,
            _h,
            _x,
            _y,
            _inst,
            base_m,
            _mats,
            translate,
            _inline,
            decls,
            graph_inputs,
            _uses_ii,
            _rect_dyn,
            _normals,
        ) = resolve_geometry_for_render_pass(
            &scene,
            &nodes_by_id,
            &ids,
            "set",
            [400.0, 400.0],
            None,
            None,
        )
        .unwrap();

        assert_eq!(
            base_m,
            [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0
            ]
        );
        let translate = translate.expect("expected runtime translate expression");
        assert!(translate.expr.contains("params.time"));
        assert!(decls.contains("fn sys_apply_trs_xyz("));
        assert!(graph_inputs.contains_key("v3"));
    }

    #[test]
    fn transform_geometry_components_can_emit_time_driven_runtime_delta_expr() {
        let nodes = vec![
            node("rect", "Rect2DGeometry", json!({})),
            node("xf", "TransformGeometry", json!({"mode": "Components"})),
            node("v3", "Vector3Input", json!({"x": 0.0, "y": 0.0, "z": 0.0})),
            node("time", "TimeInput", json!({})),
        ];
        let scene = scene(
            nodes.clone(),
            vec![
                conn("c1", "rect", "geometry", "xf", "geometry"),
                conn("c2", "v3", "vector", "xf", "rotate"),
                conn("c3", "time", "time", "v3", "y"),
            ],
        );
        let nodes_by_id: HashMap<String, Node> =
            nodes.iter().cloned().map(|n| (n.id.clone(), n)).collect();
        let ids = ids_for(&nodes);

        let (
            _buf,
            _w,
            _h,
            _x,
            _y,
            _inst,
            _base_m,
            _mats,
            translate,
            _inline,
            decls,
            graph_inputs,
            _uses_ii,
            _rect_dyn,
            _normals,
        ) = resolve_geometry_for_render_pass(
            &scene,
            &nodes_by_id,
            &ids,
            "xf",
            [400.0, 400.0],
            None,
            None,
        )
        .unwrap();

        let translate = translate.expect("expected runtime translate expression");
        assert!(translate.expr.contains("params.time"));
        assert!(decls.contains("fn sys_apply_trs_xyz("));
        assert!(graph_inputs.contains_key("v3"));
    }
}
