//! WGSL shader generation module.
//!
//! This module handles:
//! - WGSL shader bundle generation for render passes
//! - Gaussian blur utilities for post-processing effects
//! - Helper functions for formatting WGSL code

use std::collections::HashMap;

use anyhow::{anyhow, bail, Result};

use crate::{
    dsl::{find_node, incoming_connection, parse_f32, Node, SceneDSL},
    renderer::{
        node_compiler::compile_material_expr,
        scene_prep::prepare_scene,
        types::{ValueType, TypedExpr, MaterialCompileContext, WgslShaderBundle},
        utils::to_vec4_color,
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

pub(crate) fn build_fullscreen_textured_bundle(fragment_body: String) -> WgslShaderBundle {
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
        pass_textures: Vec::new(),
    }
}

impl MaterialCompileContext {
    // Extension method for generating WGSL binding declarations
    fn wgsl_decls(&self) -> String {
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

/// Build a WGSL shader bundle for the `image` input of a GuassianBlurPass.
/// This compiles the color expression from the `image` port into a fullscreen shader.
pub fn build_blur_image_wgsl_bundle(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    blur_pass_id: &str,
) -> Result<WgslShaderBundle> {
    let mut material_ctx = MaterialCompileContext::default();
    
    // Get the color expression from the `image` input.
    let fragment_expr: TypedExpr = if let Some(conn) = incoming_connection(scene, blur_pass_id, "image") {
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
        // No image input - return transparent.
        TypedExpr::new("vec4f(0.0, 0.0, 0.0, 0.0)".to_string(), ValueType::Vec4)
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

    // Fullscreen quad vertex shader - directly maps to screen space.
    let vertex_entry = r#"
@vertex
fn vs_main(@location(0) position: vec3f) -> VSOut {
    var out: VSOut;

    // Fullscreen UV: position is already in screen space, convert to [0,1] UV.
    out.uv = (position.xy / params.geo_size) + vec2f(0.5, 0.5);

    // For fullscreen pass, convert to NDC directly.
    let half = params.target_size * 0.5;
    let ndc = vec2f(position.x / half.x, position.y / half.y);
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
    let pass_textures = material_ctx.pass_textures.clone();

    Ok(WgslShaderBundle {
        common,
        vertex,
        fragment,
        compute,
        module,
        image_textures,
        pass_textures,
    })
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
        TypedExpr::new("params.color".to_string(), ValueType::Vec4)
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
    let pass_textures = material_ctx.pass_textures.clone();

    Ok(WgslShaderBundle {
        common,
        vertex,
        fragment,
        compute,
        module,
        image_textures,
        pass_textures,
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
                    let bundle = build_downsample_bundle(*step)?;
                    out.push((
                        format!("{layer_id}__downsample_{step}"),
                        bundle,
                    ));
                }

                out.push((
                    format!("{layer_id}__hblur_ds{downsample_factor}"),
                    build_horizontal_blur_bundle(kernel, offset),
                ));
                out.push((
                    format!("{layer_id}__vblur_ds{downsample_factor}"),
                    build_vertical_blur_bundle(kernel, offset),
                ));
                out.push((
                    format!("{layer_id}__upsample_bilinear_ds{downsample_factor}"),
                    build_upsample_bilinear_bundle(),
                ));
            }
            other => bail!(
                "Composite layer must be RenderPass or GuassianBlurPass, got {other} for {layer_id}"
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
    Ok(build_fullscreen_textured_bundle(body))
}

/// Build a horizontal Gaussian blur shader bundle.
pub fn build_horizontal_blur_bundle(kernel: [f32; 8], offset: [f32; 8]) -> WgslShaderBundle {
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
}

/// Build a vertical Gaussian blur shader bundle.
pub fn build_vertical_blur_bundle(kernel: [f32; 8], offset: [f32; 8]) -> WgslShaderBundle {
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
}

/// Build a bilinear upsample shader bundle.
pub fn build_upsample_bilinear_bundle() -> WgslShaderBundle {
    let body = r#"
let dst_xy = vec2f(in.position.xy);
let dst_resolution = params.target_size;
let uv = dst_xy / dst_resolution;
return textureSampleLevel(src_tex, src_samp, uv, 0.0);
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
fn fs_main(_in: VSOut) -> @location(0) vec4f {
    // Purple error screen.
    return vec4f(1.0, 0.0, 1.0, 1.0);
}
"#;

