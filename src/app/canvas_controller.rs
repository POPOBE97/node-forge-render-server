use std::{
    borrow::Cow,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::Path,
    sync::{Arc, mpsc},
};

use rust_wgpu_fiber::eframe::{
    egui::{self, Color32, Rect, pos2},
    egui_wgpu, wgpu,
};

use crate::ui::{
    animation_manager::{AnimationSpec, Easing},
    design_tokens,
    viewport_indicators::{
        ViewportIndicator, ViewportIndicatorEntry, ViewportIndicatorInteraction,
        ViewportIndicatorKind,
    },
};

use super::{
    layout_math::{clamp_zoom, lerp},
    texture_bridge,
    types::{
        App, DiffMetricMode, RefImageAlphaMode, RefImageMode, RefImageSource, RefImageState,
        RefImageTransferMode, SIDEBAR_ANIM_SECS, UiWindowMode, ViewportOperationIndicator,
        ViewportOperationIndicatorVisual,
    },
    window_mode::WindowModeFrame,
};

const ANIM_KEY_PAN_ZOOM_FACTOR: &str = "ui.canvas.pan_zoom.factor";
const VIEWPORT_OPERATION_TIMEOUT_SECS: f64 = 5.0;
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
const PIXEL_OVERLAY_MIN_ZOOM: f32 = 48.0;
const PIXEL_OVERLAY_CACHE_ID: &str = "ui.canvas.pixel_overlay_cache";
const PIXEL_OVERLAY_REFERENCE_ZOOM: f32 = 100.0;
const PIXEL_OVERLAY_BASE_PADDING_Y_AT_MAX_ZOOM: f32 = 10.0;
const PIXEL_OVERLAY_BASE_PADDING_X_AT_MAX_ZOOM: f32 = 18.0;
const PIXEL_OVERLAY_BASE_SHADOW_OFFSET_AT_MAX_ZOOM: f32 = 1.0;
const PIXEL_OVERLAY_BASE_LINE_HEIGHT_AT_MAX_ZOOM: f32 =
    (PIXEL_OVERLAY_REFERENCE_ZOOM - PIXEL_OVERLAY_BASE_PADDING_Y_AT_MAX_ZOOM * 2.0) / 4.0;
const PIXEL_OVERLAY_BASE_FONT_SIZE_AT_MAX_ZOOM: f32 =
    PIXEL_OVERLAY_BASE_LINE_HEIGHT_AT_MAX_ZOOM / 1.5;

pub(super) fn is_pan_zoom_animating(app: &App) -> bool {
    app.animations.is_active(ANIM_KEY_PAN_ZOOM_FACTOR)
}

fn is_hdr_clamp_effective(
    hdr_preview_clamp_enabled: bool,
    texture_format: Option<wgpu::TextureFormat>,
) -> bool {
    hdr_preview_clamp_enabled && matches!(texture_format, Some(wgpu::TextureFormat::Rgba16Float))
}

fn pixel_overlay_text_color() -> Color32 {
    Color32::from_rgba_unmultiplied(245, 245, 245, 242)
}

fn pixel_overlay_shadow_color() -> Color32 {
    Color32::from_rgba_unmultiplied(0, 0, 0, 214)
}

fn pixel_overlay_channel_color(channel: char) -> Color32 {
    match channel {
        'r' => Color32::from_rgba_unmultiplied(255, 0, 0, 242),
        'g' => Color32::from_rgba_unmultiplied(0, 255, 0, 242),
        'b' => Color32::from_rgba_unmultiplied(0, 120, 255, 242),
        _ => pixel_overlay_text_color(),
    }
}

#[derive(Clone, Debug)]
enum PixelOverlayReadback {
    Rgba8(Vec<u8>),
    Rgba16f(Vec<f32>),
    Unavailable,
    UnsupportedFormat,
}

#[derive(Clone, Debug)]
struct PixelOverlayCache {
    texture_name: String,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    readback: PixelOverlayReadback,
}

#[derive(Clone, Copy, Debug)]
struct ValueSamplingReference<'a> {
    mode: RefImageMode,
    offset_px: [i32; 2],
    size: [u32; 2],
    opacity: f32,
    linear_premul_rgba: &'a [f32],
}

fn value_sampling_reference_from_state(reference: &RefImageState) -> ValueSamplingReference<'_> {
    ValueSamplingReference {
        mode: reference.mode,
        offset_px: [
            reference.offset.x.round() as i32,
            reference.offset.y.round() as i32,
        ],
        size: reference.size,
        opacity: reference.opacity,
        linear_premul_rgba: reference.linear_premul_rgba.as_slice(),
    }
}

fn pixel_overlay_cache_id() -> egui::Id {
    egui::Id::new(PIXEL_OVERLAY_CACHE_ID)
}

fn should_refresh_pixel_overlay_cache(
    cache: &PixelOverlayCache,
    texture_name: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> bool {
    cache.texture_name != texture_name
        || cache.width != width
        || cache.height != height
        || cache.format != format
}

fn read_pixel_overlay_cache(
    app: &App,
    texture_name: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> Arc<PixelOverlayCache> {
    let readback = match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => app
            .shader_space
            .read_texture_rgba8(texture_name)
            .map(|image| PixelOverlayReadback::Rgba8(image.bytes))
            .unwrap_or(PixelOverlayReadback::Unavailable),
        wgpu::TextureFormat::Rgba16Float => app
            .shader_space
            .read_texture_rgba16f(texture_name)
            .map(|image| PixelOverlayReadback::Rgba16f(image.channels))
            .unwrap_or(PixelOverlayReadback::Unavailable),
        _ => PixelOverlayReadback::UnsupportedFormat,
    };

    Arc::new(PixelOverlayCache {
        texture_name: texture_name.to_string(),
        width,
        height,
        format,
        readback,
    })
}

fn hash_key<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn pixel_overlay_request_key(
    texture_name: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> u64 {
    hash_key(&(texture_name, width, height, format))
}

fn get_or_refresh_pixel_overlay_cache(
    app: &mut App,
    ctx: &egui::Context,
    texture_name: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> Arc<PixelOverlayCache> {
    let request_key = pixel_overlay_request_key(texture_name, width, height, format);
    let request_key_changed = app.pixel_overlay_last_request_key != Some(request_key);
    let cache_id = pixel_overlay_cache_id();
    let cached = ctx.memory(|mem| mem.data.get_temp::<Arc<PixelOverlayCache>>(cache_id));
    let should_refresh = app.pixel_overlay_dirty
        || request_key_changed
        || cached.as_ref().is_none_or(|existing| {
            should_refresh_pixel_overlay_cache(existing, texture_name, width, height, format)
        });

    if !should_refresh && let Some(existing) = cached {
        return existing;
    }

    let updated = read_pixel_overlay_cache(app, texture_name, width, height, format);
    app.pixel_overlay_last_request_key = Some(request_key);
    app.pixel_overlay_dirty = false;
    ctx.memory_mut(|mem| mem.data.insert_temp(cache_id, updated.clone()));
    updated
}

fn mark_pixel_overlay_dirty(app: &mut App) {
    app.pixel_overlay_dirty = true;
}

fn clear_pixel_overlay_cache(ctx: &egui::Context) {
    let cache_id = pixel_overlay_cache_id();
    ctx.memory_mut(|mem| mem.data.remove::<Arc<PixelOverlayCache>>(cache_id));
}

fn format_overlay_channel(_label: char, value: f32) -> String {
    let normalized = if value.abs() < 0.0000005 { 0.0 } else { value };
    if !normalized.is_finite() {
        return format!("{normalized}");
    }

    let abs = normalized.abs();
    let integer_digits = if abs < 1.0 {
        1usize
    } else {
        abs.log10().floor() as usize + 1
    };
    let sign_chars = usize::from(normalized.is_sign_negative());
    let precision = 7usize.saturating_sub(sign_chars + integer_digits);

    format!("{normalized:.precision$}")
}

fn rgba8_to_rgba_f32(rgba: [u8; 4]) -> [f32; 4] {
    [
        rgba[0] as f32 / 255.0,
        rgba[1] as f32 / 255.0,
        rgba[2] as f32 / 255.0,
        rgba[3] as f32 / 255.0,
    ]
}

fn sample_rgba8_pixel(bytes: &[u8], width: u32, height: u32, x: u32, y: u32) -> Option<[f32; 4]> {
    if x >= width || y >= height {
        return None;
    }
    let pixel_index = y.checked_mul(width)?.checked_add(x)?;
    let idx = (pixel_index as usize).checked_mul(4)?;
    if idx + 3 >= bytes.len() {
        return None;
    }
    Some(rgba8_to_rgba_f32([
        bytes[idx],
        bytes[idx + 1],
        bytes[idx + 2],
        bytes[idx + 3],
    ]))
}

#[cfg(test)]
fn rgba16unorm_to_rgba_f32(rgba: [u16; 4]) -> [f32; 4] {
    [
        rgba[0] as f32 / 65535.0,
        rgba[1] as f32 / 65535.0,
        rgba[2] as f32 / 65535.0,
        rgba[3] as f32 / 65535.0,
    ]
}

#[cfg(test)]
fn sample_rgba16unorm_pixel(
    channels: &[u16],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
) -> Option<[f32; 4]> {
    if x >= width || y >= height {
        return None;
    }
    let pixel_index = y.checked_mul(width)?.checked_add(x)?;
    let idx = (pixel_index as usize).checked_mul(4)?;
    if idx + 3 >= channels.len() {
        return None;
    }
    Some(rgba16unorm_to_rgba_f32([
        channels[idx],
        channels[idx + 1],
        channels[idx + 2],
        channels[idx + 3],
    ]))
}

fn sample_rgba16f_pixel(
    channels: &[f32],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
) -> Option<[f32; 4]> {
    if x >= width || y >= height {
        return None;
    }
    let pixel_index = y.checked_mul(width)?.checked_add(x)?;
    let idx = (pixel_index as usize).checked_mul(4)?;
    if idx + 3 >= channels.len() {
        return None;
    }
    Some([
        channels[idx],
        channels[idx + 1],
        channels[idx + 2],
        channels[idx + 3],
    ])
}

fn sample_overlay_pixel(cache: &PixelOverlayCache, x: u32, y: u32) -> Option<[f32; 4]> {
    match &cache.readback {
        PixelOverlayReadback::Rgba8(bytes) => {
            sample_rgba8_pixel(bytes.as_slice(), cache.width, cache.height, x, y)
        }
        PixelOverlayReadback::Rgba16f(channels) => {
            sample_rgba16f_pixel(channels.as_slice(), cache.width, cache.height, x, y)
        }
        PixelOverlayReadback::Unavailable | PixelOverlayReadback::UnsupportedFormat => None,
    }
}

fn sample_reference_pixel_rgba(
    reference: ValueSamplingReference<'_>,
    x: u32,
    y: u32,
) -> Option<[f32; 4]> {
    let rx = x as i32 - reference.offset_px[0];
    let ry = y as i32 - reference.offset_px[1];
    if rx < 0 || ry < 0 {
        return None;
    }
    sample_rgba16f_pixel(
        reference.linear_premul_rgba,
        reference.size[0],
        reference.size[1],
        rx as u32,
        ry as u32,
    )
}

fn compose_reference_over_base(
    base_rgba: [f32; 4],
    reference_rgba: [f32; 4],
    reference_opacity: f32,
) -> [f32; 4] {
    let opacity = reference_opacity.clamp(0.0, 1.0);
    let src_rgba = [
        reference_rgba[0] * opacity,
        reference_rgba[1] * opacity,
        reference_rgba[2] * opacity,
        reference_rgba[3] * opacity,
    ];
    let src_a = src_rgba[3];
    if src_a <= 0.0 {
        return base_rgba;
    }

    let inv_src_a = 1.0 - src_a;
    let out_a = src_rgba[3] + base_rgba[3] * inv_src_a;
    let out_r = src_rgba[0] + base_rgba[0] * inv_src_a;
    let out_g = src_rgba[1] + base_rgba[1] * inv_src_a;
    let out_b = src_rgba[2] + base_rgba[2] * inv_src_a;

    [out_r, out_g, out_b, out_a]
}

fn compute_diff_metric_rgba(
    render_rgba: [f32; 4],
    reference_rgba: [f32; 4],
    metric_mode: DiffMetricMode,
) -> [f32; 4] {
    let delta = [
        render_rgba[0] - reference_rgba[0],
        render_rgba[1] - reference_rgba[1],
        render_rgba[2] - reference_rgba[2],
        render_rgba[3] - reference_rgba[3],
    ];
    let eps = 1e-5_f32;
    match metric_mode {
        DiffMetricMode::E => delta,
        DiffMetricMode::AE => [
            delta[0].abs(),
            delta[1].abs(),
            delta[2].abs(),
            delta[3].abs(),
        ],
        DiffMetricMode::SE => [
            delta[0] * delta[0],
            delta[1] * delta[1],
            delta[2] * delta[2],
            delta[3] * delta[3],
        ],
        DiffMetricMode::RAE => [
            delta[0].abs() / reference_rgba[0].abs().max(eps),
            delta[1].abs() / reference_rgba[1].abs().max(eps),
            delta[2].abs() / reference_rgba[2].abs().max(eps),
            delta[3].abs() / reference_rgba[3].abs().max(eps),
        ],
        DiffMetricMode::RSE => [
            (delta[0] * delta[0]) / (reference_rgba[0] * reference_rgba[0]).max(eps),
            (delta[1] * delta[1]) / (reference_rgba[1] * reference_rgba[1]).max(eps),
            (delta[2] * delta[2]) / (reference_rgba[2] * reference_rgba[2]).max(eps),
            (delta[3] * delta[3]) / (reference_rgba[3] * reference_rgba[3]).max(eps),
        ],
    }
}

fn format_diff_stat_value(value: f32) -> String {
    if !value.is_finite() {
        return format!("{value}");
    }
    let abs = value.abs();
    if abs >= 1.0e-3 && abs < 1.0e4 {
        format!("{value:.4}")
    } else {
        format!("{value:.3e}")
    }
}

fn sample_value_pixel(
    base_cache: &PixelOverlayCache,
    x: u32,
    y: u32,
    reference: Option<ValueSamplingReference<'_>>,
    diff_metric_mode: DiffMetricMode,
    diff_output_active: bool,
    clamp_output: bool,
) -> Option<[f32; 4]> {
    let base_rgba = sample_overlay_pixel(base_cache, x, y)?;
    let Some(reference) = reference else {
        return Some(base_rgba);
    };
    let Some(reference_rgba) = sample_reference_pixel_rgba(reference, x, y) else {
        return Some(base_rgba);
    };

    match reference.mode {
        RefImageMode::Overlay => {
            let mut out = compose_reference_over_base(base_rgba, reference_rgba, reference.opacity);
            if diff_output_active && clamp_output {
                out = out.map(|v| v.clamp(0.0, 1.0));
            }
            Some(out)
        }
        RefImageMode::Diff => {
            if diff_output_active {
                let mut out = compute_diff_metric_rgba(base_rgba, reference_rgba, diff_metric_mode);
                if clamp_output {
                    out = out.map(|v| v.clamp(0.0, 1.0));
                }
                out[3] = 1.0;
                Some(out)
            } else {
                Some(reference_rgba)
            }
        }
    }
}

fn draw_pixel_overlay(
    ui: &egui::Ui,
    image_rect: Rect,
    canvas_rect: Rect,
    zoom: f32,
    resolution: [u32; 2],
    cache: Option<&PixelOverlayCache>,
    reference: Option<ValueSamplingReference<'_>>,
    diff_metric_mode: DiffMetricMode,
    diff_output_active: bool,
    clamp_output: bool,
) {
    if zoom < PIXEL_OVERLAY_MIN_ZOOM {
        return;
    }
    let [width, height] = resolution;
    if width == 0 || height == 0 {
        return;
    }

    let visible_image_rect = image_rect.intersect(canvas_rect);
    if !visible_image_rect.is_positive() {
        return;
    }

    let pixel_size = zoom.max(0.000_1);
    let x_start = (((visible_image_rect.min.x - image_rect.min.x) / pixel_size)
        .floor()
        .max(0.0) as i32)
        .min(width as i32);
    let x_end = (((visible_image_rect.max.x - image_rect.min.x) / pixel_size)
        .ceil()
        .max(0.0) as i32)
        .min(width as i32);
    let y_start = (((visible_image_rect.min.y - image_rect.min.y) / pixel_size)
        .floor()
        .max(0.0) as i32)
        .min(height as i32);
    let y_end = (((visible_image_rect.max.y - image_rect.min.y) / pixel_size)
        .ceil()
        .max(0.0) as i32)
        .min(height as i32);

    if x_start > x_end || y_start > y_end {
        return;
    }

    let painter = ui.painter().with_clip_rect(visible_image_rect);
    let grid_stroke = egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 58));
    for x in x_start..=x_end {
        let sx = image_rect.min.x + x as f32 * pixel_size;
        painter.line_segment(
            [
                pos2(sx, visible_image_rect.min.y),
                pos2(sx, visible_image_rect.max.y),
            ],
            grid_stroke,
        );
    }
    for y in y_start..=y_end {
        let sy = image_rect.min.y + y as f32 * pixel_size;
        painter.line_segment(
            [
                pos2(visible_image_rect.min.x, sy),
                pos2(visible_image_rect.max.x, sy),
            ],
            grid_stroke,
        );
    }

    let Some(cache) = cache else {
        return;
    };

    let overlay_scale = pixel_size / PIXEL_OVERLAY_REFERENCE_ZOOM;
    let text_padding_y = PIXEL_OVERLAY_BASE_PADDING_Y_AT_MAX_ZOOM * overlay_scale;
    let text_padding_x = PIXEL_OVERLAY_BASE_PADDING_X_AT_MAX_ZOOM * overlay_scale;
    let line_height = PIXEL_OVERLAY_BASE_LINE_HEIGHT_AT_MAX_ZOOM * overlay_scale;
    if line_height <= 0.0 {
        return;
    }
    let font_size = (PIXEL_OVERLAY_BASE_FONT_SIZE_AT_MAX_ZOOM * overlay_scale).max(1.0);
    let shadow_offset = PIXEL_OVERLAY_BASE_SHADOW_OFFSET_AT_MAX_ZOOM * overlay_scale;
    let font = egui::FontId::new(font_size, egui::FontFamily::Name("geist_mono".into()));
    for y in y_start..y_end {
        for x in x_start..x_end {
            let Some(rgba) = sample_value_pixel(
                cache,
                x as u32,
                y as u32,
                reference,
                diff_metric_mode,
                diff_output_active,
                clamp_output,
            ) else {
                continue;
            };
            let lines = [
                ('r', format_overlay_channel('r', rgba[0])),
                ('g', format_overlay_channel('g', rgba[1])),
                ('b', format_overlay_channel('b', rgba[2])),
                ('a', format_overlay_channel('a', rgba[3])),
            ];

            let x0 = image_rect.min.x + x as f32 * pixel_size + text_padding_x;
            let y0 = image_rect.min.y + y as f32 * pixel_size + text_padding_y;
            for (idx, (channel, line)) in lines.iter().enumerate() {
                let text_pos = pos2(x0, y0 + idx as f32 * line_height);
                painter.text(
                    pos2(text_pos.x + shadow_offset, text_pos.y + shadow_offset),
                    egui::Align2::LEFT_TOP,
                    line,
                    font.clone(),
                    pixel_overlay_shadow_color(),
                );
                painter.text(
                    text_pos,
                    egui::Align2::LEFT_TOP,
                    line,
                    font.clone(),
                    pixel_overlay_channel_color(*channel),
                );
            }
        }
    }
}

fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    let a = ((color.a() as f32) * alpha.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

fn is_supported_reference_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "exr"
            )
        })
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

struct DecodedReferenceImage {
    width: u32,
    height: u32,
    preview_rgba8: Vec<u8>,
    transfer_mode: RefImageTransferMode,
    high_precision_source: bool,
    source_linear_rgba: Vec<f32>,
    linear_premul_rgba: Vec<f32>,
}

fn srgb_to_linear_channel(x: f32) -> f32 {
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

fn apply_reference_alpha_mode_to_linear_rgba(
    source_linear_rgba: &[f32],
    alpha_mode: RefImageAlphaMode,
) -> Vec<f32> {
    let mut linear_premul_rgba = source_linear_rgba.to_vec();
    if matches!(alpha_mode, RefImageAlphaMode::Straight) {
        for rgba in linear_premul_rgba.chunks_exact_mut(4) {
            rgba[0] *= rgba[3];
            rgba[1] *= rgba[3];
            rgba[2] *= rgba[3];
        }
    }
    linear_premul_rgba
}

fn decode_reference_image(
    decoded: image::DynamicImage,
    alpha_mode: RefImageAlphaMode,
) -> DecodedReferenceImage {
    let color_type = decoded.color();
    let preview_rgba8 = decoded.to_rgba8();
    let width = preview_rgba8.width();
    let height = preview_rgba8.height();
    let preview_rgba8 = preview_rgba8.into_raw();
    let mut source_linear_rgba = decoded.to_rgba32f().into_raw();

    let transfer_mode = match color_type {
        image::ColorType::Rgb32F | image::ColorType::Rgba32F => RefImageTransferMode::Linear,
        _ => RefImageTransferMode::Srgb,
    };
    let high_precision_source = matches!(
        color_type,
        image::ColorType::L16
            | image::ColorType::La16
            | image::ColorType::Rgb16
            | image::ColorType::Rgba16
            | image::ColorType::Rgb32F
            | image::ColorType::Rgba32F
    );

    for rgba in source_linear_rgba.chunks_exact_mut(4) {
        if matches!(transfer_mode, RefImageTransferMode::Srgb) {
            rgba[0] = srgb_to_linear_channel(rgba[0].clamp(0.0, 1.0));
            rgba[1] = srgb_to_linear_channel(rgba[1].clamp(0.0, 1.0));
            rgba[2] = srgb_to_linear_channel(rgba[2].clamp(0.0, 1.0));
        }
    }
    let linear_premul_rgba =
        apply_reference_alpha_mode_to_linear_rgba(source_linear_rgba.as_slice(), alpha_mode);

    DecodedReferenceImage {
        width,
        height,
        preview_rgba8,
        transfer_mode,
        high_precision_source,
        source_linear_rgba,
        linear_premul_rgba,
    }
}

fn reference_texture_format(decoded: &DecodedReferenceImage) -> wgpu::TextureFormat {
    if decoded.high_precision_source {
        wgpu::TextureFormat::Rgba16Float
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    }
}

fn reference_upload_bytes_per_row(width: u32, bytes_per_pixel: u32) -> anyhow::Result<u32> {
    width
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| anyhow::anyhow!("reference image row stride overflow"))
}

fn upload_reference_linear_premul_rgba(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: [u32; 2],
    linear_premul_rgba: &[f32],
    texture_format: wgpu::TextureFormat,
) -> anyhow::Result<()> {
    let [width, height] = size;
    match texture_format {
        wgpu::TextureFormat::Rgba16Float => {
            let bytes_per_row = reference_upload_bytes_per_row(width, 8)?;
            let rgba16f: Vec<u16> = linear_premul_rgba
                .iter()
                .map(|v| half::f16::from_f32(*v).to_bits())
                .collect();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(rgba16f.as_slice()),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        wgpu::TextureFormat::Rgba8Unorm => {
            let bytes_per_row = reference_upload_bytes_per_row(width, 4)?;
            let rgba8: Vec<u8> = linear_premul_rgba
                .iter()
                .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
                .collect();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba8.as_slice(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        other => anyhow::bail!("unsupported reference texture format for upload: {other:?}"),
    }
    Ok(())
}

fn load_reference_image_from_decoded(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    decoded: image::DynamicImage,
    name: String,
    source: RefImageSource,
    alpha_mode: RefImageAlphaMode,
) -> anyhow::Result<()> {
    let decoded = decode_reference_image(decoded, alpha_mode);
    let texture_format = reference_texture_format(&decoded);

    let color_image = egui::ColorImage::from_rgba_unmultiplied(
        [decoded.width as usize, decoded.height as usize],
        &decoded.preview_rgba8,
    );
    let texture = ctx.load_texture(
        format!("reference:{name}"),
        color_image,
        egui::TextureOptions::NEAREST,
    );

    let wgpu_texture = render_state
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("sys.reference.image"),
            size: wgpu::Extent3d {
                width: decoded.width,
                height: decoded.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: texture_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

    upload_reference_linear_premul_rgba(
        app.shader_space.queue.as_ref(),
        &wgpu_texture,
        [decoded.width, decoded.height],
        decoded.linear_premul_rgba.as_slice(),
        texture_format,
    )?;

    let wgpu_texture_view = wgpu_texture.create_view(&wgpu::TextureViewDescriptor::default());

    clear_reference(app, renderer);
    app.ref_image = Some(RefImageState {
        name,
        source_linear_rgba: decoded.source_linear_rgba,
        linear_premul_rgba: decoded.linear_premul_rgba,
        texture,
        wgpu_texture,
        wgpu_texture_view,
        size: [decoded.width, decoded.height],
        texture_format,
        alpha_mode,
        transfer_mode: decoded.transfer_mode,
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

fn load_reference_image_from_path(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
    path: &Path,
    source: RefImageSource,
    alpha_mode: RefImageAlphaMode,
) -> anyhow::Result<()> {
    if !is_supported_reference_image(path) {
        anyhow::bail!("unsupported reference image extension: {}", path.display());
    }

    let decoded = image::open(path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("reference")
        .to_string();
    load_reference_image_from_decoded(
        app,
        ctx,
        render_state,
        renderer,
        decoded,
        name,
        source,
        alpha_mode,
    )
}

pub(super) fn sync_reference_image_from_scene(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
) {
    let desired_alpha_mode = app.reference_alpha_mode;
    let desired_asset_id = app.scene_reference_image_asset_id.clone();
    let desired_data_url = app.scene_reference_image_data_url.clone();
    let desired_path = app.scene_reference_image_path.clone();

    // Prefer assetId → asset_store lookup.
    if let Some(asset_id) = desired_asset_id {
        let already_loaded = matches!(
            app.ref_image.as_ref(),
            Some(r) if matches!(&r.source, RefImageSource::SceneNodeAssetId(v) if v == &asset_id)
                && r.alpha_mode == desired_alpha_mode
        );
        if already_loaded {
            return;
        }
        let asset_present = app.asset_store.contains(&asset_id);
        // Include current presence state in the dedup key so we retry once the
        // async asset upload completes (missing -> present transition).
        let attempt_key = format!("assetid:{asset_id}:present:{asset_present}");
        if app.last_auto_reference_attempt.as_deref() == Some(attempt_key.as_str()) {
            return;
        }
        app.last_auto_reference_attempt = Some(attempt_key);

        if !asset_present {
            return;
        }

        match app.asset_store.load_image(&asset_id) {
            Ok(Some(decoded)) => {
                if let Err(e) = load_reference_image_from_decoded(
                    app,
                    ctx,
                    render_state,
                    renderer,
                    decoded,
                    format!("ReferenceImage(assetId:{asset_id})"),
                    RefImageSource::SceneNodeAssetId(asset_id.clone()),
                    desired_alpha_mode,
                ) {
                    eprintln!("[reference-image] failed to load asset '{asset_id}': {e:#}");
                }
            }
            Ok(None) => {
                // Asset presence was observed above; if it disappears/races,
                // clear dedup state so next frame can retry.
                app.last_auto_reference_attempt = None;
                eprintln!("[reference-image] asset '{asset_id}' not found in asset store");
            }
            Err(e) => {
                eprintln!("[reference-image] failed to decode asset '{asset_id}': {e:#}");
            }
        }
        return;
    }

    if let Some(data_url) = desired_data_url {
        let already_loaded = matches!(
            app.ref_image.as_ref(),
            Some(r) if matches!(&r.source, RefImageSource::SceneNodeDataUrl(v) if v == &data_url)
                && r.alpha_mode == desired_alpha_mode
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
                if let Err(e) = load_reference_image_from_decoded(
                    app,
                    ctx,
                    render_state,
                    renderer,
                    decoded,
                    "ReferenceImage(dataUrl)".to_string(),
                    RefImageSource::SceneNodeDataUrl(data_url),
                    desired_alpha_mode,
                ) {
                    eprintln!("[reference-image] failed to load ReferenceImage.dataUrl: {e:#}");
                }
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
                app.ref_image.as_ref(),
                Some(r) if matches!(&r.source, RefImageSource::SceneNodePath(p) if p == &path)
                    && r.alpha_mode == desired_alpha_mode
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
                desired_alpha_mode,
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
                Some(
                    RefImageSource::SceneNodePath(_)
                        | RefImageSource::SceneNodeDataUrl(_)
                        | RefImageSource::SceneNodeAssetId(_)
                )
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

fn set_reference_alpha_mode(
    queue: &wgpu::Queue,
    reference: &mut RefImageState,
    alpha_mode: RefImageAlphaMode,
) -> anyhow::Result<bool> {
    if reference.alpha_mode == alpha_mode {
        return Ok(false);
    }

    let linear_premul_rgba = apply_reference_alpha_mode_to_linear_rgba(
        reference.source_linear_rgba.as_slice(),
        alpha_mode,
    );
    upload_reference_linear_premul_rgba(
        queue,
        &reference.wgpu_texture,
        reference.size,
        linear_premul_rgba.as_slice(),
        reference.texture_format,
    )?;
    reference.linear_premul_rgba = linear_premul_rgba;
    reference.alpha_mode = alpha_mode;
    Ok(true)
}

pub(super) fn pick_reference_image_from_dialog(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    renderer: &mut egui_wgpu::Renderer,
) -> anyhow::Result<bool> {
    let mut picker = rfd::FileDialog::new().add_filter("Image", &["png", "exr", "jpg", "jpeg"]);
    if let Some(reference) = app.ref_image.as_ref() {
        picker = picker.set_file_name(reference.name.as_str());
    }

    let Some(path) = picker.pick_file() else {
        return Ok(false);
    };

    load_reference_image_from_path(
        app,
        ctx,
        render_state,
        renderer,
        &path,
        RefImageSource::Manual,
        app.reference_alpha_mode,
    )?;
    app.last_auto_reference_attempt = None;
    Ok(true)
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
            app.reference_alpha_mode,
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

    if let Some(rx) = app.viewport_operation_job_rx.as_ref() {
        match rx.try_recv() {
            Ok((request_id, success)) => {
                app.viewport_operation_job_rx = None;
                if matches!(
                    app.viewport_operation_indicator,
                    ViewportOperationIndicator::InProgress {
                        request_id: active_request_id,
                        ..
                    } if active_request_id == request_id
                ) {
                    if success {
                        app.viewport_operation_indicator =
                            ViewportOperationIndicator::Success { hide_at: now + 1.0 };
                        app.viewport_operation_last_visual =
                            Some(ViewportOperationIndicatorVisual::Success);
                    } else {
                        app.viewport_operation_indicator =
                            ViewportOperationIndicator::Failure { hide_at: now + 1.0 };
                        app.viewport_operation_last_visual =
                            Some(ViewportOperationIndicatorVisual::Failure);
                    }
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                app.viewport_operation_job_rx = None;
                app.viewport_operation_indicator =
                    ViewportOperationIndicator::Failure { hide_at: now + 1.0 };
                app.viewport_operation_last_visual =
                    Some(ViewportOperationIndicatorVisual::Failure);
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    if let ViewportOperationIndicator::InProgress { started_at, .. } =
        app.viewport_operation_indicator
        && now - started_at >= VIEWPORT_OPERATION_TIMEOUT_SECS
    {
        app.viewport_operation_job_rx = None;
        app.viewport_operation_indicator =
            ViewportOperationIndicator::Failure { hide_at: now + 1.0 };
        app.viewport_operation_last_visual = Some(ViewportOperationIndicatorVisual::Failure);
    }

    match app.viewport_operation_indicator {
        ViewportOperationIndicator::Success { hide_at }
        | ViewportOperationIndicator::Failure { hide_at } => {
            if now >= hide_at {
                app.viewport_operation_indicator = ViewportOperationIndicator::Hidden;
            }
        }
        _ => {}
    }

    if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::F)) {
        requested_toggle_canvas_only = true;
    }

    if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::S)) {
        app.hdr_preview_clamp_enabled = !app.hdr_preview_clamp_enabled;
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
            mark_pixel_overlay_dirty(app);
        }
    }

    maybe_handle_reference_drop(app, ctx, render_state, renderer);

    if app.preview_texture_name.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.preview_texture_name = None;
        mark_pixel_overlay_dirty(app);
        clear_pixel_overlay_cache(ctx);
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
            mark_pixel_overlay_dirty(app);
            clear_pixel_overlay_cache(ctx);
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

    let display_texture_name = if using_preview {
        app.preview_texture_name
            .as_ref()
            .map(|name| name.as_str().to_string())
            .unwrap_or_else(|| app.output_texture_name.as_str().to_string())
    } else {
        app.output_texture_name.as_str().to_string()
    };
    let effective_resolution = app
        .shader_space
        .texture_info(display_texture_name.as_str())
        .map(|info| [info.size.width, info.size.height])
        .unwrap_or(app.resolution);
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

    let pan_zoom_animating = is_pan_zoom_animating(app);
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
            egui::CornerRadius::same(design_tokens::BORDER_RADIUS_REGULAR as u8),
            Color32::from_rgba_unmultiplied(80, 130, 255, 36),
        );
        ui.painter().rect_stroke(
            animated_canvas_rect.shrink(2.0),
            egui::CornerRadius::same(design_tokens::BORDER_RADIUS_REGULAR as u8),
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 170, 255, 220)),
            egui::StrokeKind::Outside,
        );
        ui.painter().text(
            animated_canvas_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Drop PNG / JPEG / EXR as Reference",
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
            Color32::from_rgba_unmultiplied(214, 228, 255, 240),
        );
    }

    response.context_menu(|ui| {
        if ui.button("复制材质").clicked() {
            if let Some(info) = app.shader_space.texture_info(display_texture_name.as_str())
                && let Ok(image) = app
                    .shader_space
                    .read_texture_rgba8(display_texture_name.as_str())
            {
                let width = info.size.width as usize;
                let height = info.size.height as usize;
                let bytes = image.bytes;
                app.viewport_operation_request_id =
                    app.viewport_operation_request_id.wrapping_add(1);
                let request_id = app.viewport_operation_request_id;
                let (tx, rx) = mpsc::channel::<(u64, bool)>();
                app.viewport_operation_job_rx = Some(rx);
                app.viewport_operation_indicator = ViewportOperationIndicator::InProgress {
                    started_at: now,
                    request_id,
                };
                app.viewport_operation_last_visual =
                    Some(ViewportOperationIndicatorVisual::InProgress);

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
                    let _ = tx.send((request_id, copied));
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

    if ctx.input(|i| i.key_pressed(KEY_TOGGLE_SAMPLING)) {
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

    if ctx.input(|i| i.key_pressed(KEY_TOGGLE_REFERENCE_ALPHA)) {
        app.reference_alpha_mode = match app.reference_alpha_mode {
            RefImageAlphaMode::Premultiplied => RefImageAlphaMode::Straight,
            RefImageAlphaMode::Straight => RefImageAlphaMode::Premultiplied,
        };

        let mut changed = false;
        if let Some(reference) = app.ref_image.as_mut() {
            match set_reference_alpha_mode(
                app.shader_space.queue.as_ref(),
                reference,
                app.reference_alpha_mode,
            ) {
                Ok(did_change) => changed = did_change,
                Err(e) => {
                    eprintln!("[reference-image] failed to switch alpha mode: {e:#}");
                }
            }
        }
        if changed {
            app.diff_dirty = true;
            app.analysis_dirty = true;
            app.clipping_dirty = true;
            mark_pixel_overlay_dirty(app);
        }
    }

    if ctx.input(|i| i.key_pressed(egui::Key::C)) {
        app.clip_enabled = !app.clip_enabled;
        app.clipping_dirty = true;
    }

    if let Some(reference) = app.ref_image.as_mut() {
        if ctx.input(|i| i.key_pressed(egui::Key::A)) && reference.offset != egui::Vec2::ZERO {
            reference.offset = egui::Vec2::ZERO;
            app.diff_dirty = true;
            if matches!(reference.mode, RefImageMode::Diff) {
                app.analysis_dirty = true;
                app.clipping_dirty = true;
            }
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
                    if matches!(reference.mode, RefImageMode::Diff) {
                        app.analysis_dirty = true;
                        app.clipping_dirty = true;
                    }
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

    let compare_output_active = app.diff_texture_id.is_some();
    let display_texture_format = if compare_output_active {
        app.diff_renderer
            .as_ref()
            .map(|renderer| renderer.output_format())
    } else {
        None
    }
    .or_else(|| {
        app.shader_space
            .texture_info(display_texture_name.as_str())
            .map(|info| info.format)
    });
    let hdr_clamp_effective =
        is_hdr_clamp_effective(app.hdr_preview_clamp_enabled, display_texture_format);

    let mut display_attachment = if compare_output_active {
        app.diff_texture_id
    } else if using_preview {
        app.preview_color_attachment.or(app.color_attachment)
    } else {
        app.color_attachment
    };

    if hdr_clamp_effective && !compare_output_active {
        let hdr_clamp_source = app
            .shader_space
            .textures
            .get(display_texture_name.as_str())
            .and_then(|texture| {
                texture.wgpu_texture_view.as_ref().map(|view| {
                    (
                        view.clone(),
                        [
                            texture.wgpu_texture_desc.size.width,
                            texture.wgpu_texture_desc.size.height,
                        ],
                    )
                })
            });

        if let Some((source_view, source_size)) = hdr_clamp_source {
            let clamp_renderer = app.hdr_clamp_renderer.get_or_insert_with(|| {
                crate::ui::hdr_clamp::HdrClampRenderer::new(&render_state.device, source_size)
            });
            clamp_renderer.update(
                &render_state.device,
                &render_state.queue,
                &source_view,
                source_size,
            );

            let sampler = texture_bridge::canvas_sampler_descriptor(app.texture_filter);
            if let Some(id) = app.hdr_clamp_texture_id {
                renderer.update_egui_texture_from_wgpu_texture_with_sampler_options(
                    &render_state.device,
                    clamp_renderer.output_view(),
                    sampler,
                    id,
                );
            } else {
                app.hdr_clamp_texture_id =
                    Some(renderer.register_native_texture_with_sampler_options(
                        &render_state.device,
                        clamp_renderer.output_view(),
                        sampler,
                    ));
            }
            if let Some(id) = app.hdr_clamp_texture_id {
                display_attachment = Some(id);
            }
        }
    }

    if let Some(tex_id) = display_attachment {
        ui.painter().add(
            egui::epaint::RectShape::filled(animated_canvas_rect, rounding, Color32::WHITE)
                .with_texture(tex_id, computed_uv),
        );
    }

    if !compare_output_active && let Some(reference) = app.ref_image.as_ref() {
        let reference_size = egui::vec2(reference.size[0] as f32, reference.size[1] as f32);
        let reference_min = image_rect.min + reference.offset * app.zoom;
        let reference_rect = Rect::from_min_size(reference_min, reference_size * app.zoom);
        let visible_rect = reference_rect.intersect(animated_canvas_rect);

        if visible_rect.is_positive() {
            let uv_min = (visible_rect.min - reference_rect.min) / reference_rect.size();
            let uv_max = (visible_rect.max - reference_rect.min) / reference_rect.size();
            let reference_uv =
                Rect::from_min_max(pos2(uv_min.x, uv_min.y), pos2(uv_max.x, uv_max.y));

            let tint = if matches!(reference.mode, RefImageMode::Overlay) {
                Color32::from_rgba_unmultiplied(255, 255, 255, (reference.opacity * 255.0) as u8)
            } else {
                Color32::WHITE
            };

            ui.painter().add(
                egui::epaint::RectShape::filled(visible_rect, rounding, tint)
                    .with_texture(reference.texture.id(), reference_uv),
            );
        }
    }

    if app.clip_enabled
        && let Some(clipping_texture_id) = app.clipping_texture_id
    {
        ui.painter().add(
            egui::epaint::RectShape::filled(animated_canvas_rect, rounding, Color32::WHITE)
                .with_texture(clipping_texture_id, computed_uv),
        );
    }

    let diff_output_active = compare_output_active;
    let value_sampling_texture_name = display_texture_name.clone();
    let mut value_sample_cache: Option<Arc<PixelOverlayCache>> = None;
    if app.zoom >= PIXEL_OVERLAY_MIN_ZOOM {
        value_sample_cache = if let Some(info) = app
            .shader_space
            .texture_info(value_sampling_texture_name.as_str())
        {
            Some(get_or_refresh_pixel_overlay_cache(
                app,
                ctx,
                value_sampling_texture_name.as_str(),
                info.size.width,
                info.size.height,
                info.format,
            ))
        } else {
            None
        };
        draw_pixel_overlay(
            ui,
            image_rect,
            animated_canvas_rect,
            app.zoom,
            effective_resolution,
            value_sample_cache.as_deref(),
            app.ref_image
                .as_ref()
                .map(value_sampling_reference_from_state),
            app.diff_metric_mode,
            diff_output_active,
            app.hdr_preview_clamp_enabled,
        );
    }

    let sampling_indicator = match app.texture_filter {
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
            .ref_image
            .as_ref()
            .map(|reference| reference.alpha_mode.short_label())
            .unwrap_or(app.reference_alpha_mode.short_label()),
        tooltip: "Reference alpha mode: PRE (premultiplied) / STR (straight). Press P to toggle.",
        kind: ViewportIndicatorKind::Text,
        strikethrough: false,
    };
    let current_view_is_hdr = matches!(
        display_texture_format,
        Some(wgpu::TextureFormat::Rgba16Float)
    );
    let hdr_indicator_tooltip = if hdr_clamp_effective {
        "Current view format: Rgba16Float (HDR) • Clamp to 1.0 ON (press S to toggle)"
    } else {
        "Current view format: Rgba16Float (HDR) • Clamp to 1.0 OFF (press S to toggle)"
    };
    let operation_visual = match app.viewport_operation_indicator {
        ViewportOperationIndicator::InProgress { .. } => {
            Some(ViewportOperationIndicatorVisual::InProgress)
        }
        ViewportOperationIndicator::Success { .. } => {
            Some(ViewportOperationIndicatorVisual::Success)
        }
        ViewportOperationIndicator::Failure { .. } => {
            Some(ViewportOperationIndicatorVisual::Failure)
        }
        ViewportOperationIndicator::Hidden => app.viewport_operation_last_visual,
    };

    app.viewport_indicator_manager.begin_frame();

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
        app.viewport_indicator_manager
            .register(ViewportIndicatorEntry {
                interaction: ViewportIndicatorInteraction::HoverOnly,
                callback_id: None,
                ..ViewportIndicatorEntry::compact(
                    "operation",
                    ORDER_OPERATION,
                    !matches!(
                        app.viewport_operation_indicator,
                        ViewportOperationIndicator::Hidden
                    ),
                    operation_indicator,
                )
            });
    }

    app.viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::text_badge_right_aligned_mono(
                "render_fps",
                ORDER_RENDER_FPS,
                true,
                format!("{} FPS", app.render_texture_fps_tracker.fps_at(now)),
                "Scene redraws per second (counts scene redraws only; excludes reference-image/diff/clipping/analysis-only updates)",
            )
        });

    app.viewport_indicator_manager
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
                    strikethrough: hdr_clamp_effective,
                },
            )
        });

    app.viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            animated: false,
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::compact("sampling", ORDER_SAMPLING, true, sampling_indicator)
        });

    app.viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            animated: false,
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::compact(
                "ref_alpha_mode",
                ORDER_REF_ALPHA,
                app.ref_image.is_some(),
                reference_alpha_indicator,
            )
        });

    app.viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::compact(
                "pause",
                ORDER_PAUSE,
                !app.time_updates_enabled,
                ViewportIndicator {
                    icon: "PAUSE",
                    tooltip: "Time 更新已暂停（Space 恢复）",
                    kind: ViewportIndicatorKind::Failure,
                    strikethrough: false,
                },
            )
        });

    app.viewport_indicator_manager
        .register(ViewportIndicatorEntry {
            interaction: ViewportIndicatorInteraction::HoverOnly,
            callback_id: None,
            ..ViewportIndicatorEntry::compact(
                "clipping",
                ORDER_CLIPPING,
                app.clip_enabled,
                ViewportIndicator {
                    icon: "C",
                    tooltip: "Clipping overlay 已开启",
                    kind: ViewportIndicatorKind::Failure,
                    strikethrough: false,
                },
            )
        });

    if let Some(stats) = app.diff_stats {
        app.viewport_indicator_manager
            .register(ViewportIndicatorEntry {
                interaction: ViewportIndicatorInteraction::HoverOnly,
                callback_id: None,
                ..ViewportIndicatorEntry::text_badge(
                    "diff_stats",
                    ORDER_STATS,
                    matches!(
                        app.ref_image.as_ref().map(|r| r.mode),
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

    let indicator_result =
        app.viewport_indicator_manager
            .render(ui, ctx, animated_canvas_rect, now);
    if indicator_result.needs_repaint {
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
                "Ref • {} • {} • {}×{} • α {:.2}",
                mode,
                reference.alpha_mode.short_label(),
                reference.size[0],
                reference.size[1],
                reference.opacity,
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
                let x = (uv_x * effective_resolution[0] as f32).floor() as u32;
                let y = (uv_y * effective_resolution[1] as f32).floor() as u32;
                if x < effective_resolution[0] && y < effective_resolution[1] {
                    if value_sample_cache.is_none()
                        && let Some(info) = app
                            .shader_space
                            .texture_info(value_sampling_texture_name.as_str())
                    {
                        value_sample_cache = Some(get_or_refresh_pixel_overlay_cache(
                            app,
                            ctx,
                            value_sampling_texture_name.as_str(),
                            info.size.width,
                            info.size.height,
                            info.format,
                        ));
                    }
                    if let Some(cache) = value_sample_cache.as_deref()
                        && let Some(rgba) = sample_value_pixel(
                            cache,
                            x,
                            y,
                            app.ref_image
                                .as_ref()
                                .map(value_sampling_reference_from_state),
                            app.diff_metric_mode,
                            diff_output_active,
                            app.hdr_preview_clamp_enabled,
                        )
                    {
                        app.last_sampled = Some(super::types::SampledPixel { x, y, rgba });
                    }
                }
            }
        }
    }

    app.canvas_center_prev = Some(animated_canvas_rect.center());

    requested_toggle_canvas_only
}

#[cfg(test)]
mod tests {
    use super::{
        DiffMetricMode, KEY_TOGGLE_REFERENCE_ALPHA, KEY_TOGGLE_SAMPLING, ORDER_OPERATION,
        ORDER_PAUSE, ORDER_RENDER_FPS, PixelOverlayCache, PixelOverlayReadback, RefImageAlphaMode,
        RefImageMode, ValueSamplingReference, apply_reference_alpha_mode_to_linear_rgba,
        compose_reference_over_base, compute_diff_metric_rgba, decode_reference_image,
        format_overlay_channel, is_hdr_clamp_effective, is_supported_reference_image,
        reference_texture_format, rgba8_to_rgba_f32, sample_rgba8_pixel, sample_rgba16f_pixel,
        sample_rgba16unorm_pixel, sample_value_pixel,
    };
    use image::{DynamicImage, ImageBuffer, Rgba, RgbaImage};
    use rust_wgpu_fiber::eframe::{egui, wgpu};
    use std::path::Path;

    fn assert_rgba_approx_eq(actual: [f32; 4], expected: [f32; 4]) {
        for (a, e) in actual.into_iter().zip(expected) {
            assert!((a - e).abs() <= 1e-4, "actual={a} expected={e}");
        }
    }

    fn make_rgba8_cache(width: u32, height: u32, bytes: Vec<u8>) -> PixelOverlayCache {
        PixelOverlayCache {
            texture_name: "test".to_string(),
            width,
            height,
            format: wgpu::TextureFormat::Rgba8Unorm,
            readback: PixelOverlayReadback::Rgba8(bytes),
        }
    }

    fn make_rgba16f_cache(width: u32, height: u32, channels: Vec<f32>) -> PixelOverlayCache {
        PixelOverlayCache {
            texture_name: "test".to_string(),
            width,
            height,
            format: wgpu::TextureFormat::Rgba16Float,
            readback: PixelOverlayReadback::Rgba16f(channels),
        }
    }

    #[test]
    fn render_fps_indicator_order_sits_between_operation_and_pause() {
        assert!(ORDER_OPERATION < ORDER_RENDER_FPS);
        assert!(ORDER_RENDER_FPS < ORDER_PAUSE);
    }

    #[test]
    fn keybindings_use_n_for_sampling_and_p_for_reference_alpha_mode() {
        assert_eq!(KEY_TOGGLE_SAMPLING, egui::Key::N);
        assert_eq!(KEY_TOGGLE_REFERENCE_ALPHA, egui::Key::P);
    }

    #[test]
    fn format_overlay_channel_adapts_to_eight_chars() {
        assert_eq!(format_overlay_channel('a', 1.0), "1.000000");
        assert_eq!(format_overlay_channel('a', 10.0), "10.00000");
        assert_eq!(format_overlay_channel('a', 100.0), "100.0000");
        assert_eq!(format_overlay_channel('a', 1000.0), "1000.000");
        assert_eq!(format_overlay_channel('a', 10000.0), "10000.00");
        assert_eq!(format_overlay_channel('a', 100000.0), "100000.0");
        assert_eq!(format_overlay_channel('a', 10000000.0), "10000000");
        assert_eq!(format_overlay_channel('a', -1.0), "-1.00000");
        assert_eq!(format_overlay_channel('a', -1000.0), "-1000.00");
    }

    #[test]
    fn rgba8_to_rgba_f32_converts_edge_values() {
        assert_eq!(
            rgba8_to_rgba_f32([0, 255, 128, 255]),
            [0.0, 1.0, 128.0 / 255.0, 1.0]
        );
    }

    #[test]
    fn sample_rgba8_pixel_checks_bounds() {
        let bytes = vec![
            255, 0, 0, 255, // (0,0)
            0, 255, 0, 255, // (1,0)
        ];
        assert_eq!(
            sample_rgba8_pixel(&bytes, 2, 1, 0, 0),
            Some([1.0, 0.0, 0.0, 1.0])
        );
        assert_eq!(sample_rgba8_pixel(&bytes, 2, 1, 2, 0), None);
        assert_eq!(sample_rgba8_pixel(&bytes, 2, 1, 1, 1), None);
    }

    #[test]
    fn sample_rgba16f_pixel_checks_bounds() {
        let channels = vec![
            0.25, 0.5, 0.75, 1.0, // (0,0)
            1.25, 1.5, 1.75, 2.0, // (1,0)
        ];
        assert_eq!(
            sample_rgba16f_pixel(&channels, 2, 1, 1, 0),
            Some([1.25, 1.5, 1.75, 2.0])
        );
        assert_eq!(sample_rgba16f_pixel(&channels, 2, 1, 2, 0), None);
    }

    #[test]
    fn sample_rgba16unorm_pixel_checks_bounds() {
        let channels = vec![
            0, 32768, 65535, 65535, // (0,0)
            65535, 0, 0, 65535, // (1,0)
        ];
        assert_eq!(
            sample_rgba16unorm_pixel(&channels, 2, 1, 0, 0),
            Some([0.0, 32768.0 / 65535.0, 1.0, 1.0])
        );
        assert_eq!(sample_rgba16unorm_pixel(&channels, 2, 1, 2, 0), None);
    }

    #[test]
    fn supported_reference_image_extensions_include_exr_and_png() {
        assert!(is_supported_reference_image(Path::new("foo.exr")));
        assert!(is_supported_reference_image(Path::new("foo.PNG")));
        assert!(!is_supported_reference_image(Path::new("foo.gif")));
    }

    #[test]
    fn hdr_clamp_effective_requires_toggle_and_hdr_format() {
        assert!(is_hdr_clamp_effective(
            true,
            Some(wgpu::TextureFormat::Rgba16Float)
        ));
        assert!(!is_hdr_clamp_effective(
            false,
            Some(wgpu::TextureFormat::Rgba16Float)
        ));
        assert!(!is_hdr_clamp_effective(
            true,
            Some(wgpu::TextureFormat::Rgba8Unorm)
        ));
        assert!(!is_hdr_clamp_effective(true, None));
    }

    #[test]
    fn overlay_mode_composites_reference_over_base() {
        let base_cache = make_rgba8_cache(1, 1, vec![128, 64, 32, 255]);
        let reference_pixels = [1.0, 0.0, 0.0, 128.0 / 255.0];
        let reference = ValueSamplingReference {
            mode: RefImageMode::Overlay,
            offset_px: [0, 0],
            size: [1, 1],
            opacity: 0.5,
            linear_premul_rgba: &reference_pixels,
        };

        let sampled = sample_value_pixel(
            &base_cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            true,
            false,
        )
        .expect("sampled");

        let src_r = 1.0 * 0.5;
        let src_a = (128.0 / 255.0) * 0.5;
        let inv_src_a = 1.0 - src_a;
        let expected = [
            src_r + (128.0 / 255.0) * inv_src_a,
            (64.0 / 255.0) * inv_src_a,
            (32.0 / 255.0) * inv_src_a,
            src_a + 1.0 * inv_src_a,
        ];
        assert_rgba_approx_eq(sampled, expected);
    }

    #[test]
    fn overlay_mode_transparent_reference_pixel_preserves_base() {
        let base_cache = make_rgba8_cache(1, 1, vec![51, 102, 153, 128]);
        let reference_pixels = [0.0, 0.0, 0.0, 0.0];
        let reference = ValueSamplingReference {
            mode: RefImageMode::Overlay,
            offset_px: [0, 0],
            size: [1, 1],
            opacity: 1.0,
            linear_premul_rgba: &reference_pixels,
        };

        let sampled = sample_value_pixel(
            &base_cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            true,
            false,
        )
        .expect("sampled");

        assert_rgba_approx_eq(
            sampled,
            [51.0 / 255.0, 102.0 / 255.0, 153.0 / 255.0, 128.0 / 255.0],
        );
    }

    #[test]
    fn overlay_mode_outside_reference_uses_base() {
        let base_cache = make_rgba8_cache(1, 1, vec![20, 40, 60, 80]);
        let reference_pixels = [1.0, 0.0, 0.0, 1.0];
        let reference = ValueSamplingReference {
            mode: RefImageMode::Overlay,
            offset_px: [1, 1],
            size: [1, 1],
            opacity: 1.0,
            linear_premul_rgba: &reference_pixels,
        };

        let sampled = sample_value_pixel(
            &base_cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            true,
            false,
        )
        .expect("sampled");
        assert_rgba_approx_eq(
            sampled,
            [20.0 / 255.0, 40.0 / 255.0, 60.0 / 255.0, 80.0 / 255.0],
        );
    }

    #[test]
    fn diff_mode_inside_reference_uses_metric() {
        let base_cache = make_rgba8_cache(1, 1, vec![255, 0, 127, 255]);
        let reference_pixels = [0.0, 1.0, 0.0, 0.5];
        let reference = ValueSamplingReference {
            mode: RefImageMode::Diff,
            offset_px: [0, 0],
            size: [1, 1],
            opacity: 0.2,
            linear_premul_rgba: &reference_pixels,
        };

        let sampled = sample_value_pixel(
            &base_cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            true,
            false,
        )
        .expect("sampled");

        assert_rgba_approx_eq(sampled, [1.0, 1.0, 127.0 / 255.0, 1.0]);
    }

    #[test]
    fn diff_mode_outside_reference_uses_base() {
        let base_cache = make_rgba8_cache(1, 1, vec![20, 40, 60, 80]);
        let reference_pixels = [1.0, 1.0, 1.0, 1.0];
        let reference = ValueSamplingReference {
            mode: RefImageMode::Diff,
            offset_px: [1, 0],
            size: [1, 1],
            opacity: 0.8,
            linear_premul_rgba: &reference_pixels,
        };

        let sampled = sample_value_pixel(
            &base_cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::SE,
            true,
            false,
        )
        .expect("sampled");

        assert_rgba_approx_eq(
            sampled,
            [20.0 / 255.0, 40.0 / 255.0, 60.0 / 255.0, 80.0 / 255.0],
        );
    }

    #[test]
    fn diff_mode_without_diff_output_falls_back_to_reference() {
        let base_cache = make_rgba8_cache(1, 1, vec![0, 0, 0, 255]);
        let reference_pixels = [10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0, 40.0 / 255.0];
        let reference = ValueSamplingReference {
            mode: RefImageMode::Diff,
            offset_px: [0, 0],
            size: [1, 1],
            opacity: 0.5,
            linear_premul_rgba: &reference_pixels,
        };

        let sampled = sample_value_pixel(
            &base_cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            false,
            false,
        )
        .expect("sampled");

        assert_rgba_approx_eq(
            sampled,
            [10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0, 40.0 / 255.0],
        );
    }

    #[test]
    fn value_sampling_ignores_ui_overlay_flags() {
        let base_cache = make_rgba16f_cache(1, 1, vec![2.0, 0.5, 1.2, 1.0]);
        let sampled = sample_value_pixel(&base_cache, 0, 0, None, DiffMetricMode::AE, false, false)
            .expect("sampled");
        assert_rgba_approx_eq(sampled, [2.0, 0.5, 1.2, 1.0]);
    }

    #[test]
    fn value_sampling_supports_u16_reference_pixels() {
        let base_cache = make_rgba8_cache(1, 1, vec![255, 0, 0, 255]);
        let reference_pixels = [32768.0 / 65535.0, 0.0, 0.0, 1.0];
        let reference = ValueSamplingReference {
            mode: RefImageMode::Diff,
            offset_px: [0, 0],
            size: [1, 1],
            opacity: 1.0,
            linear_premul_rgba: &reference_pixels,
        };

        let sampled = sample_value_pixel(
            &base_cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            true,
            false,
        )
        .expect("sampled");
        assert_rgba_approx_eq(sampled, [1.0 - (32768.0 / 65535.0), 0.0, 0.0, 1.0]);
    }

    #[test]
    fn value_sampling_supports_float_reference_pixels() {
        let base_cache = make_rgba8_cache(1, 1, vec![0, 0, 0, 255]);
        let reference_pixels = [1.25, 0.5, 0.25, 1.0];
        let reference = ValueSamplingReference {
            mode: RefImageMode::Diff,
            offset_px: [0, 0],
            size: [1, 1],
            opacity: 1.0,
            linear_premul_rgba: &reference_pixels,
        };

        let sampled = sample_value_pixel(
            &base_cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            false,
            false,
        )
        .expect("sampled");
        assert_rgba_approx_eq(sampled, [1.25, 0.5, 0.25, 1.0]);
    }

    #[test]
    fn compute_diff_metric_rgba_matches_expected_formulae() {
        let render = [0.75, 0.2, 0.1, 0.8];
        let reference = [0.25, 0.1, 0.4, 0.25];
        assert_rgba_approx_eq(
            compute_diff_metric_rgba(render, reference, DiffMetricMode::E),
            [0.5, 0.1, -0.3, 0.55],
        );
    }

    #[test]
    fn overlay_composition_does_not_clamp_hdr_alpha_above_one() {
        let out = compose_reference_over_base([0.0, 0.0, 0.0, 0.0], [2.0, 2.0, 2.0, 1.5], 1.0);
        assert_rgba_approx_eq(out, [2.0, 2.0, 2.0, 1.5]);
    }

    #[test]
    fn alpha_mode_conversion_differs_between_premultiplied_and_straight() {
        let source_linear_rgba = [1.0, 1.0, 1.0, 0.5];
        assert_eq!(
            apply_reference_alpha_mode_to_linear_rgba(
                &source_linear_rgba,
                RefImageAlphaMode::Premultiplied
            ),
            vec![1.0, 1.0, 1.0, 0.5]
        );
        assert_eq!(
            apply_reference_alpha_mode_to_linear_rgba(
                &source_linear_rgba,
                RefImageAlphaMode::Straight
            ),
            vec![0.5, 0.5, 0.5, 0.5]
        );
    }

    #[test]
    fn sdr_decode_applies_srgb_to_linear() {
        let mut image = RgbaImage::new(1, 1);
        image.put_pixel(0, 0, image::Rgba([128, 128, 128, 255]));
        let decoded = decode_reference_image(
            DynamicImage::ImageRgba8(image),
            RefImageAlphaMode::Premultiplied,
        );
        let expected = 0.215_860_53_f32;
        assert_eq!(decoded.transfer_mode, super::RefImageTransferMode::Srgb);
        assert!(!decoded.high_precision_source);
        assert_eq!(
            reference_texture_format(&decoded),
            wgpu::TextureFormat::Rgba8Unorm
        );
        assert!((decoded.linear_premul_rgba[0] - expected).abs() < 1e-4);
        assert!((decoded.linear_premul_rgba[1] - expected).abs() < 1e-4);
        assert!((decoded.linear_premul_rgba[2] - expected).abs() < 1e-4);
        assert!((decoded.linear_premul_rgba[3] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn u16_decode_uses_high_precision_reference_texture_format() {
        let mut image: ImageBuffer<Rgba<u16>, Vec<u16>> = ImageBuffer::new(1, 1);
        image.put_pixel(0, 0, Rgba([65535, 32768, 0, 65535]));
        let decoded = decode_reference_image(
            DynamicImage::ImageRgba16(image),
            RefImageAlphaMode::Premultiplied,
        );
        assert!(decoded.high_precision_source);
        assert_eq!(
            reference_texture_format(&decoded),
            wgpu::TextureFormat::Rgba16Float
        );
    }
}
