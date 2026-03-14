mod asset_transfer;
mod scene_delta;

use asset_transfer::{
    AssetTransferState, AssetUploadEndPayload, AssetUploadStartPayload, UploadFinalizeResult,
    handle_binary_asset_upload, request_missing_assets, send_asset_upload_ack,
    send_asset_upload_nack,
};
use scene_delta::delta_updates_only_uniform_values;
pub use scene_delta::{
    SceneCache, SceneCacheConnectionsById, SceneCacheNodesById, SceneDelta, SceneDeltaConnections,
    SceneDeltaNodes, SceneOutputs, apply_scene_delta, apply_scene_update,
    has_dangling_connection_references, materialize_scene_dsl, prune_invalid_connections,
};

use std::{
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use serde_json::Value;
use tungstenite::{Error as WsError, Message, accept};

use crate::{
    asset_store::AssetStore,
    dsl,
    dsl::{Node, SceneDSL},
    protocol::{ErrorPayload, WSMessage, now_millis},
};

#[derive(Debug, Clone, serde::Serialize)]
struct SceneResyncRequestPayload {
    reason: String,
}

fn send_scene_resync_request(ws: &mut tungstenite::WebSocket<std::net::TcpStream>, reason: &str) {
    let req = WSMessage {
        msg_type: "scene_resync_request".to_string(),
        timestamp: now_millis(),
        request_id: None,
        payload: Some(SceneResyncRequestPayload {
            reason: reason.to_string(),
        }),
    };

    if let Ok(text) = serde_json::to_string(&req) {
        let _ = ws.send(Message::Text(text));
    }
}

fn spawn_server_ping_loop(hub: WsHub) {
    thread::spawn(move || {
        loop {
            let ping = WSMessage::<Value> {
                msg_type: "ping".to_string(),
                timestamp: now_millis(),
                request_id: None,
                payload: None,
            };

            if let Ok(text) = serde_json::to_string(&ping) {
                hub.broadcast(text);
            }

            thread::sleep(Duration::from_millis(200));
        }
    });
}

#[derive(Debug, Clone)]
pub enum ParsedSceneSource {
    SceneUpdate,
    SceneDelta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnimationControlAction {
    Play,
    Stop,
}

#[derive(Debug, Clone)]
pub enum SceneUpdate {
    Parsed {
        scene: SceneDSL,
        request_id: Option<String>,
        source: ParsedSceneSource,
    },
    UniformDelta {
        updated_nodes: Vec<Node>,
        request_id: Option<String>,
    },
    ParseError {
        message: String,
        request_id: Option<String>,
    },
    /// Animation play/stop control from the editor.
    AnimationControl { action: AnimationControlAction },
}

// ---------------------------------------------------------------------------
// WebSocket server core
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct WsHub {
    clients: Arc<Mutex<Vec<Sender<String>>>>,
}

impl WsHub {
    pub fn broadcast(&self, text: String) {
        let Ok(mut clients) = self.clients.lock() else {
            return;
        };
        clients.retain(|tx| tx.send(text.clone()).is_ok());
    }

    fn register_client(&self, tx: Sender<String>) {
        if let Ok(mut clients) = self.clients.lock() {
            clients.push(tx);
        }
    }
}

pub type UiWakeCallback = Arc<dyn Fn() + Send + Sync + 'static>;

pub fn spawn_ws_server(
    addr: &str,
    scene_tx: Sender<SceneUpdate>,
    scene_drop_rx: Receiver<SceneUpdate>,
    hub: WsHub,
    last_good: Arc<Mutex<Option<SceneDSL>>>,
    asset_store: AssetStore,
    ui_wake: Option<UiWakeCallback>,
) -> Result<thread::JoinHandle<()>> {
    let scene_cache = Arc::new(Mutex::new(None::<SceneCache>));
    let addr_str = addr.to_string();
    let server =
        TcpListener::bind(addr).with_context(|| format!("failed to bind ws server at {addr}"))?;

    // Editor-side heartbeat: server periodically emits {type:"ping"}.
    // (Client may reply with {type:"pong"}, which we accept as a no-op.)
    spawn_server_ping_loop(hub.clone());

    Ok(thread::spawn(move || {
        if let Err(e) = run_ws_server(
            server,
            &addr_str,
            scene_tx,
            scene_drop_rx,
            hub.clone(),
            last_good,
            scene_cache,
            asset_store,
            ui_wake,
        ) {
            report_internal_error(&hub, None, "WS_SERVER_FAILED", &format!("{e:#}"));
        }
    }))
}

fn report_internal_error(hub: &WsHub, request_id: Option<String>, code: &str, message: &str) {
    let err = WSMessage {
        msg_type: "error".to_string(),
        timestamp: now_millis(),
        request_id,
        payload: Some(ErrorPayload {
            code: code.to_string(),
            message: message.to_string(),
        }),
    };

    if let Ok(text) = serde_json::to_string(&err) {
        hub.broadcast(text);
    }
}

fn run_ws_server(
    server: TcpListener,
    addr: &str,
    scene_tx: Sender<SceneUpdate>,
    scene_drop_rx: Receiver<SceneUpdate>,
    hub: WsHub,
    last_good: Arc<Mutex<Option<SceneDSL>>>,
    scene_cache: Arc<Mutex<Option<SceneCache>>>,
    asset_store: AssetStore,
    ui_wake: Option<UiWakeCallback>,
) -> Result<()> {
    // Treat server lifecycle logs as editor-facing diagnostics.
    let startup = WSMessage::<Value> {
        msg_type: "debug".to_string(),
        timestamp: now_millis(),
        request_id: None,
        payload: Some(serde_json::json!({
            "message": format!("[ws] listening on ws://{addr}"),
        })),
    };
    if let Ok(text) = serde_json::to_string(&startup) {
        hub.broadcast(text);
    }

    for stream in server.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                report_internal_error(&hub, None, "WS_ACCEPT_FAILED", &format!("{e:#}"));
                continue;
            }
        };

        let scene_tx = scene_tx.clone();
        let scene_drop_rx = scene_drop_rx.clone();
        let hub = hub.clone();
        let last_good = last_good.clone();
        let scene_cache = scene_cache.clone();
        let asset_store = asset_store.clone();
        let ui_wake = ui_wake.clone();

        thread::spawn(move || {
            if let Err(e) = handle_client(
                stream,
                scene_tx,
                scene_drop_rx,
                hub.clone(),
                last_good,
                scene_cache,
                asset_store,
                ui_wake,
            ) {
                report_internal_error(&hub, None, "WS_CLIENT_ENDED", &format!("{e:#}"));
            }
        });
    }

    Ok(())
}

fn handle_client(
    stream: std::net::TcpStream,
    scene_tx: Sender<SceneUpdate>,
    scene_drop_rx: Receiver<SceneUpdate>,
    hub: WsHub,
    last_good: Arc<Mutex<Option<SceneDSL>>>,
    scene_cache: Arc<Mutex<Option<SceneCache>>>,
    asset_store: AssetStore,
    ui_wake: Option<UiWakeCallback>,
) -> Result<()> {
    // Handshake is easier with a blocking socket, switch to non-blocking afterwards.
    let mut ws = accept(stream).context("websocket handshake failed")?;
    ws.get_mut()
        .set_nonblocking(true)
        .context("failed to set tcp non-blocking")?;

    let (client_tx, client_rx) = crossbeam_channel::unbounded::<String>();
    hub.register_client(client_tx);
    let mut transfer_state = AssetTransferState::default();

    loop {
        // 1) flush outbound (validation errors etc)
        while let Ok(text) = client_rx.try_recv() {
            let _ = ws.send(Message::Text(text));
        }

        // 2) read inbound
        match ws.read() {
            Ok(Message::Text(text)) => {
                if let Err(e) = handle_text_message(
                    &mut ws,
                    &text,
                    &scene_tx,
                    &scene_drop_rx,
                    &last_good,
                    &scene_cache,
                    &asset_store,
                    &mut transfer_state,
                    ui_wake.as_ref(),
                ) {
                    report_internal_error(
                        &hub,
                        None,
                        "WS_HANDLE_MESSAGE_FAILED",
                        &format!("{e:#}"),
                    );
                }
            }
            Ok(Message::Binary(data)) => {
                handle_binary_asset_upload(&mut ws, &data, &mut transfer_state, &asset_store);
            }
            Ok(Message::Ping(payload)) => {
                let _ = ws.send(Message::Pong(payload));
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Frame(_)) => {}
            Ok(Message::Close(_)) => break,
            Err(WsError::Io(ref io)) if io.kind() == std::io::ErrorKind::WouldBlock => {
                // nothing to read
            }
            Err(WsError::AlreadyClosed) | Err(WsError::ConnectionClosed) => break,
            Err(e) => return Err(e).context("websocket read failed"),
        }

        thread::sleep(Duration::from_millis(5));
    }

    Ok(())
}

fn handle_text_message(
    ws: &mut tungstenite::WebSocket<std::net::TcpStream>,
    text: &str,
    scene_tx: &Sender<SceneUpdate>,
    scene_drop_rx: &Receiver<SceneUpdate>,
    last_good: &Arc<Mutex<Option<SceneDSL>>>,
    scene_cache: &Arc<Mutex<Option<SceneCache>>>,
    asset_store: &AssetStore,
    transfer_state: &mut AssetTransferState,
    ui_wake: Option<&UiWakeCallback>,
) -> Result<()> {
    let msg: WSMessage<Value> = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            let message = format!("invalid json: {e}");
            send_error(ws, None, "PARSE_ERROR", &message);
            send_scene_update(
                scene_tx,
                scene_drop_rx,
                SceneUpdate::ParseError {
                    message,
                    request_id: None,
                },
                ui_wake,
            );
            return Ok(());
        }
    };

    match msg.msg_type.as_str() {
        "ping" => {
            let pong = WSMessage::<Value> {
                msg_type: "pong".to_string(),
                timestamp: now_millis(),
                request_id: msg.request_id,
                payload: None,
            };
            let _ = ws.send(Message::Text(serde_json::to_string(&pong)?));
        }
        "pong" => {
            // No-op: clients may auto-reply to server-initiated pings.
        }
        "heartbeat" => {
            // Backwards-compatibility / no-op.
        }
        "scene_request" => {
            let scene = last_good.lock().ok().and_then(|g| g.clone());
            if let Some(scene) = scene {
                let resp = WSMessage {
                    msg_type: "scene_update".to_string(),
                    timestamp: now_millis(),
                    request_id: msg.request_id,
                    payload: Some(serde_json::to_value(scene)?),
                };
                let _ = ws.send(Message::Text(serde_json::to_string(&resp)?));
            } else {
                send_error(ws, msg.request_id, "VALIDATION_ERROR", "no last-good scene");
            }
        }
        "scene_update" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    let message = "missing payload".to_string();
                    send_error(ws, msg.request_id.clone(), "PARSE_ERROR", &message);
                    send_scene_update(
                        scene_tx,
                        scene_drop_rx,
                        SceneUpdate::ParseError {
                            message,
                            request_id: msg.request_id,
                        },
                        ui_wake,
                    );
                    return Ok(());
                }
            };

            // A full scene_update is authoritative; clear incremental caches.
            if let Ok(mut guard) = scene_cache.lock() {
                *guard = None;
            }

            let mut scene: SceneDSL = match serde_json::from_value(payload.clone()) {
                Ok(s) => s,
                Err(e) => {
                    let message = format!("invalid SceneDSL: {e}");
                    send_error(ws, msg.request_id.clone(), "PARSE_ERROR", &message);
                    send_scene_update(
                        scene_tx,
                        scene_drop_rx,
                        SceneUpdate::ParseError {
                            message,
                            request_id: msg.request_id,
                        },
                        ui_wake,
                    );
                    return Ok(());
                }
            };

            dsl::materialize_scene_node_labels_from_raw_json(&mut scene, &payload);

            // Keep client payload compact: fill in missing params from the bundled scheme.
            if let Err(e) = dsl::normalize_scene_defaults(&mut scene) {
                let message = format!("failed to apply default params: {e:#}");
                send_error(ws, msg.request_id.clone(), "PARSE_ERROR", &message);
                send_scene_update(
                    scene_tx,
                    scene_drop_rx,
                    SceneUpdate::ParseError {
                        message,
                        request_id: msg.request_id,
                    },
                    ui_wake,
                );
                return Ok(());
            }

            // Request any assets referenced by the scene that are missing from the store.
            let referenced_ids: Vec<String> = scene.assets.keys().cloned().collect();
            request_missing_assets(ws, transfer_state, asset_store, &referenced_ids);

            if let Ok(mut guard) = scene_cache.lock() {
                let mut cache = guard
                    .take()
                    .unwrap_or_else(|| SceneCache::from_scene_update(&scene));
                apply_scene_update(&mut cache, &scene);
                *guard = Some(cache);
            }

            // Keep only latest: bounded(1) + drop stale message if receiver hasn't caught up.
            send_scene_update(
                scene_tx,
                scene_drop_rx,
                SceneUpdate::Parsed {
                    scene,
                    request_id: msg.request_id,
                    source: ParsedSceneSource::SceneUpdate,
                },
                ui_wake,
            );
        }
        "scene_delta" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    let message = "missing payload".to_string();
                    send_error(ws, msg.request_id.clone(), "PARSE_ERROR", &message);
                    return Ok(());
                }
            };

            let mut delta: SceneDelta = match serde_json::from_value(payload.clone()) {
                Ok(d) => d,
                Err(e) => {
                    let message = format!("invalid SceneDelta: {e}");
                    send_error(ws, msg.request_id.clone(), "PARSE_ERROR", &message);
                    send_scene_resync_request(ws, "delta_schema_validation_failed");
                    return Ok(());
                }
            };

            if let Some(raw_added) = payload
                .get("nodes")
                .and_then(|v| v.get("added"))
                .and_then(|v| v.as_array())
            {
                dsl::materialize_node_labels_from_raw_nodes(&mut delta.nodes.added, raw_added);
            }
            if let Some(raw_updated) = payload
                .get("nodes")
                .and_then(|v| v.get("updated"))
                .and_then(|v| v.as_array())
            {
                dsl::materialize_node_labels_from_raw_nodes(&mut delta.nodes.updated, raw_updated);
            }

            let mut scene: Option<SceneDSL> = None;
            if let Ok(mut guard) = scene_cache.lock() {
                let Some(mut cache) = guard.take() else {
                    send_error(
                        ws,
                        msg.request_id.clone(),
                        "RESYNC_REQUIRED",
                        "received scene_delta before scene_update",
                    );
                    send_scene_resync_request(ws, "missing_baseline_scene_update");
                    *guard = None;
                    return Ok(());
                };

                if delta.version != cache.version {
                    send_error(
                        ws,
                        msg.request_id.clone(),
                        "RESYNC_REQUIRED",
                        "scene_delta version mismatch; request full scene_update",
                    );
                    send_scene_resync_request(ws, "delta_version_mismatch");
                    *guard = Some(cache);
                    return Ok(());
                }

                let is_uniform_only_delta = delta_updates_only_uniform_values(&cache, &delta);
                apply_scene_delta(&mut cache, &delta);

                // Request any asset binaries the store hasn't received yet.
                let referenced_ids: Vec<String> = cache.assets.keys().cloned().collect();
                request_missing_assets(ws, transfer_state, asset_store, &referenced_ids);

                // Detect dangling references before pruning (signals a cache mismatch).
                if has_dangling_connection_references(&cache) {
                    send_error(
                        ws,
                        msg.request_id.clone(),
                        "RESYNC_REQUIRED",
                        "dangling references detected; request full scene_update",
                    );
                    send_scene_resync_request(ws, "dangling_references_detected");
                    *guard = None;
                    return Ok(());
                }

                prune_invalid_connections(&mut cache);

                if is_uniform_only_delta {
                    *guard = Some(cache);
                    send_scene_update(
                        scene_tx,
                        scene_drop_rx,
                        SceneUpdate::UniformDelta {
                            updated_nodes: delta.nodes.updated.clone(),
                            request_id: msg.request_id,
                        },
                        ui_wake,
                    );
                    return Ok(());
                }

                let mut materialized = materialize_scene_dsl(&cache);
                if let Err(e) = dsl::normalize_scene_defaults(&mut materialized) {
                    let message = format!("failed to apply default params: {e:#}");
                    send_error(ws, msg.request_id.clone(), "PARSE_ERROR", &message);
                    send_scene_resync_request(ws, "delta_apply_failed");
                    *guard = None;
                    return Ok(());
                }

                scene = Some(materialized);
                *guard = Some(cache);
            }

            if let Some(scene) = scene {
                send_scene_update(
                    scene_tx,
                    scene_drop_rx,
                    SceneUpdate::Parsed {
                        scene,
                        request_id: msg.request_id,
                        source: ParsedSceneSource::SceneDelta,
                    },
                    ui_wake,
                );
            }
        }
        "asset_remove" => {
            if let Some(payload) = msg.payload {
                if let Some(asset_id) = payload.get("assetId").and_then(|v| v.as_str()) {
                    asset_store.remove(asset_id);
                    transfer_state.on_asset_removed(asset_id);
                    // Also remove from scene cache assets if present.
                    if let Ok(mut guard) = scene_cache.lock() {
                        if let Some(cache) = guard.as_mut() {
                            cache.assets.remove(asset_id);
                        }
                    }
                }
            }
        }
        "asset_upload_start" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "asset_upload_start missing payload",
                    );
                    return Ok(());
                }
            };
            let payload: AssetUploadStartPayload = match serde_json::from_value(payload) {
                Ok(p) => p,
                Err(e) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("invalid asset_upload_start payload: {e}"),
                    );
                    return Ok(());
                }
            };

            if let Err(e) = transfer_state.on_upload_start(payload, now_millis()) {
                send_error(
                    ws,
                    msg.request_id,
                    "ASSET_UPLOAD_START_INVALID",
                    &format!("{e:#}"),
                );
            }
        }
        "asset_upload_end" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "asset_upload_end missing payload",
                    );
                    return Ok(());
                }
            };
            let payload: AssetUploadEndPayload = match serde_json::from_value(payload) {
                Ok(p) => p,
                Err(e) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("invalid asset_upload_end payload: {e}"),
                    );
                    return Ok(());
                }
            };

            let now = now_millis();
            match transfer_state.on_upload_end(&payload.asset_id, now) {
                UploadFinalizeResult::Completed(asset_data) => {
                    let asset_id = payload.asset_id;
                    let byte_len = asset_data.bytes.len();
                    asset_store.insert_or_replace(asset_id.clone(), asset_data);
                    send_asset_upload_ack(ws, &asset_id);
                    eprintln!(
                        r#"{{"event":"asset_transfer_completed","assetId":"{}","bytes":{}}}"#,
                        asset_id, byte_len
                    );
                    trigger_rerender_for_asset(
                        &asset_id,
                        scene_cache,
                        scene_tx,
                        scene_drop_rx,
                        ui_wake,
                    );
                }
                UploadFinalizeResult::MissingChunks(missing_chunks) => {
                    eprintln!(
                        r#"{{"event":"asset_transfer_nack_sent","assetId":"{}","missingChunks":{:?}}}"#,
                        payload.asset_id, missing_chunks
                    );
                    send_asset_upload_nack(
                        ws,
                        &payload.asset_id,
                        &missing_chunks,
                        "missing_chunks",
                    );
                }
                UploadFinalizeResult::NotStarted => {
                    eprintln!(
                        r#"{{"event":"asset_transfer_failed","assetId":"{}","reason":"transfer_not_started"}}"#,
                        payload.asset_id
                    );
                    send_asset_upload_nack(ws, &payload.asset_id, &[], "transfer_not_started");
                }
            }
        }
        "asset_request" => {
            // Client requests an asset by id; reply with binary frame if available.
            if let Some(payload) = msg.payload {
                if let Some(asset_id) = payload.get("assetId").and_then(|v| v.as_str()) {
                    if let Some(data) = asset_store.get(asset_id) {
                        // Binary frame format: [id_len: u32 LE][asset_id bytes][payload bytes]
                        let id_bytes = asset_id.as_bytes();
                        let mut frame = Vec::with_capacity(4 + id_bytes.len() + data.bytes.len());
                        frame.extend_from_slice(&(id_bytes.len() as u32).to_le_bytes());
                        frame.extend_from_slice(id_bytes);
                        frame.extend_from_slice(&data.bytes);
                        let _ = ws.send(Message::Binary(frame));
                    } else {
                        send_error(
                            ws,
                            msg.request_id,
                            "ASSET_NOT_FOUND",
                            &format!("asset '{asset_id}' not found"),
                        );
                    }
                }
            }
        }
        "animation_control" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "animation_control missing payload",
                    );
                    return Ok(());
                }
            };

            let action_str = match payload.get("action").and_then(|v| v.as_str()) {
                Some(a) => a,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "animation_control payload missing 'action' field",
                    );
                    return Ok(());
                }
            };

            let action = match action_str {
                "play" => AnimationControlAction::Play,
                "stop" => AnimationControlAction::Stop,
                other => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("unknown animation_control action: {other}"),
                    );
                    return Ok(());
                }
            };

            send_scene_update(
                scene_tx,
                scene_drop_rx,
                SceneUpdate::AnimationControl { action },
                ui_wake,
            );
        }
        other => {
            send_error(
                ws,
                msg.request_id,
                "PARSE_ERROR",
                &format!("unknown message type: {other}"),
            );
        }
    }

    Ok(())
}

fn trigger_rerender_for_asset(
    asset_id: &str,
    scene_cache: &Arc<Mutex<Option<SceneCache>>>,
    scene_tx: &Sender<SceneUpdate>,
    scene_drop_rx: &Receiver<SceneUpdate>,
    ui_wake: Option<&UiWakeCallback>,
) {
    let should_rerender = scene_cache
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref().map(|cache| {
                cache.assets.contains_key(asset_id)
                    || cache.nodes_by_id.values().any(|node| {
                        node.params
                            .get("assetId")
                            .and_then(|v| v.as_str())
                            .is_some_and(|id| id == asset_id)
                    })
            })
        })
        .unwrap_or(false);

    if should_rerender
        && let Some(scene) = scene_cache
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(materialize_scene_dsl))
    {
        send_scene_update(
            scene_tx,
            scene_drop_rx,
            SceneUpdate::Parsed {
                scene,
                request_id: None,
                source: ParsedSceneSource::SceneUpdate,
            },
            ui_wake,
        );
    }
}

fn send_error(
    ws: &mut tungstenite::WebSocket<std::net::TcpStream>,
    request_id: Option<String>,
    code: &str,
    message: &str,
) {
    let err = WSMessage {
        msg_type: "error".to_string(),
        timestamp: now_millis(),
        request_id,
        payload: Some(ErrorPayload {
            code: code.to_string(),
            message: message.to_string(),
        }),
    };

    if let Ok(text) = serde_json::to_string(&err) {
        let _ = ws.send(Message::Text(text));
    }
}

fn send_scene_update(
    scene_tx: &Sender<SceneUpdate>,
    scene_drop_rx: &Receiver<SceneUpdate>,
    update: SceneUpdate,
    ui_wake: Option<&UiWakeCallback>,
) {
    // Debounce policy: keep the latest *scene* update.
    // But never drop ParseError updates, otherwise we can mask the reason we
    // requested a resync and make debugging much harder.
    let queued = if scene_tx.try_send(update.clone()).is_err() {
        match update {
            SceneUpdate::Parsed { .. } => {
                while scene_drop_rx.try_recv().is_ok() {}
                scene_tx.try_send(update).is_ok()
            }
            SceneUpdate::UniformDelta { .. } => {
                // Uniform-only updates are cheap and high-frequency.
                // If the channel is full, prefer keeping the in-flight message
                // (often a full Parsed scene) and drop this delta.
                false
            }
            SceneUpdate::ParseError { .. } => {
                // Channel is full; keep the existing message rather than
                // replacing it. A future update will replace it naturally.
                false
            }
            SceneUpdate::AnimationControl { .. } => {
                // Control messages are critical; flush the channel to deliver.
                while scene_drop_rx.try_recv().is_ok() {}
                scene_tx.try_send(update).is_ok()
            }
        }
    } else {
        true
    };

    if queued && let Some(wake) = ui_wake {
        wake();
    }
}
