use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::Arc,
};

use rust_wgpu_fiber::eframe::{
    egui::{self, Color32, Rect, pos2},
    wgpu,
};

use crate::app::types::{App, DiffMetricMode, RefImageMode, RefImageState};

const PIXEL_OVERLAY_MIN_ZOOM: f32 = 48.0;
const PIXEL_OVERLAY_REFERENCE_ZOOM: f32 = 100.0;
const PIXEL_OVERLAY_BASE_PADDING_Y_AT_MAX_ZOOM: f32 = 10.0;
const PIXEL_OVERLAY_BASE_PADDING_X_AT_MAX_ZOOM: f32 = 18.0;
const PIXEL_OVERLAY_BASE_SHADOW_OFFSET_AT_MAX_ZOOM: f32 = 1.0;
const PIXEL_OVERLAY_BASE_LINE_HEIGHT_AT_MAX_ZOOM: f32 =
    (PIXEL_OVERLAY_REFERENCE_ZOOM - PIXEL_OVERLAY_BASE_PADDING_Y_AT_MAX_ZOOM * 2.0) / 4.0;
const PIXEL_OVERLAY_BASE_FONT_SIZE_AT_MAX_ZOOM: f32 =
    PIXEL_OVERLAY_BASE_LINE_HEIGHT_AT_MAX_ZOOM / 1.5;

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
pub enum PixelOverlayReadback {
    Rgba8(Vec<u8>),
    Rgba16f(Vec<f32>),
    Unavailable,
    UnsupportedFormat,
}

#[derive(Clone, Debug)]
pub struct PixelOverlayCache {
    pub texture_name: String,
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub readback: PixelOverlayReadback,
}

#[derive(Clone, Copy, Debug)]
pub struct ValueSamplingReference<'a> {
    pub mode: RefImageMode,
    pub offset_px: [i32; 2],
    pub size: [u32; 2],
    pub opacity: f32,
    pub linear_premul_rgba: &'a [f32],
}

pub fn value_sampling_reference_from_state(
    reference: &RefImageState,
) -> ValueSamplingReference<'_> {
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
            .core
            .shader_space
            .read_texture_rgba8(texture_name)
            .map(|image| PixelOverlayReadback::Rgba8(image.bytes))
            .unwrap_or(PixelOverlayReadback::Unavailable),
        wgpu::TextureFormat::Rgba16Float => app
            .core
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

pub fn get_or_refresh_cache(
    app: &mut App,
    texture_name: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> Arc<PixelOverlayCache> {
    let request_key = pixel_overlay_request_key(texture_name, width, height, format);
    let request_key_changed =
        app.canvas.display.pixel_overlay_last_request_key != Some(request_key);
    let cached = app.canvas.display.pixel_overlay_cache.clone();
    let should_refresh = app.canvas.invalidation.pixel_overlay_dirty()
        || request_key_changed
        || cached.as_ref().is_none_or(|existing| {
            should_refresh_pixel_overlay_cache(existing, texture_name, width, height, format)
        });

    if !should_refresh && let Some(existing) = cached {
        return existing;
    }

    let updated = read_pixel_overlay_cache(app, texture_name, width, height, format);
    app.canvas.display.pixel_overlay_last_request_key = Some(request_key);
    app.canvas.display.pixel_overlay_cache = Some(updated.clone());
    app.canvas.invalidation.clear_pixel_overlay();
    updated
}

pub fn clear_cache(app: &mut App) {
    app.canvas.display.pixel_overlay_cache = None;
    app.canvas.display.pixel_overlay_last_request_key = None;
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

pub fn format_diff_stat_value(value: f32) -> String {
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

pub fn sample_value_pixel(
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

pub fn draw_pixel_overlay(
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

#[cfg(test)]
mod tests {
    use super::{
        DiffMetricMode, PixelOverlayCache, PixelOverlayReadback, RefImageMode,
        ValueSamplingReference, compose_reference_over_base, compute_diff_metric_rgba,
        format_diff_stat_value, format_overlay_channel, rgba8_to_rgba_f32, sample_rgba8_pixel,
        sample_rgba16f_pixel, sample_rgba16unorm_pixel, sample_value_pixel,
    };

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
            format: super::wgpu::TextureFormat::Rgba8Unorm,
            readback: PixelOverlayReadback::Rgba8(bytes),
        }
    }

    fn make_rgba16f_cache(width: u32, height: u32, channels: Vec<f32>) -> PixelOverlayCache {
        PixelOverlayCache {
            texture_name: "test".to_string(),
            width,
            height,
            format: super::wgpu::TextureFormat::Rgba16Float,
            readback: PixelOverlayReadback::Rgba16f(channels),
        }
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
            rgba8_to_rgba_f32([0, 127, 255, 64]),
            [0.0, 127.0 / 255.0, 1.0, 64.0 / 255.0]
        );
    }

    #[test]
    fn sample_rgba8_pixel_checks_bounds() {
        let pixel = sample_rgba8_pixel(
            &[
                0, 10, 20, 30, //
                40, 50, 60, 70,
            ],
            2,
            1,
            1,
            0,
        )
        .unwrap();
        assert_rgba_approx_eq(
            pixel,
            [40.0 / 255.0, 50.0 / 255.0, 60.0 / 255.0, 70.0 / 255.0],
        );
        assert!(sample_rgba8_pixel(&[0, 1, 2, 3], 1, 1, 1, 0).is_none());
    }

    #[test]
    fn sample_rgba16f_pixel_checks_bounds() {
        let pixel = sample_rgba16f_pixel(
            &[
                0.1, 0.2, 0.3, 0.4, //
                0.5, 0.6, 0.7, 0.8,
            ],
            2,
            1,
            1,
            0,
        )
        .unwrap();
        assert_rgba_approx_eq(pixel, [0.5, 0.6, 0.7, 0.8]);
        assert!(sample_rgba16f_pixel(&[0.1, 0.2, 0.3, 0.4], 1, 1, 0, 1).is_none());
    }

    #[test]
    fn sample_rgba16unorm_pixel_checks_bounds() {
        let pixel = sample_rgba16unorm_pixel(&[0, 32768, 65535, 16384], 1, 1, 0, 0).unwrap();
        assert_rgba_approx_eq(pixel, [0.0, 32768.0 / 65535.0, 1.0, 16384.0 / 65535.0]);
        assert!(sample_rgba16unorm_pixel(&[0, 1, 2, 3], 1, 1, 1, 0).is_none());
    }

    #[test]
    fn overlay_mode_composites_reference_over_base() {
        let cache = make_rgba8_cache(1, 1, vec![128, 64, 32, 255]);
        let reference = ValueSamplingReference {
            mode: RefImageMode::Overlay,
            offset_px: [0, 0],
            size: [1, 1],
            opacity: 0.5,
            linear_premul_rgba: &[1.0, 0.0, 0.0, 1.0],
        };
        let sampled = sample_value_pixel(
            &cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            false,
            false,
        )
        .unwrap();
        assert_rgba_approx_eq(
            sampled,
            [
                0.5 + (128.0 / 255.0) * 0.5,
                (64.0 / 255.0) * 0.5,
                (32.0 / 255.0) * 0.5,
                1.0,
            ],
        );
    }

    #[test]
    fn overlay_mode_transparent_reference_pixel_preserves_base() {
        let cache = make_rgba8_cache(1, 1, vec![12, 34, 56, 78]);
        let reference = ValueSamplingReference {
            mode: RefImageMode::Overlay,
            offset_px: [0, 0],
            size: [1, 1],
            opacity: 0.8,
            linear_premul_rgba: &[0.2, 0.4, 0.6, 0.0],
        };
        let sampled = sample_value_pixel(
            &cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            false,
            false,
        )
        .unwrap();
        assert_rgba_approx_eq(
            sampled,
            [12.0 / 255.0, 34.0 / 255.0, 56.0 / 255.0, 78.0 / 255.0],
        );
    }

    #[test]
    fn overlay_mode_outside_reference_uses_base() {
        let cache = make_rgba8_cache(1, 1, vec![10, 20, 30, 40]);
        let reference = ValueSamplingReference {
            mode: RefImageMode::Overlay,
            offset_px: [5, 5],
            size: [1, 1],
            opacity: 1.0,
            linear_premul_rgba: &[1.0, 1.0, 1.0, 1.0],
        };
        let sampled = sample_value_pixel(
            &cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            false,
            false,
        )
        .unwrap();
        assert_rgba_approx_eq(
            sampled,
            [10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0, 40.0 / 255.0],
        );
    }

    #[test]
    fn diff_mode_inside_reference_uses_metric() {
        let cache = make_rgba16f_cache(1, 1, vec![0.8, 0.6, 0.4, 0.2]);
        let reference = ValueSamplingReference {
            mode: RefImageMode::Diff,
            offset_px: [0, 0],
            size: [1, 1],
            opacity: 1.0,
            linear_premul_rgba: &[0.3, 0.4, 0.5, 0.6],
        };
        let sampled = sample_value_pixel(
            &cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            true,
            false,
        )
        .unwrap();
        assert_rgba_approx_eq(sampled, [0.5, 0.2, 0.1, 1.0]);
    }

    #[test]
    fn diff_mode_outside_reference_uses_base() {
        let cache = make_rgba16f_cache(1, 1, vec![0.8, 0.6, 0.4, 0.2]);
        let reference = ValueSamplingReference {
            mode: RefImageMode::Diff,
            offset_px: [1, 0],
            size: [1, 1],
            opacity: 1.0,
            linear_premul_rgba: &[0.3, 0.4, 0.5, 0.6],
        };
        let sampled = sample_value_pixel(
            &cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            true,
            false,
        )
        .unwrap();
        assert_rgba_approx_eq(sampled, [0.8, 0.6, 0.4, 0.2]);
    }

    #[test]
    fn diff_mode_without_diff_output_falls_back_to_reference() {
        let cache = make_rgba16f_cache(1, 1, vec![0.8, 0.6, 0.4, 0.2]);
        let reference = ValueSamplingReference {
            mode: RefImageMode::Diff,
            offset_px: [0, 0],
            size: [1, 1],
            opacity: 1.0,
            linear_premul_rgba: &[0.3, 0.4, 0.5, 0.6],
        };
        let sampled = sample_value_pixel(
            &cache,
            0,
            0,
            Some(reference),
            DiffMetricMode::AE,
            false,
            false,
        )
        .unwrap();
        assert_rgba_approx_eq(sampled, [0.3, 0.4, 0.5, 0.6]);
    }

    #[test]
    fn value_sampling_supports_u16_reference_pixels() {
        let sampled = sample_rgba16unorm_pixel(&[65535, 0, 32768, 16384], 1, 1, 0, 0).unwrap();
        assert_rgba_approx_eq(sampled, [1.0, 0.0, 32768.0 / 65535.0, 16384.0 / 65535.0]);
    }

    #[test]
    fn value_sampling_supports_float_reference_pixels() {
        let sampled = sample_rgba16f_pixel(&[1.2, -0.1, 0.5, 2.0], 1, 1, 0, 0).unwrap();
        assert_rgba_approx_eq(sampled, [1.2, -0.1, 0.5, 2.0]);
    }

    #[test]
    fn compute_diff_metric_rgba_matches_expected_formulae() {
        let render = [4.0, 6.0, 8.0, 10.0];
        let reference = [2.0, 3.0, 4.0, 5.0];
        assert_eq!(
            compute_diff_metric_rgba(render, reference, DiffMetricMode::E),
            [2.0, 3.0, 4.0, 5.0]
        );
        assert_eq!(
            compute_diff_metric_rgba(render, reference, DiffMetricMode::AE),
            [2.0, 3.0, 4.0, 5.0]
        );
        assert_eq!(
            compute_diff_metric_rgba(render, reference, DiffMetricMode::SE),
            [4.0, 9.0, 16.0, 25.0]
        );
    }

    #[test]
    fn overlay_composition_does_not_clamp_hdr_alpha_above_one() {
        let composed =
            compose_reference_over_base([1.0, 1.0, 1.0, 1.5], [2.0, 0.0, 0.0, 0.75], 1.0);
        assert!(composed[3] > 1.0);
    }

    #[test]
    fn diff_stat_format_switches_to_scientific_for_extremes() {
        assert_eq!(format_diff_stat_value(0.25), "0.2500");
        assert!(format_diff_stat_value(1.0e-5).contains('e'));
    }
}
