use std::sync::Arc;

use rust_wgpu_fiber::eframe::{
    egui::{self, Color32, Rect, pos2},
    egui_wgpu, wgpu,
};

use crate::{
    app::{
        canvas::{
            actions::{CanvasAction, CanvasFrameResult},
            display::{self, DisplayFrame},
            ops,
            pixel_overlay::{
                self, draw_pixel_overlay, format_diff_stat_value,
                value_sampling_reference_from_state,
            },
            reducer, reference, viewport,
        },
        frame::commands::AppCommand,
        types::{App, RefImageMode, ViewportOperationIndicatorVisual},
        window_mode::WindowModeFrame,
    },
    ui::{
        design_tokens,
        viewport_indicators::{
            ViewportIndicator, ViewportIndicatorEntry, ViewportIndicatorInteraction,
            ViewportIndicatorKind,
        },
    },
};

const ORDER_OPERATION: i32 = 0;
const ORDER_RENDER_FPS: i32 = 1;
const ORDER_PAUSE: i32 = 10;
const ORDER_HDR: i32 = 15;
const ORDER_SAMPLING: i32 = 20;
const ORDER_REF_ALPHA: i32 = 21;
const ORDER_CLIPPING: i32 = 30;
const ORDER_STATS: i32 = 40;
const KEY_TOGGLE_SAMPLING: egui::Key = egui::Key::N;
const KEY_TOGGLE_REFERENCE_ALPHA: egui::Key = egui::Key::P;

fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    let a = ((color.a() as f32) * alpha.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

fn merge_frame_result(into: &mut CanvasFrameResult, next: CanvasFrameResult) {
    into.commands.extend(next.commands);
}

fn apply_action(
    result: &mut CanvasFrameResult,
    app: &mut App,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    action: CanvasAction,
) {
    match reducer::apply_action(app, render_state, renderer, action) {
        Ok(next) => merge_frame_result(result, next),
        Err(err) => eprintln!("[canvas] action failed: {err:#}"),
    }
}

fn draw_checkerboard(ui: &egui::Ui, ctx: &egui::Context, canvas_rect: Rect) {
    let rounding = egui::CornerRadius::ZERO;
    let checker_tex = {
        let cache_id = egui::Id::new("ui.canvas.checkerboard_texture");
        if let Some(tex) = ctx.memory(|mem| mem.data.get_temp::<egui::TextureHandle>(cache_id)) {
            tex
        } else {
            let c0 = Color32::from_gray(28);
            let c1 = Color32::from_gray(38);
            let pixels = vec![c0, c1, c1, c0];
            let img = egui::ColorImage {
                size: [2, 2],
                pixels,
                source_size: egui::Vec2::new(2.0, 2.0),
            };
            let tex = ctx.load_texture(
                "ui.canvas.checkerboard",
                img,
                egui::TextureOptions::NEAREST_REPEAT,
            );
            ctx.memory_mut(|mem| mem.data.insert_temp(cache_id, tex.clone()));
            tex
        }
    };
    let cell = 16.0_f32;
    let uv_w = canvas_rect.width() / cell;
    let uv_h = canvas_rect.height() / cell;
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(uv_w, uv_h));
    ui.painter().add(
        egui::epaint::RectShape::filled(canvas_rect, rounding, Color32::WHITE)
            .with_texture(checker_tex.id(), uv),
    );
}

fn computed_uv(image_rect: Rect, canvas_rect: Rect) -> Rect {
    let image_rect_size = image_rect.size();
    let uv0_min = (canvas_rect.min - image_rect.min) / image_rect_size;
    let uv0_max = (canvas_rect.max - image_rect.min) / image_rect_size;
    Rect::from_min_max(pos2(uv0_min.x, uv0_min.y), pos2(uv0_max.x, uv0_max.y))
}

fn draw_display_layers(
    ui: &egui::Ui,
    app: &App,
    canvas_rect: Rect,
    image_rect: Rect,
    uv: Rect,
    display_frame: &DisplayFrame,
) {
    let rounding = egui::CornerRadius::ZERO;
    if let Some(tex_id) = display_frame.display_attachment {
        ui.painter().add(
            egui::epaint::RectShape::filled(canvas_rect, rounding, Color32::WHITE)
                .with_texture(tex_id, uv),
        );
    }

    if !display_frame.compare_output_active
        && let Some(reference_image) = app.canvas.reference.ref_image.as_ref()
    {
        let reference_size = egui::vec2(
            reference_image.size[0] as f32,
            reference_image.size[1] as f32,
        );
        let reference_min = image_rect.min + reference_image.offset * app.canvas.viewport.zoom;
        let reference_rect =
            Rect::from_min_size(reference_min, reference_size * app.canvas.viewport.zoom);
        let visible_rect = reference_rect.intersect(canvas_rect);

        if visible_rect.is_positive() {
            let uv_min = (visible_rect.min - reference_rect.min) / reference_rect.size();
            let uv_max = (visible_rect.max - reference_rect.min) / reference_rect.size();
            let reference_uv =
                Rect::from_min_max(pos2(uv_min.x, uv_min.y), pos2(uv_max.x, uv_max.y));
            let tint = if matches!(reference_image.mode, RefImageMode::Overlay) {
                Color32::from_rgba_unmultiplied(
                    255,
                    255,
                    255,
                    (reference_image.opacity * 255.0) as u8,
                )
            } else {
                Color32::WHITE
            };
            ui.painter().add(
                egui::epaint::RectShape::filled(visible_rect, rounding, tint)
                    .with_texture(reference_image.texture.id(), reference_uv),
            );
        }
    }

    if app.canvas.analysis.clip_enabled
        && let Some(clipping_texture_id) = app.canvas.analysis.clipping_texture_id
    {
        ui.painter().add(
            egui::epaint::RectShape::filled(canvas_rect, rounding, Color32::WHITE)
                .with_texture(clipping_texture_id, uv),
        );
    }
}

fn draw_operation_indicators(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    canvas_rect: Rect,
    now: f64,
    display_frame: &DisplayFrame,
) {
    let sampling_indicator = match app.canvas.display.texture_filter {
        wgpu::FilterMode::Nearest => ViewportIndicator {
            icon: "N",
            tooltip: "Viewport sampling: Nearest (press N to toggle Linear)",
            kind: ViewportIndicatorKind::Text,
            strikethrough: false,
        },
        wgpu::FilterMode::Linear => ViewportIndicator {
            icon: "L",
            tooltip: "Viewport sampling: Linear (press N to toggle Nearest)",
            kind: ViewportIndicatorKind::Text,
            strikethrough: false,
        },
    };
    let reference_alpha_indicator = ViewportIndicator {
        icon: app
            .canvas
            .reference
            .ref_image
            .as_ref()
            .map(|reference_image| reference_image.alpha_mode.short_label())
            .unwrap_or(app.canvas.reference.alpha_mode.short_label()),
        tooltip: "Reference alpha mode: PRE (premultiplied) / STR (straight). Press P to toggle.",
        kind: ViewportIndicatorKind::Text,
        strikethrough: false,
    };
    let current_view_is_hdr = matches!(
        display_frame.display_texture_format,
        Some(wgpu::TextureFormat::Rgba16Float)
    );
    let hdr_indicator_tooltip = if display_frame.hdr_clamp_effective {
        "Current view format: Rgba16Float (HDR) • Clamp to 1.0 ON (press S to toggle)"
    } else {
        "Current view format: Rgba16Float (HDR) • Clamp to 1.0 OFF (press S to toggle)"
    };
    let operation_visual = ops::current_visual(&app.canvas.async_ops);

    app.canvas.viewport_indicator_manager.begin_frame();

    if let Some(visual) = operation_visual {
        let operation_indicator = match visual {
            ViewportOperationIndicatorVisual::InProgress => ViewportIndicator {
                icon: "",
                tooltip: "正在复制材质到剪贴板...",
                kind: ViewportIndicatorKind::Spinner,
                strikethrough: false,
            },
            ViewportOperationIndicatorVisual::Success => ViewportIndicator {
                icon: "✓",
                tooltip: "复制完成",
                kind: ViewportIndicatorKind::Success,
                strikethrough: false,
            },
            ViewportOperationIndicatorVisual::Failure => ViewportIndicator {
                icon: "✕",
                tooltip: "复制失败",
                kind: ViewportIndicatorKind::Failure,
                strikethrough: false,
            },
        };
        app.canvas
            .viewport_indicator_manager
            .register(ViewportIndicatorEntry {
                interaction: ViewportIndicatorInteraction::HoverOnly,
                callback_id: None,
                ..ViewportIndicatorEntry::compact(
                    "operation",
                    ORDER_OPERATION,
                    ops::is_visible(&app.canvas.async_ops),
                    operation_indicator,
                )
            });
    }

    app.canvas.viewport_indicator_manager.register(ViewportIndicatorEntry {
        interaction: ViewportIndicatorInteraction::HoverOnly,
        callback_id: None,
        ..ViewportIndicatorEntry::text_badge_right_aligned_mono(
            "render_fps",
            ORDER_RENDER_FPS,
            true,
            format!("{} FPS", app.runtime.render_texture_fps_tracker.fps_at(now)),
            "Scene redraws per second (counts scene redraws only; excludes reference-image/diff/clipping/analysis-only updates)",
        )
    });

    app.canvas
        .viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::compact(
                "preview_hdr",
                ORDER_HDR,
                current_view_is_hdr,
                ViewportIndicator {
                    icon: "HDR",
                    tooltip: hdr_indicator_tooltip,
                    kind: ViewportIndicatorKind::Hdr,
                    strikethrough: display_frame.hdr_clamp_effective,
                },
            )
        });

    app.canvas
        .viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            animated: false,
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::compact("sampling", ORDER_SAMPLING, true, sampling_indicator)
        });

    app.canvas
        .viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            animated: false,
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::compact(
                "ref_alpha_mode",
                ORDER_REF_ALPHA,
                app.canvas.reference.ref_image.is_some(),
                reference_alpha_indicator,
            )
        });

    app.canvas
        .viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::compact(
                "pause",
                ORDER_PAUSE,
                !app.runtime.time_updates_enabled,
                ViewportIndicator {
                    icon: "PAUSE",
                    tooltip: "Time 更新已暂停（Space 恢复）",
                    kind: ViewportIndicatorKind::Failure,
                    strikethrough: false,
                },
            )
        });

    app.canvas
        .viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::compact(
                "clipping",
                ORDER_CLIPPING,
                app.canvas.analysis.clip_enabled,
                ViewportIndicator {
                    icon: "C",
                    tooltip: "Clipping overlay 已开启",
                    kind: ViewportIndicatorKind::Failure,
                    strikethrough: false,
                },
            )
        });

    if let Some(stats) = app.canvas.analysis.diff_stats {
        app.canvas
            .viewport_indicator_manager
            .register(ViewportIndicatorEntry {
                interaction: ViewportIndicatorInteraction::HoverOnly,
                callback_id: None,
                ..ViewportIndicatorEntry::text_badge(
                    "diff_stats",
                    ORDER_STATS,
                    matches!(
                        app.canvas.reference.ref_image.as_ref().map(|r| r.mode),
                        Some(RefImageMode::Diff)
                    ),
                    format!(
                        "min {}  max {}  avg {}  rms {}  p95|S| {}  n {}  nonfinite {}",
                        format_diff_stat_value(stats.min),
                        format_diff_stat_value(stats.max),
                        format_diff_stat_value(stats.avg),
                        format_diff_stat_value(stats.rms),
                        format_diff_stat_value(stats.p95_abs),
                        stats.sample_count,
                        stats.non_finite_count
                    ),
                    "Diff 统计",
                )
            });
    }

    let indicator_result = app
        .canvas
        .viewport_indicator_manager
        .render(ui, ctx, canvas_rect, now);
    if indicator_result.needs_repaint {
        ctx.request_repaint();
    }
}

fn draw_badges(
    app: &App,
    ui: &egui::Ui,
    ctx: &egui::Context,
    canvas_rect: Rect,
    using_preview: bool,
) {
    let badge_font = egui::FontId::new(
        11.0,
        crate::ui::typography::mi_sans_family_for_weight(500.0),
    );
    let mut badge_y = canvas_rect.min.y + 8.0;
    let badge_x = canvas_rect.min.x + 8.0;
    let ref_tag_visible = app
        .canvas
        .reference
        .ref_image
        .as_ref()
        .is_some_and(|reference_image| reference_image.opacity > 0.001);
    let ref_tag_anim_t =
        ctx.animate_bool(egui::Id::new("ui.canvas.ref_tag.visible"), ref_tag_visible);

    if let Some(reference_image) = app.canvas.reference.ref_image.as_ref() {
        let mode = match reference_image.mode {
            RefImageMode::Overlay => "Overlay",
            RefImageMode::Diff => "Abs Diff",
        };
        let badge_text = format!(
            "Ref • {} • {} • {}×{} • α {:.2}",
            mode,
            reference_image.alpha_mode.short_label(),
            reference_image.size[0],
            reference_image.size[1],
            reference_image.opacity,
        );
        let badge_galley = ui.painter().layout_no_wrap(
            badge_text,
            badge_font.clone(),
            with_alpha(Color32::from_rgb(182, 255, 199), ref_tag_anim_t),
        );
        let badge_size = badge_galley.size() + egui::vec2(16.0, 8.0);
        if ref_tag_anim_t > 0.001 {
            let slide_y = (1.0 - ref_tag_anim_t) * -6.0;
            let badge_rect = Rect::from_min_size(pos2(badge_x, badge_y + slide_y), badge_size);
            ui.painter().rect(
                badge_rect,
                egui::CornerRadius::same(6),
                with_alpha(
                    Color32::from_rgba_unmultiplied(6, 28, 12, 196),
                    ref_tag_anim_t,
                ),
                egui::Stroke::new(
                    1.0,
                    with_alpha(
                        Color32::from_rgba_unmultiplied(56, 181, 96, 220),
                        ref_tag_anim_t,
                    ),
                ),
                egui::StrokeKind::Outside,
            );
            ui.painter().galley(
                pos2(badge_rect.min.x + 8.0, badge_rect.min.y + 4.0),
                badge_galley,
                Color32::PLACEHOLDER,
            );
        }
        badge_y += (badge_size.y + 6.0) * ref_tag_anim_t;
    }

    if let Some(preview_name) = app.canvas.display.preview_texture_name.as_ref()
        && using_preview
    {
        let badge_text =
            if let Some(info) = app.core.shader_space.texture_info(preview_name.as_str()) {
                format!(
                    "Preview • {} • {}×{} • {:?}",
                    preview_name.as_str(),
                    info.size.width,
                    info.size.height,
                    info.format,
                )
            } else {
                format!("Preview • {}", preview_name.as_str())
            };
        let badge_galley =
            ui.painter()
                .layout_no_wrap(badge_text, badge_font, Color32::from_gray(220));
        let badge_size = badge_galley.size() + egui::vec2(16.0, 8.0);
        let badge_rect = Rect::from_min_size(pos2(badge_x, badge_y), badge_size);
        ui.painter().rect(
            badge_rect,
            egui::CornerRadius::same(6),
            Color32::from_rgba_unmultiplied(0, 0, 0, 180),
            egui::Stroke::new(1.0, Color32::from_gray(32)),
            egui::StrokeKind::Outside,
        );
        ui.painter().galley(
            pos2(badge_rect.min.x + 8.0, badge_rect.min.y + 4.0),
            badge_galley,
            Color32::PLACEHOLDER,
        );
    }

    if ref_tag_anim_t > 0.001 && ref_tag_anim_t < 0.999 {
        ctx.request_repaint();
    }
}

fn maybe_sample_clicked_pixel(
    app: &mut App,
    ctx: &egui::Context,
    response: &egui::Response,
    canvas_rect: Rect,
    image_rect: Rect,
    display_frame: &DisplayFrame,
    frame: WindowModeFrame,
    value_sample_cache: Option<&pixel_overlay::PixelOverlayCache>,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
) {
    let continuous_scene_redraw = (app.runtime.scene_uses_time && app.runtime.time_updates_enabled)
        || app
            .runtime
            .animation_session
            .as_ref()
            .is_some_and(|session| session.is_active())
        || app.runtime.capture_redraw_active;

    if !response.clicked_by(egui::PointerButton::Primary) {
        return;
    }
    let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    if !canvas_rect.contains(pointer_pos)
        || (!matches!(frame.mode, crate::app::types::UiWindowMode::Sidebar)
            && !image_rect.contains(pointer_pos))
    {
        return;
    }

    let uv = computed_uv(image_rect, canvas_rect);
    let local = (pointer_pos - canvas_rect.min) / canvas_rect.size();
    let uv_x = uv.min.x + local.x * uv.width();
    let uv_y = uv.min.y + local.y * uv.height();
    let x = (uv_x * display_frame.effective_resolution[0] as f32).floor() as u32;
    let y = (uv_y * display_frame.effective_resolution[1] as f32).floor() as u32;
    if x >= display_frame.effective_resolution[0] || y >= display_frame.effective_resolution[1] {
        return;
    }

    let mut sample_cache: Option<Arc<pixel_overlay::PixelOverlayCache>> =
        value_sample_cache.cloned().map(Arc::new);
    if sample_cache.is_none() && !continuous_scene_redraw {
        if let Some(info) = app
            .core
            .shader_space
            .texture_info(display_frame.value_sampling_texture_name.as_str())
        {
            let _ = renderer;
            let _ = render_state;
            sample_cache = Some(pixel_overlay::get_or_refresh_cache(
                app,
                display_frame.value_sampling_texture_name.as_str(),
                info.size.width,
                info.size.height,
                info.format,
            ));
        }
    }
    if let Some(cache) = sample_cache.as_deref()
        && let Some(rgba) = pixel_overlay::sample_value_pixel(
            cache,
            x,
            y,
            app.canvas
                .reference
                .ref_image
                .as_ref()
                .map(value_sampling_reference_from_state),
            app.canvas.analysis.diff_metric_mode,
            display_frame.compare_output_active,
            app.canvas.display.hdr_preview_clamp_enabled,
        )
    {
        apply_action(
            &mut CanvasFrameResult::default(),
            app,
            render_state,
            renderer,
            CanvasAction::SamplePixel { x, y, rgba },
        );
    }
}

pub fn show_canvas(
    app: &mut App,
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    frame: WindowModeFrame,
    now: f64,
) -> CanvasFrameResult {
    let mut frame_result = CanvasFrameResult::default();

    if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::F)) {
        frame_result.commands.push(AppCommand::ToggleCanvasOnly);
    }
    if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::S)) {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::ToggleHdrClamp,
        );
    }
    if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::TogglePause,
        );
    }
    if ctx.input(|i| i.key_pressed(egui::Key::R)) {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::ResetView,
        );
    }
    if ctx.input(|i| i.key_pressed(KEY_TOGGLE_SAMPLING)) {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::ToggleSampling,
        );
    }
    if ctx.input(|i| i.key_pressed(KEY_TOGGLE_REFERENCE_ALPHA)) {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::ToggleReferenceAlpha,
        );
    }
    if ctx.input(|i| i.key_pressed(egui::Key::C)) {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::ToggleClipping,
        );
    }
    if ctx.input(|i| i.key_pressed(egui::Key::A)) {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::ResetReferenceOffset,
        );
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Num1)) {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::SetReferenceOpacity(0.0),
        );
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Num2)) {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::SetReferenceOpacity(1.0),
        );
    }
    if ctx.input(|i| i.key_pressed(egui::Key::D)) {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::ToggleReferenceMode,
        );
    }

    reference::maybe_handle_reference_drop(app, ctx, render_state);

    if app.canvas.display.preview_texture_name.is_some()
        && ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::ClearPreviewTexture,
        );
    }

    let using_preview = display::sync_preview_source(app, render_state, renderer);
    let display_frame = display::build_display_frame(app, render_state, renderer, using_preview);
    let image_size = egui::vec2(
        display_frame.effective_resolution[0] as f32,
        display_frame.effective_resolution[1] as f32,
    );
    let canvas_rect = ui.available_rect_before_wrap();
    let mut viewport_frame = viewport::prepare_viewport(app, frame, now, canvas_rect, image_size);
    let response = ui.allocate_rect(canvas_rect, egui::Sense::click_and_drag());

    if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
        ui.painter().rect_filled(
            canvas_rect,
            egui::CornerRadius::same(design_tokens::BORDER_RADIUS_REGULAR as u8),
            Color32::from_rgba_unmultiplied(80, 130, 255, 36),
        );
        ui.painter().rect_stroke(
            canvas_rect.shrink(2.0),
            egui::CornerRadius::same(design_tokens::BORDER_RADIUS_REGULAR as u8),
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 170, 255, 220)),
            egui::StrokeKind::Outside,
        );
        ui.painter().text(
            canvas_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Drop PNG / JPEG / EXR as Reference",
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
            Color32::from_rgba_unmultiplied(214, 228, 255, 240),
        );
    }

    let context_menu_opened_this_frame = response.secondary_clicked();
    response.context_menu(|menu_ui| {
        let copy_clicked = menu_ui.button("复制材质").clicked();
        if copy_clicked && !context_menu_opened_this_frame {
            if let Some(pass_name) = app.core.export_encode_pass_name.as_ref() {
                app.core
                    .shader_space
                    .render_pass_by_name(pass_name.as_str());
            }
            let export_tex = app.core.export_texture_name.as_str();
            if let Some(info) = app.core.shader_space.texture_info(export_tex)
                && let Ok(image) = app.core.shader_space.read_texture_rgba8(export_tex)
            {
                ops::begin_clipboard_copy(
                    &mut app.canvas.async_ops,
                    now,
                    info.size.width as usize,
                    info.size.height as usize,
                    image.bytes,
                );
            }
            menu_ui.close();
        }
    });

    if viewport_frame.pan_zoom_enabled {
        if response.drag_started_by(egui::PointerButton::Middle)
            && let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos())
        {
            apply_action(
                &mut frame_result,
                app,
                render_state,
                renderer,
                CanvasAction::BeginPanDrag(pointer_pos),
            );
        }
        if response.dragged_by(egui::PointerButton::Middle)
            && let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos())
        {
            apply_action(
                &mut frame_result,
                app,
                render_state,
                renderer,
                CanvasAction::UpdatePanDrag(pointer_pos),
            );
        } else if !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle)) {
            apply_action(
                &mut frame_result,
                app,
                render_state,
                renderer,
                CanvasAction::EndPanDrag,
            );
        }

        if app.canvas.reference.ref_image.is_some() {
            if response.drag_started_by(egui::PointerButton::Primary)
                && let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos())
            {
                apply_action(
                    &mut frame_result,
                    app,
                    render_state,
                    renderer,
                    CanvasAction::BeginReferenceDrag(pointer_pos),
                );
            }
            if response.dragged_by(egui::PointerButton::Primary)
                && let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos())
            {
                apply_action(
                    &mut frame_result,
                    app,
                    render_state,
                    renderer,
                    CanvasAction::UpdateReferenceDrag(pointer_pos),
                );
            } else if !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Primary)) {
                apply_action(
                    &mut frame_result,
                    app,
                    render_state,
                    renderer,
                    CanvasAction::EndReferenceDrag,
                );
            }
        } else {
            if response.drag_started_by(egui::PointerButton::Primary)
                && let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos())
            {
                apply_action(
                    &mut frame_result,
                    app,
                    render_state,
                    renderer,
                    CanvasAction::BeginPanDrag(pointer_pos),
                );
            }
            if response.dragged_by(egui::PointerButton::Primary)
                && let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos())
            {
                apply_action(
                    &mut frame_result,
                    app,
                    render_state,
                    renderer,
                    CanvasAction::UpdatePanDrag(pointer_pos),
                );
            } else if !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Primary))
                && !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle))
            {
                apply_action(
                    &mut frame_result,
                    app,
                    render_state,
                    renderer,
                    CanvasAction::EndPanDrag,
                );
            }
        }
    }

    let canvas_hovered = response.hovered();
    let zoom_delta = if canvas_hovered {
        ctx.input(|i| i.zoom_delta())
    } else {
        1.0
    };
    let scroll_delta = if canvas_hovered {
        ctx.input(|i| i.smooth_scroll_delta)
    } else {
        egui::Vec2::ZERO
    };
    if viewport_frame.pan_zoom_enabled
        && zoom_delta == 1.0
        && (scroll_delta.x != 0.0 || scroll_delta.y != 0.0)
    {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::ApplyScrollPan(scroll_delta),
        );
    }
    if viewport_frame.pan_zoom_enabled
        && zoom_delta != 1.0
        && let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos())
    {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::ApplyZoomAroundPointer {
                pointer_pos,
                zoom_delta,
                canvas_rect,
                image_size,
                effective_min_zoom: viewport_frame.effective_min_zoom,
            },
        );
    }

    viewport_frame.image_rect = viewport::image_rect(app, canvas_rect, image_size);
    draw_checkerboard(ui, ctx, canvas_rect);
    let uv = computed_uv(viewport_frame.image_rect, canvas_rect);
    draw_display_layers(
        ui,
        app,
        canvas_rect,
        viewport_frame.image_rect,
        uv,
        &display_frame,
    );

    let mut value_sample_cache = None;
    if app.canvas.viewport.zoom >= 48.0
        && let Some(info) = app
            .core
            .shader_space
            .texture_info(display_frame.value_sampling_texture_name.as_str())
    {
        value_sample_cache = Some(pixel_overlay::get_or_refresh_cache(
            app,
            display_frame.value_sampling_texture_name.as_str(),
            info.size.width,
            info.size.height,
            info.format,
        ));
        draw_pixel_overlay(
            ui,
            viewport_frame.image_rect,
            canvas_rect,
            app.canvas.viewport.zoom,
            display_frame.effective_resolution,
            value_sample_cache.as_deref(),
            app.canvas
                .reference
                .ref_image
                .as_ref()
                .map(value_sampling_reference_from_state),
            app.canvas.analysis.diff_metric_mode,
            display_frame.compare_output_active,
            app.canvas.display.hdr_preview_clamp_enabled,
        );
    }

    draw_operation_indicators(app, ui, ctx, canvas_rect, now, &display_frame);
    draw_badges(app, ui, ctx, canvas_rect, using_preview);
    maybe_sample_clicked_pixel(
        app,
        ctx,
        &response,
        canvas_rect,
        viewport_frame.image_rect,
        &display_frame,
        frame,
        value_sample_cache.as_deref(),
        render_state,
        renderer,
    );

    app.canvas.viewport.canvas_center_prev = Some(canvas_rect.center());
    app.canvas.interactions.last_canvas_rect = Some(canvas_rect);

    frame_result
}
