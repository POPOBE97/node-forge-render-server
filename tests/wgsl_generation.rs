use std::path::PathBuf;

use node_forge_render_server::{dsl, renderer};

fn case_dir(case_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("cases")
        .join(case_name)
}

fn list_json_cases(dir: &std::path::Path, update_goldens: bool) -> Vec<PathBuf> {
    let mut cases = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return cases;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            if std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                if update_goldens {
                    cases.push(path);
                } else {
                    // Only run golden comparisons for cases that have committed goldens.
                    // (Some JSONs are kept around as drafts / future fixtures.)
                    let stem = case_stem(&path);
                    let has_any_module_golden = std::fs::read_dir(dir)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .any(|e| {
                            let p = e.path();
                            p.is_file()
                                && p.file_name()
                                    .and_then(|s| s.to_str())
                                    .is_some_and(|name| {
                                        name.starts_with(&format!("{stem}."))
                                            && name.ends_with(".module.wgsl")
                                    })
                        });
                    if has_any_module_golden {
                        cases.push(path);
                    }
                }
            }
        }
    }
    cases.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    cases
}

fn case_stem(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("case")
        .to_string()
}

#[test]
fn dsl_json_compiles_to_valid_wgsl_modules() {
    let dir = case_dir("wgsl_generation");
    let update_goldens = std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v != "0");

    let json_cases = list_json_cases(&dir, update_goldens);
    assert!(
        !json_cases.is_empty(),
        "expected at least one *.json case in {}",
        dir.display()
    );

    for input_path in json_cases {
        let case_name = case_stem(&input_path);
        let scene = dsl::load_scene_from_path(&input_path)
            .unwrap_or_else(|e| panic!("case {case_name}: load input DSL json failed: {e}"));

        let passes = renderer::build_all_pass_wgsl_bundles_from_scene(&scene)
            .unwrap_or_else(|e| panic!("case {case_name}: build WGSL bundles failed: {e}"));

        assert!(
            !passes.is_empty(),
            "case {case_name}: expected at least one RenderPass"
        );

        for (pass_id, bundle) in passes {
            assert!(
                !bundle.module.trim().is_empty(),
                "case {case_name}, pass {pass_id}: module WGSL should not be empty"
            );
            assert!(
                bundle.vertex.contains("fn vs_main"),
                "case {case_name}, pass {pass_id}: vertex WGSL should contain vs_main"
            );
            assert!(
                bundle.fragment.contains("fn fs_main"),
                "case {case_name}, pass {pass_id}: fragment WGSL should contain fs_main"
            );

            // Optional compute stage for now.
            assert!(
                bundle.compute.is_none() || !bundle.compute.as_ref().unwrap().trim().is_empty(),
                "case {case_name}, pass {pass_id}: compute WGSL should not be empty when present"
            );
                let expected_vertex_path = dir.join(format!("{case_name}.{pass_id}.vertex.wgsl"));
                let expected_fragment_path = dir.join(format!("{case_name}.{pass_id}.fragment.wgsl"));
                let expected_module_path = dir.join(format!("{case_name}.{pass_id}.module.wgsl"));

            if update_goldens {
                std::fs::write(&expected_vertex_path, &bundle.vertex)
                    .unwrap_or_else(|e| panic!("write {:?}: {e}", expected_vertex_path));
                std::fs::write(&expected_fragment_path, &bundle.fragment)
                    .unwrap_or_else(|e| panic!("write {:?}: {e}", expected_fragment_path));
                std::fs::write(&expected_module_path, &bundle.module)
                    .unwrap_or_else(|e| panic!("write {:?}: {e}", expected_module_path));
            } else {
                let expected_vertex = std::fs::read_to_string(&expected_vertex_path)
                    .unwrap_or_else(|e| panic!("read {:?}: {e}", expected_vertex_path));
                let expected_fragment = std::fs::read_to_string(&expected_fragment_path)
                    .unwrap_or_else(|e| panic!("read {:?}: {e}", expected_fragment_path));
                let expected_module = std::fs::read_to_string(&expected_module_path)
                    .unwrap_or_else(|e| panic!("read {:?}: {e}", expected_module_path));

                assert_eq!(
                    bundle.vertex, expected_vertex,
                    "case {case_name}, pass {pass_id}: vertex golden mismatch"
                );
                assert_eq!(
                    bundle.fragment, expected_fragment,
                    "case {case_name}, pass {pass_id}: fragment golden mismatch"
                );
                assert_eq!(
                    bundle.module, expected_module,
                    "case {case_name}, pass {pass_id}: module golden mismatch"
                );
            }

            // Validate WGSL syntax without requiring a GPU.
            naga::front::wgsl::parse_str(&bundle.module).unwrap_or_else(|e| {
                panic!(
                    "case {case_name}, pass {pass_id}: WGSL parse failed: {e:?}\nWGSL:\n{}",
                    bundle.module
                )
            });
        }
    }
}

#[test]
fn unlinked_unknown_nodes_are_treeshaken_before_validation() {
    // Use a known-good golden case as the base scene.
    let input_path = case_dir("wgsl_generation").join("simple-rectangle.json");

    let mut scene = dsl::load_scene_from_path(input_path)
        .unwrap_or_else(|e| panic!("load input DSL json failed: {e}"));

    // Inject an editor-leftover node with an unknown type that is not connected.
    // Without treeshake, scheme validation would fail.
    scene.nodes.push(dsl::Node {
        id: "__unused_unknown__".to_string(),
        node_type: "__TotallyUnknownNodeType__".to_string(),
        params: std::collections::HashMap::new(),
        inputs: Vec::new(),
    });

    let passes = renderer::build_all_pass_wgsl_bundles_from_scene(&scene)
        .expect("expected WGSL generation to succeed after treeshake");
    assert!(!passes.is_empty(), "expected at least one RenderPass");
}

#[test]
fn primitive_values_can_drive_pass_inputs_via_auto_fullscreen_pass() {
    use std::collections::HashMap;

    // Build a minimal scene:
    // ColorInput.color -> Composite.pass (type pass)
    // RenderTexture.texture -> Composite.target
    // Composite.pass -> Screen.pass (RenderTarget)
    let scene = dsl::SceneDSL {
        version: "1.0".to_string(),
        metadata: dsl::Metadata {
            name: "auto-pass".to_string(),
            created: None,
            modified: None,
        },
        nodes: vec![
            dsl::Node {
                id: "out".to_string(),
                node_type: "Composite".to_string(),
                params: HashMap::new(),
                inputs: Vec::new(),
            },
            dsl::Node {
                id: "c".to_string(),
                node_type: "ColorInput".to_string(),
                params: HashMap::from([("value".to_string(), serde_json::json!([0.2, 0.3, 0.4, 1.0]))]),
                inputs: Vec::new(),
            },
            dsl::Node {
                id: "tgt".to_string(),
                node_type: "RenderTexture".to_string(),
                params: HashMap::from([
                    ("width".to_string(), serde_json::json!(64)),
                    ("height".to_string(), serde_json::json!(32)),
                    ("format".to_string(), serde_json::json!("rgba8unorm")),
                ]),
                inputs: Vec::new(),
            },
            dsl::Node {
                id: "screen".to_string(),
                node_type: "Screen".to_string(),
                params: HashMap::new(),
                inputs: Vec::new(),
            },
        ],
        connections: vec![
            dsl::Connection {
                id: "edge_color".to_string(),
                from: dsl::Endpoint {
                    node_id: "c".to_string(),
                    port_id: "color".to_string(),
                },
                to: dsl::Endpoint {
                    node_id: "out".to_string(),
                    port_id: "pass".to_string(),
                },
            },
            dsl::Connection {
                id: "edge_target".to_string(),
                from: dsl::Endpoint {
                    node_id: "tgt".to_string(),
                    port_id: "texture".to_string(),
                },
                to: dsl::Endpoint {
                    node_id: "out".to_string(),
                    port_id: "target".to_string(),
                },
            },
            dsl::Connection {
                id: "edge_present".to_string(),
                from: dsl::Endpoint {
                    node_id: "out".to_string(),
                    port_id: "pass".to_string(),
                },
                to: dsl::Endpoint {
                    node_id: "screen".to_string(),
                    port_id: "pass".to_string(),
                },
            },
        ],
        outputs: Some(HashMap::from([("composite".to_string(), "out".to_string())])),
    };

    let passes = renderer::build_all_pass_wgsl_bundles_from_scene(&scene)
        .expect("expected primitive->pass scene to compile via auto fullscreen pass");
    assert!(!passes.is_empty(), "expected at least one RenderPass bundle");
    assert!(
        passes.iter().any(|(id, _)| id.starts_with("__auto_fullscreen_pass__")),
        "expected at least one synthesized fullscreen pass id"
    );
}
