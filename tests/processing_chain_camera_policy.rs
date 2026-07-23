use std::collections::HashSet;

use node_forge_render_server::{
    asset_store::AssetStore,
    dsl,
    renderer::{self, camera::legacy_projection_camera_matrix},
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
use serde_json::json;

mod support;

fn matrices_approx_equal(lhs: &[f32; 16], rhs: &[f32; 16], epsilon: f32) -> bool {
    lhs.iter()
        .zip(rhs.iter())
        .all(|(l, r)| (*l - *r).abs() <= epsilon)
}

fn build_pass_bindings(
    scene: &dsl::SceneDSL,
    assets: AssetStore,
) -> Option<Vec<renderer::PassBindings>> {
    let headless = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(err) => {
            eprintln!("No adapter available for processing-chain camera policy test: {err:?}");
            return None;
        }
    };
    if headless.adapter.get_info().backend == rust_wgpu_fiber::eframe::wgpu::Backend::Noop {
        eprintln!("Native GPU unavailable; skipping texture-backed camera policy integration test");
        return None;
    }

    let build = renderer::ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
        .with_adapter(headless.adapter.clone())
        .with_asset_store(assets)
        .build(scene)
        .expect("build shader space");

    Some(build.pass_bindings)
}

fn add_perspective_camera(scene: &mut dsl::SceneDSL, to_node_id: &str) {
    let camera_node: dsl::Node = serde_json::from_value(json!({
        "id": "CustomPerspectiveCamera_1",
        "type": "PerspectiveCamera",
        "params": {
            "position": { "x": 196.0, "y": 435.0, "z": 820.0 },
            "target": { "x": 196.0, "y": 435.0, "z": 0.0 },
            "up": { "x": 0.0, "y": 1.0, "z": 0.0 },
            "fovY": 55.0,
            "aspect": 1.0,
            "near": 1.0,
            "far": 4000.0
        }
    }))
    .expect("deserialize custom perspective camera node");
    scene.nodes.push(camera_node);

    let camera_conn: dsl::Connection = serde_json::from_value(json!({
        "id": format!("edge_custom_camera_to_{to_node_id}"),
        "from": { "nodeId": "CustomPerspectiveCamera_1", "portId": "camera" },
        "to": { "nodeId": to_node_id, "portId": "camera" }
    }))
    .expect("deserialize camera connection");
    scene.connections.push(camera_conn);
}

#[test]
fn gaussian_blur_custom_camera_forces_source_pass_and_keeps_downstream_fullscreen() {
    let (base_scene, base_assets) = support::load_render_case("blur-guassian-20");

    let Some(base_bindings) = build_pass_bindings(&base_scene, base_assets) else {
        return;
    };
    let base_pass_ids: HashSet<&str> = base_bindings.iter().map(|b| b.pass_id.as_str()).collect();
    assert!(
        !base_pass_ids.contains("sys.blur.node_2.src.pass"),
        "default camera should keep source-pass bypass for this case, got: {base_pass_ids:?}"
    );

    let mut custom_scene = base_scene.clone();
    add_perspective_camera(&mut custom_scene, "node_2");

    let (_, custom_assets) = support::load_render_case("blur-guassian-20");
    let custom_bindings = build_pass_bindings(&custom_scene, custom_assets)
        .expect("adapter should still be available after first build");
    let custom_pass_ids: HashSet<&str> =
        custom_bindings.iter().map(|b| b.pass_id.as_str()).collect();
    assert!(
        custom_pass_ids.contains("sys.blur.node_2.src.pass"),
        "custom camera should force source pass, got: {custom_pass_ids:?}"
    );

    let src = custom_bindings
        .iter()
        .find(|b| b.pass_id == "sys.blur.node_2.src.pass")
        .expect("source pass binding");
    let src_fallback = legacy_projection_camera_matrix(src.base_params.target_size);
    assert!(
        !matrices_approx_equal(&src.base_params.camera, &src_fallback, 1e-5),
        "source pass should consume custom camera once"
    );

    let downstream = custom_bindings
        .iter()
        .find(|b| {
            b.pass_id.starts_with("sys.blur.node_2.") && b.pass_id != "sys.blur.node_2.src.pass"
        })
        .expect("downstream blur pass binding");
    let downstream_fallback = legacy_projection_camera_matrix(downstream.base_params.target_size);
    assert!(
        matrices_approx_equal(&downstream.base_params.camera, &downstream_fallback, 1e-5),
        "downstream blur passes should use fullscreen camera"
    );
}

#[test]
fn gradient_blur_custom_camera_forces_source_pass_and_keeps_downstream_fullscreen() {
    let (base_scene, base_assets) = support::load_render_case("blur-gradient");

    let Some(base_bindings) = build_pass_bindings(&base_scene, base_assets) else {
        return;
    };
    let base_pass_ids: HashSet<&str> = base_bindings.iter().map(|b| b.pass_id.as_str()).collect();
    assert!(
        !base_pass_ids.contains("sys.gb.GradientBlur_5.src.pass"),
        "default camera should keep source-pass bypass for this case, got: {base_pass_ids:?}"
    );

    let mut custom_scene = base_scene.clone();
    add_perspective_camera(&mut custom_scene, "GradientBlur_5");

    let (_, custom_assets) = support::load_render_case("blur-gradient");
    let custom_bindings = build_pass_bindings(&custom_scene, custom_assets)
        .expect("adapter should still be available after first build");
    let custom_pass_ids: HashSet<&str> =
        custom_bindings.iter().map(|b| b.pass_id.as_str()).collect();
    assert!(
        custom_pass_ids.contains("sys.gb.GradientBlur_5.src.pass"),
        "custom camera should force source pass, got: {custom_pass_ids:?}"
    );

    let src = custom_bindings
        .iter()
        .find(|b| b.pass_id == "sys.gb.GradientBlur_5.src.pass")
        .expect("gradient source pass binding");
    let src_fallback = legacy_projection_camera_matrix(src.base_params.target_size);
    assert!(
        !matrices_approx_equal(&src.base_params.camera, &src_fallback, 1e-5),
        "source pass should consume custom camera once"
    );

    let downstream = custom_bindings
        .iter()
        .find(|b| {
            b.pass_id.starts_with("sys.gb.GradientBlur_5.")
                && b.pass_id != "sys.gb.GradientBlur_5.src.pass"
        })
        .expect("downstream gradient pass binding");
    let downstream_fallback = legacy_projection_camera_matrix(downstream.base_params.target_size);
    assert!(
        matrices_approx_equal(&downstream.base_params.camera, &downstream_fallback, 1e-5),
        "downstream gradient passes should use fullscreen camera"
    );
}
