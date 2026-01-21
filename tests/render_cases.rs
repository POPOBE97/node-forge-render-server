use std::{
    path::{Path, PathBuf},
    process::Command,
};

use node_forge_render_server::renderer::validation;
use node_forge_render_server::{dsl, renderer};

#[derive(Clone, Debug)]
struct Case {
    name: &'static str,
    scene_json: &'static str,
    baseline_png: Option<&'static str>,
    expected_image_texture: Option<&'static str>,
}

fn default_baseline_png(case_name: &'static str) -> Option<&'static str> {
    // Convention: if tests/cases/<case>/baseline.png exists, use it.
    // Some cases intentionally don't have a baseline.
    match case_name {
        "colorspace-image" => None,
        // These currently shouldn't run in the suite; keep them skipped by default.
        // Use SKIP_RENDER_CASE sentinel file in the case directory.
        "coord-sanity" => None,
        "glass-node" => None,
        _ => Some("baseline.png"),
    }
}

fn default_expected_image_texture() -> Option<&'static str> {
    // If a case needs this, encode it explicitly via a custom test (or extend the generator).
    Some("ImageTexture_9")
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn cases_root() -> PathBuf {
    manifest_dir().join("tests").join("cases")
}

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

fn run_case(case: &Case) {
    let cases_root = cases_root();
    let case_dir = cases_root.join(case.name);

    let scene_json = cases_root.join(case.scene_json);
    assert!(
        scene_json.exists(),
        "case {}: missing scene json at {}",
        case.name,
        scene_json.display()
    );

    // (1) Base test: generate WGSL and ensure it validates.
    let scene = dsl::load_scene_from_path(&scene_json).unwrap_or_else(|e| {
        panic!(
            "case {}: failed to load scene {}: {e}",
            case.name,
            scene_json.display()
        )
    });

    let passes = renderer::build_all_pass_wgsl_bundles_from_scene(&scene)
        .unwrap_or_else(|e| panic!("case {}: failed to build WGSL bundles: {e}", case.name));

    let update_goldens = std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v != "0");
    let wgsl_dir = case_dir.join("wgsl");
    std::fs::create_dir_all(&wgsl_dir).unwrap_or_else(|e| {
        panic!(
            "case {}: failed to create wgsl dir {}: {e}",
            case.name,
            wgsl_dir.display()
        )
    });

    // (2) Golden align: compare generated WGSL vs golden.
    // We don't fail immediately on mismatch; we defer pass/fail until after image validation.
    let mut wgsl_golden_ok = true;

    for (pass_id, bundle) in &passes {
        validation::validate_wgsl_with_context(
            &bundle.module,
            &format!("case {}, pass {pass_id} (generated)", case.name),
        )
        .unwrap_or_else(|e| {
            panic!(
                "case {}, pass {pass_id}: GENERATED WGSL validation failed:\n{e:#}\nWGSL:\n{}",
                case.name, bundle.module
            )
        });

        let expected_vertex_path = wgsl_dir.join(format!("{pass_id}.vertex.wgsl"));
        let expected_fragment_path = wgsl_dir.join(format!("{pass_id}.fragment.wgsl"));
        let expected_module_path = wgsl_dir.join(format!("{pass_id}.module.wgsl"));

        if update_goldens {
            std::fs::write(&expected_vertex_path, &bundle.vertex).unwrap_or_else(|e| {
                panic!("case {}: write {:?}: {e}", case.name, expected_vertex_path)
            });
            std::fs::write(&expected_fragment_path, &bundle.fragment).unwrap_or_else(|e| {
                panic!(
                    "case {}: write {:?}: {e}",
                    case.name, expected_fragment_path
                )
            });
            std::fs::write(&expected_module_path, &bundle.module).unwrap_or_else(|e| {
                panic!("case {}: write {:?}: {e}", case.name, expected_module_path)
            });

            if let Some(compute) = &bundle.compute {
                let expected_compute_path = wgsl_dir.join(format!("{pass_id}.compute.wgsl"));
                std::fs::write(&expected_compute_path, compute).unwrap_or_else(|e| {
                    panic!("case {}: write {:?}: {e}", case.name, expected_compute_path)
                });
            }
        } else {
            let expected_vertex =
                std::fs::read_to_string(&expected_vertex_path).unwrap_or_else(|e| {
                    panic!(
                        "case {}: read {:?}: {e} (missing WGSL golden? run with UPDATE_GOLDENS=1)",
                        case.name, expected_vertex_path
                    )
                });
            let expected_fragment = std::fs::read_to_string(&expected_fragment_path)
                .unwrap_or_else(|e| {
                    panic!(
                        "case {}: read {:?}: {e} (missing WGSL golden? run with UPDATE_GOLDENS=1)",
                        case.name, expected_fragment_path
                    )
                });
            let expected_module =
                std::fs::read_to_string(&expected_module_path).unwrap_or_else(|e| {
                    panic!(
                        "case {}: read {:?}: {e} (missing WGSL golden? run with UPDATE_GOLDENS=1)",
                        case.name, expected_module_path
                    )
                });

            if bundle.vertex != expected_vertex {
                wgsl_golden_ok = false;
            }
            if bundle.fragment != expected_fragment {
                wgsl_golden_ok = false;
            }
            if bundle.module != expected_module {
                wgsl_golden_ok = false;
            }
        }

        if !update_goldens {
            if let Ok(golden_module) = std::fs::read_to_string(&expected_module_path) {
                validation::validate_wgsl_with_context(
                    &golden_module,
                    &format!("case {}, pass {pass_id} (golden)", case.name),
                )
                .unwrap_or_else(|e| {
                    panic!(
                        "case {}, pass {pass_id}: GOLDEN WGSL is invalid; regenerate with UPDATE_GOLDENS=1\nPATH: {}\nERROR:\n{e:#}",
                        case.name,
                        expected_module_path.display()
                    )
                });
            }
        }
    }

    // (2) Render headless and compare basic image properties
    let out_dir = case_dir.join("out");
    std::fs::create_dir_all(&out_dir).unwrap_or_else(|e| {
        panic!(
            "case {}: failed to create out dir {}: {e}",
            case.name,
            out_dir.display()
        )
    });

    // NOTE: Keep the baseline image immutable.
    // We always write headless render output to a separate file so developers can
    // manually inspect/copy it over the baseline if they choose.
    let out_png = out_dir.join("test-render-result.png");

    // (3) Ultimate ground truth: rendered output vs baseline.
    let mut image_ok = true;

    if let Some(baseline_rel) = case.baseline_png {
        let baseline_png = case_dir.join(baseline_rel);
        assert_ne!(
            baseline_png,
            out_png,
            "case {}: refusing to write render output over baseline image: {}",
            case.name,
            baseline_png.display()
        );
    }
    if out_png.exists() {
        std::fs::remove_file(&out_png).unwrap_or_else(|e| {
            panic!(
                "case {}: failed to remove old output {}: {e}",
                case.name,
                out_png.display()
            )
        });
    }

    let exe = env!("CARGO_BIN_EXE_node-forge-render-server");
    let output = Command::new(exe)
        .current_dir(&case_dir)
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
            "case {}: headless render failed (status={:?})\nstdout:\n{}\nstderr:\n{}",
            case.name,
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    assert!(
        out_png.exists(),
        "case {}: expected output image at {}, but it does not exist\nstdout:\n{}\nstderr:\n{}",
        case.name,
        out_png.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let meta = std::fs::metadata(&out_png).unwrap_or_else(|e| {
        panic!(
            "case {}: failed to stat output {}: {e}",
            case.name,
            out_png.display()
        )
    });
    assert!(
        meta.len() > 0,
        "case {}: output png is empty: {}",
        case.name,
        out_png.display()
    );

    let actual = load_rgba8(&out_png);

    if let Some(baseline_rel) = case.baseline_png {
        let baseline_png = case_dir.join(baseline_rel);
        assert!(
            baseline_png.exists(),
            "case {}: missing baseline image at {}",
            case.name,
            baseline_png.display()
        );

        let expected = load_rgba8(&baseline_png);

        if expected.dimensions() != actual.dimensions() {
            image_ok = false;
        }

        // (3) Compare pixel-by-pixel vs baseline
        let (mismatched_pixels, _max_channel_delta) = diff_stats(&expected, &actual);
        if mismatched_pixels != 0 {
            image_ok = false;
        }
    } else if let Some(node_id) = case.expected_image_texture {
        let node = scene
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .unwrap_or_else(|| panic!("case {}: missing expected ImageTexture node: {node_id}", case.name));
        assert_eq!(
            node.node_type, "ImageTexture",
            "case {}: expected_image_texture must refer to an ImageTexture node",
            case.name
        );

        let data_url = node
            .params
            .get("dataUrl")
            .and_then(|v| v.as_str())
            .or_else(|| node.params.get("data_url").and_then(|v| v.as_str()));
        let expected_img = if let Some(s) = data_url.filter(|s| !s.trim().is_empty()) {
            node_forge_render_server::renderer::utils::load_image_from_data_url(s)
                .unwrap_or_else(|e| panic!("case {}: failed to decode expected image dataUrl: {e}", case.name))
        } else {
            let path = node
                .params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("case {}: expected image node has no dataUrl/path", case.name));
            let cand = case_dir.join(path);
            image::open(&cand)
                .unwrap_or_else(|e| panic!("case {}: failed to open expected image {}: {e}", case.name, cand.display()))
        };

        let mut expected = expected_img.to_rgba8();

        // For ImageTexture, the runtime converts straight-alpha sources to premultiplied alpha
        // on the GPU (prepass). Mirror that here so we can validate the output.
        let alpha_mode = node
            .params
            .get("alphaMode")
            .and_then(|v| v.as_str())
            .unwrap_or("straight")
            .trim()
            .to_ascii_lowercase();
        if alpha_mode == "straight" {
            for p in expected.pixels_mut() {
                let a = p.0[3] as u16;
                p.0[0] = ((p.0[0] as u16 * a) / 255) as u8;
                p.0[1] = ((p.0[1] as u16 * a) / 255) as u8;
                p.0[2] = ((p.0[2] as u16 * a) / 255) as u8;
            }
        }

        if expected.dimensions() != actual.dimensions() {
            image_ok = false;
        }
        let (mismatched_pixels, max_channel_delta) = diff_stats(&expected, &actual);
        let _ = max_channel_delta;
        if mismatched_pixels != 0 {
            image_ok = false;
        }
    } else {
        // If there is no baseline/expected image, at least compare to prepared scene resolution.
        let prepared = renderer::scene_prep::prepare_scene(&scene)
            .unwrap_or_else(|e| panic!("case {}: failed to prepare scene: {e}", case.name));
        if actual.dimensions() != (prepared.resolution[0], prepared.resolution[1]) {
            image_ok = false;
        }
    }

    // Pass/fail logic:
    // - (1) WGSL generation/validation failures already panic -> red.
    // - (2) If WGSL golden passes and image fails -> red.
    // - (2) If WGSL golden passes and image passes -> green.
    // - (2) If WGSL golden fails and image passes -> green, auto-update goldens.
    // - (2) If WGSL golden fails and image fails -> red.
    if !wgsl_golden_ok {
        if image_ok {
            // Auto-update WGSL goldens to match current generated output.
            for (pass_id, bundle) in &passes {
                let expected_vertex_path = wgsl_dir.join(format!("{pass_id}.vertex.wgsl"));
                let expected_fragment_path = wgsl_dir.join(format!("{pass_id}.fragment.wgsl"));
                let expected_module_path = wgsl_dir.join(format!("{pass_id}.module.wgsl"));

                std::fs::write(&expected_vertex_path, &bundle.vertex).unwrap_or_else(|e| {
                    panic!("case {}: write {:?}: {e}", case.name, expected_vertex_path)
                });
                std::fs::write(&expected_fragment_path, &bundle.fragment).unwrap_or_else(|e| {
                    panic!("case {}: write {:?}: {e}", case.name, expected_fragment_path)
                });
                std::fs::write(&expected_module_path, &bundle.module).unwrap_or_else(|e| {
                    panic!("case {}: write {:?}: {e}", case.name, expected_module_path)
                });

                if let Some(compute) = &bundle.compute {
                    let expected_compute_path = wgsl_dir.join(format!("{pass_id}.compute.wgsl"));
                    std::fs::write(&expected_compute_path, compute).unwrap_or_else(|e| {
                        panic!("case {}: write {:?}: {e}", case.name, expected_compute_path)
                    });
                }
            }
        } else {
            panic!(
                "case {}: WGSL golden mismatch and image mismatch\noutput={}",
                case.name,
                out_png.display()
            );
        }
    } else if !image_ok {
        panic!("case {}: image mismatch\noutput={}", case.name, out_png.display());
    }
}

include!(concat!(env!("OUT_DIR"), "/generated_render_cases.rs"));
