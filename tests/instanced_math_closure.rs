use node_forge_render_server::{dsl, renderer, ui::resource_tree::ResourceSnapshot};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};

#[test]
fn instanced_math_closure_builds_and_reports_instance_count() {
    let scene = dsl::load_scene_from_path("tests/cases/instanced-math-closure/scene.json")
        .expect("load instanced-math-closure scene.json");

    let headless = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(err) => {
            eprintln!("No adapter available for instanced math closure test: {err:?}");
            return;
        }
    };

    let build = renderer::ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
        .with_adapter(headless.adapter.clone())
        .build(&scene)
        .expect("build shader space");

    let snapshot = ResourceSnapshot::capture(&build.shader_space, &build.pass_bindings, None, None);

    // Pass display names can be normalized for readability; the generated
    // RenderPass suffix still keeps the source node number.
    let pass = snapshot
        .passes
        .iter()
        .find(|p| p.name.ends_with(".rpass4.pass"))
        .unwrap_or_else(|| {
            let pass_summary = snapshot
                .passes
                .iter()
                .map(|p| {
                    format!(
                        "{} -> {:?} instances={}",
                        p.name, p.target_texture, p.instance_count
                    )
                })
                .collect::<Vec<_>>();
            panic!("find RenderPass_4 pass in snapshot: {pass_summary:#?}");
        });

    assert_eq!(
        pass.instance_count, 100,
        "expected instanced draw count to be derived from instance buffer"
    );
}
