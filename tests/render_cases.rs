use std::path::{Path, PathBuf};

use node_forge_render_server::asset_store::AssetStore;
use node_forge_render_server::renderer::validation;
use node_forge_render_server::{dsl, renderer};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};

#[derive(Clone, Debug)]
struct Case {
    name: &'static str,
    scene_source: &'static str,
    baseline_png: Option<&'static str>,
    expected_image_texture: Option<&'static str>,
}

fn default_baseline_png(case_name: &'static str) -> Option<&'static str> {
    // Convention: if tests/cases/<case>/baseline.png exists, use it.
    // Some cases intentionally don't have a baseline.
    match case_name {
        // These currently shouldn't run in the suite; keep them skipped by default.
        // Use SKIP_RENDER_CASE sentinel file in the case directory.
        "coord-sanity" => None,
        "glass-node" => None,
        // No committed baseline yet; validate render succeeds + dimensions only.
        "2dsdf-bevel" => None,
        "glass-weather-temprature-widget" => None,
        "camera-mat4-pass-nodes" => None,
        // HDR (Rgba16Float) scenes — output is tone-mapped to sRGB for PNG.
        // No committed baseline yet; WGSL golden + render-success only.
        "bloom" => None,
        "bloom-nodes" => None,
        "hdr-bloom-nodes" => None,
        // This case previously validated output against the ImageTexture source.
        // It now uses baseline.png to avoid duplicating GPU sampling/interpolation details in tests.
        _ => Some("baseline.png"),
    }
}

fn default_expected_image_texture(case_name: &'static str) -> Option<&'static str> {
    // Most cases validate against a baseline.png. Some cases are easier to validate by
    // comparing the output against the input ImageTexture bytes (with the same alpha-mode
    // semantics as runtime).
    match case_name {
        _ => None,
    }
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

fn crop_rgba8(img: &image::RgbaImage, x: u32, y: u32, w: u32, h: u32) -> image::RgbaImage {
    let mut out = image::RgbaImage::new(w, h);
    for oy in 0..h {
        for ox in 0..w {
            let p = *img.get_pixel(x + ox, y + oy);
            out.put_pixel(ox, oy, p);
        }
    }
    out
}

fn resize_nearest_rgba8(src: &image::RgbaImage, w: u32, h: u32) -> image::RgbaImage {
    // Mirror wgpu's nearest sampling semantics closely enough for tests.
    // Texel centers are at (i + 0.5) / src_w; choose the nearest center.
    // NOTE: Our rect geometry uses UVs authored at vertices (0..1) and rasterization interpolates
    // those endpoints. For a target of size W, pixel i maps more closely to i/(W-1) than to
    // (i+0.5)/W, so we use endpoint-aligned UVs here to match runtime.
    let sw = src.width().max(1);
    let sh = src.height().max(1);

    let mut out = image::RgbaImage::new(w, h);
    for oy in 0..h {
        let v = if h <= 1 {
            0.0
        } else {
            (oy as f32) / ((h - 1) as f32)
        };
        let sy = ((v * (sh as f32)) - 0.5)
            .floor()
            .clamp(0.0, (sh - 1) as f32) as u32;
        for ox in 0..w {
            let u = if w <= 1 {
                0.0
            } else {
                (ox as f32) / ((w - 1) as f32)
            };
            let sx = ((u * (sw as f32)) - 0.5)
                .floor()
                .clamp(0.0, (sw - 1) as f32) as u32;
            out.put_pixel(ox, oy, *src.get_pixel(sx, sy));
        }
    }
    out
}

fn srgb_u8_to_linear_f32(x: u8) -> f32 {
    let s = (x as f32) / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_f32_to_srgb_u8(x: f32) -> u8 {
    let x = x.clamp(0.0, 1.0);
    let s = if x <= 0.0031308 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

fn first_pixel_mismatch(
    expected: &image::RgbaImage,
    actual: &image::RgbaImage,
) -> Option<(u32, u32, image::Rgba<u8>, image::Rgba<u8>)> {
    if expected.dimensions() != actual.dimensions() {
        return None;
    }
    let w = expected.width() as usize;
    for (i, (ep, ap)) in expected.pixels().zip(actual.pixels()).enumerate() {
        if ep.0 != ap.0 {
            let x = (i % w) as u32;
            let y = (i / w) as u32;
            return Some((x, y, *ep, *ap));
        }
    }
    None
}

/// Load an image from a scene node, trying assetId → asset_store, then dataUrl, then path.
fn load_image_from_node(
    node: &dsl::Node,
    case_dir: &Path,
    asset_store: &AssetStore,
    case_name: &str,
) -> image::DynamicImage {
    // 1) Try assetId → asset_store
    if let Some(asset_id) = node.params.get("assetId").and_then(|v| v.as_str()) {
        if !asset_id.is_empty() {
            if let Some(img) = asset_store.load_image(asset_id).unwrap_or_else(|e| {
                panic!(
                    "case {case_name}: failed to load asset {asset_id} for node {}: {e}",
                    node.id
                )
            }) {
                return img;
            }
        }
    }
    // 2) Fallback: dataUrl
    let data_url = node
        .params
        .get("dataUrl")
        .and_then(|v| v.as_str())
        .or_else(|| node.params.get("data_url").and_then(|v| v.as_str()));
    if let Some(s) = data_url.filter(|s| !s.trim().is_empty()) {
        return node_forge_render_server::renderer::utils::load_image_from_data_url(s)
            .unwrap_or_else(|e| {
                panic!(
                    "case {case_name}: failed to decode dataUrl for node {}: {e}",
                    node.id
                )
            });
    }
    // 3) Fallback: path
    let path = node
        .params
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            panic!(
                "case {case_name}: node {} has no assetId/dataUrl/path",
                node.id
            )
        });
    let cand = case_dir.join(path);
    image::open(&cand).unwrap_or_else(|e| {
        panic!(
            "case {case_name}: failed to open image {}: {e}",
            cand.display()
        )
    })
}

/// Load a 32-bit float RGBA EXR image, returning (width, height, channels_f32).
fn load_exr_f32(path: &Path) -> (u32, u32, Vec<f32>) {
    let img = image::open(path)
        .unwrap_or_else(|e| panic!("failed to open EXR {}: {e}", path.display()));
    let rgba32f = img.to_rgba32f();
    let w = rgba32f.width();
    let h = rgba32f.height();
    let channels: Vec<f32> = rgba32f.into_raw();
    (w, h, channels)
}

/// Compare two EXR images channel-by-channel with a small epsilon tolerance.
/// Panics on mismatch.
fn compare_exr_baseline(
    baseline_path: &Path,
    actual_path: &Path,
    case_name: &str,
    label: &str,
) {
    if !compare_exr_baseline_soft(baseline_path, actual_path, case_name, label) {
        panic!(
            "case {case_name}: {label} EXR baseline mismatch\nbaseline={}\nactual={}",
            baseline_path.display(),
            actual_path.display(),
        );
    }
}

/// Compare two EXR images channel-by-channel. Returns false on mismatch (does not panic).
fn compare_exr_baseline_soft(
    baseline_path: &Path,
    actual_path: &Path,
    case_name: &str,
    label: &str,
) -> bool {
    let (ew, eh, expected_ch) = load_exr_f32(baseline_path);
    let (aw, ah, actual_ch) = load_exr_f32(actual_path);
    if (ew, eh) != (aw, ah) {
        eprintln!(
            "case {case_name}: {label} EXR dimension mismatch expected={ew}x{eh} actual={aw}x{ah}"
        );
        return false;
    }
    // Allow a small epsilon for floating-point precision differences.
    const EPS: f32 = 1.0 / 512.0;
    let total_pixels = (ew as u64) * (eh as u64);
    let mut mismatched: u64 = 0;
    let mut max_delta: f32 = 0.0;
    let mut first_mismatch: Option<(u32, u32, [f32; 4], [f32; 4])> = None;
    for i in 0..(total_pixels as usize) {
        let base = i * 4;
        let mut any = false;
        for c in 0..4 {
            let d = (expected_ch[base + c] - actual_ch[base + c]).abs();
            if d > EPS {
                any = true;
                max_delta = max_delta.max(d);
            }
        }
        if any {
            mismatched += 1;
            if first_mismatch.is_none() {
                let x = (i as u32) % ew;
                let y = (i as u32) / ew;
                let ep = [
                    expected_ch[base],
                    expected_ch[base + 1],
                    expected_ch[base + 2],
                    expected_ch[base + 3],
                ];
                let ap = [
                    actual_ch[base],
                    actual_ch[base + 1],
                    actual_ch[base + 2],
                    actual_ch[base + 3],
                ];
                first_mismatch = Some((x, y, ep, ap));
            }
        }
    }
    if mismatched > 0 {
        if let Some((x, y, ep, ap)) = first_mismatch {
            eprintln!(
                "case {case_name}: {label} EXR baseline mismatch: {mismatched} pixels differ, max_delta={max_delta:.6}, first at ({x},{y}) expected={ep:?} actual={ap:?}"
            );
        }
        return false;
    }
    true
}

/// Compare two RGBA8 images pixel-by-pixel. Panics on mismatch.
fn compare_rgba8_baseline(
    expected: &image::RgbaImage,
    actual: &image::RgbaImage,
    case_name: &str,
    label: &str,
    expected_path: &Path,
    actual_path: &Path,
) {
    if expected.dimensions() != actual.dimensions() {
        panic!(
            "case {case_name}: {label} dimension mismatch expected={}x{} actual={}x{}\nbaseline={}\nactual={}",
            expected.width(), expected.height(),
            actual.width(), actual.height(),
            expected_path.display(), actual_path.display(),
        );
    }
    let (mismatched_pixels, max_channel_delta) = diff_stats(expected, actual);
    if mismatched_pixels != 0 {
        let mismatch_detail = first_pixel_mismatch(expected, actual)
            .map(|(x, y, ep, ap)| {
                format!(
                    "first at ({x},{y}) expected={:?} actual={:?}",
                    ep.0, ap.0
                )
            })
            .unwrap_or_else(|| "(unknown)".to_string());
        panic!(
            "case {case_name}: {label} baseline mismatch: {mismatched_pixels} pixels differ, max_delta={max_channel_delta}, {mismatch_detail}\nbaseline={}\nactual={}",
            expected_path.display(), actual_path.display(),
        );
    }
}

fn run_case(case: &Case) {
    let cases_root = cases_root();
    let case_dir = cases_root.join(case.name);

    let scene_source = cases_root.join(case.scene_source);
    assert!(
        scene_source.exists(),
        "case {}: missing scene source at {}",
        case.name,
        scene_source.display()
    );

    // Load scene + assets based on source type (.nforge or .json)
    let (scene, asset_store) = if scene_source.extension().is_some_and(|e| e == "nforge") {
        let (s, store) = node_forge_render_server::asset_store::load_from_nforge(&scene_source)
            .unwrap_or_else(|e| {
                panic!(
                    "case {}: failed to load .nforge {}: {e}",
                    case.name,
                    scene_source.display()
                )
            });
        (s, store)
    } else {
        let s = dsl::load_scene_from_path(&scene_source).unwrap_or_else(|e| {
            panic!(
                "case {}: failed to load scene {}: {e}",
                case.name,
                scene_source.display()
            )
        });
        // Load assets from the scene directory (assets/ subfolder via scene.assets manifest)
        let store = node_forge_render_server::asset_store::load_from_scene_dir(
            &s,
            scene_source.parent().unwrap_or_else(|| Path::new(".")),
        )
        .unwrap_or_else(|e| {
            panic!(
                "case {}: failed to load assets from scene dir: {e}",
                case.name
            )
        });
        (s, store)
    };

    let passes =
        renderer::build_all_pass_wgsl_bundles_from_scene_with_assets(&scene, Some(&asset_store))
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

    // Always dump generated WGSL to out/ for inspection (separate from goldens in wgsl/).
    let out_wgsl_dir = out_dir.join("wgsl");
    std::fs::create_dir_all(&out_wgsl_dir).unwrap_or_else(|e| {
        panic!(
            "case {}: failed to create out wgsl dir {}: {e}",
            case.name,
            out_wgsl_dir.display()
        )
    });
    for (pass_id, bundle) in &passes {
        std::fs::write(
            out_wgsl_dir.join(format!("{pass_id}.vertex.wgsl")),
            &bundle.vertex,
        )
        .unwrap_or_else(|e| panic!("case {}: write out vertex wgsl: {e}", case.name));
        std::fs::write(
            out_wgsl_dir.join(format!("{pass_id}.fragment.wgsl")),
            &bundle.fragment,
        )
        .unwrap_or_else(|e| panic!("case {}: write out fragment wgsl: {e}", case.name));
        std::fs::write(
            out_wgsl_dir.join(format!("{pass_id}.module.wgsl")),
            &bundle.module,
        )
        .unwrap_or_else(|e| panic!("case {}: write out module wgsl: {e}", case.name));
        if let Some(compute) = &bundle.compute {
            std::fs::write(
                out_wgsl_dir.join(format!("{pass_id}.compute.wgsl")),
                compute,
            )
            .unwrap_or_else(|e| panic!("case {}: write out compute wgsl: {e}", case.name));
        }
    }

    // Detect animated scenes (TimeInput / Time nodes) early so we can
    // adjust baseline naming: animated → baseline_0.{ext}, static → baseline.{ext}.
    let uses_time = scene
        .nodes
        .iter()
        .any(|n| matches!(n.node_type.as_str(), "TimeInput" | "Time"));

    // (3) Ultimate ground truth: rendered output vs baseline.
    let mut image_ok = true;

    // Render in-process so we can dump intermediate textures for all cases.
    #[allow(unused_assignments)]
    let mut output_is_hdr = false;
    let out_result;
    {
        let headless =
            HeadlessRenderer::new(HeadlessRendererConfig::default()).unwrap_or_else(|e| {
                panic!(
                    "case {}: failed to create headless renderer: {e}",
                    case.name
                )
            });

        // Enable WGSL dump to out/wgsl_dump for debugging shader issues.
        let wgsl_dump_dir = out_dir.join("wgsl_dump");
        let build_options = renderer::ShaderSpaceBuildOptions {
            debug_dump_wgsl_dir: Some(wgsl_dump_dir),
            ..Default::default()
        };

        let build =
            renderer::ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
                .with_adapter(headless.adapter.clone())
                .with_options(build_options)
                .with_asset_store(asset_store.clone())
                .build(&scene)
                .unwrap_or_else(|e| {
                    panic!("case {}: failed to build shader space: {e:#}", case.name)
                });
        let shader_space = build.shader_space;
        let output_texture_name = build.scene_output_texture;
        let pass_bindings = build.pass_bindings;

        // Detect HDR output to choose EXR vs PNG for frame dumps and baselines.
        output_is_hdr = shader_space
            .texture_info(output_texture_name.as_str())
            .map(|info| {
                info.format == rust_wgpu_fiber::eframe::wgpu::TextureFormat::Rgba16Float
            })
            .unwrap_or(false);
        let output_ext = if output_is_hdr { "exr" } else { "png" };

        // NOTE: Keep baseline images immutable.
        // We always write headless render output to a separate file so developers
        // can manually inspect/copy it over the baseline if they choose.
        out_result = out_dir.join(format!("test-render-result.{output_ext}"));
        if out_result.exists() {
            std::fs::remove_file(&out_result).unwrap_or_else(|e| {
                panic!(
                    "case {}: failed to remove old output {}: {e}",
                    case.name,
                    out_result.display()
                )
            });
        }

        shader_space.render();

        let save_and_load = |tex_name: &str, out_path: &Path| -> image::RgbaImage {
            shader_space
                .save_texture_png(tex_name, out_path)
                .unwrap_or_else(|e| {
                    panic!(
                        "case {}: failed to save texture {tex_name} to {}: {e}",
                        case.name,
                        out_path.display()
                    )
                });
            load_rgba8(out_path)
        };

        let compare_stage = |label: &str,
                             tex_name: &str,
                             expected_path: &Path,
                             flip_y_actual: bool| {
            let out_path = out_dir.join(format!("stage_{label}.png"));
            let mut actual_img = save_and_load(tex_name, &out_path);
            if flip_y_actual {
                image::imageops::flip_vertical_in_place(&mut actual_img);
            }

            let expected_img = load_rgba8(expected_path);
            if expected_img.dimensions() != actual_img.dimensions() {
                panic!(
                    "case {}: {label} dimension mismatch expected={}x{} actual={}x{}\nexpected={}\nactual={}",
                    case.name,
                    expected_img.width(),
                    expected_img.height(),
                    actual_img.width(),
                    actual_img.height(),
                    expected_path.display(),
                    out_path.display(),
                );
            }

            let (mismatched_pixels, max_channel_delta) = diff_stats(&expected_img, &actual_img);
            if mismatched_pixels != 0 {
                let mismatch_detail = first_pixel_mismatch(&expected_img, &actual_img)
                    .map(|(x, y, ep, ap)| {
                        format!(
                            "first_mismatch=({x},{y}) expected_rgba={:?} actual_rgba={:?}",
                            ep.0, ap.0
                        )
                    })
                    .unwrap_or_else(|| "first_mismatch=(unknown)".to_string());
                panic!(
                    "case {}: {label} image mismatch mismatched_pixels={mismatched_pixels} max_channel_delta={max_channel_delta} {mismatch_detail}\nexpected={}\nactual={}",
                    case.name,
                    expected_path.display(),
                    out_path.display(),
                );
            }
        };

        // (a-?) Optional intermediate stages.
        // Always dump the final result, even if an intermediate stage fails.
        let dump_final = || {
            if output_is_hdr {
                shader_space
                    .save_texture_exr(output_texture_name.as_str(), &out_result)
                    .unwrap_or_else(|e| {
                        panic!(
                            "case {}: failed to save final texture {} to {}: {e}",
                            case.name,
                            output_texture_name.as_str(),
                            out_result.display()
                        )
                    });
            } else {
                shader_space
                    .save_texture_png(output_texture_name.as_str(), &out_result)
                    .unwrap_or_else(|e| {
                        panic!(
                            "case {}: failed to save final texture {} to {}: {e}",
                            case.name,
                            output_texture_name.as_str(),
                            out_result.display()
                        )
                    });
            }
        };

        let staged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // If this case includes a known premultiply stage, dump it.
            // Don't validate it against CPU expected unless the case provides a baseline.
            if case.name == "simple-guassian-blur" {
                // NOTE: flip actual in Y for visual consistency with other stage dumps.
                let premultiply_out = out_dir.join("stage_premultiply.png");
                let mut premultiplied_actual = save_and_load("node_7", &premultiply_out);
                image::imageops::flip_vertical_in_place(&mut premultiplied_actual);

                let node = scene
                    .nodes
                    .iter()
                    .find(|n| n.id == "node_7")
                    .unwrap_or_else(|| {
                        panic!("case {}: missing ImageTexture node node_7", case.name)
                    });
                let expected_img = load_image_from_node(node, &case_dir, &asset_store, case.name);
                let mut premultiplied_expected = expected_img.to_rgba8();
                for p in premultiplied_expected.pixels_mut() {
                    let a = p.0[3] as u16;
                    p.0[0] = ((p.0[0] as u16 * a) / 255) as u8;
                    p.0[1] = ((p.0[1] as u16 * a) / 255) as u8;
                    p.0[2] = ((p.0[2] as u16 * a) / 255) as u8;
                }
                if premultiplied_expected.dimensions() != premultiplied_actual.dimensions() {
                    panic!(
                        "case {}: premultiply dimension mismatch expected={}x{} actual={}x{}\nactual={}",
                        case.name,
                        premultiplied_expected.width(),
                        premultiplied_expected.height(),
                        premultiplied_actual.width(),
                        premultiplied_actual.height(),
                        premultiply_out.display(),
                    );
                }
                let (mismatched_pixels, max_channel_delta) =
                    diff_stats(&premultiplied_expected, &premultiplied_actual);
                if mismatched_pixels != 0 {
                    panic!(
                        "case {}: premultiply mismatch mismatched_pixels={mismatched_pixels} max_channel_delta={max_channel_delta}\nactual={}",
                        case.name,
                        premultiply_out.display(),
                    );
                }

                compare_stage(
                    "downsample_8",
                    "sys.blur.node_2.ds.8",
                    &case_dir.join("baseline_downsample_8.png"),
                    false,
                );
                compare_stage(
                    "downsample_2",
                    "sys.blur.node_2.ds.2",
                    &case_dir.join("baseline_downsample_2.png"),
                    false,
                );
                compare_stage(
                    "blur_h",
                    "sys.blur.node_2.h",
                    &case_dir.join("baseline_blur_h.png"),
                    false,
                );
                compare_stage(
                    "blur_v",
                    "sys.blur.node_2.v",
                    &case_dir.join("baseline_blur_v.png"),
                    false,
                );
            } else {
                // Dump all dumpable internal textures to stage_*.png.
                // If a matching baseline exists (baseline_<name>.png), validate it pixel-perfect.
                for tex_name in shader_space.list_debug_texture_names() {
                    let safe = tex_name
                        .replace('/', "_")
                        .replace("\\", "_")
                        .replace(':', "_")
                        .replace(' ', "_");
                    let expected_path = case_dir.join(format!("baseline_{safe}.png"));
                    if expected_path.exists() {
                        compare_stage(safe.as_str(), tex_name.as_str(), &expected_path, false);
                    } else {
                        let out_path = out_dir.join(format!("stage_{safe}.png"));
                        let _ = save_and_load(tex_name.as_str(), &out_path);
                    }
                }
            }
        }));

        dump_final();

        // (b) Multi-frame animation rendering for scenes that use time.
        // Render 10 frames evenly spanning 10 seconds and save each frame.
        // If a baseline_N.{ext} exists, compare against it.
        if uses_time {
            const ANIM_FRAME_COUNT: usize = 10;
            const ANIM_DURATION_SECS: f32 = 10.0;
            for frame_idx in 0..ANIM_FRAME_COUNT {
                let t = if ANIM_FRAME_COUNT <= 1 {
                    0.0
                } else {
                    ANIM_DURATION_SECS * (frame_idx as f32) / ((ANIM_FRAME_COUNT - 1) as f32)
                };
                // Update time uniform on every pass and re-render.
                for pb in &pass_bindings {
                    let mut p = pb.base_params;
                    p.time = t;
                    renderer::update_pass_params(&shader_space, pb, &p).unwrap_or_else(|e| {
                        panic!(
                            "case {}: failed to update pass params for frame {frame_idx} t={t}: {e}",
                            case.name
                        )
                    });
                }
                shader_space.render();
                let frame_path = out_dir.join(format!("frame_{frame_idx}.{output_ext}"));
                if output_is_hdr {
                    shader_space
                        .save_texture_exr(output_texture_name.as_str(), &frame_path)
                        .unwrap_or_else(|e| {
                            panic!(
                                "case {}: failed to save animation frame {frame_idx} (t={t}s) to {}: {e}",
                                case.name,
                                frame_path.display()
                            )
                        });
                } else {
                    shader_space
                        .save_texture_png(output_texture_name.as_str(), &frame_path)
                        .unwrap_or_else(|e| {
                            panic!(
                                "case {}: failed to save animation frame {frame_idx} (t={t}s) to {}: {e}",
                                case.name,
                                frame_path.display()
                            )
                        });
                }

                // Compare against per-frame baseline if it exists.
                let baseline_frame = case_dir.join(format!("baseline_{frame_idx}.{output_ext}"));
                if baseline_frame.exists() {
                    if output_is_hdr {
                        compare_exr_baseline(
                            &baseline_frame,
                            &frame_path,
                            case.name,
                            &format!("frame_{frame_idx}"),
                        );
                    } else {
                        let expected = load_rgba8(&baseline_frame);
                        let actual = load_rgba8(&frame_path);
                        compare_rgba8_baseline(
                            &expected,
                            &actual,
                            case.name,
                            &format!("frame_{frame_idx}"),
                            &baseline_frame,
                            &frame_path,
                        );
                    }
                }
            }
        }

        if let Err(payload) = staged {
            std::panic::resume_unwind(payload);
        }
    }

    let meta = std::fs::metadata(&out_result).unwrap_or_else(|e| {
        panic!(
            "case {}: failed to stat output {}: {e}",
            case.name,
            out_result.display()
        )
    });
    assert!(
        meta.len() > 0,
        "case {}: output is empty: {}",
        case.name,
        out_result.display()
    );

    // Resolve the baseline path for the initial render:
    //   animated scenes → baseline_0.{ext}
    //   static  scenes → baseline.{ext}  (from case.baseline_png, typically "baseline.png")
    let initial_baseline = if uses_time {
        let ext = if output_is_hdr { "exr" } else { "png" };
        let p = case_dir.join(format!("baseline_0.{ext}"));
        if p.exists() { Some(p) } else { None }
    } else if let Some(baseline_rel) = case.baseline_png {
        // For static scenes, also check for the HDR variant first.
        if output_is_hdr {
            let exr = case_dir.join(baseline_rel).with_extension("exr");
            if exr.exists() {
                Some(exr)
            } else {
                let png = case_dir.join(baseline_rel);
                if png.exists() { Some(png) } else { None }
            }
        } else {
            let p = case_dir.join(baseline_rel);
            if p.exists() { Some(p) } else { None }
        }
    } else {
        None
    };

    if let Some(baseline_path) = initial_baseline {
        if output_is_hdr {
            let ok = compare_exr_baseline_soft(
                &baseline_path,
                &out_result,
                case.name,
                "initial",
            );
            if !ok {
                image_ok = false;
            }
        } else {
            let expected = load_rgba8(&baseline_path);
            let actual = load_rgba8(&out_result);

            if expected.dimensions() != actual.dimensions() {
                image_ok = false;
                eprintln!(
                    "case {}: dimension mismatch expected={}x{} actual={}x{}",
                    case.name,
                    expected.width(),
                    expected.height(),
                    actual.width(),
                    actual.height()
                );
            } else {
                let (mismatched_pixels, max_channel_delta) = diff_stats(&expected, &actual);
                if mismatched_pixels != 0 {
                    image_ok = false;
                    if let Some((x, y, ep, ap)) = first_pixel_mismatch(&expected, &actual) {
                        eprintln!(
                            "case {}: baseline mismatch: {} pixels differ, max_delta={}, first at ({},{}) expected={:?} actual={:?}",
                            case.name, mismatched_pixels, max_channel_delta, x, y, ep.0, ap.0
                        );
                    }
                }
            }
        }
    } else if let Some(node_id) = case.expected_image_texture {
        let mut actual = load_rgba8(&out_result);
        let node = scene
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .unwrap_or_else(|| {
                panic!(
                    "case {}: missing expected ImageTexture node: {node_id}",
                    case.name
                )
            });
        assert_eq!(
            node.node_type, "ImageTexture",
            "case {}: expected_image_texture must refer to an ImageTexture node",
            case.name
        );

        let data_url = None::<&str>; // legacy — kept for reference; actual loading below
        let _ = data_url;
        let expected_img = load_image_from_node(node, &case_dir, &asset_store, case.name);

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
                let a = (p.0[3] as f32) / 255.0;
                let r = srgb_u8_to_linear_f32(p.0[0]) * a;
                let g = srgb_u8_to_linear_f32(p.0[1]) * a;
                let b = srgb_u8_to_linear_f32(p.0[2]) * a;
                p.0[0] = linear_f32_to_srgb_u8(r);
                p.0[1] = linear_f32_to_srgb_u8(g);
                p.0[2] = linear_f32_to_srgb_u8(b);
            }
        }

        if case.name == "dyn-rect-image-texture" {
            let crop_w: u32 = 200;
            let crop_h: u32 = 120;
            let x0: u32 = 28;
            let y0_from_bottom: u32 = 68;
            let y0: u32 = actual.height().saturating_sub(y0_from_bottom + crop_h);

            actual = crop_rgba8(&actual, x0, y0, crop_w, crop_h);
            expected = resize_nearest_rgba8(&expected, crop_w, crop_h);
        }

        if expected.dimensions() != actual.dimensions() {
            image_ok = false;
        }
        let (mismatched_pixels, max_channel_delta) = diff_stats(&expected, &actual);
        if mismatched_pixels != 0 {
            if let Some((x, y, ep, ap)) = first_pixel_mismatch(&expected, &actual) {
                eprintln!(
                    "case {}: first mismatch at ({},{}): expected={:?} actual={:?} max_channel_delta={}",
                    case.name, x, y, ep.0, ap.0, max_channel_delta
                );
            }
        }
        if mismatched_pixels != 0 {
            image_ok = false;
        }
    } else if !uses_time {
        // If there is no baseline/expected image and it's static,
        // at least compare to prepared scene resolution.
        let actual = load_rgba8(&out_result);
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
            }
        } else {
            panic!(
                "case {}: WGSL golden mismatch and image mismatch\noutput={}",
                case.name,
                out_result.display()
            );
        }
    } else if !image_ok {
        panic!(
            "case {}: image mismatch\noutput={}",
            case.name,
            out_result.display()
        );
    }
}

include!(concat!(env!("OUT_DIR"), "/generated_render_cases.rs"));
