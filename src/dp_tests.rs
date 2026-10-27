use crate::dsl::{self, Node, SceneDSL};
use crate::renderer::{graph_uniforms, node_compiler, types::MaterialCompileContext};
use serde_json::{Value, json};
use std::collections::HashMap;

pub(crate) fn scene(nodes: Value, connections: Value, dpi: f64) -> SceneDSL {
    serde_json::from_value(
        json!({"version":"6.1", "dpi":dpi, "workingColorSpace":"linear-srgb",
        "metadata":{"name":"DP test"}, "nodes":nodes, "connections":connections}),
    )
    .unwrap()
}
fn nodes(scene: &SceneDSL) -> HashMap<String, Node> {
    scene
        .nodes
        .iter()
        .cloned()
        .map(|node| (node.id.clone(), node))
        .collect()
}

#[test]
fn dpi_is_required_and_positive() {
    let original = serde_json::to_value(scene(json!([]), json!([]), 3.0)).unwrap();
    for value in [Value::Null, json!(0), json!(-1), json!("3")] {
        let mut invalid = original.clone();
        invalid["dpi"] = value;
        assert!(serde_json::from_value::<SceneDSL>(invalid).is_err());
    }
    let mut missing = original;
    missing.as_object_mut().unwrap().remove("dpi");
    assert!(serde_json::from_value::<SceneDSL>(missing).is_err());
}

#[test]
fn dp_integer_rounding_and_overflow_are_checked() {
    assert_eq!(dsl::dp_to_px(3.0, 2.5, true).unwrap(), 8.0);
    assert_eq!(dsl::dp_to_px(-3.0, 2.5, true).unwrap(), -8.0);
    assert_eq!(
        dsl::dp_to_px(i32::MAX as f64, 1.0, true).unwrap(),
        i32::MAX as f64
    );
    for (value, dpi) in [
        (1.5, 3.0),
        (i32::MAX as f64, 2.0),
        (1.0, 0.0),
        (f64::NAN, 3.0),
    ] {
        assert!(dsl::dp_to_px(value, dpi, true).is_err());
    }
}

#[test]
fn all_dp_declarations_pack_pixels_once_and_emit_valid_wgsl() {
    for (kind, mut params, expected) in [
        ("FloatInput", json!({"value":-1.25}), vec![-3.125]),
        ("IntInput", json!({"value":-3}), vec![-8.0]),
        ("IntInput", json!({"value":400000001}), vec![1000000003.0]),
        ("Vector2Input", json!({"x":0,"y":-2}), vec![0.0, -5.0]),
        (
            "Vector3Input",
            json!({"x":1,"y":-2,"z":0.5}),
            vec![2.5, -5.0, 1.25],
        ),
        (
            "Vector4Input",
            json!({"x":1,"y":-2,"z":0.5,"w":4}),
            vec![2.5, -5.0, 1.25, 10.0],
        ),
    ] {
        let units: serde_json::Map<String, Value> = params.as_object().unwrap().keys().map(|key| (key.clone(), json!("dp"))).collect();
        params["inputUnits"] = Value::Object(units);
        let scene = scene(
            json!([{"id":"input","type":kind,"params":params}]),
            json!([]),
            2.5,
        );
        let value = dsl::numeric_declaration_value(&scene, &scene.nodes[0]).unwrap();
        let actual: Vec<f64> = if let Some(array) = value.as_array() {
            array.iter().map(|v| v.as_f64().unwrap()).collect()
        } else {
            vec![value.as_f64().unwrap()]
        };
        assert_eq!(actual, expected);
        let mut ctx = MaterialCompileContext::default();
        let expr = node_compiler::compile_material_expr(
            &scene,
            &nodes(&scene),
            "input",
            Some(if kind.starts_with("Vector") {
                "vector"
            } else {
                "value"
            }),
            &mut ctx,
            &mut HashMap::new(),
        )
        .unwrap();
        let schema = graph_uniforms::build_graph_schema_with_field_names(
            &ctx.graph_input_kinds,
            &ctx.graph_input_field_names,
        );
        let bytes = graph_uniforms::pack_graph_values(&scene, &schema).unwrap();
        for (index, value) in expected.iter().enumerate() {
            let slot: [u8; 4] = bytes[index * 4..index * 4 + 4].try_into().unwrap();
            let packed = if kind == "IntInput" {
                i32::from_le_bytes(slot) as f64
            } else {
                f32::from_le_bytes(slot) as f64
            };
            assert_eq!(packed, *value);
        }
        let declarations = crate::renderer::wgsl::graph_inputs_wgsl_decl(
            &schema,
            crate::renderer::types::GraphBindingKind::Uniform,
        );
        let shader = format!(
            "{declarations}\n@compute @workgroup_size(1) fn main() {{ _ = {}; }}",
            expr.expr
        );
        naga::front::wgsl::parse_str(&shader).unwrap();
    }
}

#[test]
fn targets_resolve_connections_then_apply_explicit_unit_and_round_once() {
    for dpi in [1.0, 2.0, 3.0, 4.0] {
        for dp in [false, true] {
            let width = if dp { 360.0 } else { 1080.0 };
            let scene = scene(
                json!([{"id":"target","type":"RenderTexture","params":{"width":width,"height":width,"inputUnits":{"width":if dp {"dp"} else {"px"},"height":if dp {"dp"} else {"px"}}}}]),
                json!([]),
                dpi,
            );
            let expected = if dp { (360.0 * dpi) as u32 } else { 1080 };
            assert_eq!(
                dsl::render_texture_size(&scene, &nodes(&scene), &scene.nodes[0]).unwrap(),
                [expected, expected]
            );
        }
    }
    let mut scene = scene(
        json!([
            {"id":"input","type":"FloatInput","params":{"value":3.5,"inputUnits":{"value":"dp"}}},
            {"id":"target","type":"RenderTexture","params":{"width":999,"height":1.5}}
        ]),
        json!([{"id":"edge","from":{"nodeId":"input","portId":"value"},"to":{"nodeId":"target","portId":"width"}}]),
        2.5,
    );
    assert_eq!(
        dsl::render_texture_size(&scene, &nodes(&scene), &scene.nodes[1]).unwrap(),
        [9, 2]
    );
    scene.nodes[1].params.insert("inputUnits".into(), json!({"width":"dp","height":"dp"}));
    assert_eq!(
        dsl::render_texture_size(&scene, &nodes(&scene), &scene.nodes[1]).unwrap(),
        [9, 4]
    );
    for invalid in [0.0, -1.0, 0.1, f64::INFINITY, u32::MAX as f64 + 1.0] {
        assert!(dsl::physical_texture_dimension(invalid).is_err());
    }
}

#[test]
fn processing_size_accepts_dp_vectors_with_connected_components() {
    let scene = scene(
        json!([
            {"id":"scalar","type":"FloatInput","params":{"value":12.5,"inputUnits":{"value":"dp"}}},
            {"id":"vector","type":"Vector2Input","params":{"x":999,"y":8,"inputUnits":{"x":"dp","y":"dp"}}},
            {"id":"down","type":"Downsample"}
        ]),
        json!([
            {"id":"x","from":{"nodeId":"scalar","portId":"value"},"to":{"nodeId":"vector","portId":"x"}},
            {"id":"size","from":{"nodeId":"vector","portId":"vector"},"to":{"nodeId":"down","portId":"targetSize"}}
        ]),
        3.0,
    );
    assert_eq!(
        dsl::processing_target_size(&scene, &nodes(&scene), &scene.nodes[2]).unwrap(),
        [38, 24]
    );
}

#[test]
fn derivation_uses_document_dpi_and_preserves_runtime_scene_pixels() {
    use crate::state_machine::{
        graph::{AnimValue, GraphInputContext, evaluate_graph},
        types::DerivationDefinition,
    };
    let graph: DerivationDefinition = serde_json::from_value(json!({
        "id":"dp", "nodes":[
            {"id":"length","type":"FloatInput","inputs":[{"id":"value","type":"float"}],"params":{"value":1e308,"inputUnits":{"value":"dp"}}},
            {"id":"vector","type":"Vector4Input","params":{"x":1,"y":2,"z":3,"w":4,"inputUnits":{"x":"dp","y":"dp","z":"dp","w":"dp"}}}
        ],
        "inputBindings":[{"source":{"kind":"stateParam","stateParamId":"length"},"to":{"nodeId":"length","portId":"value"}}],
        "outputBindings":[
            {"uniform":{"nodeId":"renderLength","paramId":"value"},"from":{"nodeId":"length","portId":"value"}},
            {"uniform":{"nodeId":"renderVector","paramId":"vector"},"from":{"nodeId":"vector","portId":"vector"}}
        ],
        "passthroughBindings":[{"source":{"kind":"frameInput","frameInputId":"scene.dpi"},"uniform":{"nodeId":"density","paramId":"value"}}, {"source":{"kind":"frameInput","frameInputId":"mouse.position"},"uniform":{"nodeId":"pointer","paramId":"vector"}}]
    })).unwrap();
    let ctx = GraphInputContext {
        dpi: 2.5,
        values: HashMap::from([("length".into(), AnimValue::Float(-3.0))]),
        scene_elapsed_time: 0.0,
        local_elapsed_time: 0.0,
        mouse_position: Some(crate::state_machine::types::MousePosition { x: 123.0, y: 456.0 }),
        scene_size: None,
        dt: 0.0,
    };
    let result = evaluate_graph(&graph, &ctx).unwrap();
    assert_eq!(result["renderLength:value"], AnimValue::Float(-3.0));
    assert_eq!(
        result["renderVector:vector"],
        AnimValue::Vec4([2.5, 5.0, 7.5, 10.0])
    );
    assert_eq!(result["pointer:vector"], AnimValue::Vec2([123.0, 456.0]));
    assert_eq!(result["density:value"], AnimValue::Float(2.5));
}

#[test]
fn data_parse_preserves_converted_vector_components_through_a_chain() {
    let scene = scene(
        json!([
            {"id":"vector","type":"Vector3Input","params":{"x":-2,"y":0.5,"z":4,"inputUnits":{"x":"dp","y":"dp","z":"dp"}}},
            {"id":"first","type":"DataParse","params":{"source":"return { value: input as vec3 };"},
             "inputBindings":[{"portId":"input","variableName":"input","type":"vector3"}],
             "outputs":[{"id":"value","type":"vector3"}]},
            {"id":"second","type":"DataParse","params":{"source":"return { value: [input[0], input[1], input[2]] as vec3 };"},
             "inputBindings":[{"portId":"input","variableName":"input","type":"vector3"}],
             "outputs":[{"id":"value","type":"vector3"}]}
        ]),
        json!([
            {"id":"vector-first","from":{"nodeId":"vector","portId":"vector"},"to":{"nodeId":"first","portId":"input"}},
            {"id":"first-second","from":{"nodeId":"first","portId":"value"},"to":{"nodeId":"second","portId":"input"}}
        ]),
        2.5,
    );
    let mut runtime = crate::renderer::scene_prep::data_parse_runtime::TsRuntime::new();
    let result =
        dsl::evaluate_data_parse(&scene, &nodes(&scene), &scene.nodes[2], 0, &mut runtime).unwrap();
    assert_eq!(
        result["value"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect::<Vec<_>>(),
        vec![-5.0, 1.25, 10.0]
    );
}

#[test]
fn composite_planning_and_scene_size_use_the_physical_dp_extent() {
    use crate::renderer::render_plan::{planner::RenderPlanner, types::PlanBuildOptions};
    let scene = scene(
        json!([
            {"id":"target","type":"RenderTexture","params":{"width":360,"height":240,"inputUnits":{"width":"dp","height":"dp"},"format":"rgba8unorm"}},
            {"id":"composite","type":"Composite"},
            {"id":"screen","type":"Screen","params":{"outputColorSpace":"srgb"}},
            {"id":"draw","type":"RenderPass"},
            {"id":"geometry","type":"Rect2DGeometry","params":{"size":[100,100],"position":[180,120]}}
        ]),
        json!([
            {"id":"target","from":{"nodeId":"target","portId":"texture"},"to":{"nodeId":"composite","portId":"target"}},
            {"id":"output","from":{"nodeId":"composite","portId":"texture"},"to":{"nodeId":"screen","portId":"texture"}},
            {"id":"draw","from":{"nodeId":"draw","portId":"pass"},"to":{"nodeId":"composite","portId":"pass"}},
            {"id":"geometry","from":{"nodeId":"geometry","portId":"geometry"},"to":{"nodeId":"draw","portId":"geometry"}}
        ]),
        2.5,
    );
    let plan = RenderPlanner::new(PlanBuildOptions {
        gpu_caps: Default::default(),
        presentation_mode: Default::default(),
        debug_dump_wgsl_dir: None,
    })
    .plan(&scene, None, None)
    .unwrap();
    assert_eq!(plan.resolution, [900, 600]);
    assert_eq!(dsl::screen_resolution(&scene), Some([900, 600]));
}

#[test]
fn none_units_preserve_raw_values_at_every_dpi() {
    use crate::input_units::{convert_value, inner_params, materialize_scene};

    for dpi in [1.0, 2.5, 4.0] {
        for value in [json!(-1.25), json!(400000001), json!([[1, 2], [3, 4]]), json!({"x": 1, "y": -2})] {
            assert_eq!(convert_value(&value, &json!("none"), dpi, false).unwrap(), value);
        }
        assert_eq!(
            convert_value(&json!([1.25, 2.5, 3]), &json!(["none", "px", "dp"]), dpi, false).unwrap(),
            json!([1.25, 2.5, 3.0 * dpi])
        );
        let mut scene = scene(json!([
            {"id":"raw","type":"FloatInput","params":{"value":0.25,"inputUnits":{"value":"none"}}},
            {"id":"target","type":"RenderTexture","params":{"width":100,"height":80,"inputUnits":{"width":"none","height":"dp"}}}
        ]), json!([]), dpi);
        assert_eq!(dsl::numeric_declaration_value(&scene, &scene.nodes[0]).unwrap(), json!(0.25));
        assert_eq!(inner_params(&scene.nodes[0].params, "value", dpi, false).unwrap(), Some(json!(0.25)));
        materialize_scene(&mut scene).unwrap();
        assert_eq!(scene.nodes[0].params["value"], json!(0.25));
        assert_eq!(dsl::render_texture_size(&scene, &nodes(&scene), &scene.nodes[1]).unwrap(), [100, (80.0 * dpi) as u32]);
    }
}

#[test]
fn inline_consumer_units_materialize_once_and_connections_win() {
    let mut scene = scene(json!([
        {"id":"upstream","type":"Vector2Input","params":{"x":7,"y":8}},
        {"id":"shape","type":"Rect2DGeometry","params":{"size":[10,20],"position":[4,5],"inputUnits":{"size":["dp","px"],"position":"dp"}}},
        {"id":"target","type":"RenderTexture","params":{"width":100,"height":80,"inputUnits":{"width":"dp","height":"px"}}}
    ]), json!([
        {"id":"position","from":{"nodeId":"upstream","portId":"vector"},"to":{"nodeId":"shape","portId":"position"}}
    ]), 2.5);
    let authored = scene.clone();
    crate::input_units::materialize_scene(&mut scene).unwrap();
    assert_eq!(scene.nodes[1].params["size"], json!([25.0,20]));
    assert_eq!(scene.nodes[1].params["position"], json!([4,5]));
    assert!(!scene.nodes[1].params.contains_key("inputUnits"));
    let once = serde_json::to_value(&scene).unwrap();
    crate::input_units::materialize_scene(&mut scene).unwrap();
    assert_eq!(serde_json::to_value(&scene).unwrap(), once);
    assert_eq!(authored.nodes[1].params["size"], json!([10,20]));
    assert_eq!(dsl::render_texture_size(&scene, &nodes(&scene), &scene.nodes[2]).unwrap(), [250,80]);
    assert!(crate::input_units::convert_value(&json!([1,2]), &json!(["dp","px","dp"]), 3.0, false).is_err());
    assert_eq!(crate::input_units::convert_value(&json!([[1,2],[3,4]]), &json!([["dp","px"],["px","dp"]]), 2.5, false).unwrap(), json!([[2.5,2],[3,10.0]]));
    assert_eq!(crate::input_units::convert_value(&json!(0.5), &json!("dp"), 3.0, true).unwrap(), json!(2));
}

#[test]
fn inline_math_and_closure_inputs_reach_the_shader() {
    for (kind, params, expected) in [
        ("MathAdd", json!({"a":2,"b":3,"inputUnits":{"a":"dp"}}), "(5.0 + 3.0)"),
        ("MathMultiply", json!({"a":2,"b":3,"inputUnits":{"a":"dp"}}), "(5.0 * 3.0)"),
        ("MathMax", json!({"a":2,"b":3,"inputUnits":{"a":"dp"}}), "max(5.0, 3.0)"),
        ("Lerp", json!({"a":2,"b":3,"t":0.5,"inputUnits":{"a":"dp"}}), "mix(5.0, 3.0, 0.5)"),
    ] {
        let scene = scene(json!([{"id":"math","type":kind,"params":params}]), json!([]), 2.5);
        let expr = node_compiler::compile_material_expr(&scene, &nodes(&scene), "math", Some("result"), &mut MaterialCompileContext::default(), &mut HashMap::new()).unwrap();
        assert_eq!(expr.expr, expected, "{kind}");
        naga::front::wgsl::parse_str(&format!("@compute @workgroup_size(1) fn main() {{ _ = {}; }}", expr.expr)).unwrap();
    }
    let scene = scene(json!([{
        "id":"closure","type":"MathClosure",
        "params":{"source":"return length;","outputType":"float","length":12,"inputUnits":{"length":"dp"}},
        "inputs":[{"id":"length","name":"length","type":"float"}],
        "outputs":[{"id":"output","name":"Output","type":"float"}]
    }]), json!([]), 2.5);
    let mut ctx = MaterialCompileContext::default();
    node_compiler::compile_material_expr(&scene, &nodes(&scene), "closure", Some("output"), &mut ctx, &mut HashMap::new()).unwrap();
    assert!(ctx.inline_stmts.iter().any(|stmt| stmt.contains(", 30.0)")), "{:?}", ctx.inline_stmts);
}

#[test]
fn runtime_override_replaces_authored_units_before_preparation() {
    let mut scene = scene(json!([
        {"id":"length","type":"FloatInput","params":{"value":12,"inputUnits":{"value":"dp"}}}
    ]), json!([]), 3.0);
    assert_eq!(dsl::numeric_declaration_value(&scene, &scene.nodes[0]).unwrap(), json!(36.0));
    crate::state_machine::apply_overrides(&mut scene, &HashMap::from([
        (crate::state_machine::OverrideKey::new("length", "value"), json!(100))
    ]));
    crate::input_units::materialize_scene(&mut scene).unwrap();
    assert_eq!(dsl::numeric_declaration_value(&scene, &scene.nodes[0]).unwrap(), json!(100.0));
}
