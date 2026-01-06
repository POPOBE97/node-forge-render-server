use std::path::PathBuf;

use node_forge_render_server::{dsl, renderer};

fn case_dir(case_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("cases")
        .join(case_name)
}

fn list_json_cases(dir: &std::path::Path) -> Vec<PathBuf> {
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
                cases.push(path);
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

    let json_cases = list_json_cases(&dir);
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
