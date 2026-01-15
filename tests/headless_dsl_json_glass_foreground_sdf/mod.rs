use std::{
    path::{Path, PathBuf},
    process::Command,
};

use node_forge_render_server::{dsl, renderer};

fn load_rgba8(path: &Path) -> image::RgbaImage {
    image::open(path)
        .unwrap_or_else(|e| panic!("failed to open image {}: {e}", path.display()))
        .to_rgba8()
}

#[test]
fn headless_dsl_json_renders_glass_foreground_sdf2d() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let test_dir = manifest_dir.join("tests/glass-foreground-sdf");

    let scene_json = test_dir.join("glass-foreground-sdf.json");
    assert!(
        scene_json.exists(),
        "expected case to exist at {}",
        scene_json.display()
    );

    let scene = dsl::load_scene_from_path(&scene_json)
        .unwrap_or_else(|e| panic!("failed to load scene {}: {e}", scene_json.display()));

    let sdf2d_count = scene
        .nodes
        .iter()
        .filter(|n| n.node_type == "Sdf2D")
        .count();
    assert!(sdf2d_count > 0, "expected at least one Sdf2D node");

    let prepared = renderer::scene_prep::prepare_scene(&scene)
        .unwrap_or_else(|e| panic!("failed to prepare scene {}: {e}", scene_json.display()));

    let out_dir = test_dir.join("out");
    let out_png = out_dir.join("headless-out.png");
    std::fs::create_dir_all(&out_dir)
        .unwrap_or_else(|e| panic!("failed to create out dir {}: {e}", out_dir.display()));

    // Also dump generated shaders for inspection (like tests/wgsl_generation).
    let passes = renderer::build_all_pass_wgsl_bundles_from_scene(&scene)
        .unwrap_or_else(|e| panic!("failed to build WGSL bundles from scene {}: {e}", scene_json.display()));
    for (pass_id, bundle) in passes {
        if pass_id == "foreground_pass" {
            assert!(
                bundle.fragment.contains("in.uv"),
                "expected foreground_pass fragment to reference in.uv (UV-driven SDF), but it did not"
            );
        }
        let base = format!("glass-foreground-sdf.{pass_id}");
        std::fs::write(out_dir.join(format!("{base}.vertex.wgsl")), &bundle.vertex)
            .unwrap_or_else(|e| panic!("failed to write {base}.vertex.wgsl: {e}"));
        std::fs::write(out_dir.join(format!("{base}.fragment.wgsl")), &bundle.fragment)
            .unwrap_or_else(|e| panic!("failed to write {base}.fragment.wgsl: {e}"));
        std::fs::write(out_dir.join(format!("{base}.module.wgsl")), &bundle.module)
            .unwrap_or_else(|e| panic!("failed to write {base}.module.wgsl: {e}"));
        if let Some(compute) = &bundle.compute {
            std::fs::write(out_dir.join(format!("{base}.compute.wgsl")), compute)
                .unwrap_or_else(|e| panic!("failed to write {base}.compute.wgsl: {e}"));
        }
    }

    if out_png.exists() {
        std::fs::remove_file(&out_png)
            .unwrap_or_else(|e| panic!("failed to remove old output {}: {e}", out_png.display()));
    }

    let exe = env!("CARGO_BIN_EXE_node-forge-render-server");
    let output = Command::new(exe)
        .current_dir(&test_dir)
        .args([
            "--headless",
            "--render-to-file",
            "--dsl-json",
            scene_json
                .to_str()
                .expect("scene_json path must be valid UTF-8"),
            "--output",
            out_png.to_str().expect("out_png path must be valid UTF-8"),
        ])
        .output()
        .expect("failed to run node-forge-render-server binary");

    if !output.status.success() {
        panic!(
            "headless render failed (status={:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    assert!(
        out_png.exists(),
        "expected output image to be saved at {}, but it does not exist\nstdout:\n{}\nstderr:\n{}",
        out_png.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let meta = std::fs::metadata(&out_png)
        .unwrap_or_else(|e| panic!("failed to stat output {}: {e}", out_png.display()));
    assert!(meta.len() > 0, "output png is empty: {}", out_png.display());

    let img = load_rgba8(&out_png);
    assert_eq!(
        img.dimensions(),
        (prepared.resolution[0], prepared.resolution[1]),
        "unexpected output dimensions"
    );
}
