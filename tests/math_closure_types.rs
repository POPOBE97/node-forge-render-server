use std::collections::HashMap;

use node_forge_render_server::{
    dsl::SceneDSL,
    renderer::{
        node_compiler::math_closure::compile_math_closure,
        types::{MaterialCompileContext, TypedExpr, ValueType},
        validation::GlslShaderStage,
    },
};
use serde_json::json;

fn validate_closure(source: &str, output_type: &str, stage: GlslShaderStage, connected: bool) {
    let scene: SceneDSL = serde_json::from_value(json!({
        "version": "6.0",
        "metadata": { "name": "MathClosure boolean types" },
        "nodes": [{
            "id": "mask",
            "type": "MathClosure",
            "params": { "source": source },
            "inputs": [{ "id": "enabled", "name": "enabled", "type": "bool" }],
            "outputs": [{ "id": "output", "type": output_type }]
        }],
        "connections": if connected { json!([{
            "id": "enabled.mask",
            "from": { "nodeId": "enabled", "portId": "value" },
            "to": { "nodeId": "mask", "portId": "enabled" }
        }]) } else { json!([]) }
    }))
    .expect("parse test scene");
    let mut ctx = MaterialCompileContext::default();
    let output = compile_math_closure(
        &scene,
        &HashMap::new(),
        &scene.nodes[0],
        Some("output"),
        &mut ctx,
        &mut HashMap::new(),
        |_, _, _, _| Ok(TypedExpr::new("enabled_value", ValueType::Bool)),
        stage,
    )
    .unwrap_or_else(|err| panic!("{stage:?}, connected={connected}: {err:#}"));

    // Validate the helper AND its call site. Parsing alone would miss a boolean
    // helper being called with a float, which was the second half of this bug.
    let wgsl = format!(
        "{}\nstruct Input {{ uv: vec2f }}\nfn check(uv: vec2f, in: Input, enabled_value: bool) -> {} {{\n{}\nreturn {};\n}}",
        ctx.extra_wgsl_decls
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
        output.ty.wgsl(),
        ctx.inline_stmts.join("\n"),
        output.expr,
    );
    let module = naga::front::wgsl::parse_str(&wgsl)
        .unwrap_or_else(|err| panic!("{}", err.emit_to_string(&wgsl)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|err| panic!("{stage:?}: {err:?}\n{wgsl}"));
}

#[test]
fn math_closure_boolean_input_preserves_conditions_in_both_stages() {
    for stage in [GlslShaderStage::Fragment, GlslShaderStage::Vertex] {
        for connected in [true, false] {
            validate_closure(
                "if (enabled) { output = enabled && !false ? 40.0 : 0.0; } else { output = 0.0; }",
                "float",
                stage,
                connected,
            );
        }
    }
}

#[test]
fn math_closure_boolean_output_preserves_type_in_both_stages() {
    for stage in [GlslShaderStage::Fragment, GlslShaderStage::Vertex] {
        validate_closure("output = !enabled;", "bool", stage, true);
    }
}
