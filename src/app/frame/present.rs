use rust_wgpu_fiber::eframe::{egui, egui_wgpu};

use crate::{
    app::{canvas, display_metrics, input_scope, scene_runtime, types::App, window_mode},
    ui::{self, timeline_panel::TimelineInteraction},
};

use super::{
    commands::{self, AppCommand},
    ingest::IngestPhase,
};

const TIMELINE_PANEL_MIN_HEIGHT: f32 = 72.0;
const TIMELINE_PANEL_INITIAL_MAX_HEIGHT: f32 = 240.0;
const TIMELINE_PANEL_WINDOW_FRACTION: f32 = 0.6;

pub(super) struct PresentPhase {
    pub sidebar_animating: bool,
    pub pan_zoom_animating: bool,
    pub operation_indicator_visible: bool,
}

fn timeline_toggle_shortcut_requested(
    text_edit_focused: bool,
    command_pressed: bool,
    j_pressed: bool,
) -> bool {
    !text_edit_focused && command_pressed && j_pressed
}

fn timeline_toggle_shortcut_pressed(ctx: &egui::Context) -> bool {
    let text_edit_focused = ctx.text_edit_focused();
    let (command_pressed, j_pressed) =
        ctx.input(|input| (input.modifiers.command, input.key_pressed(egui::Key::J)));
    timeline_toggle_shortcut_requested(text_edit_focused, command_pressed, j_pressed)
}

fn update_timeline_review_state(
    review: &mut crate::app::types::TimelineReviewState,
    interaction: TimelineInteraction,
    latest_frame_id: Option<crate::animation::TimelineFrameId>,
    is_retained: impl Fn(crate::animation::TimelineFrameId) -> bool,
) -> bool {
    review.anchor_frame_id = review.anchor_frame_id.filter(|id| is_retained(*id));
    review.held_frame_id = review.held_frame_id.filter(|id| is_retained(*id));
    review.suppressed_hover_frame_id = review
        .suppressed_hover_frame_id
        .filter(|id| is_retained(*id));
    let hovered_frame_id = interaction.hovered_frame_id.filter(|id| is_retained(*id));

    if interaction.delete_anchor && review.anchor_frame_id.is_some() {
        review.anchor_frame_id = None;
        review.hovered_frame_id = None;
        review.held_frame_id = latest_frame_id;
        review.suppressed_hover_frame_id = hovered_frame_id;
        return false;
    }

    if let Some(anchor_frame_id) = interaction
        .set_anchor_frame_id
        .filter(|id| is_retained(*id))
    {
        review.anchor_frame_id = Some(anchor_frame_id);
        review.held_frame_id = None;
        review.suppressed_hover_frame_id = None;
        review.hovered_frame_id = hovered_frame_id;
        return true;
    }

    if hovered_frame_id.is_none() {
        review.hovered_frame_id = None;
        review.suppressed_hover_frame_id = None;
    } else if hovered_frame_id == review.suppressed_hover_frame_id {
        review.hovered_frame_id = None;
    } else {
        review.suppressed_hover_frame_id = None;
        review.hovered_frame_id = hovered_frame_id;
    }
    false
}

fn apply_timeline_interaction(app: &mut App, interaction: TimelineInteraction) {
    let retained_ids = app
        .runtime
        .timeline_buffer
        .as_ref()
        .map(|buffer| {
            buffer
                .frames()
                .iter()
                .map(|frame| frame.id)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();
    let latest_frame_id = app
        .runtime
        .timeline_buffer
        .as_ref()
        .and_then(crate::animation::TimelineBuffer::latest_frame_id);
    let pause_requested = update_timeline_review_state(
        &mut app.runtime.timeline_review,
        interaction,
        latest_frame_id,
        |id| retained_ids.contains(&id),
    );
    if pause_requested {
        app.runtime.time_updates_enabled = false;
    }
}

fn render_timeline_snapshot(app: &mut App, snapshot: &crate::animation::TimelineRenderSnapshot) {
    scene_runtime::apply_state_machine_overrides(app, &snapshot.active_overrides);
    for pass in &mut app.core.passes {
        let mut params = pass.base_params;
        params.time = snapshot.scene_time_secs as f32;
        let _ = crate::renderer::update_pass_params(&app.core.shader_space, pass, &params);
    }
    let profile = canvas::draw_capture::render_profiled(app, false);
    app.runtime.latest_render_profile = Some(profile);
    app.runtime.scene_redraw_pending = false;
}

fn apply_timeline_review_frame(app: &mut App) {
    let display_frame_id = app.runtime.timeline_review.display_frame_id();
    let recorded_snapshot = display_frame_id.and_then(|frame_id| {
        app.runtime
            .timeline_buffer
            .as_ref()
            .and_then(|buffer| buffer.frame_by_id(frame_id))
            .map(crate::animation::TimelineFrame::render_snapshot)
    });

    if let Some(snapshot) = recorded_snapshot {
        render_timeline_snapshot(app, &snapshot);
        app.runtime.timeline_review.preview_applied_last_frame = true;
    } else if app.runtime.timeline_review.preview_applied_last_frame {
        if let Some(snapshot) = app.runtime.last_live_snapshot.clone() {
            render_timeline_snapshot(app, &snapshot);
        }
        app.runtime.timeline_review.preview_applied_last_frame = false;
    }
}

pub(super) fn run(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    ingest: &IngestPhase,
) -> PresentPhase {
    let now = ingest.frame_time;
    let frame_state = window_mode::update_window_mode_frame(app, now);
    window_mode::maybe_apply_startup_sidebar_sizing(app, ctx);
    if input_scope::debug_shortcuts_enabled(app, ctx) && timeline_toggle_shortcut_pressed(ctx) {
        app.shell.timeline_visible = !app.shell.timeline_visible;
    }

    if app.shell.resource_snapshot_generation != app.runtime.pipeline_rebuild_count {
        let snapshot = ui::resource_tree::ResourceSnapshot::capture(
            &app.core.shader_space,
            &app.core.passes,
            Some(app.core.output_texture_name.as_str()),
            app.runtime.uniform_scene.as_ref(),
        );
        app.shell.resource_tree_nodes = snapshot.to_tree();
        app.shell.resource_snapshot = Some(snapshot);
        app.shell.resource_snapshot_generation = app.runtime.pipeline_rebuild_count;
    }
    if app.core.ws_hub.client_count() == 0 {
        app.shell.resource_snapshot_broadcast_generation = u64::MAX;
    } else if app.shell.resource_snapshot_broadcast_generation
        != app.shell.resource_snapshot_generation
        && let (Some(snapshot), Some(scene)) = (
            app.shell.resource_snapshot.as_ref(),
            app.runtime.uniform_scene.as_ref(),
        )
    {
        crate::ws::broadcast_pass_target_sizes(&app.core.ws_hub, snapshot, scene);
        app.shell.resource_snapshot_broadcast_generation = app.shell.resource_snapshot_generation;
    }

    let sidebar_full_w = ui::debug_sidebar::sidebar_width(ctx);
    let sidebar_w = sidebar_full_w * frame_state.sidebar_factor;
    let android_reference_status = app.shell.android_reference.status();
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
        qualifier: app.canvas.analysis.qualifier_settings,
        qualifier_enabled: app.canvas.analysis.qualifier_enabled,
    };
    let current_display_metrics = display_metrics::current_display_metrics(ctx);
    if app.canvas.viewport.display_ppi.is_none() {
        app.canvas.viewport.display_ppi = current_display_metrics
            .display_ppi
            .map(display_metrics::clamp_display_ppi);
    }
    let display_sidebar_state = ui::debug_sidebar::DisplaySidebarState {
        ppi: app.canvas.viewport.effective_display_ppi(),
    };
    let pass_capture_sidebar_state = ui::debug_sidebar::PassCaptureSidebarState {
        mode: app.canvas.display.pass_capture_mode,
        enabled: app.canvas.display.pass_capture.is_some(),
    };
    let state_sidebar_items = app
        .runtime
        .animation_session
        .as_ref()
        .map(|session| {
            session
                .runtime()
                .definition()
                .states
                .iter()
                .filter_map(|state| {
                    use crate::state_machine::types::AnimationStateType;
                    let kind = match state.resolved_type() {
                        AnimationStateType::AnimationState => {
                            ui::debug_sidebar::StateSidebarKind::State
                        }
                        AnimationStateType::EntryState => {
                            ui::debug_sidebar::StateSidebarKind::Entry
                        }
                        AnimationStateType::AnyState => ui::debug_sidebar::StateSidebarKind::Any,
                        AnimationStateType::ExitState => ui::debug_sidebar::StateSidebarKind::Exit,
                        AnimationStateType::DerivationNode => return None,
                    };
                    Some(ui::debug_sidebar::StateSidebarItem {
                        id: state.id.clone(),
                        name: if state.name.trim().is_empty() {
                            state.id.clone()
                        } else {
                            state.name.clone()
                        },
                        kind,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut pending_commands = Vec::<AppCommand>::new();
    let mut sidebar_result = ui::debug_sidebar::SidebarResult::default();

    // ── Bottom timeline panel ────────────────────────────────────────────
    // Rendered before sidebar and central panel so egui reserves space at
    // the bottom first. The hover result feeds into the canvas render for
    // live preview.
    let mut timeline_interaction = TimelineInteraction::default();
    if app.shell.timeline_visible
        && let Some(ref buf) = app.runtime.timeline_buffer
    {
        let anchor_frame_id = app.runtime.timeline_review.anchor_frame_id;
        let natural_height = (18.0 + buf.tracked_keys.len().max(1) as f32 * 20.0 + 16.0)
            .clamp(TIMELINE_PANEL_MIN_HEIGHT, TIMELINE_PANEL_INITIAL_MAX_HEIGHT);
        let maximum_height =
            (ui.available_height() * TIMELINE_PANEL_WINDOW_FRACTION).max(TIMELINE_PANEL_MIN_HEIGHT);
        egui::Panel::bottom("timeline_panel")
            .resizable(true)
            .default_size(natural_height)
            .min_size(TIMELINE_PANEL_MIN_HEIGHT)
            .max_size(maximum_height)
            .frame(
                egui::Frame::NONE
                    .fill(crate::color::lab(7.78201, -0.000_014_901_2, 0.0))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_gray(32))),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        timeline_interaction =
                            ui::timeline_panel::show_timeline(ui, buf, anchor_frame_id);
                    });
            });
    }

    app.canvas.interactions.last_debug_sidebar_rect = None;
    if sidebar_w > 0.0 {
        egui::Panel::left("debug_sidebar")
            .exact_size(sidebar_w)
            .resizable(false)
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {
                let clip_rect = ui.available_rect_before_wrap();
                app.canvas.interactions.last_debug_sidebar_rect = Some(clip_rect);
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
                    display_sidebar_state,
                    android_reference_status.clone(),
                    reference_sidebar_state.as_ref(),
                    ui::debug_sidebar::StateSidebarState {
                        items: &state_sidebar_items,
                        selection: app.runtime.state_control_selection.as_ref(),
                        playback_enabled: app.runtime.animation_session.is_some(),
                        playback_rate: app.runtime.playback_rate,
                        timeline_review_paused: app
                            .runtime
                            .timeline_review
                            .anchor_frame_id
                            .is_some()
                            || app.runtime.timeline_review.held_frame_id.is_some(),
                    },
                    ui::debug_sidebar::TestModeSidebarState {
                        mode: app.shell.test_mode,
                        resource_pools: &app.shell.resource_pools,
                        selected_pool_ids: &app.shell.matrix_config.selected_pool_ids,
                        max_row_cols: app.shell.matrix_config.max_row_cols,
                        show_labels: app.shell.matrix_config.show_labels,
                    },
                    pass_capture_sidebar_state,
                    &app.shell.resource_tree_nodes,
                    &mut app.shell.file_tree_state,
                );
            });

        if let Some(action) = sidebar_result.action.take() {
            pending_commands.push(commands::from_sidebar_action(action));
        }
    }

    apply_timeline_interaction(app, timeline_interaction);
    apply_timeline_review_frame(app);

    let panel_frame = egui::Frame::default()
        .fill(egui::Color32::BLACK)
        .inner_margin(egui::Margin::same(0));

    egui::CentralPanel::default()
        .frame(panel_frame)
        .show_inside(ui, |ui| {
            let frame_result = canvas::show(app, ctx, ui, render_state, renderer, frame_state, now);
            pending_commands.extend(frame_result.commands);
        });

    for command in pending_commands {
        if let Err(err) = commands::dispatch(app, ctx, render_state, renderer, now, command) {
            eprintln!("[app] command failed: {err:#}");
        }
    }

    for action in ui::pass_debug_window::show_pass_debug_windows(
        ctx,
        &mut app.shell.pass_debug_windows,
        &app.shell.pass_debug_sources,
        app.shell.pass_debug_sources_revision,
        &app.shell.pass_shader_overrides,
        &app.shell.debug_artifacts,
    ) {
        let command = match action {
            ui::pass_debug_window::PassDebugWindowAction::ApplyPatch {
                pass_name,
                source,
                reference_image,
            } => AppCommand::ApplyPassShaderPatch {
                pass_name,
                source,
                reference_image,
            },
            ui::pass_debug_window::PassDebugWindowAction::ResetPatch { pass_name } => {
                AppCommand::ResetPassShaderPatch(pass_name)
            }
            ui::pass_debug_window::PassDebugWindowAction::ResetAllPatches => {
                AppCommand::ResetAllPassShaderPatches
            }
            ui::pass_debug_window::PassDebugWindowAction::UpsertDebugArtifact {
                item,
                content_text,
            } => AppCommand::UpsertDebugArtifact { item, content_text },
        };
        if let Err(err) = commands::dispatch(app, ctx, render_state, renderer, now, command) {
            eprintln!("[app] pass debug command failed: {err:#}");
        }
    }

    if app.shell.pending_shortwire_diff_capture.is_none()
        && !ui::pass_debug_window::has_active_shortwire(&app.shell.pass_debug_windows)
    {
        canvas::clear_shortwire_clipboard_reference(app);
    }

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

#[cfg(test)]
mod tests {
    use super::{timeline_toggle_shortcut_requested, update_timeline_review_state};
    use crate::{
        animation::TimelineFrameId, app::types::TimelineReviewState,
        ui::timeline_panel::TimelineInteraction,
    };

    #[test]
    fn timeline_toggle_shortcut_requires_command_and_no_text_focus() {
        assert!(timeline_toggle_shortcut_requested(false, true, true));
        assert!(!timeline_toggle_shortcut_requested(true, true, true));
        assert!(!timeline_toggle_shortcut_requested(false, false, true));
        assert!(!timeline_toggle_shortcut_requested(false, true, false));
    }

    #[test]
    fn timeline_review_prioritizes_hover_then_returns_to_anchor() {
        let anchor = TimelineFrameId(1);
        let hover = TimelineFrameId(2);
        let mut review = TimelineReviewState::default();

        let pause_requested = update_timeline_review_state(
            &mut review,
            TimelineInteraction {
                hovered_frame_id: Some(anchor),
                set_anchor_frame_id: Some(anchor),
                delete_anchor: false,
            },
            Some(hover),
            |_| true,
        );
        assert!(pause_requested);
        assert_eq!(review.display_frame_id(), Some(anchor));

        update_timeline_review_state(
            &mut review,
            TimelineInteraction {
                hovered_frame_id: Some(hover),
                ..Default::default()
            },
            Some(hover),
            |_| true,
        );
        assert_eq!(review.display_frame_id(), Some(hover));

        update_timeline_review_state(
            &mut review,
            TimelineInteraction::default(),
            Some(hover),
            |_| true,
        );
        assert_eq!(review.display_frame_id(), Some(anchor));
    }

    #[test]
    fn deleting_anchor_holds_latest_and_suppresses_stationary_hover() {
        let anchor = TimelineFrameId(1);
        let latest = TimelineFrameId(3);
        let mut review = TimelineReviewState {
            anchor_frame_id: Some(anchor),
            ..Default::default()
        };

        let pause_requested = update_timeline_review_state(
            &mut review,
            TimelineInteraction {
                hovered_frame_id: Some(anchor),
                delete_anchor: true,
                ..Default::default()
            },
            Some(latest),
            |_| true,
        );
        assert!(!pause_requested);
        assert_eq!(review.anchor_frame_id, None);
        assert_eq!(review.display_frame_id(), Some(latest));

        update_timeline_review_state(
            &mut review,
            TimelineInteraction {
                hovered_frame_id: Some(anchor),
                ..Default::default()
            },
            Some(latest),
            |_| true,
        );
        assert_eq!(review.display_frame_id(), Some(latest));
    }

    #[test]
    fn play_clear_keeps_live_restore_edge() {
        let mut review = TimelineReviewState {
            anchor_frame_id: Some(TimelineFrameId(1)),
            preview_applied_last_frame: true,
            ..Default::default()
        };
        review.clear_targets_for_play();
        assert_eq!(review.display_frame_id(), None);
        assert!(review.preview_applied_last_frame);
    }
}
