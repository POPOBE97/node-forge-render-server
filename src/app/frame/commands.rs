use rust_wgpu_fiber::{
    ResourceName,
    eframe::{egui, egui_wgpu},
};

use crate::{
    app::{canvas, canvas::actions::CanvasAction, types::App, window_mode},
    ui,
};

#[derive(Clone, Debug)]
pub enum AppCommand {
    Canvas(CanvasAction),
    PickReferenceImage,
    ClearReference,
    ToggleCanvasOnly,
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

}
