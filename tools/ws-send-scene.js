#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const WebSocket = require('ws');

function usage() {
  console.error(
    [
      'Usage:',
      '  node tools/ws-send-scene.js <scene.json> [ws://host:port]',
      '  node tools/ws-send-scene.js --request [ws://host:port]',
      '',
      'Examples:',
      '  node tools/ws-send-scene.js assets/node-forge-example.1.json ws://127.0.0.1:8080',
      '  node tools/ws-send-scene.js --request ws://127.0.0.1:8080'
    ].join('\n')
  );
  process.exit(2);
}

const args = process.argv.slice(2);
if (args.length === 0) usage();

const wsUrl = args[args.length - 1].startsWith('ws://') || args[args.length - 1].startsWith('wss://')
  ? args[args.length - 1]
  : 'ws://127.0.0.1:8080';

const isRequest = args[0] === '--request';
const scenePath = !isRequest ? args[0] : null;

if (!isRequest && (!scenePath || scenePath.startsWith('ws://') || scenePath.startsWith('wss://'))) {
  usage();
}

const requestId = crypto.randomUUID ? crypto.randomUUID() : crypto.randomBytes(8).toString('hex');

const msg = {
  type: isRequest ? 'scene_request' : 'scene_update',
  timestamp: Date.now(),
  requestId,
  payload: null
};

if (!isRequest) {
  const abs = path.resolve(scenePath);
  const text = fs.readFileSync(abs, 'utf8');
  msg.payload = JSON.parse(text);
}

const ws = new WebSocket(wsUrl);
let receivedAny = false;
let pendingAssets = 0;
let closeTimer = null;
const activeUploads = new Map();
const completedUploads = new Set();
const CHUNK_SIZE = 4 * 1024 * 1024;

// Resolve the directory containing the scene file for asset path resolution.
const sceneDir = scenePath ? path.dirname(path.resolve(scenePath)) : null;

function encodeChunkFrame(headerObj, chunkBytes) {
  const headerBuf = Buffer.from(JSON.stringify(headerObj), 'utf8');
  const lenBuf = Buffer.alloc(4);
  lenBuf.writeUInt32BE(headerBuf.length, 0);
  return Buffer.concat([lenBuf, headerBuf, chunkBytes]);
}

function sendUploadStart(session) {
  ws.send(JSON.stringify({
    type: 'asset_upload_start',
    timestamp: Date.now(),
    payload: {
      assetId: session.assetId,
      mimeType: session.mimeType,
      originalName: session.originalName,
      size: session.size,
      chunkSize: session.chunkSize,
      totalChunks: session.totalChunks,
    },
  }));
  console.log(`[client] asset_upload_start '${session.assetId}' (${session.size} bytes, chunks=${session.totalChunks})`);
}

function sendChunk(session, chunkIndex) {
  if (chunkIndex < 0 || chunkIndex >= session.totalChunks) return;
  const offset = chunkIndex * session.chunkSize;
  const end = Math.min(offset + session.chunkSize, session.size);
  const chunkBytes = session.bytes.subarray(offset, end);
  const frame = encodeChunkFrame(
    {
      type: 'asset_upload_chunk',
      assetId: session.assetId,
      chunkIndex,
      totalChunks: session.totalChunks,
      chunkSize: chunkBytes.length,
      offset,
      timestamp: Date.now(),
    },
    chunkBytes,
  );
  ws.send(frame);
}

function sendUploadEnd(assetId) {
  ws.send(JSON.stringify({
    type: 'asset_upload_end',
    timestamp: Date.now(),
    payload: { assetId },
  }));
  console.log(`[client] asset_upload_end '${assetId}'`);
}

function startUpload(assetId, entry) {
  if (!entry || !entry.path) {
    console.warn(`[client] asset_request for '${assetId}' but no manifest entry found`);
    return;
  }
  if (completedUploads.has(assetId)) {
    console.log(`[client] skip '${assetId}' (already ACKed)`);
    return;
  }
  if (activeUploads.has(assetId)) {
    console.log(`[client] skip '${assetId}' (already uploading)`);
    return;
  }

  const assetPath = path.resolve(sceneDir, entry.path);
  if (!fs.existsSync(assetPath)) {
    console.warn(`[client] asset '${assetId}' not found at ${assetPath}`);
    return;
  }

  const bytes = fs.readFileSync(assetPath);
  const session = {
    assetId,
    bytes,
    size: bytes.length,
    chunkSize: CHUNK_SIZE,
    totalChunks: Math.ceil(bytes.length / CHUNK_SIZE),
    mimeType: entry.mimeType || 'application/octet-stream',
    originalName: entry.originalName || path.basename(assetPath),
  };

  activeUploads.set(assetId, session);
  pendingAssets++;
  sendUploadStart(session);
  for (let i = 0; i < session.totalChunks; i++) {
    sendChunk(session, i);
  }
  sendUploadEnd(assetId);
}

function resendChunks(assetId, missingChunks) {
  const session = activeUploads.get(assetId);
  if (!session) {
    console.warn(`[client] NACK for '${assetId}' but no active upload session`);
    return;
  }
  const indexes = Array.isArray(missingChunks) && missingChunks.length > 0
    ? missingChunks
    : [...Array(session.totalChunks).keys()];
  for (const idx of indexes) {
    if (!Number.isInteger(idx) || idx < 0 || idx >= session.totalChunks) continue;
    sendChunk(session, idx);
  }
  sendUploadEnd(assetId);
  console.log(`[client] resent chunks for '${assetId}': ${indexes.join(',')}`);
}

function scheduleClose(delay) {
  if (closeTimer) clearTimeout(closeTimer);
  closeTimer = setTimeout(() => {
    if (pendingAssets <= 0) ws.close();
  }, delay);
}

ws.on('open', () => {
  ws.send(JSON.stringify(msg));
  if (!isRequest) {
    console.log(`[client] sent scene_update requestId=${requestId}`);
    // Don't close immediately — wait for possible asset_request messages.
    scheduleClose(10000);
  } else {
    console.log(`[client] sent scene_request requestId=${requestId}`);
    setTimeout(() => {
      if (!receivedAny) ws.close();
    }, 2000);
  }
});

ws.on('message', (data, isBinary) => {
  // Binary messages are not expected from server in this tool; ignore.
  if (isBinary) return;

  const text = data.toString('utf8');
  receivedAny = true;
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    console.log('[server]', text);
    return;
  }

  // Handle asset_request from the server.
  if (parsed.type === 'asset_request' && parsed.payload && sceneDir && msg.payload) {
    const ids = parsed.payload.assetIds || (parsed.payload.assetId ? [parsed.payload.assetId] : []);
    const assets = msg.payload.assets || {};
    for (const id of ids) {
      const entry = assets[id];
      startUpload(id, entry);
    }
    scheduleClose(pendingAssets > 0 ? 10000 : 1500);
    return;
  }

  if (parsed.type === 'asset_upload_ack' && parsed.payload?.assetId) {
    const id = parsed.payload.assetId;
    if (activeUploads.delete(id)) {
      pendingAssets = Math.max(0, pendingAssets - 1);
      completedUploads.add(id);
      console.log(`[client] ACK '${id}'`);
    } else {
      console.log(`[client] ACK '${id}' (no active upload)`);
    }
    scheduleClose(pendingAssets > 0 ? 10000 : 1500);
    return;
  }

  if (parsed.type === 'asset_upload_nack' && parsed.payload?.assetId) {
    const id = parsed.payload.assetId;
    const missing = parsed.payload.missingChunks || [];
    console.warn(`[client] NACK '${id}' reason=${parsed.payload.reason || 'unknown'} missing=${JSON.stringify(missing)}`);
    resendChunks(id, missing);
    scheduleClose(10000);
    return;
  }

  console.log('[server]', JSON.stringify(parsed, null, 2));

  if (isRequest) {
    ws.close();
  }
});

ws.on('error', (err) => {
  console.error('[client] ws error:', err);
  process.exitCode = 1;
});

ws.on('close', () => {
  // keep it one-shot; close after first response or server close
  process.exit();
});
