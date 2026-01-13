use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn load_rgba8(path: &Path) -> image::RgbaImage {
    image::open(path)
        .unwrap_or_else(|e| panic!("failed to open image {}: {e}", path.display()))
        .to_rgba8()
}

#[test]
fn headless_dsl_json_renders_chained_blur_pass() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let scene_json = manifest_dir.join("tests/chained-blur-pass/chained-blur-pass.json");
    assert!(
        scene_json.exists(),
        "expected chained blur case to exist at {}",
        scene_json.display()
    );

    // Must be an absolute path (CLI enforces it).
    let out_png = manifest_dir.join("tests/chained-blur-pass/out/headless-out.png");

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
