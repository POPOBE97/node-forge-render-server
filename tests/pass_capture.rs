use std::collections::HashMap;

use node_forge_render_server::{dsl, renderer};
use rust_wgpu_fiber::{
    HeadlessRenderer, HeadlessRendererConfig,
    pass::Pipeline,
    shader_space::{PASS_CAPTURE_OUTPUT_TEXTURE_NAME, PassCaptureMode, PassCaptureRequest},
};
use serde_json::json;

mod support;

fn compact_shared_target_scene() -> dsl::SceneDSL {
    let mut scene = support::load_render_case_scene("blend-two-passes");
    for node in &mut scene.nodes {
        match node.id.as_str() {
            "node_5" => {
                node.params.insert("width".into(), json!(64));
                node.params.insert("height".into(), json!(64));
            }
            "node_9" => {
                node.params
                    .insert("size".into(), json!({ "x": 64, "y": 64 }));
                node.params
                    .insert("position".into(), json!({ "x": 32, "y": 32 }));
            }
            "node_12" => {
                node.params.insert("width".into(), json!(64));
                node.params.insert("height".into(), json!(64));
            }
            "Vector2Input_16" => {
                node.params.insert("x".into(), json!(24));
                node.params.insert("y".into(), json!(24));
            }
            "Vector2Input_17" => {
                node.params.insert("x".into(), json!(32));
                node.params.insert("y".into(), json!(32));
            }
            _ => {}
        }
    }
    scene
}

#[test]
fn captures_individual_draw_states_for_passes_sharing_a_target() {
    let scene = compact_shared_target_scene();
    let (_, assets) = support::load_render_case("blend-two-passes");
    let headless = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(err) => {
            eprintln!("No adapter available for pass-capture test: {err:?}");
            return;
        }
    };
    if headless.adapter.get_info().backend == rust_wgpu_fiber::eframe::wgpu::Backend::Noop {
        eprintln!("Native GPU unavailable; skipping texture-backed pass-capture integration test");
        return;
    }
    let mut build =
        renderer::ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
            .with_adapter(headless.adapter.clone())
            .with_asset_store(assets)
            .build(&scene)
            .expect("build shared-target shader space");

    let mut execution = build.shader_space.composition.flatten();
    execution.reverse();
    let mut writers_by_target: HashMap<String, Vec<String>> = HashMap::new();
    for dependency in execution {
        let pass = build
            .shader_space
            .passes
            .inner
            .get(&dependency.pass_name)
            .expect("composition pass");
        if matches!(pass.pipeline, Pipeline::Render(_))
            && let Some(target) = pass.color_attachment.as_ref()
        {
            writers_by_target
                .entry(target.as_str().to_string())
                .or_default()
                .push(dependency.pass_name.as_str().to_string());
        }
    }
    let (shared_target, writers) = writers_by_target
        .into_iter()
        .find(|(_, writers)| writers.len() >= 2)
        .expect("two render passes writing one target");
    let first = writers.first().expect("first writer").clone();
    let second = writers.get(1).expect("second writer").clone();

    let capture = |shader_space: &mut rust_wgpu_fiber::shader_space::ShaderSpace,
                   pass_name: &str,
                   mode: PassCaptureMode| {
        let request = PassCaptureRequest::new(pass_name, mode);
        shader_space
            .prepare_pass_capture(&request)
            .expect("prepare pass capture");
        let _ = shader_space.render_profiled_with_pass_capture(true, &request);
        shader_space
            .read_texture_rgba8(PASS_CAPTURE_OUTPUT_TEXTURE_NAME)
            .expect("read pass capture")
            .bytes
    };

    let before_first = capture(&mut build.shader_space, &first, PassCaptureMode::Before);
    assert!(
        before_first.iter().all(|channel| *channel == 0),
        "Before a Clear-to-transparent first writer must expose the clear state"
    );

    let after_first = capture(&mut build.shader_space, &first, PassCaptureMode::After);
    let before_second = capture(&mut build.shader_space, &second, PassCaptureMode::Before);
    assert_eq!(
        after_first, before_second,
        "Before the second draw must equal After the first draw"
    );

    let after_second = capture(&mut build.shader_space, &second, PassCaptureMode::After);
    let final_target = build
        .shader_space
        .read_texture_rgba8(&shared_target)
        .expect("read final shared target")
        .bytes;
    assert_eq!(
        after_second, final_target,
        "After the last writer must equal the final shared target"
    );

    let solo_second = capture(&mut build.shader_space, &second, PassCaptureMode::Solo);
    assert_ne!(
        solo_second, after_second,
        "Solo must exclude pixels retained from the previous writer"
    );
    assert!(
        solo_second.chunks_exact(4).any(|pixel| pixel[3] != 0),
        "Solo capture should contain pixels drawn by the selected pass"
    );
}

#[test]
fn solo_capture_resolves_a_multisampled_pass_to_a_sampleable_texture() {
    let mut scene = compact_shared_target_scene();
    let pass_node = scene
        .nodes
        .iter_mut()
        .find(|node| node.id == "node_2")
        .expect("second render-pass node");
    pass_node.params.insert("msaaSampleCount".into(), json!(4));

    let (_, assets) = support::load_render_case("blend-two-passes");
    let headless = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(err) => {
            eprintln!("No adapter available for pass-capture MSAA test: {err:?}");
            return;
        }
    };
    if headless.adapter.get_info().backend == rust_wgpu_fiber::eframe::wgpu::Backend::Noop {
        eprintln!("Native GPU unavailable; skipping texture-backed MSAA capture integration test");
        return;
    }
    let mut build =
        renderer::ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
            .with_adapter(headless.adapter.clone())
            .with_asset_store(assets)
            .build(&scene)
            .expect("build multisampled scene");

    let mut execution = build.shader_space.composition.flatten();
    execution.reverse();
    let Some(pass_name) = execution.into_iter().find_map(|dependency| {
        let pass = build.shader_space.passes.inner.get(&dependency.pass_name)?;
        matches!(pass.pipeline, Pipeline::Render(_))
            .then_some(pass)
            .filter(|pass| pass.sample_count > 1)
            .map(|_| dependency.pass_name.as_str().to_string())
    }) else {
        eprintln!("Adapter downgraded the requested MSAA pass; skipping MSAA capture assertion");
        return;
    };

    let request = PassCaptureRequest::new(pass_name, PassCaptureMode::Solo);
    let info = build
        .shader_space
        .prepare_pass_capture(&request)
        .expect("prepare MSAA pass capture");
    assert_eq!(
        build
            .shader_space
            .textures
            .get(info.output_texture_name.as_str())
            .expect("capture output texture")
            .wgpu_texture_desc
            .sample_count,
        1,
        "capture output must be sampleable even when the pass is multisampled"
    );

    let _ = build
        .shader_space
        .render_profiled_with_pass_capture(true, &request);
    let solo = build
        .shader_space
        .read_texture_rgba8(PASS_CAPTURE_OUTPUT_TEXTURE_NAME)
        .expect("read resolved MSAA capture");
    assert!(
        solo.bytes.chunks_exact(4).any(|pixel| pixel[3] != 0),
        "resolved Solo capture should contain selected-pass pixels"
    );
}
