use super::debug_artifacts::{
    DebugArtifactTransferState, DebugArtifactUploadEndPayload, DebugArtifactUploadStartPayload,
};
use super::*;

pub(super) fn handle_text_message(
    ws: &mut tungstenite::WebSocket<std::net::TcpStream>,
    text: &str,
    scene_tx: &Sender<SceneUpdate>,
    scene_drop_rx: &Receiver<SceneUpdate>,
    last_good: &Arc<Mutex<Option<SceneDSL>>>,
    scene_cache: &Arc<Mutex<Option<SceneCache>>>,
    asset_store: &AssetStore,
    transfer_state: &mut AssetTransferState,
    debug_artifact_transfer_state: &mut DebugArtifactTransferState,
    ui_wake: Option<&UiWakeCallback>,
) -> Result<()> {
    let receive_started_at = Instant::now();
    let server_received_at_ms = now_millis();
    let message_bytes = text.len();
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
    let perf_request_id = msg.request_id.clone();
    let perf_client_sent_at_ms = msg.timestamp;

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
        "shader_template_request" => {
            let payload = match msg.payload {
                Some(payload) => payload,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "missing shader template payload",
                    );
                    return Ok(());
                }
            };
            let request = match serde_json::from_value::<
                shader_templates::ShaderTemplateRequestPayload,
            >(payload)
            {
                Ok(request) => request,
                Err(error) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("invalid shader template request: {error}"),
                    );
                    return Ok(());
                }
            };
            match shader_templates::response(request, msg.request_id.clone()) {
                Ok(response) => {
                    let _ = ws.send(Message::Text(serde_json::to_string(&response)?));
                }
                Err(error) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "UNKNOWN_SHADER_TEMPLATE",
                        &error.to_string(),
                    );
                }
            }
        }
        "scene_request" => {
            let scene = last_good.lock().ok().and_then(|g| g.clone());
            if let Some(scene) = scene {
                let resp = WSMessage {
                    msg_type: "scene_update".to_string(),
                    timestamp: now_millis(),
                    request_id: msg.request_id,
                    payload: Some(serde_json::to_value(RuntimeSceneUpdatePayload {
                        scene,
                        functions:
                            crate::state_machine::mutation_function::installed_document_functions(),
                    })?),
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

            let runtime_payload: RuntimeSceneUpdatePayload = match serde_json::from_value(payload) {
                Ok(s) => s,
                Err(e) => {
                    let message = format!("invalid runtime scene payload: {e}");
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
            let raw_scene = serde_json::to_value(&runtime_payload.scene)?;
            let mut scene = runtime_payload.scene;

            dsl::materialize_scene_node_labels_from_raw_json(&mut scene, &raw_scene);

            if let Err(error) = crate::state_machine::mutation_function::install_document_functions(
                runtime_payload.functions,
            ) {
                let message = format!("invalid Mutation Function resources: {error:#}");
                send_error(ws, msg.request_id.clone(), "VALIDATION_ERROR", &message);
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
            let assets_ready = asset_ids_ready(scene.assets.keys(), asset_store);

            if let Ok(mut guard) = scene_cache.lock() {
                let mut cache = guard
                    .take()
                    .unwrap_or_else(|| SceneCache::from_scene_update(&scene));
                apply_scene_update(&mut cache, &scene);
                *guard = Some(cache);
            }

            if assets_ready {
                // Keep only latest: bounded(1) + drop stale message if receiver hasn't caught up.
                send_scene_update(
                    scene_tx,
                    scene_drop_rx,
                    SceneUpdate::Parsed {
                        scene,
                        request_id: msg.request_id,
                        source: ParsedSceneSource::SceneUpdate,
                        perf_trace: create_scene_perf_trace(
                            &perf_request_id,
                            perf_client_sent_at_ms,
                            server_received_at_ms,
                            message_bytes,
                            receive_started_at,
                        ),
                    },
                    ui_wake,
                );
            }
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
                let assets_ready = asset_ids_ready(cache.assets.keys(), asset_store);

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

                if is_uniform_only_delta && assets_ready {
                    *guard = Some(cache);
                    send_scene_update(
                        scene_tx,
                        scene_drop_rx,
                        SceneUpdate::UniformDelta {
                            updated_nodes: delta.nodes.updated.clone(),
                            request_id: msg.request_id,
                            perf_trace: create_scene_perf_trace(
                                &perf_request_id,
                                perf_client_sent_at_ms,
                                server_received_at_ms,
                                message_bytes,
                                receive_started_at,
                            ),
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

                if assets_ready {
                    scene = Some(materialized);
                } else {
                    apply_scene_update(&mut cache, &materialized);
                }
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
                        perf_trace: create_scene_perf_trace(
                            &perf_request_id,
                            perf_client_sent_at_ms,
                            server_received_at_ms,
                            message_bytes,
                            receive_started_at,
                        ),
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
                        asset_store,
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
        "debug_artifact_request" => {
            // Renderer-side debug artifacts are surfaced through UI actions that
            // broadcast upserts as they happen. The WS thread intentionally does
            // not own artifact content state, so editor requests are accepted as
            // a forward-compatible no-op in v1.
        }
        "debug_artifact_upsert" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "debug_artifact_upsert missing payload",
                    );
                    return Ok(());
                }
            };
            let payload: DebugArtifactUpsertPayload = match serde_json::from_value(payload) {
                Ok(p) => p,
                Err(e) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("invalid debug_artifact_upsert payload: {e}"),
                    );
                    return Ok(());
                }
            };
            send_scene_update(
                scene_tx,
                scene_drop_rx,
                SceneUpdate::DebugArtifactUpsert {
                    item: payload.item,
                    content_text: payload.content_text,
                },
                ui_wake,
            );
        }
        "debug_artifact_delete" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "debug_artifact_delete missing payload",
                    );
                    return Ok(());
                }
            };
            let payload: DebugArtifactDeletePayload = match serde_json::from_value(payload) {
                Ok(p) => p,
                Err(e) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("invalid debug_artifact_delete payload: {e}"),
                    );
                    return Ok(());
                }
            };
            send_scene_update(
                scene_tx,
                scene_drop_rx,
                SceneUpdate::DebugArtifactDelete {
                    artifact_id: payload.artifact_id,
                },
                ui_wake,
            );
        }
        "debug_artifact_upload_start" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "debug_artifact_upload_start missing payload",
                    );
                    return Ok(());
                }
            };
            let payload: DebugArtifactUploadStartPayload = match serde_json::from_value(payload) {
                Ok(p) => p,
                Err(error) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("invalid debug_artifact_upload_start payload: {error}"),
                    );
                    return Ok(());
                }
            };
            if let Err(error) = debug_artifact_transfer_state.start(payload) {
                send_error(
                    ws,
                    msg.request_id,
                    "DEBUG_ARTIFACT_UPLOAD_START_INVALID",
                    &format!("{error:#}"),
                );
            }
        }
        "debug_artifact_upload_end" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "debug_artifact_upload_end missing payload",
                    );
                    return Ok(());
                }
            };
            let payload: DebugArtifactUploadEndPayload = match serde_json::from_value(payload) {
                Ok(p) => p,
                Err(error) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("invalid debug_artifact_upload_end payload: {error}"),
                    );
                    return Ok(());
                }
            };
            match debug_artifact_transfer_state.finish(payload.artifact_id.as_str()) {
                Ok(Some((item, bytes))) => {
                    send_scene_update(
                        scene_tx,
                        scene_drop_rx,
                        SceneUpdate::DebugArtifactBinaryUpsert { item, bytes },
                        ui_wake,
                    );
                }
                Ok(None) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "DEBUG_ARTIFACT_UPLOAD_NOT_STARTED",
                        "debug artifact upload ended before start",
                    );
                }
                Err(error) => {
                    send_error(
                        ws,
                        msg.request_id,
                        "DEBUG_ARTIFACT_UPLOAD_INCOMPLETE",
                        &format!("{error:#}"),
                    );
                }
            }
        }
        "animation_control" => {
            let payload = match msg.payload {
                Some(p) => p,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "animation_control missing payload",
                    );
                    return Ok(());
                }
            };

            let action_str = match payload.get("action").and_then(|v| v.as_str()) {
                Some(a) => a,
                None => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        "animation_control payload missing 'action' field",
                    );
                    return Ok(());
                }
            };

            let action = match action_str {
                "play" => AnimationControlAction::Play,
                "stop" => AnimationControlAction::Stop,
                other => {
                    send_error(
                        ws,
                        msg.request_id,
                        "PARSE_ERROR",
                        &format!("unknown animation_control action: {other}"),
                    );
                    return Ok(());
                }
            };

            send_scene_update(
                scene_tx,
                scene_drop_rx,
                SceneUpdate::AnimationControl { action },
                ui_wake,
            );
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

fn asset_ids_ready<'a>(
    asset_ids: impl IntoIterator<Item = &'a String>,
    asset_store: &AssetStore,
) -> bool {
    asset_ids
        .into_iter()
        .all(|asset_id| asset_store.contains(asset_id))
}

fn scene_after_asset_upload(
    asset_id: &str,
    cache: &SceneCache,
    asset_store: &AssetStore,
) -> Option<SceneDSL> {
    if !cache.assets.contains_key(asset_id) || !asset_ids_ready(cache.assets.keys(), asset_store) {
        return None;
    }

    Some(materialize_scene_dsl(cache))
}

fn trigger_rerender_for_asset(
    asset_id: &str,
    scene_cache: &Arc<Mutex<Option<SceneCache>>>,
    asset_store: &AssetStore,
    scene_tx: &Sender<SceneUpdate>,
    scene_drop_rx: &Receiver<SceneUpdate>,
    ui_wake: Option<&UiWakeCallback>,
) {
    let scene = scene_cache.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .and_then(|cache| scene_after_asset_upload(asset_id, cache, asset_store))
    });

    if let Some(scene) = scene {
        send_scene_update(
            scene_tx,
            scene_drop_rx,
            SceneUpdate::Parsed {
                scene,
                request_id: None,
                source: ParsedSceneSource::SceneUpdate,
                perf_trace: None,
            },
            ui_wake,
        );
    }
}

#[cfg(test)]
mod asset_scene_tests {
    use super::*;
    use crate::{
        asset_store::AssetData,
        dsl::{AssetEntry, Metadata},
    };

    fn scene_cache_with_assets(asset_ids: &[&str]) -> SceneCache {
        let assets = asset_ids
            .iter()
            .map(|asset_id| {
                (
                    (*asset_id).to_string(),
                    AssetEntry {
                        path: format!("assets/{asset_id}.bin"),
                        original_name: format!("{asset_id}.bin"),
                        mime_type: "application/octet-stream".to_string(),
                        size: None,
                    },
                )
            })
            .collect();
        let scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "asset-scene".to_string(),
                created: None,
                modified: None,
            },
            nodes: Vec::new(),
            connections: Vec::new(),
            outputs: Some(HashMap::new()),
            groups: Vec::new(),
            assets,
            state_machine: None,
            debug_artifacts: None,
        };
        SceneCache::from_scene_update(&scene)
    }

    fn insert_asset(store: &AssetStore, asset_id: &str) {
        store.insert(
            asset_id,
            AssetData {
                bytes: vec![1],
                mime_type: "application/octet-stream".to_string(),
                original_name: format!("{asset_id}.bin"),
            },
        );
    }

    #[test]
    fn completed_asset_only_releases_scene_after_all_manifest_assets_are_ready() {
        let store = AssetStore::new();
        let cache = scene_cache_with_assets(&["asset-a", "asset-b"]);

        insert_asset(&store, "asset-a");
        assert!(scene_after_asset_upload("asset-a", &cache, &store).is_none());

        insert_asset(&store, "asset-b");
        assert!(scene_after_asset_upload("asset-b", &cache, &store).is_some());
    }

    #[test]
    fn completed_asset_does_not_release_an_unrelated_scene() {
        let store = AssetStore::new();
        let cache = scene_cache_with_assets(&["asset-a"]);

        insert_asset(&store, "asset-a");
        insert_asset(&store, "stale-asset");
        assert!(scene_after_asset_upload("stale-asset", &cache, &store).is_none());
    }
}

pub(super) fn send_error(
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
            SceneUpdate::AnimationControl { .. } => {
                // Control messages are critical; flush the channel to deliver.
                while scene_drop_rx.try_recv().is_ok() {}
                scene_tx.try_send(update).is_ok()
            }
            SceneUpdate::DebugArtifactUpsert { .. }
            | SceneUpdate::DebugArtifactBinaryUpsert { .. }
            | SceneUpdate::DebugArtifactDelete { .. } => {
                // Artifact messages are side-channel state for the debug UI.
                // Keep an in-flight scene update if one exists; the editor can
                // answer a later request if this update is skipped.
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
