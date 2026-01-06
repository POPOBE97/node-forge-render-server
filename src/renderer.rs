use std::{borrow::Cow, collections::HashMap, sync::Arc};

use anyhow::{anyhow, bail, Result};
use rust_wgpu_fiber::{
    eframe::wgpu::{
        self, vertex_attr_array, BlendState, Color, ShaderStages, TextureFormat, TextureUsages,
    },
    pool::{
        buffer_pool::BufferSpec,
        texture_pool::TextureSpec as FiberTextureSpec,
    },
    shader_space::{ShaderSpace, ShaderSpaceResult},
    ResourceName,
};

use crate::{
    dsl::{
        find_node, incoming_connection, parse_f32, parse_texture_format, parse_u32, Node, SceneDSL,
    },
    graph::{topo_sort, upstream_reachable},
    schema,
};

#[derive(Clone, Debug)]
pub struct PassBindings {
    pub params_buffer: ResourceName,
    pub base_params: Params,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub target_size: [f32; 2],
    pub geo_size: [f32; 2],
    pub center: [f32; 2],
    pub time: f32,
    pub _pad0: f32,
    pub color: [f32; 4],
}

fn as_bytes<T>(v: &T) -> &[u8] {
    unsafe { core::slice::from_raw_parts((v as *const T) as *const u8, core::mem::size_of::<T>()) }
}

fn as_bytes_slice<T>(v: &[T]) -> &[u8] {
    unsafe {
        core::slice::from_raw_parts(v.as_ptr() as *const u8, core::mem::size_of::<T>() * v.len())
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueType {
    F32,
    Vec2,
    Vec3,
    Vec4,
}

impl ValueType {
    fn wgsl(self) -> &'static str {
        match self {
            ValueType::F32 => "f32",
            ValueType::Vec2 => "vec2f",
            ValueType::Vec3 => "vec3f",
            ValueType::Vec4 => "vec4f",
        }
    }
}

#[derive(Clone, Debug)]
struct TypedExpr {
    ty: ValueType,
    expr: String,
    uses_time: bool,
}

fn typed(expr: impl Into<String>, ty: ValueType) -> TypedExpr {
    TypedExpr {
        ty,
        expr: expr.into(),
        uses_time: false,
    }
}

fn typed_time(expr: impl Into<String>, ty: ValueType, uses_time: bool) -> TypedExpr {
    TypedExpr {
        ty,
        expr: expr.into(),
        uses_time,
    }
}

fn splat_f32(x: &TypedExpr, target: ValueType) -> Result<TypedExpr> {
    if x.ty != ValueType::F32 {
        bail!("expected f32 for splat, got {:?}", x.ty);
    }
    Ok(match target {
        ValueType::F32 => x.clone(),
        ValueType::Vec2 => typed_time(format!("vec2f({})", x.expr), ValueType::Vec2, x.uses_time),
        ValueType::Vec3 => typed_time(format!("vec3f({})", x.expr), ValueType::Vec3, x.uses_time),
        ValueType::Vec4 => typed_time(
            format!("vec4f({}, {}, {}, 1.0)", x.expr, x.expr, x.expr),
            ValueType::Vec4,
            x.uses_time,
        ),
    })
}

fn coerce_for_binary(a: TypedExpr, b: TypedExpr) -> Result<(TypedExpr, TypedExpr, ValueType)> {
    if a.ty == b.ty {
        let ty = a.ty;
        return Ok((a, b, ty));
    }
    // Allow scalar splat to vector.
    if a.ty == ValueType::F32 {
        let target_ty = b.ty;
        let aa = splat_f32(&a, target_ty)?;
        return Ok((aa, b, target_ty));
    }
    if b.ty == ValueType::F32 {
        let target_ty = a.ty;
        let bb = splat_f32(&b, target_ty)?;
        return Ok((a, bb, target_ty));
    }
    bail!("type mismatch: {:?} vs {:?}", a.ty, b.ty)
}

fn to_vec4_color(x: TypedExpr) -> TypedExpr {
    match x.ty {
        ValueType::Vec4 => x,
        ValueType::Vec3 => typed_time(format!("vec4f({}, 1.0)", x.expr), ValueType::Vec4, x.uses_time),
        ValueType::Vec2 => typed_time(format!("vec4f({}, 0.0, 1.0)", x.expr), ValueType::Vec4, x.uses_time),
        ValueType::F32 => typed_time(
            format!("vec4f({0}, {0}, {0}, 1.0)", x.expr),
            ValueType::Vec4,
            x.uses_time,
        ),
    }
}

fn parse_const_f32(node: &Node) -> Option<f32> {
    // A few common param keys.
    parse_f32(&node.params, "value")
        .or_else(|| parse_f32(&node.params, "x"))
        .or_else(|| parse_f32(&node.params, "v"))
}

fn parse_const_vec(node: &Node, keys: [&str; 4]) -> Option<[f32; 4]> {
    let x = parse_f32(&node.params, keys[0])?;
    let y = parse_f32(&node.params, keys[1]).unwrap_or(0.0);
    let z = parse_f32(&node.params, keys[2]).unwrap_or(0.0);
    let w = parse_f32(&node.params, keys[3]).unwrap_or(1.0);
    Some([x, y, z, w])
}

fn compile_material_expr(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    out_port: Option<&str>,
    cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    let key = (
        node_id.to_string(),
        out_port.unwrap_or("value").to_string(),
    );
    if let Some(v) = cache.get(&key) {
        return Ok(v.clone());
    }

    let node = find_node(nodes_by_id, node_id)?;
    let result = match node.node_type.as_str() {
        "Attribute" => {
            let name = node
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("uv")
                .to_ascii_lowercase();
            match name.as_str() {
                "uv" => typed("in.uv".to_string(), ValueType::Vec2),
                other => bail!("unsupported Attribute.name: {other}"),
            }
        }

        "Time" => typed_time("params.time".to_string(), ValueType::F32, true),

        "Float" | "Scalar" | "Constant" => {
            let v = parse_const_f32(node).unwrap_or(0.0);
            typed(format!("{v}"), ValueType::F32)
        }

        "Vec2" => {
            let v = parse_const_vec(node, ["x", "y", "z", "w"]).unwrap_or([0.0, 0.0, 0.0, 0.0]);
            typed(format!("vec2f({}, {})", v[0], v[1]), ValueType::Vec2)
        }
        "Vec3" => {
            let v = parse_const_vec(node, ["x", "y", "z", "w"]).unwrap_or([0.0, 0.0, 0.0, 0.0]);
            typed(format!("vec3f({}, {}, {})", v[0], v[1], v[2]), ValueType::Vec3)
        }
        "Vec4" | "Color" => {
            // Accept either x/y/z/w or r/g/b/a.
            let v = parse_const_vec(node, ["x", "y", "z", "w"]).or_else(|| parse_const_vec(node, ["r", "g", "b", "a"]))
                .unwrap_or([1.0, 0.0, 1.0, 1.0]);
            typed(format!("vec4f({}, {}, {}, {})", v[0], v[1], v[2], v[3]), ValueType::Vec4)
        }

        "Sin" => {
            let input = incoming_connection(scene, node_id, "x")
                .or_else(|| incoming_connection(scene, node_id, "value"))
                .or_else(|| incoming_connection(scene, node_id, "in"))
                .ok_or_else(|| anyhow!("Sin missing input"))?;
            let x = compile_material_expr(scene, nodes_by_id, &input.from.node_id, Some(&input.from.port_id), cache)?;
            typed_time(format!("sin({})", x.expr), x.ty, x.uses_time)
        }

        "Cos" => {
            let input = incoming_connection(scene, node_id, "x")
                .or_else(|| incoming_connection(scene, node_id, "value"))
                .or_else(|| incoming_connection(scene, node_id, "in"))
                .ok_or_else(|| anyhow!("Cos missing input"))?;
            let x = compile_material_expr(scene, nodes_by_id, &input.from.node_id, Some(&input.from.port_id), cache)?;
            typed_time(format!("cos({})", x.expr), x.ty, x.uses_time)
        }

        "Add" => {
            let a_conn = incoming_connection(scene, node_id, "a")
                .or_else(|| incoming_connection(scene, node_id, "x"))
                .ok_or_else(|| anyhow!("Add missing input a"))?;
            let b_conn = incoming_connection(scene, node_id, "b")
                .or_else(|| incoming_connection(scene, node_id, "y"))
                .ok_or_else(|| anyhow!("Add missing input b"))?;
            let a = compile_material_expr(scene, nodes_by_id, &a_conn.from.node_id, Some(&a_conn.from.port_id), cache)?;
            let b = compile_material_expr(scene, nodes_by_id, &b_conn.from.node_id, Some(&b_conn.from.port_id), cache)?;
            let (aa, bb, ty) = coerce_for_binary(a, b)?;
            typed_time(
                format!("({} + {})", aa.expr, bb.expr),
                ty,
                aa.uses_time || bb.uses_time,
            )
        }

        "Mul" | "Multiply" => {
            let a_conn = incoming_connection(scene, node_id, "a")
                .or_else(|| incoming_connection(scene, node_id, "x"))
                .ok_or_else(|| anyhow!("Mul missing input a"))?;
            let b_conn = incoming_connection(scene, node_id, "b")
                .or_else(|| incoming_connection(scene, node_id, "y"))
                .ok_or_else(|| anyhow!("Mul missing input b"))?;
            let a = compile_material_expr(scene, nodes_by_id, &a_conn.from.node_id, Some(&a_conn.from.port_id), cache)?;
            let b = compile_material_expr(scene, nodes_by_id, &b_conn.from.node_id, Some(&b_conn.from.port_id), cache)?;
            let (aa, bb, ty) = coerce_for_binary(a, b)?;
            typed_time(
                format!("({} * {})", aa.expr, bb.expr),
                ty,
                aa.uses_time || bb.uses_time,
            )
        }

        "Mix" => {
            let a_conn = incoming_connection(scene, node_id, "a")
                .or_else(|| incoming_connection(scene, node_id, "x"))
                .ok_or_else(|| anyhow!("Mix missing input a"))?;
            let b_conn = incoming_connection(scene, node_id, "b")
                .or_else(|| incoming_connection(scene, node_id, "y"))
                .ok_or_else(|| anyhow!("Mix missing input b"))?;
            let t_conn = incoming_connection(scene, node_id, "t")
                .or_else(|| incoming_connection(scene, node_id, "alpha"))
                .or_else(|| incoming_connection(scene, node_id, "factor"))
                .ok_or_else(|| anyhow!("Mix missing input t"))?;

            let a = compile_material_expr(scene, nodes_by_id, &a_conn.from.node_id, Some(&a_conn.from.port_id), cache)?;
            let b = compile_material_expr(scene, nodes_by_id, &b_conn.from.node_id, Some(&b_conn.from.port_id), cache)?;
            let t = compile_material_expr(scene, nodes_by_id, &t_conn.from.node_id, Some(&t_conn.from.port_id), cache)?;
            if t.ty != ValueType::F32 {
                bail!("Mix.t must be f32, got {:?}", t.ty);
            }
            let (aa, bb, ty) = coerce_for_binary(a, b)?;
            let tt = if ty == ValueType::F32 {
                t
            } else {
                // WGSL allows vecNf(f32) splat constructors.
                typed_time(format!("{}({})", ty.wgsl(), t.expr), ty, t.uses_time)
            };
            typed_time(
                format!("mix({}, {}, {})", aa.expr, bb.expr, tt.expr),
                ty,
                aa.uses_time || bb.uses_time || tt.uses_time,
            )
        }

        "Clamp" => {
            let x_conn = incoming_connection(scene, node_id, "x")
                .or_else(|| incoming_connection(scene, node_id, "value"))
                .ok_or_else(|| anyhow!("Clamp missing input x"))?;
            let min_conn = incoming_connection(scene, node_id, "min")
                .or_else(|| incoming_connection(scene, node_id, "lo"))
                .ok_or_else(|| anyhow!("Clamp missing input min"))?;
            let max_conn = incoming_connection(scene, node_id, "max")
                .or_else(|| incoming_connection(scene, node_id, "hi"))
                .ok_or_else(|| anyhow!("Clamp missing input max"))?;
            let x = compile_material_expr(scene, nodes_by_id, &x_conn.from.node_id, Some(&x_conn.from.port_id), cache)?;
            let minv = compile_material_expr(scene, nodes_by_id, &min_conn.from.node_id, Some(&min_conn.from.port_id), cache)?;
            let maxv = compile_material_expr(scene, nodes_by_id, &max_conn.from.node_id, Some(&max_conn.from.port_id), cache)?;
            let (xx, mn, ty) = coerce_for_binary(x, minv)?;
            let (xx2, mx, _) = coerce_for_binary(xx, maxv)?;
            typed_time(
                format!("clamp({}, {}, {})", xx2.expr, mn.expr, mx.expr),
                ty,
                xx2.uses_time || mn.uses_time || mx.uses_time,
            )
        }

        "Smoothstep" => {
            let e0_conn = incoming_connection(scene, node_id, "edge0")
                .or_else(|| incoming_connection(scene, node_id, "min"))
                .ok_or_else(|| anyhow!("Smoothstep missing input edge0"))?;
            let e1_conn = incoming_connection(scene, node_id, "edge1")
                .or_else(|| incoming_connection(scene, node_id, "max"))
                .ok_or_else(|| anyhow!("Smoothstep missing input edge1"))?;
            let x_conn = incoming_connection(scene, node_id, "x")
                .or_else(|| incoming_connection(scene, node_id, "value"))
                .ok_or_else(|| anyhow!("Smoothstep missing input x"))?;
            let e0 = compile_material_expr(scene, nodes_by_id, &e0_conn.from.node_id, Some(&e0_conn.from.port_id), cache)?;
            let e1 = compile_material_expr(scene, nodes_by_id, &e1_conn.from.node_id, Some(&e1_conn.from.port_id), cache)?;
            let x = compile_material_expr(scene, nodes_by_id, &x_conn.from.node_id, Some(&x_conn.from.port_id), cache)?;
            let (e0c, e1c, ty01) = coerce_for_binary(e0, e1)?;
            let (xc, _, ty) = coerce_for_binary(x, e0c.clone())?;
            if ty != ty01 {
                bail!("Smoothstep type mismatch: {:?} vs {:?}", ty01, ty);
            }
            typed_time(
                format!("smoothstep({}, {}, {})", e0c.expr, e1c.expr, xc.expr),
                ty,
                e0c.uses_time || e1c.uses_time || xc.uses_time,
            )
        }

        other => bail!("unsupported material node type: {other}"),
    };

    cache.insert(key, result.clone());
    Ok(result)
}

fn build_pass_wgsl(scene: &SceneDSL, nodes_by_id: &HashMap<String, Node>, pass_id: &str) -> Result<String> {
    Ok(build_pass_wgsl_bundle(scene, nodes_by_id, pass_id)?.module)
}

#[derive(Clone, Debug)]
pub struct WgslShaderBundle {
    /// WGSL declarations shared between stages (types, bindings, structs).
    pub common: String,
    /// A standalone vertex WGSL module (common + @vertex entry).
    pub vertex: String,
    /// A standalone fragment WGSL module (common + @fragment entry).
    pub fragment: String,
    /// Optional compute WGSL module (common + @compute entry). Currently unused.
    pub compute: Option<String>,
    /// A combined WGSL module containing all emitted entry points.
    pub module: String,
}

pub fn build_pass_wgsl_bundle(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    pass_id: &str,
) -> Result<WgslShaderBundle> {
    // If RenderPass.material is connected, compile the upstream subgraph into an expression.
    // Otherwise, fallback to constant color.
    let fragment_expr: TypedExpr = if let Some(conn) = incoming_connection(scene, pass_id, "material") {
        let mut cache: HashMap<(String, String), TypedExpr> = HashMap::new();
        compile_material_expr(
            scene,
            nodes_by_id,
            &conn.from.node_id,
            Some(&conn.from.port_id),
            &mut cache,
        )?
    } else {
        typed("params.color".to_string(), ValueType::Vec4)
    };

    let out_color = to_vec4_color(fragment_expr);
    let fragment_body = format!("return {};", out_color.expr);

    let common = r#"
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,
    time: f32,
    _pad0: f32,
    color: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
};
"#
    .to_string();

    let vertex_entry = r#"
@vertex
fn vs_main(@location(0) position: vec3f) -> VSOut {
    var out: VSOut;

    // Local UV in [0,1] based on geometry size.
    out.uv = (position.xy / params.geo_size) + vec2f(0.5, 0.5);

    // Geometry vertices are in local pixel units centered at (0,0). Apply center translation in pixels.
    let p = position.xy + params.center;

    // Convert pixels to clip space (assumes target_size is in pixels and (0,0) is the target center).
    let half = params.target_size * 0.5;
    let ndc = vec2f(p.x / half.x, p.y / half.y);
    out.position = vec4f(ndc, position.z, 1.0);
    return out;
}
"#
    .to_string();

    let fragment_entry = format!(
        r#"
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {{
    {fragment_body}
}}
"#
    );

    let vertex = format!("{common}{vertex_entry}");
    let fragment = format!("{common}{fragment_entry}");
    let compute = None;
    let module = format!("{common}{vertex_entry}{fragment_entry}");

    Ok(WgslShaderBundle {
        common,
        vertex,
        fragment,
        compute,
        module,
    })
}

pub fn build_all_pass_wgsl_bundles_from_scene(
    scene: &SceneDSL,
) -> Result<Vec<(String, WgslShaderBundle)>> {
    schema::validate_scene(scene)?;
    let nodes_by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|n| (n.id.clone(), n))
        .collect();

    let order = topo_sort(scene)?;

    // Match build_shader_space_from_scene: prefer outputs.composite, otherwise fallback to the first CompositeOutput.
    let output_node_id: String = scene
        .outputs
        .as_ref()
        .and_then(|m| m.get("composite").cloned())
        .or_else(|| {
            scene
                .nodes
                .iter()
                .find(|n| n.node_type == "CompositeOutput")
                .map(|n| n.id.clone())
        })
        .ok_or_else(|| anyhow!("no outputs.composite and no CompositeOutput node"))?;

    let reachable = upstream_reachable(scene, &output_node_id);

    let mut out: Vec<(String, WgslShaderBundle)> = Vec::new();
    for node_id in order {
        if !reachable.contains(&node_id) {
            continue;
        }
        let node = match nodes_by_id.get(&node_id) {
            Some(n) => n,
            None => continue,
        };
        if node.node_type != "RenderPass" {
            continue;
        }
        let bundle = build_pass_wgsl_bundle(scene, &nodes_by_id, &node_id)?;
        out.push((node_id, bundle));
    }
    Ok(out)
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
}

pub fn build_shader_space_from_scene(
    scene: &SceneDSL,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
) -> Result<(ShaderSpace, [u32; 2], ResourceName, Vec<PassBindings>)> {
    schema::validate_scene(scene)?;
    let nodes_by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|n| (n.id.clone(), n))
        .collect();

    let mut ids: HashMap<String, ResourceName> = HashMap::new();
    for n in &scene.nodes {
        ids.insert(n.id.clone(), n.id.clone().into());
    }

    let order = topo_sort(scene)?;

    let output_node_id: String = scene
        .outputs
        .as_ref()
        .and_then(|m| m.get("composite").cloned())
        .or_else(|| {
            scene
                .nodes
                .iter()
                .find(|n| n.node_type == "CompositeOutput")
                .map(|n| n.id.clone())
        })
        .ok_or_else(|| anyhow!("no outputs.composite and no CompositeOutput node"))?;

    let render_pass_id: String = incoming_connection(scene, &output_node_id, "image")
        .map(|c| c.from.node_id.clone())
        .ok_or_else(|| anyhow!("CompositeOutput.image has no incoming connection"))?;

    let reachable = upstream_reachable(scene, &output_node_id);
    let render_passes_in_order: Vec<String> = order
        .iter()
        .filter(|id| reachable.contains(*id))
        .filter(|id| {
            nodes_by_id
                .get(*id)
                .is_some_and(|n| n.node_type == "RenderPass")
        })
        .cloned()
        .collect();
    if render_passes_in_order.is_empty() {
        bail!("no RenderPass reachable from CompositeOutput");
    }

    // New DSL: render target is provided by CompositeOutput.target.
    // For backward compatibility with older scenes, fall back to RenderPass.target if present.
    let last_pass_id: String = render_passes_in_order
        .last()
        .cloned()
        .unwrap_or_else(|| render_pass_id.clone());
    let output_texture_node_id: String = incoming_connection(scene, &output_node_id, "target")
        .or_else(|| incoming_connection(scene, &last_pass_id, "target"))
        .map(|c| c.from.node_id.clone())
        .ok_or_else(|| {
            anyhow!("CompositeOutput.target (or legacy RenderPass.target) has no incoming connection")
        })?;

    let output_texture_name: ResourceName = ids
        .get(&output_texture_node_id)
        .cloned()
        .ok_or_else(|| anyhow!("missing name for node: {}", output_texture_node_id))?;

    let output_texture_node = find_node(&nodes_by_id, &output_texture_node_id)?;
    if output_texture_node.node_type != "RenderTexture" {
        bail!(
            "RenderPass.target must come from RenderTexture, got {}",
            output_texture_node.node_type
        );
    }

    let width = parse_u32(&output_texture_node.params, "width").unwrap_or(1024);
    let height = parse_u32(&output_texture_node.params, "height").unwrap_or(1024);
    let resolution = [width, height];

    let mut geometry_buffers: Vec<(ResourceName, Arc<[u8]>)> = Vec::new();
    let mut textures: Vec<TextureDecl> = Vec::new();
    let mut render_pass_specs: Vec<RenderPassSpec> = Vec::new();
    let mut composite_passes: Vec<ResourceName> = Vec::new();

    for id in &order {
        if !reachable.contains(id) {
            continue;
        }
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
                let w = parse_u32(&node.params, "width").unwrap_or(width);
                let h = parse_u32(&node.params, "height").unwrap_or(height);
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

    for pass_id in &render_passes_in_order {
        let pass_name = ids
            .get(pass_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing name for node: {pass_id}"))?;

        let geometry_node_id = incoming_connection(scene, pass_id, "geometry")
            .map(|c| c.from.node_id.clone())
            .ok_or_else(|| anyhow!("RenderPass.geometry missing for {pass_id}"))?;
        // RenderPass no longer has an explicit target input in the updated DSL.
        // Use the CompositeOutput-provided target texture (or legacy per-pass target if present).
        let target_texture_id = incoming_connection(scene, pass_id, "target")
            .map(|c| c.from.node_id.clone())
            .unwrap_or_else(|| output_texture_node_id.clone());

        let geometry_node = find_node(&nodes_by_id, &geometry_node_id)?;
        if geometry_node.node_type != "Rect2DGeometry" {
            bail!(
                "RenderPass.geometry must come from Rect2DGeometry, got {}",
                geometry_node.node_type
            );
        }
        let target_node = find_node(&nodes_by_id, &target_texture_id)?;
        if target_node.node_type != "RenderTexture" {
            bail!(
                "RenderPass.target must come from RenderTexture, got {}",
                target_node.node_type
            );
        }

        let geometry_buffer = ids
            .get(&geometry_node_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing name for node: {}", geometry_node_id))?;
        let target_texture = ids
            .get(&target_texture_id)
            .cloned()
            .ok_or_else(|| anyhow!("missing name for node: {}", target_texture_id))?;

        let geo_w = parse_f32(&geometry_node.params, "width").unwrap_or(100.0);
        let geo_h = parse_f32(&geometry_node.params, "height").unwrap_or(geo_w);

        let geo_x = parse_f32(&geometry_node.params, "x").unwrap_or(0.0);
        let geo_y = parse_f32(&geometry_node.params, "y").unwrap_or(0.0);

        let tgt_w = parse_f32(&target_node.params, "width")
            .unwrap_or(width as f32)
            .max(1.0);
        let tgt_h = parse_f32(&target_node.params, "height")
            .unwrap_or(height as f32)
            .max(1.0);

        let params_name: ResourceName = format!("params_{pass_id}").into();
        let params = Params {
            target_size: [tgt_w, tgt_h],
            geo_size: [geo_w.max(1.0), geo_h.max(1.0)],
            center: [geo_x, geo_y],
            time: 0.0,
            _pad0: 0.0,
            color: [0.9, 0.2, 0.2, 1.0],
        };

        let shader_wgsl = build_pass_wgsl(scene, &nodes_by_id, pass_id)?;

        render_pass_specs.push(RenderPassSpec {
            name: pass_name.clone(),
            geometry_buffer,
            target_texture,
            params_buffer: params_name,
            params,
            shader_wgsl,
        });
        composite_passes.push(pass_name);
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
    let texture_specs: Vec<FiberTextureSpec> = textures
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
    shader_space.declare_textures(texture_specs);

    for spec in &render_pass_specs {
        let geometry_buffer = spec.geometry_buffer.clone();
        let target_texture = spec.target_texture.clone();
        let params_buffer = spec.params_buffer.clone();
        let shader_wgsl = spec.shader_wgsl.clone();
        let shader_desc: wgpu::ShaderModuleDescriptor<'static> = wgpu::ShaderModuleDescriptor {
            label: Some("node-forge-pass"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(shader_wgsl)),
        };
        shader_space.render_pass(spec.name.clone(), move |builder| {
            builder
                .shader(shader_desc)
                .bind_uniform_buffer(0, 0, params_buffer, ShaderStages::VERTEX_FRAGMENT)
                .bind_attribute_buffer(
                    0,
                    geometry_buffer,
                    wgpu::VertexStepMode::Vertex,
                    vertex_attr_array![0 => Float32x3].to_vec(),
                )
                .bind_color_attachment(target_texture)
                .blending(BlendState::REPLACE)
                .load_op(wgpu::LoadOp::Clear(Color::TRANSPARENT))
        });
    }

    shader_space.composite(move |composer| {
        let mut c = composer;
        for pass in &composite_passes {
            c = c.pass(pass.clone());
        }
        c
    });

    shader_space.prepare();

    for spec in &render_pass_specs {
        shader_space.write_buffer(spec.params_buffer.as_str(), 0, as_bytes(&spec.params))?;
    }

    Ok((shader_space, resolution, output_texture_name, pass_bindings))
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
