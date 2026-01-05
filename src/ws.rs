use std::{
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use serde_json::Value;
use tungstenite::{accept, Error as WsError, Message};

use crate::{
    dsl::SceneDSL,
    protocol::{now_millis, ErrorPayload, WSMessage},
};

#[derive(Debug, Clone)]
pub struct SceneUpdate {
    pub scene: SceneDSL,
    pub request_id: Option<String>,
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
) -> thread::JoinHandle<()> {
    let addr = addr.to_string();
    thread::spawn(move || {
        if let Err(e) = run_ws_server(&addr, scene_tx, scene_drop_rx, hub, last_good) {
            eprintln!("[ws] server failed: {e:?}");
        }
    })
}

fn run_ws_server(
    addr: &str,
    scene_tx: Sender<SceneUpdate>,
    scene_drop_rx: Receiver<SceneUpdate>,
    hub: WsHub,
    last_good: Arc<Mutex<Option<SceneDSL>>>,
) -> Result<()> {
    let server = TcpListener::bind(addr).with_context(|| format!("failed to bind ws server at {addr}"))?;
    eprintln!("[ws] listening on ws://{addr}");

    for stream in server.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[ws] accept tcp failed: {e}");
                continue;
            }
        };

        let scene_tx = scene_tx.clone();
        let scene_drop_rx = scene_drop_rx.clone();
        let hub = hub.clone();
        let last_good = last_good.clone();

        thread::spawn(move || {
            if let Err(e) = handle_client(stream, scene_tx, scene_drop_rx, hub, last_good) {
                eprintln!("[ws] client ended: {e:?}");
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
                ) {
                    eprintln!("[ws] handle message error: {e:?}");
                }
            }
            Ok(Message::Binary(_)) => {
                // ignore
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
) -> Result<()> {
    let msg: WSMessage<Value> = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            send_error(ws, None, "PARSE_ERROR", &format!("invalid json: {e}"));
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
                    send_error(ws, msg.request_id, "PARSE_ERROR", "missing payload");
                    return Ok(());
                }
            };

            let scene: SceneDSL = match serde_json::from_value(payload) {
                Ok(s) => s,
                Err(e) => {
                    send_error(ws, msg.request_id, "PARSE_ERROR", &format!("invalid SceneDSL: {e}"));
                    return Ok(());
                }
            };

            // Keep only latest: bounded(1) + drop stale message if receiver hasn't caught up.
            let update = SceneUpdate {
                scene,
                request_id: msg.request_id,
            };

            if scene_tx.try_send(update.clone()).is_err() {
                while scene_drop_rx.try_recv().is_ok() {}
                let _ = scene_tx.try_send(update);
            }
        }
        other => {
            send_error(ws, msg.request_id, "PARSE_ERROR", &format!("unknown message type: {other}"));
        }
    }

    Ok(())
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
