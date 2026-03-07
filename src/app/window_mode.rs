use rust_wgpu_fiber::eframe::egui;

use crate::ui::animation_manager::{AnimationSpec, Easing};

use super::types::{App, OUTER_MARGIN, SIDEBAR_ANIM_SECS, UiWindowMode};

pub const ANIM_KEY_SIDEBAR_FACTOR: &str = "ui.sidebar.factor";

#[derive(Clone, Copy, Debug)]
pub struct WindowModeFrame {
    pub mode: UiWindowMode,
    pub prev_mode: UiWindowMode,
    pub sidebar_factor: f32,
    pub animation_just_finished_opening: bool,
}

pub fn toggle_canvas_only(app: &mut App, now: f64) {
    app.shell.window_mode = match app.shell.window_mode {
        UiWindowMode::Sidebar => UiWindowMode::CanvasOnly,
        UiWindowMode::CanvasOnly => UiWindowMode::Sidebar,
    };

    let target_sidebar_factor = match app.shell.window_mode {
        UiWindowMode::Sidebar => 1.0,
        UiWindowMode::CanvasOnly => 0.0,
    };
    app.shell.animations.start(
        ANIM_KEY_SIDEBAR_FACTOR,
        AnimationSpec {
            from: app.shell.ui_sidebar_factor,
            to: target_sidebar_factor,
            duration_secs: SIDEBAR_ANIM_SECS,
            easing: Easing::EaseOutCubic,
        },
        now,
    );
}

pub fn update_window_mode_frame(app: &mut App, now: f64) -> WindowModeFrame {
    let prev_mode = app.shell.prev_window_mode;
    let mode = app.shell.window_mode;
    let target_sidebar_factor = match mode {
        UiWindowMode::Sidebar => 1.0,
        UiWindowMode::CanvasOnly => 0.0,
    };

    let was_animating_before_update = app.shell.animations.is_active(ANIM_KEY_SIDEBAR_FACTOR);

    if let Some((value, done)) = app
        .shell
        .animations
        .sample_f32(ANIM_KEY_SIDEBAR_FACTOR, now)
    {
        app.shell.ui_sidebar_factor = value.clamp(0.0, 1.0);
        if done {
            app.shell.ui_sidebar_factor = target_sidebar_factor;
        }
    } else {
        app.shell.ui_sidebar_factor = target_sidebar_factor;
    }

    let animation_just_finished_opening = was_animating_before_update
        && !app.shell.animations.is_active(ANIM_KEY_SIDEBAR_FACTOR)
        && app.shell.ui_sidebar_factor >= 1.0;

    WindowModeFrame {
        mode,
        prev_mode,
        sidebar_factor: app.shell.ui_sidebar_factor,
        animation_just_finished_opening,
    }
}

pub fn maybe_apply_startup_sidebar_sizing(app: &mut App, ctx: &egui::Context) {
    if app.shell.window_mode != UiWindowMode::Sidebar || app.shell.did_startup_sidebar_size {
        return;
    }

    let sidebar_w = crate::ui::debug_sidebar::sidebar_width(ctx);
    let target_width = app.core.window_resolution[0] as f32 + sidebar_w + 2.0 * OUTER_MARGIN;
    let target_height = app.core.window_resolution[1] as f32;
    let mut target = egui::vec2(target_width, target_height);

    if let Some(monitor_size) = ctx.input(|i| i.viewport().monitor_size) {
        target.x = target.x.min(monitor_size.x);
        target.y = target.y.min(monitor_size.y);
    }

    let mut min_size = egui::vec2(sidebar_w + 240.0, 240.0);
    if let Some(monitor_size) = ctx.input(|i| i.viewport().monitor_size) {
        min_size.x = min_size.x.min(monitor_size.x);
        min_size.y = min_size.y.min(monitor_size.y);
    }
    min_size.x = min_size.x.min(target.x);
    min_size.y = min_size.y.min(target.y);

    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(target));
    ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(min_size));
    app.shell.did_startup_sidebar_size = true;
}
