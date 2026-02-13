use std::{borrow::Cow, path::Path, sync::mpsc};

use rust_wgpu_fiber::eframe::{
    egui::{self, Color32, Rect, pos2},
    egui_wgpu, wgpu,
};

use crate::ui::{
    animation_manager::{AnimationSpec, Easing},
    viewport_indicators::{
        VIEWPORT_INDICATOR_GAP, VIEWPORT_INDICATOR_ITEM_SIZE, VIEWPORT_INDICATOR_RIGHT_PAD,
        VIEWPORT_INDICATOR_TOP_PAD, ViewportIndicator, ViewportIndicatorKind,
        draw_viewport_indicator_at,
    },
};

use super::{
    layout_math::{clamp_zoom, lerp},
    texture_bridge,
    types::{
        AnalysisTab, App, RefImageMode, RefImageSource, RefImageState, SIDEBAR_ANIM_SECS,
        UiWindowMode, ViewportCopyIndicator, ViewportCopyIndicatorVisual,
    },
    window_mode::WindowModeFrame,
};

const ANIM_KEY_PAN_ZOOM_FACTOR: &str = "ui.canvas.pan_zoom.factor";

fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    let a = ((color.a() as f32) * alpha.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

fn is_supported_reference_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
        .unwrap_or(false)
}

fn resolve_reference_image_path(path: &str) -> Option<std::path::PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }

    let as_path = Path::new(trimmed);
    if as_path.is_absolute() {
        return Some(as_path.to_path_buf());
    }

    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(as_path));
    }
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    candidates.push(repo_root.join(as_path));
    candidates.push(repo_root.join("assets").join(as_path));

    candidates
        .into_iter()
        .find(|candidate| candidate.exists())
        .or_else(|| Some(repo_root.join(as_path)))
}

fn load_reference_image_from_path(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    path: &Path,
    source: RefImageSource,
) -> anyhow::Result<()> {
    if !is_supported_reference_image(path) {
        anyhow::bail!("unsupported reference image extension: {}", path.display());
    }

    let decoded = image::open(path)?;
    let rgba = decoded.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    let rgba_bytes = rgba.into_raw();

    let color_image =
        egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba_bytes);
    let texture = ctx.load_texture(
        format!("reference:{}", path.display()),
        color_image,
        egui::TextureOptions::NEAREST,
    );

    let wgpu_texture = render_state
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("sys.reference.image"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

    app.shader_space.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &wgpu_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba_bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    let wgpu_texture_view = wgpu_texture.create_view(&wgpu::TextureViewDescriptor::default());

    clear_reference(app, renderer);
    app.ref_image = Some(RefImageState {
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("reference")
            .to_string(),
        rgba_bytes,
        texture,
        wgpu_texture,
        wgpu_texture_view,
        size: [width, height],
        offset: egui::Vec2::ZERO,
        mode: RefImageMode::Overlay,
        opacity: 0.5,
        drag_start: None,
        drag_start_offset: egui::Vec2::ZERO,
        source,
    });
    app.diff_dirty = true;
    app.analysis_dirty = true;
    app.clipping_dirty = true;
    Ok(())
}

fn load_reference_image_from_bytes(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    rgba_bytes: Vec<u8>,
    width: u32,
    height: u32,
    name: String,
    source: RefImageSource,
) {
    let color_image =
        egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba_bytes);
    let texture = ctx.load_texture(
        format!("reference:{}", name),
        color_image,
        egui::TextureOptions::NEAREST,
    );

    let wgpu_texture = render_state
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("sys.reference.image"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

    app.shader_space.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &wgpu_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba_bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    let wgpu_texture_view = wgpu_texture.create_view(&wgpu::TextureViewDescriptor::default());

    clear_reference(app, renderer);
    app.ref_image = Some(RefImageState {
        name,
        rgba_bytes,
        texture,
        wgpu_texture,
        wgpu_texture_view,
        size: [width, height],
        offset: egui::Vec2::ZERO,
        mode: RefImageMode::Overlay,
        opacity: 0.5,
        drag_start: None,
        drag_start_offset: egui::Vec2::ZERO,
        source,
    });
    app.diff_dirty = true;
    app.analysis_dirty = true;
    app.clipping_dirty = true;
}

pub(super) fn sync_reference_image_from_scene(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
) {
    let desired_data_url = app.scene_reference_image_data_url.clone();
    let desired_path = app.scene_reference_image_path.clone();

    if let Some(data_url) = desired_data_url {
        let already_loaded = matches!(
            app.ref_image.as_ref().map(|r| &r.source),
            Some(RefImageSource::SceneNodeDataUrl(v)) if v == &data_url
        );
        if already_loaded {
            return;
        }
        let attempt_key = format!("dataurl:{}", data_url);
        if app.last_auto_reference_attempt.as_deref() == Some(attempt_key.as_str()) {
            return;
        }
        app.last_auto_reference_attempt = Some(attempt_key);

        match crate::renderer::utils::load_image_from_data_url(&data_url) {
            Ok(decoded) => {
                let rgba = decoded.to_rgba8();
                let width = rgba.width();
                let height = rgba.height();
                let rgba_bytes = rgba.into_raw();
                load_reference_image_from_bytes(
                    app,
                    ctx,
                    render_state,
                    renderer,
                    rgba_bytes,
                    width,
                    height,
                    "ReferenceImage(dataUrl)".to_string(),
                    RefImageSource::SceneNodeDataUrl(data_url),
                );
            }
            Err(e) => {
                eprintln!("[reference-image] failed to decode ReferenceImage.dataUrl: {e:#}");
            }
        }
        return;
    }

    match desired_path {
        Some(path) => {
            let already_loaded = matches!(
                app.ref_image.as_ref().map(|r| &r.source),
                Some(RefImageSource::SceneNodePath(p)) if p == &path
            );
            if already_loaded {
                return;
            }
            if app.last_auto_reference_attempt.as_deref() == Some(path.as_str()) {
                return;
            }

            app.last_auto_reference_attempt = Some(path.clone());
            let Some(resolved_path) = resolve_reference_image_path(&path) else {
                return;
            };

            if let Err(e) = load_reference_image_from_path(
                app,
                ctx,
                render_state,
                renderer,
                &resolved_path,
                RefImageSource::SceneNodePath(path.clone()),
            ) {
                eprintln!(
                    "[reference-image] failed to load ReferenceImage.path='{}' (resolved '{}'): {e:#}",
                    path,
                    resolved_path.display()
                );
            }
        }
        None => {
            app.last_auto_reference_attempt = None;
            if matches!(
                app.ref_image.as_ref().map(|r| &r.source),
                Some(RefImageSource::SceneNodePath(_) | RefImageSource::SceneNodeDataUrl(_))
            ) {
                clear_reference(app, renderer);
            }
        }
    }
}

pub(super) fn clear_reference(app: &mut App, renderer: &mut egui_wgpu::Renderer) {
    app.ref_image = None;
    app.diff_renderer = None;
    app.diff_stats = None;
    app.diff_dirty = false;
    app.analysis_dirty = true;
    app.clipping_dirty = true;
    if let Some(id) = app.diff_texture_id.take() {
        renderer.free_texture(&id);
    }
}

fn maybe_handle_reference_drop(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
) {
    let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
    for file in dropped_files {
        let Some(path) = file.path else {
            continue;
        };
        if !is_supported_reference_image(&path) {
            continue;
        }

        if load_reference_image_from_path(
            app,
            ctx,
            render_state,
            renderer,
            &path,
            RefImageSource::Manual,
        )
        .is_ok()
        {
            app.last_auto_reference_attempt = None;
            break;
        }
    }
}

pub fn show_canvas_panel(
    app: &mut App,
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    frame: WindowModeFrame,
    now: f64,
) -> bool {
    let mut requested_toggle_canvas_only = false;

    if let Some(rx) = app.viewport_copy_job_rx.as_ref() {
        match rx.try_recv() {
            Ok(success) => {
                app.viewport_copy_job_rx = None;
                if success {
                    app.viewport_copy_indicator =
                        ViewportCopyIndicator::Success { hide_at: now + 1.0 };
                    app.viewport_copy_last_visual = Some(ViewportCopyIndicatorVisual::Success);
                } else {
                    app.viewport_copy_indicator =
                        ViewportCopyIndicator::Failure { hide_at: now + 1.0 };
                    app.viewport_copy_last_visual = Some(ViewportCopyIndicatorVisual::Failure);
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                app.viewport_copy_job_rx = None;
                app.viewport_copy_indicator = ViewportCopyIndicator::Failure { hide_at: now + 1.0 };
                app.viewport_copy_last_visual = Some(ViewportCopyIndicatorVisual::Failure);
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
    match app.viewport_copy_indicator {
        ViewportCopyIndicator::Success { hide_at } | ViewportCopyIndicator::Failure { hide_at } => {
            if now >= hide_at {
                app.viewport_copy_indicator = ViewportCopyIndicator::Hidden;
            }
        }
        _ => {}
    }

    if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::F)) {
        requested_toggle_canvas_only = true;
    }

    if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        app.time_updates_enabled = !app.time_updates_enabled;
        if app.scene_uses_time {
            if matches!(
                app.ref_image.as_ref().map(|r| r.mode),
                Some(RefImageMode::Diff)
            ) {
                app.diff_dirty = true;
            }
            app.analysis_dirty = true;
            app.clipping_dirty = true;
        }
    }

    maybe_handle_reference_drop(app, ctx, render_state, renderer);

    if app.preview_texture_name.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.preview_texture_name = None;
        app.file_tree_state.selected_id = None;
        if let Some(id) = app.preview_color_attachment.take() {
            renderer.free_texture(&id);
        }
    }

    // Sync preview texture if active.
    let using_preview = if let Some(preview_name) = app.preview_texture_name.clone() {
        // Check the texture still exists.
        if app
            .shader_space
            .textures
            .contains_key(preview_name.as_str())
        {
            texture_bridge::sync_preview_texture(
                app,
                render_state,
                renderer,
                &preview_name,
                app.texture_filter,
            );
            true
        } else {
            // Texture gone — clear preview.
            app.preview_texture_name = None;
            if let Some(id) = app.preview_color_attachment.take() {
                renderer.free_texture(&id);
            }
            false
        }
    } else {
        // Preview was cleared — free the attachment if it's still registered.
        if let Some(id) = app.preview_color_attachment.take() {
            renderer.free_texture(&id);
        }
        false
    };

    let avail_rect = ui.available_rect_before_wrap();

    // Use preview texture resolution when previewing.
    let effective_resolution = if using_preview {
        if let Some(ref pn) = app.preview_texture_name {
            if let Some(info) = app.shader_space.texture_info(pn.as_str()) {
                [info.size.width, info.size.height]
            } else {
                app.resolution
            }
        } else {
            app.resolution
        }
    } else {
        app.resolution
    };
    let image_size = egui::vec2(
        effective_resolution[0] as f32,
        effective_resolution[1] as f32,
    );

    let animated_canvas_rect = avail_rect;

    let prev_center = app
        .canvas_center_prev
        .unwrap_or(animated_canvas_rect.center());
    let new_center = animated_canvas_rect.center();
    app.pan += prev_center - new_center;

    let fit_zoom = (animated_canvas_rect.width() / image_size.x)
        .min(animated_canvas_rect.height() / image_size.y)
        .max(0.01);

    if !app.zoom_initialized {
        app.zoom = fit_zoom;
        app.zoom_initialized = true;
        app.min_zoom = Some(fit_zoom);
        app.pan_zoom_target_zoom = fit_zoom;
    }
    let min_zoom = app.min_zoom.unwrap_or(fit_zoom);

    if frame.prev_mode != frame.mode {
        // Animate pan/zoom smoothly during mode transition.
        // Preserve the user's current pan/zoom in both directions —
        // no longer reset to fit-zoom when entering Sidebar mode.
        let (start_zoom, start_pan, target_zoom, target_pan) = match frame.mode {
            UiWindowMode::Sidebar => (
                app.zoom,
                app.pan,
                if app.pan_zoom_target_zoom > 0.0 {
                    app.pan_zoom_target_zoom
                } else {
                    app.zoom
                },
                app.pan_zoom_target_pan,
            ),
            UiWindowMode::CanvasOnly => (
                app.zoom,
                app.pan,
                if app.pan_zoom_target_zoom > 0.0 {
                    app.pan_zoom_target_zoom
                } else {
                    app.zoom
                },
                app.pan_zoom_target_pan,
            ),
        };
        app.pan_zoom_start_zoom = start_zoom;
        app.pan_zoom_start_pan = start_pan;
        app.pan_zoom_target_zoom = target_zoom;
        app.pan_zoom_target_pan = target_pan;
        app.animations.start(
            ANIM_KEY_PAN_ZOOM_FACTOR,
            AnimationSpec {
                from: 0.0f32,
                to: 1.0f32,
                duration_secs: SIDEBAR_ANIM_SECS,
                easing: Easing::EaseOutCubic,
            },
            now,
        );
    }

    if let Some((factor, done)) = app.animations.sample_f32(ANIM_KEY_PAN_ZOOM_FACTOR, now) {
        app.zoom = lerp(app.pan_zoom_start_zoom, app.pan_zoom_target_zoom, factor);
        app.pan =
            app.pan_zoom_start_pan + (app.pan_zoom_target_pan - app.pan_zoom_start_pan) * factor;
        app.pan_start = None;
        if done {
            app.zoom = app.pan_zoom_target_zoom;
            app.pan = app.pan_zoom_target_pan;
        }
    }

    let pan_zoom_animating = app.animations.is_active(ANIM_KEY_PAN_ZOOM_FACTOR);
    let pan_zoom_enabled = !pan_zoom_animating;
    let effective_min_zoom = if pan_zoom_animating { 0.01 } else { min_zoom };

    if pan_zoom_enabled {
        app.pan_zoom_target_zoom = app.zoom;
        app.pan_zoom_target_pan = app.pan;
    }

    let zoom = clamp_zoom(app.zoom, effective_min_zoom);
    app.zoom = zoom;
    let draw_size = image_size * zoom;
    let base_min = animated_canvas_rect.center() - draw_size * 0.5;
    let mut image_rect = Rect::from_min_size(base_min + app.pan, draw_size);

    let response = ui.allocate_rect(avail_rect, egui::Sense::click_and_drag());

    if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
        ui.painter().rect_filled(
            animated_canvas_rect,
            egui::CornerRadius::same(8),
            Color32::from_rgba_unmultiplied(80, 130, 255, 36),
        );
        ui.painter().rect_stroke(
            animated_canvas_rect.shrink(2.0),
            egui::CornerRadius::same(8),
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 170, 255, 220)),
            egui::StrokeKind::Outside,
        );
        ui.painter().text(
            animated_canvas_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Drop PNG / JPEG as Reference",
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
            Color32::from_rgba_unmultiplied(214, 228, 255, 240),
        );
    }

    let active_texture_name = if using_preview {
        app.preview_texture_name
            .as_ref()
            .map(|n| n.as_str().to_string())
            .unwrap_or_else(|| app.output_texture_name.as_str().to_string())
    } else {
        app.output_texture_name.as_str().to_string()
    };
    response.context_menu(|ui| {
        if ui.button("复制材质").clicked() {
            if let Some(info) = app.shader_space.texture_info(active_texture_name.as_str())
                && let Ok(image) = app
                    .shader_space
                    .read_texture_rgba8(active_texture_name.as_str())
            {
                let width = info.size.width as usize;
                let height = info.size.height as usize;
                let bytes = image.bytes;
                let (tx, rx) = mpsc::channel::<bool>();
                app.viewport_copy_job_rx = Some(rx);
                app.viewport_copy_indicator = ViewportCopyIndicator::InProgress;
                app.viewport_copy_last_visual = Some(ViewportCopyIndicatorVisual::InProgress);

                std::thread::spawn(move || {
                    let copied = arboard::Clipboard::new()
                        .and_then(|mut clipboard| {
                            clipboard.set_image(arboard::ImageData {
                                width,
                                height,
                                bytes: Cow::Owned(bytes),
                            })
                        })
                        .is_ok();
                    let _ = tx.send(copied);
                });
            }
            ui.close();
        }
    });

    if pan_zoom_enabled && ctx.input(|i| i.key_pressed(egui::Key::R)) {
        app.zoom = fit_zoom;
        app.pan = egui::Vec2::ZERO;
        app.pan_start = None;
        app.pan_zoom_target_zoom = fit_zoom;
        app.pan_zoom_target_pan = egui::Vec2::ZERO;
        let draw_size = image_size * app.zoom;
        let base_min = animated_canvas_rect.center() - draw_size * 0.5;
        image_rect = Rect::from_min_size(base_min, draw_size);
    }

    if app.pending_view_reset {
        app.zoom = fit_zoom;
        app.pan = egui::Vec2::ZERO;
        app.pan_start = None;
        app.pan_zoom_target_zoom = fit_zoom;
        app.pan_zoom_target_pan = egui::Vec2::ZERO;
        let draw_size = image_size * app.zoom;
        let base_min = animated_canvas_rect.center() - draw_size * 0.5;
        image_rect = Rect::from_min_size(base_min, draw_size);
        app.pending_view_reset = false;
    }

    if ctx.input(|i| i.key_pressed(egui::Key::P)) {
        app.texture_filter = match app.texture_filter {
            wgpu::FilterMode::Nearest => wgpu::FilterMode::Linear,
            wgpu::FilterMode::Linear => wgpu::FilterMode::Nearest,
        };
        if let Some(ref preview_name) = app.preview_texture_name.clone() {
            texture_bridge::sync_preview_texture(
                app,
                render_state,
                renderer,
                preview_name,
                app.texture_filter,
            );
        }
        let texture_name = app.output_texture_name.clone();
        texture_bridge::sync_output_texture(
            app,
            render_state,
            renderer,
            &texture_name,
            app.texture_filter,
        );
    }

    if let Some(reference) = app.ref_image.as_mut() {
        if ctx.input(|i| i.key_pressed(egui::Key::A)) && reference.offset != egui::Vec2::ZERO {
            reference.offset = egui::Vec2::ZERO;
            app.diff_dirty = true;
            app.analysis_dirty = true;
            app.clipping_dirty = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Num1)) {
            reference.opacity = 0.0;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Num2)) {
            reference.opacity = 1.0;
        }
    }

    let shift_down = ctx.input(|i| i.modifiers.shift);
    if shift_down && !app.shift_was_down {
        if let Some(reference) = app.ref_image.as_mut() {
            reference.mode = match reference.mode {
                RefImageMode::Overlay => RefImageMode::Diff,
                RefImageMode::Diff => RefImageMode::Overlay,
            };
            app.diff_dirty = true;
            app.analysis_dirty = true;
            app.clipping_dirty = true;
        }
    }
    app.shift_was_down = shift_down;

    if pan_zoom_enabled {
        // Pan with middle mouse button drag.
        if response.drag_started_by(egui::PointerButton::Middle) {
            if let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) {
                app.pan_start = Some(pointer_pos);
            }
        }
        if response.dragged_by(egui::PointerButton::Middle) {
            if let (Some(start), Some(pointer_pos)) =
                (app.pan_start, ctx.input(|i| i.pointer.hover_pos()))
            {
                app.pan += pointer_pos - start;
                app.pan_start = Some(pointer_pos);
                image_rect = Rect::from_min_size(base_min + app.pan, draw_size);
            }
        } else if !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle)) {
            app.pan_start = None;
        }

        // Primary drag moves reference image when available; otherwise pans canvas.
        if app.ref_image.is_some() {
            if response.drag_started_by(egui::PointerButton::Primary)
                && let Some(reference) = app.ref_image.as_mut()
                && let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos())
            {
                reference.drag_start = Some(pointer_pos);
                reference.drag_start_offset = reference.offset;
            }
            if response.dragged_by(egui::PointerButton::Primary)
                && let Some(reference) = app.ref_image.as_mut()
                && let (Some(start), Some(pointer_pos)) =
                    (reference.drag_start, ctx.input(|i| i.pointer.hover_pos()))
            {
                let delta = (pointer_pos - start) / app.zoom.max(0.000_1);
                let next_offset = egui::vec2(
                    (reference.drag_start_offset.x + delta.x).round(),
                    (reference.drag_start_offset.y + delta.y).round(),
                );
                if reference.offset != next_offset {
                    reference.offset = next_offset;
                    app.diff_dirty = true;
                    app.analysis_dirty = true;
                    app.clipping_dirty = true;
                }
            } else if !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Primary))
                && let Some(reference) = app.ref_image.as_mut()
            {
                reference.drag_start = None;
            }
        } else {
            // Pan with primary button drag (for trackpad users).
            if response.drag_started_by(egui::PointerButton::Primary) {
                if let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) {
                    app.pan_start = Some(pointer_pos);
                }
            }
            if response.dragged_by(egui::PointerButton::Primary) {
                if let (Some(start), Some(pointer_pos)) =
                    (app.pan_start, ctx.input(|i| i.pointer.hover_pos()))
                {
                    app.pan += pointer_pos - start;
                    app.pan_start = Some(pointer_pos);
                    image_rect = Rect::from_min_size(base_min + app.pan, draw_size);
                }
            } else if !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Primary))
                && !ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle))
            {
                app.pan_start = None;
            }
        }
    }

    // Only process scroll/zoom when pointer is over the canvas, so sidebar
    // scroll events don't leak into the canvas.
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

    // Pan with two-finger scroll (trackpad) when not pinch-zooming.
    if pan_zoom_enabled && zoom_delta == 1.0 && (scroll_delta.x != 0.0 || scroll_delta.y != 0.0) {
        app.pan += scroll_delta;
        image_rect = Rect::from_min_size(base_min + app.pan, draw_size);
    }

    let scroll_zoom = if zoom_delta != 1.0 { zoom_delta } else { 1.0 };
    if pan_zoom_enabled && scroll_zoom != 1.0 {
        if let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) {
            let prev_zoom = app.zoom;
            let next_zoom = clamp_zoom(prev_zoom * scroll_zoom, effective_min_zoom);
            if next_zoom != prev_zoom {
                let prev_size = image_size * prev_zoom;
                let prev_min = animated_canvas_rect.center() - prev_size * 0.5 + app.pan;
                let local = (pointer_pos - prev_min) / prev_size;
                app.zoom = next_zoom;
                let next_size = image_size * next_zoom;
                let next_min = pointer_pos - local * next_size;
                let desired_pan = next_min - (animated_canvas_rect.center() - next_size * 0.5);
                app.pan = desired_pan;
                image_rect = Rect::from_min_size(
                    animated_canvas_rect.center() - next_size * 0.5 + app.pan,
                    next_size,
                );
            }
        }
    }

    let rounding = egui::CornerRadius::ZERO;

    // Draw checkerboard background for transparency (GPU-tiled 2×2 texture).
    {
        let checker_tex = {
            let cache_id = egui::Id::new("ui.canvas.checkerboard_texture");
            if let Some(tex) = ctx.memory(|mem| mem.data.get_temp::<egui::TextureHandle>(cache_id))
            {
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
        let uv_w = animated_canvas_rect.width() / cell;
        let uv_h = animated_canvas_rect.height() / cell;
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(uv_w, uv_h));
        ui.painter().add(
            egui::epaint::RectShape::filled(animated_canvas_rect, rounding, Color32::WHITE)
                .with_texture(checker_tex.id(), uv),
        );
    }

    let image_rect_size = image_rect.size();
    let uv0_min = (animated_canvas_rect.min - image_rect.min) / image_rect_size;
    let uv0_max = (animated_canvas_rect.max - image_rect.min) / image_rect_size;
    let computed_uv = Rect::from_min_max(pos2(uv0_min.x, uv0_min.y), pos2(uv0_max.x, uv0_max.y));

    let display_attachment = if using_preview {
        app.preview_color_attachment.or(app.color_attachment)
    } else {
        app.color_attachment
    };

    if let Some(tex_id) = display_attachment {
        ui.painter().add(
            egui::epaint::RectShape::filled(animated_canvas_rect, rounding, Color32::WHITE)
                .with_texture(tex_id, computed_uv),
        );
    }

    let mut diff_visible_for_clipping: Option<(Rect, Rect)> = None;
    if let Some(reference) = app.ref_image.as_ref() {
        let reference_size = egui::vec2(reference.size[0] as f32, reference.size[1] as f32);
        let reference_min = image_rect.min + reference.offset * app.zoom;
        let reference_rect = Rect::from_min_size(reference_min, reference_size * app.zoom);
        let visible_rect = reference_rect.intersect(animated_canvas_rect);

        if visible_rect.is_positive() {
            let uv_min = (visible_rect.min - reference_rect.min) / reference_rect.size();
            let uv_max = (visible_rect.max - reference_rect.min) / reference_rect.size();
            let reference_uv =
                Rect::from_min_max(pos2(uv_min.x, uv_min.y), pos2(uv_max.x, uv_max.y));
            if matches!(reference.mode, RefImageMode::Diff) {
                diff_visible_for_clipping = Some((visible_rect, reference_uv));
            }

            let texture_id = if matches!(reference.mode, RefImageMode::Diff) {
                app.diff_texture_id.unwrap_or(reference.texture.id())
            } else {
                reference.texture.id()
            };

            let tint = if matches!(reference.mode, RefImageMode::Overlay) {
                Color32::from_rgba_unmultiplied(255, 255, 255, (reference.opacity * 255.0) as u8)
            } else {
                Color32::WHITE
            };

            ui.painter().add(
                egui::epaint::RectShape::filled(visible_rect, rounding, tint)
                    .with_texture(texture_id, reference_uv),
            );
        }
    }

    if matches!(app.analysis_tab, AnalysisTab::Clipping)
        && let Some(clipping_texture_id) = app.clipping_texture_id
    {
        if app.analysis_source_is_diff {
            if let Some((visible_rect, reference_uv)) = diff_visible_for_clipping {
                ui.painter().add(
                    egui::epaint::RectShape::filled(visible_rect, rounding, Color32::WHITE)
                        .with_texture(clipping_texture_id, reference_uv),
                );
            }
        } else {
            ui.painter().add(
                egui::epaint::RectShape::filled(animated_canvas_rect, rounding, Color32::WHITE)
                    .with_texture(clipping_texture_id, computed_uv),
            );
        }
    }

    let sampling_indicator = match app.texture_filter {
        wgpu::FilterMode::Nearest => ViewportIndicator {
            icon: "N",
            tooltip: "Viewport sampling: Nearest (press P to toggle Linear)",
            kind: ViewportIndicatorKind::Text,
        },
        wgpu::FilterMode::Linear => ViewportIndicator {
            icon: "L",
            tooltip: "Viewport sampling: Linear (press P to toggle Nearest)",
            kind: ViewportIndicatorKind::Text,
        },
    };
    let copy_visible = !matches!(app.viewport_copy_indicator, ViewportCopyIndicator::Hidden);
    let copy_anim_t = ctx.animate_bool(
        egui::Id::new("ui.viewport.copy_indicator.visible"),
        copy_visible,
    );
    let pause_visible = !app.time_updates_enabled;
    let pause_anim_t = ctx.animate_bool(
        egui::Id::new("ui.viewport.pause_indicator.visible"),
        pause_visible,
    );
    let clipping_visible = matches!(app.analysis_tab, AnalysisTab::Clipping);
    let clipping_anim_t = ctx.animate_bool(
        egui::Id::new("ui.viewport.clipping_indicator.visible"),
        clipping_visible,
    );
    let stats_visible = matches!(
        app.ref_image.as_ref().map(|r| r.mode),
        Some(RefImageMode::Diff)
    ) && app.diff_stats.is_some();
    let stats_anim_t = ctx.animate_bool(
        egui::Id::new("ui.viewport.diff_stats_indicator.visible"),
        stats_visible,
    );

    let copy_visual = match app.viewport_copy_indicator {
        ViewportCopyIndicator::InProgress => Some(ViewportCopyIndicatorVisual::InProgress),
        ViewportCopyIndicator::Success { .. } => Some(ViewportCopyIndicatorVisual::Success),
        ViewportCopyIndicator::Failure { .. } => Some(ViewportCopyIndicatorVisual::Failure),
        ViewportCopyIndicator::Hidden => app.viewport_copy_last_visual,
    };

    let indicator_y = animated_canvas_rect.min.y + VIEWPORT_INDICATOR_TOP_PAD;
    let copy_slot = copy_anim_t * (VIEWPORT_INDICATOR_ITEM_SIZE + VIEWPORT_INDICATOR_GAP);
    let pause_item_width = 44.0;
    let pause_slot = pause_anim_t * (pause_item_width + VIEWPORT_INDICATOR_GAP);
    let clipping_slot = clipping_anim_t * (VIEWPORT_INDICATOR_ITEM_SIZE + VIEWPORT_INDICATOR_GAP);
    let sampling_x = animated_canvas_rect.max.x
        - VIEWPORT_INDICATOR_RIGHT_PAD
        - VIEWPORT_INDICATOR_ITEM_SIZE
        - copy_slot
        - pause_slot
        - clipping_slot;
    let sampling_rect = Rect::from_min_size(
        pos2(sampling_x, indicator_y),
        egui::vec2(VIEWPORT_INDICATOR_ITEM_SIZE, VIEWPORT_INDICATOR_ITEM_SIZE),
    );
    draw_viewport_indicator_at(ui, sampling_rect, &sampling_indicator, now, 1.0);

    if clipping_anim_t > 0.001 {
        let clipping_indicator = ViewportIndicator {
            icon: "C",
            tooltip: "Clipping overlay 已开启",
            kind: ViewportIndicatorKind::Failure,
        };
        let slide_x = (1.0 - clipping_anim_t) * 8.0;
        let clipping_rect = Rect::from_min_size(
            pos2(
                sampling_rect.min.x - VIEWPORT_INDICATOR_GAP - VIEWPORT_INDICATOR_ITEM_SIZE
                    + slide_x,
                indicator_y,
            ),
            egui::vec2(VIEWPORT_INDICATOR_ITEM_SIZE, VIEWPORT_INDICATOR_ITEM_SIZE),
        );
        draw_viewport_indicator_at(ui, clipping_rect, &clipping_indicator, now, clipping_anim_t);
    }

    if pause_anim_t > 0.001 {
        let pause_indicator = ViewportIndicator {
            icon: "PAUSE",
            tooltip: "Time 更新已暂停（Space 恢复）",
            kind: ViewportIndicatorKind::Failure,
        };
        let slide_x = (1.0 - pause_anim_t) * 8.0;
        let pause_rect = Rect::from_min_size(
            pos2(
                animated_canvas_rect.max.x
                    - VIEWPORT_INDICATOR_RIGHT_PAD
                    - pause_item_width
                    - copy_slot
                    + slide_x,
                indicator_y,
            ),
            egui::vec2(pause_item_width, VIEWPORT_INDICATOR_ITEM_SIZE),
        );
        draw_viewport_indicator_at(ui, pause_rect, &pause_indicator, now, pause_anim_t);
    }

    if let Some(stats) = app.diff_stats
        && stats_anim_t > 0.001
    {
        let diff_text = format!(
            "min {:.4}  max {:.4}  avg {:.4}",
            stats.min, stats.max, stats.avg
        );
        let diff_galley = ui.painter().layout_no_wrap(
            diff_text,
            egui::FontId::new(
                10.0,
                crate::ui::typography::mi_sans_family_for_weight(500.0),
            ),
            Color32::from_rgba_unmultiplied(220, 220, 220, (220.0 * stats_anim_t) as u8),
        );
        let full_badge_w = diff_galley.size().x + 14.0;
        let slide_x = (1.0 - stats_anim_t) * 8.0;
        let badge_x = sampling_rect.min.x - VIEWPORT_INDICATOR_GAP - full_badge_w + slide_x;
        let badge_rect = Rect::from_min_size(
            pos2(badge_x, indicator_y),
            egui::vec2(full_badge_w, VIEWPORT_INDICATOR_ITEM_SIZE),
        );

        ui.painter().rect(
            badge_rect,
            egui::CornerRadius::same(6),
            Color32::from_rgba_unmultiplied(0, 0, 0, (176.0 * stats_anim_t) as u8),
            egui::Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(52, 52, 52, (220.0 * stats_anim_t) as u8),
            ),
            egui::StrokeKind::Outside,
        );
        ui.painter().galley(
            pos2(
                badge_rect.min.x + 7.0,
                badge_rect.center().y - diff_galley.size().y * 0.5,
            ),
            diff_galley,
            Color32::PLACEHOLDER,
        );
    }

    if let Some(visual) = copy_visual
        && copy_anim_t > 0.001
    {
        let copy_indicator = match visual {
            ViewportCopyIndicatorVisual::InProgress => ViewportIndicator {
                icon: "",
                tooltip: "正在复制材质到剪贴板...",
                kind: ViewportIndicatorKind::Spinner,
            },
            ViewportCopyIndicatorVisual::Success => ViewportIndicator {
                icon: "✓",
                tooltip: "复制完成",
                kind: ViewportIndicatorKind::Success,
            },
            ViewportCopyIndicatorVisual::Failure => ViewportIndicator {
                icon: "✕",
                tooltip: "复制失败",
                kind: ViewportIndicatorKind::Failure,
            },
        };
        let slide_x = (1.0 - copy_anim_t) * 8.0;
        let copy_rect = Rect::from_min_size(
            pos2(
                animated_canvas_rect.max.x
                    - VIEWPORT_INDICATOR_RIGHT_PAD
                    - VIEWPORT_INDICATOR_ITEM_SIZE
                    + slide_x,
                indicator_y,
            ),
            egui::vec2(VIEWPORT_INDICATOR_ITEM_SIZE, VIEWPORT_INDICATOR_ITEM_SIZE),
        );
        draw_viewport_indicator_at(ui, copy_rect, &copy_indicator, now, copy_anim_t);
    }

    if copy_anim_t > 0.001
        || pause_anim_t > 0.001
        || clipping_anim_t > 0.001
        || stats_anim_t > 0.001
    {
        ctx.request_repaint();
    }

    // Draw top-left tags (Preview / Reference). Both can coexist.
    {
        let badge_font = egui::FontId::new(
            11.0,
            crate::ui::typography::mi_sans_family_for_weight(500.0),
        );
        let mut badge_y = animated_canvas_rect.min.y + 8.0;
        let badge_x = animated_canvas_rect.min.x + 8.0;
        let ref_tag_visible = app
            .ref_image
            .as_ref()
            .is_some_and(|reference| reference.opacity > 0.001);
        let ref_tag_anim_t =
            ctx.animate_bool(egui::Id::new("ui.canvas.ref_tag.visible"), ref_tag_visible);

        if let Some(reference) = app.ref_image.as_ref() {
            let mode = match reference.mode {
                RefImageMode::Overlay => "Overlay",
                RefImageMode::Diff => "Abs Diff",
            };
            let badge_text = format!(
                "Ref • {} • {}×{} • α {:.2}",
                mode, reference.size[0], reference.size[1], reference.opacity,
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

        if let Some(ref preview_name) = app.preview_texture_name
            && using_preview
        {
            let badge_text =
                if let Some(info) = app.shader_space.texture_info(preview_name.as_str()) {
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

    if response.clicked_by(egui::PointerButton::Primary) {
        if let Some(pointer_pos) = ctx.input(|i| i.pointer.hover_pos()) {
            if animated_canvas_rect.contains(pointer_pos)
                && (if matches!(frame.mode, UiWindowMode::Sidebar) {
                    animated_canvas_rect.contains(pointer_pos)
                } else {
                    image_rect.contains(pointer_pos)
                })
            {
                let local = (pointer_pos - animated_canvas_rect.min) / animated_canvas_rect.size();
                let uv_x = computed_uv.min.x + local.x * computed_uv.width();
                let uv_y = computed_uv.min.y + local.y * computed_uv.height();
                let x = (uv_x * app.resolution[0] as f32).floor() as u32;
                let y = (uv_y * app.resolution[1] as f32).floor() as u32;
                if x < app.resolution[0] && y < app.resolution[1] {
                    if let Ok(image) = app
                        .shader_space
                        .read_texture_rgba8(app.output_texture_name.as_str())
                    {
                        let idx = ((y * app.resolution[0] + x) * 4) as usize;
                        if idx + 3 < image.bytes.len() {
                            app.last_sampled = Some(super::types::SampledPixel {
                                x,
                                y,
                                rgba: [
                                    image.bytes[idx],
                                    image.bytes[idx + 1],
                                    image.bytes[idx + 2],
                                    image.bytes[idx + 3],
                                ],
                            });
                        }
                    }
                }
            }
        }
    }

    app.canvas_center_prev = Some(animated_canvas_rect.center());

    requested_toggle_canvas_only
}
