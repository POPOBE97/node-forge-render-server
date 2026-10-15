//! ChromaticBlur's radial → RGB → masked Gaussian pipeline.
//! Controls compile through ordinary graph expressions, preserving declaration identity.

use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use super::{
    node_compiler::{
        compile_material_expr,
        shader_material::{SYSTEM_DECL, SYSTEM_DECL_KEY},
    },
    types::{GraphBindingKind, MaterialCompileContext, TypedExpr, ValueType, WgslShaderBundle},
    utils::coerce_to_type,
    wgsl::{graph_inputs_wgsl_decl, merge_graph_input_kinds},
    wgsl_gradient_blur::{FULLSCREEN_VERTEX, PARAMS_AND_VSOUT},
};
use crate::{
    dsl::{Node, SceneDSL, incoming_connection},
    schema,
};

const MASK_HELPER: &str = r#"
fn cb_mask(uv: vec2f, center: vec2f, size: vec2f, mask: vec4f) -> f32 {
    let distance = length((uv - center) * size);
    let outer = max(mask.x, 1.0);
    let inner = outer * (1.0 - clamp(mask.y, 0.001, 1.0));
    let outside = smoothstep(inner, outer, distance);
    return pow(select(1.0 - outside, outside, mask.w > 0.5), max(mask.z, 0.01));
}
"#;

const SAMPLE_HELPER: &str = r#"
fn cb_read(image: texture_2d<f32>, image_sampler: sampler, uv: vec2f) -> vec4f {
    let value = sample_texture_local_uv(image, image_sampler, uv);
    return select(value, vec4f(0.0), any(uv < vec2f(0.0)) || any(uv > vec2f(1.0)));
}

fn cb_radial(image: texture_2d<f32>, image_sampler: sampler, uv: vec2f,
    center: vec2f, amount: f32, samples: i32) -> vec4f {
    let count = u32(clamp(samples, 2, 48));
    let ray = uv - center;
    var result = vec4f(0.0);
    for (var i = 0u; i < count; i++) {
        let t = f32(i) / f32(count - 1u) - 0.5;
        result += cb_read(image, image_sampler, uv + ray * t * amount);
    }
    return result / f32(count);
}

fn cb_rgb(image: texture_2d<f32>, image_sampler: sampler, uv: vec2f,
    center: vec2f, size: vec2f, amount: f32, offset: f32, spread: f32,
    samples: i32, channels: vec3f, fixed_direction: bool, angle_degrees: f32) -> vec4f {
    let delta = (uv - center) * size;
    let angle = angle_degrees * 0.017453292519943295;
    let direction = select(delta / max(length(delta), 0.0001), vec2f(cos(angle), sin(angle)), fixed_direction);
    let step = direction * amount / size;
    let count = u32(clamp(samples, 1, 16));
    var result = vec3f(0.0);
    var weights = 0.0;
    for (var i = 0u; i < count; i++) {
        let t = select(f32(i) / f32(max(count - 1u, 1u)) * 2.0 - 1.0, 0.0, count == 1u);
        let weight = exp(-2.0 * t * t);
        result += vec3f(
            cb_read(image, image_sampler, uv + step * (offset * channels.x + t * spread)).r,
            cb_read(image, image_sampler, uv + step * (offset * channels.y + t * spread)).g,
            cb_read(image, image_sampler, uv + step * (offset * channels.z + t * spread)).b
        ) * weight;
        weights += weight;
    }
    return vec4f(result / weights, cb_read(image, image_sampler, uv).a);
}
"#;

#[derive(Clone, Copy, Debug)]
pub enum ChromaticStage {
    Radial,
    Rgb,
}

impl ChromaticStage {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Radial => "radial",
            Self::Rgb => "rgb",
        }
    }
}

fn input(
    scene: &SceneDSL,
    nodes: &HashMap<String, Node>,
    node: &Node,
    port: &str,
    ty: ValueType,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<String> {
    if let Some(edge) = incoming_connection(scene, &node.id, port) {
        let value = compile_material_expr(
            scene,
            nodes,
            &edge.from.node_id,
            Some(&edge.from.port_id),
            ctx,
            cache,
        )?;
        return Ok(coerce_to_type(value, ty)?.expr);
    }
    let scheme;
    let value = if let Some(value) = node.params.get(port) {
        value
    } else {
        scheme = schema::load_default_scheme()?;
        schema::input_port_default_value(&scheme, "ChromaticBlur", port)
            .ok_or_else(|| anyhow!("ChromaticBlur {}: missing {port}", node.id))?
    };
    let invalid = || anyhow!("ChromaticBlur {}: invalid {port}: {value}", node.id);
    let number = |v: &serde_json::Value| -> Result<String> {
        let n = v.as_f64().filter(|v| v.is_finite()).ok_or_else(invalid)? as f32;
        if !n.is_finite() {
            return Err(invalid());
        }
        Ok(format!("{n:?}"))
    };
    Ok(match ty {
        ValueType::Bool => value.as_bool().ok_or_else(invalid)?.to_string(),
        ValueType::I32 => value
            .as_i64()
            .and_then(|v| i32::try_from(v).ok())
            .ok_or_else(invalid)?
            .to_string(),
        ValueType::F32 => number(value)?,
        ValueType::Vec2 | ValueType::Vec3 | ValueType::Vec4 => {
            let count = match ty {
                ValueType::Vec2 => 2,
                ValueType::Vec3 => 3,
                _ => 4,
            };
            let fields = ["x", "y", "z", "w"];
            let values = (0..count)
                .map(|i| {
                    value
                        .get(i)
                        .or_else(|| value.get(fields[i]))
                        .ok_or_else(invalid)
                        .and_then(number)
                })
                .collect::<Result<Vec<_>>>()?;
            format!("vec{count}f({})", values.join(", "))
        }
        _ => bail!("unsupported ChromaticBlur input type: {ty:?}"),
    })
}

pub(crate) fn compile_sigma(
    scene: &SceneDSL,
    nodes: &HashMap<String, Node>,
    node: &Node,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    ctx.extra_wgsl_decls
        .insert("chromatic.mask".into(), MASK_HELPER.into());
    let center = input(scene, nodes, node, "center", ValueType::Vec2, ctx, cache)?;
    let mask = input(
        scene,
        nodes,
        node,
        "gaussianMask",
        ValueType::Vec4,
        ctx,
        cache,
    )?;
    let sigma = input(
        scene,
        nodes,
        node,
        "gaussianSigma",
        ValueType::F32,
        ctx,
        cache,
    )?;
    let enabled = input(
        scene,
        nodes,
        node,
        "gaussianEnabled",
        ValueType::Bool,
        ctx,
        cache,
    )?;
    Ok(TypedExpr::new(
        format!(
            "select(0.0, max({sigma}, 0.0) * cb_mask(in.local_px.xy / params.geo_size, {center}, params.geo_size, {mask}), {enabled})"
        ),
        ValueType::F32,
    ))
}

pub fn build_chromatic_stage_bundle(
    scene: &SceneDSL,
    nodes: &HashMap<String, Node>,
    node_id: &str,
    source_id: &str,
    stage: ChromaticStage,
    forced_kind: Option<GraphBindingKind>,
) -> Result<WgslShaderBundle> {
    let node = nodes
        .get(node_id)
        .ok_or_else(|| anyhow!("missing ChromaticBlur {node_id}"))?;
    let mut ctx = MaterialCompileContext::default();
    let mut cache = HashMap::new();
    ctx.register_pass_texture(source_id);
    ctx.extra_wgsl_decls
        .insert(SYSTEM_DECL_KEY.into(), SYSTEM_DECL.into());
    ctx.extra_wgsl_decls
        .insert("chromatic.mask".into(), MASK_HELPER.into());
    ctx.extra_wgsl_decls
        .insert("chromatic.sample".into(), SAMPLE_HELPER.into());
    let mut get = |port: &str, ty| input(scene, nodes, node, port, ty, &mut ctx, &mut cache);
    let center = get("center", ValueType::Vec2)?;
    let prefix = stage.name();
    let mask = get(&format!("{prefix}Mask"), ValueType::Vec4)?;
    let enabled = get(&format!("{prefix}Enabled"), ValueType::Bool)?;
    let samples = get(&format!("{prefix}Samples"), ValueType::I32)?;
    // Same renderer-owned RasterUV → LocalUV boundary as ShaderMaterialInput.
    // Preserve interpolated UV precision instead of dividing interpolated pixel positions.
    let uv = "vec2f(in.uv.x, 1.0 - in.uv.y)";
    let amount =
        format!("select(0.0, cb_mask({uv}, {center}, params.geo_size, {mask}), {enabled})");
    let tex = MaterialCompileContext::pass_tex_var_name(source_id);
    let sampler = MaterialCompileContext::pass_sampler_var_name(source_id);
    let expression = match stage {
        ChromaticStage::Radial => {
            let strength = get("radialStrength", ValueType::F32)?;
            format!("cb_radial({tex}, {sampler}, {uv}, {center}, {amount} * {strength}, {samples})")
        }
        ChromaticStage::Rgb => {
            let offset = get("rgbOffset", ValueType::F32)?;
            let spread = get("rgbSpread", ValueType::F32)?;
            let channels = get("rgbChannels", ValueType::Vec3)?;
            let fixed = get("rgbFixedDirection", ValueType::Bool)?;
            let angle = get("rgbAngle", ValueType::F32)?;
            format!(
                "cb_rgb({tex}, {sampler}, {uv}, {center}, params.geo_size, {amount}, {offset}, max({spread}, 0.0), {samples}, {channels}, {fixed}, {angle})"
            )
        }
    };
    let graph_schema = merge_graph_input_kinds(&ctx, &Default::default());
    let graph_binding_kind = graph_schema
        .as_ref()
        .map(|_| forced_kind.unwrap_or(GraphBindingKind::Uniform));
    let mut common = PARAMS_AND_VSOUT.to_string();
    if let (Some(schema), Some(kind)) = (&graph_schema, graph_binding_kind) {
        common.push_str(&graph_inputs_wgsl_decl(schema, kind));
    }
    common.push_str(&ctx.wgsl_decls());
    let body = ctx.build_fragment_body(&expression);
    let fragment =
        format!("@fragment\nfn fs_main(in: VSOut) -> @location(0) vec4f {{\n{body}\n}}\n");
    Ok(WgslShaderBundle {
        vertex: format!("{common}{FULLSCREEN_VERTEX}"),
        fragment: format!("{common}{fragment}"),
        module: format!("{common}{FULLSCREEN_VERTEX}{fragment}"),
        common,
        compute: None,
        image_textures: ctx.image_textures,
        pass_textures: ctx.pass_textures,
        graph_schema,
        graph_binding_kind,
        shader_parameter_schema: None,
    })
}
