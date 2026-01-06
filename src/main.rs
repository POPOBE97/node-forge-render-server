use std::{sync::{Arc, Mutex}, time::Instant};

use anyhow::{anyhow, Result};
use rust_wgpu_fiber::eframe::{self, egui};
use node_forge_render_server::{app, dsl, renderer, ws};

fn main() -> Result<()> {
    let scene = match dsl::load_scene_from_default_asset() {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("[startup] failed to load/parse default scene; showing purple error screen: {e:#}");
            None
        }
    };

    let resolution_hint = scene
        .as_ref()
        .and_then(|scene| {
            scene
                .nodes
                .iter()
                .find(|n| n.node_type == "RenderTexture")
                .map(|n| {
                    let w = dsl::parse_u32(&n.params, "width").unwrap_or(1024);
                    let h = dsl::parse_u32(&n.params, "height").unwrap_or(1024);
                    [w, h]
                })
        })
        .unwrap_or([1024, 1024]);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_resizable(false)
            .with_transparent(true)
            .with_inner_size(resolution_hint.map(|x| x as f32))
            .with_min_inner_size(resolution_hint.map(|x| x as f32)),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "Node Forge Render Server",
        native_options,
        Box::new(move |cc| {
            let render_state = cc
                .wgpu_render_state
                .as_ref()
                .ok_or_else(|| anyhow!("wgpu render state not available"))?;

            let (shader_space, resolution, output_texture_name, passes, last_good_initial) =
                if let Some(scene) = scene.clone() {
                    match renderer::build_shader_space_from_scene(
                        &scene,
                        Arc::new(render_state.device.clone()),
                        Arc::new(render_state.queue.clone()),
                    ) {
                        Ok((shader_space, resolution, output_texture_name, passes)) => {
                            (shader_space, resolution, output_texture_name, passes, Some(scene))
                        }
                        Err(e) => {
                            eprintln!("[startup] scene build failed; showing purple error screen: {e:#}");
                            let (shader_space, resolution, output_texture_name, passes) =
                                renderer::build_error_shader_space(
                                    Arc::new(render_state.device.clone()),
                                    Arc::new(render_state.queue.clone()),
                                    resolution_hint,
                                )?;
                            (shader_space, resolution, output_texture_name, passes, None)
                        }
                    }
                } else {
                    let (shader_space, resolution, output_texture_name, passes) =
                        renderer::build_error_shader_space(
                            Arc::new(render_state.device.clone()),
                            Arc::new(render_state.queue.clone()),
                            resolution_hint,
                        )?;
                    (shader_space, resolution, output_texture_name, passes, None)
                };

            // WS scene updates (keep latest only).
            let (scene_tx, scene_rx) = crossbeam_channel::bounded::<ws::SceneUpdate>(1);
            let app_scene_rx = scene_rx.clone();
            let drop_rx = scene_rx;

            let last_good = Arc::new(Mutex::new(last_good_initial));
            let hub = ws::WsHub::default();
            let _ws_handle = ws::spawn_ws_server(
                "0.0.0.0:8080",
                scene_tx,
                drop_rx,
                hub.clone(),
                last_good.clone(),
            );

            Ok(Box::new(app::App {
                shader_space,
                resolution,
                output_texture_name,
                color_attachment: None,
                start: Instant::now(),
                passes,

                scene_rx: app_scene_rx,
                ws_hub: hub,
                last_good,
            }))
        }),
    )
    .map_err(|e| anyhow!("eframe run failed: {e}"))?;

    Ok(())
}
