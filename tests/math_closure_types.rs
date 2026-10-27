use std::collections::HashMap;

use node_forge_render_server::{
    dsl::{SceneDSL, materialize_scene_node_labels_from_raw_json},
    renderer::{
        node_compiler::math_closure::compile_math_closure,
        types::{MaterialCompileContext, TypedExpr, ValueType},
        validation::GlslShaderStage,
    },
};
use serde_json::json;

fn validate_closure(source: &str, output_type: &str, stage: GlslShaderStage, connected: bool) {
    let scene: SceneDSL = serde_json::from_value(json!({
        "version": "6.1", "workingColorSpace": "linear-srgb", "dpi": 3.0,
        "metadata": { "name": "MathClosure boolean types" },
        "nodes": [{
            "id": "mask",
            "type": "MathClosure",
            "params": { "source": source, "outputType": output_type },
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

#[test]
fn math_closure_array_loop_validates_helper_and_call_site() {
    let scene: SceneDSL = serde_json::from_value(json!({
        "version": "6.1", "workingColorSpace": "linear-srgb", "dpi": 3.0, "metadata": { "name": "arrays" },
        "nodes": [{
            "id": "array", "type": "MathClosure",
            "params": { "source": "for (int i=0; i<2; i++) { output[i] = points[i] * 2.0; }",
                "outputType": "vector2[]", "outputArrayLength": 2 },
            "inputs": [{ "id": "points", "name": "points", "type": "vector2[]", "arrayLength": 2 }],
            "outputs": [{ "id": "output", "type": "vector2[]", "arrayLength": 2 }]
        }], "connections": []
    })).unwrap();
    for stage in [GlslShaderStage::Fragment, GlslShaderStage::Vertex] {
        let mut context = MaterialCompileContext::default();
        let output = compile_math_closure(&scene, &HashMap::new(), &scene.nodes[0], Some("output"),
            &mut context, &mut HashMap::new(), |_, _, _, _| unreachable!(), stage).unwrap();
        let wgsl = format!("{}\nstruct Input {{ uv: vec2f }}\nfn check(uv: vec2f, in: Input) -> {} {{\n{}\nreturn {};\n}}",
            context.extra_wgsl_decls.values().cloned().collect::<Vec<_>>().join("\n"),
            output.ty.wgsl_type_string(), context.inline_stmts.join("\n"), output.expr);
        let module = naga::front::wgsl::parse_str(&wgsl).unwrap();
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module).unwrap();
    }
}

#[test]
fn math_closure_rejects_missing_contract_instead_of_guessing() {
    let scene: SceneDSL = serde_json::from_value(json!({
        "version": "6.1", "workingColorSpace": "linear-srgb", "dpi": 3.0, "metadata": { "name": "contract" },
        "nodes": [{ "id": "invalid", "type": "MathClosure", "params": { "source": "output = vec3(1.0);" },
            "outputs": [{ "id": "output", "type": "vector3" }] }], "connections": []
    })).unwrap();
    let error = compile_math_closure(&scene, &HashMap::new(), &scene.nodes[0], Some("output"),
        &mut MaterialCompileContext::default(), &mut HashMap::new(), |_, _, _, _| unreachable!(),
        GlslShaderStage::Fragment).unwrap_err();
    assert!(error.to_string().contains("missing outputType"));
}

#[test]
fn math_closure_display_labels_preserve_function_links_in_both_stages() {
    // Live scene loading materializes labels in params. Numeric and non-ASCII labels
    // can introduce underscores that Naga normalizes during WGSL emission.
    let labels = ["06 大刻度 · 极坐标位置", "06", "大刻度", "!!!"];
    let nodes: Vec<_> = labels.iter().enumerate().map(|(index, label)| json!({
        "id": format!("gauge.position.{index}"),
        "type": "MathClosure",
        "label": label,
        "params": {
            "source": "float a = radians(angle - float(index) * 30.0);\nfloat theta = 150.0 + angle - float(index) * 30.0;\nvec2 v = base.xy - vec2(1575.0, -90.0);\nvec2 p = vec2(1575.0, -90.0) + vec2(cos(a) * v.x - sin(a) * v.y, sin(a) * v.x + cos(a) * v.y);\noutput = theta < 132.0 || theta > 168.0 ? vec3(100000.0, 100000.0, 0.0) : vec3(p, base.z);",
            "outputType": "vector3"
        },
        "inputs": [
            { "id": "angle", "name": "angle", "type": "float" },
            { "id": "base", "name": "base", "type": "vector3" },
            { "id": "index", "name": "index", "type": "int" }
        ],
        "outputs": [{ "id": "output", "type": "vector3" }]
    })).collect();
    let raw_scene = json!({
        "version": "6.1", "workingColorSpace": "linear-srgb", "dpi": 3.0,
        "metadata": { "name": "MathClosure display labels" },
        "nodes": nodes, "connections": []
    });
    let mut scene: SceneDSL = serde_json::from_value(raw_scene.clone()).unwrap();
    materialize_scene_node_labels_from_raw_json(&mut scene, &raw_scene);
    let mut context = MaterialCompileContext::default();
    for stage in [GlslShaderStage::Vertex, GlslShaderStage::Fragment] {
        for node in &scene.nodes {
            compile_math_closure(
                &scene, &HashMap::new(), node, Some("output"),
                &mut context, &mut HashMap::new(), |_, _, _, _| unreachable!(), stage,
            ).unwrap_or_else(|err| panic!("{stage:?}, {}: {err:#}", node.id));
        }
    }
    // Labels that normalize identically must still get distinct helper names.
    assert_eq!(context.extra_wgsl_decls.len(), labels.len() * 2);
    let wgsl = format!(
        "{}\nstruct Input {{ uv: vec2f }}\nfn check(uv: vec2f, in: Input) {{\n{}\n}}",
        context.extra_wgsl_decls.values().cloned().collect::<Vec<_>>().join("\n"),
        context.inline_stmts.join("\n"),
    );
    let module = naga::front::wgsl::parse_str(&wgsl)
        .unwrap_or_else(|err| panic!("{}", err.emit_to_string(&wgsl)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all(),
    ).validate(&module).unwrap();
}

#[test]
fn math_closure_stage_names_do_not_alias_in_a_shared_context() {
    let scene: SceneDSL = serde_json::from_value(json!({
        "version": "6.1", "workingColorSpace": "linear-srgb", "dpi": 3.0, "metadata": { "name": "stages" },
        "nodes": [{ "id": "closure", "type": "MathClosure", "params": { "source": "output = 1.0;", "outputType": "float" },
            "outputs": [{ "id": "output", "type": "float" }] }], "connections": []
    })).unwrap();
    let mut context = MaterialCompileContext::default();
    let mut cache = HashMap::new();
    let mut outputs = Vec::new();
    for stage in [GlslShaderStage::Vertex, GlslShaderStage::Fragment] {
        outputs.push(compile_math_closure(&scene, &HashMap::new(), &scene.nodes[0], Some("output"),
            &mut context, &mut cache, |_, _, _, _| unreachable!(), stage).unwrap());
    }
    assert_eq!(context.extra_wgsl_decls.len(), 2);
    assert_ne!(outputs[0].expr, outputs[1].expr);
    let wgsl = format!("{}\nstruct Input {{ uv: vec2f }}\nfn check(uv: vec2f, in: Input) {{\n{}\n}}",
        context.extra_wgsl_decls.values().cloned().collect::<Vec<_>>().join("\n"), context.inline_stmts.join("\n"));
    let module = naga::front::wgsl::parse_str(&wgsl).unwrap();
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module).unwrap();
}
