use super::*;

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
pub(super) struct DebugArtifactUploadChunkHeader {
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
pub(super) struct DebugArtifactTransferState {
    uploads: HashMap<String, IncomingDebugArtifactUpload>,
}

impl DebugArtifactTransferState {
    pub(super) fn start(&mut self, payload: DebugArtifactUploadStartPayload) -> Result<()> {
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

    pub(super) fn chunk(
        &mut self,
        header: DebugArtifactUploadChunkHeader,
        bytes: &[u8],
    ) -> Result<()> {
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

    pub(super) fn finish(
        &mut self,
        artifact_id: &str,
    ) -> Result<Option<(DebugArtifactItem, Vec<u8>)>> {
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

pub(super) fn parse_binary_frame_header(data: &[u8]) -> Option<(String, serde_json::Value, &[u8])> {
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
