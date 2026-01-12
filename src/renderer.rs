use std::{borrow::Cow, collections::{HashMap, HashSet}, path::PathBuf, sync::Arc};

use anyhow::{anyhow, bail, Result};
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
        types::{ValueType, TypedExpr, MaterialCompileContext},
        utils::{sanitize_wgsl_ident, to_vec4_color, coerce_for_binary},
    },
};

fn port_type_contains(t: &schema::PortTypeSpec, candidate: &str) -> bool {
    match t {
        schema::PortTypeSpec::One(s) => s == candidate,
        schema::PortTypeSpec::Many(v) => v.iter().any(|s| s == candidate),
    }
}

fn port_type_contains_any_of(t: &schema::PortTypeSpec, candidates: &[&str]) -> bool {
    candidates.iter().any(|c| port_type_contains(t, c))
}

fn get_from_port_type(
    scheme: &schema::NodeScheme,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    port_id: &str,
) -> Option<schema::PortTypeSpec> {
    let node = nodes_by_id.get(node_id)?;
    let ty = scheme.nodes.get(&node.node_type)?.outputs.get(port_id)?;
    Some(ty.clone())
}

fn get_to_port_type(
    scheme: &schema::NodeScheme,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    port_id: &str,
) -> Option<schema::PortTypeSpec> {
    let node = nodes_by_id.get(node_id)?;
    let node_scheme = scheme.nodes.get(&node.node_type)?;

    if let Some(t) = node_scheme.inputs.get(port_id) {
        return Some(t.clone());
    }

    // Composite supports dynamic layer inputs (dynamic_*) that behave like its base pass input.
    if node.node_type == "Composite" && port_id.starts_with("dynamic_") {
        if let Some(pass_ty) = node_scheme.inputs.get("pass") {
            return Some(pass_ty.clone());
        }
        return Some(schema::PortTypeSpec::One("pass".to_string()));
    }

    None
}

/// If a `pass`-typed input is driven by a primitive shader value (color/vec*/float/int/bool),
/// synthesize a default fullscreen RenderPass (and geometry) and rewire the connection.
fn auto_wrap_primitive_pass_inputs(scene: &mut SceneDSL, scheme: &schema::NodeScheme) {
    let nodes_by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|n| (n.id.clone(), n))
        .collect();

    // Best-effort: infer output target size from outputs.composite -> Composite.target -> RenderTexture.
    let mut target_size: Option<[f32; 2]> = None;
    if let Some(outputs) = scene.outputs.as_ref() {
        if let Some(composite_id) = outputs.get("composite") {
            if let Some(conn) = incoming_connection(scene, composite_id, "target") {
                if let Some(tgt_node) = nodes_by_id.get(&conn.from.node_id) {
                    if tgt_node.node_type == "RenderTexture" {
                        let w = parse_f32(&tgt_node.params, "width")
                            .or_else(|| parse_u32(&tgt_node.params, "width").map(|x| x as f32))
                            .unwrap_or(1024.0)
                            .max(1.0);
                        let h = parse_f32(&tgt_node.params, "height")
                            .or_else(|| parse_u32(&tgt_node.params, "height").map(|x| x as f32))
                            .unwrap_or(1024.0)
                            .max(1.0);
                        target_size = Some([w, h]);
                    }
                }
            }
        }
    }
    let [tgt_w, tgt_h] = target_size.unwrap_or([1024.0, 1024.0]);

    let primitive_candidates: [&str; 10] = [
        "color",
        "vector2",
        "vector3",
        "vector4",
        "float",
        "int",
        "bool",
        // Common aliases used by some editors/schemes.
        "vec2",
        "vec3",
        "vec4",
    ];

    #[derive(Clone)]
    struct WrapPlan {
        conn_index: usize,
        conn_id: String,
        original_from: Endpoint,
        pass_id: String,
        geo_id: String,
    }

    // Plan first (no mutation of vectors while iterating).
    let mut plans: Vec<WrapPlan> = Vec::new();
    for (idx, c) in scene.connections.iter().enumerate() {
        let Some(to_ty) = get_to_port_type(scheme, &nodes_by_id, &c.to.node_id, &c.to.port_id)
        else {
            continue;
        };
        if !port_type_contains(&to_ty, "pass") {
            continue;
        }

        let Some(from_ty) =
            get_from_port_type(scheme, &nodes_by_id, &c.from.node_id, &c.from.port_id)
        else {
            continue;
        };

        if port_type_contains(&from_ty, "pass") {
            continue;
        }
        if !port_type_contains_any_of(&from_ty, &primitive_candidates) {
            continue;
        }

        plans.push(WrapPlan {
            conn_index: idx,
            conn_id: c.id.clone(),
            original_from: c.from.clone(),
            pass_id: format!("__auto_fullscreen_pass__{}", c.id),
            geo_id: format!("__auto_fullscreen_geo__{}", c.id),
        });
    }

    // Apply plans.
    let mut new_connections: Vec<Connection> = Vec::new();
    for p in &plans {
        let mut geo_params = HashMap::new();
        geo_params.insert("width".to_string(), serde_json::json!(tgt_w));
        geo_params.insert("height".to_string(), serde_json::json!(tgt_h));
        geo_params.insert("x".to_string(), serde_json::json!(0.0));
        geo_params.insert("y".to_string(), serde_json::json!(0.0));

        scene.nodes.push(Node {
            id: p.geo_id.clone(),
            node_type: "Rect2DGeometry".to_string(),
            params: geo_params,
            inputs: Vec::new(),
        });
        scene.nodes.push(Node {
            id: p.pass_id.clone(),
            node_type: "RenderPass".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
        });

        new_connections.push(Connection {
            id: format!("__auto_edge_geo__{}", p.conn_id),
            from: Endpoint {
                node_id: p.geo_id.clone(),
                port_id: "geometry".to_string(),
            },
            to: Endpoint {
                node_id: p.pass_id.clone(),
                port_id: "geometry".to_string(),
            },
        });
        new_connections.push(Connection {
            id: format!("__auto_edge_mat__{}", p.conn_id),
            from: p.original_from.clone(),
            to: Endpoint {
                node_id: p.pass_id.clone(),
                port_id: "material".to_string(),
            },
        });

        if let Some(c) = scene.connections.get_mut(p.conn_index) {
            c.from.node_id = p.pass_id.clone();
            c.from.port_id = "pass".to_string();
        }
    }
    scene.connections.extend(new_connections);
}

struct PreparedScene {
    scene: SceneDSL,
    nodes_by_id: HashMap<String, Node>,
    ids: HashMap<String, ResourceName>,
    topo_order: Vec<String>,
    composite_layers_in_draw_order: Vec<String>,
    output_texture_node_id: String,
    output_texture_name: ResourceName,
    resolution: [u32; 2],
}

fn prepare_scene(input: &SceneDSL) -> Result<PreparedScene> {
    // 1) Locate the RenderTarget-category node. Without it, the graph has no "main" entry.
    let scheme = schema::load_default_scheme()?;
    let render_targets: Vec<&Node> = input
        .nodes
        .iter()
        .filter(|n| {
            scheme
                .nodes
                .get(&n.node_type)
                .and_then(|s| s.category.as_deref())
                == Some("RenderTarget")
        })
        .collect();

    if render_targets.is_empty() {
        bail!("missing RenderTarget category node (e.g. Screen/File)");
    }
    if render_targets.len() != 1 {
        let ids: Vec<String> = render_targets
            .iter()
            .map(|n| format!("{} ({})", n.id, n.node_type))
            .collect();
        bail!(
            "expected exactly 1 RenderTarget node, got {}: {}",
            render_targets.len(),
            ids.join(", ")
        );
    }

    let render_target_id = render_targets[0].id.clone();

    // 2) Keep only the upstream subgraph that contributes to the RenderTarget.
    // This avoids validation/compile failures caused by unrelated leftover subgraphs.
    let keep = upstream_reachable(input, &render_target_id);

    let nodes: Vec<Node> = input
        .nodes
        .iter()
        .cloned()
        .filter(|n| keep.contains(&n.id))
        .collect();
    let connections = input
        .connections
        .iter()
        .cloned()
        .filter(|c| keep.contains(&c.from.node_id) && keep.contains(&c.to.node_id))
        .collect();
    let scene = SceneDSL {
        version: input.version.clone(),
        metadata: input.metadata.clone(),
        nodes,
        connections,
        outputs: input.outputs.clone(),
    };

    // Coerce primitive shader values into passes by synthesizing a fullscreen RenderPass.
    let mut scene = scene;
    auto_wrap_primitive_pass_inputs(&mut scene, &scheme);

    // 3) The RenderTarget must be driven by Composite.pass.
    let output_node_id: String = incoming_connection(&scene, &render_target_id, "pass")
        .map(|c| c.from.node_id.clone())
        .ok_or_else(|| anyhow!("RenderTarget.pass has no incoming connection"))?;

    // 4) Validate only the kept subgraph.
    schema::validate_scene_against(&scene, &scheme)?;

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

    let topo_order = topo_sort(&scene)?;

    let composite_layers_in_draw_order =
        composite_layers_in_draw_order(&scene, &nodes_by_id, &output_node_id)?;

    let output_node = find_node(&nodes_by_id, &output_node_id)?;
    if output_node.node_type != "Composite" {
        bail!(
            "RenderTarget.pass must come from Composite, got {}",
            output_node.node_type
        );
    }

    // New DSL contract: output target must be provided by Composite.target.
    let output_texture_node_id: String = incoming_connection(&scene, &output_node_id, "target")
        .map(|c| c.from.node_id.clone())
        .ok_or_else(|| anyhow!("Composite.target has no incoming connection"))?;

    let output_texture_name: ResourceName = ids
        .get(&output_texture_node_id)
        .cloned()
        .ok_or_else(|| anyhow!("missing name for node: {}", output_texture_node_id))?;

    let output_texture_node = find_node(&nodes_by_id, &output_texture_node_id)?;
    if output_texture_node.node_type != "RenderTexture" {
        bail!(
            "Composite.target must come from RenderTexture, got {}",
            output_texture_node.node_type
        );
    }

    let width = parse_u32(&output_texture_node.params, "width").unwrap_or(1024);
    let height = parse_u32(&output_texture_node.params, "height").unwrap_or(1024);
    let resolution = [width, height];

    Ok(PreparedScene {
        scene,
        nodes_by_id,
        ids,
        topo_order,
        composite_layers_in_draw_order,
        output_texture_node_id,
        output_texture_name,
        resolution,
    })
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

fn clamp_min_1(v: u32) -> u32 {
    v.max(1)
}

fn gaussian_mip_level_and_sigma_p(sigma: f32) -> (u32, f32) {
    // Ported from BlurMipmapGenerator.GetMipLevelAndSigmaP.
    let mut m_sample_count: u32 = 0;
    let mut sigma_p: f32 = sigma * sigma;
    let step1ds8_vd_target: f32 = 20.0 * 20.0;
    let step1ds4_vd_target: f32 = 9.5 * 9.5;
    let mut step1ds2_vd_target: f32 = 3.6 * 3.5;
    if sigma_p > 100.0 {
        step1ds2_vd_target = 5.5 * 5.5;
    }
    if sigma_p > step1ds8_vd_target {
        sigma_p = sigma_p / 64.0 - 0.140625;
        m_sample_count = 3;
    }
    if sigma_p >= step1ds4_vd_target {
        if m_sample_count == 0 {
            sigma_p = sigma_p / 16.0 - 0.47265625;
            m_sample_count = 2;
        }
    }
    if sigma_p >= step1ds2_vd_target {
        sigma_p = sigma_p / 4.0 - 0.756625;
        if m_sample_count >= 1 {
            m_sample_count += 1;
        } else {
            m_sample_count = 1;
        }
    }
    (m_sample_count, sigma_p)
}

fn gaussian_kernel_8(sigma: f32) -> ([f32; 8], [f32; 8], u32) {
    // Ported from BlurMipmapGenerator.GetGuassianKernel.
    let mut gaussian_kernel: [f64; 27] = [0.0; 27];
    let narrow_band: i32 = 27;
    let coefficient: f64 = 1.0 / f64::sqrt(sigma as f64 * std::f64::consts::PI * 2.0);
    let mut weight_sum: f32 = 0.0;

    for weight_index in 0..27 {
        let x = (weight_index as i32 - 13) as f64;
        let weight = f64::exp(-1.0 * x * x * 0.5 / sigma as f64) * coefficient;
        gaussian_kernel[weight_index] = weight;
        weight_sum += weight as f32;
    }

    for i in 0..27 {
        gaussian_kernel[i] /= weight_sum as f64;
    }
    gaussian_kernel[13] /= 2.0;

    let weight1 = gaussian_kernel[11] + gaussian_kernel[10];
    let offset0 = gaussian_kernel[12] / (gaussian_kernel[13] + gaussian_kernel[12]);

    let (weight1, offset1) = if (gaussian_kernel[10] + gaussian_kernel[11]) < 0.002 {
        (0.0, 0.0)
    } else {
        (
            weight1,
            gaussian_kernel[10] / (gaussian_kernel[10] + gaussian_kernel[11]) + 2.0,
        )
    };

    let (weight2, offset2) = if narrow_band < 11 || ((gaussian_kernel[8] + gaussian_kernel[9]) < 0.002) {
        (0.0, 0.0)
    } else {
        (
            gaussian_kernel[8] + gaussian_kernel[9],
            gaussian_kernel[8] / (gaussian_kernel[8] + gaussian_kernel[9]) + 4.0,
        )
    };

    let (weight3, offset3) = if narrow_band < 15 || ((gaussian_kernel[6] + gaussian_kernel[7]) < 0.002) {
        (0.0, 0.0)
    } else {
        (
            gaussian_kernel[6] + gaussian_kernel[7],
            gaussian_kernel[6] / (gaussian_kernel[6] + gaussian_kernel[7]) + 6.0,
        )
    };

    let (weight4, offset4) = if narrow_band < 19 || ((gaussian_kernel[4] + gaussian_kernel[5]) < 0.002) {
        (0.0, 0.0)
    } else {
        (
            gaussian_kernel[4] + gaussian_kernel[5],
            gaussian_kernel[4] / (gaussian_kernel[4] + gaussian_kernel[5]) + 8.0,
        )
    };

    let (weight5, offset5) = if narrow_band < 23 || ((gaussian_kernel[2] + gaussian_kernel[3]) < 0.002) {
        (0.0, 0.0)
    } else {
        (
            gaussian_kernel[2] + gaussian_kernel[3],
            gaussian_kernel[2] / (gaussian_kernel[2] + gaussian_kernel[3]) + 10.0,
        )
    };

    let (weight6, offset6) = if narrow_band < 27 || ((gaussian_kernel[0] + gaussian_kernel[1]) < 0.002) {
        (0.0, 0.0)
    } else {
        (
            gaussian_kernel[0] + gaussian_kernel[1],
            gaussian_kernel[0] / (gaussian_kernel[0] + gaussian_kernel[1]) + 12.0,
        )
    };

    let weight0 = 0.5 - (weight1 + weight2 + weight3 + weight4 + weight5 + weight6);

    let kernel: [f32; 8] = [
        weight0 as f32,
        weight1 as f32,
        weight2 as f32,
        weight3 as f32,
        weight4 as f32,
        weight5 as f32,
        weight6 as f32,
        0.0,
    ];
    let offset: [f32; 8] = [
        offset0 as f32,
        offset1 as f32,
        offset2 as f32,
        offset3 as f32,
        offset4 as f32,
        offset5 as f32,
        offset6 as f32,
        0.0,
    ];

    let mut num: u32 = 0;
    for i in 0..8 {
        let w = kernel[i];
        if w > 0.0 && w < 1.0 {
            num += 1;
        }
    }
    (kernel, offset, num)
}

fn fmt_f32(v: f32) -> String {
    if v.is_finite() {
        let s = format!("{v:.9}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        "0.0".to_string()
    }
}

fn array8_f32_wgsl(values: [f32; 8]) -> String {
    let parts: Vec<String> = values.into_iter().map(fmt_f32).collect();
    format!("array<f32, 8>({})", parts.join(", "))
}

fn build_fullscreen_textured_bundle(fragment_body: String) -> WgslShaderBundle {
    // Shared Params struct to match the runtime uniform.
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

@group(1) @binding(0)
var src_tex: texture_2d<f32>;
@group(1) @binding(1)
var src_samp: sampler;
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
    let module = format!("{common}{vertex_entry}{fragment_entry}");

    WgslShaderBundle {
        common,
        vertex,
        fragment,
        compute: None,
        module,
        image_textures: Vec::new(),
    }
}

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
                let Some(hi) = hex(hi) else { bail!("invalid percent-encoding"); };
                let Some(lo) = hex(lo) else { bail!("invalid percent-encoding"); };
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

// Helper functions for backward compatibility
fn typed(expr: impl Into<String>, ty: ValueType) -> TypedExpr {
    TypedExpr::new(expr, ty)
}

fn typed_time(expr: impl Into<String>, ty: ValueType, uses_time: bool) -> TypedExpr {
    TypedExpr::with_time(expr, ty, uses_time)
}

impl MaterialCompileContext {
    // Extension method for generating WGSL binding declarations
    fn wgsl_decls(&self) -> String {
        if self.image_textures.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        for (i, node_id) in self.image_textures.iter().enumerate() {
            let tex_binding = (i as u32) * 2;
            let samp_binding = tex_binding + 1;
            out.push_str(&format!(
                "@group(1) @binding({tex_binding})\nvar {}: texture_2d<f32>;\n\n",
                Self::tex_var_name(node_id)
            ));
            out.push_str(&format!(
                "@group(1) @binding({samp_binding})\nvar {}: sampler;\n\n",
                Self::sampler_var_name(node_id)
            ));
        }
        out
    }
}

// The compile_material_expr function has been moved to the modular renderer::node_compiler module.
// It is now implemented as a dispatch system that routes to specific node compiler modules.
// See: src/renderer/node_compiler/mod.rs
//
// The old monolithic implementation (356 lines) has been replaced with focused modules:
// - input_nodes.rs, math_nodes.rs, attribute.rs, texture_nodes.rs, trigonometry_nodes.rs
// - legacy_nodes.rs, vector_nodes.rs, color_nodes.rs
// 
// Use: renderer::node_compiler::compile_material_expr instead.

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

    /// ImageTexture node ids referenced by this pass's material graph, in binding order.
    pub image_textures: Vec<String>,
}

pub fn build_pass_wgsl_bundle(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    pass_id: &str,
) -> Result<WgslShaderBundle> {
    // If RenderPass.material is connected, compile the upstream subgraph into an expression.
    // Otherwise, fallback to constant color.
    let mut material_ctx = MaterialCompileContext::default();
    let fragment_expr: TypedExpr = if let Some(conn) = incoming_connection(scene, pass_id, "material") {
        let mut cache: HashMap<(String, String), TypedExpr> = HashMap::new();
        compile_material_expr(
            scene,
            nodes_by_id,
            &conn.from.node_id,
            Some(&conn.from.port_id),
            &mut material_ctx,
            &mut cache,
        )?
    } else {
        typed("params.color".to_string(), ValueType::Vec4)
    };

    let image_textures = material_ctx.image_textures.clone();

    let out_color = to_vec4_color(fragment_expr);
    let fragment_body = format!("return {};", out_color.expr);

    let mut common = r#"
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

    common.push_str(&material_ctx.wgsl_decls());

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
        image_textures,
    })
}

pub fn build_all_pass_wgsl_bundles_from_scene(
    scene: &SceneDSL,
) -> Result<Vec<(String, WgslShaderBundle)>> {
    let prepared = prepare_scene(scene)?;

    let mut out: Vec<(String, WgslShaderBundle)> = Vec::new();
    for layer_id in prepared.composite_layers_in_draw_order {
        let node = find_node(&prepared.nodes_by_id, &layer_id)?;
        match node.node_type.as_str() {
            "RenderPass" => {
                let bundle =
                    build_pass_wgsl_bundle(&prepared.scene, &prepared.nodes_by_id, &layer_id)?;
                out.push((layer_id, bundle));
            }
            "GuassianBlurPass" => {
                // Emit synthetic passes:
                // - downsample_* (one step, or 8 then 2 when factor=16)
                // - hblur / vblur at downsampled resolution
                // - upsample bilinear back to original target size
                let sigma = parse_f32(&node.params, "radius").unwrap_or(0.0).max(0.0);
                let (mip_level, sigma_p) = gaussian_mip_level_and_sigma_p(sigma);
                let downsample_factor: u32 = 1 << mip_level;
                let (kernel, offset, _num) = gaussian_kernel_8(sigma_p.max(1e-6));

                let downsample_steps: Vec<u32> = if downsample_factor == 16 {
                    vec![8, 2]
                } else {
                    vec![downsample_factor]
                };

                for step in &downsample_steps {
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
                        other => {
                            return Err(anyhow!(
                                "GuassianBlurPass: unsupported downsample factor {other}"
                            ));
                        }
                    };
                    out.push((
                        format!("{layer_id}__downsample_{step}"),
                        build_fullscreen_textured_bundle(body),
                    ));
                }
                let hblur_body = {
                    let kernel_wgsl = array8_f32_wgsl(kernel);
                    let offset_wgsl = array8_f32_wgsl(offset);
                    format!(
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
                    )
                };

                let vblur_body = {
                    let kernel_wgsl = array8_f32_wgsl(kernel);
                    let offset_wgsl = array8_f32_wgsl(offset);
                    format!(
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
                    )
                };

                let upsample_body = {
                    format!(
                        r#"
let dst_xy = vec2f(in.position.xy);
let dst_resolution = params.target_size;
let uv = dst_xy / dst_resolution;
return textureSampleLevel(src_tex, src_samp, uv, 0.0);
"#
                    )
                };

                out.push((
                    format!("{layer_id}__hblur_ds{downsample_factor}"),
                    build_fullscreen_textured_bundle(hblur_body),
                ));
                out.push((
                    format!("{layer_id}__vblur_ds{downsample_factor}"),
                    build_fullscreen_textured_bundle(vblur_body),
                ));
                out.push((
                    format!("{layer_id}__upsample_bilinear_ds{downsample_factor}"),
                    build_fullscreen_textured_bundle(upsample_body),
                ));
            }
            other => bail!(
                "Composite layer must be RenderPass or GuassianBlurPass, got {other} for {layer_id}"
            ),
        }
    }
    Ok(out)
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
