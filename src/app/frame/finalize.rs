use rust_wgpu_fiber::eframe::egui;

use crate::app::types::App;

use super::{advance::AdvancePhase, present::PresentPhase};

fn should_request_immediate_repaint(
    time_driven_scene: bool,
    sidebar_animating: bool,
    pan_zoom_animating: bool,
    operation_indicator_visible: bool,
    capture_redraw_active: bool,
) -> bool {
    time_driven_scene
        || sidebar_animating
        || pan_zoom_animating
        || operation_indicator_visible
        || capture_redraw_active
}

pub(super) fn run(app: &App, ctx: &egui::Context, advance: &AdvancePhase, present: &PresentPhase) {
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));

    let title = if let Some(sampled) = app.canvas.viewport.last_sampled {
        format!(
            "Node Forge Render Server - x={} y={} rgba=({:.3}, {:.3}, {:.3}, {:.3})",
            sampled.x,
            sampled.y,
            sampled.rgba[0],
            sampled.rgba[1],
            sampled.rgba[2],
            sampled.rgba[3]
        )
    } else {
        "Node Forge Render Server".to_string()
    };
    ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

    if should_request_immediate_repaint(
        advance.time_driven_scene || advance.animation_session_active,
        present.sidebar_animating,
        present.pan_zoom_animating,
        present.operation_indicator_visible,
        app.runtime.capture_redraw_active,
    ) {
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::should_request_immediate_repaint;

    #[test]
    fn repaint_policy_requests_immediate_for_time_driven_scene() {
        assert!(should_request_immediate_repaint(
            true, false, false, false, false
        ));
    }

    #[test]
    fn repaint_policy_requests_immediate_for_any_active_animation() {
        assert!(should_request_immediate_repaint(
            false, true, false, false, false
        ));
        assert!(should_request_immediate_repaint(
            false, false, true, false, false
        ));
    }

    #[test]
    fn repaint_policy_requests_immediate_when_operation_indicator_visible() {
        assert!(should_request_immediate_repaint(
            false, false, false, true, false
        ));
    }

    #[test]
    fn repaint_policy_requests_immediate_when_capture_redraw_active() {
        assert!(should_request_immediate_repaint(
            false, false, false, false, true
        ));
    }

    #[test]
    fn repaint_policy_skips_immediate_when_capture_inactive_and_other_triggers_inactive() {
        assert!(!should_request_immediate_repaint(
            false, false, false, false, false
        ));
    }
}
