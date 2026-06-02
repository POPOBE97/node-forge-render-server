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
        display_metrics,
        frame::commands::AppCommand,
        matrix_render,
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
const ORDER_QUALIFIER: i32 = 31;
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

    if app.canvas.analysis.qualifier_enabled
        && let Some(qualifier_texture_id) = app.canvas.analysis.qualifier_texture_id
    {
        ui.painter().add(
            egui::epaint::RectShape::filled(canvas_rect, rounding, Color32::WHITE)
                .with_texture(qualifier_texture_id, uv),
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

    app.canvas
        .viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::compact(
                "qualifier",
                ORDER_QUALIFIER,
                app.canvas.analysis.qualifier_enabled,
                ViewportIndicator {
                    icon: "Q",
                    tooltip: "Qualifier overlay 已开启",
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

fn draw_matrix_indicators(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    canvas_rect: Rect,
    now: f64,
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

    app.canvas
        .viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::text_badge_right_aligned_mono(
                "render_fps",
                ORDER_RENDER_FPS,
                true,
                format!("{} FPS", app.runtime.render_texture_fps_tracker.fps_at(now)),
                "Scene redraws per second",
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

    let hdr_clamp_active = app.canvas.display.hdr_preview_clamp_enabled;
    let hdr_indicator_tooltip = if hdr_clamp_active {
        "Matrix cells: Rgba16Float (HDR) • Clamp to 1.0 ON (press S to toggle)"
    } else {
        "Matrix cells: Rgba16Float (HDR) • Clamp to 1.0 OFF (press S to toggle)"
    };
    app.canvas
        .viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::compact(
                "preview_hdr",
                ORDER_HDR,
                true,
                ViewportIndicator {
                    icon: "HDR",
                    tooltip: hdr_indicator_tooltip,
                    kind: ViewportIndicatorKind::Hdr,
                    strikethrough: hdr_clamp_active,
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

    app.canvas
        .viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::compact(
                "qualifier",
                ORDER_QUALIFIER,
                app.canvas.analysis.qualifier_enabled,
                ViewportIndicator {
                    icon: "Q",
                    tooltip: "Qualifier overlay 已开启",
                    kind: ViewportIndicatorKind::Failure,
                    strikethrough: false,
                },
            )
        });

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
        || app.runtime.capture_redraw_active
        || app.runtime.force_continuous_redraw;

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
    let current_display_metrics = display_metrics::current_display_metrics(ctx);

    if ctx.input(|i| i.key_pressed(egui::Key::Num1) && i.modifiers.command) {
        apply_action(
            &mut frame_result,
            app,
            render_state,
            renderer,
            CanvasAction::CenterAt1x {
                pixels_per_point: current_display_metrics.pixels_per_point,
                current_display_ppi: current_display_metrics.display_ppi,
            },
        );
    }

    let plain_shortcuts_enabled = !ctx.wants_keyboard_input();
    if plain_shortcuts_enabled {
        if ctx.input(|i| i.key_pressed(egui::Key::F)) {
            frame_result.commands.push(AppCommand::ToggleCanvasOnly);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::S)) {
            apply_action(
                &mut frame_result,
                app,
                render_state,
                renderer,
                CanvasAction::ToggleHdrClamp,
            );
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
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
                CanvasAction::ResetView {
                    current_display_ppi: current_display_metrics.display_ppi,
                },
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
        if ctx.input(|i| i.key_pressed(egui::Key::Num1) && !i.modifiers.command) {
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
    }

    reference::maybe_handle_reference_drop(app, ctx, render_state);

    if app.canvas.display.preview_texture_name.is_some()
        && plain_shortcuts_enabled
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

    // ── Determine image_size: matrix grid or single scene ────────────
    let matrix_active = app.shell.test_mode == crate::app::types::TestMode::Matrix
        && !app.shell.matrix_state.cells.is_empty();

    let (using_preview, display_frame, image_size) = if matrix_active {
        let ms = &app.shell.matrix_state;
        let cell_w = ms.cell_resolution[0] as f32;
        let cell_h = ms.cell_resolution[1] as f32;
        let cols = ms.grid_cols.max(1) as f32;
        let rows = ms.grid_rows.max(1) as f32;
        let row_gap = matrix_row_gap_px(ms);
        let total_w = cell_w * cols + MATRIX_GRID_GAP_PX * (cols - 1.0);
        let total_h = cell_h * rows + row_gap * (rows - 1.0);
        (false, None, egui::vec2(total_w.max(1.0), total_h.max(1.0)))
    } else {
        let using_preview = display::sync_preview_source(app, render_state, renderer);
        let df = display::build_display_frame(app, render_state, renderer, using_preview);
        let sz = egui::vec2(
            df.effective_resolution[0] as f32,
            df.effective_resolution[1] as f32,
        );
        (using_preview, Some(df), sz)
    };

    let canvas_rect = ui.available_rect_before_wrap();
    let mut viewport_frame = viewport::prepare_viewport(
        app,
        frame,
        now,
        canvas_rect,
        image_size,
        current_display_metrics,
    );
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
                current_display_ppi: current_display_metrics.display_ppi,
                pixels_per_point: current_display_metrics.pixels_per_point,
            },
        );
    }

    viewport_frame.image_rect = viewport::image_rect(app, canvas_rect, image_size);
    draw_checkerboard(ui, ctx, canvas_rect);

    if matrix_active {
        update_matrix_hover(app, ctx, canvas_rect, viewport_frame.image_rect);
        draw_matrix_grid_viewport(ui, app, canvas_rect, viewport_frame.image_rect);
        draw_matrix_pixel_overlays(ui, app, canvas_rect, viewport_frame.image_rect);
        draw_matrix_indicators(app, ui, ctx, canvas_rect, now);
        maybe_sample_matrix_clicked_pixel(
            app,
            ctx,
            &response,
            canvas_rect,
            viewport_frame.image_rect,
            render_state,
            renderer,
            &mut frame_result,
        );
    } else if let Some(ref display_frame) = display_frame {
        let uv = computed_uv(viewport_frame.image_rect, canvas_rect);
        draw_display_layers(
            ui,
            app,
            canvas_rect,
            viewport_frame.image_rect,
            uv,
            display_frame,
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

        draw_operation_indicators(app, ui, ctx, canvas_rect, now, display_frame);
        draw_badges(app, ui, ctx, canvas_rect, using_preview);
        maybe_sample_clicked_pixel(
            app,
            ctx,
            &response,
            canvas_rect,
            viewport_frame.image_rect,
            display_frame,
            frame,
            value_sample_cache.as_deref(),
            render_state,
            renderer,
        );
    }

    app.canvas.viewport.canvas_center_prev = Some(canvas_rect.center());
    app.canvas.interactions.last_canvas_rect = Some(canvas_rect);

    frame_result
}

/// Gap between matrix cells in texel units (native resolution pixels).
const MATRIX_GRID_GAP_PX: f32 = 4.0;
const MATRIX_LABEL_FONT_SIZE: f32 = 11.0;
const MATRIX_LABEL_ROW_HEIGHT_PX: f32 = MATRIX_LABEL_FONT_SIZE + 6.0;
const MATRIX_WRAPPED_ROW_GAP_PX: f32 = MATRIX_LABEL_ROW_HEIGHT_PX + 4.0;

fn matrix_row_gap_px(state: &matrix_render::MatrixRenderState) -> f32 {
    if state.show_labels && state.row_chunks_per_logical_row > 1 {
        MATRIX_WRAPPED_ROW_GAP_PX
    } else {
        MATRIX_GRID_GAP_PX
    }
}

fn matrix_cell_screen_rect(
    state: &matrix_render::MatrixRenderState,
    coord: matrix_render::MatrixCellCoord,
    image_rect: Rect,
    zoom: f32,
) -> Option<Rect> {
    let display_coord = state
        .cells
        .iter()
        .find(|cell| cell.coord == coord)
        .map(|cell| cell.display_coord)?;
    Some(matrix_display_cell_screen_rect(
        state,
        display_coord,
        image_rect,
        zoom,
    ))
}

fn matrix_display_cell_screen_rect(
    state: &matrix_render::MatrixRenderState,
    display_coord: matrix_render::MatrixCellCoord,
    image_rect: Rect,
    zoom: f32,
) -> Rect {
    let cell_w = state.cell_resolution[0] as f32;
    let cell_h = state.cell_resolution[1] as f32;
    let local_x = display_coord.col as f32 * (cell_w + MATRIX_GRID_GAP_PX);
    let local_y = display_coord.row as f32 * (cell_h + matrix_row_gap_px(state));
    Rect::from_min_size(
        pos2(
            image_rect.min.x + local_x * zoom,
            image_rect.min.y + local_y * zoom,
        ),
        egui::vec2(cell_w * zoom, cell_h * zoom),
    )
}

fn matrix_hit_test(
    state: &matrix_render::MatrixRenderState,
    pointer_pos: egui::Pos2,
    image_rect: Rect,
    zoom: f32,
) -> Option<matrix_render::MatrixCellCoord> {
    state.cells.iter().find_map(|cell| {
        let rect = matrix_display_cell_screen_rect(state, cell.display_coord, image_rect, zoom);
        rect.contains(pointer_pos).then_some(cell.coord)
    })
}

fn update_matrix_hover(app: &mut App, ctx: &egui::Context, canvas_rect: Rect, image_rect: Rect) {
    let hovered = ctx.input(|i| i.pointer.hover_pos()).and_then(|pos| {
        if !canvas_rect.contains(pos) {
            return None;
        }
        matrix_hit_test(
            &app.shell.matrix_state,
            pos,
            image_rect,
            app.canvas.viewport.zoom,
        )
    });
    let state = &mut app.shell.matrix_state;
    state.hovered_coord = hovered;
    if let Some(coord) = hovered {
        state.sticky_stats_coord = Some(coord);
    }
}

fn draw_matrix_grid_viewport(ui: &egui::Ui, app: &App, canvas_rect: Rect, image_rect: Rect) {
    let state = &app.shell.matrix_state;
    if state.cells.is_empty() {
        return;
    }

    let cell_w = state.cell_resolution[0] as f32;
    let cell_h = state.cell_resolution[1] as f32;
    let zoom = app.canvas.viewport.zoom;

    let painter = ui.painter_at(canvas_rect);

    let use_hdr_clamp = app.canvas.display.hdr_preview_clamp_enabled;
    let reference = app.canvas.reference.ref_image.as_ref();
    let in_diff_mode = matches!(reference.map(|r| r.mode), Some(RefImageMode::Diff));
    let clip_enabled = app.canvas.analysis.clip_enabled;
    let qualifier_enabled = app.canvas.analysis.qualifier_enabled;

    for cell in &state.cells {
        let cell_rect =
            matrix_display_cell_screen_rect(state, cell.display_coord, image_rect, zoom);

        let visible = cell_rect.intersect(canvas_rect);
        if !visible.is_positive() {
            continue;
        }

        let uv_min = (visible.min - cell_rect.min) / cell_rect.size();
        let uv_max = (visible.max - cell_rect.min) / cell_rect.size();
        let cell_uv = Rect::from_min_max(pos2(uv_min.x, uv_min.y), pos2(uv_max.x, uv_max.y));

        // Base layer: in Diff mode, the diff texture replaces the cell render
        // (mirrors the single-image `compare_output_active` semantics).
        let base_texture_id = if in_diff_mode && cell.diff_texture_id.is_some() {
            cell.diff_texture_id
        } else if use_hdr_clamp {
            cell.hdr_clamped_egui_id.or(cell.egui_texture_id)
        } else {
            cell.egui_texture_id
        };
        let Some(base_texture_id) = base_texture_id else {
            continue;
        };

        painter.add(
            egui::epaint::RectShape::filled(visible, egui::CornerRadius::ZERO, Color32::WHITE)
                .with_texture(base_texture_id, cell_uv),
        );

        // Reference overlay: only in Overlay mode, drawn relative to each
        // cell's local origin so the same offset/opacity applies per cell.
        if !in_diff_mode && let Some(ref_img) = reference {
            let ref_size_px = egui::vec2(ref_img.size[0] as f32, ref_img.size[1] as f32);
            let ref_min = cell_rect.min + ref_img.offset * zoom;
            let ref_rect = Rect::from_min_size(ref_min, ref_size_px * zoom);
            let ref_visible = ref_rect.intersect(cell_rect).intersect(canvas_rect);
            if ref_visible.is_positive() {
                let ru_min = (ref_visible.min - ref_rect.min) / ref_rect.size();
                let ru_max = (ref_visible.max - ref_rect.min) / ref_rect.size();
                let ref_uv = Rect::from_min_max(pos2(ru_min.x, ru_min.y), pos2(ru_max.x, ru_max.y));
                let tint = if matches!(ref_img.mode, RefImageMode::Overlay) {
                    Color32::from_rgba_unmultiplied(255, 255, 255, (ref_img.opacity * 255.0) as u8)
                } else {
                    Color32::WHITE
                };
                painter.add(
                    egui::epaint::RectShape::filled(ref_visible, egui::CornerRadius::ZERO, tint)
                        .with_texture(ref_img.texture.id(), ref_uv),
                );
            }
        }

        // Clipping overlay: per-cell, bounded to the cell rect.
        if clip_enabled && let Some(clip_id) = cell.clipping_texture_id {
            painter.add(
                egui::epaint::RectShape::filled(visible, egui::CornerRadius::ZERO, Color32::WHITE)
                    .with_texture(clip_id, cell_uv),
            );
        }

        // Qualifier overlay: per-cell, bounded to the cell rect.
        if qualifier_enabled && let Some(qualifier_id) = cell.qualifier_texture_id {
            painter.add(
                egui::epaint::RectShape::filled(visible, egui::CornerRadius::ZERO, Color32::WHITE)
                    .with_texture(qualifier_id, cell_uv),
            );
        }
    }

    if !state.show_labels {
        return;
    }

    let label_font = egui::FontId::new(MATRIX_LABEL_FONT_SIZE, egui::FontFamily::Monospace);
    let col_header_h = MATRIX_LABEL_ROW_HEIGHT_PX;
    let row_header_w = MATRIX_LABEL_FONT_SIZE + 6.0;
    let row_chunks_per_logical_row = state.row_chunks_per_logical_row.max(1);

    if row_chunks_per_logical_row <= 1
        && let Some(ref col_pool_id) = state.col_pool_id
    {
        let col_pool = app
            .shell
            .resource_pools
            .iter()
            .find(|p| p.node_id == *col_pool_id);
        for col in 0..state.logical_cols {
            let fallback = format!("{col}");
            let item_name = col_pool
                .and_then(|p| p.item_names.get(col))
                .map(|s| s.as_str())
                .unwrap_or(&fallback);
            let local_x = col as f32 * (cell_w + MATRIX_GRID_GAP_PX);
            let screen_x = image_rect.min.x + local_x * zoom;
            let lane_w = cell_w * zoom;
            let bg_rect = Rect::from_min_size(
                pos2(screen_x, image_rect.min.y - col_header_h - 2.0),
                egui::vec2(lane_w, col_header_h),
            );
            let visible = bg_rect.intersect(canvas_rect);
            if !visible.is_positive() {
                continue;
            }
            painter.rect_filled(visible, egui::CornerRadius::ZERO, Color32::BLACK);
            let galley =
                painter.layout_no_wrap(item_name.to_owned(), label_font.clone(), Color32::WHITE);
            let text_pos = pos2(
                bg_rect.center().x - galley.size().x * 0.5,
                bg_rect.center().y - galley.size().y * 0.5,
            );
            if canvas_rect.contains(text_pos) {
                painter.galley(text_pos, galley, Color32::PLACEHOLDER);
            }
        }
    } else if row_chunks_per_logical_row > 1
        && let Some(ref col_pool_id) = state.col_pool_id
    {
        let row_gap_screen = matrix_row_gap_px(state) * zoom;
        if row_gap_screen >= col_header_h + 2.0 {
            let col_pool = app
                .shell
                .resource_pools
                .iter()
                .find(|p| p.node_id == *col_pool_id);
            for cell in &state.cells {
                let cell_rect =
                    matrix_display_cell_screen_rect(state, cell.display_coord, image_rect, zoom);
                if !cell_rect.intersects(canvas_rect) {
                    continue;
                }

                let fallback = format!("{}", cell.coord.col);
                let item_name = col_pool
                    .and_then(|p| p.item_names.get(cell.coord.col))
                    .map(|s| s.as_str())
                    .unwrap_or(&fallback);
                let label_rect = Rect::from_min_size(
                    pos2(cell_rect.min.x, cell_rect.min.y - row_gap_screen + 1.0),
                    egui::vec2(cell_rect.width(), col_header_h),
                );
                let visible = label_rect.intersect(canvas_rect);
                if !visible.is_positive() {
                    continue;
                }
                painter.rect_filled(
                    visible,
                    egui::CornerRadius::ZERO,
                    Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                );
                let galley = painter.layout_no_wrap(
                    item_name.to_owned(),
                    label_font.clone(),
                    Color32::WHITE,
                );
                let text_pos = pos2(
                    label_rect.left() + 3.0,
                    label_rect.center().y - galley.size().y * 0.5,
                );
                painter
                    .with_clip_rect(visible)
                    .galley(text_pos, galley, Color32::PLACEHOLDER);
            }
        }
    }

    if let Some(ref row_pool_id) = state.row_pool_id {
        let row_pool = app
            .shell
            .resource_pools
            .iter()
            .find(|p| p.node_id == *row_pool_id);
        for row in 0..state.logical_rows {
            let fallback = format!("{row}");
            let item_name = row_pool
                .and_then(|p| p.item_names.get(row))
                .map(|s| s.as_str())
                .unwrap_or(&fallback);
            let display_row = row * row_chunks_per_logical_row;
            let row_gap = matrix_row_gap_px(state);
            let local_y = display_row as f32 * (cell_h + row_gap);
            let screen_y = image_rect.min.y + local_y * zoom;
            let chunk_count = row_chunks_per_logical_row;
            let lane_h = (cell_h * chunk_count as f32
                + row_gap * chunk_count.saturating_sub(1) as f32)
                * zoom;
            let bg_rect = Rect::from_min_size(
                pos2(image_rect.min.x - row_header_w - 2.0, screen_y),
                egui::vec2(row_header_w, lane_h),
            );
            let visible = bg_rect.intersect(canvas_rect);
            if !visible.is_positive() {
                continue;
            }
            painter.rect_filled(visible, egui::CornerRadius::ZERO, Color32::BLACK);
            let galley =
                painter.layout_no_wrap(item_name.to_owned(), label_font.clone(), Color32::WHITE);
            let text_origin = pos2(
                bg_rect.center().x - galley.size().y * 0.5,
                bg_rect.center().y + galley.size().x * 0.5,
            );
            let rotated = egui::epaint::TextShape {
                pos: text_origin,
                galley,
                override_text_color: Some(Color32::WHITE),
                underline: egui::Stroke::NONE,
                fallback_color: Color32::WHITE,
                opacity_factor: 1.0,
                angle: -std::f32::consts::FRAC_PI_2,
            };
            painter.add(rotated);
        }
    }
}

fn draw_matrix_pixel_overlays(ui: &egui::Ui, app: &mut App, canvas_rect: Rect, image_rect: Rect) {
    let zoom = app.canvas.viewport.zoom;
    if zoom < 48.0 {
        return;
    }

    let state = &mut app.shell.matrix_state;
    if state.cells.is_empty() {
        return;
    }
    let cell_w = state.cell_resolution[0] as f32;
    let cell_h = state.cell_resolution[1] as f32;
    let resolution = state.cell_resolution;
    let row_gap = matrix_row_gap_px(state);

    for cell in &mut state.cells {
        let local_x = cell.display_coord.col as f32 * (cell_w + MATRIX_GRID_GAP_PX);
        let local_y = cell.display_coord.row as f32 * (cell_h + row_gap);
        let cell_image_rect = Rect::from_min_size(
            pos2(
                image_rect.min.x + local_x * zoom,
                image_rect.min.y + local_y * zoom,
            ),
            egui::vec2(cell_w * zoom, cell_h * zoom),
        );

        if !cell_image_rect.intersects(canvas_rect) {
            continue;
        }

        matrix_render::ensure_cell_pixel_cache(cell);
        let cache = cell.pixel_cache.as_ref();
        draw_pixel_overlay(
            ui,
            cell_image_rect,
            canvas_rect,
            zoom,
            resolution,
            cache,
            None,
            crate::app::types::DiffMetricMode::AE,
            false,
            false,
        );
    }
}

fn maybe_sample_matrix_clicked_pixel(
    app: &mut App,
    ctx: &egui::Context,
    response: &egui::Response,
    canvas_rect: Rect,
    image_rect: Rect,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    frame_result: &mut CanvasFrameResult,
) {
    if !response.clicked_by(egui::PointerButton::Primary) {
        return;
    }
    let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    if !canvas_rect.contains(pointer_pos) {
        return;
    }

    let zoom = app.canvas.viewport.zoom;
    let Some(coord) = matrix_hit_test(&app.shell.matrix_state, pointer_pos, image_rect, zoom)
    else {
        return;
    };
    let cell_w = app.shell.matrix_state.cell_resolution[0] as f32;
    let cell_h = app.shell.matrix_state.cell_resolution[1] as f32;
    let Some(cell_image_rect) =
        matrix_cell_screen_rect(&app.shell.matrix_state, coord, image_rect, zoom)
    else {
        return;
    };
    let local = (pointer_pos - cell_image_rect.min) / cell_image_rect.size();
    let x = (local.x * cell_w).floor() as u32;
    let y = (local.y * cell_h).floor() as u32;
    if x >= app.shell.matrix_state.cell_resolution[0]
        || y >= app.shell.matrix_state.cell_resolution[1]
    {
        return;
    }

    let Some(cell) = app
        .shell
        .matrix_state
        .cells
        .iter_mut()
        .find(|c| c.coord == coord)
    else {
        return;
    };
    matrix_render::ensure_cell_pixel_cache(cell);
    let Some(cache) = cell.pixel_cache.as_ref() else {
        return;
    };
    let Some(rgba) = pixel_overlay::sample_value_pixel(
        cache,
        x,
        y,
        None,
        crate::app::types::DiffMetricMode::AE,
        false,
        false,
    ) else {
        return;
    };
    apply_action(
        frame_result,
        app,
        render_state,
        renderer,
        CanvasAction::SamplePixel { x, y, rgba },
    );
}
