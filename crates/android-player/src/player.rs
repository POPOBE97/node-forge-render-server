use crate::gpu::Gpu;
use anyhow::{Context, Result, bail};
use node_forge_render_server::{
    dsl::SceneDSL,
    mobile_ws::{
        Bridge, Packet,
        scene_delta::{
            SceneCache, SceneDelta, apply_scene_delta, delta_updates_only_uniform_values,
            materialize_scene_dsl,
        },
    },
    playback::PlaybackRuntime,
    renderer::{ShaderSpacePresentationMode, node_compiler::template_loader},
    state_machine::graph_function::{self, FunctionResource},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::Arc, time::Instant};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Material {
    node_id: String,
    content: String,
}

struct Pending {
    scene: SceneDSL,
    functions: Vec<FunctionResource>,
    materials: Vec<Material>,
    request: Option<String>,
    revision: u64,
    uniform_only: bool,
    last_resource_request: Option<Instant>,
}

pub struct Player {
    pub gpu: Gpu,
    bridge: Arc<Bridge>,
    runtime: Option<PlaybackRuntime>,
    cache: Option<SceneCache>,
    pending: Option<Pending>,
    functions: Vec<FunctionResource>,
    materials: Vec<Material>,
    generation: u64,
    revision: u64,
    frames: u64,
    probe: bool,
    deterministic: bool,
    error: Option<String>,
    awaiting_frame: Option<Option<String>>,
    display_metrics: Value,
}

impl Player {
    pub fn new(mut gpu: Gpu) -> Result<Self> {
        let bridge = Bridge::start()?;
        gpu.native_pixels = bridge.status.lock().unwrap()["nativePixels"]
            .as_bool()
            .unwrap_or(false);
        let player = Self {
            gpu,
            bridge,
            runtime: None,
            cache: None,
            pending: None,
            functions: vec![],
            materials: vec![],
            generation: 0,
            revision: 0,
            frames: 0,
            probe: false,
            deterministic: false,
            error: None,
            awaiting_frame: None,
            display_metrics: Value::Null,
        };
        player.publish();
        player.bridge.send(
            player.bridge.generation(),
            "scene_resync_request",
            None,
            json!({"reason":"Android Surface ready; resend scene"}),
        );
        Ok(player)
    }

    fn publish(&self) {
        *self.bridge.status.lock().unwrap() = json!({
            "ready":true,"hdr":self.gpu.output().hdr(),
            "surfaceFormat":self.gpu.output().format_name(),"colorSpace":self.gpu.output().color_space_name(),
            "bufferSdrRatio":self.gpu.output().buffer_ratio(),
            "screenColorSpace":self.gpu.output().window_space,
            "workingColorSpace":self.runtime.as_ref().map(PlaybackRuntime::working_color_space),
            "gpu":self.gpu.adapter.get_info().name,"backend":"Vulkan",
            "drawable":[self.gpu.config.width,self.gpu.config.height],
            "resolution":self.runtime.as_ref().map(PlaybackRuntime::resolution),
            "stateId":self.runtime.as_ref().and_then(PlaybackRuntime::current_state_id),
            "playing":self.runtime.as_ref().is_some_and(PlaybackRuntime::is_playing),
            "generation":self.generation,"revision":self.revision,"frames":self.frames,"error":self.error,
            "display":self.display_metrics,
            "nativePixels":self.gpu.native_pixels,
        });
    }

    fn error(&mut self, request: Option<&str>, error: &anyhow::Error) {
        let message = format!("{error:#}");
        log::error!("{message}");
        self.error = Some(message.clone());
        self.bridge.send(
            self.generation,
            "error",
            request,
            json!({"code":"PLAYER_ERROR","message":message}),
        );
    }

    pub fn frame(&mut self, dt: f64) -> Result<String> {
        if self.generation != self.bridge.generation() {
            self.generation = self.bridge.generation();
            self.cache = None;
            self.pending = None;
            self.revision = 0;
            self.awaiting_frame = None;
            if let Some(runtime) = &mut self.runtime {
                runtime.pause();
            }
            self.deterministic = false;
        }
        for _ in 0..8 {
            let Some(packet) = self.bridge.take() else {
                break;
            };
            if packet.generation != self.generation {
                continue;
            }
            let request = packet.value["requestId"].as_str().map(str::to_owned);
            if let Err(error) = self.command(packet) {
                self.error(request.as_deref(), &error);
            }
        }
        if let Some(mut pending) = self.pending.take() {
            let missing: Vec<_> = pending
                .scene
                .assets
                .keys()
                .filter(|id| !self.bridge.assets.contains(id))
                .cloned()
                .collect();
            if !missing.is_empty() {
                if pending
                    .last_resource_request
                    .is_none_or(|time| time.elapsed().as_secs_f32() >= 2.0)
                {
                    self.bridge.send(
                        self.generation,
                        "asset_request",
                        None,
                        json!({"assetIds":missing}),
                    );
                    pending.last_resource_request = Some(Instant::now());
                }
                self.pending = Some(pending);
            } else {
                let request = pending.request.clone();
                if let Err(error) = self.apply(pending) {
                    self.error(request.as_deref(), &error);
                }
            }
        }
        if !self.bridge.connected() {
            if let Some(runtime) = &mut self.runtime {
                runtime.pause();
            }
        }
        if let Some(runtime) = &mut self.runtime {
            if !self.deterministic {
                let frame = runtime.advance(dt)?;
                if !frame.diagnostics.is_empty() {
                    self.error = Some(frame.diagnostics.join("\n"));
                    self.bridge.send(
                        self.generation,
                        "player_diagnostics",
                        None,
                        json!({"messages":frame.diagnostics}),
                    );
                }
            }
            let presented = self.gpu.present(
                runtime.present_texture_view(),
                runtime.resolution(),
                self.probe,
            )?;
            if presented {
                self.publish();
                if let Some(request) = self.awaiting_frame.take() {
                    self.bridge.send(self.generation, "scene_applied", request.as_deref(), json!({"revision":self.revision,"generation":self.generation,"resolution":self.runtime.as_ref().map(PlaybackRuntime::resolution),"firstFrameSubmitted":true}));
                }
            }
        } else {
            self.gpu.present(
                None,
                [self.gpu.config.width, self.gpu.config.height],
                self.probe,
            )?;
        }
        self.frames += 1;
        self.publish();
        Ok(if let Some(error) = &self.error {
            error.clone()
        } else if self.pending.is_some() {
            "Node Forge · Syncing scene".into()
        } else if !self.bridge.connected() {
            "Node Forge · Connect from Editor over USB".into()
        } else if self.runtime.is_none() {
            "Node Forge · Waiting for scene".into()
        } else {
            String::new()
        })
    }

    fn apply(&mut self, pending: Pending) -> Result<()> {
        template_loader::validate_document_overrides(
            &pending.scene,
            pending.materials.iter().map(|m| m.node_id.as_str()),
        )?;
        graph_function::validate_function_resources(&pending.scene, &pending.functions)?;
        let same_functions =
            serde_json::to_value(&pending.functions)? == serde_json::to_value(&self.functions)?;
        let same_materials = pending
            .materials
            .iter()
            .map(|m| (&m.node_id, &m.content))
            .eq(self.materials.iter().map(|m| (&m.node_id, &m.content)));
        let updated = pending.uniform_only
            && same_functions
            && same_materials
            && self
                .runtime
                .as_mut()
                .map(|runtime| runtime.update_uniform_scene(&pending.scene))
                .transpose()?
                .unwrap_or(false);
        if !updated {
            graph_function::install_document_functions(pending.functions.clone())?;
            template_loader::install_document_overrides(
                pending
                    .materials
                    .iter()
                    .map(|m| (m.node_id.clone(), m.content.clone())),
            );
            let result = (|| {
                let mut runtime = PlaybackRuntime::new_with_presentation(
                    self.gpu.device.clone(),
                    self.gpu.queue.clone(),
                    self.gpu.adapter.clone(),
                    pending.scene.clone(),
                    self.bridge.assets.clone(),
                    ShaderSpacePresentationMode::UiHdrNative,
                )?;
                let frame = runtime.advance(0.0)?;
                if !frame.diagnostics.is_empty() {
                    bail!("{}", frame.diagnostics.join("\n"));
                }
                runtime.play();
                self.gpu.check_errors()?;
                // Use the authored target, never the float Screen conversion intermediate.
                let source = runtime.shader_space().textures
                    .get(runtime.scene_output_texture_name().as_str())
                    .context("Scene output texture missing")?;
                self.gpu.configure_output(
                    source.wgpu_texture_desc.format,
                    node_forge_render_server::dsl::screen_color_space(&pending.scene)?,
                )?;
                Ok(runtime)
            })();
            match result {
                Ok(runtime) => self.runtime = Some(runtime),
                Err(error) => {
                    let _ = self.gpu.check_errors();
                    graph_function::install_document_functions(self.functions.clone())?;
                    template_loader::install_document_overrides(
                        self.materials
                            .iter()
                            .map(|m| (m.node_id.clone(), m.content.clone())),
                    );
                    if let Some(cache) = &self.cache {
                        if let Some(machine) = &cache.state_machine {
                            graph_function::prepare_state_machine(machine)?;
                        }
                    }
                    return Err(error);
                }
            }
        } else if let Some(runtime) = &mut self.runtime {
            runtime.advance(0.0)?;
            self.gpu.check_errors()?;
        }
        self.functions = pending.functions;
        self.materials = pending.materials;
        self.cache = Some(SceneCache::from_scene_update(&pending.scene));
        self.revision = pending.revision;
        self.probe = false;
        self.error = None;
        self.awaiting_frame = Some(pending.request);
        Ok(())
    }

    fn command(&mut self, packet: Packet) -> Result<()> {
        let value = packet.value;
        let kind = value["type"].as_str().unwrap_or("");
        let payload = &value["payload"];
        let request = value["requestId"].as_str();
        match kind {
            "scene_update" => {
                self.deterministic = payload["deterministic"].as_bool().unwrap_or(false);
                let revision = payload["revision"].as_u64().unwrap_or(self.revision + 1);
                let mut scene: SceneDSL = serde_json::from_value(payload["scene"].clone())?;
                node_forge_render_server::dsl::normalize_scene_defaults(&mut scene)?;
                self.pending = Some(Pending {
                    scene,
                    functions: serde_json::from_value(
                        payload.get("functions").cloned().unwrap_or(json!([])),
                    )?,
                    materials: serde_json::from_value(
                        payload.get("materials").cloned().unwrap_or(json!([])),
                    )?,
                    request: request.map(str::to_owned),
                    revision,
                    uniform_only: false,
                    last_resource_request: None,
                });
            }
            "scene_delta" => {
                let base = self
                    .pending
                    .as_ref()
                    .map(|pending| SceneCache::from_scene_update(&pending.scene))
                    .or_else(|| self.cache.clone());
                let Some(mut cache) = base else {
                    self.bridge.send(
                        self.generation,
                        "scene_resync_request",
                        None,
                        json!({"reason":"missing base scene"}),
                    );
                    return Ok(());
                };
                let previous_revision = self
                    .pending
                    .as_ref()
                    .map_or(self.revision, |pending| pending.revision);
                if payload["baseRevision"]
                    .as_u64()
                    .is_some_and(|base| base != previous_revision)
                {
                    self.bridge.send(
                        self.generation,
                        "scene_resync_request",
                        None,
                        json!({"reason":"revision mismatch"}),
                    );
                    return Ok(());
                }
                let delta: SceneDelta = serde_json::from_value(payload.clone())?;
                let uniform_only = self
                    .pending
                    .as_ref()
                    .is_none_or(|pending| pending.uniform_only)
                    && delta_updates_only_uniform_values(&cache, &delta);
                apply_scene_delta(&mut cache, &delta);
                let mut scene = materialize_scene_dsl(&cache);
                node_forge_render_server::dsl::normalize_scene_defaults(&mut scene)?;
                let functions = self.pending.as_ref().map_or_else(
                    || self.functions.clone(),
                    |pending| pending.functions.clone(),
                );
                let materials = self.pending.as_ref().map_or_else(
                    || self.materials.clone(),
                    |pending| pending.materials.clone(),
                );
                self.pending = Some(Pending {
                    scene,
                    functions,
                    materials,
                    request: request.map(str::to_owned),
                    revision: payload["revision"]
                        .as_u64()
                        .unwrap_or(previous_revision + 1),
                    uniform_only,
                    last_resource_request: None,
                });
            }
            "interaction_event" => {
                if let Some(runtime) = &mut self.runtime {
                    runtime.dispatch(serde_json::from_value(payload.clone())?);
                }
            }
            "player_control" => {
                let action = payload["action"]
                    .as_str()
                    .context("control action missing")?;
                if action == "hdr_probe" {
                    self.probe = true;
                } else if action == "display" {
                    self.gpu.native_pixels = payload["nativePixels"]
                        .as_bool()
                        .context("nativePixels missing")?;
                } else {
                    let runtime = self.runtime.as_mut().context("No loaded scene")?;
                    match action {
                        "pause" => {
                            runtime.pause();
                            self.deterministic = true;
                        }
                        "play" => {
                            runtime.play();
                            self.deterministic = false;
                        }
                        "reset" => {
                            runtime.reset()?;
                            self.deterministic = true;
                        }
                        "state" => {
                            runtime.force_state(
                                payload["stateId"].as_str().context("stateId missing")?,
                            )?;
                        }
                        "step" => {
                            self.deterministic = true;
                            runtime.play();
                            let dt = payload["dt"].as_f64().context("dt missing")?;
                            if !dt.is_finite() || !(0.0..=10.0).contains(&dt) {
                                bail!("invalid step dt");
                            }
                            let frame = runtime.advance(dt)?;
                            if !frame.diagnostics.is_empty() {
                                bail!("{}", frame.diagnostics.join("\n"));
                            }
                        }
                        _ => bail!("unknown control '{action}'"),
                    }
                }
                self.publish();
                self.bridge.send(
                    self.generation,
                    "player_control_done",
                    request,
                    json!({"action":action}),
                );
            }
            "player_capture" => match payload["stage"].as_str().unwrap_or("scene") {
                "scene" => self.capture(request)?,
                "presentation" => {
                    let bytes = self.gpu.capture_hdr_transport()?;
                    self.bridge.send_capture(self.generation, json!({
                        "type":"player_capture", "requestId":request, "stage":"presentation",
                        "width":self.gpu.config.width, "height":self.gpu.config.height,
                        "format":"rgb10a2unorm", "colorSpace":"extended-display-p3",
                        "transfer":"srgb", "bufferSdrRatio":self.gpu.output().buffer_ratio(),
                        "revision":self.revision, "generation":self.generation,
                    }), &bytes)?;
                }
                stage => bail!("Unknown capture stage '{stage}'"),
            },
            "player_snapshot" => {
                let runtime = self.runtime.as_ref().context("No loaded scene")?;
                self.bridge.send(
                    self.generation,
                    "player_snapshot",
                    request,
                    runtime.diagnostic_snapshot(),
                );
            }
            "asset_remove" => {} // Keep content-addressed cached assets until session cleanup.
            _ => bail!("unsupported player message '{kind}'"),
        }
        Ok(())
    }

    pub fn input(&mut self, value: Value) -> Result<()> {
        if value["type"] == "display_metrics" {
            self.display_metrics = value;
            return Ok(());
        }
        if self.deterministic {
            return Ok(());
        }
        let Some(runtime) = &mut self.runtime else {
            return Ok(());
        };
        let [width, height] = runtime.resolution().map(|value| value as f64);
        let (sw, sh) = (self.gpu.config.width as f64, self.gpu.config.height as f64);
        let scale = if self.gpu.native_pixels {
            1.0
        } else {
            (sw / width).min(sh / height)
        };
        let (mut ox, mut oy) = ((sw - width * scale) * 0.5, (sh - height * scale) * 0.5);
        if self.gpu.native_pixels {
            ox = ox.floor();
            oy = oy.floor();
        }
        let x = (value["x"].as_f64().unwrap_or(0.0) - ox) / scale;
        let y = height - (value["y"].as_f64().unwrap_or(0.0) - oy) / scale;
        if x < 0.0 || y < 0.0 || x > width || y > height {
            return Ok(());
        }
        let event = json!({"eventType":value["type"],"seq":self.frames,"data":{"position":{"clientX":x,"clientY":y,"canvasX":x,"canvasY":y,"fragPixelX":x,"fragPixelY":y},"button":"left"}});
        runtime.dispatch(serde_json::from_value(event.clone())?);
        self.bridge
            .send(self.generation, "interaction_event", None, event);
        Ok(())
    }

    fn capture(&self, request: Option<&str>) -> Result<()> {
        let runtime = self.runtime.as_ref().context("No loaded scene")?;
        let (width, height, channels) = runtime.capture_linear_rgba()?;
        self.gpu.check_errors()?;
        let bytes: Vec<u8> = channels
            .iter()
            .flat_map(|value| half::f16::from_f32(*value).to_bits().to_le_bytes())
            .collect();
        self.bridge.send_capture(self.generation,json!({"type":"player_capture","requestId":request,"width":width,"height":height,"format":"rgba16f","colorSpace":runtime.working_color_space(),"revision":self.revision,"generation":self.generation}),&bytes)
    }
}
