use std::{collections::HashSet, path::Path};

use node_forge_render_server::{asset_store, dsl, renderer};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};

#[test]
fn downsample_output_target_deduplicates_compose_pass() {
    let scene_path = "tests/cases/back-pin-pin/scene.json";
    let scene = dsl::load_scene_from_path(scene_path).expect("load back-pin-pin scene");
    let scene_dir = Path::new(scene_path)
        .parent()
        .expect("scene path should have parent");

    let assets = asset_store::load_from_scene_dir(&scene, scene_dir)
        .expect("load assets for back-pin-pin scene");

    let headless = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(err) => {
            eprintln!("No adapter available for downsample dedup test: {err:?}");
            return;
        }
    };

    let build = renderer::ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
        .with_adapter(headless.adapter.clone())
        .with_asset_store(assets)
        .build(&scene)
        .expect("build shader space");

    let pass_ids: HashSet<&str> = build
        .pass_bindings
        .iter()
        .map(|binding| binding.pass_id.as_str())
        .collect();

    assert!(
        pass_ids.contains("sys.downsample.Downsample_10.pass"),
        "expected downsample pass to be present, got: {pass_ids:?}"
    );
    assert!(
        pass_ids.contains("sys.downsample.Downsample_10.upsample.pass"),
        "expected downsample upsample pass to be present, got: {pass_ids:?}"
    );
    assert!(
        !pass_ids.contains("sys.downsample.Downsample_10.to.Composite_5.compose.pass"),
        "expected redundant compose pass to be removed, got: {pass_ids:?}"
    );
}
