//! Compilers for input nodes (BoolInput, ColorInput, FloatInput, MidiInput, IntInput, Vector2Input, Vector3Input, TextureInput, TimeInput, ResourcePool).

use anyhow::{Result, bail};
use std::collections::HashMap;

use super::super::types::{GraphFieldKind, MaterialCompileContext, TypedExpr, ValueType};
use super::super::utils::{coerce_to_type, readable_wgsl_ident};
use crate::dsl::{self, Node, SceneDSL, incoming_connection};
use crate::renderer::graph_uniforms::graph_field_name;
use crate::renderer::validation::GlslShaderStage;

fn display_param<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
    node.params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn graph_input_preferred_name(node: &Node) -> String {
    let raw = display_param(node, "__group_input_name")
        .or_else(|| display_param(node, "label"))
        .unwrap_or(node.id.as_str());
    let name = readable_wgsl_ident(raw);
    match name.as_str() {
        "float_input" | "int_input" | "bool_input" | "vector2_input" | "vector3_input"
        | "vector4_input" | "color_input" | "input" => readable_wgsl_ident(&node.id),
        _ => name,
    }
}

fn register_graph_field(
    ctx: &mut MaterialCompileContext,
    node: &Node,
    kind: GraphFieldKind,
) -> String {
    if ctx.preserve_legacy_graph_input_names {
        return ctx.register_graph_input_named(node.id.as_str(), kind, &graph_field_name(&node.id));
    }
    ctx.register_graph_input_named(&node.id, kind, &graph_input_preferred_name(node))
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
pub fn compile_color_input(
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
) -> Result<TypedExpr> {
    let field = register_graph_field(ctx, node, GraphFieldKind::Vec4Color);
    Ok(TypedExpr::new(
        format!(
            "vec4f((graph_inputs.{field}).rgb * (graph_inputs.{field}).a, (graph_inputs.{field}).a)"
        ),
        ValueType::Vec4,
    ))
}

/// Compile a FloatInput, MidiInput, or IntInput node to WGSL.
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
    if node.node_type == "IntInput" {
        let field = register_graph_field(ctx, node, GraphFieldKind::I32);
        Ok(TypedExpr::new(
            format!("f32((graph_inputs.{field}).x)"),
            ValueType::F32,
        ))
    } else {
        let field = register_graph_field(ctx, node, GraphFieldKind::F32);
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
    let field = register_graph_field(ctx, node, GraphFieldKind::Bool);
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
    let field = register_graph_field(ctx, node, GraphFieldKind::Vec2);
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
    let field = register_graph_field(ctx, node, GraphFieldKind::Vec3);
    Ok(TypedExpr::new(
        format!("(graph_inputs.{field}).xyz"),
        ValueType::Vec3,
    ))
}

/// Compile a Vector4Input node to WGSL.
///
/// Vector4Input nodes provide a constant 4D vector value with no color
/// semantics. Distinct from `ColorInput`, which premultiplies alpha at the
/// read site.
///
/// # Parameters
/// - `x`/`y`/`z`/`w`: components, each defaulting to 0.0.
///
/// # Output
/// - Type: vec4f
/// - Uses time: false
pub fn compile_vector4_input(
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
) -> Result<TypedExpr> {
    let field = register_graph_field(ctx, node, GraphFieldKind::Vec4);
    Ok(TypedExpr::new(
        format!("(graph_inputs.{field})"),
        ValueType::Vec4,
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
        "xy" => Ok(TypedExpr::new(
            "in.local_px.xy".to_string(),
            ValueType::Vec2,
        )),
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

/// Compile a ResourcePool node.
///
/// ResourcePool is a pure passthrough: it selects one of N dynamic inputs by index
/// and forwards that expression. No WGSL code is generated for the node itself.
pub fn compile_resource_pool<F>(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
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
    let dynamic_inputs: Vec<&str> = node
        .inputs
        .iter()
        .filter(|p| p.id != "selectedIndex")
        .map(|p| p.id.as_str())
        .collect();

    let output_type = node
        .outputs
        .first()
        .and_then(|p| p.port_type.as_deref())
        .and_then(|t| map_pool_port_type(t));

    if dynamic_inputs.is_empty() {
        return Ok(zero_value_for(output_type.unwrap_or(ValueType::F32)));
    }

    let idx = dsl::resolve_input_i64(scene, nodes_by_id, &node.id, "selectedIndex")?
        .unwrap_or(0)
        .max(0) as usize;
    let idx = idx.min(dynamic_inputs.len() - 1);

    let selected_port = dynamic_inputs[idx];

    let Some(conn) = incoming_connection(scene, &node.id, selected_port) else {
        return Ok(zero_value_for(output_type.unwrap_or(ValueType::F32)));
    };

    let expr = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;

    if let Some(target) = output_type {
        if target != expr.ty {
            return coerce_to_type(expr, target);
        }
    }

    Ok(expr)
}

fn map_pool_port_type(s: &str) -> Option<ValueType> {
    let t = s.to_ascii_lowercase();
    match t.as_str() {
        "float" | "f32" | "number" => Some(ValueType::F32),
        "int" | "i32" => Some(ValueType::I32),
        "bool" | "boolean" => Some(ValueType::Bool),
        "vector2" | "vec2" => Some(ValueType::Vec2),
        "vector3" | "vec3" => Some(ValueType::Vec3),
        "vector4" | "vec4" | "color" => Some(ValueType::Vec4),
        "any" => None,
        _ => None,
    }
}

fn zero_value_for(ty: ValueType) -> TypedExpr {
    match ty {
        ValueType::F32 => TypedExpr::new("0.0", ValueType::F32),
        ValueType::I32 => TypedExpr::new("0", ValueType::I32),
        ValueType::U32 => TypedExpr::new("0u", ValueType::U32),
        ValueType::Bool => TypedExpr::new("false", ValueType::Bool),
        ValueType::Vec2 => TypedExpr::new("vec2f(0.0, 0.0)", ValueType::Vec2),
        ValueType::Vec3 => TypedExpr::new("vec3f(0.0, 0.0, 0.0)", ValueType::Vec3),
        ValueType::Vec4 => TypedExpr::new("vec4f(0.0, 0.0, 0.0, 0.0)", ValueType::Vec4),
        _ => TypedExpr::new("0.0", ValueType::F32),
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
            wgsl_override: None,
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
            wgsl_override: None,
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
            wgsl_override: None,
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
            wgsl_override: None,
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
            wgsl_override: None,
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
            wgsl_override: None,
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
            wgsl_override: None,
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
            wgsl_override: None,
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
            wgsl_override: None,
        };

        let result = compile_vector3_input(&node, None, &mut ctx).unwrap();
        assert_eq!(result.ty, ValueType::Vec3);
        assert!(result.expr.contains(".xyz"));
    }

    #[test]
    fn test_vector4_input() {
        let mut ctx = MaterialCompileContext::default();
        let node = Node {
            id: "vec4_1".to_string(),
            node_type: "Vector4Input".to_string(),
            params: HashMap::from([
                ("x".to_string(), serde_json::json!(1.0)),
                ("y".to_string(), serde_json::json!(2.0)),
                ("z".to_string(), serde_json::json!(3.0)),
                ("w".to_string(), serde_json::json!(4.0)),
            ]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };

        let result = compile_vector4_input(&node, None, &mut ctx).unwrap();
        assert_eq!(result.ty, ValueType::Vec4);
        assert!(result.expr.contains("graph_inputs."));
        // No swizzle: Vector4Input reads the full 4-channel slot.
        assert!(!result.expr.contains(".xyz"));
        assert!(!result.expr.contains(".xy"));
        assert_eq!(
            ctx.graph_input_kinds.get("vec4_1"),
            Some(&GraphFieldKind::Vec4)
        );
        assert!(!result.uses_time);
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
            wgsl_override: None,
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
            wgsl_override: None,
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
            wgsl_override: None,
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
            wgsl_override: None,
        };

        let err = compile_time_input(&node, Some("value"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("TimeInput: unsupported output port 'value'"));
    }
}

#[cfg(test)]
mod resource_pool_tests {
    use super::*;
    use crate::dsl::NodePort;
    use crate::renderer::node_compiler::test_utils::{test_connection, test_scene};

    fn make_float_node(id: &str, value: f64) -> Node {
        Node {
            id: id.to_string(),
            node_type: "FloatInput".to_string(),
            params: HashMap::from([("value".to_string(), serde_json::json!(value))]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: vec![NodePort {
                id: "value".to_string(),
                name: None,
                port_type: Some("float".to_string()),
            }],
            wgsl_override: None,
        }
    }

    fn make_pool_node(id: &str, dynamic_port_ids: &[&str], selected_index: i64) -> Node {
        let mut inputs: Vec<NodePort> = vec![NodePort {
            id: "selectedIndex".to_string(),
            name: Some("Index".to_string()),
            port_type: Some("int".to_string()),
        }];
        for port_id in dynamic_port_ids {
            inputs.push(NodePort {
                id: port_id.to_string(),
                name: None,
                port_type: Some("any".to_string()),
            });
        }

        Node {
            id: id.to_string(),
            node_type: "ResourcePool".to_string(),
            params: HashMap::from([(
                "selectedIndex".to_string(),
                serde_json::json!(selected_index),
            )]),
            inputs,
            input_bindings: Vec::new(),
            outputs: vec![NodePort {
                id: "output".to_string(),
                name: Some("Output".to_string()),
                port_type: Some("any".to_string()),
            }],
            wgsl_override: None,
        }
    }

    #[test]
    fn test_resource_pool_selects_second_input() {
        let float_a = make_float_node("fa", 1.0);
        let float_b = make_float_node("fb", 2.0);
        let float_c = make_float_node("fc", 3.0);
        let pool = make_pool_node("pool", &["d1", "d2", "d3"], 1);

        let connections = vec![
            test_connection("fa", "value", "pool", "d1"),
            test_connection("fb", "value", "pool", "d2"),
            test_connection("fc", "value", "pool", "d3"),
        ];

        let scene = test_scene(vec![float_a, float_b, float_c, pool.clone()], connections);
        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.clone()))
            .collect();

        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_resource_pool(
            &scene,
            &nodes_by_id,
            &pool,
            None,
            &mut ctx,
            &mut cache,
            |node_id, _port, ctx, _cache| {
                // Mock: return the node_id as an expression tagged with the id
                ctx.register_graph_input(node_id, GraphFieldKind::F32);
                let field = crate::renderer::graph_uniforms::graph_field_name(node_id);
                Ok(TypedExpr::new(
                    format!("(graph_inputs.{field}).x"),
                    ValueType::F32,
                ))
            },
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        let fb_field = crate::renderer::graph_uniforms::graph_field_name("fb");
        assert!(
            result.expr.contains(&fb_field),
            "expected expression to reference fb, got: {}",
            result.expr
        );
    }

    #[test]
    fn test_resource_pool_clamps_out_of_range_index() {
        let float_a = make_float_node("fa", 1.0);
        let pool = make_pool_node("pool", &["d1"], 99);

        let connections = vec![test_connection("fa", "value", "pool", "d1")];

        let scene = test_scene(vec![float_a, pool.clone()], connections);
        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.clone()))
            .collect();

        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_resource_pool(
            &scene,
            &nodes_by_id,
            &pool,
            None,
            &mut ctx,
            &mut cache,
            |node_id, _port, ctx, _cache| {
                ctx.register_graph_input(node_id, GraphFieldKind::F32);
                let field = crate::renderer::graph_uniforms::graph_field_name(node_id);
                Ok(TypedExpr::new(
                    format!("(graph_inputs.{field}).x"),
                    ValueType::F32,
                ))
            },
        )
        .unwrap();

        // Should clamp to index 0 (only input)
        assert_eq!(result.ty, ValueType::F32);
        let fa_field = crate::renderer::graph_uniforms::graph_field_name("fa");
        assert!(result.expr.contains(&fa_field));
    }

    #[test]
    fn test_resource_pool_no_dynamic_inputs_returns_zero() {
        let pool = make_pool_node("pool", &[], 0);

        let scene = test_scene(vec![pool.clone()], vec![]);
        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.clone()))
            .collect();

        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_resource_pool(
            &scene,
            &nodes_by_id,
            &pool,
            None,
            &mut ctx,
            &mut cache,
            |_, _, _, _| unreachable!("should not compile any upstream"),
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, "0.0");
    }

    #[test]
    fn test_resource_pool_unconnected_selected_port_returns_zero() {
        // Pool has dynamic ports declared but the selected one is not connected
        let pool = make_pool_node("pool", &["d1", "d2"], 0);

        let scene = test_scene(vec![pool.clone()], vec![]);
        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.clone()))
            .collect();

        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let result = compile_resource_pool(
            &scene,
            &nodes_by_id,
            &pool,
            None,
            &mut ctx,
            &mut cache,
            |_, _, _, _| unreachable!("should not compile any upstream"),
        )
        .unwrap();

        assert_eq!(result.ty, ValueType::F32);
        assert_eq!(result.expr, "0.0");
    }
}
