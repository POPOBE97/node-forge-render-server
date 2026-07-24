use rust_wgpu_fiber::eframe::egui;

use super::types::{App, StateControlSelection};

fn animation_canvas_input_active_state(
    state_control_selection: Option<&StateControlSelection>,
    has_animation_session: bool,
    design_active: bool,
) -> bool {
    matches!(state_control_selection, Some(StateControlSelection::Play))
        && has_animation_session
        && !design_active
}

pub(super) fn animation_canvas_input_active(app: &App) -> bool {
    animation_canvas_input_active_state(
        app.runtime.state_control_selection.as_ref(),
        app.runtime.animation_session.is_some(),
        app.canvas.design.active.is_some(),
    )
}

fn pointer_over_debug_sidebar(
    pointer_pos: Option<egui::Pos2>,
    debug_sidebar_rect: Option<egui::Rect>,
) -> bool {
    pointer_pos
        .is_some_and(|position| debug_sidebar_rect.is_some_and(|rect| rect.contains(position)))
}

fn debug_shortcut_scope_enabled_state(
    animation_input_active: bool,
    pointer_pos: Option<egui::Pos2>,
    debug_sidebar_rect: Option<egui::Rect>,
) -> bool {
    !animation_input_active || pointer_over_debug_sidebar(pointer_pos, debug_sidebar_rect)
}

fn canvas_keyboard_events_enabled_state(
    animation_input_active: bool,
    pointer_pos: Option<egui::Pos2>,
    debug_sidebar_rect: Option<egui::Rect>,
) -> bool {
    !animation_input_active || !pointer_over_debug_sidebar(pointer_pos, debug_sidebar_rect)
}

pub(super) fn debug_shortcuts_enabled(app: &App, ctx: &egui::Context) -> bool {
    if ctx.text_edit_focused() {
        return false;
    }
    debug_shortcut_scope_enabled_state(
        animation_canvas_input_active(app),
        ctx.input(|input| input.pointer.hover_pos()),
        app.canvas.interactions.last_debug_sidebar_rect,
    )
}

pub(super) fn canvas_keyboard_events_enabled(app: &App, pointer_pos: Option<egui::Pos2>) -> bool {
    canvas_keyboard_events_enabled_state(
        animation_canvas_input_active(app),
        pointer_pos,
        app.canvas.interactions.last_debug_sidebar_rect,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        animation_canvas_input_active_state, canvas_keyboard_events_enabled_state,
        debug_shortcut_scope_enabled_state,
    };
    use crate::app::StateControlSelection;
    use rust_wgpu_fiber::eframe::egui;

    #[test]
    fn animation_canvas_input_is_only_captured_while_playing() {
        let play = StateControlSelection::Play;
        let pinned = StateControlSelection::State("visible".to_string());

        assert!(!animation_canvas_input_active_state(None, false, false));
        assert!(!animation_canvas_input_active_state(
            Some(&play),
            false,
            false
        ));
        assert!(!animation_canvas_input_active_state(None, true, false));
        assert!(animation_canvas_input_active_state(
            Some(&play),
            true,
            false
        ));
        assert!(!animation_canvas_input_active_state(
            Some(&pinned),
            true,
            false
        ));
        assert!(!animation_canvas_input_active_state(
            Some(&play),
            true,
            true
        ));
    }

    #[test]
    fn shortcut_and_scene_keyboard_scopes_follow_the_active_region() {
        let sidebar = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 800.0));
        let over_sidebar = Some(egui::pos2(100.0, 100.0));
        let over_canvas = Some(egui::pos2(500.0, 100.0));

        assert!(debug_shortcut_scope_enabled_state(
            false,
            over_canvas,
            Some(sidebar),
        ));
        assert!(canvas_keyboard_events_enabled_state(
            false,
            over_sidebar,
            Some(sidebar),
        ));

        assert!(!debug_shortcut_scope_enabled_state(
            true,
            over_canvas,
            Some(sidebar),
        ));
        assert!(canvas_keyboard_events_enabled_state(
            true,
            over_canvas,
            Some(sidebar),
        ));

        assert!(debug_shortcut_scope_enabled_state(
            true,
            over_sidebar,
            Some(sidebar),
        ));
        assert!(!canvas_keyboard_events_enabled_state(
            true,
            over_sidebar,
            Some(sidebar),
        ));
    }
}
