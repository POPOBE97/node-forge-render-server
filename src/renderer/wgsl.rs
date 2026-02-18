//! WGSL shader generation module.
//!
//! This module handles:
//! - WGSL shader bundle generation for render passes
//! - Gaussian blur utilities for post-processing effects
//! - Helper functions for formatting WGSL code

use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use crate::{
    dsl::{Node, SceneDSL, find_node, incoming_connection},
    renderer::{
        graph_uniforms::build_graph_schema,
        node_compiler::compile_material_expr,
        render_plan::{parse_kernel_source_js_like, resolve_geometry_for_render_pass},
        scene_prep::prepare_scene,
        types::{
            GraphBindingKind, GraphFieldKind, GraphSchema, Kernel2D, MaterialCompileContext,
            TypedExpr, ValueType, WgslShaderBundle,
        },
        utils::{cpu_num_f32_min_0, cpu_num_u32_min_1, fmt_f32 as fmt_f32_utils, to_vec4_color},
    },
};

pub(crate) fn clamp_min_1(v: u32) -> u32 {
    v.max(1)
}

pub(crate) fn gaussian_mip_level_and_sigma_p(sigma: f32) -> (u32, f32) {
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

pub(crate) fn gaussian_kernel_8(sigma: f32) -> ([f32; 8], [f32; 8], u32) {
    // Ported from BlurMipmapGenerator.GetGuassianKernel.
    let mut gaussian_kernel: [f64; 27] = [0.0; 27];
    let narrow_band: i32 = 27;
    let coefficient: f64 = 1.0 / f64::sqrt(sigma as f64 * std::f64::consts::PI * 2.0);
    let mut weight_sum: f64 = 0.0;

    for weight_index in 0..27 {
        let x = (weight_index as i32 - 13) as f64;
        let weight = f64::exp(-1.0 * x * x * 0.5 / sigma as f64) * coefficient;
        gaussian_kernel[weight_index] = weight;
        weight_sum += weight;
    }

    for i in 0..27 {
        gaussian_kernel[i] /= weight_sum;
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

    let (weight2, offset2) =
        if narrow_band < 11 || ((gaussian_kernel[8] + gaussian_kernel[9]) < 0.002) {
            (0.0, 0.0)
        } else {
            (
                gaussian_kernel[8] + gaussian_kernel[9],
                gaussian_kernel[8] / (gaussian_kernel[8] + gaussian_kernel[9]) + 4.0,
            )
        };

    let (weight3, offset3) =
        if narrow_band < 15 || ((gaussian_kernel[6] + gaussian_kernel[7]) < 0.002) {
            (0.0, 0.0)
        } else {
            (
                gaussian_kernel[6] + gaussian_kernel[7],
                gaussian_kernel[6] / (gaussian_kernel[6] + gaussian_kernel[7]) + 6.0,
            )
        };

    let (weight4, offset4) =
        if narrow_band < 19 || ((gaussian_kernel[4] + gaussian_kernel[5]) < 0.002) {
            (0.0, 0.0)
        } else {
            (
                gaussian_kernel[4] + gaussian_kernel[5],
                gaussian_kernel[4] / (gaussian_kernel[4] + gaussian_kernel[5]) + 8.0,
            )
        };

    let (weight5, offset5) =
        if narrow_band < 23 || ((gaussian_kernel[2] + gaussian_kernel[3]) < 0.002) {
            (0.0, 0.0)
        } else {
            (
                gaussian_kernel[2] + gaussian_kernel[3],
                gaussian_kernel[2] / (gaussian_kernel[2] + gaussian_kernel[3]) + 10.0,
            )
        };

    let (weight6, offset6) =
        if narrow_band < 27 || ((gaussian_kernel[0] + gaussian_kernel[1]) < 0.002) {
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

pub(crate) fn fmt_f32(v: f32) -> String {
    if v.is_finite() {
        let s = format!("{v:.9}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        "0.0".to_string()
    }
}

pub(crate) fn array8_f32_wgsl(values: [f32; 8]) -> String {
    let parts: Vec<String> = values.into_iter().map(fmt_f32).collect();
    format!("array<f32, 8>({})", parts.join(", "))
}

/// Build a compose pass shader that samples a source texture and draws it at a position
/// determined by dynamic graph inputs (Vector2Input nodes for position and size).
///
/// This is used when a RenderPass with dynamic geometry is a Downsample source and also
/// a composite layer. The main pass renders to an intermediate fullscreen texture, and
/// this compose pass samples that texture and positions it on the final target using
/// the runtime graph_inputs values.
pub(crate) fn build_dynamic_rect_compose_bundle(
    graph_inputs_wgsl: &str,
    position_expr: &str,
    size_expr: &str,
) -> WgslShaderBundle {
    // Shared Params struct to match the runtime uniform.
    let common = format!(
        r#"
struct Params {{
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,

    geo_translate: vec2f,
    geo_scale: vec2f,

    // Pack to 16-byte boundary.
    time: f32,
    _pad0: f32,

    // 16-byte aligned.
    color: vec4f,
}};


@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {{
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    // GLSL-like gl_FragCoord.xy: bottom-left origin, pixel-centered.
    @location(1) frag_coord_gl: vec2f,
    // Geometry-local pixel coordinate (GeoFragcoord): origin at bottom-left.
    @location(2) local_px: vec3f,
    // Geometry size in pixels after applying geometry/instance transforms.
    @location(3) geo_size_px: vec2f,
}};

{graph_inputs_wgsl}

@group(1) @binding(0)
var src_tex: texture_2d<f32>;
@group(1) @binding(1)
var src_samp: sampler;
"#
    );

    // Vertex shader that applies dynamic position/size from graph_inputs
    let vertex_entry = format!(
        r#"
@vertex
fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {{
    var out: VSOut;

    // UV passed as vertex attribute.
    out.uv = uv;

    // Dynamic rect position and size from graph inputs.
    let rect_center_px = {position_expr};
    let rect_size_px = {size_expr};

    out.geo_size_px = rect_size_px;
    // Geometry-local pixel coordinate (GeoFragcoord): bottom-left origin.
    // UV is top-left convention, so flip Y for GLSL-like local_px.
    out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, position.z);

    // Unit vertices [-0.5, 0.5] scaled by dynamic size.
    let p_local = vec3f(position.xy * rect_size_px, position.z);

    // Convert to target pixel coordinates with bottom-left origin.
    let p_px = rect_center_px + p_local.xy;

    // Convert pixels to clip space assuming bottom-left origin.
    // (0,0) => (-1,-1), (target_size) => (1,1)
    let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);
    out.position = vec4f(ndc, position.z / params.target_size.x, 1.0);

    // Pixel-centered like GLSL gl_FragCoord.xy.
    out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
    return out;
}}
"#
    );

    // Fragment shader samples the source texture directly.
    let fragment_entry = r#"
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    return textureSample(src_tex, src_samp, in.uv);
}
"#
    .to_string();

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
        pass_textures: Vec::new(),
        graph_schema: None,
        graph_binding_kind: None,
    }
}

pub(crate) fn build_fullscreen_textured_bundle(fragment_body: String) -> WgslShaderBundle {
    build_fullscreen_textured_bundle_with_instance_index(fragment_body, false)
}

pub(crate) fn build_fullscreen_textured_bundle_with_instance_index(
    fragment_body: String,
    uses_instance_index: bool,
) -> WgslShaderBundle {
    // Shared Params struct to match the runtime uniform.
    let mut common = r#"
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,

    geo_translate: vec2f,
    geo_scale: vec2f,

    // Pack to 16-byte boundary.
    time: f32,
    _pad0: f32,

    // 16-byte aligned.
    color: vec4f,
};


@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    // GLSL-like gl_FragCoord.xy: bottom-left origin, pixel-centered.
    @location(1) frag_coord_gl: vec2f,
    // Geometry-local pixel coordinate (GeoFragcoord): origin at bottom-left.
    @location(2) local_px: vec3f,
    // Geometry size in pixels after applying geometry/instance transforms.
    @location(3) geo_size_px: vec2f,
    @location(4) instance_index: u32,
};


@group(1) @binding(0)

var src_tex: texture_2d<f32>;
@group(1) @binding(1)
var src_samp: sampler;
"#
    .to_string();

    if !uses_instance_index {
        common = common.replace("    @location(4) instance_index: u32,\n", "");
    }

    // NOTE: keep common minimal; sampling behavior is controlled via wgpu sampler.

    let vertex_entry = if uses_instance_index {
        r#"
 @vertex
   fn vs_main(
       @location(0) position: vec3f,
       @location(1) uv: vec2f,
       @builtin(instance_index) instance_index: u32,
   ) -> VSOut {
       var out: VSOut;
       out.instance_index = instance_index;
  
      let _unused_geo_size = params.geo_size;
      let _unused_geo_translate = params.geo_translate;
      let _unused_geo_scale = params.geo_scale;
  
         // UV passed as vertex attribute.
         out.uv = uv;

         out.geo_size_px = params.geo_size;

         // Geometry-local pixel coordinate (GeoFragcoord): bottom-left origin.
         // UV is top-left convention, so flip Y for GLSL-like local_px.
         out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, position.z);
  
       // Geometry vertices are in local pixel units centered at (0,0).
       // Convert to target pixel coordinates with bottom-left origin.
       let p_px = params.center + position.xy;



     // Convert pixels to clip space assuming bottom-left origin.
     // (0,0) => (-1,-1), (target_size) => (1,1)
     let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);
     out.position = vec4f(ndc, position.z / params.target_size.x, 1.0);

     // Pixel-centered like GLSL gl_FragCoord.xy.
      out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
      return out;
  }
  "#
    } else {
        r#"
 @vertex
  fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {
      var out: VSOut;
 
      let _unused_geo_size = params.geo_size;
      let _unused_geo_translate = params.geo_translate;
     let _unused_geo_scale = params.geo_scale;
 
        // UV passed as vertex attribute.
        out.uv = uv;

        out.geo_size_px = params.geo_size;

         // Geometry-local pixel coordinate (GeoFragcoord): bottom-left origin.
         // UV is top-left convention, so flip Y for GLSL-like local_px.
         out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, position.z);
 
       // Geometry vertices are in local pixel units centered at (0,0).
       // Convert to target pixel coordinates with bottom-left origin.
       let p_px = params.center + position.xy;



     // Convert pixels to clip space assuming bottom-left origin.
     // (0,0) => (-1,-1), (target_size) => (1,1)
     let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);
     out.position = vec4f(ndc, position.z / params.target_size.x, 1.0);

      // Pixel-centered like GLSL gl_FragCoord.xy.
      out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
      return out;
  }
  "#
    }
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
        pass_textures: Vec::new(),
        graph_schema: None,
        graph_binding_kind: None,
    }
}

impl MaterialCompileContext {
    // Extension method for generating WGSL binding declarations
    pub(crate) fn wgsl_decls(&self) -> String {
        let mut out = String::new();

        // Image texture bindings
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

        // Pass texture bindings (offset by image texture count)
        let pass_binding_offset = (self.image_textures.len() as u32) * 2;
        for (i, pass_node_id) in self.pass_textures.iter().enumerate() {
            let tex_binding = pass_binding_offset + (i as u32) * 2;
            let samp_binding = tex_binding + 1;
            out.push_str(&format!(
                "@group(1) @binding({tex_binding})\nvar {}: texture_2d<f32>;\n\n",
                Self::pass_tex_var_name(pass_node_id)
            ));
            out.push_str(&format!(
                "@group(1) @binding({samp_binding})\nvar {}: sampler;\n\n",
                Self::pass_sampler_var_name(pass_node_id)
            ));
        }

        // Extra helper declarations (functions, structs, consts) emitted by compilers.
        if !self.extra_wgsl_decls.is_empty() {
            out.push_str("\n// --- Extra WGSL declarations (generated) ---\n");
            for (_name, decl) in self.extra_wgsl_decls.iter() {
                out.push_str(decl);
                if !decl.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
        }

        out
    }
}

pub(crate) fn merge_graph_input_kinds(
    material_ctx: &MaterialCompileContext,
    extra: &std::collections::BTreeMap<String, GraphFieldKind>,
) -> Option<GraphSchema> {
    let mut kinds = material_ctx.graph_input_kinds.clone();
    for (node_id, kind) in extra {
        kinds.entry(node_id.clone()).or_insert(*kind);
    }
    if kinds.is_empty() {
        None
    } else {
        Some(build_graph_schema(&kinds))
    }
}

pub(crate) fn graph_inputs_wgsl_decl(schema: &GraphSchema, kind: GraphBindingKind) -> String {
    let mut out = String::new();
    out.push_str("\nstruct GraphInputs {\n");
    for field in &schema.fields {
        out.push_str(&format!(
            "    // Node: {}\n    {}: {},\n",
            field.node_id,
            field.field_name,
            field.kind.wgsl_slot_type()
        ));
    }
    out.push_str("};\n\n");

    out.push_str("@group(0) @binding(2)\n");
    match kind {
        GraphBindingKind::Uniform => out.push_str("var<uniform> graph_inputs: GraphInputs;\n"),
        GraphBindingKind::StorageRead => {
            out.push_str("var<storage, read> graph_inputs: GraphInputs;\n")
        }
    }
    out
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

/// Build a WGSL shader bundle for the `pass` input of a GuassianBlurPass.
///
/// The node scheme models GuassianBlurPass's source as a `pass`-typed input. During scene prep,
/// non-pass inputs can be auto-wrapped into a synthesized fullscreen RenderPass.
/// This shader bundle samples the upstream pass texture into a fullscreen render target.
pub fn build_blur_image_wgsl_bundle(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    blur_pass_id: &str,
) -> Result<WgslShaderBundle> {
    build_blur_image_wgsl_bundle_with_graph_binding(scene, nodes_by_id, blur_pass_id, None)
}

pub fn build_blur_image_wgsl_bundle_with_graph_binding(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    blur_pass_id: &str,
    forced_graph_binding_kind: Option<GraphBindingKind>,
) -> Result<WgslShaderBundle> {
    // Source is provided on the `pass` input.
    let Some(conn) = incoming_connection(scene, blur_pass_id, "pass") else {
        // No input - return transparent.
        return Ok(build_fullscreen_textured_bundle(
            "return vec4f(0.0, 0.0, 0.0, 0.0);".to_string(),
        ));
    };

    let source_is_pass = nodes_by_id.get(&conn.from.node_id).is_some_and(|node| {
        matches!(
            node.node_type.as_str(),
            "RenderPass" | "GuassianBlurPass" | "Downsample"
        )
    });

    if source_is_pass {
        let mut bundle =
            crate::renderer::wgsl_templates::fullscreen::build_fullscreen_sampled_bundle();
        bundle.pass_textures = vec![conn.from.node_id.clone()];
        return Ok(bundle);
    }

    // Non-pass source: compile the connected material expression directly.
    let mut material_ctx = MaterialCompileContext::default();
    let mut cache: HashMap<(String, String), TypedExpr> = HashMap::new();
    let fragment_expr = compile_material_expr(
        scene,
        nodes_by_id,
        &conn.from.node_id,
        Some(&conn.from.port_id),
        &mut material_ctx,
        &mut cache,
    )?;
    let out_color = to_vec4_color(fragment_expr);
    let fragment_body = material_ctx.build_fragment_body(&out_color.expr);

    let graph_schema = merge_graph_input_kinds(&material_ctx, &std::collections::BTreeMap::new());
    let graph_binding_kind = graph_schema
        .as_ref()
        .map(|_| forced_graph_binding_kind.unwrap_or(GraphBindingKind::Uniform));

    let mut common = r#"
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,

    geo_translate: vec2f,
    geo_scale: vec2f,

    // Pack to 16-byte boundary.
    time: f32,
    _pad0: f32,

    // 16-byte aligned.
    color: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    // GLSL-like gl_FragCoord.xy: bottom-left origin, pixel-centered.
    @location(1) frag_coord_gl: vec2f,
    // Geometry-local pixel coordinate (GeoFragcoord): origin at bottom-left.
    @location(2) local_px: vec3f,
    // Geometry size in pixels after applying geometry/instance transforms.
    @location(3) geo_size_px: vec2f,
};
"#
    .to_string();

    if let (Some(schema), Some(kind)) = (graph_schema.as_ref(), graph_binding_kind) {
        common.push_str(&graph_inputs_wgsl_decl(schema, kind));
    }
    common.push_str(&material_ctx.wgsl_decls());

    let vertex = r#"
@vertex
fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {
    var out: VSOut;
    out.uv = uv;
    out.geo_size_px = params.geo_size;
    // UV is top-left convention, so flip Y for GLSL-like local_px.
    out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, position.z);

    let p_px = params.center + position.xy;
    let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);
    out.position = vec4f(ndc, position.z / params.target_size.x, 1.0);
    out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
    return out;
}
"#;

    let fragment = format!(
        r#"
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {{
{}
}}
"#,
        fragment_body
    );

    let vertex_src = format!("{common}{vertex}");
    let fragment_src = format!("{common}{fragment}");
    let module = format!("{common}{vertex}{fragment}");

    Ok(WgslShaderBundle {
        common,
        vertex: vertex_src,
        fragment: fragment_src,
        compute: None,
        module,
        image_textures: material_ctx.image_textures,
        pass_textures: material_ctx.pass_textures,
        graph_schema,
        graph_binding_kind,
    })
}

pub fn build_pass_wgsl_bundle(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    baked_data_parse: Option<
        std::sync::Arc<
            std::collections::HashMap<
                (String, String, String),
                Vec<crate::renderer::types::BakedValue>,
            >,
        >,
    >,
    baked_data_parse_meta: Option<std::sync::Arc<crate::renderer::types::BakedDataParseMeta>>,
    pass_id: &str,
    is_instanced: bool,
    vertex_translate_expr: Option<String>,
    vertex_inline_stmts: Vec<String>,
    vertex_wgsl_decls: String,
    vertex_uses_instance_index: bool,
) -> Result<WgslShaderBundle> {
    build_pass_wgsl_bundle_with_graph_binding(
        scene,
        nodes_by_id,
        baked_data_parse,
        baked_data_parse_meta,
        pass_id,
        is_instanced,
        vertex_translate_expr,
        vertex_inline_stmts,
        vertex_wgsl_decls,
        vertex_uses_instance_index,
        None,
        std::collections::BTreeMap::new(),
        None,
        false, // fullscreen_vertex_positioning
        false, // has_normals
    )
}

pub(crate) fn build_pass_wgsl_bundle_with_graph_binding(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    baked_data_parse: Option<
        std::sync::Arc<
            std::collections::HashMap<
                (String, String, String),
                Vec<crate::renderer::types::BakedValue>,
            >,
        >,
    >,
    baked_data_parse_meta: Option<std::sync::Arc<crate::renderer::types::BakedDataParseMeta>>,
    pass_id: &str,
    is_instanced: bool,
    vertex_translate_expr: Option<String>,
    vertex_inline_stmts: Vec<String>,
    vertex_wgsl_decls: String,
    vertex_uses_instance_index: bool,
    _rect2d_dynamic_inputs: Option<crate::renderer::render_plan::geometry::Rect2DDynamicInputs>,
    vertex_graph_input_kinds: std::collections::BTreeMap<String, GraphFieldKind>,
    forced_graph_binding_kind: Option<GraphBindingKind>,
    // When true, vertex positioning uses fullscreen within target (params.center), but
    // geo_size_px/local_px still use dynamic size from _rect2d_dynamic_inputs if available.
    // This is used when a pass renders to an intermediate texture that will be composited later.
    fullscreen_vertex_positioning: bool,
    // When true, the vertex shader declares @location(6) normal: vec3f and passes it through VSOut.
    has_normals: bool,
) -> Result<WgslShaderBundle> {
    // If RenderPass.material is connected, compile the upstream subgraph into an expression.
    // Otherwise, fallback to constant color.
    let mut material_ctx = MaterialCompileContext {
        baked_data_parse,
        baked_data_parse_meta,
        ..Default::default()
    };
    let fragment_expr: TypedExpr =
        if let Some(conn) = incoming_connection(scene, pass_id, "material") {
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
            // Premultiply params.color on the shader side to match premultiplied blending defaults.
            TypedExpr::new(
                "vec4f(params.color.rgb * params.color.a, params.color.a)".to_string(),
                ValueType::Vec4,
            )
        };

    let image_textures = material_ctx.image_textures.clone();

    let out_color = to_vec4_color(fragment_expr);
    let fragment_body = material_ctx.build_fragment_body(&out_color.expr);

    let graph_schema = merge_graph_input_kinds(&material_ctx, &vertex_graph_input_kinds);
    let graph_binding_kind = graph_schema
        .as_ref()
        .map(|_| forced_graph_binding_kind.unwrap_or(GraphBindingKind::Uniform));

    let mut common = r#"
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,

    geo_translate: vec2f,
    geo_scale: vec2f,

    // Pack to 16-byte boundary.
    time: f32,
    _pad0: f32,

    // 16-byte aligned.
    color: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

 struct VSOut {
     @builtin(position) position: vec4f,
     @location(0) uv: vec2f,
     // GLSL-like gl_FragCoord.xy: bottom-left origin, pixel-centered.
     @location(1) frag_coord_gl: vec2f,
     // Geometry-local pixel coordinate (GeoFragcoord): origin at bottom-left.
     @location(2) local_px: vec3f,
     // Geometry size in pixels after applying geometry/instance transforms.
     @location(3) geo_size_px: vec2f,
     @location(4) instance_index: u32,
     @location(5) normal: vec3f,
 };

"#
    .to_string();

    if let (Some(schema), Some(kind)) = (graph_schema.as_ref(), graph_binding_kind) {
        common.push_str(&graph_inputs_wgsl_decl(schema, kind));
    }

    if !(vertex_uses_instance_index || material_ctx.uses_instance_index) {
        common = common.replace("    @location(4) instance_index: u32,\n", "");
    }

    if !has_normals {
        common = common.replace("     @location(5) normal: vec3f,\n", "");
    }

    if material_ctx.baked_data_parse_meta.is_some() {
        common.push_str(
            "\n@group(0) @binding(1)\nvar<storage, read> baked_data_parse: array<vec4f>;\n",
        );
    }

    common.push_str(&material_ctx.wgsl_decls());
    common.push_str(&vertex_wgsl_decls);

    // If we inject a vertex expression (TransformGeometry driven by a node graph), we must
    // include any inline statements (e.g. MathClosure local bindings) in the vertex entry.
    let vertex_inline_stmts = vertex_inline_stmts.join("\n");

    // When fullscreen_vertex_positioning is true, we use fullscreen geometry for vertex positioning
    // but still use dynamic inputs for geo_size_px/local_px if available.
    let rect_unit_geometry = _rect2d_dynamic_inputs.is_some() && !fullscreen_vertex_positioning;
    let has_dynamic_geo_size = _rect2d_dynamic_inputs.is_some();
    let rect_position_expr = _rect2d_dynamic_inputs
        .as_ref()
        .and_then(|d| d.position_expr.as_ref())
        .map(|e| e.expr.as_str());
    let rect_size_expr = _rect2d_dynamic_inputs
        .as_ref()
        .and_then(|d| d.size_expr.as_ref())
        .map(|e| e.expr.as_str());

    let mut vertex_args = String::new();
    vertex_args.push_str("     @location(0) position: vec3f,\n");
    vertex_args.push_str("     @location(1) uv: vec2f,\n");

    if is_instanced {
        vertex_args.push_str("     @location(2) i0: vec4f,\n");
        vertex_args.push_str("     @location(3) i1: vec4f,\n");
        vertex_args.push_str("     @location(4) i2: vec4f,\n");
        vertex_args.push_str("     @location(5) i3: vec4f,\n");
    }

    if vertex_uses_instance_index || material_ctx.uses_instance_index {
        vertex_args.push_str("     @builtin(instance_index) instance_index: u32,\n");
    }

    if has_normals {
        vertex_args.push_str("     @location(6) normal: vec3f,\n");
    }

    let mut vertex_entry = String::new();
    vertex_entry.push_str("\n @vertex\n fn vs_main(\n");
    vertex_entry.push_str(&vertex_args);
    vertex_entry.push_str(" ) -> VSOut {\n");
    vertex_entry.push_str(" var out: VSOut;\n\n");

    // Keep any MathClosure-generated locals declared before other code.
    if !vertex_inline_stmts.trim().is_empty() {
        vertex_entry.push_str(&vertex_inline_stmts);
        vertex_entry.push_str("\n");
    }

    if vertex_uses_instance_index || material_ctx.uses_instance_index {
        vertex_entry.push_str(" out.instance_index = instance_index;\n\n");
    }

    vertex_entry.push_str(" let _unused_geo_size = params.geo_size;\n");
    vertex_entry.push_str(" let _unused_geo_translate = params.geo_translate;\n");
    vertex_entry.push_str(" let _unused_geo_scale = params.geo_scale;\n\n");

    vertex_entry.push_str(" // UV passed as vertex attribute.\n");
    vertex_entry.push_str(" out.uv = uv;\n\n");

    if has_normals {
        vertex_entry.push_str(" out.normal = normal;\n\n");
    }

    if is_instanced {
        vertex_entry.push_str(" let inst_m = mat4x4f(i0, i1, i2, i3);\n");

        // Geometry-local pixel coordinate (GeoFragcoord).
        // `params.geo_size` is the logical pre-transform size. If the instance matrix scales the
        // geometry, reflect that so SDFs and other pixel-space evaluations scale with the geometry.
        vertex_entry.push_str(" let geo_sx = length(inst_m[0].xy);\n");
        vertex_entry.push_str(" let geo_sy = length(inst_m[1].xy);\n");

        if rect_unit_geometry {
            // Dynamic Rect2DGeometry uses a unit quad and applies size/position in the vertex stage.
            vertex_entry.push_str(" let rect_size_px_base = ");
            vertex_entry.push_str(rect_size_expr.unwrap_or("params.geo_size"));
            vertex_entry.push_str(";\n");
            vertex_entry.push_str(" let rect_center_px = ");
            vertex_entry.push_str(rect_position_expr.unwrap_or("params.center"));
            vertex_entry.push_str(";\n");
            vertex_entry.push_str(" let rect_dyn = vec4f(rect_center_px, rect_size_px_base);\n");

            vertex_entry.push_str(" let geo_size_px = rect_dyn.zw * vec2f(geo_sx, geo_sy);\n");
            vertex_entry.push_str(" out.geo_size_px = geo_size_px;\n");
            vertex_entry.push_str(" out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * geo_size_px, 0.0);\n\n");

            vertex_entry
                .push_str(" let p_rect_local_px = vec3f(position.xy * rect_dyn.zw, position.z);\n");
            vertex_entry.push_str(" var p_local = (inst_m * vec4f(p_rect_local_px, 1.0)).xyz;\n\n");
        } else {
            vertex_entry.push_str(" let geo_size_px = params.geo_size * vec2f(geo_sx, geo_sy);\n");
            vertex_entry.push_str(" out.geo_size_px = geo_size_px;\n");
            vertex_entry.push_str(" out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * geo_size_px, 0.0);\n\n");

            vertex_entry.push_str(" var p_local = (inst_m * vec4f(position, 1.0)).xyz;\n\n");
        }

        if let Some(expr) = vertex_translate_expr.as_deref() {
            vertex_entry.push_str(" let delta_t = ");
            vertex_entry.push_str(expr);
            vertex_entry.push_str(";\n");
            vertex_entry.push_str(" p_local = p_local + delta_t;\n\n");
        }
    } else {
        if rect_unit_geometry {
            vertex_entry.push_str(" let rect_size_px_base = ");
            vertex_entry.push_str(rect_size_expr.unwrap_or("params.geo_size"));
            vertex_entry.push_str(";\n");
            vertex_entry.push_str(" let rect_center_px = ");
            vertex_entry.push_str(rect_position_expr.unwrap_or("params.center"));
            vertex_entry.push_str(";\n");
            vertex_entry.push_str(" let rect_dyn = vec4f(rect_center_px, rect_size_px_base);\n");

            vertex_entry.push_str(" out.geo_size_px = rect_dyn.zw;\n");
            vertex_entry.push_str(" // Geometry-local pixel coordinate (GeoFragcoord).\n");
            vertex_entry.push_str(" out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, 0.0);\n\n");
            vertex_entry
                .push_str(" let p_rect_local_px = vec3f(position.xy * rect_dyn.zw, position.z);\n");

            if let Some(expr) = vertex_translate_expr.as_deref() {
                vertex_entry.push_str(" let delta_t = ");
                vertex_entry.push_str(expr);
                vertex_entry.push_str(";\n");
                vertex_entry.push_str(" let p_local = p_rect_local_px + delta_t;\n\n");
            } else {
                vertex_entry.push_str(" let p_local = p_rect_local_px;\n\n");
            }
        } else {
            // has_dynamic_geo_size: use dynamic size for geo_size_px (GeoFragcoord/GeoSize)
            // but keep fullscreen vertex positioning via params.center.
            if has_dynamic_geo_size {
                vertex_entry.push_str(" let rect_size_px_base = ");
                vertex_entry.push_str(rect_size_expr.unwrap_or("params.geo_size"));
                vertex_entry.push_str(";\n");
                vertex_entry.push_str(" out.geo_size_px = rect_size_px_base;\n");
            } else {
                vertex_entry.push_str(" out.geo_size_px = params.geo_size;\n");
            }
            vertex_entry.push_str(" // Geometry-local pixel coordinate (GeoFragcoord).\n");
            vertex_entry.push_str(" out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, 0.0);\n\n");

            if let Some(expr) = vertex_translate_expr.as_deref() {
                vertex_entry.push_str(" let delta_t = ");
                vertex_entry.push_str(expr);
                vertex_entry.push_str(";\n");
                vertex_entry.push_str(" let p_local = position + delta_t;\n\n");
            } else {
                // Keep vertex output identical for non-instanced passes.
                vertex_entry.push_str(" let p_local = position;\n\n");
            }
        }
    }

    vertex_entry.push_str(" // Geometry vertices are in local pixel units centered at (0,0).\n");
    vertex_entry.push_str(" // Convert to target pixel coordinates with bottom-left origin.\n");
    // Update local_px.z with the final transformed Z from p_local.
    vertex_entry.push_str(" out.local_px = vec3f(out.local_px.xy, p_local.z);\n");
    if rect_unit_geometry {
        // NOTE: rect_dyn is declared inside the rect_unit_geometry branch above.
        vertex_entry.push_str(" let p_px = rect_dyn.xy + p_local.xy;\n\n");
    } else {
        vertex_entry.push_str(" let p_px = params.center + p_local.xy;\n\n");
    }

    vertex_entry.push_str(" // Convert pixels to clip space assuming bottom-left origin.\n");
    vertex_entry.push_str(" // (0,0) => (-1,-1), (target_size) => (1,1)\n");
    vertex_entry.push_str(" let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);\n");
    vertex_entry.push_str(" out.position = vec4f(ndc, p_local.z / params.target_size.x, 1.0);\n\n");

    vertex_entry.push_str(" // Pixel-centered like GLSL gl_FragCoord.xy.\n");
    vertex_entry.push_str(" out.frag_coord_gl = p_px + vec2f(0.5, 0.5);\n");
    vertex_entry.push_str(" return out;\n }");

    let vertex_entry = vertex_entry;

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
    let pass_textures = material_ctx.pass_textures.clone();

    Ok(WgslShaderBundle {
        common,
        vertex,
        fragment,
        compute,
        module,
        image_textures,
        pass_textures,
        graph_schema,
        graph_binding_kind,
    })
}

pub fn build_all_pass_wgsl_bundles_from_scene(
    scene: &SceneDSL,
) -> Result<Vec<(String, WgslShaderBundle)>> {
    build_all_pass_wgsl_bundles_from_scene_with_assets(scene, None)
}

pub fn build_all_pass_wgsl_bundles_from_scene_with_assets(
    scene: &SceneDSL,
    asset_store: Option<&crate::asset_store::AssetStore>,
) -> Result<Vec<(String, WgslShaderBundle)>> {
    let prepared = prepare_scene(scene)?;
    let nodes_by_id = &prepared.nodes_by_id;
    let ids = &prepared.ids;
    let output_target_node = find_node(nodes_by_id, &prepared.output_texture_node_id)?;
    let render_target_size = [
        cpu_num_u32_min_1(
            &prepared.scene,
            nodes_by_id,
            output_target_node,
            "width",
            prepared.resolution[0],
        )? as f32,
        cpu_num_u32_min_1(
            &prepared.scene,
            nodes_by_id,
            output_target_node,
            "height",
            prepared.resolution[1],
        )? as f32,
    ];

    let mut baked_data_parse = prepared.baked_data_parse.clone();

    let mut out: Vec<(String, WgslShaderBundle)> = Vec::new();
    for layer_id in prepared.composite_layers_in_draw_order {
        let node = find_node(nodes_by_id, &layer_id)?;
        match node.node_type.as_str() {
            "RenderPass" => {
                let render_geo_node_id =
                    incoming_connection(&prepared.scene, &layer_id, "geometry")
                        .map(|c| c.from.node_id.clone())
                        .ok_or_else(|| anyhow!("RenderPass.geometry missing for {layer_id}"))?;

                let (
                    _geometry_buffer,
                    _geo_w,
                    _geo_h,
                    _geo_x,
                    _geo_y,
                    instance_count,
                    _base_m,
                    _instance_mats,
                    _translate_expr,
                    _vertex_inline_stmts,
                    _vertex_wgsl_decls,
                    _vertex_graph_input_kinds,
                    _vertex_uses_instance_index,
                    _rect_dyn,
                    _normals_bytes,
                ) = resolve_geometry_for_render_pass(
                    &prepared.scene,
                    nodes_by_id,
                    ids,
                    &render_geo_node_id,
                    render_target_size,
                    None,
                    asset_store,
                )?;

                let is_instanced = instance_count > 1;

                baked_data_parse.extend(crate::renderer::scene_prep::bake_data_parse_nodes(
                    nodes_by_id,
                    &layer_id,
                    instance_count,
                )?);

                baked_data_parse.extend(crate::renderer::scene_prep::bake_data_parse_nodes(
                    nodes_by_id,
                    "__global",
                    instance_count,
                )?);

                let meta = {
                    let mut slot_by_output: std::collections::HashMap<
                        (String, String, String),
                        u32,
                    > = std::collections::HashMap::new();
                    let mut keys: Vec<(String, String, String)> = baked_data_parse
                        .keys()
                        .filter(|(pass_id, _, _)| pass_id == &layer_id)
                        .cloned()
                        .collect();
                    keys.sort();
                    for (i, k) in keys.iter().enumerate() {
                        slot_by_output.insert(k.clone(), i as u32);
                    }
                    std::sync::Arc::new(crate::renderer::types::BakedDataParseMeta {
                        pass_id: layer_id.clone(),
                        outputs_per_instance: keys.len() as u32,
                        slot_by_output,
                    })
                };

                let (
                    _geometry_buffer_2,
                    _geo_w_2,
                    _geo_h_2,
                    _geo_x_2,
                    _geo_y_2,
                    _instance_count_2,
                    _base_m_2,
                    _instance_mats_2,
                    translate_expr,
                    vertex_inline_stmts,
                    vertex_wgsl_decls,
                    vertex_graph_input_kinds,
                    vertex_uses_instance_index,
                    rect_dyn_2,
                    _normals_bytes_2,
                ) = resolve_geometry_for_render_pass(
                    &prepared.scene,
                    nodes_by_id,
                    ids,
                    &render_geo_node_id,
                    render_target_size,
                    Some(&MaterialCompileContext {
                        baked_data_parse: Some(std::sync::Arc::new(baked_data_parse.clone())),
                        baked_data_parse_meta: Some(meta.clone()),
                        ..Default::default()
                    }),
                    asset_store,
                )?;

                let bundle = build_pass_wgsl_bundle_with_graph_binding(
                    &prepared.scene,
                    nodes_by_id,
                    Some(std::sync::Arc::new(baked_data_parse.clone())),
                    Some(meta),
                    &layer_id,
                    is_instanced,
                    translate_expr.map(|e| e.expr),
                    vertex_inline_stmts,
                    vertex_wgsl_decls,
                    vertex_uses_instance_index,
                    rect_dyn_2,
                    vertex_graph_input_kinds,
                    None,
                    false, // fullscreen_vertex_positioning
                    false, // has_normals
                )?;

                out.push((layer_id, bundle));
            }
            "Downsample" => {
                // Downsample pass WGSL uses a 2D kernel authored in a connected Kernel node.
                let kernel_node_id = incoming_connection(&prepared.scene, &layer_id, "kernel")
                    .map(|c| c.from.node_id.clone())
                    .ok_or_else(|| anyhow!("Downsample.kernel missing for {layer_id}"))?;
                let kernel_node = find_node(nodes_by_id, &kernel_node_id)?;
                if kernel_node.node_type != "Kernel" {
                    bail!(
                        "Downsample.kernel must come from Kernel node, got {} for {}",
                        kernel_node.node_type,
                        kernel_node_id
                    );
                }
                let kernel_src = kernel_node
                    .params
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let kernel: Kernel2D = parse_kernel_source_js_like(kernel_src.as_str())?;

                let pass_id = format!("sys.downsample.{layer_id}.pass");
                let bundle = build_downsample_pass_wgsl_bundle(&kernel)?;
                out.push((pass_id, bundle));
            }
            "GuassianBlurPass" => {
                // SceneDSL `radius` is authored as an analytic 1D cutoff radius in full-res pixels,
                // not as Gaussian sigma.
                //
                // We map radius -> sigma using the same cutoff epsilon (~0.002) that our packed
                // 27-wide Gaussian kernel effectively uses when pruning tiny weights
                // (see `gaussian_kernel_8`).
                //
                // k = sqrt(2*ln(1/eps)) with eps=0.002 -> k≈3.525494, so sigma = radius/k.
                let radius_px =
                    cpu_num_f32_min_0(&prepared.scene, &prepared.nodes_by_id, node, "radius", 0.0)?;
                let sigma = radius_px / 3.525_494;
                let (mip_level, sigma_p) = gaussian_mip_level_and_sigma_p(sigma);
                let downsample_factor: u32 = 1 << mip_level;
                let (kernel, offset, _num) = gaussian_kernel_8(sigma_p.max(1e-6));

                let downsample_steps: Vec<u32> = if downsample_factor == 16 {
                    vec![8, 2]
                } else {
                    vec![downsample_factor]
                };

                // 0) Source image expression pass (renders `image` input to an intermediate texture).
                let src_bundle =
                    build_blur_image_wgsl_bundle(&prepared.scene, nodes_by_id, &layer_id)?;
                out.push((format!("sys.blur.{layer_id}.src.pass"), src_bundle));

                for step in &downsample_steps {
                    let bundle = build_downsample_bundle(*step)?;
                    out.push((format!("sys.blur.{layer_id}.ds.{step}.pass"), bundle));
                }

                out.push((
                    format!("sys.blur.{layer_id}.h.ds{downsample_factor}.pass"),
                    build_horizontal_blur_bundle(kernel, offset),
                ));
                out.push((
                    format!("sys.blur.{layer_id}.v.ds{downsample_factor}.pass"),
                    build_vertical_blur_bundle(kernel, offset),
                ));
                out.push((
                    format!("sys.blur.{layer_id}.upsample_bilinear.ds{downsample_factor}.pass"),
                    build_upsample_bilinear_bundle(),
                ));
            }
            "GradientBlur" => {
                use crate::renderer::wgsl_gradient_blur::*;

                // Resolve source dimensions to compute padding.
                let src_resolution = {
                    let mut res = prepared.resolution;
                    if let Some(conn) = incoming_connection(&prepared.scene, &layer_id, "source") {
                        if let Some(src_node) = nodes_by_id.get(&conn.from.node_id) {
                            if src_node.node_type == "ImageTexture" {
                                if let Some(dims) =
                                    crate::renderer::shader_space::image_node_dimensions(
                                        src_node, None,
                                    )
                                {
                                    res = dims;
                                }
                            }
                        }
                    }
                    res
                };
                let [padded_w, padded_h] =
                    gradient_blur_padded_size(src_resolution[0], src_resolution[1]);
                let src_w = src_resolution[0] as f32;
                let src_h = src_resolution[1] as f32;
                let pad_w = padded_w as f32;
                let pad_h = padded_h as f32;
                let pad_offset = [(pad_w - src_w) * 0.5, (pad_h - src_h) * 0.5];

                // 0) Source pass
                let src_bundle = build_gradient_blur_source_wgsl_bundle(
                    &prepared.scene,
                    nodes_by_id,
                    &layer_id,
                )?;
                out.push((format!("sys.gb.{layer_id}.src.pass"), src_bundle));

                // 1) Pad pass
                let pad_bundle = build_gradient_blur_pad_wgsl_bundle(src_w, src_h, pad_w, pad_h);
                out.push((format!("sys.gb.{layer_id}.pad.pass"), pad_bundle));

                // 2) Mip chain (6 downsample passes)
                let mip_pass_ids: Vec<String> = (0..GB_MIP_LEVELS)
                    .map(|i| {
                        if i == 0 {
                            format!("sys.gb.{layer_id}.pad")
                        } else {
                            format!("sys.gb.{layer_id}.mip{i}")
                        }
                    })
                    .collect();

                for i in 1..GB_MIP_LEVELS {
                    let ds_bundle =
                        build_downsample_pass_wgsl_bundle(&gradient_blur_cross_kernel())?;
                    out.push((format!("sys.gb.{layer_id}.mip{i}.pass"), ds_bundle));
                }

                // 3) Final composite pass
                let composite_bundle = build_gradient_blur_composite_wgsl_bundle(
                    &prepared.scene,
                    nodes_by_id,
                    &layer_id,
                    &mip_pass_ids,
                    [pad_w, pad_h],
                    pad_offset,
                )?;
                out.push((format!("sys.gb.{layer_id}.final.pass"), composite_bundle));
            }
            other => bail!(
                "Composite layer must be RenderPass, Downsample, or GuassianBlurPass, got {other} for {layer_id}"
            ),
        }
    }

    Ok(out)
}

/// Build a downsample shader bundle for the given factor (1, 2, 4, or 8).
pub fn build_downsample_bundle(factor: u32) -> Result<WgslShaderBundle> {
    let body = match factor {
        1 => {
            r#"
 let src_resolution = vec2f(textureDimensions(src_tex));
 return textureSampleLevel(src_tex, src_samp, in.uv, 0.0);
 "#
        }
        2 => {
            r#"
 let src_resolution = params.target_size * 2.0;
 let src_center = in.uv * src_resolution;
 let base = src_center - vec2f(0.5);
 
 var sum = vec4f(0.0);
 for (var y: i32 = 0; y < 2; y = y + 1) {
     for (var x: i32 = 0; x < 2; x = x + 1) {
         let uv = (base + vec2f(f32(x), f32(y))) / src_resolution;
         sum = sum + textureSampleLevel(src_tex, src_samp, uv, 0.0);
     }
 }
 
 return sum * 0.25;
 "#
        }
        4 => {
            r#"
 let src_resolution = params.target_size * 4.0;
 let src_center = in.uv * src_resolution;
 let base = src_center - vec2f(1.5);
 
 var sum = vec4f(0.0);
 for (var y: i32 = 0; y < 4; y = y + 1) {
     for (var x: i32 = 0; x < 4; x = x + 1) {
         let uv = (base + vec2f(f32(x), f32(y))) / src_resolution;
         sum = sum + textureSampleLevel(src_tex, src_samp, uv, 0.0);
     }
 }
 
 return sum * (1.0 / 16.0);
 "#
        }
        8 => {
            r#"
 let src_resolution = params.target_size * 8.0;
 let src_center = in.uv * src_resolution;
 let base = src_center - vec2f(3.5);
 
 var sum = vec4f(0.0);
 for (var y: i32 = 0; y < 8; y = y + 1) {
     for (var x: i32 = 0; x < 8; x = x + 1) {
         let uv = (base + vec2f(f32(x), f32(y))) / src_resolution;
         sum = sum + textureSampleLevel(src_tex, src_samp, uv, 0.0);
     }
 }
 
 return sum * (1.0 / 64.0);
 "#
        }
        other => {
            return Err(anyhow!(
                "GuassianBlurPass: unsupported downsample factor {other}"
            ));
        }
    };
    Ok(build_fullscreen_textured_bundle(body.to_string()))
}

/// Build a Downsample pass WGSL bundle.
///
/// The Downsample node downsamples an upstream pass into a target resolution using a 2D kernel.
/// Sampling behavior (Mirror/Repeat/Clamp/ClampToBorder) is handled via the runtime sampler.
pub fn build_downsample_pass_wgsl_bundle(kernel: &Kernel2D) -> Result<WgslShaderBundle> {
    let w = kernel.width as i32;
    let h = kernel.height as i32;
    if w <= 0 || h <= 0 {
        bail!(
            "Downsample: invalid kernel size {}x{}",
            kernel.width,
            kernel.height
        );
    }

    let expected = (kernel.width as usize).saturating_mul(kernel.height as usize);
    if kernel.values.len() != expected {
        bail!(
            "Downsample: kernel values length mismatch: expected {expected}, got {}",
            kernel.values.len()
        );
    }

    // Emit the kernel as a WGSL const array.
    let mut kernel_elems: Vec<String> = Vec::with_capacity(kernel.values.len());
    for v in &kernel.values {
        kernel_elems.push(fmt_f32_utils(*v));
    }
    let kernel_arr = format!(
        "array<f32, {}>({})",
        kernel.values.len(),
        kernel_elems.join(", ")
    );

    // Convolve in source pixel space.
    //
    // NOTE: Use `in.local_px` (UV-derived local pixel coordinate) as destination pixel-space.
    //
    // This algorithm matches Godot's downsample shader:
    // 1. Compute center_xy = ceil(normalized_uv * src_dims)
    // 2. Sample at U/D/L/R offsets with manual bilinear (4-point average at integer coords)
    //
    // Sampling behavior (Mirror/Repeat/Clamp) is handled via the runtime sampler.
    let body = format!(
        r#"
    let src_dims_u = textureDimensions(src_tex);
    let src_dims = vec2f(src_dims_u);
    let dst_dims = params.target_size;
    // Use in.uv (top-left convention) to map directly to source pixel space.
    let center_xy = in.uv * src_dims;

  let kw: i32 = {w};
  let kh: i32 = {h};
  let half_w: i32 = kw / 2;
  let half_h: i32 = kh / 2;
  let k = {kernel_arr};

    var sum = vec4f(0.0);
    for (var y: i32 = 0; y < kh; y = y + 1) {{
        for (var x: i32 = 0; x < kw; x = x + 1) {{
            let ix = x - half_w;
            let iy = y - half_h;
            // Offset from integer center.
            let sample_xy = center_xy + vec2f(f32(ix), f32(iy));
            // Sample at integer-coord / src_dims (texel boundary).
            // With a linear sampler this gives a proper 2x2 bilinear average,
            // matching Godot's manual bilinear() at integer coordinates.
            let uv = sample_xy / src_dims;

            let idx: i32 = y * kw + x;
            sum = sum + textureSampleLevel(src_tex, src_samp, uv, 0.0) * k[u32(idx)];
        }}
    }}
    return sum;
  "#
    );

    Ok(build_fullscreen_textured_bundle(body))
}

/// Build a horizontal Gaussian blur shader bundle.
pub fn build_horizontal_blur_bundle(kernel: [f32; 8], offset: [f32; 8]) -> WgslShaderBundle {
    let kernel_wgsl = array8_f32_wgsl(kernel);
    let offset_wgsl = array8_f32_wgsl(offset);
    let body = format!(
        r#"
 let original = vec2f(textureDimensions(src_tex));
 let xy = in.uv * original;
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
}

/// Build a vertical Gaussian blur shader bundle.
pub fn build_vertical_blur_bundle(kernel: [f32; 8], offset: [f32; 8]) -> WgslShaderBundle {
    let kernel_wgsl = array8_f32_wgsl(kernel);
    let offset_wgsl = array8_f32_wgsl(offset);
    let body = format!(
        r#"
 let original = vec2f(textureDimensions(src_tex));
 let xy = in.uv * original;
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
}

/// Build a bilinear upsample shader bundle.
pub fn build_upsample_bilinear_bundle() -> WgslShaderBundle {
    let body = r#"
 return textureSampleLevel(src_tex, src_samp, in.uv, 0.0);
 "#
    .to_string();
    build_fullscreen_textured_bundle(body)
}

/// Build an error shader (purple screen) WGSL source.
pub const ERROR_SHADER_WGSL: &str = r#"
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
fn fs_main() -> @location(0) vec4f {
    // Purple error screen.
    return vec4f(1.0, 0.0, 1.0, 1.0);
}
"#;
