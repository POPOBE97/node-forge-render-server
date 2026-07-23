use std::collections::HashSet;

use node_forge_render_server::renderer;
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};

mod support;

#[test]
fn downsample_output_target_deduplicates_compose_pass() {
    let (scene, assets) = support::load_render_case("bloom-nodes");

    let headless = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(err) => {
            eprintln!("No adapter available for downsample dedup test: {err:?}");
            return;
        }
    };
    if headless.adapter.get_info().backend == rust_wgpu_fiber::eframe::wgpu::Backend::Noop {
        eprintln!("Native GPU unavailable; skipping texture-backed downsample integration test");
        return;
    }

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
        !pass_ids.contains("sys.downsample.Downsample_10.to.Composite_5.compose.pass"),
        "expected redundant compose pass to be removed, got: {pass_ids:?}"
    );
}
