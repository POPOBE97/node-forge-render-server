use node_forge_render_server::{dsl, renderer, ui::resource_tree::ResourceSnapshot};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};

#[test]
fn instanced_math_closure_builds_and_reports_instance_count() {
    let scene = dsl::load_scene_from_path("tests/cases/instanced-math-closure/scene.json")
        .expect("load instanced-math-closure scene.json");

    let headless =
        HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("create headless renderer");

    let build = renderer::ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
        .with_adapter(headless.adapter.clone())
        .build(&scene)
        .expect("build shader space");

    let snapshot = ResourceSnapshot::capture(&build.shader_space, &build.pass_bindings, None);

    // Pass names are ResourceNames; for a RenderPass node this is typically `<id>.pass`.
    let pass = snapshot
        .passes
        .iter()
        .find(|p| p.name.contains("RenderPass_4"))
        .expect("find RenderPass_4 in snapshot");

    assert_eq!(
        pass.instance_count, 100,
        "expected instanced draw count to be derived from instance buffer"
    );
}
