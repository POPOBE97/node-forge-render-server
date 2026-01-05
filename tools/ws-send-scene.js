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

ws.on('open', () => {
  ws.send(JSON.stringify(msg));
  if (!isRequest) {
    console.log(`[client] sent scene_update requestId=${requestId}`);
    setTimeout(() => ws.close(), 100);
  } else {
    console.log(`[client] sent scene_request requestId=${requestId}`);
    setTimeout(() => {
      if (!receivedAny) ws.close();
    }, 2000);
  }
});

ws.on('message', (data) => {
  const text = data.toString('utf8');
  receivedAny = true;
  try {
    const parsed = JSON.parse(text);
    console.log('[server]', JSON.stringify(parsed, null, 2));
  } catch {
    console.log('[server]', text);
  }

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
