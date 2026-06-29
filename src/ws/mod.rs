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
    collections::HashMap,
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
    dsl::{DebugArtifactItem, Node, SceneDSL},
    protocol::{
        DesignParamPatchPayload, ErrorPayload, PassTargetSizeEntry, PassTargetSizesPayload,
        WSMessage, now_millis,
    },
    ui::resource_tree::ResourceSnapshot,
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
    AnimationControl {
        action: AnimationControlAction,
    },
    DebugArtifactUpsert {
        item: DebugArtifactItem,
        content_text: Option<String>,
    },
    DebugArtifactBinaryUpsert {
        item: DebugArtifactItem,
        bytes: Vec<u8>,
    },
    DebugArtifactDelete {
        artifact_id: String,
    },
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct DebugArtifactRequestPayload {
    #[serde(rename = "artifactId", skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(rename = "artifactIds", default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_ids: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct DebugArtifactUpsertPayload {
    pub item: DebugArtifactItem,
    #[serde(rename = "contentText", skip_serializing_if = "Option::is_none")]
    pub content_text: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct DebugArtifactDeletePayload {
    #[serde(rename = "artifactId")]
    pub artifact_id: String,
}

const DEBUG_ARTIFACT_UPLOAD_CHUNK_SIZE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct DebugArtifactUploadStartPayload {
    pub item: DebugArtifactItem,
    pub size: u64,
    #[serde(rename = "chunkSize")]
    pub chunk_size: u64,
    #[serde(rename = "totalChunks")]
    pub total_chunks: u64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct DebugArtifactUploadEndPayload {
    #[serde(rename = "artifactId")]
    pub artifact_id: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct DebugArtifactUploadChunkHeader {
    #[serde(rename = "type")]
    frame_type: String,
    #[serde(rename = "artifactId")]
    artifact_id: String,
    #[serde(rename = "chunkIndex")]
    chunk_index: u64,
    #[serde(rename = "totalChunks")]
    total_chunks: u64,
    #[serde(rename = "chunkSize")]
    chunk_size: u64,
    offset: u64,
    timestamp: u64,
}

fn debug_artifact_upload_chunk_frame(
    artifact_id: &str,
    chunk_index: usize,
    total_chunks: usize,
    chunk: &[u8],
    offset: usize,
) -> Option<Vec<u8>> {
    let header = DebugArtifactUploadChunkHeader {
        frame_type: "debug_artifact_upload_chunk".to_string(),
        artifact_id: artifact_id.to_string(),
        chunk_index: chunk_index as u64,
        total_chunks: total_chunks as u64,
        chunk_size: chunk.len() as u64,
        offset: offset as u64,
        timestamp: now_millis(),
    };
    let header_bytes = serde_json::to_vec(&header).ok()?;
    let header_len = u32::try_from(header_bytes.len()).ok()?;
    let mut frame = Vec::with_capacity(4 + header_bytes.len() + chunk.len());
    frame.extend_from_slice(&header_len.to_be_bytes());
    frame.extend_from_slice(header_bytes.as_slice());
    frame.extend_from_slice(chunk);
    Some(frame)
}

#[derive(Debug)]
struct IncomingDebugArtifactUpload {
    item: DebugArtifactItem,
    size: usize,
    total_chunks: usize,
    chunks: Vec<Option<Vec<u8>>>,
}

#[derive(Default)]
struct DebugArtifactTransferState {
    uploads: HashMap<String, IncomingDebugArtifactUpload>,
}

impl DebugArtifactTransferState {
    fn start(&mut self, payload: DebugArtifactUploadStartPayload) -> Result<()> {
        let artifact_id = payload.item.id.clone();
        let size = usize::try_from(payload.size).context("debug artifact size too large")?;
        let total_chunks = usize::try_from(payload.total_chunks)
            .context("debug artifact chunk count too large")?;
        if artifact_id.trim().is_empty() {
            anyhow::bail!("debug artifact upload missing artifact id");
        }
        if total_chunks == 0 {
            anyhow::bail!("debug artifact upload totalChunks must be > 0");
        }
        self.uploads.insert(
            artifact_id,
            IncomingDebugArtifactUpload {
                item: payload.item,
                size,
                total_chunks,
                chunks: vec![None; total_chunks],
            },
        );
        Ok(())
    }

    fn chunk(&mut self, header: DebugArtifactUploadChunkHeader, bytes: &[u8]) -> Result<()> {
        let upload = self
            .uploads
            .get_mut(header.artifact_id.as_str())
            .ok_or_else(|| anyhow::anyhow!("debug artifact chunk before upload start"))?;
        if header.frame_type != "debug_artifact_upload_chunk" {
            anyhow::bail!("invalid debug artifact chunk frame type");
        }
        let chunk_index =
            usize::try_from(header.chunk_index).context("debug artifact chunk index too large")?;
        let total_chunks =
            usize::try_from(header.total_chunks).context("debug artifact totalChunks too large")?;
        let chunk_size =
            usize::try_from(header.chunk_size).context("debug artifact chunk size too large")?;
        let offset = usize::try_from(header.offset).context("debug artifact offset too large")?;
        if total_chunks != upload.total_chunks {
            anyhow::bail!("debug artifact chunk totalChunks mismatch");
        }
        if chunk_index >= upload.total_chunks {
            anyhow::bail!("debug artifact chunk index out of range");
        }
        if chunk_size != bytes.len() {
            anyhow::bail!("debug artifact chunk size mismatch");
        }
        if offset > upload.size || offset.saturating_add(bytes.len()) > upload.size {
            anyhow::bail!("debug artifact chunk writes past expected size");
        }
        upload.chunks[chunk_index] = Some(bytes.to_vec());
        Ok(())
    }

    fn finish(&mut self, artifact_id: &str) -> Result<Option<(DebugArtifactItem, Vec<u8>)>> {
        let Some(upload) = self.uploads.remove(artifact_id) else {
            return Ok(None);
        };
        if upload.chunks.iter().any(Option::is_none) {
            anyhow::bail!("debug artifact upload missing chunks");
        }
        let mut bytes = Vec::with_capacity(upload.size);
        for chunk in upload.chunks {
            bytes.extend_from_slice(chunk.as_deref().unwrap_or_default());
        }
        if bytes.len() != upload.size {
            anyhow::bail!("debug artifact upload size mismatch");
        }
        Ok(Some((upload.item, bytes)))
    }
}

fn parse_binary_frame_header(data: &[u8]) -> Option<(String, serde_json::Value, &[u8])> {
    if data.len() < 4 {
        return None;
    }
    let header_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if header_len == 0 || data.len() < 4 + header_len {
        return None;
    }
    let header_bytes = &data[4..4 + header_len];
    let payload = &data[4 + header_len..];
    let value = serde_json::from_slice::<serde_json::Value>(header_bytes).ok()?;
    let frame_type = value
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    Some((frame_type, value, payload))
}

pub fn broadcast_debug_artifact_request(hub: &WsHub, artifact_ids: Vec<String>) {
    if artifact_ids.is_empty() {
        return;
    }
    let msg = WSMessage {
        msg_type: "debug_artifact_request".to_string(),
        timestamp: now_millis(),
        request_id: None,
        payload: Some(DebugArtifactRequestPayload {
            artifact_id: None,
            artifact_ids,
        }),
    };
    if let Ok(text) = serde_json::to_string(&msg) {
        hub.broadcast(text);
    }
}

pub fn broadcast_design_param_patch(hub: &WsHub, payload: DesignParamPatchPayload) {
    let msg = WSMessage {
        msg_type: "design_param_patch".to_string(),
        timestamp: now_millis(),
        request_id: None,
        payload: Some(payload),
    };
    if let Ok(text) = serde_json::to_string(&msg) {
        hub.broadcast(text);
    }
}

pub fn broadcast_pass_target_sizes(hub: &WsHub, snapshot: &ResourceSnapshot, scene: &SceneDSL) {
    let pass_sizes: HashMap<&str, ([u32; 2], Option<String>)> = snapshot
        .passes
        .iter()
        .filter_map(|pass| {
            let (width, height) = pass.target_size?;
            Some((pass.name.as_str(), ([width, height], pass.target_texture.clone())))
        })
        .collect();

    let passes = scene
        .nodes
        .iter()
        .filter(|node| node.node_type == "MeshGradient")
        .filter_map(|node| {
            let pass_name = format!("sys.mesh_gradient.{}.pass", node.id);
            let (target_size, target_texture) = pass_sizes.get(pass_name.as_str())?;
            Some(PassTargetSizeEntry {
                pass_name,
                node_id: node.id.clone(),
                node_type: Some(node.node_type.clone()),
                target_texture: target_texture.clone(),
                target_size: *target_size,
            })
        })
        .collect();

    let msg = WSMessage {
        msg_type: "pass_target_sizes".to_string(),
        timestamp: now_millis(),
        request_id: None,
        payload: Some(PassTargetSizesPayload { passes }),
    };
    if let Ok(text) = serde_json::to_string(&msg) {
        hub.broadcast(text);
    }
}

pub fn broadcast_debug_artifact_upsert(
    hub: &WsHub,
    item: DebugArtifactItem,
    content_text: Option<String>,
) {
    let msg = WSMessage {
        msg_type: "debug_artifact_upsert".to_string(),
        timestamp: now_millis(),
        request_id: None,
        payload: Some(DebugArtifactUpsertPayload { item, content_text }),
    };
    if let Ok(text) = serde_json::to_string(&msg) {
        hub.broadcast(text);
    }
}

pub fn broadcast_debug_artifact_binary_upload(
    hub: &WsHub,
    item: DebugArtifactItem,
    bytes: Vec<u8>,
) {
    let chunk_size = DEBUG_ARTIFACT_UPLOAD_CHUNK_SIZE_BYTES;
    let total_chunks = bytes.len().div_ceil(chunk_size).max(1);
    let start = WSMessage {
        msg_type: "debug_artifact_upload_start".to_string(),
        timestamp: now_millis(),
        request_id: None,
        payload: Some(DebugArtifactUploadStartPayload {
            item: item.clone(),
            size: bytes.len() as u64,
            chunk_size: chunk_size as u64,
            total_chunks: total_chunks as u64,
        }),
    };
    if let Ok(text) = serde_json::to_string(&start) {
        hub.broadcast(text);
    }

    for chunk_index in 0..total_chunks {
        let start = chunk_index * chunk_size;
        let end = (start + chunk_size).min(bytes.len());
        let chunk = &bytes[start..end];
        if let Some(frame) = debug_artifact_upload_chunk_frame(
            item.id.as_str(),
            chunk_index,
            total_chunks,
            chunk,
            start,
        ) {
            hub.broadcast_binary(frame);
        }
    }

    let end = WSMessage {
        msg_type: "debug_artifact_upload_end".to_string(),
        timestamp: now_millis(),
        request_id: None,
        payload: Some(DebugArtifactUploadEndPayload {
            artifact_id: item.id,
        }),
    };
    if let Ok(text) = serde_json::to_string(&end) {
        hub.broadcast(text);
    }
}

pub fn broadcast_debug_artifact_delete(hub: &WsHub, artifact_id: String) {
    let msg = WSMessage {
        msg_type: "debug_artifact_delete".to_string(),
        timestamp: now_millis(),
        request_id: None,
        payload: Some(DebugArtifactDeletePayload { artifact_id }),
    };
    if let Ok(text) = serde_json::to_string(&msg) {
        hub.broadcast(text);
    }
}

// ---------------------------------------------------------------------------
// WebSocket server core
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct WsHub {
    clients: Arc<Mutex<Vec<Sender<Message>>>>,
}

impl WsHub {
    pub fn client_count(&self) -> usize {
        self.clients
            .lock()
            .map(|clients| clients.len())
            .unwrap_or_default()
    }

    pub fn broadcast(&self, text: String) {
        self.broadcast_message(Message::Text(text));
    }

    pub fn broadcast_binary(&self, bytes: Vec<u8>) {
        self.broadcast_message(Message::Binary(bytes));
    }

    fn broadcast_message(&self, message: Message) {
        let Ok(mut clients) = self.clients.lock() else {
            return;
        };
        clients.retain(|tx| tx.send(message.clone()).is_ok());
    }

    fn register_client(&self, tx: Sender<Message>) {
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

    let (client_tx, client_rx) = crossbeam_channel::unbounded::<Message>();
    hub.register_client(client_tx);
    let mut transfer_state = AssetTransferState::default();
    let mut debug_artifact_transfer_state = DebugArtifactTransferState::default();

    loop {
        // 1) flush outbound (validation errors etc)
        while let Ok(message) = client_rx.try_recv() {
            let _ = ws.send(message);
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
                    &mut debug_artifact_transfer_state,
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
                if let Some((frame_type, header_value, chunk_payload)) =
                    parse_binary_frame_header(&data)
                    && frame_type == "debug_artifact_upload_chunk"
                {
                    match serde_json::from_value::<DebugArtifactUploadChunkHeader>(header_value) {
                        Ok(header) => {
                            if let Err(error) =
                                debug_artifact_transfer_state.chunk(header, chunk_payload)
                            {
                                send_error(
                                    &mut ws,
                                    None,
                                    "DEBUG_ARTIFACT_UPLOAD_CHUNK_INVALID",
                                    &format!("{error:#}"),
                                );
                            }
                        }
                        Err(error) => {
                            send_error(
                                &mut ws,
                                None,
                                "PARSE_ERROR",
                                &format!("invalid debug artifact chunk header: {error}"),
                            );
                        }
                    }
                } else {
                    handle_binary_asset_upload(&mut ws, &data, &mut transfer_state, &asset_store);
                }
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
    debug_artifact_transfer_state: &mut DebugArtifactTransferState,
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
        "debug_artifact_request" => {
            // Renderer-side debug artifacts are surfaced through UI actions that
            // broadcast upserts as they happen. The WS thread intentionally does
            // not own artifact content state, so editor requests are accepted as
            // a forward-compatible no-op in v1.
        }
        "debug_artifact_upsert" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "debug_artifact_upsert missing payload",
                    );
                    return Ok(());
                }
            };
            let payload: DebugArtifactUpsertPayload = match serde_json::from_value(payload) {
                Ok(p) => p,
                Err(e) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("invalid debug_artifact_upsert payload: {e}"),
                    );
                    return Ok(());
                }
            };
            send_scene_update(
                scene_tx,
                scene_drop_rx,
                SceneUpdate::DebugArtifactUpsert {
                    item: payload.item,
                    content_text: payload.content_text,
                },
                ui_wake,
            );
        }
        "debug_artifact_delete" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "debug_artifact_delete missing payload",
                    );
                    return Ok(());
                }
            };
            let payload: DebugArtifactDeletePayload = match serde_json::from_value(payload) {
                Ok(p) => p,
                Err(e) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("invalid debug_artifact_delete payload: {e}"),
                    );
                    return Ok(());
                }
            };
            send_scene_update(
                scene_tx,
                scene_drop_rx,
                SceneUpdate::DebugArtifactDelete {
                    artifact_id: payload.artifact_id,
                },
                ui_wake,
            );
        }
        "debug_artifact_upload_start" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "debug_artifact_upload_start missing payload",
                    );
                    return Ok(());
                }
            };
            let payload: DebugArtifactUploadStartPayload = match serde_json::from_value(payload) {
                Ok(p) => p,
                Err(error) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("invalid debug_artifact_upload_start payload: {error}"),
                    );
                    return Ok(());
                }
            };
            if let Err(error) = debug_artifact_transfer_state.start(payload) {
                send_error(
                    ws,
                    msg.request_id,
                    "DEBUG_ARTIFACT_UPLOAD_START_INVALID",
                    &format!("{error:#}"),
                );
            }
        }
        "debug_artifact_upload_end" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "debug_artifact_upload_end missing payload",
                    );
                    return Ok(());
                }
            };
            let payload: DebugArtifactUploadEndPayload = match serde_json::from_value(payload) {
                Ok(p) => p,
                Err(error) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("invalid debug_artifact_upload_end payload: {error}"),
                    );
                    return Ok(());
                }
            };
            match debug_artifact_transfer_state.finish(payload.artifact_id.as_str()) {
                Ok(Some((item, bytes))) => {
                    send_scene_update(
                        scene_tx,
                        scene_drop_rx,
                        SceneUpdate::DebugArtifactBinaryUpsert { item, bytes },
                        ui_wake,
                    );
                }
                Ok(None) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "DEBUG_ARTIFACT_UPLOAD_NOT_STARTED",
                        "debug artifact upload ended before start",
                    );
                }
                Err(error) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "DEBUG_ARTIFACT_UPLOAD_INCOMPLETE",
                        &format!("{error:#}"),
                    );
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
            SceneUpdate::DebugArtifactUpsert { .. }
            | SceneUpdate::DebugArtifactBinaryUpsert { .. }
            | SceneUpdate::DebugArtifactDelete { .. } => {
                // Artifact messages are side-channel state for the debug UI.
                // Keep an in-flight scene update if one exists; the editor can
                // answer a later request if this update is skipped.
                false
            }
        }
    } else {
        true
    };

    if queued && let Some(wake) = ui_wake {
        wake();
    }
}
