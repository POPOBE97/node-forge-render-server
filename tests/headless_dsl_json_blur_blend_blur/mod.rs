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
fn headless_dsl_json_renders_blur_blend_blur() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let scene_json = manifest_dir.join("tests/blur-blend-blur/blur-blend-blur.json");
    assert!(
        scene_json.exists(),
        "expected blur-blend-blur case to exist at {}",
        scene_json.display()
    );

    // Sanity: this scene is expected to composite TWO Gaussian blur layers.
    // If only one is considered a Composite layer, it will look like only one draw contributed.
    let scene = dsl::load_scene_from_path(&scene_json)
        .unwrap_or_else(|e| panic!("failed to load scene {}: {e}", scene_json.display()));
    let prepared = renderer::scene_prep::prepare_scene(&scene)
        .unwrap_or_else(|e| panic!("failed to prepare scene {}: {e}", scene_json.display()));
    assert!(
        prepared
            .composite_layers_in_draw_order
            .iter()
            .any(|id| id == "GuassianBlurPass_17"),
        "expected Composite layers to include GuassianBlurPass_17, got {:?}",
        prepared.composite_layers_in_draw_order
    );
    assert!(
        prepared
            .composite_layers_in_draw_order
            .iter()
            .any(|id| id == "GuassianBlurPass_20"),
        "expected Composite layers to include GuassianBlurPass_20, got {:?}",
        prepared.composite_layers_in_draw_order
    );

    // Must be an absolute path (CLI enforces it).
    let out_png = manifest_dir.join("tests/blur-blend-blur/out/headless-out.png");
    if let Some(parent) = out_png.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("failed to create output dir {}: {e}", parent.display()));
    }

    if out_png.exists() {
        std::fs::remove_file(&out_png)
            .unwrap_or_else(|e| panic!("failed to remove old output {}: {e}", out_png.display()));
    }

    let exe = env!("CARGO_BIN_EXE_node-forge-render-server");
    let output = Command::new(exe)
        .current_dir(&manifest_dir)
        .args([
            "--headless",
            "--dsl-json",
            scene_json
                .to_str()
                .expect("scene_json path must be valid UTF-8"),
            "--render-to-file",
            "--output",
            out_png
                .to_str()
                .expect("out_png path must be valid UTF-8"),
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

    // The case uses a 1080x2400 RenderTexture as Composite.target.
    let img = load_rgba8(&out_png);
    assert_eq!(img.dimensions(), (1080, 2400));
}
