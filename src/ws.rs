use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
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
    /// Asset metadata added/updated by this delta (upsert semantics).
    #[serde(rename = "assetsAdded", default)]
    pub assets_added: Option<std::collections::HashMap<String, crate::dsl::AssetEntry>>,
    /// Asset ids removed by this delta.
    #[serde(rename = "assetsRemoved", default)]
    pub assets_removed: Option<Vec<String>>,
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
        let mut merged = node.clone();
        let has_label = merged
            .params
            .get("label")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty());
        if !has_label {
            if let Some(prev) = cache.nodes_by_id.get(&merged.id) {
                if let Some(prev_label) = prev
                    .params
                    .get("label")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                {
                    merged
                        .params
                        .insert("label".to_string(), Value::String(prev_label.to_string()));
                }
            }
        }
        cache.nodes_by_id.insert(merged.id.clone(), merged);
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
    if let Some(assets) = delta.assets_added.as_ref() {
        for (id, entry) in assets {
            cache.assets.insert(id.clone(), entry.clone());
        }
    }

    if let Some(asset_ids) = delta.assets_removed.as_ref() {
        for asset_id in asset_ids {
            cache.assets.remove(asset_id);
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
        || delta.assets_added.is_some()
        || delta.assets_removed.is_some()
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
            | ("RenderPass", "camera")
            | ("GuassianBlurPass", "camera")
            | ("GradientBlur", "camera")
            | ("Downsample", "targetSize")
            | ("Downsample", "camera")
            | ("Upsample", "camera")
            | ("Upsample", "targetSize")
            | ("Composite", "camera")
            | ("SetTransform", "matrix")
            | ("TransformGeometry", "matrix")
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

const ASSET_REQUEST_STALE_MS: u64 = 5_000;
const ASSET_RECEIVE_STALE_MS: u64 = 15_000;
const ASSET_REQUEST_BACKOFF_BASE_MS: u64 = 1_000;
const ASSET_REQUEST_BACKOFF_MAX_MS: u64 = 30_000;
const MAX_ASSET_SIZE_BYTES: usize = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssetTransferStatus {
    Missing,
    Requested,
    Receiving,
    Ready,
    Failed,
}

#[derive(Debug, Clone)]
struct AssetTransferEntry {
    status: AssetTransferStatus,
    last_event_ms: u64,
    failure_count: u32,
    next_retry_ms: u64,
}

impl Default for AssetTransferEntry {
    fn default() -> Self {
        Self {
            status: AssetTransferStatus::Missing,
            last_event_ms: 0,
            failure_count: 0,
            next_retry_ms: 0,
        }
    }
}

impl AssetTransferEntry {
    fn mark_requested(&mut self, now_ms: u64) {
        self.status = AssetTransferStatus::Requested;
        self.last_event_ms = now_ms;
    }

    fn mark_receiving(&mut self, now_ms: u64) {
        self.status = AssetTransferStatus::Receiving;
        self.last_event_ms = now_ms;
    }

    fn mark_ready(&mut self, now_ms: u64) {
        self.status = AssetTransferStatus::Ready;
        self.last_event_ms = now_ms;
        self.failure_count = 0;
        self.next_retry_ms = 0;
    }

    fn mark_failed(&mut self, now_ms: u64) {
        self.status = AssetTransferStatus::Failed;
        self.last_event_ms = now_ms;
        self.failure_count = self.failure_count.saturating_add(1);
        let shift = self.failure_count.saturating_sub(1).min(5);
        let backoff_ms = (ASSET_REQUEST_BACKOFF_BASE_MS.saturating_mul(1_u64 << shift))
            .min(ASSET_REQUEST_BACKOFF_MAX_MS);
        self.next_retry_ms = now_ms.saturating_add(backoff_ms);
    }

    fn mark_missing(&mut self, now_ms: u64) {
        self.status = AssetTransferStatus::Missing;
        self.last_event_ms = now_ms;
    }

    fn should_timeout(&self, now_ms: u64) -> bool {
        match self.status {
            AssetTransferStatus::Requested => {
                now_ms.saturating_sub(self.last_event_ms) > ASSET_REQUEST_STALE_MS
            }
            AssetTransferStatus::Receiving => {
                now_ms.saturating_sub(self.last_event_ms) > ASSET_RECEIVE_STALE_MS
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AssetUploadStartPayload {
    #[serde(rename = "assetId")]
    asset_id: String,
    #[serde(rename = "mimeType")]
    mime_type: String,
    #[serde(rename = "originalName", default)]
    original_name: String,
    size: u64,
    #[serde(rename = "chunkSize")]
    chunk_size: u64,
    #[serde(rename = "totalChunks")]
    total_chunks: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AssetUploadEndPayload {
    #[serde(rename = "assetId")]
    asset_id: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AssetUploadChunkHeader {
    #[serde(rename = "type")]
    frame_type: String,
    #[serde(rename = "assetId")]
    asset_id: String,
    #[serde(rename = "chunkIndex")]
    chunk_index: u64,
    #[serde(rename = "totalChunks")]
    total_chunks: u64,
    #[serde(rename = "chunkSize")]
    chunk_size: u64,
    offset: u64,
    #[serde(default)]
    timestamp: Option<u64>,
}

#[derive(Debug, Clone)]
struct IncomingAssetUpload {
    asset_id: String,
    mime_type: String,
    original_name: String,
    expected_size: usize,
    chunk_size: usize,
    total_chunks: usize,
    bytes: Vec<u8>,
    received_chunks: Vec<bool>,
    received_chunk_count: usize,
}

impl IncomingAssetUpload {
    fn new(payload: AssetUploadStartPayload) -> Result<Self> {
        let asset_id = payload.asset_id.trim().to_string();
        if asset_id.is_empty() {
            bail!("asset_upload_start missing assetId");
        }

        let expected_size = usize::try_from(payload.size)
            .context("asset_upload_start size does not fit platform usize")?;
        if expected_size == 0 {
            bail!("asset_upload_start size must be > 0");
        }
        if expected_size > MAX_ASSET_SIZE_BYTES {
            bail!("asset_upload_start size exceeds configured maximum");
        }

        let chunk_size = usize::try_from(payload.chunk_size)
            .context("asset_upload_start chunkSize does not fit platform usize")?;
        if chunk_size == 0 {
            bail!("asset_upload_start chunkSize must be > 0");
        }

        let total_chunks = usize::try_from(payload.total_chunks)
            .context("asset_upload_start totalChunks does not fit platform usize")?;
        if total_chunks == 0 {
            bail!("asset_upload_start totalChunks must be > 0");
        }

        let expected_chunks = expected_size.div_ceil(chunk_size);
        if expected_chunks != total_chunks {
            bail!(
                "asset_upload_start totalChunks mismatch: expected {}, got {}",
                expected_chunks,
                total_chunks
            );
        }

        Ok(Self {
            asset_id,
            mime_type: payload.mime_type,
            original_name: payload.original_name,
            expected_size,
            chunk_size,
            total_chunks,
            bytes: vec![0; expected_size],
            received_chunks: vec![false; total_chunks],
            received_chunk_count: 0,
        })
    }

    fn apply_chunk(&mut self, header: &AssetUploadChunkHeader, chunk_bytes: &[u8]) -> Result<()> {
        if header.asset_id != self.asset_id {
            bail!("asset_upload_chunk assetId mismatch");
        }
        let chunk_index = usize::try_from(header.chunk_index)
            .context("asset_upload_chunk chunkIndex does not fit platform usize")?;
        let total_chunks = usize::try_from(header.total_chunks)
            .context("asset_upload_chunk totalChunks does not fit platform usize")?;
        let chunk_size = usize::try_from(header.chunk_size)
            .context("asset_upload_chunk chunkSize does not fit platform usize")?;
        let offset = usize::try_from(header.offset)
            .context("asset_upload_chunk offset does not fit usize")?;

        if total_chunks != self.total_chunks {
            bail!(
                "asset_upload_chunk totalChunks mismatch: expected {}, got {}",
                self.total_chunks,
                total_chunks
            );
        }
        if chunk_index >= self.total_chunks {
            bail!(
                "asset_upload_chunk chunkIndex out of range: {} >= {}",
                chunk_index,
                self.total_chunks
            );
        }

        let expected_offset = chunk_index.saturating_mul(self.chunk_size);
        if offset != expected_offset {
            bail!(
                "asset_upload_chunk offset mismatch: expected {}, got {}",
                expected_offset,
                offset
            );
        }

        let expected_len = if chunk_index + 1 == self.total_chunks {
            self.expected_size.saturating_sub(offset)
        } else {
            self.chunk_size
        };
        if chunk_size != chunk_bytes.len() {
            bail!(
                "asset_upload_chunk chunkSize mismatch: header {}, payload {}",
                chunk_size,
                chunk_bytes.len()
            );
        }
        if expected_len != chunk_bytes.len() {
            bail!(
                "asset_upload_chunk length mismatch at chunk {}: expected {}, got {}",
                chunk_index,
                expected_len,
                chunk_bytes.len()
            );
        }
        if offset.saturating_add(chunk_bytes.len()) > self.expected_size {
            bail!("asset_upload_chunk writes past expected size");
        }

        self.bytes[offset..offset + chunk_bytes.len()].copy_from_slice(chunk_bytes);
        if !self.received_chunks[chunk_index] {
            self.received_chunks[chunk_index] = true;
            self.received_chunk_count += 1;
        }
        Ok(())
    }

    fn is_complete(&self) -> bool {
        self.received_chunk_count == self.total_chunks
    }

    fn missing_chunks(&self) -> Vec<u32> {
        self.received_chunks
            .iter()
            .enumerate()
            .filter_map(|(idx, received)| (!*received).then_some(idx as u32))
            .collect()
    }

    fn into_asset_data(self) -> AssetData {
        AssetData {
            bytes: self.bytes,
            mime_type: self.mime_type,
            original_name: self.original_name,
        }
    }
}

#[derive(Debug)]
enum UploadFinalizeResult {
    Completed(AssetData),
    MissingChunks(Vec<u32>),
    NotStarted,
}

#[derive(Debug, Default)]
struct AssetTransferState {
    entries: HashMap<String, AssetTransferEntry>,
    uploads: HashMap<String, IncomingAssetUpload>,
}

impl AssetTransferState {
    fn sync_with_manifest(&mut self, asset_ids: &[String], asset_store: &AssetStore, now_ms: u64) {
        let referenced: HashSet<&str> = asset_ids.iter().map(String::as_str).collect();
        self.entries
            .retain(|id, _| referenced.contains(id.as_str()));
        self.uploads
            .retain(|id, _| referenced.contains(id.as_str()));

        for asset_id in asset_ids {
            let entry = self.entries.entry(asset_id.clone()).or_default();
            if asset_store.contains(asset_id) {
                entry.mark_ready(now_ms);
            } else if matches!(entry.status, AssetTransferStatus::Ready) {
                entry.mark_missing(now_ms);
            }
        }
    }

    fn collect_requestable_missing(
        &mut self,
        asset_ids: &[String],
        asset_store: &AssetStore,
        now_ms: u64,
    ) -> Vec<String> {
        self.sync_with_manifest(asset_ids, asset_store, now_ms);

        let mut request_ids = Vec::new();
        let mut seen = HashSet::new();

        for asset_id in asset_ids {
            if !seen.insert(asset_id.as_str()) {
                continue;
            }

            if asset_store.contains(asset_id) {
                let entry = self.entries.entry(asset_id.clone()).or_default();
                entry.mark_ready(now_ms);
                continue;
            }

            let timed_out = self
                .entries
                .entry(asset_id.clone())
                .or_default()
                .should_timeout(now_ms);
            if timed_out {
                let dropped_upload = self.uploads.remove(asset_id).is_some();
                let entry = self.entries.entry(asset_id.clone()).or_default();
                entry.mark_failed(now_ms);
                eprintln!(
                    r#"{{"event":"asset_transfer_failed","assetId":"{}","reason":"stale_state","status":"{:?}","droppedUpload":{}}}"#,
                    asset_id, entry.status, dropped_upload
                );
            }

            let has_active_upload = self.uploads.contains_key(asset_id);
            let entry = self.entries.entry(asset_id.clone()).or_default();
            if has_active_upload {
                if !matches!(entry.status, AssetTransferStatus::Receiving) {
                    entry.mark_receiving(now_ms);
                }
                eprintln!(
                    r#"{{"event":"asset_request_dedup_skipped","assetId":"{}","reason":"receiving"}}"#,
                    asset_id
                );
                continue;
            }

            let should_request = match entry.status {
                AssetTransferStatus::Missing => true,
                AssetTransferStatus::Failed => now_ms >= entry.next_retry_ms,
                AssetTransferStatus::Requested => false,
                AssetTransferStatus::Receiving => false,
                AssetTransferStatus::Ready => {
                    entry.mark_missing(now_ms);
                    true
                }
            };

            if should_request {
                entry.mark_requested(now_ms);
                request_ids.push(asset_id.clone());
            } else {
                eprintln!(
                    r#"{{"event":"asset_request_dedup_skipped","assetId":"{}","status":"{:?}"}}"#,
                    asset_id, entry.status
                );
            }
        }

        request_ids
    }

    fn on_upload_start(&mut self, payload: AssetUploadStartPayload, now_ms: u64) -> Result<()> {
        let session = IncomingAssetUpload::new(payload)?;
        let asset_id = session.asset_id.clone();
        self.uploads.insert(asset_id.clone(), session);
        self.entries
            .entry(asset_id.clone())
            .or_default()
            .mark_receiving(now_ms);
        eprintln!(
            r#"{{"event":"asset_transfer_started","assetId":"{}"}}"#,
            asset_id
        );
        Ok(())
    }

    fn on_upload_chunk(
        &mut self,
        header: AssetUploadChunkHeader,
        chunk_bytes: &[u8],
        now_ms: u64,
    ) -> Result<()> {
        if header.frame_type != "asset_upload_chunk" {
            bail!("unsupported binary frame type: {}", header.frame_type);
        }

        let asset_id = header.asset_id.clone();
        let session = self
            .uploads
            .get_mut(&asset_id)
            .ok_or_else(|| anyhow!("asset_upload_chunk received before asset_upload_start"))?;
        let chunk_index = header.chunk_index;
        let _ = header.timestamp;
        session.apply_chunk(&header, chunk_bytes)?;

        self.entries
            .entry(asset_id.clone())
            .or_default()
            .mark_receiving(now_ms);
        eprintln!(
            r#"{{"event":"asset_chunk_received","assetId":"{}","chunkIndex":{}}}"#,
            asset_id, chunk_index
        );
        Ok(())
    }

    fn on_upload_end(&mut self, asset_id: &str, now_ms: u64) -> UploadFinalizeResult {
        let Some(upload) = self.uploads.get(asset_id) else {
            self.entries
                .entry(asset_id.to_string())
                .or_default()
                .mark_failed(now_ms);
            return UploadFinalizeResult::NotStarted;
        };

        if upload.is_complete() {
            let upload = self
                .uploads
                .remove(asset_id)
                .expect("upload exists before completion");
            self.entries
                .entry(asset_id.to_string())
                .or_default()
                .mark_ready(now_ms);
            UploadFinalizeResult::Completed(upload.into_asset_data())
        } else {
            let missing_chunks = upload.missing_chunks();
            self.entries
                .entry(asset_id.to_string())
                .or_default()
                .mark_failed(now_ms);
            UploadFinalizeResult::MissingChunks(missing_chunks)
        }
    }

    fn on_asset_removed(&mut self, asset_id: &str) {
        self.entries.remove(asset_id);
        self.uploads.remove(asset_id);
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

/// Parse chunk frames:
/// `[header_len: u32 BE][JSON header (UTF-8)][raw chunk bytes]`
/// where the JSON header `type` is `asset_upload_chunk`.
fn handle_binary_asset_upload(
    ws: &mut tungstenite::WebSocket<std::net::TcpStream>,
    data: &[u8],
    transfer_state: &mut AssetTransferState,
    asset_store: &AssetStore,
) {
    if data.len() < 4 {
        send_error(ws, None, "PARSE_ERROR", "binary frame too short");
        return;
    }

    let header_len_be = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if header_len_be == 0 || data.len() < 4 + header_len_be {
        send_error(ws, None, "PARSE_ERROR", "invalid binary frame header");
        return;
    }

    let header_bytes = &data[4..4 + header_len_be];
    let chunk_payload = &data[4 + header_len_be..];
    let header: AssetUploadChunkHeader = match serde_json::from_slice(header_bytes) {
        Ok(h) => h,
        Err(e) => {
            send_error(
                ws,
                None,
                "PARSE_ERROR",
                &format!("invalid binary chunk header: {e}"),
            );
            return;
        }
    };

    let asset_id = header.asset_id.clone();
    if let Err(e) = transfer_state.on_upload_chunk(header, chunk_payload, now_millis()) {
        eprintln!(
            r#"{{"event":"asset_transfer_failed","assetId":"{}","reason":"invalid_chunk","error":"{}"}}"#,
            asset_id,
            e.to_string().replace('"', "'")
        );
        let missing_chunks = transfer_state
            .uploads
            .get(&asset_id)
            .map(IncomingAssetUpload::missing_chunks)
            .unwrap_or_default();
        send_asset_upload_nack(ws, &asset_id, &missing_chunks, "invalid_chunk");
    } else if asset_store.contains(&asset_id) {
        // Keep state coherent if duplicate chunks arrive after the asset is ready.
        transfer_state
            .entries
            .entry(asset_id)
            .or_default()
            .mark_ready(now_millis());
    }
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

fn request_missing_assets(
    ws: &mut tungstenite::WebSocket<std::net::TcpStream>,
    transfer_state: &mut AssetTransferState,
    asset_store: &AssetStore,
    referenced_ids: &[String],
) {
    let missing =
        transfer_state.collect_requestable_missing(referenced_ids, asset_store, now_millis());
    if missing.is_empty() {
        return;
    }

    eprintln!(
        r#"{{"event":"asset_request_sent","count":{},"assetIds":{:?}}}"#,
        missing.len(),
        missing
    );
    send_asset_request(ws, &missing);
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

fn send_asset_upload_ack(ws: &mut tungstenite::WebSocket<std::net::TcpStream>, asset_id: &str) {
    #[derive(serde::Serialize)]
    struct AssetUploadAckPayload {
        #[serde(rename = "assetId")]
        asset_id: String,
    }

    let ack = WSMessage {
        msg_type: "asset_upload_ack".to_string(),
        timestamp: now_millis(),
        request_id: None,
        payload: Some(AssetUploadAckPayload {
            asset_id: asset_id.to_string(),
        }),
    };

    if let Ok(text) = serde_json::to_string(&ack) {
        let _ = ws.send(Message::Text(text));
    }
}

fn send_asset_upload_nack(
    ws: &mut tungstenite::WebSocket<std::net::TcpStream>,
    asset_id: &str,
    missing_chunks: &[u32],
    reason: &str,
) {
    #[derive(serde::Serialize)]
    struct AssetUploadNackPayload {
        #[serde(rename = "assetId")]
        asset_id: String,
        #[serde(rename = "missingChunks")]
        missing_chunks: Vec<u32>,
        reason: String,
    }

    let nack = WSMessage {
        msg_type: "asset_upload_nack".to_string(),
        timestamp: now_millis(),
        request_id: None,
        payload: Some(AssetUploadNackPayload {
            asset_id: asset_id.to_string(),
            missing_chunks: missing_chunks.to_vec(),
            reason: reason.to_string(),
        }),
    };

    if let Ok(text) = serde_json::to_string(&nack) {
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
        }
    } else {
        true
    };

    if queued && let Some(wake) = ui_wake {
        wake();
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
            assets_added: None,
            assets_removed: None,
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
            assets_added: None,
            assets_removed: None,
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
            assets_added: None,
            assets_removed: None,
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
            assets_added: None,
            assets_removed: None,
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
            assets_added: None,
            assets_removed: None,
        };
        assert!(!delta_updates_only_uniform_values(&cache, &delta));
    }

    #[test]
    fn delta_updates_only_uniform_values_rejects_camera_chain_change() {
        let scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "scene".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("v3", "Vector3Input", json!({"x": 0.0, "y": 0.0, "z": 10.0})),
                node(
                    "cam",
                    "PerspectiveCamera",
                    json!({
                        "target": {"x": 0.0, "y": 0.0, "z": 0.0},
                        "up": {"x": 0.0, "y": 1.0, "z": 0.0},
                        "fovY": 60.0,
                        "aspect": 1.0,
                        "near": 0.1,
                        "far": 100.0
                    }),
                ),
                node("rect", "Rect2DGeometry", json!({})),
                node("pass", "RenderPass", json!({})),
                node("comp", "Composite", json!({})),
                node("rt", "RenderTexture", json!({"width": 400, "height": 400})),
            ],
            connections: vec![
                Connection {
                    id: "c1".to_string(),
                    from: Endpoint {
                        node_id: "v3".to_string(),
                        port_id: "vector".to_string(),
                    },
                    to: Endpoint {
                        node_id: "cam".to_string(),
                        port_id: "position".to_string(),
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
                        node_id: "cam".to_string(),
                        port_id: "camera".to_string(),
                    },
                    to: Endpoint {
                        node_id: "pass".to_string(),
                        port_id: "camera".to_string(),
                    },
                },
                Connection {
                    id: "c4".to_string(),
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
                    id: "c5".to_string(),
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
                updated: vec![node(
                    "v3",
                    "Vector3Input",
                    json!({"x": 5.0, "y": 0.0, "z": 10.0}),
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
            assets_added: None,
            assets_removed: None,
        };
        assert!(!delta_updates_only_uniform_values(&cache, &delta));
    }

    #[test]
    fn asset_transfer_state_reassembles_chunks_and_completes() {
        let mut state = AssetTransferState::default();
        state
            .on_upload_start(
                AssetUploadStartPayload {
                    asset_id: "asset-1".to_string(),
                    mime_type: "application/octet-stream".to_string(),
                    original_name: "asset-1.bin".to_string(),
                    size: 6,
                    chunk_size: 4,
                    total_chunks: 2,
                },
                100,
            )
            .unwrap();

        state
            .on_upload_chunk(
                AssetUploadChunkHeader {
                    frame_type: "asset_upload_chunk".to_string(),
                    asset_id: "asset-1".to_string(),
                    chunk_index: 0,
                    total_chunks: 2,
                    chunk_size: 4,
                    offset: 0,
                    timestamp: None,
                },
                b"ABCD",
                120,
            )
            .unwrap();
        state
            .on_upload_chunk(
                AssetUploadChunkHeader {
                    frame_type: "asset_upload_chunk".to_string(),
                    asset_id: "asset-1".to_string(),
                    chunk_index: 1,
                    total_chunks: 2,
                    chunk_size: 2,
                    offset: 4,
                    timestamp: None,
                },
                b"EF",
                130,
            )
            .unwrap();

        match state.on_upload_end("asset-1", 140) {
            UploadFinalizeResult::Completed(data) => {
                assert_eq!(data.bytes, b"ABCDEF");
            }
            other => panic!("unexpected finalize result: {other:?}"),
        }
    }

    #[test]
    fn asset_transfer_state_nacks_missing_chunks_then_accepts_retry() {
        let mut state = AssetTransferState::default();
        state
            .on_upload_start(
                AssetUploadStartPayload {
                    asset_id: "asset-2".to_string(),
                    mime_type: "application/octet-stream".to_string(),
                    original_name: "asset-2.bin".to_string(),
                    size: 8,
                    chunk_size: 4,
                    total_chunks: 2,
                },
                1,
            )
            .unwrap();
        state
            .on_upload_chunk(
                AssetUploadChunkHeader {
                    frame_type: "asset_upload_chunk".to_string(),
                    asset_id: "asset-2".to_string(),
                    chunk_index: 0,
                    total_chunks: 2,
                    chunk_size: 4,
                    offset: 0,
                    timestamp: None,
                },
                b"ABCD",
                2,
            )
            .unwrap();

        match state.on_upload_end("asset-2", 3) {
            UploadFinalizeResult::MissingChunks(missing) => assert_eq!(missing, vec![1]),
            other => panic!("unexpected finalize result: {other:?}"),
        }

        state
            .on_upload_chunk(
                AssetUploadChunkHeader {
                    frame_type: "asset_upload_chunk".to_string(),
                    asset_id: "asset-2".to_string(),
                    chunk_index: 1,
                    total_chunks: 2,
                    chunk_size: 4,
                    offset: 4,
                    timestamp: None,
                },
                b"EFGH",
                4,
            )
            .unwrap();

        match state.on_upload_end("asset-2", 5) {
            UploadFinalizeResult::Completed(data) => assert_eq!(data.bytes, b"ABCDEFGH"),
            other => panic!("unexpected finalize result: {other:?}"),
        }
    }

    #[test]
    fn asset_request_dedup_and_backoff_prevents_tight_request_loop() {
        let store = AssetStore::new();
        let mut state = AssetTransferState::default();
        let ids = vec!["asset-loop".to_string()];

        // First pass sends request.
        let first = state.collect_requestable_missing(&ids, &store, 0);
        assert_eq!(first, vec!["asset-loop".to_string()]);

        // Immediate pass should dedup while request is in-flight.
        let second = state.collect_requestable_missing(&ids, &store, 100);
        assert!(second.is_empty());

        // After request timeout we still honor retry backoff.
        let timed_out = state.collect_requestable_missing(&ids, &store, ASSET_REQUEST_STALE_MS + 1);
        assert!(timed_out.is_empty());

        // Once backoff expires, request is emitted again.
        let retry = state.collect_requestable_missing(
            &ids,
            &store,
            ASSET_REQUEST_STALE_MS + ASSET_REQUEST_BACKOFF_BASE_MS + 2,
        );
        assert_eq!(retry, vec!["asset-loop".to_string()]);
    }
}
