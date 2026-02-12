use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use rust_wgpu_fiber::eframe::{egui, egui_wgpu, wgpu};

use crate::{protocol, renderer, ws};

use super::types::App;

pub struct SceneApplyResult {
    pub did_rebuild_shader_space: bool,
    pub texture_filter_override: Option<wgpu::FilterMode>,
    pub reset_viewport: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SceneUpdateMode {
    Rebuild,
    UniformOnly,
}

#[derive(Debug)]
struct GraphBufferUpdate {
    pass_index: usize,
    bytes: Vec<u8>,
    hash: [u8; 32],
}

fn choose_scene_update_mode(
    last_pipeline_signature: Option<[u8; 32]>,
    next_pipeline_signature: [u8; 32],
) -> SceneUpdateMode {
    if last_pipeline_signature == Some(next_pipeline_signature) {
        SceneUpdateMode::UniformOnly
    } else {
        SceneUpdateMode::Rebuild
    }
}

fn collect_graph_uniform_updates(
    scene: &crate::dsl::SceneDSL,
    passes: &[renderer::PassBindings],
) -> Result<Vec<GraphBufferUpdate>> {
    let mut out = Vec::new();
    for (pass_index, pass) in passes.iter().enumerate() {
        let Some(binding) = pass.graph_binding.as_ref() else {
            continue;
        };
        let bytes = renderer::graph_uniforms::pack_graph_values(scene, &binding.schema)
            .with_context(|| format!("failed to pack graph values for pass '{}'", pass.pass_id))?;
        let hash = renderer::graph_uniforms::hash_bytes(bytes.as_slice());
        if pass.last_graph_hash != Some(hash) {
            out.push(GraphBufferUpdate {
                pass_index,
                bytes,
                hash,
            });
        }
    }
    Ok(out)
}

fn apply_graph_uniform_updates(app: &mut App, scene: &crate::dsl::SceneDSL) -> Result<usize> {
    let updates = collect_graph_uniform_updates(scene, &app.passes)?;
    for update in &updates {
        let buffer_name = app.passes[update.pass_index]
            .graph_binding
            .as_ref()
            .map(|b| b.buffer_name.clone())
            .with_context(|| {
                format!(
                    "graph binding missing while applying update for pass '{}'",
                    app.passes[update.pass_index].pass_id
                )
            })?;
        app.shader_space
            .write_buffer(buffer_name.as_str(), 0, update.bytes.as_slice())
            .with_context(|| format!("failed to write graph buffer '{}'", buffer_name.as_str()))?;
        app.passes[update.pass_index].last_graph_hash = Some(update.hash);
    }
    Ok(updates.len())
}

fn apply_uniform_node_param_updates(
    scene: &mut crate::dsl::SceneDSL,
    updated_nodes: &[crate::dsl::Node],
    allow_suffix_match: bool,
) -> Result<()> {
    for updated in updated_nodes {
        if let Some(target) = scene.nodes.iter_mut().find(|n| n.id == updated.id) {
            if target.node_type != updated.node_type {
                bail!(
                    "uniform delta node type mismatch for '{}': cached='{}' incoming='{}'",
                    updated.id,
                    target.node_type,
                    updated.node_type
                );
            }
            for (k, v) in &updated.params {
                target.params.insert(k.clone(), v.clone());
            }
            continue;
        }

        if allow_suffix_match {
            let suffix = format!("/{}", updated.id);
            let mut matched = 0usize;
            for target in &mut scene.nodes {
                if !target.id.ends_with(&suffix) {
                    continue;
                }
                if target.node_type != updated.node_type {
                    continue;
                }
                for (k, v) in &updated.params {
                    target.params.insert(k.clone(), v.clone());
                }
                matched += 1;
            }
            if matched > 0 {
                continue;
            }
        }

        return Err(anyhow!(
            "uniform delta references missing node '{}'",
            updated.id
        ));
    }
    Ok(())
}

pub fn drain_latest_scene_update(app: &App) -> Option<ws::SceneUpdate> {
    let mut latest: Option<ws::SceneUpdate> = None;
    while let Ok(update) = app.scene_rx.try_recv() {
        latest = Some(update);
    }
    latest
}

pub fn apply_scene_resolution_to_window_state(
    current_window_resolution: [u32; 2],
    scene_screen_resolution: Option<[u32; 2]>,
    follow_scene_resolution_for_window: bool,
) -> ([u32; 2], Option<[f32; 2]>) {
    let Some([w, h]) = scene_screen_resolution else {
        return (current_window_resolution, None);
    };

    if [w, h] == current_window_resolution {
        return (current_window_resolution, None);
    }

    let next_window_resolution = [w, h];
    let maybe_resize = if follow_scene_resolution_for_window {
        Some([w as f32, h as f32])
    } else {
        None
    };

    (next_window_resolution, maybe_resize)
}

pub fn apply_scene_update(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    update: ws::SceneUpdate,
) -> SceneApplyResult {
    match update {
        ws::SceneUpdate::UniformDelta {
            updated_nodes,
            request_id,
        } => {
            let scene = match app.last_good.lock() {
                Ok(mut guard) => guard.take(),
                Err(_) => None,
            };

            let Some(mut scene) = scene else {
                let message =
                    "received uniform-only update without a baseline scene; waiting for scene_update"
                        .to_string();
                eprintln!("[scene-runtime] {message}");
                broadcast_error(app, request_id, "RESYNC_REQUIRED", message);
                return SceneApplyResult {
                    did_rebuild_shader_space: false,
                    texture_filter_override: None,
                    reset_viewport: false,
                };
            };

            let mut cached_uniform_scene = app.uniform_scene.take();
            let update_result = (|| -> Result<crate::dsl::SceneDSL> {
                apply_uniform_node_param_updates(&mut scene, &updated_nodes, false)?;

                let mut uniform_scene = if let Some(cached) = cached_uniform_scene.take() {
                    cached
                } else {
                    renderer::prepare_scene(&scene)
                        .context("failed to prepare baseline scene for uniform-only update")?
                        .scene
                };

                apply_uniform_node_param_updates(&mut uniform_scene, &updated_nodes, true)?;
                let _ = apply_graph_uniform_updates(app, &uniform_scene)?;
                Ok(uniform_scene)
            })();

            if let Ok(mut guard) = app.last_good.lock() {
                *guard = Some(scene);
            }

            match update_result {
                Ok(uniform_scene) => {
                    app.uniform_scene = Some(uniform_scene);
                    app.uniform_only_update_count = app.uniform_only_update_count.saturating_add(1);
                    SceneApplyResult {
                        did_rebuild_shader_space: false,
                        texture_filter_override: None,
                        reset_viewport: false,
                    }
                }
                Err(e) => {
                    app.uniform_scene = None;
                    let message = format!("uniform-only update failed: {e:#}");
                    eprintln!("[scene-runtime] {message}");
                    broadcast_error(app, request_id, "UNIFORM_UPDATE_FAILED", message);
                    SceneApplyResult {
                        did_rebuild_shader_space: false,
                        texture_filter_override: None,
                        reset_viewport: false,
                    }
                }
            }
        }
        ws::SceneUpdate::Parsed {
            scene,
            request_id,
            source,
        } => {
            let should_reset_viewport = matches!(source, ws::ParsedSceneSource::SceneUpdate);
            let (next_window_resolution, maybe_resize) = apply_scene_resolution_to_window_state(
                app.window_resolution,
                crate::dsl::screen_resolution(&scene),
                app.follow_scene_resolution_for_window,
            );
            app.window_resolution = next_window_resolution;

            if let Some([w, h]) = maybe_resize {
                let size = egui::vec2(w, h);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(size));
            }

            let mut prepared_scene_candidate: Option<crate::dsl::SceneDSL> = None;
            if let Ok(prepared_for_fast_path) = renderer::prepare_scene(&scene) {
                prepared_scene_candidate = Some(prepared_for_fast_path.scene.clone());
                let next_pipeline_signature =
                    renderer::graph_uniforms::compute_pipeline_signature_for_pass_bindings(
                        &prepared_for_fast_path.scene,
                        &app.passes,
                    );
                if choose_scene_update_mode(app.last_pipeline_signature, next_pipeline_signature)
                    == SceneUpdateMode::UniformOnly
                {
                    match apply_graph_uniform_updates(app, &prepared_for_fast_path.scene) {
                        Ok(_updated_count) => {
                            app.last_pipeline_signature = Some(next_pipeline_signature);
                            app.uniform_scene = prepared_scene_candidate;
                            app.uniform_only_update_count =
                                app.uniform_only_update_count.saturating_add(1);
                            if let Ok(mut g) = app.last_good.lock() {
                                *g = Some(scene);
                            }
                            return SceneApplyResult {
                                did_rebuild_shader_space: false,
                                texture_filter_override: None,
                                reset_viewport: should_reset_viewport,
                            };
                        }
                        Err(e) => {
                            eprintln!(
                                "[scene-runtime] uniform-only graph update failed; forcing rebuild: {e:#}"
                            );
                        }
                    }
                }
            }

            let build_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                renderer::ShaderSpaceBuilder::new(
                    Arc::new(render_state.device.clone()),
                    Arc::new(render_state.queue.clone()),
                )
                .with_options(renderer::ShaderSpaceBuildOptions {
                    presentation_mode: renderer::ShaderSpacePresentationMode::UiSdrDisplayEncode,
                    debug_dump_wgsl_dir: None,
                })
                .build(&scene)
            }));

            match build_result {
                Ok(Ok(result)) => {
                    app.shader_space = result.shader_space;
                    app.resolution = result.resolution;
                    app.passes = result.pass_bindings;
                    app.output_texture_name = result.present_output_texture;
                    app.last_pipeline_signature = Some(result.pipeline_signature);
                    app.uniform_scene = prepared_scene_candidate
                        .or_else(|| renderer::prepare_scene(&scene).ok().map(|p| p.scene));
                    app.pipeline_rebuild_count = app.pipeline_rebuild_count.saturating_add(1);

                    if let Ok(mut g) = app.last_good.lock() {
                        *g = Some(scene);
                    }

                    SceneApplyResult {
                        did_rebuild_shader_space: true,
                        texture_filter_override: None,
                        reset_viewport: should_reset_viewport,
                    }
                }
                Ok(Err(e)) => {
                    let message = format!("{e:#}");
                    eprintln!("[error-plane] scene build failed: {message}");
                    app.uniform_scene = None;
                    broadcast_error(app, request_id, "VALIDATION_ERROR", message);
                    apply_error_plane(app, render_state);
                    SceneApplyResult {
                        did_rebuild_shader_space: true,
                        texture_filter_override: None,
                        reset_viewport: should_reset_viewport,
                    }
                }
                Err(panic_payload) => {
                    let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                        (*s).to_string()
                    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "(non-string panic payload)".to_string()
                    };
                    let message = format!("scene build panicked; showing error plane: {panic_msg}");
                    eprintln!("{message}");
                    app.uniform_scene = None;
                    broadcast_error(app, request_id, "PANIC", message);
                    apply_error_plane(app, render_state);
                    SceneApplyResult {
                        did_rebuild_shader_space: true,
                        texture_filter_override: Some(wgpu::FilterMode::Linear),
                        reset_viewport: should_reset_viewport,
                    }
                }
            }
        }
        ws::SceneUpdate::ParseError {
            message,
            request_id,
        } => {
            eprintln!("[error-plane] scene parse error: {message}");
            broadcast_error(app, request_id, "PARSE_ERROR", message);
            apply_error_plane(app, render_state);
            SceneApplyResult {
                did_rebuild_shader_space: true,
                texture_filter_override: None,
                reset_viewport: false,
            }
        }
    }
}

fn apply_error_plane(app: &mut App, render_state: &egui_wgpu::RenderState) {
    if let Ok(result) = renderer::ShaderSpaceBuilder::new(
        Arc::new(render_state.device.clone()),
        Arc::new(render_state.queue.clone()),
    )
    .build_error(app.resolution)
    {
        app.shader_space = result.shader_space;
        app.resolution = result.resolution;
        app.output_texture_name = result.present_output_texture;
        app.passes = result.pass_bindings;
        app.last_pipeline_signature = None;
    }
}

fn broadcast_error(app: &App, request_id: Option<String>, code: &str, message: String) {
    let msg = protocol::WSMessage {
        msg_type: "error".to_string(),
        timestamp: protocol::now_millis(),
        request_id,
        payload: Some(protocol::ErrorPayload {
            code: code.to_string(),
            message,
        }),
    };
    if let Ok(text) = serde_json::to_string(&msg) {
        app.ws_hub.broadcast(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::types::{
        GraphBinding, GraphBindingKind, GraphField, GraphFieldKind, GraphSchema, Params,
        PassBindings,
    };
    use rust_wgpu_fiber::ResourceName;
    use std::collections::HashMap;

    #[test]
    fn apply_scene_resolution_updates_window_state_without_forcing_resize_by_default() {
        let (next, resize) =
            apply_scene_resolution_to_window_state([1024, 1024], Some([800, 600]), false);
        assert_eq!(next, [800, 600]);
        assert_eq!(resize, None);
    }

    #[test]
    fn apply_scene_resolution_can_request_resize_when_enabled() {
        let (next, resize) =
            apply_scene_resolution_to_window_state([1024, 1024], Some([800, 600]), true);
        assert_eq!(next, [800, 600]);
        assert_eq!(resize, Some([800.0, 600.0]));
    }

    #[test]
    fn apply_scene_resolution_is_noop_when_same_or_missing() {
        let (next, resize) =
            apply_scene_resolution_to_window_state([800, 600], Some([800, 600]), true);
        assert_eq!(next, [800, 600]);
        assert_eq!(resize, None);

        let (next, resize) = apply_scene_resolution_to_window_state([800, 600], None, true);
        assert_eq!(next, [800, 600]);
        assert_eq!(resize, None);
    }

    #[test]
    fn scene_update_mode_selects_uniform_only_when_signature_matches() {
        let sig = [7_u8; 32];
        assert_eq!(
            choose_scene_update_mode(Some(sig), sig),
            SceneUpdateMode::UniformOnly
        );
        assert_eq!(
            choose_scene_update_mode(None, sig),
            SceneUpdateMode::Rebuild
        );
    }

    #[test]
    fn collect_graph_uniform_updates_skips_unchanged_buffers() {
        let scene = crate::dsl::SceneDSL {
            version: "1.0".to_string(),
            metadata: crate::dsl::Metadata {
                name: "scene".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![crate::dsl::Node {
                id: "float1".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::from([("value".to_string(), serde_json::json!(2.0))]),
                inputs: Vec::new(),
                outputs: Vec::new(),
                input_bindings: Vec::new(),
            }],
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
        };

        let schema = GraphSchema {
            fields: vec![GraphField {
                node_id: "float1".to_string(),
                field_name: "node_float1".to_string(),
                kind: GraphFieldKind::F32,
            }],
            size_bytes: 16,
        };
        let bytes = renderer::graph_uniforms::pack_graph_values(&scene, &schema).unwrap();
        let same_hash = renderer::graph_uniforms::hash_bytes(bytes.as_slice());

        let pass = PassBindings {
            pass_id: "passA".to_string(),
            params_buffer: ResourceName::from("params.passA"),
            base_params: Params {
                target_size: [1.0, 1.0],
                geo_size: [1.0, 1.0],
                center: [0.5, 0.5],
                geo_translate: [0.0, 0.0],
                geo_scale: [1.0, 1.0],
                time: 0.0,
                _pad0: 0.0,
                color: [1.0, 1.0, 1.0, 1.0],
            },
            graph_binding: Some(GraphBinding {
                buffer_name: ResourceName::from("params.passA.graph"),
                kind: GraphBindingKind::Uniform,
                schema,
            }),
            last_graph_hash: Some(same_hash),
        };

        let updates = collect_graph_uniform_updates(&scene, &[pass]).unwrap();
        assert!(updates.is_empty());
    }

    #[test]
    fn collect_graph_uniform_updates_emits_when_value_changes() {
        let scene = crate::dsl::SceneDSL {
            version: "1.0".to_string(),
            metadata: crate::dsl::Metadata {
                name: "scene".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![crate::dsl::Node {
                id: "float1".to_string(),
                node_type: "FloatInput".to_string(),
                params: HashMap::from([("value".to_string(), serde_json::json!(3.0))]),
                inputs: Vec::new(),
                outputs: Vec::new(),
                input_bindings: Vec::new(),
            }],
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
        };

        let schema = GraphSchema {
            fields: vec![GraphField {
                node_id: "float1".to_string(),
                field_name: "node_float1".to_string(),
                kind: GraphFieldKind::F32,
            }],
            size_bytes: 16,
        };

        let pass = PassBindings {
            pass_id: "passA".to_string(),
            params_buffer: ResourceName::from("params.passA"),
            base_params: Params {
                target_size: [1.0, 1.0],
                geo_size: [1.0, 1.0],
                center: [0.5, 0.5],
                geo_translate: [0.0, 0.0],
                geo_scale: [1.0, 1.0],
                time: 0.0,
                _pad0: 0.0,
                color: [1.0, 1.0, 1.0, 1.0],
            },
            graph_binding: Some(GraphBinding {
                buffer_name: ResourceName::from("params.passA.graph"),
                kind: GraphBindingKind::Uniform,
                schema,
            }),
            last_graph_hash: None,
        };

        let updates = collect_graph_uniform_updates(&scene, &[pass]).unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].pass_index, 0);
        assert_eq!(updates[0].bytes.len(), 16);
    }
}
