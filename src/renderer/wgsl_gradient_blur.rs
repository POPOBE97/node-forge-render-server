//! WGSL shader generation for gradient blur.
//!
//! The GradientBlur node is a multi-pass effect that:
//! 1. Renders the source material into an intermediate texture (source pass).
//! 2. Pads/centers the source in an enlarged texture with mirror-repeat edges (pad pass).
//! 3. Creates a 6-level mip chain via cross-box 3×3 downsampling (mip passes).
//! 4. Composites the final result using a mask-driven mip-level selection with
//!    Mitchell-Netravali bicubic (B-spline) reconstruction (composite pass).
//!
//! The mask material expression outputs a blur sigma in pixels. Internally the node
//! converts sigma → mip level: `clamp(log2(sigma * 1.333333), 0.0, 6.0)`.

use std::collections::HashMap;

use anyhow::{Result, bail};

use crate::{
    dsl::{Node, SceneDSL, incoming_connection},
    renderer::{
        node_compiler::compile_material_expr,
        types::{GraphBindingKind, MaterialCompileContext, TypedExpr, ValueType, WgslShaderBundle},
        utils::{coerce_to_type, to_vec4_color},
        wgsl::{build_fullscreen_textured_bundle, graph_inputs_wgsl_decl, merge_graph_input_kinds},
    },
};

/// Number of mip levels including level 0 (original/padded resolution).
pub const GB_MIP_LEVELS: u32 = 7;

/// Compute enlarged (padded) texture size from source dimensions.
///
/// Formula: `ceil((w + 64) / 64) * 64` — ensures at least 64px total padding.
pub fn gradient_blur_padded_size(w: u32, h: u32) -> [u32; 2] {
    let pad_w = ((w as u64 + 64 + 63) / 64 * 64) as u32;
    let pad_h = ((h as u64 + 64 + 63) / 64 * 64) as u32;
    [pad_w, pad_h]
}

// ---------------------------------------------------------------------------
// Source pass — renders the upstream material expression into a texture.
// ---------------------------------------------------------------------------

/// Build WGSL for the GradientBlur source pass.
///
/// This is analogous to `build_blur_image_wgsl_bundle` in wgsl.rs, but reads
/// the `"source"` input instead of `"pass"`.
pub fn build_gradient_blur_source_wgsl_bundle(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    gb_node_id: &str,
) -> Result<WgslShaderBundle> {
    build_gradient_blur_source_wgsl_bundle_with_graph_binding(scene, nodes_by_id, gb_node_id, None)
}

pub fn build_gradient_blur_source_wgsl_bundle_with_graph_binding(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    gb_node_id: &str,
    forced_graph_binding_kind: Option<GraphBindingKind>,
) -> Result<WgslShaderBundle> {
    let Some(conn) = incoming_connection(scene, gb_node_id, "source") else {
        return Ok(build_fullscreen_textured_bundle(
            "return vec4f(0.0, 0.0, 0.0, 0.0);".to_string(),
        ));
    };

    let source_is_pass = nodes_by_id.get(&conn.from.node_id).is_some_and(|node| {
        matches!(
            node.node_type.as_str(),
            "RenderPass"
                | "GuassianBlurPass"
                | "Downsample"
                | "Upsample"
                | "GradientBlur"
                | "Composite"
        )
    });

    if source_is_pass {
        let mut bundle =
            crate::renderer::wgsl_templates::fullscreen::build_fullscreen_sampled_bundle();
        bundle.pass_textures = vec![conn.from.node_id.clone()];
        return Ok(bundle);
    }

    // Non-pass source: compile the connected material expression.
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

    let mut common = PARAMS_AND_VSOUT.to_string();

    if let (Some(schema), Some(kind)) = (graph_schema.as_ref(), graph_binding_kind) {
        common.push_str(&graph_inputs_wgsl_decl(schema, kind));
    }
    common.push_str(&material_ctx.wgsl_decls());

    let vertex = FULLSCREEN_VERTEX;
    let fragment = format!(
        r#"
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {{
{fragment_body}
}}
"#
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

// ---------------------------------------------------------------------------
// Pad pass — renders source into enlarged texture with mirror-repeat edges.
// ---------------------------------------------------------------------------

/// Build the pad pass WGSL bundle.
///
/// This pass renders a fullscreen quad at the enlarged resolution. It samples
/// the upstream source texture with UV remapping and a mirror-repeat sampler
/// so that the source appears centered in the larger canvas.
pub fn build_gradient_blur_pad_wgsl_bundle(
    src_w: f32,
    src_h: f32,
    padded_w: f32,
    padded_h: f32,
) -> WgslShaderBundle {
    let offset_x = (padded_w - src_w) * 0.5;
    let offset_y = (padded_h - src_h) * 0.5;
    // Fragment: remap UV from padded space to source space.
    // The hardware mirror-repeat sampler handles out-of-[0,1] UVs.
    let body = format!(
        r#"let padded_size = vec2f({padded_w:.1}, {padded_h:.1});
    let src_size = vec2f({src_w:.1}, {src_h:.1});
    let offset = vec2f({offset_x:.1}, {offset_y:.1});
    let src_coord = in.uv * padded_size - offset;
    let src_uv = src_coord / src_size;
    return textureSampleLevel(src_tex, src_samp, src_uv, 0.0);"#,
    );
    build_fullscreen_textured_bundle(body)
}

// ---------------------------------------------------------------------------
// Downsample pass — uses the standard `build_downsample_pass_wgsl_bundle`
// with a cross-box 3×3 kernel, matching the existing `Downsample` node.
// ---------------------------------------------------------------------------

/// Cross-box 3×3 kernel for GradientBlur mip chain downsampling.
///
/// This is a 3×3 cross pattern (4 directional samples, each weighted 0.25)
/// matching the Godot `downsample.gdshader`.
pub fn gradient_blur_cross_kernel() -> crate::renderer::types::Kernel2D {
    crate::renderer::types::Kernel2D {
        width: 3,
        height: 3,
        values: vec![0.0, 0.25, 0.0, 0.25, 0.0, 0.25, 0.0, 0.25, 0.0],
    }
}

// ---------------------------------------------------------------------------
// Composite pass — mask-driven mip-level blending with bicubic reconstruction.
// ---------------------------------------------------------------------------

/// Build the final composite WGSL bundle for gradient blur.
///
/// This shader evaluates the mask material expression (sigma in pixels),
/// converts to mip level, and blends adjacent mip levels using Mitchell-Netravali
/// bicubic B-spline reconstruction.
///
/// `mip_pass_ids`: ordered list of mip pass IDs from level 0..6.
/// `padded_size`: enlarged texture size [w, h] (mip 0 resolution).
/// `padding_offset`: pixel offset of the source origin within the padded texture [ox, oy].
pub fn build_gradient_blur_composite_wgsl_bundle(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    gb_node_id: &str,
    mip_pass_ids: &[String],
    padded_size: [f32; 2],
    padding_offset: [f32; 2],
) -> Result<WgslShaderBundle> {
    build_gradient_blur_composite_wgsl_bundle_with_graph_binding(
        scene,
        nodes_by_id,
        gb_node_id,
        mip_pass_ids,
        padded_size,
        padding_offset,
        None,
    )
}

pub fn build_gradient_blur_composite_wgsl_bundle_with_graph_binding(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    gb_node_id: &str,
    mip_pass_ids: &[String],
    padded_size: [f32; 2],
    padding_offset: [f32; 2],
    forced_graph_binding_kind: Option<GraphBindingKind>,
) -> Result<WgslShaderBundle> {
    assert_eq!(mip_pass_ids.len(), GB_MIP_LEVELS as usize);

    // --- 1. Compile the mask expression ----------------------------------
    let mask_conn = incoming_connection(scene, gb_node_id, "mask").ok_or_else(|| {
        anyhow::anyhow!("GradientBlur {gb_node_id}: 'mask' input is not connected")
    })?;

    let mask_upstream = nodes_by_id.get(&mask_conn.from.node_id).ok_or_else(|| {
        anyhow::anyhow!(
            "GradientBlur: mask upstream node not found: {}",
            mask_conn.from.node_id
        )
    })?;

    // Validate mask is a material expression, not a pass.
    if matches!(
        mask_upstream.node_type.as_str(),
        "RenderPass"
            | "GuassianBlurPass"
            | "Downsample"
            | "Upsample"
            | "GradientBlur"
            | "Composite"
    ) {
        bail!(
            "GradientBlur: unsupported capability — mask must be a material expression, got pass node {} ({})",
            mask_conn.from.node_id,
            mask_upstream.node_type
        );
    }

    let mut material_ctx = MaterialCompileContext::default();
    let mut cache: HashMap<(String, String), TypedExpr> = HashMap::new();
    let mask_expr = compile_material_expr(
        scene,
        nodes_by_id,
        &mask_conn.from.node_id,
        Some(&mask_conn.from.port_id),
        &mut material_ctx,
        &mut cache,
    )?;

    // Coerce mask to f32 (clamp to safe range in WGSL, not here).
    let mask_f32 = coerce_to_type(mask_expr, ValueType::F32)?;

    // --- 2. Register mip pass textures -----------------------------------
    for mip_id in mip_pass_ids {
        material_ctx.register_pass_texture(mip_id);
    }

    // --- 3. Build graph schema -------------------------------------------
    let graph_schema = merge_graph_input_kinds(&material_ctx, &std::collections::BTreeMap::new());
    let graph_binding_kind = graph_schema
        .as_ref()
        .map(|_| forced_graph_binding_kind.unwrap_or(GraphBindingKind::Uniform));

    // --- 4. Build WGSL ---------------------------------------------------
    let mut common = PARAMS_AND_VSOUT.to_string();

    if let (Some(schema), Some(kind)) = (graph_schema.as_ref(), graph_binding_kind) {
        common.push_str(&graph_inputs_wgsl_decl(schema, kind));
    }
    common.push_str(&material_ctx.wgsl_decls());

    // Helper functions: clamp_to_edge, bilinear per mip level, mvb_up, sample_from_mipmap
    common.push_str(&build_composite_helpers(mip_pass_ids, padded_size));

    let vertex = FULLSCREEN_VERTEX;

    // Build the mask evaluation + mip blending fragment body.
    let mask_stmts = if material_ctx.inline_stmts.is_empty() {
        String::new()
    } else {
        format!("{}\n", material_ctx.inline_stmts.join("\n"))
    };

    let fragment = format!(
        r#"
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {{
    {mask_stmts}// Evaluate mask → sigma in pixels.
    // NOTE: The mask expression sees user coordinates (in.local_px),
    // i.e. (0,0) = bottom-left of the original source image.
    let gb_sigma = max({mask_expr}, 0.0);

    // Sigma → mip level (clamped to safe range).
    var gb_m: f32 = 0.0;
    if (gb_sigma > 0.0) {{
        gb_m = clamp(log2(gb_sigma * 1.333333), 0.0, 6.0);
    }}

    let gb_mip0_size = vec2f({mip0_w:.1}, {mip0_h:.1});

    // Transform from user coordinates (original image space) to padded
    // texture coordinates.  User (0,0) → padded (pad_offset).
    let gb_pad_offset = vec2f({pad_ox:.1}, {pad_oy:.1});
    let gb_coord = in.local_px.xy + gb_pad_offset;

    // Floor / ceil mip levels.
    let gb_mLo = floor(gb_m);
    var gb_cLo: vec4f;

    if (gb_mLo < 0.1) {{
        gb_cLo = gb_sample_from_mipmap(gb_coord, gb_mip0_size, 0);
    }} else {{
        let gb_scale_lo = 1.0 / pow(2.0, gb_mLo);
        let gb_lo_res = gb_mip0_size / pow(2.0, gb_mLo);
        let gb_w_lo = gb_mvb_up(gb_coord, gb_scale_lo);
        gb_cLo = gb_w_lo[0].x * gb_w_lo[0].y * gb_sample_from_mipmap(vec2f(gb_w_lo[2].x, gb_w_lo[2].y), gb_lo_res, i32(gb_mLo))
                + gb_w_lo[1].x * gb_w_lo[0].y * gb_sample_from_mipmap(vec2f(gb_w_lo[3].x, gb_w_lo[2].y), gb_lo_res, i32(gb_mLo))
                + gb_w_lo[0].x * gb_w_lo[1].y * gb_sample_from_mipmap(vec2f(gb_w_lo[2].x, gb_w_lo[3].y), gb_lo_res, i32(gb_mLo))
                + gb_w_lo[1].x * gb_w_lo[1].y * gb_sample_from_mipmap(vec2f(gb_w_lo[3].x, gb_w_lo[3].y), gb_lo_res, i32(gb_mLo));
    }}

    let gb_mHi = gb_mLo + 1.0;
    let gb_scale_hi = 1.0 / pow(2.0, gb_mHi);
    let gb_hi_res = gb_mip0_size / pow(2.0, gb_mHi);
    let gb_w_hi = gb_mvb_up(gb_coord, gb_scale_hi);
    let gb_cHi = gb_w_hi[0].x * gb_w_hi[0].y * gb_sample_from_mipmap(vec2f(gb_w_hi[2].x, gb_w_hi[2].y), gb_hi_res, i32(gb_mHi))
               + gb_w_hi[1].x * gb_w_hi[0].y * gb_sample_from_mipmap(vec2f(gb_w_hi[3].x, gb_w_hi[2].y), gb_hi_res, i32(gb_mHi))
               + gb_w_hi[0].x * gb_w_hi[1].y * gb_sample_from_mipmap(vec2f(gb_w_hi[2].x, gb_w_hi[3].y), gb_hi_res, i32(gb_mHi))
               + gb_w_hi[1].x * gb_w_hi[1].y * gb_sample_from_mipmap(vec2f(gb_w_hi[3].x, gb_w_hi[3].y), gb_hi_res, i32(gb_mHi));

    return mix(gb_cLo, gb_cHi, gb_m - gb_mLo);
}}
"#,
        mask_expr = mask_f32.expr,
        mip0_w = padded_size[0],
        mip0_h = padded_size[1],
        pad_ox = padding_offset[0],
        pad_oy = padding_offset[1],
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

// ---------------------------------------------------------------------------
// WGSL helper generation for the composite pass.
// ---------------------------------------------------------------------------

/// Build the WGSL helper functions for the composite pass:
/// - `gb_mvb_up` (Mitchell-Netravali bicubic upsampling weights)
/// - `gb_sample_from_mipmap` (dispatches to the correct mip level via hardware bilinear)
fn build_composite_helpers(mip_pass_ids: &[String], _padded_size: [f32; 2]) -> String {
    let mut out = String::new();

    out.push_str(
        r#"
// --- GradientBlur composite helpers (generated) ---

"#,
    );

    // Mitchell-Netravali bicubic B-spline weights (matches Godot mvbUp).
    out.push_str(
        r#"fn gb_mvb_up(dc: vec2f, scale: f32) -> array<vec2f, 4> {
    let d     = dc * scale - 0.5;
    let c     = floor(d);
    let x     = c - d + 1.0;
    let X     = d - c;
    let x3    = x * x * x;
    let coeff = 0.5 * x * x + 0.5 * x + 0.166667;
    let w1    = -0.333333 * x3 + coeff;
    let w2    = 1.0 - w1;
    let o1    = (-0.5 * x3 + coeff) / w1 + c - 0.5;
    let o2    = (X * X * X / 6.0) / w2 + c + 1.5;
    return array<vec2f, 4>(w1, w2, o1, o2);
}

"#,
    );

    // sample_from_mipmap dispatcher — hardware-bilinear sampling with Y-flip.
    // Input `xy` is in GLSL-like bottom-left origin (from in.local_px + pad_offset).
    // Texture UV is top-left origin, so flip Y: uv.y = 1.0 - xy.y / resolution.y.
    out.push_str("fn gb_sample_from_mipmap(xy: vec2f, resolution: vec2f, level: i32) -> vec4f {\n");
    out.push_str("    let uv = vec2f(xy.x, resolution.y - xy.y) / resolution;\n");
    for (i, mip_id) in mip_pass_ids.iter().enumerate() {
        let tex_var = MaterialCompileContext::pass_tex_var_name(mip_id);
        let samp_var = MaterialCompileContext::pass_sampler_var_name(mip_id);
        if i == 0 {
            out.push_str(&format!("    if (level == {i}) {{\n"));
        } else {
            out.push_str(&format!("    }} else if (level == {i}) {{\n"));
        }
        out.push_str(&format!(
            "        return textureSampleLevel({tex_var}, {samp_var}, uv, 0.0);\n"
        ));
    }
    out.push_str("    }\n    return vec4f(0.0);\n}\n\n");

    out
}

// ---------------------------------------------------------------------------
// Shared WGSL snippets.
// ---------------------------------------------------------------------------

const PARAMS_AND_VSOUT: &str = r#"
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

"#;

const FULLSCREEN_VERTEX: &str = r#"
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
