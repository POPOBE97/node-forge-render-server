use std::sync::Arc;

use rust_wgpu_fiber::{
    ResourceName,
    eframe::{egui, egui_wgpu},
};

use crate::{
    app::{
        canvas,
        canvas::actions::CanvasAction,
        display_metrics, matrix_render, scene_runtime, texture_bridge,
        types::{App, ShortwireReferenceImage},
        window_mode,
    },
    protocol::DesignParamPatchPayload,
    ui,
};

#[derive(Clone, Debug)]
pub enum AppCommand {
    PlayStateMachine,
    ForceState(String),
    ClearStateControl,
    Canvas(CanvasAction),
    PickReferenceImage,
    ClearReference,
    StartAndroidReferenceUsb,
    StopAndroidReference,
    OpenPassDebug(String),
    OpenPassDesign(ui::resource_tree::PassDesignTarget),
    SendDesignParamPatch(DesignParamPatchPayload),
    ApplyPassShaderPatch {
        pass_name: String,
        source: String,
        reference_image: Option<ShortwireReferenceImage>,
    },
    ResetPassShaderPatch(String),
    ResetAllPassShaderPatches,
    UpsertDebugArtifact {
        item: crate::dsl::DebugArtifactItem,
        content_text: String,
    },
    ToggleCanvasOnly,
    SetTestMode(crate::app::TestMode),
    ToggleMatrixPool(String),
    SetMatrixMaxRowCols(usize),
    SetMatrixLabelsVisible(bool),
    SetDisplayPpi(f32),
}

pub fn from_sidebar_action(action: ui::debug_sidebar::SidebarAction) -> AppCommand {
    match action {
        ui::debug_sidebar::SidebarAction::PlayStateMachine => AppCommand::PlayStateMachine,
        ui::debug_sidebar::SidebarAction::ForceState(state_id) => AppCommand::ForceState(state_id),
        ui::debug_sidebar::SidebarAction::ClearStateControl => AppCommand::ClearStateControl,
        ui::debug_sidebar::SidebarAction::PreviewTexture(name) => AppCommand::Canvas(
            CanvasAction::SetPreviewTexture(ResourceName::from(name.as_str())),
        ),
        ui::debug_sidebar::SidebarAction::PreviewPass(pass_name) => {
            AppCommand::Canvas(CanvasAction::SetPassCapture(pass_name))
        }
        ui::debug_sidebar::SidebarAction::SetPassCaptureMode(mode) => {
            AppCommand::Canvas(CanvasAction::SetPassCaptureMode(mode))
        }
        ui::debug_sidebar::SidebarAction::OpenPassDebug(pass_name) => {
            AppCommand::OpenPassDebug(pass_name)
        }
        ui::debug_sidebar::SidebarAction::OpenPassDesign(target) => {
            AppCommand::OpenPassDesign(target)
        }
        ui::debug_sidebar::SidebarAction::ClearPreview => {
            AppCommand::Canvas(CanvasAction::ClearPreviewTexture)
        }
        ui::debug_sidebar::SidebarAction::SetReferenceOpacity(opacity) => {
            AppCommand::Canvas(CanvasAction::SetReferenceOpacity(opacity))
        }
        ui::debug_sidebar::SidebarAction::ToggleReferenceMode => {
            AppCommand::Canvas(CanvasAction::ToggleReferenceMode)
        }
        ui::debug_sidebar::SidebarAction::PickReferenceImage => AppCommand::PickReferenceImage,
        ui::debug_sidebar::SidebarAction::RemoveReferenceImage => AppCommand::ClearReference,
        ui::debug_sidebar::SidebarAction::StartAndroidReferenceUsb => {
            AppCommand::StartAndroidReferenceUsb
        }
        ui::debug_sidebar::SidebarAction::StopAndroidReference => AppCommand::StopAndroidReference,
        ui::debug_sidebar::SidebarAction::SetDiffMetricMode(mode) => {
            AppCommand::Canvas(CanvasAction::SetDiffMetricMode(mode))
        }
        ui::debug_sidebar::SidebarAction::SetAnalysisTab(tab) => {
            AppCommand::Canvas(CanvasAction::SetAnalysisTab(tab))
        }
        ui::debug_sidebar::SidebarAction::SetClipEnabled(enabled) => {
            AppCommand::Canvas(CanvasAction::SetClipEnabled(enabled))
        }
        ui::debug_sidebar::SidebarAction::SetClippingShadowThreshold(threshold) => {
            AppCommand::Canvas(CanvasAction::SetClippingShadowThreshold(threshold))
        }
        ui::debug_sidebar::SidebarAction::SetClippingHighlightThreshold(threshold) => {
            AppCommand::Canvas(CanvasAction::SetClippingHighlightThreshold(threshold))
        }
        ui::debug_sidebar::SidebarAction::SetQualifierEnabled(enabled) => {
            AppCommand::Canvas(CanvasAction::SetQualifierEnabled(enabled))
        }
        ui::debug_sidebar::SidebarAction::SetQualifierRange { channel, min, max } => {
            AppCommand::Canvas(CanvasAction::SetQualifierRange { channel, min, max })
        }
        ui::debug_sidebar::SidebarAction::SetTestMode(mode) => AppCommand::SetTestMode(mode),
        ui::debug_sidebar::SidebarAction::ToggleMatrixPool(pool_id) => {
            AppCommand::ToggleMatrixPool(pool_id)
        }
        ui::debug_sidebar::SidebarAction::SetMatrixMaxRowCols(max_cols) => {
            AppCommand::SetMatrixMaxRowCols(max_cols)
        }
        ui::debug_sidebar::SidebarAction::SetMatrixLabelsVisible(visible) => {
            AppCommand::SetMatrixLabelsVisible(visible)
        }
        ui::debug_sidebar::SidebarAction::SetDisplayPpi(ppi) => AppCommand::SetDisplayPpi(ppi),
    }
}

fn start_matrix_rebuild_if_needed(
    app: &mut App,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
) {
    let Some(ref scene) = app.runtime.uniform_scene else {
        return;
    };
    if app.shell.matrix_config.selected_pool_ids.is_empty() {
        return;
    }
    let params = matrix_render::MatrixBuildParams {
        scene,
        config: &app.shell.matrix_config,
        resource_pools: &app.shell.resource_pools,
        device: Arc::new(render_state.device.clone()),
        queue: Arc::new(render_state.queue.clone()),
        adapter: Some(&render_state.adapter),
        asset_store: &app.core.asset_store,
    };
    if let Err(e) =
        matrix_render::start_matrix_rebuild(params, renderer, &mut app.shell.matrix_state)
    {
        eprintln!("[matrix] rebuild failed: {e:#}");
    }
}

pub fn dispatch(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    now: f64,
    command: AppCommand,
) -> anyhow::Result<()> {
    match command {
        AppCommand::PlayStateMachine => {
            scene_runtime::select_state_control(app, crate::app::StateControlSelection::Play)?;
            ctx.request_repaint();
        }
        AppCommand::ForceState(state_id) => {
            scene_runtime::select_state_control(
                app,
                crate::app::StateControlSelection::State(state_id),
            )?;
            ctx.request_repaint();
        }
        AppCommand::ClearStateControl => {
            scene_runtime::clear_state_control(app);
            ctx.request_repaint();
        }
        AppCommand::Canvas(action) => {
            let _ = canvas::reducer::apply_action(app, render_state, renderer, action)?;
        }
        AppCommand::PickReferenceImage => {
            if canvas::pick_reference_image_from_dialog(app, ctx, render_state)? {
                app.shell.android_reference.stop();
            }
        }
        AppCommand::ClearReference => {
            app.shell.android_reference.stop();
            if app.canvas.reference.ref_image.is_some() {
                canvas::clear_reference(app);
            }
        }
        AppCommand::StartAndroidReferenceUsb => {
            app.shell.android_reference.start_usb()?;
            app.canvas.reference.desired_override =
                Some(crate::app::canvas::state::ReferenceDesiredSource::Manual);
            app.canvas.reference.last_attempt_key = None;
            ctx.request_repaint();
        }
        AppCommand::StopAndroidReference => {
            app.shell.android_reference.stop();
            if matches!(
                app.canvas.reference.ref_image.as_ref().map(|r| &r.source),
                Some(crate::app::types::RefImageSource::AndroidScrcpyUsb(_))
            ) {
                canvas::clear_reference(app);
            }
            app.canvas.reference.desired_override =
                Some(crate::app::canvas::state::ReferenceDesiredSource::Manual);
            app.canvas.reference.last_attempt_key = None;
            ctx.request_repaint();
        }
        AppCommand::OpenPassDebug(pass_name) => {
            ui::pass_debug_window::open_pass_debug_window(
                &mut app.shell.pass_debug_windows,
                &app.shell.pass_debug_sources,
                app.shell.pass_debug_sources_revision,
                &app.shell.pass_shader_overrides,
                &app.shell.debug_artifacts,
                pass_name,
            );
        }
        AppCommand::OpenPassDesign(target) => {
            let _ = canvas::reducer::apply_action(
                app,
                render_state,
                renderer,
                CanvasAction::EnterPassDesign(target),
            )?;
        }
        AppCommand::SendDesignParamPatch(payload) => {
            crate::ws::broadcast_design_param_patch(&app.core.ws_hub, payload);
        }
        AppCommand::ApplyPassShaderPatch {
            pass_name,
            source,
            reference_image,
        } => {
            match scene_runtime::apply_pass_shader_patch(
                app,
                render_state,
                &pass_name,
                source.clone(),
            ) {
                Ok(result) => {
                    let status =
                        sync_after_shader_rebuild(app, ctx, render_state, renderer, result, now);
                    let patch_result = ui::pass_debug_window::mark_patch_applied(
                        &mut app.shell.pass_debug_windows,
                        &pass_name,
                        app.shell.pass_debug_sources.get(&pass_name),
                        app.shell.pass_debug_sources_revision,
                        source,
                        status,
                    );
                    if let Some(diff_capture) = patch_result.diff_capture {
                        app.shell.pending_shortwire_diff_capture = Some(diff_capture);
                        app.canvas.invalidation.mark_diff_dirty();
                    }
                    for (item, content_text) in patch_result.artifacts {
                        upsert_debug_artifact(app, item, content_text);
                    }
                    for (item, bytes) in patch_result.binary_artifacts {
                        app.shell
                            .debug_artifacts
                            .upsert_bytes(item.clone(), bytes.clone());
                        crate::ws::broadcast_debug_artifact_binary_upload(
                            &app.core.ws_hub,
                            item,
                            bytes,
                        );
                        app.persist_debug_artifacts_to_source_nforge();
                    }
                    if let Some(reference_image) = reference_image.as_ref() {
                        if let Some(bytes) = app
                            .shell
                            .debug_artifacts
                            .bytes(reference_image.artifact_id.as_str())
                            .map(|bytes| bytes.to_vec())
                        {
                            if let Err(error) = canvas::reference::load_shortwire_reference_image(
                                app,
                                ctx,
                                render_state,
                                reference_image,
                                bytes.as_slice(),
                            ) {
                                eprintln!(
                                    "[shortwire-diff] failed to load stored reference image artifact_id={}: {error:#}",
                                    reference_image.artifact_id,
                                );
                            }
                        } else {
                            eprintln!(
                                "[shortwire-diff] missing stored reference image artifact_id={}",
                                reference_image.artifact_id,
                            );
                        }
                    }
                }
                Err(err) => {
                    ui::pass_debug_window::record_patch_error(
                        &mut app.shell.pass_debug_windows,
                        &pass_name,
                        format!("{err:#}"),
                    );
                }
            }
        }
        AppCommand::ResetPassShaderPatch(pass_name) => {
            match scene_runtime::reset_pass_shader_patch(app, render_state, &pass_name) {
                Ok(result) => {
                    let status =
                        sync_after_shader_rebuild(app, ctx, render_state, renderer, result, now);
                    ui::pass_debug_window::mark_patch_reset(
                        &mut app.shell.pass_debug_windows,
                        &pass_name,
                        app.shell.pass_debug_sources.get(&pass_name),
                        app.shell.pass_debug_sources_revision,
                        status,
                    );
                }
                Err(err) => {
                    ui::pass_debug_window::record_patch_error(
                        &mut app.shell.pass_debug_windows,
                        &pass_name,
                        format!("{err:#}"),
                    );
                }
            }
        }
        AppCommand::ResetAllPassShaderPatches => {
            match scene_runtime::reset_all_pass_shader_patches(app, render_state) {
                Ok(result) => {
                    let status =
                        sync_after_shader_rebuild(app, ctx, render_state, renderer, result, now);
                    ui::pass_debug_window::mark_all_patches_reset(
                        &mut app.shell.pass_debug_windows,
                        &app.shell.pass_debug_sources,
                        app.shell.pass_debug_sources_revision,
                        status,
                    );
                }
                Err(err) => {
                    ui::pass_debug_window::record_all_patch_error(
                        &mut app.shell.pass_debug_windows,
                        format!("{err:#}"),
                    );
                }
            }
        }
        AppCommand::UpsertDebugArtifact { item, content_text } => {
            upsert_debug_artifact(app, item, content_text);
        }
        AppCommand::ToggleCanvasOnly => {
            window_mode::toggle_canvas_only(app, now);
        }
        AppCommand::SetTestMode(mode) => {
            app.shell.test_mode = mode;
            if mode == crate::app::TestMode::Matrix {
                start_matrix_rebuild_if_needed(app, render_state, renderer);
            } else {
                app.shell.matrix_state.clear(renderer);
            }
        }
        AppCommand::ToggleMatrixPool(pool_id) => {
            let selected = &mut app.shell.matrix_config.selected_pool_ids;
            if let Some(pos) = selected.iter().position(|id| *id == pool_id) {
                selected.remove(pos);
            } else if selected.len() < 2 {
                selected.push(pool_id);
            }
            if app.shell.test_mode == crate::app::TestMode::Matrix {
                start_matrix_rebuild_if_needed(app, render_state, renderer);
            }
        }
        AppCommand::SetMatrixMaxRowCols(max_cols) => {
            if app.shell.matrix_config.max_row_cols != max_cols {
                app.shell.matrix_config.max_row_cols = max_cols;
                matrix_render::relayout_matrix(
                    &app.shell.matrix_config,
                    &mut app.shell.matrix_state,
                );
            }
        }
        AppCommand::SetMatrixLabelsVisible(visible) => {
            if app.shell.matrix_config.show_labels != visible {
                app.shell.matrix_config.show_labels = visible;
                matrix_render::relayout_matrix(
                    &app.shell.matrix_config,
                    &mut app.shell.matrix_state,
                );
            }
        }
        AppCommand::SetDisplayPpi(ppi) => {
            let current_display_metrics = display_metrics::current_display_metrics(ctx);
            let _ = canvas::reducer::apply_action(
                app,
                render_state,
                renderer,
                CanvasAction::SetDisplayPpi {
                    ppi,
                    current_display_ppi: current_display_metrics.display_ppi,
                    pixels_per_point: current_display_metrics.pixels_per_point,
                },
            )?;
        }
    }

    Ok(())
}

fn sync_after_shader_rebuild(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    result: scene_runtime::SceneApplyResult,
    now: f64,
) -> String {
    if !result.did_rebuild_shader_space {
        return "No shader rebuild needed".to_string();
    }
    render_current_shader_space(app, now);
    let filter = result
        .texture_filter_override
        .unwrap_or(app.canvas.display.texture_filter);
    let texture_name = app.core.output_texture_name.clone();
    texture_bridge::sync_output_texture(app, render_state, renderer, &texture_name, filter);
    let _ = canvas::display::sync_preview_source(app, render_state, renderer);
    app.canvas.invalidation.preview_source_changed();
    app.runtime.scene_redraw_pending = false;
    ctx.request_repaint();
    match (
        result.previous_output_hash,
        scene_runtime::current_output_hash(app),
    ) {
        (Some(before), Some(after)) if before != after => {
            "Shader rebuild applied; output texture changed".to_string()
        }
        (Some(_), Some(_)) => "Shader rebuild applied; output texture unchanged".to_string(),
        _ => "Shader rebuild applied; output change not measured".to_string(),
    }
}

fn upsert_debug_artifact(app: &mut App, item: crate::dsl::DebugArtifactItem, content_text: String) {
    app.shell
        .debug_artifacts
        .upsert(item.clone(), Some(content_text.clone()));
    app.persist_debug_artifacts_to_source_nforge();
    crate::ws::broadcast_debug_artifact_upsert(&app.core.ws_hub, item, Some(content_text));
}

fn render_current_shader_space(app: &mut App, now: f64) {
    let t = app.runtime.time_value_secs;
    for pass in &mut app.core.passes {
        let mut params = pass.base_params;
        params.time = t;
        let _ = crate::renderer::update_pass_params(&app.core.shader_space, pass, &params);
    }
    let profile = canvas::draw_capture::render_profiled(app, false);
    app.runtime.latest_render_profile = Some(profile);
    app.runtime
        .render_texture_fps_tracker
        .record_scene_redraw(now);
}

#[cfg(test)]
mod tests {
    use super::{AppCommand, from_sidebar_action};
    use crate::{
        app::{AnalysisTab, DiffMetricMode, canvas::actions::CanvasAction},
        ui::debug_sidebar::SidebarAction,
    };
    use rust_wgpu_fiber::shader_space::PassCaptureMode;

    #[test]
    fn sidebar_state_controls_map_to_local_commands() {
        let play = from_sidebar_action(SidebarAction::PlayStateMachine);
        let state = from_sidebar_action(SidebarAction::ForceState("entry".to_string()));
        let clear = from_sidebar_action(SidebarAction::ClearStateControl);
        assert!(matches!(play, AppCommand::PlayStateMachine));
        assert!(matches!(state, AppCommand::ForceState(state_id) if state_id == "entry"));
        assert!(matches!(clear, AppCommand::ClearStateControl));
    }

    #[test]
    fn sidebar_texture_preview_maps_to_canvas_command() {
        let command = from_sidebar_action(SidebarAction::PreviewTexture("foo.bar".to_string()));
        assert!(matches!(command, AppCommand::Canvas(_)));
    }

    #[test]
    fn sidebar_pass_capture_maps_to_canvas_commands() {
        let preview = from_sidebar_action(SidebarAction::PreviewPass("draw.pass".to_string()));
        assert!(matches!(
            preview,
            AppCommand::Canvas(CanvasAction::SetPassCapture(pass_name))
                if pass_name == "draw.pass"
        ));

        let mode = from_sidebar_action(SidebarAction::SetPassCaptureMode(PassCaptureMode::After));
        assert!(matches!(
            mode,
            AppCommand::Canvas(CanvasAction::SetPassCaptureMode(PassCaptureMode::After))
        ));
    }

    #[test]
    fn sidebar_reference_picker_maps_to_app_command() {
        let command = from_sidebar_action(SidebarAction::PickReferenceImage);
        assert!(matches!(command, AppCommand::PickReferenceImage));
    }

    #[test]
    fn sidebar_reference_remove_maps_to_clear_reference_command() {
        let command = from_sidebar_action(SidebarAction::RemoveReferenceImage);
        assert!(matches!(command, AppCommand::ClearReference));
    }

    #[test]
    fn sidebar_android_reference_maps_to_app_command() {
        let start = from_sidebar_action(SidebarAction::StartAndroidReferenceUsb);
        let stop = from_sidebar_action(SidebarAction::StopAndroidReference);
        assert!(matches!(start, AppCommand::StartAndroidReferenceUsb));
        assert!(matches!(stop, AppCommand::StopAndroidReference));
    }

    #[test]
    fn sidebar_pass_debug_maps_to_app_command() {
        let command = from_sidebar_action(SidebarAction::OpenPassDebug("node_2.pass".to_string()));
        assert!(
            matches!(command, AppCommand::OpenPassDebug(pass_name) if pass_name == "node_2.pass")
        );
    }

    #[test]
    fn sidebar_analysis_controls_map_to_canvas_commands() {
        let analysis = from_sidebar_action(SidebarAction::SetAnalysisTab(AnalysisTab::Parade));
        let diff = from_sidebar_action(SidebarAction::SetDiffMetricMode(DiffMetricMode::SE));
        assert!(matches!(analysis, AppCommand::Canvas(_)));
        assert!(matches!(diff, AppCommand::Canvas(_)));
    }

    #[test]
    fn sidebar_display_ppi_maps_to_app_command() {
        let command = from_sidebar_action(SidebarAction::SetDisplayPpi(264.0));
        assert!(
            matches!(command, AppCommand::SetDisplayPpi(ppi) if (ppi - 264.0).abs() < f32::EPSILON)
        );
    }
}
