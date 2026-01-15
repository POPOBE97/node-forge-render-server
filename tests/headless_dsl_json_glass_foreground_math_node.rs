// Keep the actual test + fixtures next to each other under tests/glass-foreground-math-node/.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn load_rgba8(path: &Path) -> image::RgbaImage {
    image::open(path)
        .unwrap_or_else(|e| panic!("failed to open image {}: {e}", path.display()))
        .to_rgba8()
}

fn diff_stats(expected: &image::RgbaImage, actual: &image::RgbaImage) -> (u64, u8) {
    let mut mismatched_pixels: u64 = 0;
    let mut max_channel_delta: u8 = 0;
    for (ep, ap) in expected.pixels().zip(actual.pixels()) {
        let mut any = false;
        for c in 0..4 {
            let d = ep.0[c].abs_diff(ap.0[c]);
            if d != 0 {
                any = true;
                max_channel_delta = max_channel_delta.max(d);
            }
        }
        if any {
            mismatched_pixels += 1;
        }
    }
    (mismatched_pixels, max_channel_delta)
}

#[test]
fn headless_dsl_json_renders_glass_foreground_math_node() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let test_dir = manifest_dir.join("tests/glass-foreground-math-node");

    let scene_json = test_dir.join("glass-foreground-math-node.json");
    let baseline_png = test_dir.join("baseline.png");
    let out_dir = test_dir.join("out");
    let out_png = out_dir.join("headless-out.png");

    std::fs::create_dir_all(&out_dir)
        .unwrap_or_else(|e| panic!("failed to create out dir {}: {e}", out_dir.display()));

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

    assert!(
        baseline_png.exists(),
        "missing baseline image at {}.\n\nTo (re)generate baseline locally:\n  cargo run -- --headless --dsl-json {} --output {}",
        baseline_png.display(),
        scene_json.display(),
        baseline_png.display()
    );

    let expected = load_rgba8(&baseline_png);
    let actual = load_rgba8(&out_png);

    assert_eq!(
        expected.dimensions(),
        actual.dimensions(),
        "image dimension mismatch: baseline={} output={}",
        baseline_png.display(),
        out_png.display()
    );

    let (mismatched_pixels, max_channel_delta) = diff_stats(&expected, &actual);
    assert_eq!(
        mismatched_pixels,
        0,
        "pixel diff detected: mismatched_pixels={mismatched_pixels}, max_channel_delta={max_channel_delta}\nbaseline={}\noutput={}",
        baseline_png.display(),
        out_png.display()
    );
}
