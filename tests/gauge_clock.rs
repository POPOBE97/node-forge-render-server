mod support;

use node_forge_render_server::playback::PlaybackRuntime;
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};

fn lamp_cyan(pixels: &[f32], width: u32, height: u32, center: (u32, u32)) -> f32 {
    // Sample the lamp centre in bottom-left ScenePx coordinates.
    let mut cyan = 0.0;
    for y in center.1 - 4..=center.1 + 4 {
        for x in center.0 - 4..=center.0 + 4 {
            let i = (((height - 1 - y) * width + x) * 4) as usize;
            cyan += pixels[i + 1] - pixels[i];
        }
    }
    cyan / 81.0
}

fn palette_oklab(color: [f64; 4]) -> [f64; 3] {
    let [r, g, b] = node_forge_render_server::color::linear_p3_to_linear_srgb(
        [color[0] as f32, color[1] as f32, color[2] as f32],
    ).map(f64::from);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

fn assert_palette(uniforms: &serde_json::Value, position: f64) {
    let progress = ((position - 80.0) / 10.0).clamp(0.0, 1.0);
    for (node, blue, red) in [
        ("gauge.accent", [0.0, 1.0, 0.92, 1.0], [1.0, 0.005, 0.018, 1.0]),
        ("gauge.glass.tint", [0.0, 0.55, 0.506, 1.0], [0.55, 0.00275, 0.0099, 1.0]),
        ("gauge.label.base", [0.0, 0.22, 0.2, 1.0], [0.22, 0.0011, 0.00396, 1.0]),
        ("gauge.label.ambient", [0.04, 0.18, 0.19, 1.0], [0.19, 0.04, 0.043, 1.0]),
        ("gauge.surface.base", [0.0002, 0.0050, 0.0047, 1.0], [0.0050, 0.0002, 0.0005, 1.0]),
        ("gauge.surface.wash", [0.0, 0.0120, 0.0100, 1.0], [0.0120, 0.0, 0.0018, 1.0]),
    ] {
        let actual = std::array::from_fn(|i| uniforms[node]["value"][i].as_f64().unwrap());
        if progress == 0.0 || progress == 1.0 {
            assert_eq!(actual, if progress == 0.0 { blue } else { red }, "{node} endpoint at {position}");
        } else {
            let start = palette_oklab(blue);
            let end = palette_oklab(red);
            let lab = palette_oklab(actual);
            for i in 0..3 {
                let expected = start[i] + (end[i] - start[i]) * progress;
                assert!((lab[i] - expected).abs() < 1e-5, "{node} must interpolate in OKLab at {position}: {lab:?}");
            }
            assert_eq!(actual[3], 1.0);
        }
    }
}

#[test]
fn gauge_advances_by_six_blends_oklab_from_80_to_90_and_holds_at_100() {
    let _registry = support::function_registry_lock();
    let (scene, assets) = support::load_render_case("practice-gaudge-meter");
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default())
        .expect("native GPU required");
    let mut player = PlaybackRuntime::new(
        renderer.device.clone(), renderer.queue.clone(), renderer.adapter.clone(), scene, assets,
    ).unwrap();
    player.play();
    assert!(player.advance(0.0).unwrap().diagnostics.is_empty());
    assert_eq!(player.current_state_id(), Some("gauge.clock.default"));
    let initial_uniforms = player.diagnostic_snapshot()["uniforms"].clone();
    let (width, height, initial) = player.capture_linear_rgba().unwrap();
    assert!(lamp_cyan(&initial, width, height, (469, 552)) > 0.5, "major cavity must reveal the lamp at 0s");
    if let Ok(directory) = std::env::var("GAUGE_CLOCK_CAPTURE_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        player.shader_space().save_texture_exr(player.scene_output_texture_name().as_str(),
            std::path::Path::new(&directory).join("frame-0.exr")).unwrap();
    }
    let mut previous = 0.0;
    let mut completed = None;
    for (time, value) in [
        (0.5, 0.0), (0.999, 0.0), (1.0, 6.0),
        (2.0, 12.0), (3.0, 18.0), (4.0, 24.0),
        (5.0, 30.0), (9.0, 54.0), (10.0, 60.0),
        (14.0, 84.0), (15.0, 90.0), (15.999, 90.0), (16.0, 96.0),
        (17.0, 100.0), (60.0, 100.0),
    ] {
        let frame = player.advance(time - previous).unwrap();
        assert!(frame.diagnostics.is_empty(), "{:?}", frame.diagnostics);
        assert_eq!(player.current_state_id(), Some("gauge.clock.default"));
        let snapshot = player.diagnostic_snapshot();
        let uniforms = &snapshot["uniforms"];
        assert_eq!(uniforms["gauge.clock.seconds"]["value"].as_f64().unwrap(), value);
        assert_palette(uniforms, value);
        let dial_offset = (value - 85.0).max(0.0) * 1.2;
        let pointer_angle = dial_offset;
        assert!((uniforms["gauge.clock.angle"]["value"].as_f64().unwrap() - value * 3.0 - dial_offset).abs() < 1e-5,
            "dial marks must use the integer progress and its upper-range offset");
        let radians = pointer_angle.to_radians();
        let pointer_x = 1575.0 + radians.cos() * (469.5 - 1575.0) - radians.sin() * (552.0 + 90.0);
        let pointer_y = -90.0 + radians.sin() * (469.5 - 1575.0) + radians.cos() * (552.0 + 90.0);
        assert!((uniforms["gauge.pointer.position"]["x"].as_f64().unwrap() - pointer_x).abs() < 1e-5);
        assert!((uniforms["gauge.pointer.position"]["y"].as_f64().unwrap() - pointer_y).abs() < 1e-5);
        assert!((uniforms["gauge.pointer.rotation"]["z"].as_f64().unwrap() - (-30.0 + pointer_angle)).abs() < 1e-5);
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
            assert_eq!(uniforms, &initial_uniforms, "each step must hold for one second");
        }
        if time == 1.0 {
            let changed = initial.chunks_exact(4).zip(pixels.chunks_exact(4))
                .filter(|(a, b)| (0..3).any(|c| (a[c] - b[c]).abs() > 0.005)).count();
            assert!(changed > 1000, "uniform updates must move rendered ticks: {changed}");
        }
        let cyan = lamp_cyan(&pixels, width, height, (pointer_x.round() as u32, pointer_y.round() as u32));
        if value <= 80.0 && (value as u32) % 10 == 0 {
            assert!(cyan > 0.5, "major cavity must reveal cyan at value {value}: {cyan}");
        }
        if value >= 90.0 && (value as u32) % 10 == 0 {
            assert!(cyan < -0.5, "major cavity must reveal red from 90 onward: {cyan}");
        }
        if value == 96.0 {
            assert!(cyan.abs() < 0.25, "lamp outside a major window must stay occluded at 96: {cyan}");
        }
        if value >= 90.0 {
            let red_pixels = pixels.chunks_exact(4)
                .filter(|p| p[0] > 0.3 && p[0] > p[1] * 3.0 && p[0] > p[2] * 3.0).count();
            assert!(red_pixels > 1000, "warning must turn rendered lettering red: {red_pixels}");
            let background = ((100 * width + 500) * 4) as usize;
            assert!(pixels[background] > pixels[background + 1], "background must use the red palette");
        }
        if value == 100.0 {
            assert!(pointer_x < 469.5 && pointer_y < 552.0, "upper-range pointer must move toward the lower-left");
            assert!(cyan < -0.5, "major cavity must reveal red at the lower-left endpoint: {cyan}");
            if let Some(completed) = &completed {
                assert_eq!(uniforms, completed, "completed gauge must hold its render values");
            } else {
                completed = Some(uniforms.clone());
            }
        }
        previous = time;
    }

    assert!(player.force_state("gauge.sweep.continuous").unwrap().diagnostics.is_empty());
    assert_eq!(player.current_state_id(), Some("gauge.sweep.continuous"));
    assert_eq!(player.diagnostic_snapshot()["uniforms"]["gauge.clock.seconds"]["value"], 0.0);
    previous = 0.0;
    let mut held_value = 0.0;
    let mut held_uniforms = player.diagnostic_snapshot()["uniforms"].clone();
    let mut held_pixels: Option<Vec<f32>> = None;
    for (time, value, angle, pointer_angle) in [
        (0.99 / 36.0, 0.0, 0.0, -30.0), (1.01 / 36.0, 0.0, 0.0, -30.0),
        (1.99 / 36.0, 0.0, 0.0, -30.0), (2.01 / 36.0, 2.0, 6.0, -30.0),
        (0.125, 4.0, 12.0, -30.0), (9.25 / 36.0, 8.0, 24.0, -30.0),
        (36.25 / 36.0, 36.0, 108.0, -30.0), (72.25 / 36.0, 72.0, 216.0, -30.0),
        (79.75 / 36.0, 78.0, 234.0, -30.0), (80.25 / 36.0, 80.0, 240.0, -30.0),
        (83.05 / 36.0, 82.0, 246.0, -30.0), (83.95 / 36.0, 82.0, 246.0, -30.0),
        (84.15 / 36.0, 84.0, 252.0, -30.0),
        (256.05 / 108.0, 84.0, 252.0, -30.0), (257.1 / 108.0, 84.0, 252.0, -30.0),
        (263.358 / 108.0, 86.0, 259.2, -28.8), (263.442 / 108.0, 86.0, 259.2, -28.8),
        (274.95 / 108.0, 88.0, 267.6, -26.4), (277.05 / 108.0, 90.0, 276.0, -24.0),
        (293.85 / 108.0, 94.0, 292.8, -19.2),
        (298.68 / 108.0, 94.0, 292.8, -19.2),
        (3.0, 100.0, 318.0, -12.0), (10.0, 100.0, 318.0, -12.0),
    ] {
        let frame = player.advance(time - previous).unwrap();
        assert!(frame.diagnostics.is_empty(), "{:?}", frame.diagnostics);
        let snapshot = player.diagnostic_snapshot();
        let uniforms = &snapshot["uniforms"];
        assert_eq!(uniforms["gauge.clock.seconds"]["value"].as_f64().unwrap(), value);
        assert!((uniforms["gauge.clock.angle"]["value"].as_f64().unwrap() - angle).abs() < 1e-4,
            "dial marks must floor progress to whole ticks");
        assert!((uniforms["gauge.pointer.rotation"]["z"].as_f64().unwrap() - pointer_angle).abs() < 1e-4,
            "pointer must use floored progress without an oscillating tick phase");
        assert_palette(uniforms, value);
        let tick_index = (uniforms["gauge.clock.angle"]["value"].as_f64().unwrap()
            - uniforms["gauge.pointer.rotation"]["z"].as_f64().unwrap() - 30.0) / 6.0;
        assert!((tick_index - tick_index.round()).abs() < 1e-5,
            "every held position must align to a whole tick, never a half tick");
        if value == held_value {
            assert_eq!(uniforms, &held_uniforms,
                "marks, pointer, digits and palette must all hold within the same tick");
        }
        held_value = value;
        held_uniforms = uniforms.clone();
        if value == 82.0 {
            let (_, _, pixels) = player.capture_linear_rgba().unwrap();
            if let Some(before) = &held_pixels {
                // Allow small half-float rendering differences, but no visible movement.
                let max_delta = pixels.iter().zip(before)
                    .map(|(a, b)| (a - b).abs()).fold(0.0_f32, f32::max);
                assert!(max_delta < 0.005, "rendered output must hold within one tick: {max_delta}");
            }
            held_pixels = Some(pixels);
        }
        if value == 94.0 {
            let x = uniforms["gauge.pointer.position"]["x"].as_f64().unwrap();
            let y = uniforms["gauge.pointer.position"]["y"].as_f64().unwrap();
            let (_, _, pixels) = player.capture_linear_rgba().unwrap();
            let cyan = lamp_cyan(&pixels, width, height, (x.round() as u32, y.round() as u32));
            assert!(cyan.abs() < 0.25, "floored progress must also occlude the lamp outside major windows: {cyan}");
            if let Ok(directory) = std::env::var("GAUGE_CLOCK_CAPTURE_DIR") {
                player.shader_space().save_texture_exr(player.scene_output_texture_name().as_str(),
                    std::path::Path::new(&directory).join("frame-floor-94.exr")).unwrap();
            }
        }
        previous = time;
    }
    assert!(player.force_state("gauge.clock.default").unwrap().diagnostics.is_empty());
    assert_eq!(player.diagnostic_snapshot()["uniforms"]["gauge.clock.seconds"]["value"], 0.0);
    assert!(player.advance(0.5).unwrap().diagnostics.is_empty());
    assert_eq!(player.diagnostic_snapshot()["uniforms"]["gauge.clock.seconds"]["value"], 0.0);
    assert!(player.advance(0.5).unwrap().diagnostics.is_empty());
    assert_eq!(player.diagnostic_snapshot()["uniforms"]["gauge.clock.seconds"]["value"], 6.0);
}
