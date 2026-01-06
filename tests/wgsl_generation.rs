use std::path::PathBuf;

use node_forge_render_server::{dsl, renderer};

fn case_dir(case_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("cases")
        .join(case_name)
}

#[test]
fn dsl_json_compiles_to_valid_wgsl_modules() {
    let dir = case_dir("wgsl_generation");
    let input_path = dir.join("input.json");
    let scene = dsl::load_scene_from_path(&input_path).expect("load input DSL json");

    let passes = renderer::build_all_pass_wgsl_bundles_from_scene(&scene)
        .expect("build WGSL bundles from scene");

    assert!(!passes.is_empty(), "expected at least one RenderPass");

    let update_goldens = std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v != "0");

    for (pass_id, bundle) in passes {
        assert!(
            !bundle.module.trim().is_empty(),
            "pass {pass_id}: module WGSL should not be empty"
        );
        assert!(
            bundle.vertex.contains("fn vs_main"),
            "pass {pass_id}: vertex WGSL should contain vs_main"
        );
        assert!(
            bundle.fragment.contains("fn fs_main"),
            "pass {pass_id}: fragment WGSL should contain fs_main"
        );

        // Optional compute stage for now.
        assert!(bundle.compute.is_none() || !bundle.compute.as_ref().unwrap().trim().is_empty());

        let expected_vertex_path = dir.join(format!("{pass_id}.vertex.wgsl"));
        let expected_fragment_path = dir.join(format!("{pass_id}.fragment.wgsl"));
        let expected_module_path = dir.join(format!("{pass_id}.module.wgsl"));

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

            assert_eq!(bundle.vertex, expected_vertex, "pass {pass_id}: vertex golden mismatch");
            assert_eq!(bundle.fragment, expected_fragment, "pass {pass_id}: fragment golden mismatch");
            assert_eq!(bundle.module, expected_module, "pass {pass_id}: module golden mismatch");
        }

        // Validate WGSL syntax without requiring a GPU.
        naga::front::wgsl::parse_str(&bundle.module).unwrap_or_else(|e| {
            panic!(
                "pass {pass_id}: WGSL parse failed: {e:?}\nWGSL:\n{}",
                bundle.module
            )
        });
    }
}
