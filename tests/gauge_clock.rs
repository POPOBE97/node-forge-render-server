mod support;

use node_forge_render_server::playback::PlaybackRuntime;
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};

fn lamp_cyan(pixels: &[f32], width: u32, height: u32) -> f32 {
    // A patch at the fixed lamp centre, in bottom-left ScenePx coordinates.
    let mut cyan = 0.0;
    for y in 548..557 {
        for x in 465..474 {
            let i = (((height - 1 - y) * width + x) * 4) as usize;
            cyan += pixels[i + 1] - pixels[i];
        }
    }
    cyan / 81.0
}

#[test]
fn default_state_derivation_rotates_glass_and_updates_seconds_during_playback() {
    let _registry = support::function_registry_lock();
    let (scene, assets) = support::load_render_case("practive-gaudge-meter");
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default())
        .expect("native GPU required");
    let mut player = PlaybackRuntime::new(
        renderer.device.clone(), renderer.queue.clone(), renderer.adapter.clone(), scene, assets,
    ).unwrap();
    player.play();
    assert!(player.advance(0.0).unwrap().diagnostics.is_empty());
    assert_eq!(player.current_state_id(), Some("gauge.clock.default"));
    let (width, height, initial) = player.capture_linear_rgba().unwrap();
    assert!(lamp_cyan(&initial, width, height) > 0.5, "major cavity must reveal the lamp at 0s");
    if let Ok(directory) = std::env::var("GAUGE_CLOCK_CAPTURE_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        player.shader_space().save_texture_exr(player.scene_output_texture_name().as_str(),
            std::path::Path::new(&directory).join("frame-0.exr")).unwrap();
    }
    let mut previous = 0.0;
    for (time, angle, seconds) in [
        (0.5, 0.0, 0.0), (0.999, 0.0, 0.0), (1.0, -6.0, 1.0),
        (2.0, -12.0, 2.0), (3.0, -18.0, 3.0), (4.0, -24.0, 4.0),
        (5.0, -30.0, 5.0), (9.0, -54.0, 9.0),
        (10.0, -60.0, 10.0), (59.0, -354.0, 59.0), (60.0, 0.0, 0.0),
    ] {
        let frame = player.advance(time - previous).unwrap();
        assert!(frame.diagnostics.is_empty(), "{:?}", frame.diagnostics);
        assert_eq!(player.current_state_id(), Some("gauge.clock.default"));
        let snapshot = player.diagnostic_snapshot();
        let uniforms = &snapshot["uniforms"];
        assert!((uniforms["gauge.clock.angle"]["value"].as_f64().unwrap() - angle).abs() < 1e-5);
        assert_eq!(uniforms["gauge.clock.seconds"]["value"].as_f64().unwrap(), seconds);
        let (_, _, pixels) = player.capture_linear_rgba().unwrap();
        if let Ok(directory) = std::env::var("GAUGE_CLOCK_CAPTURE_DIR") {
            player.shader_space().save_texture_exr(player.scene_output_texture_name().as_str(),
                std::path::Path::new(&directory).join(format!("frame-{time}.exr"))).unwrap();
        }
        assert!(pixels.iter().all(|v| v.is_finite()));
        for (x, y) in [(0, 0), (width - 1, 0), (0, height - 1), (width - 1, height - 1)] {
            assert_eq!(pixels[((y * width + x) * 4 + 3) as usize], 0.0);
        }
        if time < 1.0 {
            let maximum = initial.iter().zip(&pixels).map(|(a, b)| (a - b).abs()).fold(0.0_f32, f32::max);
            assert!(maximum < 0.001, "fractional seconds must hold the entire frame: {maximum}");
        }
        if time == 1.0 {
            let changed = initial.chunks_exact(4).zip(pixels.chunks_exact(4))
                .filter(|(a, b)| (0..3).any(|c| (a[c] - b[c]).abs() > 0.005)).count();
            assert!(changed > 1000, "uniform updates must move rendered ticks: {changed}");
        }
        let cyan = lamp_cyan(&pixels, width, height);
        if (seconds as u32) % 5 == 0 {
            assert!(cyan > 0.5, "fixed lamp must reappear through each major at {time}s: {cyan}");
        } else {
            assert!(cyan < 0.04, "small marks must cover the fixed lamp at {time}s: {cyan}");
        }
        if time == 60.0 {
            let maximum = initial.iter().zip(&pixels).map(|(a, b)| (a - b).abs()).fold(0.0_f32, f32::max);
            assert!(maximum < 0.001, "full minute must return to its initial pixels: {maximum}");
        }
        previous = time;
    }
}
