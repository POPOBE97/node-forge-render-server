use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn load_rgba8(path: &Path) -> image::RgbaImage {
    image::open(path)
        .unwrap_or_else(|e| panic!("failed to open image {}: {e}", path.display()))
        .to_rgba8()
}

fn ensure_baseline_png(path: &Path) -> bool {
    if path.exists() {
        return false;
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("failed to create dir {}: {e}", parent.display()));
    }

    // Minimal deterministic image so the scene can load ImageTexture.
    // Content doesn't matter for this test; only resolution math + headless render success.
    let mut img = image::RgbaImage::new(64, 64);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let r = (x as u8).wrapping_mul(4);
        let g = (y as u8).wrapping_mul(4);
        *p = image::Rgba([r, g, 0, 255]);
    }

    img.save(path)
        .unwrap_or_else(|e| panic!("failed to write baseline image {}: {e}", path.display()));

    true
}

#[test]
fn headless_dsl_json_renders_calculated_window_size() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let test_dir = manifest_dir.join("tests/calculated-window-size");

    let scene_json = test_dir.join("calculated-window-size.json");
    assert!(
        scene_json.exists(),
        "expected calculated-window-size case to exist at {}",
        scene_json.display()
    );

    // The JSON references "baseline-test.png" with a relative path.
    // Create it on the fly in the same folder to keep the repo free of binary fixtures.
    let baseline_png = test_dir.join("baseline-test.png");
    let baseline_created = ensure_baseline_png(&baseline_png);

    // This case is designed so RenderTexture.width is *NOT* correct in params,
    // and must be driven by chained math via connections:
    // IntInput_7.value (399) * FloatInput_12.value (2.756) => floor(1099.644) = 1099
    // IntInput_8.value (871) * FloatInput_12.value (2.756) => floor(2400.476) = 2400
    const EXPECTED_W: u32 = 1099;
    const EXPECTED_H: u32 = 2400;

    // Render headlessly to file.
    let out_png = test_dir.join("out/headless-out.png");
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
        .current_dir(&test_dir)
        .args([
            "--headless",
            "--dsl-json",
            scene_json
                .to_str()
                .expect("scene_json path must be valid UTF-8"),
            "--render-to-file",
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
    assert_eq!(img.dimensions(), (EXPECTED_W, EXPECTED_H));

    // Cleanup on success: keep artifacts when failing to aid debugging.
    let _ = std::fs::remove_file(&out_png);
    if baseline_created {
        let _ = std::fs::remove_file(&baseline_png);
    }
}
