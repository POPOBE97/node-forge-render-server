use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

use anyhow::{Result, anyhow};
use node_forge_render_server::{app, dsl, renderer, ws};
use rust_wgpu_fiber::eframe::{self, egui, egui_wgpu, wgpu};

#[derive(Debug, Default, Clone)]
struct Cli {
    headless: bool,
    dsl_json: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    output: Option<PathBuf>,
    render_to_file: bool,
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
            "--output" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --output"));
                };
                cli.output = Some(PathBuf::from(v));
                i += 2;
            }
            "--render-to-file" => {
                cli.render_to_file = true;
                i += 1;
            }
            other => {
                return Err(anyhow!(
                    "unknown argument: {other} (supported: --headless, --dsl-json <scene.json>, --render-to-file, --output <abs/path/to/output.png>, --outputdir <dir>)"
                ));
            }
        }
    }

    if cli.output.is_some() && cli.output_dir.is_some() {
        return Err(anyhow!(
            "cannot use --output together with --outputdir/--output-dir"
        ));
    }

    Ok(cli)
}

fn validate_absolute_output_path(path: &PathBuf) -> Result<()> {
    if !path.is_absolute() {
        return Err(anyhow!(
            "--output must be an absolute path, got: {}",
            path.display()
        ));
    }
    Ok(())
}

fn ensure_parent_dir_exists(path: &PathBuf) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Err(anyhow!(
            "output path has no parent directory: {}",
            path.display()
        ));
    };
    std::fs::create_dir_all(parent).map_err(|e| {
        anyhow!(
            "failed to create output directory {}: {e}",
            parent.display()
        )
    })?;
    Ok(())
}

fn resolve_file_output_path_under(output_dir: &PathBuf, rt: &dsl::FileRenderTarget) -> PathBuf {
    let mut out = output_dir.clone();
    out.push(&rt.file_name);
    out
}

fn run_headless_json_render_once(
    dsl_json_path: &std::path::Path,
    output_dir: Option<PathBuf>,
    output: Option<PathBuf>,
    render_to_file: bool,
) -> Result<()> {
    let text = std::fs::read_to_string(dsl_json_path).map_err(|e| {
        anyhow!(
            "failed to read --dsl-json file {}: {e}",
            dsl_json_path.display()
        )
    })?;

    let mut scene: dsl::SceneDSL = serde_json::from_str(&text)
        .map_err(|e| anyhow!("invalid SceneDSL json in {}: {e}", dsl_json_path.display()))?;

    dsl::normalize_scene_defaults(&mut scene)
        .map_err(|e| anyhow!("failed to apply default params: {e:#}"))?;

    let out_path = if render_to_file {
        let out =
            output.ok_or_else(|| anyhow!("--render-to-file requires --output <absolute path>"))?;
        validate_absolute_output_path(&out)?;
        out
    } else {
        let rt = dsl::file_render_target(&scene)?
            .ok_or_else(|| anyhow!("--dsl-json headless render requires RenderTarget=File (or pass --render-to-file --output <abs/path.png>)"))?;

        if let Some(out) = output {
            validate_absolute_output_path(&out)?;
            out
        } else {
            let output_dir = output_dir.unwrap_or_else(|| {
                dsl_json_path
                    .parent()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| PathBuf::from("."))
            });
            resolve_file_output_path_under(&output_dir, &rt)
        }
    };

    ensure_parent_dir_exists(&out_path)?;

    renderer::render_scene_to_png_headless(&scene, &out_path)?;
    println!("[headless] saved: {}", out_path.display());
    Ok(())
}

fn run_headless_ws_render_once(
    addr: &str,
    output: Option<PathBuf>,
    render_to_file: bool,
) -> Result<()> {
    use std::{thread, time::Duration};

    let (scene_tx, scene_rx) = crossbeam_channel::bounded::<ws::SceneUpdate>(1);
    let app_scene_rx = scene_rx.clone();
    let drop_rx = scene_rx;

    let last_good = Arc::new(Mutex::new(None));
    let hub = ws::WsHub::default();
    // Bind errors must be fatal in headless mode; otherwise we'd block forever waiting for DSL.
    let _ws_handle = ws::spawn_ws_server(addr, scene_tx, drop_rx, hub.clone(), last_good)?;

    // Wait for a renderable SceneDSL update, render, reply, then exit.
    loop {
        let update = app_scene_rx
            .recv()
            .map_err(|e| anyhow!("scene_update channel closed: {e}"))?;

        match update {
            ws::SceneUpdate::Parsed {
                scene,
                request_id,
                source: _,
            } => {
                let out_path = if render_to_file {
                    let out = output.clone().ok_or_else(|| {
                        anyhow!("--render-to-file requires --output <absolute path>")
                    })?;
                    validate_absolute_output_path(&out)?;
                    out
                } else {
                    let rt = dsl::file_render_target(&scene)?
                        .ok_or_else(|| anyhow!("--headless mode requires RenderTarget=File (or pass --render-to-file --output <abs/path.png>)"))?;

                    if let Some(out) = output.clone() {
                        validate_absolute_output_path(&out)?;
                        out
                    } else {
                        resolve_file_output_path(&rt)
                    }
                };

                ensure_parent_dir_exists(&out_path)?;

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
                return Ok(());
            }
            ws::SceneUpdate::UniformDelta { .. } => {
                // Headless one-shot render requires a full scene payload.
                // Ignore uniform deltas and wait for Parsed / ParseError.
            }
            ws::SceneUpdate::ParseError {
                message,
                request_id,
            } => {
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
                return Ok(());
            }
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
        if pb.is_absolute() { pb } else { base.join(pb) }
    };
    path.push(&rt.file_name);
    path
}

fn configure_egui_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "MiSans-Thin".to_string(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/MiSans/MiSans-Thin.ttf"))
            .into(),
    );
    fonts.font_data.insert(
        "MiSans-ExtraLight".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../assets/fonts/MiSans/MiSans-ExtraLight.ttf"
        ))
        .into(),
    );
    fonts.font_data.insert(
        "MiSans-Light".to_string(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/MiSans/MiSans-Light.ttf"))
            .into(),
    );
    fonts.font_data.insert(
        "MiSans-Normal".to_string(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/MiSans/MiSans-Normal.ttf"))
            .into(),
    );
    fonts.font_data.insert(
        "MiSans-Regular".to_string(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/MiSans/MiSans-Regular.ttf"))
            .into(),
    );
    fonts.font_data.insert(
        "MiSans-Medium".to_string(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/MiSans/MiSans-Medium.ttf"))
            .into(),
    );
    fonts.font_data.insert(
        "MiSans-Demibold".to_string(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/MiSans/MiSans-Demibold.ttf"))
            .into(),
    );
    fonts.font_data.insert(
        "MiSans-Semibold".to_string(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/MiSans/MiSans-Semibold.ttf"))
            .into(),
    );
    fonts.font_data.insert(
        "MiSans-Bold".to_string(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/MiSans/MiSans-Bold.ttf"))
            .into(),
    );
    fonts.font_data.insert(
        "MiSans-Heavy".to_string(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/MiSans/MiSans-Heavy.ttf"))
            .into(),
    );

    let mut proportional = vec!["MiSans-Normal".to_string()];
    proportional.extend(
        fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    fonts
        .families
        .insert(egui::FontFamily::Proportional, proportional);

    for family in [
        "MiSans-Thin",
        "MiSans-ExtraLight",
        "MiSans-Light",
        "MiSans-Normal",
        "MiSans-Regular",
        "MiSans-Medium",
        "MiSans-Demibold",
        "MiSans-Semibold",
        "MiSans-Bold",
        "MiSans-Heavy",
    ] {
        fonts.families.insert(
            egui::FontFamily::Name(family.into()),
            vec![family.to_string()],
        );
    }

    ctx.set_fonts(fonts);
}

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cli = parse_cli(&argv)?;

    // Script-friendly mode: pass DSL JSON directly.
    if cli.headless {
        if let Some(dsl_json_path) = cli.dsl_json.as_deref() {
            return run_headless_json_render_once(
                dsl_json_path,
                cli.output_dir,
                cli.output,
                cli.render_to_file,
            );
        }

        // Editor-driven mode: wait for editor to connect over ws and send SceneDSL.
        return run_headless_ws_render_once("127.0.0.1:8080", cli.output, cli.render_to_file);
    }

    let scene = match dsl::load_scene_from_default_asset() {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!(
                "[startup] failed to load/parse default scene; showing purple error screen: {e:#}"
            );
            None
        }
    };

    let resolution_hint = scene
        .as_ref()
        .and_then(dsl::screen_resolution)
        .unwrap_or([1024, 1024]);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_resizable(true)
            .with_transparent(true)
            .with_inner_size(resolution_hint.map(|x| x as f32))
            // Keep the OS window non-resizable, but don't tie the minimum size to the scene
            // resolution; UI mode (sidebar/canvas toggle + one-shot startup sizing) is the source
            // of truth and may need to grow/shrink the viewport independently.
            .with_min_inner_size(egui::vec2(240.0, 240.0)),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew {
                device_descriptor: std::sync::Arc::new(|_adapter| wgpu::DeviceDescriptor {
                    label: Some("eframe wgpu device"),
                    required_features: wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        "Node Forge Render Server",
        native_options,
        Box::new(move |cc| {
            configure_egui_fonts(&cc.egui_ctx);
            let render_state = cc
                .wgpu_render_state
                .as_ref()
                .ok_or_else(|| anyhow!("wgpu render state not available"))?;

            let (
                shader_space,
                resolution,
                output_texture_name,
                passes,
                last_good_initial,
                last_pipeline_signature,
            ) = if let Some(scene) = scene.clone() {
                match renderer::ShaderSpaceBuilder::new(
                    Arc::new(render_state.device.clone()),
                    Arc::new(render_state.queue.clone()),
                )
                .with_options(renderer::ShaderSpaceBuildOptions {
                    presentation_mode: renderer::ShaderSpacePresentationMode::UiSdrDisplayEncode,
                    debug_dump_wgsl_dir: None,
                })
                .build(&scene)
                {
                    Ok(result) => (
                        result.shader_space,
                        result.resolution,
                        result.present_output_texture,
                        result.pass_bindings,
                        Some(scene),
                        Some(result.pipeline_signature),
                    ),
                    Err(e) => {
                        eprintln!(
                            "[startup] scene build failed; showing purple error screen: {e:#}"
                        );
                        let result = renderer::ShaderSpaceBuilder::new(
                            Arc::new(render_state.device.clone()),
                            Arc::new(render_state.queue.clone()),
                        )
                        .build_error(resolution_hint)?;
                        (
                            result.shader_space,
                            result.resolution,
                            result.present_output_texture,
                            result.pass_bindings,
                            None,
                            None,
                        )
                    }
                }
            } else {
                let result = renderer::ShaderSpaceBuilder::new(
                    Arc::new(render_state.device.clone()),
                    Arc::new(render_state.queue.clone()),
                )
                .build_error(resolution_hint)?;
                (
                    result.shader_space,
                    result.resolution,
                    result.present_output_texture,
                    result.pass_bindings,
                    None,
                    None,
                )
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

            Ok(Box::new(app::App::from_init(app::AppInit {
                shader_space,
                resolution,
                window_resolution: resolution_hint,
                follow_scene_resolution_for_window: false,
                output_texture_name,
                start: Instant::now(),
                passes,
                scene_rx: app_scene_rx,
                ws_hub: hub,
                last_good,
                uniform_scene: None,
                last_pipeline_signature,
            })))
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

    #[test]
    fn parse_cli_headless_json_output_override() {
        let args = vec![
            "--headless".to_string(),
            "--dsl-json".to_string(),
            "scene.json".to_string(),
            "--output".to_string(),
            "/tmp/out.png".to_string(),
        ];
        let cli = parse_cli(&args).unwrap();
        assert!(cli.headless);
        assert_eq!(cli.dsl_json.as_ref().unwrap(), &PathBuf::from("scene.json"));
        assert_eq!(cli.output.as_ref().unwrap(), &PathBuf::from("/tmp/out.png"));
        assert!(cli.output_dir.is_none());
    }

    #[test]
    fn parse_cli_rejects_output_and_outputdir_together() {
        let args = vec![
            "--headless".to_string(),
            "--dsl-json".to_string(),
            "scene.json".to_string(),
            "--outputdir".to_string(),
            "out".to_string(),
            "--output".to_string(),
            "/tmp/out.png".to_string(),
        ];
        let err = parse_cli(&args).unwrap_err().to_string();
        assert!(err.contains("cannot use --output"));
    }

    #[test]
    fn parse_cli_render_to_file_flag() {
        let args = vec!["--headless".to_string(), "--render-to-file".to_string()];
        let cli = parse_cli(&args).unwrap();
        assert!(cli.headless);
        assert!(cli.render_to_file);
    }
}
