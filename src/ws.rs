use std::{
    collections::{HashSet, VecDeque},
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
    asset_store::{AssetData, AssetStore},
    dsl,
    dsl::{Connection, GroupDSL, Metadata, Node, SceneDSL},
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
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SceneDelta {
    pub version: String,
    pub nodes: SceneDeltaNodes,
    pub connections: SceneDeltaConnections,
    #[serde(default)]
    pub outputs: Option<std::collections::HashMap<String, String>>,
    // Groups are currently only sent in full `scene_update` messages.
    // Keep this optional for forward-compatibility if editors start sending deltas.
    #[serde(default)]
    pub groups: Option<Vec<GroupDSL>>,
    /// Asset metadata added/updated by this delta.  Optional because older
    /// editors may not include it; in that case we synthesize entries from
    /// the binary AssetStore.
    #[serde(default)]
    pub assets: Option<std::collections::HashMap<String, crate::dsl::AssetEntry>>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SceneDeltaNodes {
    pub added: Vec<Node>,
    pub updated: Vec<Node>,
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SceneDeltaConnections {
    pub added: Vec<Connection>,
    pub updated: Vec<Connection>,
    pub removed: Vec<String>,
}

pub type SceneOutputs = std::collections::HashMap<String, String>;

pub type SceneCacheNodesById = std::collections::HashMap<String, Node>;
pub type SceneCacheConnectionsById = std::collections::HashMap<String, Connection>;

#[derive(Debug, Clone)]
pub struct SceneCache {
    pub version: String,
    pub metadata: Metadata,
    pub nodes_by_id: SceneCacheNodesById,
    pub connections_by_id: SceneCacheConnectionsById,
    pub outputs: SceneOutputs,
    pub groups: Vec<GroupDSL>,
    pub assets: std::collections::HashMap<String, crate::dsl::AssetEntry>,
}

impl SceneCache {
    pub fn from_scene_update(scene: &SceneDSL) -> Self {
        let mut cache = Self {
            version: scene.version.clone(),
            metadata: scene.metadata.clone(),
            nodes_by_id: std::collections::HashMap::new(),
            connections_by_id: std::collections::HashMap::new(),
            outputs: scene.outputs.clone().unwrap_or_default(),
            groups: scene.groups.clone(),
            assets: scene.assets.clone(),
        };
        apply_scene_update(&mut cache, scene);
        cache
    }
}

pub fn apply_scene_update(cache: &mut SceneCache, scene: &SceneDSL) {
    cache.version = scene.version.clone();
    cache.metadata = scene.metadata.clone();
    cache.groups = scene.groups.clone();
    cache.assets = scene.assets.clone();

    cache.nodes_by_id.clear();
    for node in &scene.nodes {
        cache.nodes_by_id.insert(node.id.clone(), node.clone());
    }

    cache.connections_by_id.clear();
    for conn in &scene.connections {
        cache
            .connections_by_id
            .insert(conn.id.clone(), conn.clone());
    }

    cache.outputs = scene.outputs.clone().unwrap_or_default();
}

pub fn apply_scene_delta(cache: &mut SceneCache, delta: &SceneDelta) {
    for connection_id in &delta.connections.removed {
        cache.connections_by_id.remove(connection_id);
    }

    for node_id in &delta.nodes.removed {
        cache.nodes_by_id.remove(node_id);
    }

    for node in &delta.nodes.added {
        cache.nodes_by_id.insert(node.id.clone(), node.clone());
    }
    for node in &delta.nodes.updated {
        cache.nodes_by_id.insert(node.id.clone(), node.clone());
    }

    for conn in &delta.connections.added {
        cache
            .connections_by_id
            .insert(conn.id.clone(), conn.clone());
    }
    for conn in &delta.connections.updated {
        cache
            .connections_by_id
            .insert(conn.id.clone(), conn.clone());
    }

    if let Some(outputs) = delta.outputs.as_ref() {
        cache.outputs = outputs.clone();
    }

    if let Some(groups) = delta.groups.as_ref() {
        cache.groups = groups.clone();
    }

    // Merge asset metadata carried by the delta.
    if let Some(assets) = delta.assets.as_ref() {
        for (id, entry) in assets {
            cache.assets.insert(id.clone(), entry.clone());
        }
    }
}

/// Scan nodes for `assetId` param references that are missing from the
/// scene-level `assets` map.  When the binary data is already available in
/// the `AssetStore` we synthesize an `AssetEntry` from it so that downstream
/// geometry resolution can find the metadata without requiring the editor to
/// explicitly include `assets` in every `scene_delta`.
fn ensure_asset_metadata_for_nodes(
    nodes_by_id: &SceneCacheNodesById,
    assets: &mut std::collections::HashMap<String, crate::dsl::AssetEntry>,
    asset_store: &AssetStore,
) {
    for node in nodes_by_id.values() {
        if let Some(aid) = node
            .params
            .get("assetId")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            if assets.contains_key(aid) {
                continue;
            }
            // Try to construct an AssetEntry from the binary store.
            if let Some(data) = asset_store.get(aid) {
                eprintln!(
                    "[asset-metadata] synthesized AssetEntry for '{}' from asset store (original_name='{}')",
                    aid, data.original_name
                );
                assets.insert(
                    aid.to_string(),
                    crate::dsl::AssetEntry {
                        path: data.original_name.clone(),
                        original_name: data.original_name,
                        mime_type: data.mime_type,
                        size: Some(data.bytes.len() as u64),
                    },
                );
            }
        }
    }
}

fn is_value_driven_input_node_type(node_type: &str) -> bool {
    matches!(
        node_type,
        "BoolInput" | "FloatInput" | "IntInput" | "Vector2Input" | "Vector3Input" | "ColorInput"
    )
}

fn is_uniform_param_key(key: &str) -> bool {
    matches!(key, "value" | "x" | "y" | "z" | "w" | "v")
}

fn node_params_changed_only_uniform_keys(
    prev: &std::collections::HashMap<String, Value>,
    next: &std::collections::HashMap<String, Value>,
) -> bool {
    let mut saw_change = false;
    for (key, after) in next {
        let before = prev.get(key);
        if before != Some(after) {
            saw_change = true;
            if !is_uniform_param_key(key) {
                return false;
            }
        }
    }

    saw_change
}

fn delta_updates_only_uniform_values(cache: &SceneCache, delta: &SceneDelta) -> bool {
    if delta.nodes.updated.is_empty() {
        return false;
    }

    if !delta.nodes.added.is_empty()
        || !delta.nodes.removed.is_empty()
        || !delta.connections.added.is_empty()
        || !delta.connections.updated.is_empty()
        || !delta.connections.removed.is_empty()
        || delta.outputs.is_some()
        || delta.groups.is_some()
    {
        return false;
    }

    for updated in &delta.nodes.updated {
        let Some(prev) = cache.nodes_by_id.get(&updated.id) else {
            return false;
        };
        if prev.node_type != updated.node_type {
            return false;
        }
        if !is_value_driven_input_node_type(updated.node_type.as_str()) {
            return false;
        }
        if !node_params_changed_only_uniform_keys(&prev.params, &updated.params) {
            return false;
        }
        if uniform_delta_change_affects_geometry_allocation(cache, &updated.id) {
            return false;
        }
    }

    true
}

fn is_geometry_allocation_sink(node_type: &str, port_id: &str) -> bool {
    matches!(
        (node_type, port_id),
        ("Rect2DGeometry", "size")
            | ("Downsample", "targetSize")
            | ("RenderTexture", "width")
            | ("RenderTexture", "height")
    )
}

fn uniform_delta_change_affects_geometry_allocation(
    cache: &SceneCache,
    updated_node_id: &str,
) -> bool {
    let mut queue: VecDeque<String> = VecDeque::from([updated_node_id.to_string()]);
    let mut visited: HashSet<String> = HashSet::new();

    while let Some(from_id) = queue.pop_front() {
        if !visited.insert(from_id.clone()) {
            continue;
        }

        for conn in cache.connections_by_id.values() {
            if conn.from.node_id != from_id {
                continue;
            }

            if let Some(dst_node) = cache.nodes_by_id.get(&conn.to.node_id) {
                if is_geometry_allocation_sink(
                    dst_node.node_type.as_str(),
                    conn.to.port_id.as_str(),
                ) {
                    return true;
                }
            }
            queue.push_back(conn.to.node_id.clone());
        }
    }

    false
}

pub fn prune_invalid_connections(cache: &mut SceneCache) {
    cache.connections_by_id.retain(|_, conn| {
        cache.nodes_by_id.contains_key(&conn.from.node_id)
            && cache.nodes_by_id.contains_key(&conn.to.node_id)
    });
}

pub fn has_dangling_connection_references(cache: &SceneCache) -> bool {
    cache.connections_by_id.values().any(|conn| {
        !cache.nodes_by_id.contains_key(&conn.from.node_id)
            || !cache.nodes_by_id.contains_key(&conn.to.node_id)
    })
}

pub fn materialize_scene_dsl(cache: &SceneCache) -> SceneDSL {
    SceneDSL {
        version: cache.version.clone(),
        metadata: cache.metadata.clone(),
        nodes: cache.nodes_by_id.values().cloned().collect(),
        connections: cache.connections_by_id.values().cloned().collect(),
        outputs: Some(cache.outputs.clone()),
        groups: cache.groups.clone(),
        assets: cache.assets.clone(),
    }
}

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

pub fn spawn_ws_server(
    addr: &str,
    scene_tx: Sender<SceneUpdate>,
    scene_drop_rx: Receiver<SceneUpdate>,
    hub: WsHub,
    last_good: Arc<Mutex<Option<SceneDSL>>>,
    asset_store: AssetStore,
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

        thread::spawn(move || {
            if let Err(e) = handle_client(
                stream,
                scene_tx,
                scene_drop_rx,
                hub.clone(),
                last_good,
                scene_cache,
                asset_store,
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
) -> Result<()> {
    // Handshake is easier with a blocking socket, switch to non-blocking afterwards.
    let mut ws = accept(stream).context("websocket handshake failed")?;
    ws.get_mut()
        .set_nonblocking(true)
        .context("failed to set tcp non-blocking")?;

    let (client_tx, client_rx) = crossbeam_channel::unbounded::<String>();
    hub.register_client(client_tx);

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
                handle_binary_asset_upload(
                    &data,
                    &asset_store,
                    &scene_cache,
                    &scene_tx,
                    &scene_drop_rx,
                );
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
                    );
                    return Ok(());
                }
            };

            // A full scene_update is authoritative; clear incremental caches.
            if let Ok(mut guard) = scene_cache.lock() {
                *guard = None;
            }

            let mut scene: SceneDSL = match serde_json::from_value(payload) {
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
                    );
                    return Ok(());
                }
            };

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
                );
                return Ok(());
            }

            // Request any assets referenced by the scene that are missing from the store.
            let referenced_ids: Vec<String> = scene.assets.keys().cloned().collect();
            let missing = asset_store.missing_ids(&referenced_ids);
            if !missing.is_empty() {
                eprintln!(
                    "[asset-request] requesting {} missing asset(s): {:?}",
                    missing.len(),
                    missing
                );
                send_asset_request(ws, &missing);
            }

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

            let delta: SceneDelta = match serde_json::from_value(payload) {
                Ok(d) => d,
                Err(e) => {
                    let message = format!("invalid SceneDelta: {e}");
                    send_error(ws, msg.request_id.clone(), "PARSE_ERROR", &message);
                    send_scene_resync_request(ws, "delta_schema_validation_failed");
                    return Ok(());
                }
            };

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

                // Ensure asset metadata exists for every node that references an
                // assetId.  When the editor doesn't include the `assets` map in
                // the delta we synthesize an AssetEntry from the binary AssetStore
                // (which was populated earlier via sendAllAssets / asset_upload).
                ensure_asset_metadata_for_nodes(&cache.nodes_by_id, &mut cache.assets, asset_store);

                // Request any asset binaries the store hasn't received yet.
                {
                    let referenced_ids: Vec<String> = cache.assets.keys().cloned().collect();
                    let missing = asset_store.missing_ids(&referenced_ids);
                    if !missing.is_empty() {
                        eprintln!(
                            "[asset-request] (scene_delta) requesting {} missing asset(s): {:?}",
                            missing.len(),
                            missing
                        );
                        send_asset_request(ws, &missing);
                    }
                }

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
                );
            }
        }
        "asset_remove" => {
            if let Some(payload) = msg.payload {
                if let Some(asset_id) = payload.get("assetId").and_then(|v| v.as_str()) {
                    asset_store.remove(asset_id);
                    // Also remove from scene cache assets if present.
                    if let Ok(mut guard) = scene_cache.lock() {
                        if let Some(cache) = guard.as_mut() {
                            cache.assets.remove(asset_id);
                        }
                    }
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

/// Try parsing the new binary frame format:
/// `[header_len: u32 BE][JSON header (UTF-8)][raw asset data]`
///
/// Falls back to the legacy format:
/// `[id_len: u32 LE][asset_id bytes][payload bytes]`
///
/// After storing the asset, if the current scene references it, re-send the
/// scene for rendering so the pipeline picks up the newly-available asset.
fn handle_binary_asset_upload(
    data: &[u8],
    asset_store: &AssetStore,
    scene_cache: &Arc<Mutex<Option<SceneCache>>>,
    scene_tx: &Sender<SceneUpdate>,
    scene_drop_rx: &Receiver<SceneUpdate>,
) {
    if data.len() < 4 {
        return;
    }

    let mut uploaded_asset_id: Option<String> = None;

    // Try new format first: big-endian header length + JSON header + raw data.
    let header_len_be = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if header_len_be > 0
        && header_len_be < data.len().saturating_sub(4)
        && data.len() >= 4 + header_len_be
    {
        let header_bytes = &data[4..4 + header_len_be];
        if let Ok(header_str) = std::str::from_utf8(header_bytes) {
            if let Ok(header) = serde_json::from_str::<Value>(header_str) {
                if header.get("type").and_then(|v| v.as_str()) == Some("asset_upload") {
                    if let Some(asset_id) = header.get("assetId").and_then(|v| v.as_str()) {
                        let mime = header
                            .get("mimeType")
                            .and_then(|v| v.as_str())
                            .unwrap_or("application/octet-stream")
                            .to_string();
                        let original_name = header
                            .get("originalName")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let payload = data[4 + header_len_be..].to_vec();
                        eprintln!(
                            "[asset-upload] received '{}' ({} bytes, {})",
                            asset_id,
                            payload.len(),
                            mime
                        );
                        let aid = asset_id.to_string();
                        asset_store.insert(
                            aid.clone(),
                            AssetData {
                                bytes: payload,
                                mime_type: mime,
                                original_name,
                            },
                        );
                        uploaded_asset_id = Some(aid);
                    }
                }
            }
        }
    }

    // Legacy format: [id_len: u32 LE][asset_id bytes][payload bytes]
    if uploaded_asset_id.is_none() {
        let id_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if data.len() < 4 + id_len {
            return;
        }
        let asset_id = match std::str::from_utf8(&data[4..4 + id_len]) {
            Ok(s) => s.to_string(),
            Err(_) => return,
        };
        let payload = data[4 + id_len..].to_vec();

        let mime = if payload.starts_with(b"\x89PNG") {
            "image/png"
        } else if payload.starts_with(b"\xff\xd8\xff") {
            "image/jpeg"
        } else {
            "application/octet-stream"
        };

        eprintln!(
            "[asset-upload] received '{}' ({} bytes, {}) [legacy format]",
            asset_id,
            payload.len(),
            mime
        );
        asset_store.insert(
            asset_id.clone(),
            AssetData {
                bytes: payload,
                mime_type: mime.to_string(),
                original_name: String::new(),
            },
        );
        uploaded_asset_id = Some(asset_id);
    }

    // If the current scene references this asset, re-send it for rendering.
    if let Some(aid) = uploaded_asset_id {
        let mut should_rerender = false;

        if let Ok(mut guard) = scene_cache.lock() {
            if let Some(cache) = guard.as_mut() {
                if cache.assets.contains_key(&aid) {
                    // Metadata already present; just need to re-render.
                    should_rerender = true;
                } else {
                    // Check if any node references this asset via its params.
                    // If so, synthesize the AssetEntry from the store and flag
                    // re-render so the pipeline picks up the newly-available
                    // geometry/texture.
                    let node_refs_asset = cache.nodes_by_id.values().any(|n| {
                        n.params
                            .get("assetId")
                            .and_then(|v| v.as_str())
                            .is_some_and(|id| id == aid)
                    });
                    if node_refs_asset {
                        if let Some(data) = asset_store.get(&aid) {
                            eprintln!(
                                "[asset-upload] synthesized AssetEntry for '{}' referenced by node (original_name='{}')",
                                aid, data.original_name
                            );
                            cache.assets.insert(
                                aid.clone(),
                                crate::dsl::AssetEntry {
                                    path: data.original_name.clone(),
                                    original_name: data.original_name,
                                    mime_type: data.mime_type,
                                    size: Some(data.bytes.len() as u64),
                                },
                            );
                        }
                        should_rerender = true;
                    }
                }
            }
        }

        if should_rerender {
            if let Some(scene) = scene_cache
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(materialize_scene_dsl))
            {
                eprintln!(
                    "[asset-upload] asset '{}' referenced by current scene; triggering re-render",
                    aid
                );
                send_scene_update(
                    scene_tx,
                    scene_drop_rx,
                    SceneUpdate::Parsed {
                        scene,
                        request_id: None,
                        source: ParsedSceneSource::SceneUpdate,
                    },
                );
            }
        }
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

/// Send an `asset_request` message to the editor for a list of missing asset IDs.
fn send_asset_request(ws: &mut tungstenite::WebSocket<std::net::TcpStream>, asset_ids: &[String]) {
    #[derive(serde::Serialize)]
    struct AssetRequestPayload {
        #[serde(rename = "assetIds")]
        asset_ids: Vec<String>,
    }

    let req = WSMessage {
        msg_type: "asset_request".to_string(),
        timestamp: now_millis(),
        request_id: None,
        payload: Some(AssetRequestPayload {
            asset_ids: asset_ids.to_vec(),
        }),
    };

    if let Ok(text) = serde_json::to_string(&req) {
        let _ = ws.send(Message::Text(text));
    }
}

fn send_scene_update(
    scene_tx: &Sender<SceneUpdate>,
    scene_drop_rx: &Receiver<SceneUpdate>,
    update: SceneUpdate,
) {
    // Debounce policy: keep the latest *scene* update.
    // But never drop ParseError updates, otherwise we can mask the reason we
    // requested a resync and make debugging much harder.
    if scene_tx.try_send(update.clone()).is_err() {
        match update {
            SceneUpdate::Parsed { .. } => {
                while scene_drop_rx.try_recv().is_ok() {}
                let _ = scene_tx.try_send(update);
            }
            SceneUpdate::UniformDelta { .. } => {
                // Uniform-only updates are cheap and high-frequency.
                // If the channel is full, prefer keeping the in-flight message
                // (often a full Parsed scene) and drop this delta.
            }
            SceneUpdate::ParseError { .. } => {
                // Channel is full; keep the existing message rather than
                // replacing it. A future update will replace it naturally.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{Endpoint, Metadata, SceneDSL};
    use serde_json::json;

    fn node(id: &str, node_type: &str, params: serde_json::Value) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params: serde_json::from_value(params).unwrap_or_default(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_bindings: Vec::new(),
        }
    }

    fn base_scene() -> SceneDSL {
        SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "scene".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node(
                    "FloatInput_1",
                    "FloatInput",
                    json!({"value": 0.5, "min": 0.0}),
                ),
                node("MathAdd_1", "MathAdd", json!({})),
            ],
            connections: vec![Connection {
                id: "c1".to_string(),
                from: Endpoint {
                    node_id: "FloatInput_1".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "MathAdd_1".to_string(),
                    port_id: "a".to_string(),
                },
            }],
            outputs: Some(std::collections::HashMap::new()),
            groups: Vec::new(),
            assets: Default::default(),
        }
    }

    #[test]
    fn delta_updates_only_uniform_values_accepts_float_value_change() {
        let scene = base_scene();
        let cache = SceneCache::from_scene_update(&scene);
        let delta = SceneDelta {
            version: "1.0".to_string(),
            nodes: SceneDeltaNodes {
                added: Vec::new(),
                updated: vec![node(
                    "FloatInput_1",
                    "FloatInput",
                    json!({"value": 0.75, "min": 0.0}),
                )],
                removed: Vec::new(),
            },
            connections: SceneDeltaConnections {
                added: Vec::new(),
                updated: Vec::new(),
                removed: Vec::new(),
            },
            outputs: None,
            groups: None,
            assets: None,
        };
        assert!(delta_updates_only_uniform_values(&cache, &delta));
    }

    #[test]
    fn delta_updates_only_uniform_values_accepts_partial_param_patch() {
        let scene = base_scene();
        let cache = SceneCache::from_scene_update(&scene);
        let delta = SceneDelta {
            version: "1.0".to_string(),
            nodes: SceneDeltaNodes {
                added: Vec::new(),
                updated: vec![node("FloatInput_1", "FloatInput", json!({"value": 0.9}))],
                removed: Vec::new(),
            },
            connections: SceneDeltaConnections {
                added: Vec::new(),
                updated: Vec::new(),
                removed: Vec::new(),
            },
            outputs: None,
            groups: None,
            assets: None,
        };
        assert!(delta_updates_only_uniform_values(&cache, &delta));
    }

    #[test]
    fn delta_updates_only_uniform_values_rejects_structural_connection_changes() {
        let scene = base_scene();
        let cache = SceneCache::from_scene_update(&scene);
        let delta = SceneDelta {
            version: "1.0".to_string(),
            nodes: SceneDeltaNodes {
                added: Vec::new(),
                updated: vec![node(
                    "FloatInput_1",
                    "FloatInput",
                    json!({"value": 0.75, "min": 0.0}),
                )],
                removed: Vec::new(),
            },
            connections: SceneDeltaConnections {
                added: vec![Connection {
                    id: "c2".to_string(),
                    from: Endpoint {
                        node_id: "FloatInput_1".to_string(),
                        port_id: "value".to_string(),
                    },
                    to: Endpoint {
                        node_id: "MathAdd_1".to_string(),
                        port_id: "b".to_string(),
                    },
                }],
                updated: Vec::new(),
                removed: Vec::new(),
            },
            outputs: None,
            groups: None,
            assets: None,
        };
        assert!(!delta_updates_only_uniform_values(&cache, &delta));
    }

    #[test]
    fn delta_updates_only_uniform_values_rejects_non_uniform_param_change() {
        let scene = base_scene();
        let cache = SceneCache::from_scene_update(&scene);
        let delta = SceneDelta {
            version: "1.0".to_string(),
            nodes: SceneDeltaNodes {
                added: Vec::new(),
                updated: vec![node(
                    "FloatInput_1",
                    "FloatInput",
                    json!({"value": 0.75, "min": -1.0}),
                )],
                removed: Vec::new(),
            },
            connections: SceneDeltaConnections {
                added: Vec::new(),
                updated: Vec::new(),
                removed: Vec::new(),
            },
            outputs: None,
            groups: None,
            assets: None,
        };
        assert!(!delta_updates_only_uniform_values(&cache, &delta));
    }

    #[test]
    fn delta_updates_only_uniform_values_rejects_geometry_allocation_sensitive_change() {
        let scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "scene".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("v2", "Vector2Input", json!({"x": 108.0, "y": 240.0})),
                node("rect", "Rect2DGeometry", json!({})),
                node("pass", "RenderPass", json!({})),
                node("comp", "Composite", json!({})),
                node("rt", "RenderTexture", json!({"width": 400, "height": 400})),
            ],
            connections: vec![
                Connection {
                    id: "c1".to_string(),
                    from: Endpoint {
                        node_id: "v2".to_string(),
                        port_id: "vector".to_string(),
                    },
                    to: Endpoint {
                        node_id: "rect".to_string(),
                        port_id: "size".to_string(),
                    },
                },
                Connection {
                    id: "c2".to_string(),
                    from: Endpoint {
                        node_id: "rect".to_string(),
                        port_id: "geometry".to_string(),
                    },
                    to: Endpoint {
                        node_id: "pass".to_string(),
                        port_id: "geometry".to_string(),
                    },
                },
                Connection {
                    id: "c3".to_string(),
                    from: Endpoint {
                        node_id: "pass".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "comp".to_string(),
                        port_id: "pass".to_string(),
                    },
                },
                Connection {
                    id: "c4".to_string(),
                    from: Endpoint {
                        node_id: "rt".to_string(),
                        port_id: "texture".to_string(),
                    },
                    to: Endpoint {
                        node_id: "comp".to_string(),
                        port_id: "target".to_string(),
                    },
                },
            ],
            outputs: Some(std::collections::HashMap::new()),
            groups: Vec::new(),
            assets: Default::default(),
        };
        let cache = SceneCache::from_scene_update(&scene);
        let delta = SceneDelta {
            version: "1.0".to_string(),
            nodes: SceneDeltaNodes {
                added: Vec::new(),
                updated: vec![node("v2", "Vector2Input", json!({"x": 54.0, "y": 120.0}))],
                removed: Vec::new(),
            },
            connections: SceneDeltaConnections {
                added: Vec::new(),
                updated: Vec::new(),
                removed: Vec::new(),
            },
            outputs: None,
            groups: None,
            assets: None,
        };
        assert!(!delta_updates_only_uniform_values(&cache, &delta));
    }
}
