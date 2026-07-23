use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};
use naga::{ArraySize, ImageClass, ImageDimension, ScalarKind, TypeInner, VectorSize};

use super::super::types::{
    GraphFieldKind, MaterialCompileContext, PassTextureRef, TypedExpr, ValueType,
};
use crate::dsl::{Node, SceneDSL, incoming_connection};
use crate::renderer::geometry_resolver::is_pass_like_node_type;
use crate::renderer::utils::{coerce_to_type, sanitize_wgsl_ident};

const SYSTEM_DECL_KEY: &str = "00.shader_material.system";
const SYSTEM_DECL: &str = r#"
struct ShaderMaterialInput {
    uv: vec2f,
    frag_coord: vec2f,
    local_position: vec3f,
    geometry_size: vec2f,
    target_size: vec2f,
    time: f32,
};
"#;

#[derive(Clone, Debug)]
enum ReflectedParameter {
    Value {
        name: String,
        port_id: String,
        value_type: ValueType,
        graph_kind: GraphFieldKind,
    },
    Resource {
        name: String,
        port_id: String,
        texture_parameter: String,
        sampler_parameter: String,
    },
}

#[derive(Clone, Debug)]
enum ReflectedArgument {
    Value {
        name: String,
        value_type: ValueType,
        graph_kind: GraphFieldKind,
    },
    Texture(String),
    Sampler(String),
}

fn vector_len(size: VectorSize) -> usize {
    match size {
        VectorSize::Bi => 2,
        VectorSize::Tri => 3,
        VectorSize::Quad => 4,
    }
}

fn reflect_argument(
    module: &naga::Module,
    ty: naga::Handle<naga::Type>,
    name: &str,
) -> Result<ReflectedArgument> {
    match &module.types[ty].inner {
        TypeInner::Scalar(scalar) if scalar.kind == ScalarKind::Float && scalar.width == 4 => {
            Ok(ReflectedArgument::Value {
                name: name.to_string(),
                value_type: ValueType::F32,
                graph_kind: GraphFieldKind::F32,
            })
        }
        TypeInner::Scalar(scalar) if scalar.kind == ScalarKind::Sint && scalar.width == 4 => {
            Ok(ReflectedArgument::Value {
                name: name.to_string(),
                value_type: ValueType::I32,
                graph_kind: GraphFieldKind::I32,
            })
        }
        TypeInner::Scalar(scalar) if scalar.kind == ScalarKind::Bool => {
            Ok(ReflectedArgument::Value {
                name: name.to_string(),
                value_type: ValueType::Bool,
                graph_kind: GraphFieldKind::Bool,
            })
        }
        TypeInner::Vector { size, scalar }
            if scalar.kind == ScalarKind::Float && scalar.width == 4 =>
        {
            let (value_type, graph_kind) = match size {
                VectorSize::Bi => (ValueType::Vec2, GraphFieldKind::Vec2),
                VectorSize::Tri => (ValueType::Vec3, GraphFieldKind::Vec3),
                VectorSize::Quad => (ValueType::Vec4, GraphFieldKind::Vec4),
            };
            Ok(ReflectedArgument::Value {
                name: name.to_string(),
                value_type,
                graph_kind,
            })
        }
        TypeInner::Matrix {
            columns: VectorSize::Quad,
            rows: VectorSize::Quad,
            scalar,
        } if scalar.kind == ScalarKind::Float && scalar.width == 4 => {
            Ok(ReflectedArgument::Value {
                name: name.to_string(),
                value_type: ValueType::Mat4,
                graph_kind: GraphFieldKind::Mat4,
            })
        }
        TypeInner::Array { base, size, .. } => {
            let ArraySize::Constant(length) = size else {
                bail!(
                    "ShaderMaterial parameter '{name}' uses a runtime-sized array; only fixed arrays are supported"
                );
            };
            let length = length.get() as usize;
            let (value_type, graph_kind) = match &module.types[*base].inner {
                TypeInner::Scalar(scalar)
                    if scalar.kind == ScalarKind::Float && scalar.width == 4 =>
                {
                    (
                        ValueType::F32Array(length),
                        GraphFieldKind::F32Array(length),
                    )
                }
                TypeInner::Vector { size, scalar }
                    if scalar.kind == ScalarKind::Float && scalar.width == 4 =>
                {
                    match vector_len(*size) {
                        2 => (
                            ValueType::Vec2Array(length),
                            GraphFieldKind::Vec2Array(length),
                        ),
                        3 => (
                            ValueType::Vec3Array(length),
                            GraphFieldKind::Vec3Array(length),
                        ),
                        4 => (
                            ValueType::Vec4Array(length),
                            GraphFieldKind::Vec4Array(length),
                        ),
                        _ => unreachable!(),
                    }
                }
                _ => {
                    bail!("ShaderMaterial parameter '{name}' has an unsupported array element type")
                }
            };
            Ok(ReflectedArgument::Value {
                name: name.to_string(),
                value_type,
                graph_kind,
            })
        }
        TypeInner::Image {
            dim: ImageDimension::D2,
            arrayed: false,
            class:
                ImageClass::Sampled {
                    kind: ScalarKind::Float,
                    multi: false,
                },
        } => Ok(ReflectedArgument::Texture(name.to_string())),
        TypeInner::Sampler { comparison: false } => {
            Ok(ReflectedArgument::Sampler(name.to_string()))
        }
        TypeInner::Struct { .. } => bail!(
            "ShaderMaterial parameter '{name}' uses a user-defined struct; struct parameters are not supported"
        ),
        _ => bail!("ShaderMaterial parameter '{name}' has an unsupported WGSL type"),
    }
}

fn is_vec4f(module: &naga::Module, ty: naga::Handle<naga::Type>) -> bool {
    matches!(
        &module.types[ty].inner,
        TypeInner::Vector {
            size: VectorSize::Quad,
            scalar: naga::Scalar {
                kind: ScalarKind::Float,
                width: 4
            }
        }
    )
}

fn reflect_parameters(source: &str) -> Result<Vec<ReflectedParameter>> {
    let combined = format!("{SYSTEM_DECL}\n{source}");
    let module = naga::front::wgsl::parse_str(&combined).map_err(|error| {
        anyhow!(
            "ShaderMaterial WGSL parse failed:\n{}",
            error.emit_to_string(&combined)
        )
    })?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|error| anyhow!("ShaderMaterial WGSL validation failed: {error:?}"))?;

    if !module.entry_points.is_empty() {
        bail!(
            "ShaderMaterial entry points are not allowed; define only fn shader_material(...) -> vec4f"
        );
    }
    if !module.global_variables.is_empty() {
        bail!(
            "ShaderMaterial global variables and custom @group/@binding declarations are not supported"
        );
    }

    let function = module
        .functions
        .iter()
        .find_map(|(_, function)| {
            (function.name.as_deref() == Some("shader_material")).then_some(function)
        })
        .ok_or_else(|| {
            anyhow!("ShaderMaterial source is missing fn shader_material(...) -> vec4f")
        })?;

    let first = function.arguments.first().ok_or_else(|| {
        anyhow!("shader_material must take ShaderMaterialInput as its first parameter")
    })?;
    if module.types[first.ty].name.as_deref() != Some("ShaderMaterialInput") {
        bail!("shader_material first parameter must be ShaderMaterialInput");
    }
    if !function
        .result
        .as_ref()
        .is_some_and(|result| is_vec4f(&module, result.ty))
    {
        bail!("shader_material return type must be vec4f");
    }

    let mut raw = Vec::new();
    for (index, argument) in function.arguments.iter().enumerate().skip(1) {
        let name = argument
            .name
            .clone()
            .unwrap_or_else(|| format!("parameter_{index}"));
        raw.push(reflect_argument(&module, argument.ty, &name)?);
    }

    let mut reflected = Vec::new();
    let mut index = 0;
    while index < raw.len() {
        match &raw[index] {
            ReflectedArgument::Value {
                name,
                value_type,
                graph_kind,
            } => {
                reflected.push(ReflectedParameter::Value {
                    name: name.clone(),
                    port_id: format!("param:{name}"),
                    value_type: *value_type,
                    graph_kind: *graph_kind,
                });
                index += 1;
            }
            ReflectedArgument::Texture(texture_parameter) => {
                let Some(ReflectedArgument::Sampler(sampler_parameter)) = raw.get(index + 1) else {
                    bail!(
                        "ShaderMaterial texture parameter '{texture_parameter}' must be immediately followed by a sampler parameter"
                    );
                };
                reflected.push(ReflectedParameter::Resource {
                    name: texture_parameter.clone(),
                    port_id: format!("resource:{texture_parameter}"),
                    texture_parameter: texture_parameter.clone(),
                    sampler_parameter: sampler_parameter.clone(),
                });
                index += 2;
            }
            ReflectedArgument::Sampler(name) => {
                bail!(
                    "ShaderMaterial sampler parameter '{name}' must immediately follow a texture_2d<f32> parameter"
                );
            }
        }
    }

    Ok(reflected)
}

fn expression_originates_from_system_input(
    module: &naga::Module,
    function: &naga::Function,
    handle: naga::Handle<naga::Expression>,
    depth: usize,
) -> bool {
    if depth > 16 {
        return false;
    }
    match function.expressions[handle] {
        naga::Expression::FunctionArgument(index) => function
            .arguments
            .get(index as usize)
            .is_some_and(|argument| {
                module.types[argument.ty].name.as_deref() == Some("ShaderMaterialInput")
            }),
        naga::Expression::Load { pointer } => {
            expression_originates_from_system_input(module, function, pointer, depth + 1)
        }
        naga::Expression::Access { base, .. }
        | naga::Expression::AccessIndex { base, .. }
        | naga::Expression::Swizzle { vector: base, .. } => {
            expression_originates_from_system_input(module, function, base, depth + 1)
        }
        naga::Expression::LocalVariable(local) => {
            function.local_variables[local]
                .init
                .is_some_and(|initializer| {
                    expression_originates_from_system_input(
                        module,
                        function,
                        initializer,
                        depth + 1,
                    )
                })
        }
        _ => false,
    }
}

fn source_uses_system_time(source: &str) -> Result<bool> {
    let combined = format!("{SYSTEM_DECL}\n{source}");
    let module = naga::front::wgsl::parse_str(&combined).map_err(|error| {
        anyhow!(
            "ShaderMaterial WGSL parse failed while detecting time usage:\n{}",
            error.emit_to_string(&combined)
        )
    })?;
    Ok(module.functions.iter().any(|(_, function)| {
        function.expressions.iter().any(|(_, expression)| {
            matches!(
                expression,
                naga::Expression::AccessIndex { base, index: 5 }
                    if expression_originates_from_system_input(&module, function, *base, 0)
            )
        })
    }))
}

fn load_node_source(node: &Node) -> String {
    let override_path = node
        .wgsl_override
        .as_deref()
        .and_then(super::template_loader::resolve_override_path);
    super::template_loader::load_template_with_override(
        override_path.as_deref(),
        "shader_material_default.wgsl",
    )
}

pub fn validate_node_source(node: &Node) -> Result<()> {
    reflect_parameters(&load_node_source(node)).map(|_| ())
}

pub fn node_uses_time(node: &Node) -> bool {
    source_uses_system_time(&load_node_source(node)).unwrap_or(false)
}

fn graph_value_expression(
    ctx: &mut MaterialCompileContext,
    node: &Node,
    parameter_name: &str,
    value_type: ValueType,
    kind: GraphFieldKind,
) -> TypedExpr {
    let parameter_key = format!("{}::param:{parameter_name}", node.id);
    let preferred = format!(
        "shader_{}_{}",
        sanitize_wgsl_ident(&node.id),
        sanitize_wgsl_ident(parameter_name)
    );
    let field = ctx.register_shader_parameter_named(&parameter_key, kind, &preferred);
    let base = format!("shader_material_params.{field}");
    let expression = match value_type {
        ValueType::F32 => format!("({base}).x"),
        ValueType::I32 => format!("({base}).x"),
        ValueType::Bool => format!("(({base}).x != 0)"),
        ValueType::Vec2 => format!("({base}).xy"),
        ValueType::Vec3 => format!("({base}).xyz"),
        ValueType::Vec4 => base,
        ValueType::Mat4 => base,
        ValueType::F32Array(length) => {
            let values = (0..length)
                .map(|index| format!("({base}[{index}]).x"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("array<f32, {length}>({values})")
        }
        ValueType::Vec2Array(length) => {
            let values = (0..length)
                .map(|index| format!("({base}[{index}]).xy"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("array<vec2f, {length}>({values})")
        }
        ValueType::Vec3Array(length) => {
            let values = (0..length)
                .map(|index| format!("({base}[{index}]).xyz"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("array<vec3f, {length}>({values})")
        }
        ValueType::Vec4Array(length) => {
            let values = (0..length)
                .map(|index| format!("{base}[{index}]"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("array<vec4f, {length}>({values})")
        }
        _ => unreachable!("unsupported ShaderMaterial graph value type"),
    };
    TypedExpr::new(expression, value_type)
}

fn resolve_resource(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    port_id: &str,
    ctx: &mut MaterialCompileContext,
) -> Result<(String, String)> {
    let connection = incoming_connection(scene, &node.id, port_id)
        .ok_or_else(|| anyhow!("ShaderMaterial resource '{port_id}' is not connected"))?;
    let upstream = nodes_by_id.get(&connection.from.node_id).ok_or_else(|| {
        anyhow!(
            "ShaderMaterial resource upstream node not found: {}",
            connection.from.node_id
        )
    })?;

    match upstream.node_type.as_str() {
        "ImageTexture" if connection.from.port_id == "texture" => {
            ctx.register_image_texture(&upstream.id);
            Ok((
                MaterialCompileContext::tex_var_name(&upstream.id),
                MaterialCompileContext::sampler_var_name(&upstream.id),
            ))
        }
        "PassTexture" if connection.from.port_id == "texture" => {
            let pass_connection = incoming_connection(scene, &upstream.id, "pass")
                .ok_or_else(|| anyhow!("PassTexture.pass input is not connected"))?;
            let pass = nodes_by_id
                .get(&pass_connection.from.node_id)
                .ok_or_else(|| anyhow!("PassTexture upstream pass not found"))?;
            if !is_pass_like_node_type(&pass.node_type) {
                bail!(
                    "PassTexture.pass must be connected to a pass node, got {}",
                    pass.node_type
                );
            }
            let texture_ref = PassTextureRef::through_pass_texture(
                &upstream.id,
                &pass.id,
                &pass_connection.from.port_id,
            );
            ctx.register_pass_texture_ref(texture_ref);
            Ok((
                MaterialCompileContext::pass_tex_var_name(&upstream.id),
                MaterialCompileContext::pass_sampler_var_name(&upstream.id),
            ))
        }
        _ => bail!(
            "ShaderMaterial resource '{port_id}' expects ImageTexture.texture or PassTexture.texture"
        ),
    }
}

fn renamed_source(source: &str, suffix: &str) -> Result<String> {
    let combined = format!("{SYSTEM_DECL}\n{source}");
    let module = naga::front::wgsl::parse_str(&combined).map_err(|error| {
        anyhow!(
            "ShaderMaterial WGSL parse failed while namespacing functions:\n{}",
            error.emit_to_string(&combined)
        )
    })?;
    let function_names = module
        .functions
        .iter()
        .filter_map(|(_, function)| function.name.as_deref())
        .map(|name| (name, format!("{name}_{suffix}")))
        .collect::<HashMap<_, _>>();
    if !function_names.contains_key("shader_material") {
        bail!("ShaderMaterial source is missing fn shader_material");
    }

    // WGSL has no first-class function values, so a function identifier is
    // referenced only where the next token is `(` (declarations and calls).
    // Restricting replacements to those sites avoids touching fields or local
    // variables that happen to share a helper function's name.
    let mut output = String::with_capacity(source.len() + function_names.len() * suffix.len());
    let mut index = 0;
    while index < source.len() {
        let character = source[index..].chars().next().expect("valid source index");
        if character.is_ascii_alphabetic() || character == '_' {
            let start = index;
            index += character.len_utf8();
            while index < source.len() {
                let next = source[index..].chars().next().expect("valid source index");
                if !next.is_ascii_alphanumeric() && next != '_' {
                    break;
                }
                index += next.len_utf8();
            }
            let identifier = &source[start..index];
            let next_is_call = source[index..]
                .chars()
                .skip_while(|next| next.is_whitespace())
                .next()
                == Some('(');
            if next_is_call {
                if let Some(replacement) = function_names.get(identifier) {
                    output.push_str(replacement);
                    continue;
                }
            }
            output.push_str(identifier);
        } else {
            output.push(character);
            index += character.len_utf8();
        }
    }
    Ok(output)
}

pub fn compile_shader_material<F>(
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
    let source = load_node_source(node);
    let reflected = reflect_parameters(&source)?;

    ctx.extra_wgsl_decls
        .entry(SYSTEM_DECL_KEY.to_string())
        .or_insert_with(|| SYSTEM_DECL.to_string());

    let suffix = sanitize_wgsl_ident(&node.id);
    let function_name = format!("shader_material_{suffix}");
    let source_key = format!("10.shader_material.{suffix}");
    ctx.extra_wgsl_decls
        .entry(source_key)
        .or_insert(renamed_source(&source, &suffix)?);

    let mut arguments = vec![
        "ShaderMaterialInput(in.uv, in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time)"
            .to_string(),
    ];
    let mut uses_time = source_uses_system_time(&source)?;

    for parameter in reflected {
        match parameter {
            ReflectedParameter::Value {
                name,
                port_id,
                value_type,
                graph_kind,
            } => {
                let expression =
                    if let Some(connection) = incoming_connection(scene, &node.id, &port_id) {
                        coerce_to_type(
                            compile_fn(
                                &connection.from.node_id,
                                Some(&connection.from.port_id),
                                ctx,
                                cache,
                            )?,
                            value_type,
                        )?
                    } else {
                        graph_value_expression(ctx, node, &name, value_type, graph_kind)
                    };
                uses_time |= expression.uses_time;
                arguments.push(expression.expr);
            }
            ReflectedParameter::Resource {
                name,
                port_id,
                texture_parameter,
                sampler_parameter,
            } => {
                let (texture, sampler) =
                    resolve_resource(scene, nodes_by_id, node, &port_id, ctx).map_err(|error| {
                        anyhow!(
                            "ShaderMaterial resource '{name}' ({texture_parameter}, {sampler_parameter}): {error:#}"
                        )
                    })?;
                arguments.push(texture);
                arguments.push(sampler);
            }
        }
    }

    Ok(TypedExpr::with_time(
        format!("{function_name}({})", arguments.join(", ")),
        ValueType::Vec4,
        uses_time,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{Connection, Endpoint, Metadata};
    use serde_json::json;

    #[test]
    fn reflects_fixed_arrays_and_texture_sampler_pairs() {
        let parameters = reflect_parameters(
            r#"
fn shader_material(
    in: ShaderMaterialInput,
    gain: f32,
    transform: mat4x4f,
    weights: array<f32, 4>,
    background: texture_2d<f32>,
    background_sampler: sampler,
) -> vec4f {
    return textureSample(background, background_sampler, in.uv) * gain * weights[0];
}
"#,
        )
        .unwrap();
        assert_eq!(parameters.len(), 4);
        assert!(matches!(
            parameters[2],
            ReflectedParameter::Value {
                value_type: ValueType::F32Array(4),
                ..
            }
        ));
        assert!(matches!(parameters[3], ReflectedParameter::Resource { .. }));
    }

    #[test]
    fn rejects_struct_parameters() {
        let error = reflect_parameters(
            r#"
struct Lighting { gain: f32 };
fn shader_material(in: ShaderMaterialInput, lighting: Lighting) -> vec4f {
    return vec4f(lighting.gain);
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("struct"));
    }

    #[test]
    fn namespaces_user_helper_functions_per_node() {
        let source = r#"
fn tone(value: vec4f) -> vec4f {
    return value * 0.5;
}
fn shader_material(in: ShaderMaterialInput) -> vec4f {
    return tone(vec4f(in.uv, 0.0, 1.0));
}
"#;
        let first = renamed_source(source, "first").unwrap();
        let second = renamed_source(source, "second").unwrap();
        let combined = format!("{SYSTEM_DECL}\n{first}\n{second}");
        assert!(combined.contains("fn tone_first("));
        assert!(combined.contains("tone_second(vec4f"));
        let module = naga::front::wgsl::parse_str(&combined).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn detects_system_time_usage_in_main_and_helper_functions() {
        assert!(
            source_uses_system_time(
                r#"
fn animate(in: ShaderMaterialInput) -> f32 {
    return sin(in.time);
}
fn shader_material(in: ShaderMaterialInput) -> vec4f {
    return vec4f(in.uv, animate(in), 1.0);
}
"#
            )
            .unwrap()
        );
        assert!(
            !source_uses_system_time(
                r#"
fn shader_material(in: ShaderMaterialInput) -> vec4f {
    return vec4f(in.uv, 0.0, 1.0);
}
"#
            )
            .unwrap()
        );
    }

    #[test]
    fn compiles_default_uv_material_into_node_scoped_function() {
        let node = Node {
            id: "shader_1".to_string(),
            node_type: "ShaderMaterial".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };
        let scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "shader".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![node.clone()],
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };
        let nodes_by_id = HashMap::from([(node.id.clone(), node.clone())]);
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();
        let expression = compile_shader_material(
            &scene,
            &nodes_by_id,
            &node,
            Some("material"),
            &mut ctx,
            &mut cache,
            |_node_id, _port, _ctx, _cache| bail!("default ShaderMaterial has no graph parameters"),
        )
        .unwrap();

        assert_eq!(expression.ty, ValueType::Vec4);
        assert!(expression.expr.starts_with("shader_material_shader_1("));
        let declarations = ctx
            .extra_wgsl_decls
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(declarations.contains("struct ShaderMaterialInput"));
        assert!(declarations.contains("fn shader_material_shader_1"));
        assert!(declarations.contains("vec4f(in.uv, 0.0, 1.0)"));
    }

    #[test]
    fn default_uv_material_produces_valid_complete_wgsl() {
        let shader = Node {
            id: "shader_1".to_string(),
            node_type: "ShaderMaterial".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };
        let pass = Node {
            id: "pass_1".to_string(),
            node_type: "RenderPass".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };
        let scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "shader".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![shader.clone(), pass.clone()],
            connections: vec![Connection {
                id: "material".to_string(),
                from: Endpoint {
                    node_id: shader.id.clone(),
                    port_id: "material".to_string(),
                },
                to: Endpoint {
                    node_id: pass.id.clone(),
                    port_id: "material".to_string(),
                },
            }],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };
        let nodes_by_id = scene
            .nodes
            .iter()
            .cloned()
            .map(|node| (node.id.clone(), node))
            .collect();
        let bundle = crate::renderer::wgsl::build_pass_wgsl_bundle(
            &scene,
            &nodes_by_id,
            None,
            None,
            &pass.id,
            false,
            None,
            Vec::new(),
            String::new(),
            false,
        )
        .unwrap();

        crate::renderer::validation::validate_wgsl(&bundle.module).unwrap();
    }

    #[test]
    fn values_matrices_arrays_and_sampled_resources_produce_valid_complete_wgsl() {
        let override_path = std::env::temp_dir().join(format!(
            "node-forge-shader-material-{}-{}.wgsl",
            std::process::id(),
            std::thread::current().name().unwrap_or("resource-test")
        ));
        std::fs::write(
            &override_path,
            r#"
fn shader_material(
    in: ShaderMaterialInput,
    gain: f32,
    tint: vec4f,
    transform: mat4x4f,
    weights: array<f32, 2>,
    image: texture_2d<f32>,
    image_sampler: sampler,
) -> vec4f {
    let transformed = transform * vec4f(in.local_position, 1.0);
    return textureSample(image, image_sampler, in.uv) * tint * gain * weights[0]
        + transformed * 0.0;
}
"#,
        )
        .unwrap();

        let shader = Node {
            id: "shader_resource".to_string(),
            node_type: "ShaderMaterial".to_string(),
            params: serde_json::from_value(json!({
                "param:gain": 1.5,
                "param:tint": [1.0, 0.5, 0.25, 1.0],
                "param:transform": [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0,
                                    0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
                "param:weights": [1.0, 0.5],
            }))
            .unwrap(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: Some(override_path.to_string_lossy().to_string()),
        };
        let image = Node {
            id: "image_1".to_string(),
            node_type: "ImageTexture".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };
        let pass = Node {
            id: "pass_1".to_string(),
            node_type: "RenderPass".to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        };
        let scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "shader-resource".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![shader.clone(), image.clone(), pass.clone()],
            connections: vec![
                Connection {
                    id: "resource".to_string(),
                    from: Endpoint {
                        node_id: image.id.clone(),
                        port_id: "texture".to_string(),
                    },
                    to: Endpoint {
                        node_id: shader.id.clone(),
                        port_id: "resource:image".to_string(),
                    },
                },
                Connection {
                    id: "material".to_string(),
                    from: Endpoint {
                        node_id: shader.id.clone(),
                        port_id: "material".to_string(),
                    },
                    to: Endpoint {
                        node_id: pass.id.clone(),
                        port_id: "material".to_string(),
                    },
                },
            ],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        };
        let nodes_by_id = scene
            .nodes
            .iter()
            .cloned()
            .map(|node| (node.id.clone(), node))
            .collect();
        let bundle = crate::renderer::wgsl::build_pass_wgsl_bundle(
            &scene,
            &nodes_by_id,
            None,
            None,
            &pass.id,
            false,
            None,
            Vec::new(),
            String::new(),
            false,
        )
        .unwrap();

        crate::renderer::validation::validate_wgsl(&bundle.module).unwrap();
        assert_eq!(bundle.image_textures, vec![image.id]);
        assert_eq!(
            bundle
                .shader_parameter_schema
                .as_ref()
                .map(|schema| schema.size_bytes),
            Some(128)
        );
        assert!(bundle.module.contains("@group(0) @binding(3)"));
        assert!(
            bundle
                .module
                .contains("shader_material_params.shader_shader_resource_gain")
        );
        let _ = std::fs::remove_file(override_path);
    }
}
