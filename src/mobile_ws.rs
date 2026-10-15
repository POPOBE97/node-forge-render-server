//! USB loopback transport. Socket and asset IO never run on the render thread.
use crate::{asset_store::AssetStore, protocol::now_millis};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig};

#[path = "ws/asset_transfer.rs"]
mod asset_transfer;
#[path = "ws/scene_delta.rs"]
pub mod scene_delta;
#[path = "ws/shader_templates.rs"]
mod shader_templates;
use asset_transfer::*;

pub const PORT: u16 = 18080;
pub const PROTOCOL_VERSION: u32 = 1;

pub struct Packet {
    pub generation: u64,
    pub value: Value,
}

pub struct Bridge {
    incoming: Mutex<VecDeque<Packet>>,
    outgoing: Mutex<VecDeque<(u64, Message)>>,
    pub assets: AssetStore,
    pub status: Mutex<Value>,
    generation: AtomicU64,
    connected: AtomicU64,
}

impl Bridge {
    pub fn start() -> Result<Arc<Self>> {
        static INSTANCE: OnceLock<Arc<Bridge>> = OnceLock::new();
        if let Some(instance) = INSTANCE.get() {
            return Ok(instance.clone());
        }
        let listener = TcpListener::bind(("127.0.0.1", PORT)).context("Bind Android USB player")?;
        let bridge = Arc::new(Self {
            incoming: Mutex::new(VecDeque::new()),
            outgoing: Mutex::new(VecDeque::new()),
            assets: AssetStore::new(),
            status: Mutex::new(json!({"ready":false})),
            generation: AtomicU64::new(0),
            connected: AtomicU64::new(0),
        });
        let server = bridge.clone();
        thread::Builder::new()
            .name("NodeForgeUSB".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else {
                        continue;
                    };
                    let server = server.clone();
                    thread::spawn(move || {
                        if let Err(error) = serve(stream, &server) {
                            eprintln!("USB player: {error:#}");
                        }
                    });
                }
            })?;
        let _ = INSTANCE.set(bridge.clone());
        Ok(bridge)
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Acquire) == self.generation() && self.generation() != 0
    }
    pub fn take(&self) -> Option<Packet> {
        self.incoming.lock().unwrap().pop_front()
    }
    pub fn send(&self, generation: u64, kind: &str, request: Option<&str>, payload: Value) {
        self.send_raw(
            generation,
            Message::Text(
                json!({"type":kind,"timestamp":now_millis(),"requestId":request,"payload":payload})
                    .to_string(),
            ),
        );
    }
    pub fn send_raw(&self, generation: u64, message: Message) {
        if generation != self.generation() || !self.connected() {
            return;
        }
        let mut queue = self.outgoing.lock().unwrap();
        if queue.len() < 128 {
            queue.push_back((generation, message));
        } else {
            let _ =
                self.connected
                    .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }
    pub fn send_capture(&self, generation: u64, header: Value, bytes: &[u8]) -> Result<()> {
        let header = serde_json::to_vec(&header)?;
        let mut frame = Vec::with_capacity(4 + header.len() + bytes.len());
        frame.extend_from_slice(&(header.len() as u32).to_be_bytes());
        frame.extend_from_slice(&header);
        frame.extend_from_slice(bytes);
        self.send_raw(generation, Message::Binary(frame));
        Ok(())
    }
}

fn send_error(ws: &mut WebSocket<TcpStream>, request: Option<String>, code: &str, detail: &str) {
    let _ = ws.send(Message::Text(json!({"type":"error","timestamp":now_millis(),"requestId":request,"payload":{"code":code,"message":detail}}).to_string()));
}

fn serve(stream: TcpStream, bridge: &Bridge) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    stream.set_nodelay(true)?;
    let config = WebSocketConfig {
        max_message_size: Some(64 * 1024 * 1024),
        max_frame_size: Some(64 * 1024 * 1024),
        ..Default::default()
    };
    let mut ws = tungstenite::accept_with_config(stream, Some(config))?;
    ws.get_mut()
        .set_read_timeout(Some(Duration::from_millis(8)))?;
    let generation = bridge.generation.fetch_add(1, Ordering::AcqRel) + 1;
    struct Connection<'a> {
        bridge: &'a Bridge,
        generation: u64,
    }
    impl Drop for Connection<'_> {
        fn drop(&mut self) {
            if self.bridge.generation() == self.generation {
                let _ = self.bridge.connected.compare_exchange(
                    self.generation,
                    0,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
        }
    }
    let _connection = Connection { bridge, generation };
    bridge.incoming.lock().unwrap().clear();
    bridge.outgoing.lock().unwrap().clear();
    bridge.connected.store(generation, Ordering::Release);
    bridge.send(generation, "player_hello", None, json!({"protocolVersion":PROTOCOL_VERSION,"functionAbi":10,"schema":"6.0","generation":generation,"status":*bridge.status.lock().unwrap()}));
    bridge.send(
        generation,
        "scene_resync_request",
        None,
        json!({"reason":"USB player connected"}),
    );
    let mut transfers = AssetTransferState::default();
    loop {
        if bridge.generation() != generation {
            send_error(
                &mut ws,
                None,
                "SESSION_REPLACED",
                "Another Editor or validation session took control of this phone",
            );
            let _ = ws.close(None);
            return Ok(());
        }
        if !bridge.connected() {
            bail!("Outgoing queue overflow; reconnect to resynchronize");
        }
        loop {
            let message = {
                let mut queue = bridge.outgoing.lock().unwrap();
                if bridge.generation() != generation {
                    None
                } else {
                    queue.pop_front()
                }
            };
            let Some((owner, message)) = message else {
                break;
            };
            if owner == generation {
                ws.send(message)?;
            }
        }
        let message = match ws.read() {
            Ok(message) => message,
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        match message {
            Message::Binary(bytes) => {
                handle_binary_asset_upload(&mut ws, &bytes, &mut transfers, &bridge.assets)
            }
            Message::Text(text) => {
                let value: Value = match serde_json::from_str(&text) {
                    Ok(value) => value,
                    Err(error) => {
                        send_error(&mut ws, None, "PARSE_ERROR", &error.to_string());
                        continue;
                    }
                };
                let request = value["requestId"].as_str();
                let result: Result<bool> = (|| {
                    match value["type"].as_str().unwrap_or("") {
                        "ping" => {
                            bridge.send(generation, "pong", request, json!({}));
                        }
                        "player_status" => {
                            bridge.send(
                                generation,
                                "player_status",
                                request,
                                bridge.status.lock().unwrap().clone(),
                            );
                        }
                        "shader_template_request" => {
                            let response = shader_templates::response(
                                serde_json::from_value(value["payload"].clone())?,
                                request.map(str::to_owned),
                            )
                            .map_err(|error| anyhow::anyhow!("{error}"))?;
                            bridge.send_raw(
                                generation,
                                Message::Text(serde_json::to_string(&response)?),
                            );
                        }
                        "asset_upload_start" => {
                            transfers.on_upload_start(
                                serde_json::from_value(value["payload"].clone())?,
                                now_millis(),
                            )?;
                        }
                        "asset_upload_end" => {
                            let payload: AssetUploadEndPayload =
                                serde_json::from_value(value["payload"].clone())?;
                            match transfers.on_upload_end(&payload.asset_id, now_millis()) {
                                UploadFinalizeResult::Completed(data) => {
                                    let hash = format!("{:x}", Sha256::digest(&data.bytes));
                                    if payload.asset_id != hash[..12] {
                                        bail!("Asset content hash mismatch: {}", payload.asset_id);
                                    }
                                    bridge
                                        .assets
                                        .insert_or_replace(payload.asset_id.clone(), data);
                                    send_asset_upload_ack(&mut ws, &payload.asset_id);
                                }
                                UploadFinalizeResult::MissingChunks(chunks) => {
                                    send_asset_upload_nack(
                                        &mut ws,
                                        &payload.asset_id,
                                        &chunks,
                                        "missing_chunks",
                                    )
                                }
                                UploadFinalizeResult::NotStarted => {
                                    bail!("asset upload not started")
                                }
                            }
                        }
                        "frame_freezes_sync"
                        | "debug_artifact_upsert"
                        | "debug_artifact_delete" => {}
                        _ => return Ok(false),
                    }
                    Ok(true)
                })();
                match result {
                    Ok(true) => {}
                    Ok(false) => {
                        let mut queue = bridge.incoming.lock().unwrap();
                        if queue.len() >= 32 {
                            queue.clear();
                            bridge.send(
                                generation,
                                "scene_resync_request",
                                None,
                                json!({"reason":"backpressure"}),
                            );
                        } else {
                            queue.push_back(Packet { generation, value });
                        }
                    }
                    Err(error) => send_error(
                        &mut ws,
                        request.map(str::to_owned),
                        "PROTOCOL_ERROR",
                        &format!("{error:#}"),
                    ),
                }
            }
            Message::Close(_) => return Ok(()),
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {
                let _ = ws.flush();
            }
        }
    }
}
