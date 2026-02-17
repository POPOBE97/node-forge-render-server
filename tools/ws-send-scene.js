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

// Resolve the directory containing the scene file for asset path resolution.
const sceneDir = scenePath ? path.dirname(path.resolve(scenePath)) : null;

/**
 * Upload an asset as a binary frame using the new spec format:
 * [header_len: u32 BE][JSON header (UTF-8)][raw asset data]
 */
function uploadAsset(assetId, assetBytes, mimeType, originalName) {
  const header = JSON.stringify({
    type: 'asset_upload',
    assetId,
    mimeType: mimeType || 'application/octet-stream',
    originalName: originalName || '',
    size: assetBytes.length,
    timestamp: Date.now(),
  });
  const headerBuf = Buffer.from(header, 'utf8');
  const lenBuf = Buffer.alloc(4);
  lenBuf.writeUInt32BE(headerBuf.length, 0);
  const frame = Buffer.concat([lenBuf, headerBuf, assetBytes]);
  ws.send(frame);
  console.log(`[client] uploaded asset '${assetId}' (${assetBytes.length} bytes, ${mimeType || '?'})`);
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
    scheduleClose(2000);
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
      if (!entry || !entry.path) {
        console.warn(`[client] asset_request for '${id}' but no manifest entry found`);
        continue;
      }
      const assetPath = path.resolve(sceneDir, entry.path);
      if (!fs.existsSync(assetPath)) {
        console.warn(`[client] asset '${id}' not found at ${assetPath}`);
        continue;
      }
      pendingAssets++;
      const assetBytes = fs.readFileSync(assetPath);
      uploadAsset(id, assetBytes, entry.mimeType, entry.originalName);
      pendingAssets--;
    }
    // Give the server a moment to rebuild, then close.
    scheduleClose(1000);
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
