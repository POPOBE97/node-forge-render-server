use rust_wgpu_fiber::eframe::{egui, egui_wgpu};

use crate::{
    app::{canvas, types::App, window_mode},
    ui,
};

use super::{
    commands::{self, AppCommand},
    ingest::IngestPhase,
    interaction_bridge,
};

pub(super) struct PresentPhase {
    pub sidebar_animating: bool,
    pub pan_zoom_animating: bool,
    pub operation_indicator_visible: bool,
}

pub(super) fn run(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    ingest: &IngestPhase,
) -> PresentPhase {
    let now = ingest.frame_time;
    let frame_state = window_mode::update_window_mode_frame(app, now);
    window_mode::maybe_apply_startup_sidebar_sizing(app, ctx);

    if app.shell.resource_snapshot_generation != app.runtime.pipeline_rebuild_count {
        let snapshot = ui::resource_tree::ResourceSnapshot::capture(
            &app.core.shader_space,
            &app.core.passes,
            Some(app.core.output_texture_name.as_str()),
        );
        app.shell.resource_tree_nodes = snapshot.to_tree();
        app.shell.resource_snapshot = Some(snapshot);
        app.shell.resource_snapshot_generation = app.runtime.pipeline_rebuild_count;
    }

    let sidebar_full_w = ui::debug_sidebar::sidebar_width(ctx);
    let sidebar_w = sidebar_full_w * frame_state.sidebar_factor;
    let reference_sidebar_state = app.canvas.reference.ref_image.as_ref().map(|reference| {
        ui::debug_sidebar::ReferenceSidebarState {
            name: reference.name.clone(),
            mode: reference.mode,
            opacity: reference.opacity,
            diff_metric_mode: app.canvas.analysis.diff_metric_mode,
            diff_stats: app.canvas.analysis.diff_stats,
        }
    });
    let analysis_sidebar_state = ui::debug_sidebar::AnalysisSidebarState {
        tab: app.canvas.analysis.analysis_tab,
        clipping: app.canvas.analysis.clipping_settings,
        clip_enabled: app.canvas.analysis.clip_enabled,
    };
    let state_machine_snapshot = interaction_bridge::state_machine_snapshot(app);

    let mut pending_commands = Vec::<AppCommand>::new();
    if sidebar_w > 0.0 {
        let mut sidebar_action = None;
        egui::SidePanel::left("debug_sidebar")
            .exact_width(sidebar_w)
            .resizable(false)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let clip_rect = ui.available_rect_before_wrap();
                let x_offset = -sidebar_full_w * (1.0 - frame_state.sidebar_factor);
                let sidebar_rect = egui::Rect::from_min_size(
                    clip_rect.min + egui::vec2(x_offset, 0.0),
                    egui::vec2(sidebar_full_w, clip_rect.height()),
                );

                sidebar_action = ui::debug_sidebar::show_in_rect(
                    ctx,
                    ui,
                    frame_state.sidebar_factor,
                    frame_state.animation_just_finished_opening,
                    clip_rect,
                    sidebar_rect,
                    app.canvas.analysis.histogram_texture_id,
                    app.canvas.analysis.parade_texture_id,
                    app.canvas.analysis.vectorscope_texture_id,
                    analysis_sidebar_state,
                    reference_sidebar_state.as_ref(),
                    &app.shell.resource_tree_nodes,
                    &mut app.shell.file_tree_state,
                    state_machine_snapshot.as_ref(),
                );
            });

        if let Some(action) = sidebar_action {
            pending_commands.push(commands::from_sidebar_action(action));
        }
    }

    let panel_frame = egui::Frame::default()
        .fill(egui::Color32::BLACK)
        .inner_margin(egui::Margin::same(0));

    egui::CentralPanel::default()
        .frame(panel_frame)
        .show(ctx, |ui| {
            let frame_result = canvas::show(app, ctx, ui, render_state, renderer, frame_state, now);
            pending_commands.extend(frame_result.commands);
        });

    for command in pending_commands {
        if let Err(err) = commands::dispatch(app, ctx, render_state, renderer, now, command) {
            eprintln!("[app] command failed: {err:#}");
        }
    }

    interaction_bridge::broadcast_payloads(app, &ingest.queued_interaction_payloads);
    app.shell.prev_window_mode = frame_state.mode;

    PresentPhase {
        sidebar_animating: app
            .shell
            .animations
            .is_active(window_mode::ANIM_KEY_SIDEBAR_FACTOR),
        pan_zoom_animating: canvas::is_pan_zoom_animating(app),
        operation_indicator_visible: canvas::ops::is_visible(&app.canvas.async_ops),
    }
}
