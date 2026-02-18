//! Compilers for input nodes (BoolInput, ColorInput, FloatInput, IntInput, Vector2Input, Vector3Input, TextureInput, TimeInput).

use anyhow::{Result, bail};

use super::super::types::{GraphFieldKind, MaterialCompileContext, TypedExpr, ValueType};
use crate::dsl::Node;
use crate::renderer::graph_uniforms::graph_field_name;
use crate::renderer::validation::GlslShaderStage;

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
pub fn compile_color_input(
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
) -> Result<TypedExpr> {
    ctx.register_graph_input(&node.id, GraphFieldKind::Vec4Color);
    let field = graph_field_name(&node.id);
    Ok(TypedExpr::new(
        format!(
            "vec4f((graph_inputs.{field}).rgb * (graph_inputs.{field}).a, (graph_inputs.{field}).a)"
        ),
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
pub fn compile_float_or_int_input(
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
) -> Result<TypedExpr> {
    let field = graph_field_name(&node.id);
    if node.node_type == "IntInput" {
        ctx.register_graph_input(&node.id, GraphFieldKind::I32);
        Ok(TypedExpr::new(
            format!("f32((graph_inputs.{field}).x)"),
            ValueType::F32,
        ))
    } else {
        ctx.register_graph_input(&node.id, GraphFieldKind::F32);
        Ok(TypedExpr::new(
            format!("(graph_inputs.{field}).x"),
            ValueType::F32,
        ))
    }
}

/// Compile a BoolInput node to WGSL.
///
/// BoolInput nodes provide a constant boolean value.
///
/// # Parameters
/// - `value`: Boolean value, defaults to false
///
/// # Output
/// - Type: bool
/// - Uses time: false
pub fn compile_bool_input(
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
) -> Result<TypedExpr> {
    ctx.register_graph_input(&node.id, GraphFieldKind::Bool);
    let field = graph_field_name(&node.id);
    Ok(TypedExpr::new(
        format!("((graph_inputs.{field}).x != 0)"),
        ValueType::Bool,
    ))
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
pub fn compile_vector2_input(
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
) -> Result<TypedExpr> {
    ctx.register_graph_input(&node.id, GraphFieldKind::Vec2);
    let field = graph_field_name(&node.id);
    Ok(TypedExpr::new(
        format!("(graph_inputs.{field}).xy"),
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
pub fn compile_vector3_input(
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
) -> Result<TypedExpr> {
    ctx.register_graph_input(&node.id, GraphFieldKind::Vec3);
    let field = graph_field_name(&node.id);
    Ok(TypedExpr::new(
        format!("(graph_inputs.{field}).xyz"),
        ValueType::Vec3,
    ))
}

/// Compile a TimeInput node to WGSL.
///
/// TimeInput exposes monotonic time in seconds from runtime uniforms.
///
/// # Output
/// - Port `time`: Type f32
/// - Uses time: true
pub fn compile_time_input(_node: &Node, out_port: Option<&str>) -> Result<TypedExpr> {
    let port = out_port.unwrap_or("time");
    match port {
        "time" => Ok(TypedExpr::with_time(
            "params.time".to_string(),
            ValueType::F32,
            true,
        )),
        other => bail!("TimeInput: unsupported output port '{other}'"),
    }
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
/// Origin is at **bottom-left** with Y increasing upward, matching OpenGL's gl_FragCoord.
///
/// This is computed as: in.uv * geometry_size
///
/// # Output
/// - Port `xy`: Type vec2f
/// - Uses time: false
pub fn compile_geo_fragcoord(_node: &Node, out_port: Option<&str>) -> Result<TypedExpr> {
    let port = out_port.unwrap_or("xy");
    match port {
        "xy" => Ok(TypedExpr::new("in.local_px.xy".to_string(), ValueType::Vec2)),
        "xyz" => Ok(TypedExpr::new("in.local_px".to_string(), ValueType::Vec3)),
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
        assert_eq!(expr.expr, "in.local_px.xy");
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
        let mut ctx = MaterialCompileContext::default();
        let node = Node {
            id: "color1".to_string(),
            node_type: "ColorInput".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_color_input(&node, None, &mut ctx).unwrap();
        assert_eq!(result.ty, ValueType::Vec4);
        assert!(result.expr.contains("graph_inputs."));
        assert!(!result.uses_time);
        assert_eq!(
            ctx.graph_input_kinds.get("color1"),
            Some(&GraphFieldKind::Vec4Color)
        );
    }

    #[test]
    fn test_color_input_custom() {
        let mut ctx = MaterialCompileContext::default();
        let node = Node {
            id: "color1".to_string(),
            node_type: "ColorInput".to_string(),
            params: HashMap::from([("value".to_string(), serde_json::json!([0.5, 0.3, 0.8, 1.0]))]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_color_input(&node, None, &mut ctx).unwrap();
        assert_eq!(result.ty, ValueType::Vec4);
        assert!(result.expr.contains("graph_inputs."));
    }

    #[test]
    fn test_float_input() {
        let mut ctx = MaterialCompileContext::default();
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

        let result = compile_float_or_int_input(&node, None, &mut ctx).unwrap();
        assert_eq!(result.ty, ValueType::F32);
        assert!(result.expr.contains("graph_inputs."));
    }

    #[test]
    fn test_vector2_input() {
        let mut ctx = MaterialCompileContext::default();
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

        let result = compile_vector2_input(&node, None, &mut ctx).unwrap();
        assert_eq!(result.ty, ValueType::Vec2);
        assert!(result.expr.contains(".xy"));
    }

    #[test]
    fn test_vector3_input() {
        let mut ctx = MaterialCompileContext::default();
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

        let result = compile_vector3_input(&node, None, &mut ctx).unwrap();
        assert_eq!(result.ty, ValueType::Vec3);
        assert!(result.expr.contains(".xyz"));
    }

    #[test]
    fn test_bool_input_default() {
        let mut ctx = MaterialCompileContext::default();
        let node = Node {
            id: "bool1".to_string(),
            node_type: "BoolInput".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_bool_input(&node, None, &mut ctx).unwrap();
        assert_eq!(result.ty, ValueType::Bool);
        assert!(result.expr.contains("graph_inputs."));
        assert!(!result.uses_time);
    }

    #[test]
    fn test_bool_input_true() {
        let mut ctx = MaterialCompileContext::default();
        let node = Node {
            id: "bool1".to_string(),
            node_type: "BoolInput".to_string(),
            params: HashMap::from([("value".to_string(), serde_json::json!(true))]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_bool_input(&node, None, &mut ctx).unwrap();
        assert_eq!(result.ty, ValueType::Bool);
        assert!(result.expr.contains("!= 0"));
        assert!(!result.uses_time);
    }

    #[test]
    fn test_time_input_time_port() {
        let node = Node {
            id: "time1".to_string(),
            node_type: "TimeInput".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let result = compile_time_input(&node, Some("time")).unwrap();
        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, "params.time");
        assert!(result.uses_time);
    }

    #[test]
    fn test_time_input_invalid_port() {
        let node = Node {
            id: "time1".to_string(),
            node_type: "TimeInput".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        };

        let err = compile_time_input(&node, Some("value"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("TimeInput: unsupported output port 'value'"));
    }
}
