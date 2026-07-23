mod asset_transfer;
mod debug_artifacts;
mod dispatch;
mod hub;
mod scene_delta;
mod shader_templates;

use asset_transfer::{
    AssetTransferState, AssetUploadEndPayload, AssetUploadStartPayload, UploadFinalizeResult,
    handle_binary_asset_upload, request_missing_assets, send_asset_upload_ack,
    send_asset_upload_nack,
};
use debug_artifacts::{
    DebugArtifactTransferState, DebugArtifactUploadChunkHeader, parse_binary_frame_header,
};
pub use debug_artifacts::{
    DebugArtifactUploadEndPayload, DebugArtifactUploadStartPayload,
    broadcast_debug_artifact_binary_upload, broadcast_debug_artifact_delete,
    broadcast_debug_artifact_request, broadcast_debug_artifact_upsert,
};
use dispatch::{handle_text_message, send_error};
pub use hub::WsHub;
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
    time::{Duration, Instant},
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
        perf_trace: Option<ScenePerfTrace>,
    },
    UniformDelta {
        updated_nodes: Vec<Node>,
        request_id: Option<String>,
        perf_trace: Option<ScenePerfTrace>,
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
#[serde(rename_all = "camelCase")]
struct RuntimeSceneUpdatePayload {
    scene: SceneDSL,
    functions: Vec<crate::state_machine::mutation_function::FunctionResource>,
}

#[derive(Debug, Clone)]
pub struct ScenePerfTrace {
    pub trace_id: String,
    pub client_sent_at_ms: u64,
    pub server_received_at_ms: u64,
    pub message_bytes: usize,
    pub ws_parse_ms: f64,
    pub enqueued_at: Instant,
}

impl SceneUpdate {
    pub fn perf_trace(&self) -> Option<&ScenePerfTrace> {
        match self {
            Self::Parsed { perf_trace, .. } | Self::UniformDelta { perf_trace, .. } => {
                perf_trace.as_ref()
            }
            _ => None,
        }
    }

    pub fn perf_update_kind(&self) -> &'static str {
        match self {
            Self::Parsed {
                source: ParsedSceneSource::SceneUpdate,
                ..
            } => "scene_update",
            Self::Parsed {
                source: ParsedSceneSource::SceneDelta,
                ..
            } => "scene_delta",
            Self::UniformDelta { .. } => "uniform_delta",
            _ => "other",
        }
    }
}

fn create_scene_perf_trace(
    request_id: &Option<String>,
    client_sent_at_ms: u64,
    server_received_at_ms: u64,
    message_bytes: usize,
    receive_started_at: Instant,
) -> Option<ScenePerfTrace> {
    let trace_id = request_id
        .as_deref()
        .filter(|value| value.starts_with("nforge-perf-"))?
        .to_string();
    Some(ScenePerfTrace {
        trace_id,
        client_sent_at_ms,
        server_received_at_ms,
        message_bytes,
        ws_parse_ms: receive_started_at.elapsed().as_secs_f64() * 1000.0,
        enqueued_at: Instant::now(),
    })
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
            Some((
                pass.name.as_str(),
                ([width, height], pass.target_texture.clone()),
            ))
        })
        .collect();

    let passes = scene
        .nodes
        .iter()
        .filter_map(|node| {
            let pass_name = match node.node_type.as_str() {
                "MeshGradient" => format!("sys.mesh_gradient.{}.pass", node.id),
                "IntelligentLight" => format!("sys.ilight.{}.upsample.pass", node.id),
                _ => return None,
            };
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

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ScenePerfTracePayload {
    trace_id: String,
    message_bytes: usize,
    transport_to_server_ms: f64,
    ws_parse_ms: f64,
    renderer_queue_ms: f64,
    renderer_apply_ms: f64,
    end_to_end_ms: f64,
    did_rebuild_shader_space: bool,
    update_kind: String,
}

pub fn broadcast_scene_perf_trace(
    hub: &WsHub,
    trace: ScenePerfTrace,
    renderer_queue_ms: f64,
    renderer_apply_ms: f64,
    did_rebuild_shader_space: bool,
    update_kind: &str,
) {
    let end_to_end_ms = now_millis().saturating_sub(trace.client_sent_at_ms) as f64;
    let message = WSMessage {
        msg_type: "perf_trace".to_string(),
        timestamp: now_millis(),
        request_id: Some(trace.trace_id.clone()),
        payload: Some(ScenePerfTracePayload {
            trace_id: trace.trace_id,
            message_bytes: trace.message_bytes,
            transport_to_server_ms: trace
                .server_received_at_ms
                .saturating_sub(trace.client_sent_at_ms) as f64,
            ws_parse_ms: trace.ws_parse_ms,
            renderer_queue_ms,
            renderer_apply_ms,
            end_to_end_ms,
            did_rebuild_shader_space,
            update_kind: update_kind.to_string(),
        }),
    };
    if let Ok(text) = serde_json::to_string(&message) {
        hub.broadcast(text);
    }
}

// ---------------------------------------------------------------------------
// WebSocket server core
// ---------------------------------------------------------------------------

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
