use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn load_rgba8(path: &Path) -> image::RgbaImage {
    image::open(path)
        .unwrap_or_else(|e| panic!("failed to open image {}: {e}", path.display()))
        .to_rgba8()
}

fn assert_u8_close(name: &str, got: u8, expected: u8, tol: u8) {
    let g = got as i16;
    let e = expected as i16;
    let d = (g - e).abs() as u8;
    assert!(
        d <= tol,
        "{name} mismatch: got={got} expected={expected} tol={tol}"
    );
}

#[test]
fn headless_pass_texture_outputs_premultiplied_alpha() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let scene_json = manifest_dir.join("tests/pass-texture-alpha/pass-texture-alpha.json");
    assert!(
        scene_json.exists(),
        "expected case to exist at {}",
        scene_json.display()
    );

    // Must be an absolute path (CLI enforces it).
    let out_png = manifest_dir.join("tests/pass-texture-alpha/out/headless-out.png");
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

    let img = load_rgba8(&out_png);
    assert_eq!(img.dimensions(), (64, 64));

    // Center pixel should be premultiplied RGBA: (1,0,0,0.5) -> (128,0,0,~128).
    let p = img.get_pixel(32, 32);
    let [r, g, b, a] = p.0;

    assert_u8_close("r", r, 128, 1);
    assert_u8_close("g", g, 0, 1);
    assert_u8_close("b", b, 0, 1);
    assert_u8_close("a", a, 128, 1);
}
