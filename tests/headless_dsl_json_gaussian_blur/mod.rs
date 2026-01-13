use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn load_rgba8(path: &Path) -> image::RgbaImage {
    image::open(path)
        .unwrap_or_else(|e| panic!("failed to open image {}: {e}", path.display()))
        .to_rgba8()
}

fn linear_u8_to_srgb_u8(v: u8) -> u8 {
    let c = (v as f32) / 255.0;
    let srgb = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (srgb.clamp(0.0, 1.0) * 255.0).round() as u8
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
fn headless_dsl_json_renders_and_matches_baseline_gaussian_blur_sigma60() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let test_dir = manifest_dir.join("tests/headless_dsl_json_gaussian_blur");
    let case_dir = test_dir.join("case");

    let scene_json = case_dir.join("simple-gaussian-blur-sigma-60.json");
    let baseline_png = case_dir.join("down4-sigma60.png");
    let out_png = test_dir.join("out/headless-out.png");

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

    // Extra diagnostics: if baseline is sRGB but output is linear (or vice versa),
    // the direct byte diff will show massive mismatch. Try an approximate conversion
    // on the output and show the stats to help decide the correct comparison.
    let mut actual_linear_to_srgb = actual.clone();
    for p in actual_linear_to_srgb.pixels_mut() {
        p.0[0] = linear_u8_to_srgb_u8(p.0[0]);
        p.0[1] = linear_u8_to_srgb_u8(p.0[1]);
        p.0[2] = linear_u8_to_srgb_u8(p.0[2]);
    }
    let (mm2, max2) = diff_stats(&expected, &actual_linear_to_srgb);

    assert_eq!(
        mismatched_pixels,
        0,
        "pixel diff detected: mismatched_pixels={mismatched_pixels}, max_channel_delta={max_channel_delta} (direct)\nlinear->sRGB on output: mismatched_pixels={mm2}, max_channel_delta={max2}\nbaseline={}\noutput={}",
        baseline_png.display(),
        out_png.display()
    );
}
