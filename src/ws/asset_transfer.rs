use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow, bail};
use tungstenite::Message;

use crate::asset_store::{AssetData, AssetStore};
use crate::protocol::{WSMessage, now_millis};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const ASSET_REQUEST_STALE_MS: u64 = 5_000;
const ASSET_RECEIVE_STALE_MS: u64 = 15_000;
const ASSET_REQUEST_BACKOFF_BASE_MS: u64 = 1_000;
const ASSET_REQUEST_BACKOFF_MAX_MS: u64 = 30_000;
const MAX_ASSET_SIZE_BYTES: usize = 2 * 1024 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Transfer tracking
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Upload payload types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Deserialize)]
pub(super) struct AssetUploadStartPayload {
    #[serde(rename = "assetId")]
    pub(super) asset_id: String,
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
pub(super) struct AssetUploadEndPayload {
    #[serde(rename = "assetId")]
    pub(super) asset_id: String,
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

// ---------------------------------------------------------------------------
// Incoming upload reassembly
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Transfer state machine
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(super) enum UploadFinalizeResult {
    Completed(AssetData),
    MissingChunks(Vec<u32>),
    NotStarted,
}

#[derive(Debug, Default)]
pub(super) struct AssetTransferState {
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

    pub(super) fn collect_requestable_missing(
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

    pub(super) fn on_upload_start(
        &mut self,
        payload: AssetUploadStartPayload,
        now_ms: u64,
    ) -> Result<()> {
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

    pub(super) fn on_upload_end(&mut self, asset_id: &str, now_ms: u64) -> UploadFinalizeResult {
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

    pub(super) fn on_asset_removed(&mut self, asset_id: &str) {
        self.entries.remove(asset_id);
        self.uploads.remove(asset_id);
    }
}

// ---------------------------------------------------------------------------
// Wire protocol helpers
// ---------------------------------------------------------------------------

/// Parse chunk frames:
/// `[header_len: u32 BE][JSON header (UTF-8)][raw chunk bytes]`
/// where the JSON header `type` is `asset_upload_chunk`.
pub(super) fn handle_binary_asset_upload(
    ws: &mut tungstenite::WebSocket<std::net::TcpStream>,
    data: &[u8],
    transfer_state: &mut AssetTransferState,
    asset_store: &AssetStore,
) {
    if data.len() < 4 {
        super::send_error(ws, None, "PARSE_ERROR", "binary frame too short");
        return;
    }

    let header_len_be = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if header_len_be == 0 || data.len() < 4 + header_len_be {
        super::send_error(ws, None, "PARSE_ERROR", "invalid binary frame header");
        return;
    }

    let header_bytes = &data[4..4 + header_len_be];
    let chunk_payload = &data[4 + header_len_be..];
    let header: AssetUploadChunkHeader = match serde_json::from_slice(header_bytes) {
        Ok(h) => h,
        Err(e) => {
            super::send_error(
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

pub(super) fn request_missing_assets(
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

pub(super) fn send_asset_upload_ack(
    ws: &mut tungstenite::WebSocket<std::net::TcpStream>,
    asset_id: &str,
) {
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

pub(super) fn send_asset_upload_nack(
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_store::AssetStore;

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
