use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

use anyhow::{Result, anyhow, bail};
use node_forge_render_server::animation::{
    AnimationSession, KeyFilter, TraceAnalyzeConfig, TraceRunConfig, TraceScenario, format_summary,
    format_table, load_event_schedule, run_schedule_trace, run_trace, write_report_json,
};
use node_forge_render_server::state_machine::TickSchedule;
use node_forge_render_server::state_machine::types::AnimationStateType;
use node_forge_render_server::{app, asset_store, dsl, profile, renderer, ws};
use rust_wgpu_fiber::eframe::{self, egui, egui_wgpu, wgpu};

#[derive(Debug, Default, Clone)]
struct Cli {
    headless: bool,
    dsl_json: Option<PathBuf>,
    nforge: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    output: Option<PathBuf>,
    dump_wgsl_dir: Option<PathBuf>,
    dump_shader_deps: Option<String>,
    dump_shader_deps_output: Option<PathBuf>,
    headless_state: Option<String>,
    headless_overrides_output: Option<PathBuf>,
    export_texture: Option<String>,
    capture_pass: Option<String>,
    render_to_file: bool,
    continuous_redraw: bool,
    profile: bool,
    profile_output: Option<PathBuf>,
    profile_format: Option<String>,
    profile_frames: u32,
    profile_warmup_frames: u32,
    // --trace-animation
    trace_animation: bool,
    trace_scenario: Option<PathBuf>,
    trace_seconds: Option<f64>,
    trace_fps: Option<u32>,
    trace_include_end: Option<bool>,
    trace_initial_state: Option<String>,
    trace_events: Option<PathBuf>,
    trace_channels: Option<String>,
    trace_overrides: Option<String>,
    trace_values: Option<String>,
    trace_output: Option<PathBuf>,
    trace_format: Option<String>,
    trace_jump_threshold_channel: Option<f64>,
    trace_jump_threshold_override: Option<f64>,
    trace_check_identity: Option<bool>,
    trace_fail_on_jump: bool,
    trace_pretty: Option<bool>,
}

#[derive(Debug, Clone)]
struct HeadlessProfileOptions {
    config: profile::ProfileRunConfig,
    output: profile::ProfileOutputTarget,
}

#[derive(serde::Serialize)]
struct ShaderDependencyDump<'a> {
    pass_name: &'a str,
    dependency_root_target_id: Option<&'a str>,
    targets: Vec<&'a renderer::PassDebugDependencyTarget>,
    dependency_trees: BTreeMap<String, renderer::PassDebugDependencyNode>,
    parse_error: Option<&'a str>,
    dependency_error: Option<&'a str>,
}

#[cfg(all(target_os = "macos", debug_assertions))]
fn spawn_metal_capture_state_watcher(
    egui_ctx: egui::Context,
) -> Option<crossbeam_channel::Receiver<bool>> {
    use std::{thread, time::Duration};

    let (tx, rx) = crossbeam_channel::bounded::<bool>(4);
    thread::spawn(move || {
        let mut last_state = metal::CaptureManager::shared().is_capturing();
        if last_state {
            if tx.send(true).is_err() {
                return;
            }
            eprintln!("[capture] metal capture active at startup; enabling continuous redraw");
            egui_ctx.request_repaint();
        }

        loop {
            thread::sleep(Duration::from_millis(16));
            let current_state = metal::CaptureManager::shared().is_capturing();
            if current_state == last_state {
                continue;
            }

            last_state = current_state;
            if tx.send(current_state).is_err() {
                break;
            }

            if current_state {
                eprintln!("[capture] metal capture started");
            } else {
                eprintln!("[capture] metal capture stopped");
            }
            egui_ctx.request_repaint();
        }
    });
    Some(rx)
}

#[cfg(not(all(target_os = "macos", debug_assertions)))]
fn spawn_metal_capture_state_watcher(
    _egui_ctx: egui::Context,
) -> Option<crossbeam_channel::Receiver<bool>> {
    None
}

fn spawn_template_watcher(
    scene_tx: crossbeam_channel::Sender<ws::SceneUpdate>,
    last_good: Arc<Mutex<Option<dsl::SceneDSL>>>,
    egui_ctx: egui::Context,
) {
    use notify::{RecursiveMode, Watcher};
    use std::time::Duration;

    let templates_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("renderer")
        .join("node_compiler")
        .join("templates");

    // Per-scene WGSL material overrides live here; the editor passes the path
    // via NODE_FORGE_MATERIALS_DIR when spawning the renderer. Watching this
    // dir means saving the override file in VSCode triggers an HMR rebuild
    // through the same code path as the bundled-template watcher.
    let materials_dir =
        renderer::node_compiler::template_loader::materials_root().map(|p| p.to_path_buf());

    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[template-hmr] failed to create watcher: {e}");
                return;
            }
        };
        if let Err(e) = watcher.watch(&templates_dir, RecursiveMode::Recursive) {
            eprintln!(
                "[template-hmr] failed to watch {}: {e}",
                templates_dir.display()
            );
            return;
        }
        eprintln!("[template-hmr] watching {}", templates_dir.display());

        if let Some(dir) = materials_dir.as_ref() {
            // Best-effort create the dir so notify doesn't fail on first-launch.
            let _ = std::fs::create_dir_all(dir);
            match watcher.watch(dir, RecursiveMode::NonRecursive) {
                Ok(()) => eprintln!("[material-override-hmr] watching {}", dir.display()),
                Err(e) => eprintln!(
                    "[material-override-hmr] failed to watch {}: {e}",
                    dir.display()
                ),
            }
        }

        let debounce = Duration::from_millis(100);
        loop {
            match rx.recv() {
                Ok(_event) => {
                    // Debounce: drain any further events within the window
                    std::thread::sleep(debounce);
                    while rx.try_recv().is_ok() {}

                    renderer::node_compiler::template_loader::invalidate_cache();
                    eprintln!("[template-hmr] template changed, triggering rebuild");

                    let scene = last_good.lock().ok().and_then(|g| g.clone());
                    if let Some(scene) = scene {
                        if let Some(error) = scene
                            .nodes
                            .iter()
                            .filter(|node| node.node_type == "ShaderMaterial")
                            .find_map(|node| {
                                renderer::node_compiler::shader_material::validate_node_source(node)
                                    .err()
                            })
                        {
                            eprintln!(
                                "[shader-material-hmr] keeping last-good render after WGSL error: {error:#}"
                            );
                            egui_ctx.request_repaint();
                            continue;
                        }
                        let _ = scene_tx.try_send(ws::SceneUpdate::Parsed {
                            scene,
                            request_id: None,
                            source: ws::ParsedSceneSource::SceneDelta,
                            perf_trace: None,
                        });
                        egui_ctx.request_repaint();
                    }
                }
                Err(_) => break,
            }
        }
    });
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
            "--nforge" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --nforge"));
                };
                cli.nforge = Some(PathBuf::from(v));
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
            "--dump-wgsl-dir" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --dump-wgsl-dir"));
                };
                cli.dump_wgsl_dir = Some(PathBuf::from(v));
                i += 2;
            }
            "--dump-shader-deps" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --dump-shader-deps"));
                };
                cli.dump_shader_deps = Some(v.clone());
                i += 2;
            }
            "--dump-shader-deps-output" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --dump-shader-deps-output"));
                };
                cli.dump_shader_deps_output = Some(PathBuf::from(v));
                i += 2;
            }
            "--headless-state" => {
                let Some(value) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --headless-state"));
                };
                cli.headless_state = Some(value.clone());
                i += 2;
            }
            "--headless-overrides-output" => {
                let Some(value) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --headless-overrides-output"));
                };
                cli.headless_overrides_output = Some(PathBuf::from(value));
                i += 2;
            }
            "--export-texture" => {
                let Some(value) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --export-texture"));
                };
                cli.export_texture = Some(value.clone());
                i += 2;
            }
            "--capture-pass" => {
                let Some(value) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --capture-pass"));
                };
                cli.capture_pass = Some(value.clone());
                i += 2;
            }
            "--render-to-file" => {
                cli.render_to_file = true;
                i += 1;
            }
            "--continuous-redraw" | "--force-continuous-redraw" => {
                cli.continuous_redraw = true;
                i += 1;
            }
            "--profile" => {
                cli.profile = true;
                i += 1;
            }
            "--profile-output" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --profile-output"));
                };
                if v != "-" {
                    cli.profile_output = Some(PathBuf::from(v));
                }
                i += 2;
            }
            "--profile-format" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --profile-format"));
                };
                cli.profile_format = Some(v.clone());
                i += 2;
            }
            "--profile-frames" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --profile-frames"));
                };
                cli.profile_frames = v
                    .parse::<u32>()
                    .map_err(|_| anyhow!("--profile-frames must be a positive integer"))?;
                i += 2;
            }
            "--profile-warmup-frames" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --profile-warmup-frames"));
                };
                cli.profile_warmup_frames = v
                    .parse::<u32>()
                    .map_err(|_| anyhow!("--profile-warmup-frames must be an integer"))?;
                i += 2;
            }
            "--trace-animation" => {
                cli.trace_animation = true;
                i += 1;
            }
            "--trace-scenario" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --trace-scenario"));
                };
                cli.trace_scenario = Some(PathBuf::from(v));
                i += 2;
            }
            "--trace-seconds" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --trace-seconds"));
                };
                cli.trace_seconds = Some(
                    v.parse::<f64>()
                        .map_err(|_| anyhow!("--trace-seconds must be a number"))?,
                );
                i += 2;
            }
            "--trace-fps" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --trace-fps"));
                };
                cli.trace_fps = Some(
                    v.parse::<u32>()
                        .map_err(|_| anyhow!("--trace-fps must be a positive integer"))?,
                );
                i += 2;
            }
            "--trace-include-end" => {
                cli.trace_include_end = Some(true);
                i += 1;
            }
            "--trace-no-include-end" => {
                cli.trace_include_end = Some(false);
                i += 1;
            }
            "--trace-initial-state" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --trace-initial-state"));
                };
                cli.trace_initial_state = Some(v.clone());
                i += 2;
            }
            "--trace-events" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --trace-events"));
                };
                cli.trace_events = Some(PathBuf::from(v));
                i += 2;
            }
            "--trace-channels" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --trace-channels"));
                };
                cli.trace_channels = Some(v.clone());
                i += 2;
            }
            "--trace-overrides" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --trace-overrides"));
                };
                cli.trace_overrides = Some(v.clone());
                i += 2;
            }
            "--trace-values" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --trace-values"));
                };
                cli.trace_values = Some(v.clone());
                i += 2;
            }
            "--trace-output" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --trace-output"));
                };
                cli.trace_output = Some(PathBuf::from(v));
                i += 2;
            }
            "--trace-format" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --trace-format"));
                };
                cli.trace_format = Some(v.clone());
                i += 2;
            }
            "--trace-jump-threshold-channel" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --trace-jump-threshold-channel"));
                };
                cli.trace_jump_threshold_channel = Some(
                    v.parse::<f64>()
                        .map_err(|_| anyhow!("--trace-jump-threshold-channel must be a number"))?,
                );
                i += 2;
            }
            "--trace-jump-threshold-override" => {
                let Some(v) = args.get(i + 1) else {
                    return Err(anyhow!("missing value for --trace-jump-threshold-override"));
                };
                cli.trace_jump_threshold_override =
                    Some(v.parse::<f64>().map_err(|_| {
                        anyhow!("--trace-jump-threshold-override must be a number")
                    })?);
                i += 2;
            }
            "--trace-check-identity" => {
                cli.trace_check_identity = Some(true);
                i += 1;
            }
            "--trace-no-check-identity" => {
                cli.trace_check_identity = Some(false);
                i += 1;
            }
            "--trace-fail-on-jump" => {
                cli.trace_fail_on_jump = true;
                i += 1;
            }
            "--trace-pretty" => {
                cli.trace_pretty = Some(true);
                i += 1;
            }
            "--trace-compact" => {
                cli.trace_pretty = Some(false);
                i += 1;
            }
            other => {
                return Err(anyhow!(
                    "unknown argument: {other} (supported: --headless, --dsl-json <scene.json>, --nforge <file.nforge>, --headless-state <id|name>, --headless-overrides-output <path>, --export-texture <resource-name>, --capture-pass <pass-id>, --render-to-file, --continuous-redraw, --output <abs/path/to/output>, --outputdir <dir>, --dump-wgsl-dir <dir>, --dump-shader-deps <pass-name>, --dump-shader-deps-output <path>, --profile, --profile-output <path|->, --profile-format ndjson, --profile-frames <n>, --profile-warmup-frames <n>, --trace-animation, --trace-scenario <path>, --trace-seconds <n>, --trace-fps <n>, --trace-output <path|->, --trace-format json,summary,table, ...)"
                ));
            }
        }
    }

    if cli.output.is_some() && cli.output_dir.is_some() {
        return Err(anyhow!(
            "cannot use --output together with --outputdir/--output-dir"
        ));
    }
    if cli.dump_shader_deps.is_some() && cli.dsl_json.is_none() && cli.nforge.is_none() {
        return Err(anyhow!(
            "--dump-shader-deps requires --dsl-json <scene.json> or --nforge <file.nforge>"
        ));
    }
    if cli.dump_shader_deps_output.is_some() && cli.dump_shader_deps.is_none() {
        return Err(anyhow!(
            "--dump-shader-deps-output requires --dump-shader-deps <pass-name>"
        ));
    }
    if cli.headless_state.is_some()
        && (!cli.headless || (cli.nforge.is_none() && cli.dsl_json.is_none()))
    {
        return Err(anyhow!(
            "--headless-state requires --headless and --nforge/--dsl-json"
        ));
    }
    if cli.headless_overrides_output.is_some() && cli.headless_state.is_none() {
        return Err(anyhow!(
            "--headless-overrides-output requires --headless-state"
        ));
    }
    if cli.export_texture.is_some() && cli.capture_pass.is_some() {
        return Err(anyhow!(
            "--export-texture and --capture-pass are mutually exclusive"
        ));
    }
    if cli.export_texture.is_some() {
        if !cli.headless || (cli.nforge.is_none() && cli.dsl_json.is_none()) {
            return Err(anyhow!(
                "--export-texture requires --headless and --nforge/--dsl-json"
            ));
        }
        if cli.output.is_none() {
            return Err(anyhow!(
                "--export-texture requires --output <absolute path>"
            ));
        }
        if cli.profile {
            return Err(anyhow!(
                "--export-texture cannot be combined with --profile"
            ));
        }
    }
    if cli.capture_pass.is_some() {
        if !cli.headless || (cli.nforge.is_none() && cli.dsl_json.is_none()) {
            return Err(anyhow!(
                "--capture-pass requires --headless and --nforge/--dsl-json"
            ));
        }
        if cli.output.is_none() {
            return Err(anyhow!("--capture-pass requires --output <absolute path>"));
        }
        if cli.profile {
            return Err(anyhow!("--capture-pass cannot be combined with --profile"));
        }
    }
    if let Some(format) = cli.profile_format.as_deref()
        && format != "ndjson"
    {
        return Err(anyhow!(
            "unsupported --profile-format {format:?}; currently supported: ndjson"
        ));
    }
    if cli.profile && cli.profile_frames == 0 {
        cli.profile_frames = 1;
    }
    if cli.profile_frames > 0 && !cli.profile {
        return Err(anyhow!("--profile-frames requires --profile"));
    }
    if cli.profile_warmup_frames > 0 && !cli.profile {
        return Err(anyhow!("--profile-warmup-frames requires --profile"));
    }
    if cli.profile_output.is_some() && !cli.profile {
        return Err(anyhow!("--profile-output requires --profile"));
    }
    if cli.profile_format.is_some() && !cli.profile {
        return Err(anyhow!("--profile-format requires --profile"));
    }

    if cli.trace_animation {
        if cli.nforge.is_none() && cli.dsl_json.is_none() {
            return Err(anyhow!(
                "--trace-animation requires --nforge <file.nforge> or --dsl-json <scene.json>"
            ));
        }
        if cli.profile
            || cli.render_to_file
            || cli.dump_wgsl_dir.is_some()
            || cli.continuous_redraw
            || cli.headless
        {
            return Err(anyhow!(
                "--trace-animation is CPU-only and cannot be combined with --headless, --profile, --render-to-file, --dump-wgsl-dir, or --continuous-redraw"
            ));
        }
        if cli.trace_scenario.is_some() && cli.trace_events.is_some() {
            return Err(anyhow!(
                "--trace-events is ignored when --trace-scenario is set; remove one of them"
            ));
        }
        if let Some(fps) = cli.trace_fps
            && fps == 0
        {
            return Err(anyhow!("--trace-fps must be > 0"));
        }
        if let Some(seconds) = cli.trace_seconds
            && (!seconds.is_finite() || seconds < 0.0)
        {
            return Err(anyhow!("--trace-seconds must be finite and >= 0"));
        }
        if cli.trace_scenario.is_none() && cli.trace_seconds.is_none() {
            // Default free-run duration when no scenario is provided.
            cli.trace_seconds = Some(2.0);
        }
    } else {
        let has_trace_opt = cli.trace_scenario.is_some()
            || cli.trace_seconds.is_some()
            || cli.trace_fps.is_some()
            || cli.trace_events.is_some()
            || cli.trace_output.is_some()
            || cli.trace_format.is_some()
            || cli.trace_channels.is_some()
            || cli.trace_overrides.is_some()
            || cli.trace_initial_state.is_some()
            || cli.trace_fail_on_jump;
        if has_trace_opt {
            return Err(anyhow!("trace options require --trace-animation"));
        }
    }

    Ok(cli)
}

fn run_trace_animation(cli: &Cli) -> Result<()> {
    let (scene, source) = if let Some(path) = cli.nforge.as_ref() {
        let (scene, _) = asset_store::load_from_nforge(path)
            .map_err(|e| anyhow!("failed to load {}: {e:#}", path.display()))?;
        (scene, Some(path.display().to_string()))
    } else if let Some(path) = cli.dsl_json.as_ref() {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("failed to read {}: {e}", path.display()))?;
        let scene: dsl::SceneDSL = serde_json::from_str(&text)
            .map_err(|e| anyhow!("failed to parse {}: {e}", path.display()))?;
        (scene, Some(path.display().to_string()))
    } else {
        bail!("--trace-animation requires --nforge or --dsl-json");
    };

    let mut formats = parse_trace_formats(cli.trace_format.as_deref().unwrap_or("json,summary"))?;
    let pretty = cli.trace_pretty.unwrap_or(true);
    let output = cli
        .trace_output
        .clone()
        .unwrap_or_else(|| PathBuf::from("-"));
    // A concrete --trace-output path always implies a JSON write so the file
    // is never silently skipped when the user only asked for summary/table.
    if output != PathBuf::from("-") && !formats.contains(&TraceFormat::Json) {
        formats.insert(0, TraceFormat::Json);
    }

    let mut analyze = TraceAnalyzeConfig::default();
    if let Some(v) = cli.trace_jump_threshold_channel {
        analyze.jump_threshold_channel = v;
    }
    if let Some(v) = cli.trace_jump_threshold_override {
        analyze.jump_threshold_override = v;
    }
    if let Some(v) = cli.trace_check_identity {
        analyze.check_physic_identity = v;
    }
    analyze.fail_on_jump = cli.trace_fail_on_jump;

    let channel_filter = KeyFilter::from_cli_csv(cli.trace_channels.as_deref())?;
    let override_filter = KeyFilter::from_cli_csv(cli.trace_overrides.as_deref())?;
    let include_values = match cli.trace_values.as_deref() {
        None | Some("all") | Some("filter") => true,
        Some("none") => false,
        Some(other) => bail!("unsupported --trace-values {other:?}; use all|none|filter"),
    };

    let result = if let Some(scenario_path) = cli.trace_scenario.as_ref() {
        let scenario = TraceScenario::from_path(scenario_path)?;
        if let Some(cli_fps) = cli.trace_fps
            && cli_fps != scenario.fps
        {
            bail!(
                "--trace-fps ({cli_fps}) conflicts with scenario fps ({})",
                scenario.fps
            );
        }
        let mut config = scenario.into_run_config(source);
        config.initial_state = cli.trace_initial_state.clone();
        if cli.trace_channels.is_some() {
            config.channel_filter = channel_filter;
        }
        if cli.trace_overrides.is_some() {
            config.override_filter = override_filter;
        }
        config.include_values = include_values;
        // CLI analyze overrides scenario defaults when explicitly set.
        if cli.trace_jump_threshold_channel.is_some()
            || cli.trace_jump_threshold_override.is_some()
            || cli.trace_check_identity.is_some()
            || cli.trace_fail_on_jump
        {
            config.analyze = analyze;
        } else {
            config.analyze.fail_on_jump = cli.trace_fail_on_jump;
        }
        if cli.trace_initial_state.is_some() {
            config.routing_forced = true;
        }
        run_trace(&scene, &config)?
    } else {
        let fps = cli.trace_fps.unwrap_or(60);
        let seconds = cli.trace_seconds.unwrap_or(2.0);
        let include_end = cli.trace_include_end.unwrap_or(true);
        let schedule = TickSchedule::new(0.0, seconds, fps, include_end)?;
        let events = if let Some(path) = cli.trace_events.as_ref() {
            load_event_schedule(path)?
        } else {
            Vec::new()
        };
        let mut config = TraceRunConfig::free_run_meta(
            fps,
            source,
            channel_filter,
            override_filter,
            include_values,
            analyze,
            cli.trace_initial_state.clone(),
        );
        if cli.trace_initial_state.is_some() {
            config.routing_forced = true;
        }
        run_schedule_trace(&scene, &schedule, &events, &config)?
    };

    if formats.contains(&TraceFormat::Json) {
        write_report_json(&result.report, &output, pretty)?;
        if output != PathBuf::from("-") {
            eprintln!(
                "[trace-animation] wrote {} frames to {}",
                result.report.summary.frame_count,
                output.display()
            );
        }
    }
    if formats.contains(&TraceFormat::Summary) {
        let text = format_summary(&result.report);
        if formats.contains(&TraceFormat::Json) && output == PathBuf::from("-") {
            eprintln!("{text}");
        } else {
            println!("{text}");
        }
    }
    if formats.contains(&TraceFormat::Table) {
        let text = format_table(&result.report, 200);
        if formats.contains(&TraceFormat::Json) && output == PathBuf::from("-") {
            eprintln!("{text}");
        } else {
            println!("{text}");
        }
    }

    if let Some(error) = result.assert_error {
        eprintln!("[trace-animation] {error}");
        std::process::exit(result.exit_code);
    }
    if result.exit_code != 0 {
        eprintln!(
            "[trace-animation] exiting with code {} (fail-on-jump or assert)",
            result.exit_code
        );
        std::process::exit(result.exit_code);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TraceFormat {
    Json,
    Summary,
    Table,
}

fn parse_trace_formats(raw: &str) -> Result<Vec<TraceFormat>> {
    let mut out = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part {
            "json" => out.push(TraceFormat::Json),
            "summary" => out.push(TraceFormat::Summary),
            "table" => out.push(TraceFormat::Table),
            other => bail!("unsupported --trace-format token {other:?}; use json,summary,table"),
        }
    }
    if out.is_empty() {
        bail!("--trace-format must list at least one of json,summary,table");
    }
    Ok(out)
}

fn headless_profile_options(cli: &Cli) -> Option<HeadlessProfileOptions> {
    cli.profile.then(|| HeadlessProfileOptions {
        config: profile::ProfileRunConfig {
            frames: cli.profile_frames.max(1),
            warmup_frames: cli.profile_warmup_frames,
        },
        output: cli
            .profile_output
            .clone()
            .map(profile::ProfileOutputTarget::File)
            .unwrap_or(profile::ProfileOutputTarget::Stdout),
    })
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

fn write_text_file(path: PathBuf, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow!(
                "failed to create output directory {}: {e}",
                parent.display()
            )
        })?;
    }
    std::fs::write(&path, contents).map_err(|e| anyhow!("failed to write {}: {e}", path.display()))
}

fn apply_headless_state_at_zero(
    scene: &mut dsl::SceneDSL,
    selector: Option<&str>,
    overrides_output: Option<&PathBuf>,
) -> Result<()> {
    let Some(selector) = selector else {
        return Ok(());
    };
    let state_machine = scene
        .state_machine
        .as_ref()
        .ok_or_else(|| anyhow!("--headless-state requires a scene state machine"))?;
    let id_match = state_machine.states.iter().find(|state| {
        state.state_type == AnimationStateType::AnimationState && state.id == selector
    });
    let state_id = if let Some(state) = id_match {
        state.id.clone()
    } else {
        let name_matches = state_machine
            .states
            .iter()
            .filter(|state| {
                state.state_type == AnimationStateType::AnimationState && state.name == selector
            })
            .collect::<Vec<_>>();
        match name_matches.as_slice() {
            [state] => state.id.clone(),
            [] => bail!("selectable animation state not found: {selector:?}"),
            _ => bail!("animation state name is ambiguous: {selector:?}"),
        }
    };

    let mut session = AnimationSession::from_scene(scene)?
        .ok_or_else(|| anyhow!("--headless-state requires a scene state machine"))?;
    let step = session.force_state(&state_id)?;
    if step.scene_time_secs != 0.0 {
        bail!(
            "headless state evaluation must remain at t=0, got {}",
            step.scene_time_secs
        );
    }
    if let Some(transition_id) = step.active_transition_id.as_deref() {
        bail!("headless state evaluation unexpectedly entered transition {transition_id:?}");
    }
    if !step.diagnostics.is_empty() {
        bail!(
            "headless state derivation diagnostics for {state_id}: {}",
            step.diagnostics.join("; ")
        );
    }

    if let Some(output_path) = overrides_output {
        let ordered_overrides = step
            .active_overrides
            .iter()
            .map(|(key, value)| (format!("{}:{}", key.node_id, key.param_name), value.clone()))
            .collect::<BTreeMap<_, _>>();
        let payload = serde_json::json!({
            "stateId": step.current_state_id,
            "sceneTimeSeconds": step.scene_time_secs,
            "activeTransitionId": step.active_transition_id,
            "overrides": ordered_overrides,
        });
        write_text_file(
            output_path.clone(),
            &serde_json::to_string_pretty(&payload)?,
        )?;
    }

    node_forge_render_server::state_machine::apply_overrides(scene, &step.active_overrides);
    eprintln!("[headless] forced state at t=0: {selector} ({state_id})");
    Ok(())
}

fn dump_scene_wgsl(
    scene: &dsl::SceneDSL,
    store: Option<&asset_store::AssetStore>,
    dump_dir: Option<&PathBuf>,
) -> Result<()> {
    let Some(dump_dir) = dump_dir else {
        return Ok(());
    };

    let bundles = renderer::build_all_pass_wgsl_bundles_from_scene_with_assets(scene, store)?;
    for (pass_id, bundle) in bundles {
        write_text_file(
            dump_dir.join(format!("{pass_id}.vertex.wgsl")),
            &bundle.vertex,
        )?;
        write_text_file(
            dump_dir.join(format!("{pass_id}.fragment.wgsl")),
            &bundle.fragment,
        )?;
        write_text_file(
            dump_dir.join(format!("{pass_id}.module.wgsl")),
            &bundle.module,
        )?;
        if let Some(compute) = bundle.compute.as_ref() {
            write_text_file(dump_dir.join(format!("{pass_id}.compute.wgsl")), compute)?;
        }
    }

    eprintln!("[headless] dumped wgsl: {}", dump_dir.display());
    Ok(())
}

fn load_scene_from_dsl_json_path(
    dsl_json_path: &std::path::Path,
) -> Result<(dsl::SceneDSL, asset_store::AssetStore)> {
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

    let base_dir = dsl_json_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let store = asset_store::load_from_scene_dir(&scene, base_dir)?;
    Ok((scene, store))
}

fn pass_debug_source_for_scene(
    scene: &dsl::SceneDSL,
    store: Option<&asset_store::AssetStore>,
    pass_name: &str,
) -> Result<renderer::PassDebugSource> {
    let bundles = renderer::build_all_pass_wgsl_bundles_from_scene_with_assets(scene, store)?;
    let available_passes = bundles
        .iter()
        .map(|(pass_id, _)| pass_id.clone())
        .collect::<Vec<_>>();
    let Some((pass_id, bundle)) = bundles
        .into_iter()
        .find(|(pass_id, _)| pass_id == pass_name)
    else {
        return Err(anyhow!(
            "shader pass `{}` not found; available passes: {}",
            pass_name,
            available_passes.join(", ")
        ));
    };
    Ok(renderer::PassDebugSource::from_wgsl(pass_id, bundle.module))
}

fn expanded_dependency_trees_for_dump(
    source: &renderer::PassDebugSource,
) -> BTreeMap<String, renderer::PassDebugDependencyNode> {
    source
        .dependency_trees
        .iter()
        .map(|(target_id, tree)| {
            (
                target_id.clone(),
                expand_dependency_node_for_dump(tree, source, &mut HashSet::new()),
            )
        })
        .collect()
}

fn expand_dependency_node_for_dump(
    node: &renderer::PassDebugDependencyNode,
    source: &renderer::PassDebugSource,
    reference_stack: &mut HashSet<String>,
) -> renderer::PassDebugDependencyNode {
    let mut expanded = node.clone();
    let mut inserted_reference_target = None;
    let source_children = node
        .reference
        .then(|| node.target_id.as_deref())
        .flatten()
        .and_then(|target_id| {
            if reference_stack.insert(target_id.to_string()) {
                inserted_reference_target = Some(target_id.to_string());
                source
                    .dependency_trees
                    .get(target_id)
                    .map(|tree| tree.children.as_slice())
            } else {
                None
            }
        })
        .unwrap_or_else(|| node.children.as_slice());

    expanded.children = source_children
        .iter()
        .map(|child| expand_dependency_node_for_dump(child, source, reference_stack))
        .collect();

    if let Some(target_id) = inserted_reference_target {
        reference_stack.remove(&target_id);
    }

    expanded
}

fn shader_dependency_dump_json(source: &renderer::PassDebugSource) -> Result<String> {
    let mut targets = source.dependency_targets.iter().collect::<Vec<_>>();
    targets.sort_by(|a, b| a.id.cmp(&b.id));

    let dump = ShaderDependencyDump {
        pass_name: source.pass_name.as_str(),
        dependency_root_target_id: source.dependency_root_target_id.as_deref(),
        targets,
        dependency_trees: expanded_dependency_trees_for_dump(source),
        parse_error: source.parse_error.as_deref(),
        dependency_error: source.dependency_error.as_deref(),
    };

    serde_json::to_string_pretty(&dump)
        .map_err(|e| anyhow!("failed to serialize dependency dump: {e}"))
}

fn run_shader_dependency_dump(cli: &Cli) -> Result<()> {
    let pass_name = cli
        .dump_shader_deps
        .as_deref()
        .ok_or_else(|| anyhow!("--dump-shader-deps requires a pass name"))?;

    let (scene, store) = if let Some(nforge_path) = cli.nforge.as_deref() {
        asset_store::load_from_nforge(nforge_path)?
    } else if let Some(dsl_json_path) = cli.dsl_json.as_deref() {
        load_scene_from_dsl_json_path(dsl_json_path)?
    } else {
        return Err(anyhow!(
            "--dump-shader-deps requires --dsl-json <scene.json> or --nforge <file.nforge>"
        ));
    };

    let source = pass_debug_source_for_scene(&scene, Some(&store), pass_name)?;
    let json = shader_dependency_dump_json(&source)?;

    if let Some(output_path) = cli.dump_shader_deps_output.as_ref() {
        write_text_file(output_path.clone(), &json)?;
        println!("[shader-deps] dumped: {}", output_path.display());
    } else {
        println!("{json}");
    }

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
    dump_wgsl_dir: Option<PathBuf>,
    render_to_file: bool,
    profile: Option<HeadlessProfileOptions>,
    headless_state: Option<String>,
    headless_overrides_output: Option<PathBuf>,
    export_texture: Option<String>,
    capture_pass: Option<String>,
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
    apply_headless_state_at_zero(
        &mut scene,
        headless_state.as_deref(),
        headless_overrides_output.as_ref(),
    )?;

    // Load assets from the scene directory if the scene has an assets manifest.
    let base_dir = dsl_json_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let store = asset_store::load_from_scene_dir(&scene, base_dir)?;
    dump_scene_wgsl(&scene, Some(&store), dump_wgsl_dir.as_ref())?;

    let out_path = if render_to_file || export_texture.is_some() || capture_pass.is_some() {
        let out =
            output.ok_or_else(|| anyhow!("--render-to-file requires --output <absolute path>"))?;
        validate_absolute_output_path(&out)?;
        out
    } else {
        let rt = dsl::file_render_target(&scene)?
            .ok_or_else(|| anyhow!("--dsl-json headless render requires RenderTarget=File (or pass --render-to-file --output <abs/path>)"))?;

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

    if let Some(pass_name) = capture_pass.as_deref() {
        renderer::render_scene_pass_to_file_headless(&scene, pass_name, &out_path, Some(&store))?;
        println!(
            "[headless] captured pass {pass_name}: {}",
            out_path.display()
        );
    } else if let Some(texture_name) = export_texture.as_deref() {
        renderer::render_scene_texture_to_file_headless(
            &scene,
            texture_name,
            &out_path,
            Some(&store),
        )?;
        println!(
            "[headless] saved texture {texture_name}: {}",
            out_path.display()
        );
    } else if let Some(profile) = profile {
        let stdout_profile = profile.output.is_stdout();
        let mut writer = profile::ProfileWriter::new(&profile.output)?;
        renderer::render_scene_to_file_headless_profiled(
            &scene,
            &out_path,
            Some(&store),
            &profile.config,
            &mut writer,
        )?;
        eprintln!("[headless] saved: {}", out_path.display());
        if !stdout_profile {
            eprintln!("[headless] profile saved");
        }
    } else {
        renderer::render_scene_to_file_headless(&scene, &out_path, Some(&store))?;
        println!("[headless] saved: {}", out_path.display());
    }
    Ok(())
}

fn run_headless_nforge_render_once(
    nforge_path: &std::path::Path,
    output_dir: Option<PathBuf>,
    output: Option<PathBuf>,
    dump_wgsl_dir: Option<PathBuf>,
    render_to_file: bool,
    profile: Option<HeadlessProfileOptions>,
    headless_state: Option<String>,
    headless_overrides_output: Option<PathBuf>,
    export_texture: Option<String>,
    capture_pass: Option<String>,
) -> Result<()> {
    let (mut scene, store) = asset_store::load_from_nforge(nforge_path)?;
    apply_headless_state_at_zero(
        &mut scene,
        headless_state.as_deref(),
        headless_overrides_output.as_ref(),
    )?;
    dump_scene_wgsl(&scene, Some(&store), dump_wgsl_dir.as_ref())?;

    let out_path = if render_to_file || export_texture.is_some() || capture_pass.is_some() {
        let out =
            output.ok_or_else(|| anyhow!("--render-to-file requires --output <absolute path>"))?;
        validate_absolute_output_path(&out)?;
        out
    } else {
        let rt = dsl::file_render_target(&scene)?
            .ok_or_else(|| anyhow!("--nforge headless render requires RenderTarget=File (or pass --render-to-file --output <abs/path>)"))?;

        if let Some(out) = output {
            validate_absolute_output_path(&out)?;
            out
        } else {
            let output_dir = output_dir.unwrap_or_else(|| {
                nforge_path
                    .parent()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| PathBuf::from("."))
            });
            resolve_file_output_path_under(&output_dir, &rt)
        }
    };

    ensure_parent_dir_exists(&out_path)?;

    if let Some(pass_name) = capture_pass.as_deref() {
        renderer::render_scene_pass_to_file_headless(&scene, pass_name, &out_path, Some(&store))?;
        println!(
            "[headless] captured pass {pass_name}: {}",
            out_path.display()
        );
    } else if let Some(texture_name) = export_texture.as_deref() {
        renderer::render_scene_texture_to_file_headless(
            &scene,
            texture_name,
            &out_path,
            Some(&store),
        )?;
        println!(
            "[headless] saved texture {texture_name}: {}",
            out_path.display()
        );
    } else if let Some(profile) = profile {
        let stdout_profile = profile.output.is_stdout();
        let mut writer = profile::ProfileWriter::new(&profile.output)?;
        renderer::render_scene_to_file_headless_profiled(
            &scene,
            &out_path,
            Some(&store),
            &profile.config,
            &mut writer,
        )?;
        eprintln!("[headless] saved: {}", out_path.display());
        if !stdout_profile {
            eprintln!("[headless] profile saved");
        }
    } else {
        renderer::render_scene_to_file_headless(&scene, &out_path, Some(&store))?;
        println!("[headless] saved: {}", out_path.display());
    }
    Ok(())
}

fn run_headless_ws_render_once(
    addr: &str,
    output: Option<PathBuf>,
    dump_wgsl_dir: Option<PathBuf>,
    render_to_file: bool,
    profile: Option<HeadlessProfileOptions>,
) -> Result<()> {
    use std::{thread, time::Duration};

    let (scene_tx, scene_rx) = crossbeam_channel::bounded::<ws::SceneUpdate>(1);
    let app_scene_rx = scene_rx.clone();
    let drop_rx = scene_rx;

    let last_good = Arc::new(Mutex::new(None));
    let hub = ws::WsHub::default();
    let asset_store = asset_store::AssetStore::new();
    // Bind errors must be fatal in headless mode; otherwise we'd block forever waiting for DSL.
    let _ws_handle = ws::spawn_ws_server(
        addr,
        scene_tx,
        drop_rx,
        hub.clone(),
        last_good,
        asset_store.clone(),
        None,
    )?;

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
                perf_trace: _,
            } => {
                dump_scene_wgsl(&scene, None, dump_wgsl_dir.as_ref())?;

                let out_path = if render_to_file {
                    let out = output.clone().ok_or_else(|| {
                        anyhow!("--render-to-file requires --output <absolute path>")
                    })?;
                    validate_absolute_output_path(&out)?;
                    out
                } else {
                    let rt = dsl::file_render_target(&scene)?
                        .ok_or_else(|| anyhow!("--headless mode requires RenderTarget=File (or pass --render-to-file --output <abs/path>)"))?;

                    if let Some(out) = output.clone() {
                        validate_absolute_output_path(&out)?;
                        out
                    } else {
                        resolve_file_output_path(&rt)
                    }
                };

                ensure_parent_dir_exists(&out_path)?;

                let result = if let Some(profile) = profile.as_ref() {
                    let mut writer = profile::ProfileWriter::new(&profile.output)?;
                    renderer::render_scene_to_file_headless_profiled(
                        &scene,
                        &out_path,
                        None,
                        &profile.config,
                        &mut writer,
                    )
                } else {
                    renderer::render_scene_to_file_headless(&scene, &out_path, None)
                };
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
                            if profile.is_some() {
                                eprintln!("Rendered to file at {}", out_path.display());
                                eprintln!("[headless]: {}", text);
                            } else {
                                println!("Rendered to file at {}", out_path.display());
                                println!("[headless]: {}", text);
                            }
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
                            if profile.is_some() {
                                eprintln!("[headless]: {}", text);
                            } else {
                                println!("[headless]: {}", text);
                            }
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
            ws::SceneUpdate::DebugArtifactUpsert { .. }
            | ws::SceneUpdate::DebugArtifactBinaryUpsert { .. }
            | ws::SceneUpdate::DebugArtifactDelete { .. } => {
                // Debug artifacts do not affect headless render output.
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
        "GeistMono-Regular".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../assets/fonts/GeistMono/GeistMono-Regular.ttf"
        ))
        .into(),
    );
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
    fonts.families.insert(
        egui::FontFamily::Name("geist_mono".into()),
        vec!["GeistMono-Regular".to_string()],
    );

    ctx.set_fonts(fonts);
}

pub(crate) fn run() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cli = parse_cli(&argv)?;

    if cli.trace_animation {
        return run_trace_animation(&cli);
    }

    if cli.dump_shader_deps.is_some() {
        return run_shader_dependency_dump(&cli);
    }

    // Script-friendly mode: pass DSL JSON directly.
    if cli.headless {
        let profile_options = headless_profile_options(&cli);
        if let Some(nforge_path) = cli.nforge.as_deref() {
            return run_headless_nforge_render_once(
                nforge_path,
                cli.output_dir,
                cli.output,
                cli.dump_wgsl_dir,
                cli.render_to_file,
                profile_options.clone(),
                cli.headless_state.clone(),
                cli.headless_overrides_output.clone(),
                cli.export_texture.clone(),
                cli.capture_pass.clone(),
            );
        }
        if let Some(dsl_json_path) = cli.dsl_json.as_deref() {
            return run_headless_json_render_once(
                dsl_json_path,
                cli.output_dir,
                cli.output,
                cli.dump_wgsl_dir,
                cli.render_to_file,
                profile_options.clone(),
                cli.headless_state.clone(),
                cli.headless_overrides_output.clone(),
                cli.export_texture.clone(),
                cli.capture_pass.clone(),
            );
        }

        // Editor-driven mode: wait for editor to connect over ws and send SceneDSL.
        return run_headless_ws_render_once(
            "127.0.0.1:8080",
            cli.output,
            cli.dump_wgsl_dir,
            cli.render_to_file,
            profile_options,
        );
    }

    let startup_nforge_path = cli.nforge.clone();
    let (scene, startup_asset_store, startup_debug_artifacts) = if let Some(nforge_path) =
        cli.nforge.as_deref()
    {
        match asset_store::load_from_nforge_with_debug_artifacts(nforge_path) {
            Ok(loaded) => (
                Some(loaded.scene),
                loaded.asset_store,
                loaded.debug_artifacts,
            ),
            Err(e) => {
                eprintln!(
                    "[startup] failed to load .nforge {}; showing error fallback screen: {e:#}",
                    nforge_path.display()
                );
                (
                    None,
                    asset_store::AssetStore::new(),
                    node_forge_render_server::debug_artifacts::DebugArtifactStore::default(),
                )
            }
        }
    } else {
        match dsl::load_scene_from_default_asset() {
            Ok(s) => (
                Some(s),
                asset_store::AssetStore::new(),
                node_forge_render_server::debug_artifacts::DebugArtifactStore::default(),
            ),
            Err(e) => {
                eprintln!(
                    "[startup] failed to load/parse default scene; showing error fallback screen: {e:#}"
                );
                (
                    None,
                    asset_store::AssetStore::new(),
                    node_forge_render_server::debug_artifacts::DebugArtifactStore::default(),
                )
            }
        }
    };

    let resolution_hint = scene
        .as_ref()
        .and_then(dsl::screen_resolution)
        .unwrap_or([1024, 1024]);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_resizable(true)
            .with_transparent(false)
            .with_decorations(true)
            .with_icon(
                eframe::icon_data::from_png_bytes(
                    &include_bytes!("../assets/icons/node-forge-icon.png")[..],
                )
                .expect("embedded render server icon must be a valid PNG"),
            )
            .with_fullsize_content_view(false)
            .with_titlebar_shown(true)
            .with_title_shown(true)
            .with_titlebar_buttons_shown(true)
            .with_has_shadow(true)
            // Include the default sidebar width up front so the first canvas fit
            // uses the same viewport geometry as a manual reset.
            .with_inner_size(app::default_main_window_size(resolution_hint))
            // Keep the OS window non-resizable, but don't tie the minimum size to the scene
            // resolution; UI mode (sidebar/canvas toggle + one-shot startup sizing) is the source
            // of truth and may need to grow/shrink the viewport independently.
            .with_min_inner_size(egui::vec2(240.0, 240.0)),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew({
                let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
                setup.device_descriptor = std::sync::Arc::new(|adapter| {
                    let mut required_features = wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER;
                    if adapter
                        .features()
                        .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
                    {
                        required_features |=
                            wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
                    }
                    if adapter.features().contains(wgpu::Features::SHADER_F16) {
                        required_features |= wgpu::Features::SHADER_F16;
                    }
                    if adapter
                        .features()
                        .contains(wgpu::Features::POLYGON_MODE_LINE)
                    {
                        required_features |= wgpu::Features::POLYGON_MODE_LINE;
                    }
                    if adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
                        required_features |= wgpu::Features::TIMESTAMP_QUERY;
                    }
                    if adapter
                        .features()
                        .contains(wgpu::Features::PIPELINE_STATISTICS_QUERY)
                    {
                        required_features |= wgpu::Features::PIPELINE_STATISTICS_QUERY;
                    }
                    wgpu::DeviceDescriptor {
                        label: Some("eframe wgpu device"),
                        required_features,
                        ..Default::default()
                    }
                });
                setup
            }),
            // Use Rgba16Float surface for HDR-native preview (macOS EDR).
            preferred_surface_format: Some(wgpu::TextureFormat::Rgba16Float),
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
                scene_output_texture_name,
                export_texture_name,
                export_encode_pass_name,
                passes,
                pass_debug_sources,
                last_good_initial,
                uniform_scene,
                matrix_source_scene,
                last_pipeline_signature,
                gaussian_blur_bundles,
            ) = if let Some(scene) = scene.clone() {
                match renderer::ShaderSpaceBuilder::new(
                    Arc::new(render_state.device.clone()),
                    Arc::new(render_state.queue.clone()),
                )
                .with_adapter(render_state.adapter.clone())
                .with_asset_store(startup_asset_store.clone())
                .with_options(renderer::ShaderSpaceBuildOptions {
                    presentation_mode: renderer::ShaderSpacePresentationMode::UiHdrNative,
                    debug_dump_wgsl_dir: None,
                    pass_shader_overrides: Default::default(),
                    strict_pass_shader_overrides: false,
                })
                .build(&scene)
                {
                    Ok(result) => (
                        result.shader_space,
                        result.resolution,
                        result.present_output_texture,
                        result.scene_output_texture,
                        result.export_output_texture,
                        result.export_encode_pass_name,
                        result.pass_bindings,
                        result.pass_debug_sources,
                        Some(scene),
                        result.prepared_scene,
                        result.matrix_source_scene,
                        Some(result.pipeline_signature),
                        result.gaussian_blur_bundles,
                    ),
                    Err(e) => {
                        eprintln!(
                            "[startup] scene build failed; showing error fallback screen: {e:#}"
                        );
                        let result = renderer::ShaderSpaceBuilder::new(
                            Arc::new(render_state.device.clone()),
                            Arc::new(render_state.queue.clone()),
                        )
                        .with_adapter(render_state.adapter.clone())
                        .build_error(resolution_hint)?;
                        (
                            result.shader_space,
                            result.resolution,
                            result.present_output_texture,
                            result.scene_output_texture,
                            result.export_output_texture,
                            result.export_encode_pass_name,
                            result.pass_bindings,
                            std::collections::HashMap::new(),
                            None,
                            None,
                            None,
                            None,
                            std::collections::HashMap::new(),
                        )
                    }
                }
            } else {
                let result = renderer::ShaderSpaceBuilder::new(
                    Arc::new(render_state.device.clone()),
                    Arc::new(render_state.queue.clone()),
                )
                .with_adapter(render_state.adapter.clone())
                .build_error(resolution_hint)?;
                (
                    result.shader_space,
                    result.resolution,
                    result.present_output_texture,
                    result.scene_output_texture,
                    result.export_output_texture,
                    result.export_encode_pass_name,
                    result.pass_bindings,
                    std::collections::HashMap::new(),
                    None,
                    None,
                    None,
                    None,
                    std::collections::HashMap::new(),
                )
            };

            // WS scene updates (keep latest only).
            let (scene_tx, scene_rx) = crossbeam_channel::bounded::<ws::SceneUpdate>(1);
            let app_scene_rx = scene_rx.clone();
            let drop_rx = scene_rx;

            let last_good = Arc::new(Mutex::new(last_good_initial));
            let hub = ws::WsHub::default();
            let asset_store = startup_asset_store.clone();
            let template_scene_tx = scene_tx.clone();
            let ui_repaint_ctx = cc.egui_ctx.clone();
            let ui_wake: ws::UiWakeCallback = Arc::new(move || ui_repaint_ctx.request_repaint());
            if let Err(e) = ws::spawn_ws_server(
                "0.0.0.0:8080",
                scene_tx,
                drop_rx,
                hub.clone(),
                last_good.clone(),
                asset_store.clone(),
                Some(ui_wake),
            ) {
                eprintln!("[ws] failed to start ws server: {e:#}");
            }
            spawn_template_watcher(template_scene_tx, last_good.clone(), cc.egui_ctx.clone());
            let capture_state_rx = spawn_metal_capture_state_watcher(cc.egui_ctx.clone());
            if cli.continuous_redraw {
                eprintln!("[capture] forcing continuous redraw via CLI flag");
            }

            let animation_session = last_good
                .lock()
                .ok()
                .and_then(|g| g.as_ref().cloned())
                .and_then(|s| {
                    node_forge_render_server::animation::AnimationSession::from_scene(&s)
                        .ok()
                        .flatten()
                });

            Ok(Box::new(app::App::from_init(app::AppInit {
                shader_space,
                resolution,
                window_resolution: resolution_hint,
                follow_scene_resolution_for_window: false,
                output_texture_name,
                scene_output_texture_name,
                export_texture_name,
                export_encode_pass_name,
                start: Instant::now(),
                passes,
                scene_rx: app_scene_rx,
                capture_state_rx,
                ws_hub: hub,
                last_good,
                uniform_scene,
                matrix_source_scene,
                last_pipeline_signature,
                force_continuous_redraw: cli.continuous_redraw,
                asset_store,
                animation_session,
                pass_debug_sources,
                gaussian_blur_bundles,
                debug_artifacts: startup_debug_artifacts.clone(),
                nforge_path: startup_nforge_path.clone(),
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

    #[test]
    fn parse_cli_headless_state_and_texture_export() {
        let args = vec![
            "--headless".to_string(),
            "--nforge".to_string(),
            "scene.nforge".to_string(),
            "--headless-state".to_string(),
            "Listening".to_string(),
            "--headless-overrides-output".to_string(),
            "/tmp/listening.json".to_string(),
            "--export-texture".to_string(),
            "sys.ilight.IntelligentLight_30.inter".to_string(),
            "--output".to_string(),
            "/tmp/listening.rgba16f".to_string(),
        ];
        let cli = parse_cli(&args).unwrap();
        assert_eq!(cli.headless_state.as_deref(), Some("Listening"));
        assert_eq!(
            cli.headless_overrides_output.as_ref(),
            Some(&PathBuf::from("/tmp/listening.json"))
        );
        assert_eq!(
            cli.export_texture.as_deref(),
            Some("sys.ilight.IntelligentLight_30.inter")
        );
    }

    #[test]
    fn parse_cli_headless_solo_pass_capture() {
        let args = vec![
            "--headless".to_string(),
            "--nforge".to_string(),
            "scene.nforge".to_string(),
            "--capture-pass".to_string(),
            "render.pass.pass26.pass".to_string(),
            "--output".to_string(),
            "/tmp/glow.rgba16f".to_string(),
        ];
        let cli = parse_cli(&args).unwrap();
        assert_eq!(cli.capture_pass.as_deref(), Some("render.pass.pass26.pass"));
        assert!(cli.export_texture.is_none());
    }

    #[test]
    fn parse_cli_rejects_texture_export_with_pass_capture() {
        let args = vec![
            "--headless".to_string(),
            "--nforge".to_string(),
            "scene.nforge".to_string(),
            "--export-texture".to_string(),
            "texture".to_string(),
            "--capture-pass".to_string(),
            "pass".to_string(),
            "--output".to_string(),
            "/tmp/out.rgba16f".to_string(),
        ];
        let error = parse_cli(&args).unwrap_err().to_string();
        assert!(error.contains("mutually exclusive"));
    }

    #[test]
    fn parse_cli_texture_export_requires_an_output_path() {
        let args = vec![
            "--headless".to_string(),
            "--nforge".to_string(),
            "scene.nforge".to_string(),
            "--export-texture".to_string(),
            "sys.ilight.IntelligentLight_30.inter".to_string(),
        ];
        let error = parse_cli(&args).unwrap_err().to_string();
        assert!(error.contains("--export-texture requires --output"));
    }

    #[test]
    fn parse_cli_dump_wgsl_dir() {
        let args = vec![
            "--headless".to_string(),
            "--nforge".to_string(),
            "scene.nforge".to_string(),
            "--dump-wgsl-dir".to_string(),
            "wgsl-out".to_string(),
        ];
        let cli = parse_cli(&args).unwrap();
        assert!(cli.headless);
        assert_eq!(cli.nforge.as_ref().unwrap(), &PathBuf::from("scene.nforge"));
        assert_eq!(
            cli.dump_wgsl_dir.as_ref().unwrap(),
            &PathBuf::from("wgsl-out")
        );
    }

    #[test]
    fn parse_cli_dump_shader_deps() {
        let args = vec![
            "--dsl-json".to_string(),
            "scene.json".to_string(),
            "--dump-shader-deps".to_string(),
            "RenderPass_4".to_string(),
        ];
        let cli = parse_cli(&args).unwrap();
        assert_eq!(cli.dsl_json.as_ref().unwrap(), &PathBuf::from("scene.json"));
        assert_eq!(cli.dump_shader_deps.as_deref(), Some("RenderPass_4"));
    }

    #[test]
    fn parse_cli_dump_shader_deps_output() {
        let args = vec![
            "--nforge".to_string(),
            "scene.nforge".to_string(),
            "--dump-shader-deps".to_string(),
            "RenderPass_4".to_string(),
            "--dump-shader-deps-output".to_string(),
            "/tmp/deps.json".to_string(),
        ];
        let cli = parse_cli(&args).unwrap();
        assert_eq!(cli.nforge.as_ref().unwrap(), &PathBuf::from("scene.nforge"));
        assert_eq!(cli.dump_shader_deps.as_deref(), Some("RenderPass_4"));
        assert_eq!(
            cli.dump_shader_deps_output.as_ref().unwrap(),
            &PathBuf::from("/tmp/deps.json")
        );
    }

    #[test]
    fn parse_cli_dump_shader_deps_requires_scene_input() {
        let args = vec!["--dump-shader-deps".to_string(), "RenderPass_4".to_string()];
        let err = parse_cli(&args).unwrap_err().to_string();
        assert!(err.contains("--dump-shader-deps requires --dsl-json"));
    }

    #[test]
    fn parse_cli_continuous_redraw_flag() {
        let args = vec!["--continuous-redraw".to_string()];
        let cli = parse_cli(&args).unwrap();
        assert!(cli.continuous_redraw);
    }

    #[test]
    fn parse_cli_force_continuous_redraw_alias() {
        let args = vec!["--force-continuous-redraw".to_string()];
        let cli = parse_cli(&args).unwrap();
        assert!(cli.continuous_redraw);
    }

    #[test]
    fn parse_cli_profile_defaults_to_stdout_ndjson() {
        let args = vec![
            "--headless".to_string(),
            "--dsl-json".to_string(),
            "scene.json".to_string(),
            "--profile".to_string(),
        ];
        let cli = parse_cli(&args).unwrap();
        assert!(cli.profile);
        assert_eq!(cli.profile_frames, 1);
        assert_eq!(cli.profile_warmup_frames, 0);
        assert!(cli.profile_output.is_none());
    }

    #[test]
    fn parse_cli_profile_output_and_frames() {
        let args = vec![
            "--headless".to_string(),
            "--dsl-json".to_string(),
            "scene.json".to_string(),
            "--profile".to_string(),
            "--profile-output".to_string(),
            "/tmp/profile.ndjson".to_string(),
            "--profile-frames".to_string(),
            "12".to_string(),
            "--profile-warmup-frames".to_string(),
            "3".to_string(),
        ];
        let cli = parse_cli(&args).unwrap();
        assert!(cli.profile);
        assert_eq!(
            cli.profile_output.as_ref().unwrap(),
            &PathBuf::from("/tmp/profile.ndjson")
        );
        assert_eq!(cli.profile_frames, 12);
        assert_eq!(cli.profile_warmup_frames, 3);
    }

    #[test]
    fn parse_cli_profile_frames_requires_profile() {
        let args = vec!["--profile-frames".to_string(), "2".to_string()];
        let err = parse_cli(&args).unwrap_err().to_string();
        assert!(err.contains("--profile-frames requires --profile"));
    }

    #[test]
    fn parse_cli_trace_animation_defaults() {
        let args = vec![
            "--trace-animation".to_string(),
            "--nforge".to_string(),
            "scene.nforge".to_string(),
        ];
        let cli = parse_cli(&args).unwrap();
        assert!(cli.trace_animation);
        assert_eq!(cli.trace_seconds, Some(2.0));
        assert_eq!(cli.nforge.as_ref(), Some(&PathBuf::from("scene.nforge")));
    }

    #[test]
    fn parse_cli_trace_rejects_gpu_flags() {
        let args = vec![
            "--trace-animation".to_string(),
            "--nforge".to_string(),
            "scene.nforge".to_string(),
            "--headless".to_string(),
        ];
        let err = parse_cli(&args).unwrap_err().to_string();
        assert!(err.contains("CPU-only"));
    }

    #[test]
    fn parse_cli_trace_options_require_mode() {
        let args = vec!["--trace-seconds".to_string(), "1".to_string()];
        let err = parse_cli(&args).unwrap_err().to_string();
        assert!(err.contains("require --trace-animation"));
    }

    #[test]
    fn parse_cli_trace_scenario_and_format() {
        let args = vec![
            "--trace-animation".to_string(),
            "--nforge".to_string(),
            "scene.nforge".to_string(),
            "--trace-scenario".to_string(),
            "scenarios/demo.json".to_string(),
            "--trace-format".to_string(),
            "json,summary,table".to_string(),
            "--trace-fail-on-jump".to_string(),
        ];
        let cli = parse_cli(&args).unwrap();
        assert!(cli.trace_scenario.is_some());
        assert_eq!(cli.trace_format.as_deref(), Some("json,summary,table"));
        assert!(cli.trace_fail_on_jump);
        // Scenario present → no default free-run seconds.
        assert_eq!(cli.trace_seconds, None);
    }

    fn collect_target_nodes<'a>(
        node: &'a renderer::PassDebugDependencyNode,
        target_id: &str,
        out: &mut Vec<&'a renderer::PassDebugDependencyNode>,
    ) {
        if node.target_id.as_deref() == Some(target_id) {
            out.push(node);
        }
        for child in &node.children {
            collect_target_nodes(child, target_id, out);
        }
    }

    #[test]
    fn shader_dependency_dump_glass_final_alpha_reference_keeps_children() {
        let scene_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("render")
            .join("editor-examples")
            .join("glass")
            .join("scene.nforge");
        let (scene, store) = asset_store::load_from_nforge(&scene_path).unwrap();
        let source = pass_debug_source_for_scene(&scene, Some(&store), "RenderPass_4").unwrap();
        let final_alpha_id = source
            .dependency_targets
            .iter()
            .find(|target| target.name == "final_alpha")
            .map(|target| target.id.as_str())
            .expect("final_alpha target");
        let root_id = source
            .dependency_root_target_id
            .as_deref()
            .expect("dependency root target");
        let dependency_trees = expanded_dependency_trees_for_dump(&source);
        let root = dependency_trees.get(root_id).expect("root dependency tree");

        let declaration_start =
            source.module_source.find("var final_alpha").unwrap() + "var ".len();
        let reference_start = source.module_source.find("final_alpha * 1.0").unwrap();
        let mut final_alpha_nodes = Vec::new();
        collect_target_nodes(root, final_alpha_id, &mut final_alpha_nodes);
        let reference_node = final_alpha_nodes
            .iter()
            .copied()
            .find(|node| {
                node.source_range
                    .map(|range| range.start_byte == reference_start)
                    .unwrap_or(false)
                    && !node.children.is_empty()
            })
            .expect("downstream final_alpha reference node with dependency children");

        let range = reference_node.source_range.unwrap();
        assert_eq!(range.start_byte, reference_start);
        assert_ne!(range.start_byte, declaration_start);
        assert_eq!(
            &source.module_source[range.start_byte..range.end_byte],
            "final_alpha"
        );

        assert!(
            !reference_node.children.is_empty(),
            "expected downstream final_alpha reference to keep dependency children"
        );
    }
}
