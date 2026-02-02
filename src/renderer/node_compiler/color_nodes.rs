//! Compilers for color manipulation nodes (ColorMix/Blend Color, ColorRamp, HSVAdjust, Luminance).

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use super::super::utils::{fmt_f32, to_vec4_color};
use crate::dsl::{incoming_connection, Node, SceneDSL};

fn parse_json_number_f32(v: &Value) -> Option<f32> {
    v.as_f64()
        .map(|x| x as f32)
        .or_else(|| v.as_i64().map(|x| x as f32))
        .or_else(|| v.as_u64().map(|x| x as f32))
}

fn parse_vec4_like(v: &Value) -> Option<[f32; 4]> {
    if let Some(arr) = v.as_array() {
        let get = |i: usize, default: f32| -> f32 {
            arr.get(i)
                .and_then(parse_json_number_f32)
                .unwrap_or(default)
        };
        return Some([get(0, 0.0), get(1, 0.0), get(2, 0.0), get(3, 1.0)]);
    }

    if let Some(obj) = v.as_object() {
        let get = |key: &str, default: f32| -> f32 {
            obj.get(key)
                .and_then(parse_json_number_f32)
                .unwrap_or(default)
        };

        // Accept both {r,g,b,a} and {x,y,z,w}.
        let has_rgba = obj.contains_key("r") || obj.contains_key("g") || obj.contains_key("b");
        if has_rgba {
            return Some([get("r", 0.0), get("g", 0.0), get("b", 0.0), get("a", 1.0)]);
        }

        let has_xyzw = obj.contains_key("x") || obj.contains_key("y") || obj.contains_key("z");
        if has_xyzw {
            return Some([get("x", 0.0), get("y", 0.0), get("z", 0.0), get("w", 1.0)]);
        }
    }

    None
}

fn vec4_const_premul(rgba_straight: [f32; 4]) -> TypedExpr {
    // Renderer convention: premultiplied alpha.
    let a = rgba_straight[3];
    let r = rgba_straight[0] * a;
    let g = rgba_straight[1] * a;
    let b = rgba_straight[2] * a;
    TypedExpr::new(
        format!(
            "vec4f({}, {}, {}, {})",
            fmt_f32(r),
            fmt_f32(g),
            fmt_f32(b),
            fmt_f32(a)
        ),
        ValueType::Vec4,
    )
}

pub fn compile_luminance<F>(
    scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: F,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    let color_conn = incoming_connection(scene, &node.id, "color")
        .or_else(|| incoming_connection(scene, &node.id, "input"))
        .ok_or_else(|| anyhow!("Luminance missing input color"))?;

    let color = compile_fn(
        &color_conn.from.node_id,
        Some(&color_conn.from.port_id),
        ctx,
        cache,
    )?;

    let color_vec4 = to_vec4_color(color);
    let luma_expr = format!(
        "clamp(dot(({}).rgb, vec3f(0.2126, 0.7152, 0.0722)), 0.0, 1.0)",
        color_vec4.expr
    );

    Ok(TypedExpr::with_time(
        luma_expr,
        ValueType::F32,
        color_vec4.uses_time,
    ))
}

const COLORMIX_WGSL_LIB_KEY: &str = "colormix_blend_lib";

fn ensure_colormix_wgsl_lib(ctx: &mut MaterialCompileContext) {
    if ctx.extra_wgsl_decls.contains_key(COLORMIX_WGSL_LIB_KEY) {
        return;
    }

    // NOTE: These formulas assume premultiplied alpha.
    // Keep this WGSL self-contained (no external uniforms).
    let wgsl = r#"
// ---- ColorMix (Blend Color) helpers (generated) ----

fn blendColorBurnComponent(src: vec2f, dst: vec2f) -> f32 {
    let t = select(0.0, dst.y, dst.y == dst.x);
    let d = select(
        t,
        dst.y - min(dst.y, (dst.y - dst.x) * src.y / (src.x + 0.001)),
        abs(src.x) > 0.0,
    );
    return (d * src.y + src.x * (1.0 - dst.y)) + dst.x * (1.0 - src.y);
}

fn blendColorDodgeComponent(src: vec2f, dst: vec2f) -> f32 {
    let dxScale = select(1.0, 0.0, dst.x == 0.0);
    let delta = dxScale * min(
        dst.y,
        select(dst.y, (dst.x * src.y) / ((src.y - src.x) + 0.001), abs(src.y - src.x) > 0.0),
    );
    return (delta * src.y + src.x * (1.0 - dst.y)) + dst.x * (1.0 - src.y);
}

fn blendOverlayComponent(src: vec2f, dst: vec2f) -> f32 {
    return select(
        src.y * dst.y - (2.0 * (dst.y - dst.x)) * (src.y - src.x),
        (2.0 * src.x) * dst.x,
        2.0 * dst.x <= dst.y,
    );
}

fn blendSoftLightComponent(src: vec2f, dst: vec2f) -> f32 {
    let EPSILON = 0.0;

    if (2.0 * src.x <= src.y) {
        return (((dst.x * dst.x) * (src.y - 2.0 * src.x)) / (dst.y + EPSILON) +
            (1.0 - dst.y) * src.x) +
            dst.x * ((-src.y + 2.0 * src.x) + 1.0);
    } else if (4.0 * dst.x <= dst.y) {
        let dSqd = dst.x * dst.x;
        let dCub = dSqd * dst.x;
        let daSqd = dst.y * dst.y;
        let daCub = daSqd * dst.y;

        return (((daSqd * (src.x - dst.x * ((3.0 * src.y - 6.0 * src.x) - 1.0)) +
            ((12.0 * dst.y) * dSqd) * (src.y - 2.0 * src.x)) -
            (16.0 * dCub) * (src.y - 2.0 * src.x)) -
            daCub * src.x) / (daSqd + EPSILON);
    } else {
        return ((dst.x * ((src.y - 2.0 * src.x) + 1.0) + src.x) -
            sqrt(dst.y * dst.x) * (src.y - 2.0 * src.x)) -
            dst.y * src.x;
    }
}

fn blendColorSaturation(color: vec3f) -> f32 {
    return max(max(color.x, color.y), color.z) - min(min(color.x, color.y), color.z);
}

fn blendHSLColor(flipSat: vec2f, src: vec4f, dst: vec4f) -> vec4f {
    let EPSILON = 0.0;
    let MIN_NORMAL_HALF = 6.10351562e-05;

    let alpha = dst.a * src.a;
    let sda = src.rgb * dst.a;
    let dsa = dst.rgb * src.a;

    let flip_x = flipSat.x != 0.0;
    let flip_y = flipSat.y != 0.0;

    var l = select(sda, dsa, flip_x);
    var r = select(dsa, sda, flip_x);

    if (flip_y) {
        let mn = min(min(l.x, l.y), l.z);
        let mx = max(max(l.x, l.y), l.z);
        l = select(vec3f(0.0), ((l - mn) * blendColorSaturation(r)) / (mx - mn), mx > mn);
        r = dsa;
    }

    let lum = dot(vec3f(0.3, 0.59, 0.11), r);
    var result = (lum - dot(vec3f(0.3, 0.59, 0.11), l)) + l;

    let minComp = min(min(result.x, result.y), result.z);
    let maxComp = max(max(result.x, result.y), result.z);

    if (minComp < 0.0 && lum != minComp) {
        result = lum + (result - lum) * (lum / ((lum - minComp + MIN_NORMAL_HALF) + EPSILON));
    }
    if (maxComp > alpha && maxComp != lum) {
        result = lum + ((result - lum) * (alpha - lum)) / ((maxComp - lum + MIN_NORMAL_HALF) + EPSILON);
    }

    return vec4f(
        ((result + dst.rgb) - dsa + src.rgb) - sda,
        src.a + dst.a - alpha,
    );
}

fn blendNormal(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb * (1.0 - src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendDarken(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - max(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendMultiply(src: vec4f, dst: vec4f) -> vec4f {
    return src * (1.0 - dst.a) + dst * (1.0 - src.a) + src * dst;
}

fn blendPlusDarker(src: vec4f, dst: vec4f) -> vec4f {
    let a = src.a + (1.0 - src.a) * dst.a;
    let color = max(vec3f(0.0), a - (dst.a - dst.rgb) - (src.a - src.rgb));
    return vec4f(color, a);
}

fn blendColorBurn(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        blendColorBurnComponent(src.ra, dst.ra),
        blendColorBurnComponent(src.ga, dst.ga),
        blendColorBurnComponent(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendLighten(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - min(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendScreen(src: vec4f, dst: vec4f) -> vec4f {
    return vec4f(1.0 - (1.0 - src.rgb) * (1.0 - dst.rgb), src.a + dst.a * (1.0 - src.a));
}

fn blendPlusLighter(src: vec4f, dst: vec4f) -> vec4f {
    let color = min(src.rgb + dst.rgb, vec3f(1.0));
    let alpha = src.a + (1.0 - src.a) * dst.a;
    return vec4f(color, alpha);
}

fn blendColorDodge(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        blendColorDodgeComponent(src.ra, dst.ra),
        blendColorDodgeComponent(src.ga, dst.ga),
        blendColorDodgeComponent(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendOverlay(src: vec4f, dst: vec4f) -> vec4f {
    var c = vec3f(
        blendOverlayComponent(src.ra, dst.ra),
        blendOverlayComponent(src.ga, dst.ga),
        blendOverlayComponent(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    c += dst.rgb * (1.0 - src.a) + src.rgb * (1.0 - dst.a);
    return vec4f(c, a);
}

fn blendSoftLight(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        blendSoftLightComponent(src.ra, dst.ra),
        blendSoftLightComponent(src.ga, dst.ga),
        blendSoftLightComponent(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendHardLight(src: vec4f, dst: vec4f) -> vec4f {
    return blendOverlay(dst, src);
}

fn blendDifference(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - 2.0 * min(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendExclusion(src: vec4f, dst: vec4f) -> vec4f {
    let c = (dst.rgb + src.rgb) - (2.0 * dst.rgb * src.rgb);
    let a = src.a + (1.0 - src.a) * dst.a;
    return vec4f(c, a);
}

fn blendHue(src: vec4f, dst: vec4f) -> vec4f {
    return blendHSLColor(vec2f(0.0, 1.0), src, dst);
}

fn blendSaturation(src: vec4f, dst: vec4f) -> vec4f {
    return blendHSLColor(vec2f(1.0), src, dst);
}

fn blendColor(src: vec4f, dst: vec4f) -> vec4f {
    return blendHSLColor(vec2f(0.0), src, dst);
}

fn blendLuminance(src: vec4f, dst: vec4f) -> vec4f {
    return blendHSLColor(vec2f(1.0, 0.0), src, dst);
}
"#;

    ctx.extra_wgsl_decls
        .insert(COLORMIX_WGSL_LIB_KEY.to_string(), wgsl.to_string());
}

/// Compile a ColorMix node.
///
/// NOTE: ColorMix has been repurposed to the editor's "Blend Color" node.
/// It folds N colors left-to-right using a Figma blend mode.
pub fn compile_color_mix<F>(
    scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: F,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    ensure_colormix_wgsl_lib(ctx);

    let mode = node
        .params
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("normal");

    let blend_fn = match mode {
        "normal" => "blendNormal",
        "darken" => "blendDarken",
        "multiply" => "blendMultiply",
        "plus-darker" => "blendPlusDarker",
        "color-burn" => "blendColorBurn",
        "lighten" => "blendLighten",
        "screen" => "blendScreen",
        "plus-lighter" => "blendPlusLighter",
        "color-dodge" => "blendColorDodge",
        "overlay" => "blendOverlay",
        "soft-light" => "blendSoftLight",
        "hard-light" => "blendHardLight",
        "difference" => "blendDifference",
        "exclusion" => "blendExclusion",
        "hue" => "blendHue",
        "saturation" => "blendSaturation",
        "color" => "blendColor",
        "luminosity" => "blendLuminance",
        _ => "blendNormal",
    };

    let mut port_ids: Vec<String> = vec!["color0".to_string(), "color1".to_string()];
    port_ids.extend(node.inputs.iter().map(|p| p.id.clone()));

    let resolve_color = |port_id: &str,
                         ctx: &mut MaterialCompileContext,
                         cache: &mut HashMap<(String, String), TypedExpr>|
     -> Result<TypedExpr> {
        // 1) Connected
        if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
            let v = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
            return Ok(to_vec4_color(v));
        }

        // 2) Inline constant param by portId
        if let Some(v) = node.params.get(port_id).and_then(parse_vec4_like) {
            return Ok(vec4_const_premul(v));
        }

        // 3) Static port defaults
        if port_id == "color0" {
            return Ok(vec4_const_premul([1.0, 0.0, 0.0, 1.0]));
        }
        if port_id == "color1" {
            return Ok(vec4_const_premul([0.0, 0.0, 1.0, 1.0]));
        }

        // 4) Missing dynamic input => vec4(0.0)
        Ok(TypedExpr::new("vec4f(0.0)", ValueType::Vec4))
    };

    let mut colors: Vec<TypedExpr> = Vec::with_capacity(port_ids.len());
    for pid in &port_ids {
        colors.push(resolve_color(pid, ctx, cache)?);
    }

    let mut acc = colors
        .first()
        .cloned()
        .unwrap_or_else(|| TypedExpr::new("vec4f(0.0)", ValueType::Vec4));

    for src in colors.into_iter().skip(1) {
        let uses_time = acc.uses_time || src.uses_time;
        acc = TypedExpr::with_time(
            format!("{}(({}), ({}))", blend_fn, src.expr, acc.expr),
            ValueType::Vec4,
            uses_time,
        );
    }

    Ok(acc)
}

/// Compile a ColorRamp node.
///
/// Maps a scalar value through a color gradient (simplified implementation).
pub fn compile_color_ramp<F>(
    scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: F,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    let fac_conn = incoming_connection(scene, &node.id, "fac")
        .or_else(|| incoming_connection(scene, &node.id, "factor"))
        .or_else(|| incoming_connection(scene, &node.id, "value"))
        .ok_or_else(|| anyhow!("ColorRamp missing input factor"))?;

    let fac = compile_fn(
        &fac_conn.from.node_id,
        Some(&fac_conn.from.port_id),
        ctx,
        cache,
    )?;

    if fac.ty != ValueType::F32 {
        bail!("ColorRamp.factor must be f32, got {:?}", fac.ty);
    }

    // For simplicity, implement a basic grayscale-to-color ramp
    // In a full implementation, this would read gradient stops from params
    // For now, just create a simple gradient from black to white
    Ok(TypedExpr::with_time(
        format!("vec4f({}, {}, {}, 1.0)", fac.expr, fac.expr, fac.expr),
        ValueType::Vec4,
        fac.uses_time,
    ))
}

/// Compile an HSVAdjust node.
///
/// Adjusts the hue, saturation, and value of a color.
pub fn compile_hsv_adjust<F>(
    scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: F,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    let color_conn = incoming_connection(scene, &node.id, "color")
        .or_else(|| incoming_connection(scene, &node.id, "input"))
        .ok_or_else(|| anyhow!("HSVAdjust missing input color"))?;

    let color = compile_fn(
        &color_conn.from.node_id,
        Some(&color_conn.from.port_id),
        ctx,
        cache,
    )?;

    // Convert to vec4 color if needed
    let color_vec4 = to_vec4_color(color);

    // Get adjustment parameters (default to no adjustment)
    let hue = node
        .params
        .get("hue")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as f32;
    let saturation = node
        .params
        .get("saturation")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0) as f32;
    let value = node
        .params
        .get("value")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0) as f32;

    // For simplicity, if no adjustments, just return the color
    if hue == 0.0 && saturation == 1.0 && value == 1.0 {
        return Ok(color_vec4);
    }

    // For now, just implement a simple value (brightness) adjustment
    // A full implementation would convert RGB->HSV, adjust, and convert back
    Ok(TypedExpr::with_time(
        format!("({} * {})", color_vec4.expr, value),
        ValueType::Vec4,
        color_vec4.uses_time,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::super::types::ValueType;
    use super::super::test_utils::test_scene;
    use super::*;
    use crate::dsl::NodePort;

    fn mock_color_compile_fn(
        _node_id: &str,
        _out_port: Option<&str>,
        _ctx: &mut MaterialCompileContext,
        _cache: &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr> {
        Ok(TypedExpr::new(
            "vec4f(1.0, 0.0, 0.0, 1.0)".to_string(),
            ValueType::Vec4,
        ))
    }

    fn mock_f32_compile_fn(
        _node_id: &str,
        _out_port: Option<&str>,
        _ctx: &mut MaterialCompileContext,
        _cache: &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr> {
        Ok(TypedExpr::new("0.5".to_string(), ValueType::F32))
    }

    #[test]
    fn test_compile_luminance() {
        use super::super::test_utils::test_connection;
        let connections = vec![test_connection("color", "value", "lum1", "color")];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "lum1".to_string(),
            node_type: "Luminance".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_luminance(
            &scene,
            &nodes_by_id,
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_color_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert!(result.expr.contains("dot("));
        assert!(result.expr.contains("0.2126"));
        assert!(result.expr.contains("clamp("));
    }

    #[test]
    fn test_color_mix_defaults_fold_and_mode_default() {
        let scene = test_scene(vec![], vec![]);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "mix1".to_string(),
            node_type: "ColorMix".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_color_mix(
            &scene,
            &nodes_by_id,
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_color_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::Vec4);
        assert!(ctx.extra_wgsl_decls.contains_key(COLORMIX_WGSL_LIB_KEY));
        assert!(result.expr.contains("blendNormal("));
        assert!(
            result.expr.contains("vec4f(1, 0, 0, 1")
                || result.expr.contains("vec4f(1.0, 0.0, 0.0, 1.0")
        );
        assert!(
            result.expr.contains("vec4f(0, 0, 1, 1")
                || result.expr.contains("vec4f(0.0, 0.0, 1.0, 1.0")
        );
    }

    #[test]
    fn test_color_mix_dynamic_inputs_respect_node_inputs_order() {
        let scene = test_scene(vec![], vec![]);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "mix1".to_string(),
            node_type: "ColorMix".to_string(),
            params: HashMap::from([
                ("dyn_a".to_string(), serde_json::json!([0.0, 1.0, 0.0, 1.0])),
                ("dyn_b".to_string(), serde_json::json!([1.0, 1.0, 0.0, 1.0])),
            ]),
            inputs: vec![
                NodePort {
                    id: "dyn_a".to_string(),
                    name: Some("color2".to_string()),
                    port_type: Some("color".to_string()),
                },
                NodePort {
                    id: "dyn_b".to_string(),
                    name: Some("color3".to_string()),
                    port_type: Some("color".to_string()),
                },
            ],
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_color_mix(
            &scene,
            &nodes_by_id,
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_color_compile_fn,
        )
        .unwrap();

        assert!(result.expr.contains("blendNormal("));

        // Ordering check: dyn_b should be the outermost src argument, dyn_a should appear inside.
        let idx_b = result
            .expr
            .find("vec4f(1, 1, 0, 1")
            .or_else(|| result.expr.find("vec4f(1.0, 1.0, 0.0, 1.0"))
            .unwrap();
        let idx_a = result
            .expr
            .find("vec4f(0, 1, 0, 1")
            .or_else(|| result.expr.find("vec4f(0.0, 1.0, 0.0, 1.0"))
            .unwrap();
        assert!(idx_b < idx_a);
    }

    #[test]
    fn test_color_mix_mode_dispatch_multiply() {
        let scene = test_scene(vec![], vec![]);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "mix1".to_string(),
            node_type: "ColorMix".to_string(),
            params: HashMap::from([("mode".to_string(), serde_json::json!("multiply"))]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_color_mix(
            &scene,
            &nodes_by_id,
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_color_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::Vec4);
        assert!(result.expr.contains("blendMultiply("));
    }

    #[test]
    fn test_color_ramp() {
        use super::super::test_utils::test_connection;
        let connections = vec![test_connection("factor_node", "value", "ramp1", "factor")];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "ramp1".to_string(),
            node_type: "ColorRamp".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_color_ramp(
            &scene,
            &nodes_by_id,
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_f32_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::Vec4);
        assert!(result.expr.contains("vec4f"));
    }

    #[test]
    fn test_hsv_adjust_no_change() {
        use super::super::test_utils::test_connection;
        let connections = vec![test_connection("color_in", "value", "hsv1", "color")];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "hsv1".to_string(),
            node_type: "HSVAdjust".to_string(),
            params: HashMap::from([
                ("hue".to_string(), serde_json::json!(0.0)),
                ("saturation".to_string(), serde_json::json!(1.0)),
                ("value".to_string(), serde_json::json!(1.0)),
            ]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_hsv_adjust(
            &scene,
            &nodes_by_id,
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_color_compile_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::Vec4);
    }
}
