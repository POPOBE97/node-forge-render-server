use node_forge_render_server::renderer;
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig, eframe::wgpu};

mod support;

#[test]
fn live_gpu_profiles_are_bounded_reusable_and_keep_submission_identity() {
    let headless = match HeadlessRenderer::new(HeadlessRendererConfig {
        backends: wgpu::Backends::all() - wgpu::Backends::NOOP,
        ..Default::default()
    }) {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("Native GPU unavailable: {error}");
            return;
        }
    };
    if !headless
        .device
        .features()
        .contains(wgpu::Features::TIMESTAMP_QUERY)
    {
        let profile = headless.shader_space.render_profiled_async(None);
        assert!(!profile.timestamp_query_used);
        assert!(profile.frame_gpu_duration_ms.is_none());
        assert_eq!(headless.shader_space.live_profile_status(), (0, 0));
        return;
    }
    let (scene, assets) = support::load_render_case("blend-two-passes");
    let build = renderer::ShaderSpaceBuilder::new(headless.device.clone(), headless.queue.clone())
        .with_adapter(headless.adapter.clone())
        .with_asset_store(assets)
        .build(&scene)
        .expect("build profiled scene");
    let space = &build.shader_space;
    for round in 0..10 {
        let mut submitted = Vec::new();
        for _ in 0..3 {
            let profile = space.render_profiled_async(None);
            assert!(profile.timestamp_query_used);
            assert!(
                profile.queue_wait_ms.is_none(),
                "live path must not wait for GPU"
            );
            assert!(
                profile.frame_gpu_duration_ms.is_none(),
                "GPU result must arrive separately"
            );
            submitted.push(profile);
        }
        let skipped = space.render_profiled_async(None);
        assert!(!skipped.timestamp_query_used);
        assert_eq!(space.live_profile_status(), (3, round + 1));
        // Only the test waits. Interactive sampling always polls without waiting.
        headless
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        let mut completed = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while completed.len() < 3 && std::time::Instant::now() < deadline {
            completed.extend(space.take_completed_render_profiles());
            headless
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(std::time::Duration::from_secs(5)),
                })
                .unwrap();
        }
        assert_eq!(completed.len(), 3);
        assert_eq!(space.live_profile_status(), (0, round + 1));
        for (cpu, gpu) in submitted.iter().zip(&completed) {
            assert_eq!(cpu.frame_cpu_encode_ms, gpu.frame_cpu_encode_ms);
            assert_eq!(cpu.submit_cpu_ms, gpu.submit_cpu_ms);
            assert!(gpu.timestamp_error.is_none(), "{:?}", gpu.timestamp_error);
            assert!(
                gpu.frame_gpu_duration_ms
                    .is_some_and(|ms| ms.is_finite() && ms > 0.0)
            );
            assert_eq!(cpu.passes.len(), gpu.passes.len());
            for (a, b) in cpu.passes.iter().zip(&gpu.passes) {
                assert_eq!(a.pass_name, b.pass_name);
                assert_eq!(a.cpu_encode_ms, b.cpu_encode_ms);
                assert!(
                    b.gpu_duration_ms
                        .is_some_and(|ms| ms.is_finite() && ms > 0.0)
                );
            }
        }
        assert!(
            space.take_completed_render_profiles().is_empty(),
            "samples consumed once"
        );
    }
    let sync = space.render_profiled(true);
    assert!(
        sync.frame_gpu_duration_ms.is_some(),
        "synchronous profiler remains supported"
    );
    assert!(sync.queue_wait_ms.is_some());

    // A static scene submits only once. Its final sample must complete through
    // non-blocking polls alone, without another scene submission or GPU wait.
    let submitted = space.render_profiled_async(None);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let completed = loop {
        let completed = space.take_completed_render_profiles();
        if !completed.is_empty() {
            break completed;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "static scene readback stalled"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    assert_eq!(completed.len(), 1);
    assert_eq!(
        completed[0].frame_cpu_encode_ms,
        submitted.frame_cpu_encode_ms
    );
    assert!(completed[0].queue_wait_ms.is_none());
    assert!(completed[0].timestamp_error.is_none());
    assert!(
        completed[0]
            .frame_gpu_duration_ms
            .is_some_and(|ms| ms > 0.0)
    );
    assert_eq!(space.live_profile_status().0, 0);
    assert!(space.take_completed_render_profiles().is_empty());
}
