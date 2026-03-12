use rust_wgpu_fiber::eframe::{egui, egui_wgpu};

use crate::{
    app::{canvas, scene_runtime, types::App, window_mode},
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

    let mut pending_commands = Vec::<AppCommand>::new();
    let mut sidebar_result = ui::debug_sidebar::SidebarResult::default();

    // ── Bottom timeline panel ────────────────────────────────────────────
    // Rendered before sidebar and central panel so egui reserves space at
    // the bottom first. The hover result feeds into the canvas render for
    // live preview.
    let mut timeline_hover: Option<ui::debug_sidebar::TimelineHover> = None;
    if let Some(ref buf) = app.runtime.timeline_buffer {
        egui::TopBottomPanel::bottom("timeline_panel")
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(crate::color::lab(7.78201, -0.000_014_901_2, 0.0))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(32))),
            )
            .show(ctx, |ui| {
                let interaction = ui::timeline_panel::show_timeline(ui, buf);
                if let Some(idx) = interaction.hovered_frame_index {
                    timeline_hover = Some(ui::debug_sidebar::TimelineHover { frame_index: idx });
                }
            });
    }

    if sidebar_w > 0.0 {
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

                sidebar_result = ui::debug_sidebar::show_in_rect(
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
                );
            });

        if let Some(action) = sidebar_result.action.take() {
            pending_commands.push(commands::from_sidebar_action(action));
        }
    }

    // ── Timeline hover preview ─────────────────────────────────────────
    //
    // When the user hovers a frame in the timeline panel we want the canvas
    // to show that frame's parameter state immediately.  The animation
    // session keeps advancing under the hood (advance phase already ran),
    // but we override the GPU texture here so the canvas displays the
    // hovered frame.
    //
    // On hover-exit we snap back to the pre-hover snapshot.  During
    // playback `last_live_overrides` is continuously updated by advance,
    // so we prefer that (it represents the latest head).  When stopped we
    // fall back to the snapshot we captured when hover started.
    let hovering_now = timeline_hover.is_some();

    if let Some(ref hover) = timeline_hover {
        // On the first hover frame, snapshot the current uniform_scene
        // values for every override key the timeline tracks.
        if !app.runtime.timeline_preview_was_active {
            if let Some(ref buf) = app.runtime.timeline_buffer {
                if let Some(ref uniform_scene) = app.runtime.uniform_scene {
                    let mut snap = std::collections::HashMap::new();
                    for frame in buf.frames().iter().rev().take(1) {
                        for key in frame.active_overrides.keys() {
                            if snap.contains_key(key) {
                                continue;
                            }
                            if let Some(node) =
                                uniform_scene.nodes.iter().find(|n| n.id == key.node_id)
                                && let Some(val) = node.params.get(&key.param_name)
                            {
                                snap.insert(key.clone(), val.clone());
                            }
                        }
                    }
                    app.runtime.timeline_pre_hover_overrides = Some(snap);
                }
            }
        }

        if let Some(frame) = app
            .runtime
            .timeline_buffer
            .as_ref()
            .and_then(|buf| buf.frame_at(hover.frame_index))
        {
            let hovered_overrides = frame.active_overrides.clone();
            if let Some(ref mut uniform_scene) = app.runtime.uniform_scene {
                crate::state_machine::apply_overrides(uniform_scene, &hovered_overrides);
            }
            if let Some(ref uniform_scene) = app.runtime.uniform_scene {
                let _ = scene_runtime::apply_graph_uniform_updates_parts(
                    &mut app.core.passes,
                    &mut app.core.shader_space,
                    uniform_scene,
                );
            }
            for pass in &mut app.core.passes {
                let mut params = pass.base_params;
                params.time = app.runtime.time_value_secs;
                let _ = crate::renderer::update_pass_params(&app.core.shader_space, pass, &params);
            }
            app.core.shader_space.render();
            app.runtime.scene_redraw_pending = false;
        }
    } else if app.runtime.timeline_preview_was_active {
        // Hover just exited — snap back.
        // Prefer last_live_overrides (updated every advance tick while
        // playing) so we land on the true head.  Fall back to the
        // pre-hover snapshot for the stopped case.
        let restore = app
            .runtime
            .last_live_overrides
            .clone()
            .or_else(|| app.runtime.timeline_pre_hover_overrides.take());
        if let Some(ref overrides) = restore {
            if let Some(ref mut uniform_scene) = app.runtime.uniform_scene {
                crate::state_machine::apply_overrides(uniform_scene, overrides);
            }
            if let Some(ref uniform_scene) = app.runtime.uniform_scene {
                let _ = scene_runtime::apply_graph_uniform_updates_parts(
                    &mut app.core.passes,
                    &mut app.core.shader_space,
                    uniform_scene,
                );
            }
            for pass in &mut app.core.passes {
                let mut params = pass.base_params;
                params.time = app.runtime.time_value_secs;
                let _ = crate::renderer::update_pass_params(&app.core.shader_space, pass, &params);
            }
            app.core.shader_space.render();
            app.runtime.scene_redraw_pending = false;
        }
        app.runtime.timeline_pre_hover_overrides = None;
    }
    app.runtime.timeline_preview_was_active = hovering_now;

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
