//! Compilers for color manipulation nodes (ColorMix, ColorRamp, HSVAdjust).

use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use super::super::utils::to_vec4_color;
use crate::dsl::{Node, SceneDSL, incoming_connection};

/// Compile a ColorMix node.
///
/// Mixes two colors based on a factor.
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
    let a_conn = incoming_connection(scene, &node.id, "a")
        .or_else(|| incoming_connection(scene, &node.id, "color1"))
        .ok_or_else(|| anyhow!("ColorMix missing input a"))?;
    let b_conn = incoming_connection(scene, &node.id, "b")
        .or_else(|| incoming_connection(scene, &node.id, "color2"))
        .ok_or_else(|| anyhow!("ColorMix missing input b"))?;
    let fac_conn = incoming_connection(scene, &node.id, "fac")
        .or_else(|| incoming_connection(scene, &node.id, "factor"))
        .ok_or_else(|| anyhow!("ColorMix missing input factor"))?;

    let a = compile_fn(&a_conn.from.node_id, Some(&a_conn.from.port_id), ctx, cache)?;
    let b = compile_fn(&b_conn.from.node_id, Some(&b_conn.from.port_id), ctx, cache)?;
    let fac = compile_fn(
        &fac_conn.from.node_id,
        Some(&fac_conn.from.port_id),
        ctx,
        cache,
    )?;

    // Ensure factor is f32
    if fac.ty != ValueType::F32 {
        bail!("ColorMix.factor must be f32, got {:?}", fac.ty);
    }

    // Convert inputs to vec4 color format
    let aa = to_vec4_color(a);
    let bb = to_vec4_color(b);

    // WGSL allows vec4f(f32) splat constructors for the factor
    let factor_vec4 = TypedExpr::with_time(
        format!("vec4f({})", fac.expr),
        ValueType::Vec4,
        fac.uses_time,
    );

    Ok(TypedExpr::with_time(
        format!("mix({}, {}, {})", aa.expr, bb.expr, factor_vec4.expr),
        ValueType::Vec4,
        aa.uses_time || bb.uses_time || fac.uses_time,
    ))
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
    fn test_color_mix() {
        use super::super::test_utils::test_connection;
        let connections = vec![
            test_connection("color1", "value", "mix1", "a"),
            test_connection("color2", "value", "mix1", "b"),
            test_connection("factor_node", "value", "mix1", "factor"),
        ];
        let scene = test_scene(vec![], connections);
        let nodes_by_id = HashMap::new();
        let node = Node {
            id: "mix1".to_string(),
            node_type: "ColorMix".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
        };
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        // Mock that returns color for color nodes and f32 for factor node
        let mock_fn = |node_id: &str,
                       _out_port: Option<&str>,
                       _ctx: &mut MaterialCompileContext,
                       _cache: &mut HashMap<(String, String), TypedExpr>|
         -> Result<TypedExpr> {
            if node_id == "factor_node" {
                Ok(TypedExpr::new("0.5".to_string(), ValueType::F32))
            } else {
                Ok(TypedExpr::new(
                    "vec4f(1.0, 0.0, 0.0, 1.0)".to_string(),
                    ValueType::Vec4,
                ))
            }
        };

        let result = compile_color_mix(
            &scene,
            &nodes_by_id,
            &node,
            None,
            &mut ctx,
            &mut cache,
            mock_fn,
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::Vec4);
        assert!(result.expr.contains("mix("));
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
