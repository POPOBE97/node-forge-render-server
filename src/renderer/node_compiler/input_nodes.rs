//! Compilers for input nodes (ColorInput, FloatInput, IntInput, Vector2Input, Vector3Input, TextureInput).

use anyhow::{bail, Result};
use serde_json::Value;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use crate::dsl::Node;
use crate::renderer::validation::GlslShaderStage;

/// Parse a JSON value as an f32.
fn parse_json_number_f32(v: &Value) -> Option<f32> {
    v.as_f64()
        .map(|x| x as f32)
        .or_else(|| v.as_i64().map(|x| x as f32))
        .or_else(|| v.as_u64().map(|x| x as f32))
}

/// Parse a constant float value from a node's params.
fn parse_const_f32(node: &Node) -> Option<f32> {
    parse_json_number_f32(node.params.get("value")?)
        .or_else(|| parse_json_number_f32(node.params.get("x")?))
        .or_else(|| parse_json_number_f32(node.params.get("v")?))
}

/// Parse a vec4 array from a node's params.
fn parse_vec4_value_array(node: &Node, key: &str) -> Option<[f32; 4]> {
    let arr = node.params.get(key)?.as_array()?;
    let get = |i: usize, default: f32| -> f32 {
        arr.get(i)
            .and_then(parse_json_number_f32)
            .unwrap_or(default)
    };
    Some([get(0, 0.0), get(1, 0.0), get(2, 0.0), get(3, 1.0)])
}

/// Parse a vector from node params using specified keys.
fn parse_const_vec(node: &Node, keys: [&str; 4]) -> Option<[f32; 4]> {
    let x = parse_json_number_f32(node.params.get(keys[0])?)?;
    let y = node
        .params
        .get(keys[1])
        .and_then(parse_json_number_f32)
        .unwrap_or(0.0);
    let z = node
        .params
        .get(keys[2])
        .and_then(parse_json_number_f32)
        .unwrap_or(0.0);
    let w = node
        .params
        .get(keys[3])
        .and_then(parse_json_number_f32)
        .unwrap_or(1.0);
    Some([x, y, z, w])
}

/// Compile a ColorInput node to WGSL.
///
/// ColorInput nodes provide a constant RGBA color value.
///
/// # Parameters
/// - `value`: Array of 4 floats [r, g, b, a], defaults to [1.0, 0.0, 1.0, 1.0]
///
/// # Output
/// - Type: vec4f
/// - Uses time: false
pub fn compile_color_input(node: &Node, _out_port: Option<&str>) -> Result<TypedExpr> {
    let v = parse_vec4_value_array(node, "value").unwrap_or([1.0, 0.0, 1.0, 1.0]);
    // Premultiplied alpha convention: store RGB already multiplied by A.
    let a = v[3];
    Ok(TypedExpr::new(
        format!("vec4f({}, {}, {}, {})", v[0] * a, v[1] * a, v[2] * a, a),
        ValueType::Vec4,
    ))
}

/// Compile a FloatInput or IntInput node to WGSL.
///
/// These nodes provide a constant scalar value.
///
/// # Parameters
/// - `value`: Float or integer value, defaults to 0.0
///
/// # Output
/// - Type: f32
/// - Uses time: false
pub fn compile_float_or_int_input(node: &Node, _out_port: Option<&str>) -> Result<TypedExpr> {
    let v = parse_const_f32(node).unwrap_or(0.0);
    // Ensure WGSL treats this as an abstract-float / f32 literal.
    // `1` is inferred as i32 in WGSL when used in `let` bindings.
    if v.fract() == 0.0 {
        Ok(TypedExpr::new(format!("{v:.1}"), ValueType::F32))
    } else {
        Ok(TypedExpr::new(format!("{v}"), ValueType::F32))
    }
}

/// Compile a Vector2Input node to WGSL.
///
/// Vector2Input nodes provide a constant 2D vector value.
///
/// # Parameters
/// - `x`: X component, defaults to 0.0
/// - `y`: Y component, defaults to 0.0
///
/// # Output
/// - Type: vec2f
/// - Uses time: false
pub fn compile_vector2_input(node: &Node, _out_port: Option<&str>) -> Result<TypedExpr> {
    let v = parse_const_vec(node, ["x", "y", "z", "w"]).unwrap_or([0.0, 0.0, 0.0, 0.0]);
    Ok(TypedExpr::new(
        format!("vec2f({}, {})", v[0], v[1]),
        ValueType::Vec2,
    ))
}

/// Compile a Vector3Input node to WGSL.
///
/// Vector3Input nodes provide a constant 3D vector value.
///
/// # Parameters
/// - `x`: X component, defaults to 0.0
/// - `y`: Y component, defaults to 0.0
/// - `z`: Z component, defaults to 0.0
///
/// # Output
/// - Type: vec3f
/// - Uses time: false
pub fn compile_vector3_input(node: &Node, _out_port: Option<&str>) -> Result<TypedExpr> {
    let v = parse_const_vec(node, ["x", "y", "z", "w"]).unwrap_or([0.0, 0.0, 0.0, 0.0]);
    Ok(TypedExpr::new(
        format!("vec3f({}, {}, {})", v[0], v[1], v[2]),
        ValueType::Vec3,
    ))
}

/// Compile a FragCoord node to WGSL.
///
/// FragCoord provides the fragment's pixel-space coordinate.
///
/// # GLSL
/// `gl_FragCoord.xy`
///
/// # WGSL (this renderer)
/// Our fragment entry uses `VSOut` with `@builtin(position) position: vec4f`.
/// The vertex shader writes clip-space into that builtin, and the GPU provides the
/// corresponding fragment position in render-target pixel space to the fragment shader.
/// So `in.position.xy` is the equivalent of `gl_FragCoord.xy`.
///
/// # Output
/// - Port `xy`: Type vec2f
/// - Uses time: false
pub fn compile_frag_coord(_node: &Node, out_port: Option<&str>) -> Result<TypedExpr> {
    let port = out_port.unwrap_or("xy");
    match port {
        "xy" => Ok(TypedExpr::new(
            "in.frag_coord_gl".to_string(),
            ValueType::Vec2,
        )),
        other => bail!("FragCoord: unsupported output port '{other}'"),
    }
}

/// Compile a GeoFragcoord node to WGSL.
///
/// GeoFragcoord provides the fragment coordinate in the *current geometry's local pixel space*.
/// This matches the editor contract: `xy = in.uv * geometry_size`.
///
/// In our renderer, per-pass `params.geo_size` is the geometry size in pixels.
/// Vertex shader emits `in.uv` in [0,1] over the geometry.
///
/// # Output
/// - Port `xy`: Type vec2f
/// - Uses time: false
pub fn compile_geo_fragcoord(_node: &Node, out_port: Option<&str>) -> Result<TypedExpr> {
    let port = out_port.unwrap_or("xy");
    match port {
        "xy" => Ok(TypedExpr::new("in.local_px".to_string(), ValueType::Vec2)),
        other => bail!("GeoFragcoord: unsupported output port '{other}'"),
    }
}

/// Compile a GeoSize node to WGSL.
///
/// GeoSize provides the current geometry's bounding size in pixels.
///
/// In this renderer, it's passed per-pass via the uniform buffer as `params.geo_size`.
///
/// # Output
/// - Port `xy`: Type vec2f
/// - Uses time: false
pub fn compile_geo_size(_node: &Node, out_port: Option<&str>) -> Result<TypedExpr> {
    compile_geo_size_for_stage(_node, out_port, GlslShaderStage::Vertex)
}

pub fn compile_geo_size_for_stage(
    _node: &Node,
    out_port: Option<&str>,
    stage: GlslShaderStage,
) -> Result<TypedExpr> {
    let port = out_port.unwrap_or("xy");
    match port {
        "xy" => {
            let expr = match stage {
                GlslShaderStage::Fragment => "in.geo_size_px",
                _ => "params.geo_size",
            };
            Ok(TypedExpr::new(expr.to_string(), ValueType::Vec2))
        }
        other => bail!("GeoSize: unsupported output port '{other}'"),
    }
}

/// Compile an Index node to WGSL.
///
/// For instanced geometry, this exposes the per-instance index.
///
/// This must come from the vertex shader builtin `@builtin(instance_index)`.
///
/// Today we only need the index in the vertex shader, but we keep the plumbing flexible
/// so it can be forwarded to fragment later if needed.
///
/// # Output
/// - Port `index`: Type i32
pub fn compile_index(
    _node: &Node,
    out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
) -> Result<TypedExpr> {
    let port = out_port.unwrap_or("index");
    match port {
        "index" => {
            ctx.uses_instance_index = true;
            // WGSL builtin `instance_index` is `u32`. Keep it as `u32` so MathClosure argument
            // types line up when a closure declares `int index`.
            Ok(TypedExpr::new("instance_index", ValueType::U32))
        }
        other => bail!("Index: unsupported output port '{other}'"),
    }
}

#[cfg(test)]
mod fragcoord_tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_fragcoord_xy() {
        let node = Node {
            id: "fc".to_string(),
            node_type: "FragCoord".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let expr = compile_frag_coord(&node, Some("xy")).unwrap();
        assert_eq!(expr.ty, ValueType::Vec2);
        assert_eq!(expr.expr, "in.frag_coord_gl");
        assert!(!expr.uses_time);
    }

    #[test]
    fn test_geo_fragcoord_xy() {
        let node = Node {
            id: "gfc".to_string(),
            node_type: "GeoFragcoord".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let expr = compile_geo_fragcoord(&node, Some("xy")).unwrap();
        assert_eq!(expr.ty, ValueType::Vec2);
        assert_eq!(expr.expr, "in.local_px");
        assert!(!expr.uses_time);
    }

    #[test]
    fn test_geo_size_xy() {
        let node = Node {
            id: "gs".to_string(),
            node_type: "GeoSize".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let expr = compile_geo_size_for_stage(&node, Some("xy"), GlslShaderStage::Vertex).unwrap();
        assert_eq!(expr.ty, ValueType::Vec2);
        assert_eq!(expr.expr, "params.geo_size");
        assert!(!expr.uses_time);
    }

    #[test]
    fn test_geo_size_xy_fragment() {
        let node = Node {
            id: "gs".to_string(),
            node_type: "GeoSize".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };
        let expr =
            compile_geo_size_for_stage(&node, Some("xy"), GlslShaderStage::Fragment).unwrap();
        assert_eq!(expr.ty, ValueType::Vec2);
        assert_eq!(expr.expr, "in.geo_size_px");
        assert!(!expr.uses_time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_color_input_default() {
        let node = Node {
            id: "color1".to_string(),
            node_type: "ColorInput".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_color_input(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec4);
        assert_eq!(result.expr, "vec4f(1, 0, 1, 1)");
        assert!(!result.uses_time);
    }

    #[test]
    fn test_color_input_custom() {
        let node = Node {
            id: "color1".to_string(),
            node_type: "ColorInput".to_string(),
            params: HashMap::from([("value".to_string(), serde_json::json!([0.5, 0.3, 0.8, 1.0]))]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_color_input(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec4);
        assert!(result.expr.contains("0.5"));
        assert!(result.expr.contains("0.3"));
    }

    #[test]
    fn test_float_input() {
        let node = Node {
            id: "float1".to_string(),
            node_type: "FloatInput".to_string(),
            params: HashMap::from([(
                "value".to_string(),
                serde_json::json!(core::f32::consts::PI),
            )]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_float_or_int_input(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, core::f32::consts::PI.to_string());
    }

    #[test]
    fn test_vector2_input() {
        let node = Node {
            id: "vec3_1".to_string(),
            node_type: "Vector3Input".to_string(),
            params: HashMap::from([
                ("x".to_string(), serde_json::json!(1.0)),
                ("y".to_string(), serde_json::json!(2.0)),
                ("z".to_string(), serde_json::json!(3.0)),
            ]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_vector2_input(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec2);
        assert_eq!(result.expr, "vec2f(1, 2)");
    }

    #[test]
    fn test_vector3_input() {
        let node = Node {
            id: "vec3_1".to_string(),
            node_type: "Vector3Input".to_string(),
            params: HashMap::from([
                ("x".to_string(), serde_json::json!(1.0)),
                ("y".to_string(), serde_json::json!(2.0)),
                ("z".to_string(), serde_json::json!(3.0)),
            ]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_vector3_input(&node, None).unwrap();
        assert_eq!(result.ty, ValueType::Vec3);
        assert_eq!(result.expr, "vec3f(1, 2, 3)");
    }
}
