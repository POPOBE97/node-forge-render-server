use std::sync::Arc;

use rust_wgpu_fiber::{
    ResourceName,
    eframe::{egui, egui_wgpu},
};

use crate::{
    app::{
        canvas, canvas::actions::CanvasAction, display_metrics, matrix_render, types::App,
        window_mode,
    },
    ui,
};

#[derive(Clone, Debug)]
pub enum AppCommand {
    Canvas(CanvasAction),
    PickReferenceImage,
    ClearReference,
    ToggleCanvasOnly,
    SetTestMode(crate::app::TestMode),
    ToggleMatrixPool(String),
    SetMatrixMaxRowCols(usize),
    SetMatrixLabelsVisible(bool),
    SetDisplayPpi(f32),
}

pub fn from_sidebar_action(action: ui::debug_sidebar::SidebarAction) -> AppCommand {
    match action {
        ui::debug_sidebar::SidebarAction::PreviewTexture(name) => AppCommand::Canvas(
            CanvasAction::SetPreviewTexture(ResourceName::from(name.as_str())),
        ),
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

fn rebuild_matrix_if_needed(
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
        matrix_render::rebuild_matrix(params, render_state, renderer, &mut app.shell.matrix_state)
    {
        eprintln!("[matrix] rebuild failed: {e:#}");
    }
    if app.canvas.display.hdr_preview_clamp_enabled {
        matrix_render::sync_matrix_hdr_clamp(
            &mut app.shell.matrix_state,
            render_state,
            renderer,
            true,
            app.canvas.display.texture_filter,
        );
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
        AppCommand::Canvas(action) => {
            let _ = canvas::reducer::apply_action(app, render_state, renderer, action)?;
        }
        AppCommand::PickReferenceImage => {
            let _ = canvas::pick_reference_image_from_dialog(app, ctx, render_state)?;
        }
        AppCommand::ClearReference => {
            if app.canvas.reference.ref_image.is_some() {
                canvas::clear_reference(app);
            }
        }
        AppCommand::ToggleCanvasOnly => {
            window_mode::toggle_canvas_only(app, now);
        }
        AppCommand::SetTestMode(mode) => {
            app.shell.test_mode = mode;
            if mode == crate::app::TestMode::Matrix {
                rebuild_matrix_if_needed(app, render_state, renderer);
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
                rebuild_matrix_if_needed(app, render_state, renderer);
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
        AppCommand::SetDisplayPpi(target_ppi) => {
            let current_display_metrics = display_metrics::current_display_metrics(ctx);
            let _ = canvas::reducer::apply_action(
                app,
                render_state,
                renderer,
                CanvasAction::SetDisplayPpi {
                    target_ppi,
                    current_display_ppi: current_display_metrics.display_ppi,
                    pixels_per_point: current_display_metrics.pixels_per_point,
                },
            )?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AppCommand, from_sidebar_action};
    use crate::{
        app::{AnalysisTab, DiffMetricMode},
        ui::debug_sidebar::SidebarAction,
    };

    #[test]
    fn sidebar_texture_preview_maps_to_canvas_command() {
        let command = from_sidebar_action(SidebarAction::PreviewTexture("foo.bar".to_string()));
        assert!(matches!(command, AppCommand::Canvas(_)));
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
