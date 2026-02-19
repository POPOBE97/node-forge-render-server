use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use node_forge_render_server::{dsl, renderer};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
use serde_json::json;

fn can_run_headless() -> bool {
    HeadlessRenderer::new(HeadlessRendererConfig::default()).is_ok()
}

fn unique_temp_output(ext: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for tests")
        .as_nanos();
    std::env::temp_dir().join(format!("node-forge-hdr-export-{nonce}.{ext}"))
}

fn load_scene_with_output_format(format: &str) -> dsl::SceneDSL {
    let mut scene = dsl::load_scene_from_path("tests/cases/graph-rectangle/scene.json")
        .expect("load graph-rectangle scene");

    let composite_id = scene
        .outputs
        .as_ref()
        .and_then(|outputs| outputs.get("composite"))
        .cloned()
        .expect("scene.outputs.composite must exist");

    let target_texture_id = scene
        .connections
        .iter()
        .find(|conn| conn.to.node_id == composite_id && conn.to.port_id == "target")
        .map(|conn| conn.from.node_id.clone())
        .expect("Composite.target must be connected");

    let target_texture = scene
        .nodes
        .iter_mut()
        .find(|node| node.id == target_texture_id)
        .expect("target RenderTexture node must exist");
    target_texture
        .params
        .insert("format".to_string(), json!(format));

    scene
}

#[test]
fn hdr_headless_export_writes_exr_file_with_magic_bytes() {
    if !can_run_headless() {
        eprintln!("No adapter available; skipping hdr EXR export test.");
        return;
    }

    let scene = load_scene_with_output_format("rgba16float");
    let output_path = unique_temp_output("exr");
    let _ = fs::remove_file(&output_path);

    renderer::render_scene_to_file_headless(&scene, &output_path, None)
        .expect("HDR scene should export EXR");

    let bytes = fs::read(&output_path).expect("read EXR output");
    assert!(bytes.len() > 4, "EXR file should be non-empty");
    assert_eq!(&bytes[..4], &[0x76, 0x2f, 0x31, 0x01], "invalid EXR magic");

    let _ = fs::remove_file(&output_path);
}

#[test]
fn hdr_headless_export_to_png_path_is_rejected() {
    if !can_run_headless() {
        eprintln!("No adapter available; skipping hdr PNG rejection test.");
        return;
    }

    let scene = load_scene_with_output_format("rgba16float");
    let output_path = unique_temp_output("png");
    let _ = fs::remove_file(&output_path);

    let err = renderer::render_scene_to_file_headless(&scene, &output_path, None)
        .expect_err("HDR export to non-EXR path should fail");
    let msg = format!("{err:#}");
    assert!(msg.contains(".exr required"), "unexpected error: {msg}");
}
