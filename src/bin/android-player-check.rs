//! Run every canonical archive through desktop and connected Android playback.
//! Usage: cargo run --bin android-player-check -- EXAMPLES WS_URL OUTPUT
use anyhow::{Context, Result, bail};
use node_forge_render_server::{
    nforge, playback::PlaybackRuntime, renderer::ShaderSpacePresentationMode,
    state_machine::graph_function,
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    net::TcpStream,
    path::{Path, PathBuf},
    time::Duration,
};
use tungstenite::{Message, WebSocket};
static GPU_ERRORS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
fn check_gpu() -> Result<()> {
    let errors = std::mem::take(&mut *GPU_ERRORS.lock().unwrap());
    if !errors.is_empty() {
        bail!("Desktop GPU: {}", errors.join("\n"));
    }
    Ok(())
}

type Socket = WebSocket<TcpStream>;
fn send(ws: &mut Socket, kind: &str, payload: Value) -> Result<()> {
    ws.send(Message::Text(
        json!({"type":kind,"timestamp":0,"requestId":kind,"payload":payload}).to_string(),
    ))?;
    Ok(())
}
fn receive(ws: &mut Socket, kind: &str) -> Result<Value> {
    loop {
        match ws.read()? {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(&text)?;
                if value["type"] == "error" || value["type"] == "player_diagnostics" {
                    bail!("{value}");
                }
                if value["type"] == kind {
                    return Ok(value["payload"].clone());
                }
            }
            Message::Close(_) => bail!("Player disconnected"),
            _ => {}
        }
    }
}
fn control(ws: &mut Socket, payload: Value) -> Result<()> {
    send(ws, "player_control", payload)?;
    receive(ws, "player_control_done")?;
    Ok(())
}
fn phone_capture(ws: &mut Socket, expected: [u32; 2]) -> Result<Vec<f32>> {
    send(ws, "player_capture", json!({}))?;
    loop {
        match ws.read()? {
            Message::Binary(bytes) => {
                if bytes.len() < 4 {
                    bail!("Short capture");
                }
                let len = u32::from_be_bytes(bytes[..4].try_into()?) as usize;
                let header: Value = serde_json::from_slice(
                    bytes.get(4..4 + len).context("Invalid capture header")?,
                )?;
                if header["type"] != "player_capture" {
                    continue;
                }
                if header["width"] != expected[0]
                    || header["height"] != expected[1]
                    || header["format"] != "rgba16f"
                {
                    bail!("Capture format/size mismatch: {header}");
                }
                let data = &bytes[4 + len..];
                if data.len() != expected[0] as usize * expected[1] as usize * 8 {
                    bail!("Capture byte length mismatch");
                }
                return Ok(data
                    .chunks_exact(2)
                    .map(|v| half::f16::from_bits(u16::from_le_bytes([v[0], v[1]])).to_f32())
                    .collect());
            }
            Message::Text(text) => {
                let value: Value = serde_json::from_str(&text)?;
                if value["type"] == "error" || value["type"] == "player_diagnostics" {
                    bail!("{value}");
                }
            }
            Message::Close(_) => bail!("Player disconnected during capture"),
            _ => {}
        }
    }
}
fn desktop_capture(runtime: &PlaybackRuntime) -> Result<Vec<f32>> {
    Ok(runtime.capture_linear_rgba()?.2)
}
fn preview(path: &Path, values: &[f32], size: [u32; 2]) -> Result<()> {
    let bytes: Vec<u8> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let v = v.clamp(0.0, 1.0);
            let v = if i % 4 == 3 {
                v
            } else if v <= 0.0031308 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
            (v * 255.0).round() as u8
        })
        .collect();
    image::save_buffer(path, &bytes, size[0], size[1], image::ColorType::Rgba8)?;
    Ok(())
}
fn compare(ws: &mut Socket, runtime: &PlaybackRuntime, out: &Path, label: &str) -> Result<Value> {
    send(ws, "player_snapshot", json!({}))?;
    let snapshot = receive(ws, "player_snapshot")?;
    std::fs::write(
        out.join(format!("{label}.android.json")),
        serde_json::to_vec_pretty(&snapshot)?,
    )?;
    std::fs::write(
        out.join(format!("{label}.desktop.json")),
        serde_json::to_vec_pretty(&runtime.diagnostic_snapshot())?,
    )?;
    check_gpu()?;
    let reference = desktop_capture(runtime)?;
    let actual = phone_capture(ws, runtime.resolution())?;
    if reference.len() != actual.len() {
        bail!("Pixel count mismatch");
    }
    let mut max_abs = 0.0f32;
    let mut sum_sq = 0.0f64;
    let mut over = 0usize;
    let mut exact = 0usize;
    let mut max_reference = 0.0f32;
    let mut max_android = 0.0f32;
    let mut errors = Vec::with_capacity(actual.len());
    for (a, b) in reference.iter().zip(&actual) {
        if !a.is_finite() || !b.is_finite() {
            bail!("Non-finite pixel");
        }
        let error = (a - b).abs();
        max_abs = max_abs.max(error);
        sum_sq += (error as f64).powi(2);
        let relative = error / a.abs().max(1.0);
        if relative > 0.01 {
            over += 1
        };
        if a == b {
            exact += 1
        };
        errors.push(relative);
        max_reference = max_reference.max(*a);
        max_android = max_android.max(*b);
    }
    errors.sort_unstable_by(f32::total_cmp);
    preview(
        &out.join(format!("{label}.desktop.png")),
        &reference,
        runtime.resolution(),
    )?;
    preview(
        &out.join(format!("{label}.android.png")),
        &actual,
        runtime.resolution(),
    )?;
    let report = json!({"sample":label,"resolution":runtime.resolution(),"maxAbsoluteError":max_abs,"rmse":(sum_sq/actual.len() as f64).sqrt(),"p99NormalizedError":errors[(errors.len()-1)*99/100],"fractionOver1Percent":over as f64/actual.len() as f64,"fractionExact":exact as f64/actual.len() as f64,"desktopMax":max_reference,"androidMax":max_android,"withinTolerance":over as f64/actual.len() as f64<=0.001});
    Ok(report)
}
fn run_case(gpu: &HeadlessRenderer, ws: &mut Socket, path: &Path, out: &Path) -> Result<Value> {
    let source_hash = format!("{:x}", Sha256::digest(std::fs::read(path)?));
    let loaded = nforge::load(path)?;
    let resources = graph_function::installed_document_functions();
    let materials = nforge::load_material_sources(path)?
        .into_iter()
        .map(|(id, source)| json!({"nodeId":id,"content":source}))
        .collect::<Vec<_>>();
    if source_hash != format!("{:x}", Sha256::digest(std::fs::read(path)?)) {
        bail!("Archive changed while loading; rerun this case");
    }
    let protocol_scene = loaded.scene.clone();
    for id in loaded.scene.assets.keys() {
        let asset = loaded
            .asset_store
            .get(id)
            .context("Archive asset missing")?;
        let chunk_size = 256 * 1024;
        let count = asset.bytes.len().div_ceil(chunk_size);
        send(
            ws,
            "asset_upload_start",
            json!({"assetId":id,"mimeType":asset.mime_type,"originalName":asset.original_name,"size":asset.bytes.len(),"chunkSize":chunk_size,"totalChunks":count}),
        )?;
        for (i, bytes) in asset.bytes.chunks(chunk_size).enumerate() {
            let header = serde_json::to_vec(
                &json!({"type":"asset_upload_chunk","assetId":id,"chunkIndex":i,"totalChunks":count,"chunkSize":bytes.len(),"offset":i*chunk_size}),
            )?;
            let mut frame = (header.len() as u32).to_be_bytes().to_vec();
            frame.extend(header);
            frame.extend(bytes);
            ws.send(Message::Binary(frame))?;
        }
        send(ws, "asset_upload_end", json!({"assetId":id}))?;
        receive(ws, "asset_upload_ack")?;
    }
    let states = serde_json::to_value(&loaded.scene.state_machine)?;
    send(
        ws,
        "scene_update",
        json!({"scene":loaded.scene,"functions":resources,"materials":materials,"revision":1,"deterministic":true}),
    )?;
    receive(ws, "scene_applied")?;
    let mut runtime = PlaybackRuntime::new_with_presentation(
        gpu.device.clone(),
        gpu.queue.clone(),
        gpu.adapter.clone(),
        loaded.scene,
        loaded.asset_store,
        ShaderSpacePresentationMode::SceneLinear,
    )?;
    let frame = runtime.advance(0.0)?;
    if !frame.diagnostics.is_empty() {
        bail!("Desktop: {:?}", frame.diagnostics)
    }
    check_gpu()?;
    runtime.play();
    std::fs::create_dir_all(out)?;
    let mut samples = vec![compare(ws, &runtime, out, "initial")?];
    for _ in 0..60 {
        let frame = runtime.advance(1.0 / 60.0)?;
        if !frame.diagnostics.is_empty() {
            bail!("Desktop: {:?}", frame.diagnostics)
        }
        control(ws, json!({"action":"step","dt":1.0/60.0}))?;
    }
    samples.push(compare(ws, &runtime, out, "one-second")?);
    if let Some(states) = states["states"].as_array() {
        for (index, state) in states
            .iter()
            .filter(|s| s["type"] == "animationState")
            .enumerate()
        {
            let id = state["id"].as_str().context("State ID missing")?;
            let frame = runtime.force_state(id)?;
            if !frame.diagnostics.is_empty() {
                bail!("Desktop State {id}: {:?}", frame.diagnostics)
            }
            control(ws, json!({"action":"state","stateId":id}))?;
            samples.push(compare(ws, &runtime, out, &format!("state-{index}"))?);
        }
    }
    std::fs::write(
        out.join("samples.json"),
        serde_json::to_vec_pretty(&samples)?,
    )?;
    let protocol = if std::env::var_os("NODE_FORGE_PROTOCOL_CHECK").is_some() {
        // A rejected update must keep the last successfully presented scene and revision.
        send(ws, "player_snapshot", json!({}))?;
        let before = receive(ws, "player_snapshot")?;
        send(
            ws,
            "scene_delta",
            json!({"version":"6.1", "workingColorSpace": "linear-srgb", "dpi": 3.0,"baseRevision":999,"revision":2,"nodes":{"added":[],"updated":[],"removed":[]},"connections":{"added":[],"updated":[],"removed":[]}}),
        )?;
        receive(ws, "scene_resync_request")?;
        let mut invalid = protocol_scene.clone();
        invalid.nodes.clear();
        invalid.connections.clear();
        invalid.groups.clear();
        send(
            ws,
            "scene_update",
            json!({"scene":invalid,"revision":2,"functions":[],"materials":[],"deterministic":true}),
        )?;
        loop {
            if let Message::Text(text) = ws.read()? {
                let value: Value = serde_json::from_str(&text)?;
                if value["type"] == "error" {
                    break;
                }
                if value["type"] == "scene_applied" {
                    bail!("Invalid scene was accepted")
                }
            }
        }
        send(ws, "player_snapshot", json!({}))?;
        if receive(ws, "player_snapshot")? != before {
            bail!("Rejected update changed playback state");
        }
        send(ws, "player_status", json!({}))?;
        if receive(ws, "player_status")?["revision"] != 1 {
            bail!("Rejected scene advanced revision")
        }
        if let Some(node) = protocol_scene
            .nodes
            .iter()
            .find(|n| n.node_type == "FloatInput")
        {
            let mut edited = node.clone();
            let next = edited
                .params
                .get("value")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                + 0.25;
            edited.params.insert("value".into(), json!(next));
            send(
                ws,
                "scene_delta",
                json!({"version":"6.1", "workingColorSpace": "linear-srgb", "dpi": 3.0,"baseRevision":1,"revision":2,"nodes":{"added":[],"updated":[edited],"removed":[]},"connections":{"added":[],"updated":[],"removed":[]}}),
            )?;
            if receive(ws, "scene_applied")?["revision"] != 2 {
                bail!("Uniform delta revision did not commit")
            }
            send(ws, "player_snapshot", json!({}))?;
            let after = receive(ws, "player_snapshot")?;
            if after["uniforms"][&node.id]["value"] != next {
                bail!("Uniform edit did not reach the GPU scene")
            }
            if after["sceneTime"] != before["sceneTime"] || after["stateId"] != before["stateId"] {
                bail!("Uniform edit reset animation playback")
            }
            control(ws, json!({"action":"reset"}))?;
            send(ws, "player_snapshot", json!({}))?;
            if receive(ws, "player_snapshot")?["uniforms"][&node.id]["value"] != next {
                bail!("Reset discarded the updated uniform baseline")
            }
        }
        control(ws, json!({"action":"display","nativePixels":true}))?;
        send(ws, "player_status", json!({}))?;
        if receive(ws, "player_status")?["nativePixels"] != true {
            bail!("1:1 display mode did not activate")
        }
        control(ws, json!({"action":"display","nativePixels":false}))?;
        Some("revision resync, rejected-scene rollback, uniform delta, 1:1 display: passed")
    } else {
        None
    };
    Ok(
        json!({"example":path.file_name().unwrap().to_string_lossy(),"sourceSha256":source_hash,"status":"rendered","samples":samples,"protocol":protocol}),
    )
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        bail!("Usage: android-player-check EXAMPLES WS_URL OUTPUT");
    }
    let out = PathBuf::from(&args[3]);
    std::fs::create_dir_all(&out)?;
    let address = args[2]
        .strip_prefix("ws://")
        .context("Use a USB forwarded ws:// address")?;
    let connect = || -> Result<Socket> {
        let started = std::time::Instant::now();
        loop {
            let result = (|| -> Result<Socket> {
                let stream = TcpStream::connect(address)?;
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                stream.set_write_timeout(Some(Duration::from_secs(30)))?;
                let config = tungstenite::protocol::WebSocketConfig {
                    max_message_size: Some(256 * 1024 * 1024),
                    max_frame_size: Some(256 * 1024 * 1024),
                    ..Default::default()
                };
                let socket =
                    tungstenite::client::client_with_config(&args[2], stream, Some(config))?.0;
                socket
                    .get_ref()
                    .set_read_timeout(Some(Duration::from_secs(90)))?;
                Ok(socket)
            })();
            match result {
                Ok(socket) => return Ok(socket),
                Err(error) if started.elapsed() > Duration::from_secs(30) => return Err(error),
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    };
    let gpu = HeadlessRenderer::new(HeadlessRendererConfig::default())?;
    gpu.device.on_uncaptured_error(std::sync::Arc::new(|error| {
        GPU_ERRORS.lock().unwrap().push(error.to_string());
    }));
    let mut paths = std::fs::read_dir(&args[1])?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|p| p.extension().is_some_and(|e| e == "nforge"));
    paths.sort();
    if let Ok(filter) = std::env::var("NODE_FORGE_EXAMPLE_FILTER") {
        paths.retain(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(&filter)
        });
    }
    let mut results = vec![];
    for path in paths {
        let name = path.file_stem().unwrap().to_string_lossy();
        println!("RUN {name}");
        let mut ws = connect()?;
        let hello = receive(&mut ws, "player_hello")?;
        GPU_ERRORS.lock().unwrap().clear();
        let result = match run_case(&gpu, &mut ws, &path, &out.join(name.as_ref())) {
            Ok(v) => v,
            Err(error) => {
                json!({"example":path.file_name().unwrap().to_string_lossy(),"status":"error","error":format!("{error:#}")})
            }
        };
        let _ = ws.close(None);
        println!("RESULT {result}");
        results.push(result);
        std::fs::write(
            out.join("report.json"),
            serde_json::to_vec_pretty(
                &json!({"android":hello,"desktopGpu":format!("{:?}",gpu.adapter.get_info()),"results":results}),
            )?,
        )?;
    }
    if results.iter().any(|r| r["status"] == "error") {
        bail!("Some examples failed; inspect report.json")
    }
    if results.iter().any(|r| {
        r["samples"].as_array().is_some_and(|samples| {
            samples
                .iter()
                .any(|sample| sample["withinTolerance"] == false)
        })
    }) {
        bail!(
            "All examples rendered, but pixel comparison exceeded the diagnostic tolerance; inspect report.json"
        );
    }
    Ok(())
}
