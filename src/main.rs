use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

use anyhow::{anyhow, Result};
use rust_wgpu_fiber::eframe::{self, egui};
use node_forge_render_server::{app, dsl, renderer, ws};

#[derive(Debug, Default, Clone)]
struct Cli {
    headless: bool,
    dsl_json: Option<PathBuf>,
    output_dir: Option<PathBuf>,
}

fn parse_cli(args: &[String]) -> Result<Cli> {
    let mut cli = Cli::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--headless" => {
                cli.headless = true;
                i += 1;
            }
            "--dsl-json" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --dsl-json"));
                };
                cli.dsl_json = Some(PathBuf::from(v));
                i += 2;
            }
            "--outputdir" | "--output-dir" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --outputdir"));
                };
                cli.output_dir = Some(PathBuf::from(v));
                i += 2;
            }
            other => {
                return Err(anyhow!(
                    "unknown argument: {other} (supported: --headless, --dsl-json <scene.json>, --outputdir <dir>)"
                ));
            }
        }
    }
    Ok(cli)
}

fn resolve_file_output_path_under(output_dir: &PathBuf, rt: &dsl::FileRenderTarget) -> PathBuf {
    let mut out = output_dir.clone();
    out.push(&rt.file_name);
    out
}

fn run_headless_json_render_once(dsl_json_path: &std::path::Path, output_dir: PathBuf) -> Result<()> {
    let text = std::fs::read_to_string(dsl_json_path)
        .map_err(|e| anyhow!("failed to read --dsl-json file {}: {e}", dsl_json_path.display()))?;

    let mut scene: dsl::SceneDSL = serde_json::from_str(&text)
        .map_err(|e| anyhow!("invalid SceneDSL json in {}: {e}", dsl_json_path.display()))?;

    dsl::normalize_scene_defaults(&mut scene)
        .map_err(|e| anyhow!("failed to apply default params: {e:#}"))?;

    let rt = dsl::file_render_target(&scene)?
        .ok_or_else(|| anyhow!("--dsl-json headless render requires RenderTarget=File"))?;
    let out_path = resolve_file_output_path_under(&output_dir, &rt);

    renderer::render_scene_to_png_headless(&scene, &out_path)?;
    println!("[headless] saved: {}", out_path.display());
    Ok(())
}

fn run_headless_ws_render_once(addr: &str) -> Result<()> {
    use std::{thread, time::Duration};

    let (scene_tx, scene_rx) = crossbeam_channel::bounded::<ws::SceneUpdate>(1);
    let app_scene_rx = scene_rx.clone();
    let drop_rx = scene_rx;

    let last_good = Arc::new(Mutex::new(None));
    let hub = ws::WsHub::default();
    // Bind errors must be fatal in headless mode; otherwise we'd block forever waiting for DSL.
    let _ws_handle = ws::spawn_ws_server(addr, scene_tx, drop_rx, hub.clone(), last_good)?;

    // Wait for a single DSL update, render, reply, then exit.
    let update = app_scene_rx
        .recv()
        .map_err(|e| anyhow!("scene_update channel closed: {e}"))?;

    match update {
        ws::SceneUpdate::Parsed { scene, request_id } => {
            let rt = dsl::file_render_target(&scene)?
                .ok_or_else(|| anyhow!("--headless mode requires RenderTarget=File"))?;
            let out_path = resolve_file_output_path(&rt);

            let result = renderer::render_scene_to_png_headless(&scene, &out_path);
            match result {
                Ok(()) => {
                    let msg = node_forge_render_server::protocol::WSMessage {
                        msg_type: "render_to_file_done".to_string(),
                        timestamp: node_forge_render_server::protocol::now_millis(),
                        request_id,
                        payload: Some(serde_json::json!({
                            "path": out_path.display().to_string(),
                        })),
                    };
                    if let Ok(text) = serde_json::to_string(&msg) {
                        println!("Rendered to file at {}", out_path.display());
                        println!("[headless]: {}", text);
                        hub.broadcast(text);
                    }
                }
                Err(e) => {
                    let msg = node_forge_render_server::protocol::WSMessage {
                        msg_type: "error".to_string(),
                        timestamp: node_forge_render_server::protocol::now_millis(),
                        request_id,
                        payload: Some(node_forge_render_server::protocol::ErrorPayload {
                            code: "RENDER_TO_FILE_ERROR".to_string(),
                            message: format!("{e:#}"),
                        }),
                    };
                    if let Ok(text) = serde_json::to_string(&msg) {
                        println!("[headless]: {}", text);
                        hub.broadcast(text);
                    }
                }
            }

            // Give the ws writer loop a brief chance to flush before exiting.
            thread::sleep(Duration::from_millis(150));
            Ok(())
        }
        ws::SceneUpdate::ParseError { message, request_id } => {
            let msg = node_forge_render_server::protocol::WSMessage {
                msg_type: "error".to_string(),
                timestamp: node_forge_render_server::protocol::now_millis(),
                request_id,
                payload: Some(node_forge_render_server::protocol::ErrorPayload {
                    code: "PARSE_ERROR".to_string(),
                    message,
                }),
            };
            if let Ok(text) = serde_json::to_string(&msg) {
                hub.broadcast(text);
            }
            thread::sleep(Duration::from_millis(150));
            Ok(())
        }
    }
}

fn resolve_file_output_path(rt: &dsl::FileRenderTarget) -> std::path::PathBuf {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = rt.directory.trim();
    let mut path = if dir.is_empty() {
        base
    } else {
        let pb = std::path::PathBuf::from(dir);
        if pb.is_absolute() {
            pb
        } else {
            base.join(pb)
        }
    };
    path.push(&rt.file_name);
    path
}

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cli = parse_cli(&argv)?;

    // Script-friendly mode: pass DSL JSON directly.
    if cli.headless {
        if let Some(dsl_json_path) = cli.dsl_json.as_deref() {
            let output_dir = cli.output_dir.unwrap_or_else(|| {
                dsl_json_path
                    .parent()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| PathBuf::from("."))
            });
            return run_headless_json_render_once(dsl_json_path, output_dir);
        }

        // Editor-driven mode: wait for editor to connect over ws and send SceneDSL.
        return run_headless_ws_render_once("127.0.0.1:8080");
    }

    let scene = match dsl::load_scene_from_default_asset() {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("[startup] failed to load/parse default scene; showing purple error screen: {e:#}");
            None
        }
    };

    let resolution_hint = scene
        .as_ref()
        .and_then(dsl::screen_resolution)
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
            if let Err(e) = ws::spawn_ws_server(
                "0.0.0.0:8080",
                scene_tx,
                drop_rx,
                hub.clone(),
                last_good.clone(),
            ) {
                eprintln!("[ws] failed to start ws server: {e:#}");
            }

            Ok(Box::new(app::App {
                shader_space,
                resolution,
                window_resolution: resolution_hint,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_headless_json_outputdir() {
        let args = vec![
            "--headless".to_string(),
            "--dsl-json".to_string(),
            "scene.json".to_string(),
            "--outputdir".to_string(),
            "out".to_string(),
        ];
        let cli = parse_cli(&args).unwrap();
        assert!(cli.headless);
        assert_eq!(cli.dsl_json.as_ref().unwrap(), &PathBuf::from("scene.json"));
        assert_eq!(cli.output_dir.as_ref().unwrap(), &PathBuf::from("out"));
    }
}
