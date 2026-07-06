use std::{
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use rust_wgpu_fiber::eframe::{egui, egui_wgpu, wgpu};

use crate::{
    android_reference::AndroidReferenceFrame,
    app::types::{
        App, RefImageAlphaMode, RefImageMode, RefImageSource, RefImageState, RefImageTransferMode,
        ShortwirePastedReferenceImage, ShortwireReferenceImage,
    },
};

use super::state::{ReferenceAttemptKey, ReferenceDesiredSource};

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

struct AndroidReferenceDecodedFrame {
    rgba8: Vec<u8>,
}

struct AndroidReferenceUploadSample {
    frame_id: u64,
    size: [u32; 2],
    input_bytes: usize,
    decode_elapsed: Duration,
    egui_elapsed: Duration,
    gpu_upload_elapsed: Duration,
    total_elapsed: Duration,
    recreated_texture: bool,
}

#[derive(Default)]
struct AndroidReferenceUploadPerf {
    window_start: Option<Instant>,
    frames: u32,
    recreated_textures: u32,
    dropped_frames: u64,
    last_frame_id: u64,
    input_bytes: usize,
    decode_total: Duration,
    decode_max: Duration,
    egui_total: Duration,
    egui_max: Duration,
    gpu_upload_total: Duration,
    gpu_upload_max: Duration,
    total: Duration,
    total_max: Duration,
}

fn record_android_reference_upload_perf(sample: AndroidReferenceUploadSample) {
    static PERF: OnceLock<Mutex<AndroidReferenceUploadPerf>> = OnceLock::new();
    let now = Instant::now();
    let Ok(mut perf) = PERF
        .get_or_init(|| Mutex::new(AndroidReferenceUploadPerf::default()))
        .lock()
    else {
        return;
    };

    let window_start = *perf.window_start.get_or_insert(now);
    if perf.last_frame_id == 0 {
        perf.dropped_frames += sample.frame_id.saturating_sub(1);
    } else if sample.frame_id > perf.last_frame_id + 1 {
        perf.dropped_frames += sample.frame_id - perf.last_frame_id - 1;
    }
    perf.last_frame_id = sample.frame_id;
    perf.frames += 1;
    perf.input_bytes += sample.input_bytes;
    perf.decode_total += sample.decode_elapsed;
    perf.decode_max = perf.decode_max.max(sample.decode_elapsed);
    perf.egui_total += sample.egui_elapsed;
    perf.egui_max = perf.egui_max.max(sample.egui_elapsed);
    perf.gpu_upload_total += sample.gpu_upload_elapsed;
    perf.gpu_upload_max = perf.gpu_upload_max.max(sample.gpu_upload_elapsed);
    perf.total += sample.total_elapsed;
    perf.total_max = perf.total_max.max(sample.total_elapsed);
    if sample.recreated_texture {
        perf.recreated_textures += 1;
    }

    let window_elapsed = now.duration_since(window_start);
    if window_elapsed < Duration::from_secs(1) {
        return;
    }

    let frames = perf.frames.max(1) as f64;
    let seconds = window_elapsed.as_secs_f64().max(0.001);
    let fps = frames / seconds;
    let input_mb = perf.input_bytes as f64 / (1024.0 * 1024.0);
    eprintln!(
        "[android-reference:upload] fps={fps:.1} frames={} dropped={} recreate={} total_ms_avg={:.2} total_ms_max={:.2} decode_ms_avg={:.2} decode_ms_max={:.2} egui_ms_avg={:.2} egui_ms_max={:.2} gpu_ms_avg={:.2} gpu_ms_max={:.2} last_frame={} size={}x{} input_mb={input_mb:.1} input_mb_s={:.1}",
        perf.frames,
        perf.dropped_frames,
        perf.recreated_textures,
        perf.total.as_secs_f64() * 1000.0 / frames,
        perf.total_max.as_secs_f64() * 1000.0,
        perf.decode_total.as_secs_f64() * 1000.0 / frames,
        perf.decode_max.as_secs_f64() * 1000.0,
        perf.egui_total.as_secs_f64() * 1000.0 / frames,
        perf.egui_max.as_secs_f64() * 1000.0,
        perf.gpu_upload_total.as_secs_f64() * 1000.0 / frames,
        perf.gpu_upload_max.as_secs_f64() * 1000.0,
        sample.frame_id,
        sample.size[0],
        sample.size[1],
        input_mb / seconds,
    );

    let last_frame_id = perf.last_frame_id;
    *perf = AndroidReferenceUploadPerf {
        window_start: Some(now),
        last_frame_id,
        ..AndroidReferenceUploadPerf::default()
    };
}

fn srgb_to_linear_channel(x: f32) -> f32 {
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

pub fn apply_reference_alpha_mode_to_linear_rgba(
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

fn validate_android_reference_rgba8(
    width: u32,
    height: u32,
    rgba8: Vec<u8>,
) -> anyhow::Result<AndroidReferenceDecodedFrame> {
    let expected_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("Android reference frame is too large"))?;
    if rgba8.len() != expected_len as usize {
        anyhow::bail!(
            "Android reference frame has {} bytes, expected {}",
            rgba8.len(),
            expected_len
        );
    }
    Ok(AndroidReferenceDecodedFrame { rgba8 })
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

fn upload_reference_rgba8(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: [u32; 2],
    rgba8: &[u8],
) -> anyhow::Result<()> {
    let [width, height] = size;
    let expected_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("reference image upload size overflow"))?;
    if rgba8.len() != expected_len as usize {
        anyhow::bail!(
            "reference image upload has {} bytes, expected {}",
            rgba8.len(),
            expected_len
        );
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba8,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(reference_upload_bytes_per_row(width, 4)?),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    Ok(())
}

fn clear_reference_internal(app: &mut App, clear_override: bool) {
    if let Some(reference) = app.canvas.reference.ref_image.take()
        && let Some(id) = reference.native_texture_id
    {
        app.canvas.display.deferred_texture_frees.push(id);
    }
    app.canvas.analysis.diff_renderer = None;
    app.canvas.analysis.diff_stats = None;
    app.canvas.analysis.last_diff_request_key = None;
    app.canvas.analysis.last_diff_stats_request_key = None;
    if let Some(id) = app.canvas.analysis.diff_texture_id.take() {
        app.canvas.display.deferred_texture_frees.push(id);
    }
    if clear_override {
        app.canvas.reference.desired_override = None;
    }
    app.canvas.reference.last_attempt_key = None;
    app.canvas.invalidation.reference_removed();
}

pub fn clear_reference(app: &mut App) {
    clear_reference_internal(app, true);
}

pub fn clear_shortwire_clipboard_reference(app: &mut App) -> bool {
    if matches!(
        app.canvas.reference.ref_image.as_ref().map(|r| &r.source),
        Some(RefImageSource::ShortwireClipboard | RefImageSource::ShortwirePatch)
    ) {
        eprintln!("[shortwire-paste] clearing shortwire clipboard reference");
        clear_reference_internal(app, true);
        return true;
    }
    false
}

fn load_reference_image_from_decoded(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    decoded: image::DynamicImage,
    name: String,
    source: RefImageSource,
    alpha_mode: RefImageAlphaMode,
) -> anyhow::Result<()> {
    let decoded = decode_reference_image(decoded, alpha_mode);
    load_reference_image_from_decoded_reference(
        app,
        ctx,
        render_state,
        decoded,
        name,
        source,
        alpha_mode,
    )
}

fn load_reference_image_from_decoded_reference(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    decoded: DecodedReferenceImage,
    name: String,
    source: RefImageSource,
    alpha_mode: RefImageAlphaMode,
) -> anyhow::Result<()> {
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
        app.core.shader_space.queue.as_ref(),
        &wgpu_texture,
        [decoded.width, decoded.height],
        decoded.linear_premul_rgba.as_slice(),
        texture_format,
    )?;

    let wgpu_texture_view = wgpu_texture.create_view(&wgpu::TextureViewDescriptor::default());

    clear_reference_internal(app, false);
    app.canvas.reference.ref_image = Some(RefImageState {
        name,
        source_linear_rgba: decoded.source_linear_rgba,
        linear_premul_rgba: decoded.linear_premul_rgba,
        texture,
        native_texture_id: None,
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
    app.canvas.invalidation.reference_mode_changed();
    Ok(())
}

pub fn load_or_update_android_reference_frame(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    frame: AndroidReferenceFrame,
) -> anyhow::Result<()> {
    let total_start = Instant::now();
    let frame_id = frame.id;
    let serial = frame.serial;
    let frame_width = frame.width;
    let frame_height = frame.height;
    let frame_size = [frame_width, frame_height];
    let input_bytes = frame.rgba.len();
    let alpha_mode = app.canvas.reference.alpha_mode;
    let can_update_existing = app.canvas.reference.ref_image.as_ref().is_some_and(|reference| {
        matches!(&reference.source, RefImageSource::AndroidScrcpyUsb(existing_serial) if existing_serial == &serial)
            && reference.size == [frame_width, frame_height]
            && reference.texture_format == wgpu::TextureFormat::Rgba8UnormSrgb
    });
    let decode_start = Instant::now();
    let decoded = validate_android_reference_rgba8(frame_width, frame_height, frame.rgba)?;
    let decode_elapsed = decode_start.elapsed();
    let name = format!("Scrcpy USB {serial} {frame_width}x{frame_height}");
    let source = RefImageSource::AndroidScrcpyUsb(serial.clone());

    if can_update_existing && let Some(reference) = app.canvas.reference.ref_image.as_mut() {
        let gpu_upload_start = Instant::now();
        upload_reference_rgba8(
            app.core.shader_space.queue.as_ref(),
            &reference.wgpu_texture,
            reference.size,
            decoded.rgba8.as_slice(),
        )?;
        let gpu_upload_elapsed = gpu_upload_start.elapsed();
        reference.name = name;
        reference.alpha_mode = alpha_mode;
        let mode = reference.mode;
        app.canvas.invalidation.reference_pixels_changed(mode);
        record_android_reference_upload_perf(AndroidReferenceUploadSample {
            frame_id,
            size: frame_size,
            input_bytes,
            decode_elapsed,
            egui_elapsed: Duration::ZERO,
            gpu_upload_elapsed,
            total_elapsed: total_start.elapsed(),
            recreated_texture: false,
        });
        return Ok(());
    }

    let previous_display = app
        .canvas
        .reference
        .ref_image
        .as_ref()
        .and_then(|reference| {
            matches!(&reference.source, RefImageSource::AndroidScrcpyUsb(_)).then_some((
                reference.mode,
                reference.opacity,
                reference.offset,
            ))
        });

    let egui_start = Instant::now();
    let color_image = egui::ColorImage::from_rgba_unmultiplied(
        [frame_width as usize, frame_height as usize],
        &decoded.rgba8,
    );
    let texture = ctx.load_texture(
        format!("reference:{name}"),
        color_image,
        egui::TextureOptions::NEAREST,
    );
    let egui_elapsed = egui_start.elapsed();

    let wgpu_texture = render_state
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("sys.reference.android"),
            size: wgpu::Extent3d {
                width: frame_width,
                height: frame_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

    let gpu_upload_start = Instant::now();
    upload_reference_rgba8(
        app.core.shader_space.queue.as_ref(),
        &wgpu_texture,
        [frame_width, frame_height],
        decoded.rgba8.as_slice(),
    )?;
    let gpu_upload_elapsed = gpu_upload_start.elapsed();
    let wgpu_texture_view = wgpu_texture.create_view(&wgpu::TextureViewDescriptor::default());

    clear_reference_internal(app, false);
    app.canvas.reference.ref_image = Some(RefImageState {
        name,
        source_linear_rgba: Vec::new(),
        linear_premul_rgba: Vec::new(),
        texture,
        native_texture_id: None,
        wgpu_texture,
        wgpu_texture_view,
        size: [frame_width, frame_height],
        texture_format: wgpu::TextureFormat::Rgba8UnormSrgb,
        alpha_mode,
        transfer_mode: RefImageTransferMode::Srgb,
        offset: egui::Vec2::ZERO,
        mode: RefImageMode::Overlay,
        opacity: 0.5,
        drag_start: None,
        drag_start_offset: egui::Vec2::ZERO,
        source,
    });
    if let (Some((mode, opacity, offset)), Some(reference)) =
        (previous_display, app.canvas.reference.ref_image.as_mut())
    {
        reference.mode = mode;
        reference.opacity = opacity;
        reference.offset = offset;
    }
    app.canvas.reference.desired_override = Some(ReferenceDesiredSource::Manual);
    app.canvas.reference.last_attempt_key = None;
    record_android_reference_upload_perf(AndroidReferenceUploadSample {
        frame_id,
        size: frame_size,
        input_bytes,
        decode_elapsed,
        egui_elapsed,
        gpu_upload_elapsed,
        total_elapsed: total_start.elapsed(),
        recreated_texture: true,
    });
    Ok(())
}

pub fn sync_android_reference_frame(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
) {
    if let Some(frame) = app.shell.android_reference.take_latest_frame() {
        if let Err(error) = load_or_update_android_reference_frame(app, ctx, render_state, frame) {
            eprintln!("[android-reference] failed to load frame: {error:#}");
        }
    }
    if app.shell.android_reference.status().running {
        ctx.request_repaint();
    }
}

fn load_reference_image_from_path(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
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
    load_reference_image_from_decoded(app, ctx, render_state, decoded, name, source, alpha_mode)
}

fn active_desired_source(app: &App) -> Option<&ReferenceDesiredSource> {
    app.canvas
        .reference
        .desired_override
        .as_ref()
        .or(app.canvas.reference.scene_desired.as_ref())
}

pub fn sync_from_scene(app: &mut App, ctx: &egui::Context, render_state: &egui_wgpu::RenderState) {
    let desired_source = active_desired_source(app).cloned();

    match desired_source {
        Some(ReferenceDesiredSource::Manual) => {}
        Some(ReferenceDesiredSource::AndroidScrcpyUsb { alpha_mode }) => {
            app.canvas.reference.alpha_mode = alpha_mode;
            if app.shell.android_reference.status().running {
                return;
            }

            let attempt_key = ReferenceAttemptKey::AndroidScrcpyUsb { alpha_mode };
            if app.canvas.reference.last_attempt_key.as_ref() == Some(&attempt_key) {
                return;
            }
            app.canvas.reference.last_attempt_key = Some(attempt_key);

            match app.shell.android_reference.start_usb() {
                Ok(status) => {
                    eprintln!("[android-reference] started from ReferenceImage source: {status}");
                    app.canvas.reference.last_attempt_key = None;
                    ctx.request_repaint();
                }
                Err(error) => {
                    eprintln!(
                        "[android-reference] failed to start from ReferenceImage source: {error:#}"
                    );
                }
            }
        }
        Some(ReferenceDesiredSource::SceneAsset {
            asset_id,
            alpha_mode,
        }) => {
            app.shell.android_reference.stop();
            let already_loaded = matches!(
                app.canvas.reference.ref_image.as_ref(),
                Some(r)
                    if matches!(&r.source, RefImageSource::SceneNodeAssetId(v) if v == &asset_id)
                        && r.alpha_mode == alpha_mode
            );
            if already_loaded {
                return;
            }

            let attempt_key = ReferenceAttemptKey::Asset {
                asset_id: asset_id.clone(),
                alpha_mode,
                asset_store_revision: app.core.asset_store.revision(),
            };
            if app.canvas.reference.last_attempt_key.as_ref() == Some(&attempt_key) {
                return;
            }
            app.canvas.reference.last_attempt_key = Some(attempt_key);

            if !app.core.asset_store.contains(&asset_id) {
                return;
            }

            match app.core.asset_store.load_image(&asset_id) {
                Ok(Some(decoded)) => {
                    if let Err(e) = load_reference_image_from_decoded(
                        app,
                        ctx,
                        render_state,
                        decoded,
                        format!("ReferenceImage(assetId:{asset_id})"),
                        RefImageSource::SceneNodeAssetId(asset_id.clone()),
                        alpha_mode,
                    ) {
                        eprintln!("[reference-image] failed to load asset '{asset_id}': {e:#}");
                    }
                }
                Ok(None) => {
                    app.canvas.reference.last_attempt_key = None;
                    eprintln!("[reference-image] asset '{asset_id}' not found in asset store");
                }
                Err(e) => {
                    eprintln!("[reference-image] failed to decode asset '{asset_id}': {e:#}");
                }
            }
        }
        Some(ReferenceDesiredSource::SceneDataUrl {
            data_hash,
            original_data_url,
            alpha_mode,
        }) => {
            app.shell.android_reference.stop();
            let already_loaded = matches!(
                app.canvas.reference.ref_image.as_ref(),
                Some(r)
                    if matches!(
                        &r.source,
                        RefImageSource::SceneNodeDataUrl(v) if v == &original_data_url
                    ) && r.alpha_mode == alpha_mode
            );
            if already_loaded {
                return;
            }

            let attempt_key = ReferenceAttemptKey::DataUrl {
                data_hash,
                alpha_mode,
            };
            if app.canvas.reference.last_attempt_key.as_ref() == Some(&attempt_key) {
                return;
            }
            app.canvas.reference.last_attempt_key = Some(attempt_key);

            match crate::renderer::utils::load_image_from_data_url(&original_data_url) {
                Ok(decoded) => {
                    if let Err(e) = load_reference_image_from_decoded(
                        app,
                        ctx,
                        render_state,
                        decoded,
                        "ReferenceImage(dataUrl)".to_string(),
                        RefImageSource::SceneNodeDataUrl(original_data_url),
                        alpha_mode,
                    ) {
                        eprintln!("[reference-image] failed to load ReferenceImage.dataUrl: {e:#}");
                    }
                }
                Err(e) => {
                    eprintln!("[reference-image] failed to decode ReferenceImage.dataUrl: {e:#}");
                }
            }
        }
        Some(ReferenceDesiredSource::ScenePath { path, alpha_mode }) => {
            app.shell.android_reference.stop();
            let already_loaded = matches!(
                app.canvas.reference.ref_image.as_ref(),
                Some(r)
                    if matches!(&r.source, RefImageSource::SceneNodePath(p) if p == &path)
                        && r.alpha_mode == alpha_mode
            );
            if already_loaded {
                return;
            }

            let attempt_key = ReferenceAttemptKey::Path {
                path: path.clone(),
                alpha_mode,
            };
            if app.canvas.reference.last_attempt_key.as_ref() == Some(&attempt_key) {
                return;
            }
            app.canvas.reference.last_attempt_key = Some(attempt_key);

            let Some(resolved_path) = resolve_reference_image_path(&path) else {
                return;
            };
            if let Err(e) = load_reference_image_from_path(
                app,
                ctx,
                render_state,
                &resolved_path,
                RefImageSource::SceneNodePath(path.clone()),
                alpha_mode,
            ) {
                eprintln!(
                    "[reference-image] failed to load ReferenceImage.path='{}' (resolved '{}'): {e:#}",
                    path,
                    resolved_path.display()
                );
            }
        }
        None => {
            app.shell.android_reference.stop();
            app.canvas.reference.last_attempt_key = None;
            if matches!(
                app.canvas.reference.ref_image.as_ref().map(|r| &r.source),
                Some(
                    RefImageSource::SceneNodePath(_)
                        | RefImageSource::SceneNodeDataUrl(_)
                        | RefImageSource::SceneNodeAssetId(_)
                        | RefImageSource::AndroidScrcpyUsb(_)
                )
            ) {
                clear_reference_internal(app, false);
            }
        }
    }
}

pub fn set_reference_alpha_mode(
    queue: &wgpu::Queue,
    reference: &mut RefImageState,
    alpha_mode: RefImageAlphaMode,
) -> anyhow::Result<bool> {
    if reference.alpha_mode == alpha_mode {
        return Ok(false);
    }
    if matches!(reference.source, RefImageSource::AndroidScrcpyUsb(_))
        && reference.source_linear_rgba.is_empty()
    {
        reference.alpha_mode = alpha_mode;
        return Ok(true);
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

pub fn pick_reference_image_from_dialog(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
) -> anyhow::Result<bool> {
    let mut picker = rfd::FileDialog::new().add_filter("Image", &["png", "exr", "jpg", "jpeg"]);
    if let Some(reference) = app.canvas.reference.ref_image.as_ref() {
        picker = picker.set_file_name(reference.name.as_str());
    }

    let Some(path) = picker.pick_file() else {
        return Ok(false);
    };

    load_reference_image_from_path(
        app,
        ctx,
        render_state,
        &path,
        RefImageSource::Manual,
        app.canvas.reference.alpha_mode,
    )?;
    app.canvas.reference.desired_override = Some(ReferenceDesiredSource::Manual);
    app.canvas.reference.last_attempt_key = None;
    Ok(true)
}

fn encode_rgba8_png(rgba: &image::RgbaImage) -> anyhow::Result<Vec<u8>> {
    let mut png_bytes = Vec::new();
    PngEncoder::new(&mut png_bytes)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|error| anyhow::anyhow!("failed to encode shortwire reference png: {error}"))?;
    Ok(png_bytes)
}

pub fn load_shortwire_reference_image(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
    snapshot: &ShortwireReferenceImage,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let decoded = image::load_from_memory(bytes)
        .map_err(|error| anyhow::anyhow!("failed to decode shortwire reference image: {error}"))?;
    load_reference_image_from_decoded(
        app,
        ctx,
        render_state,
        decoded,
        snapshot.name.clone(),
        RefImageSource::ShortwirePatch,
        snapshot.alpha_mode,
    )?;
    if let Some(reference) = app.canvas.reference.ref_image.as_mut() {
        reference.mode = snapshot.mode;
        reference.opacity = snapshot.opacity.clamp(0.0, 1.0);
        reference.offset = egui::vec2(snapshot.offset[0], snapshot.offset[1]);
    }
    app.canvas.reference.desired_override = Some(ReferenceDesiredSource::Manual);
    app.canvas.reference.last_attempt_key = None;
    app.canvas.invalidation.reference_mode_changed();
    eprintln!(
        "[shortwire-diff] loaded stored reference image name={} size={}x{} alpha_mode={:?} mode={:?} offset={:.2},{:.2}",
        snapshot.name,
        snapshot.width,
        snapshot.height,
        snapshot.alpha_mode,
        snapshot.mode,
        snapshot.offset[0],
        snapshot.offset[1],
    );
    Ok(())
}

pub fn paste_shortwire_reference_from_clipboard(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
) -> anyhow::Result<Option<ShortwirePastedReferenceImage>> {
    eprintln!(
        "[shortwire-paste] arboard start alpha_mode={:?} existing_ref={}",
        app.canvas.reference.alpha_mode,
        app.canvas
            .reference
            .ref_image
            .as_ref()
            .map(|reference| format!(
                "{} {:?} {}x{}",
                reference.name, reference.source, reference.size[0], reference.size[1]
            ))
            .unwrap_or_else(|| "none".to_string())
    );
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            eprintln!("[shortwire-paste] arboard init failed: {error}");
            return Err(error.into());
        }
    };
    let clipboard_image = match clipboard.get_image() {
        Ok(image) => {
            eprintln!(
                "[shortwire-paste] arboard image found width={} height={} bytes={}",
                image.width,
                image.height,
                image.bytes.len()
            );
            image
        }
        Err(arboard::Error::ContentNotAvailable) => {
            eprintln!("[shortwire-paste] arboard content not available");
            return Ok(None);
        }
        Err(error) => {
            eprintln!("[shortwire-paste] arboard get_image failed: {error}");
            return Err(error.into());
        }
    };
    let width: u32 = clipboard_image
        .width
        .try_into()
        .map_err(|_| anyhow::anyhow!("clipboard image width too large"))?;
    let height: u32 = clipboard_image
        .height
        .try_into()
        .map_err(|_| anyhow::anyhow!("clipboard image height too large"))?;
    let rgba = image::RgbaImage::from_raw(width, height, clipboard_image.bytes.into_owned())
        .ok_or_else(|| anyhow::anyhow!("clipboard image has invalid RGBA data"))?;
    let png_bytes = encode_rgba8_png(&rgba)?;
    let alpha_mode = app.canvas.reference.alpha_mode;
    let name = format!("Shortwire clipboard {width}x{height}");
    load_reference_image_from_decoded(
        app,
        ctx,
        render_state,
        image::DynamicImage::ImageRgba8(rgba),
        name.clone(),
        RefImageSource::ShortwireClipboard,
        alpha_mode,
    )?;
    app.canvas.reference.desired_override = Some(ReferenceDesiredSource::Manual);
    app.canvas.reference.last_attempt_key = None;
    eprintln!(
        "[shortwire-paste] loaded shortwire clipboard reference width={width} height={height} alpha_mode={alpha_mode:?}"
    );
    Ok(Some(ShortwirePastedReferenceImage {
        name,
        png_bytes,
        width,
        height,
        alpha_mode,
        mode: RefImageMode::Overlay,
        opacity: 0.5,
        offset: [0.0, 0.0],
    }))
}

pub fn maybe_handle_reference_drop(
    app: &mut App,
    ctx: &egui::Context,
    render_state: &egui_wgpu::RenderState,
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
            &path,
            RefImageSource::Manual,
            app.canvas.reference.alpha_mode,
        )
        .is_ok()
        {
            app.canvas.reference.desired_override = Some(ReferenceDesiredSource::Manual);
            app.canvas.reference.last_attempt_key = None;
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RefImageAlphaMode, apply_reference_alpha_mode_to_linear_rgba, decode_reference_image,
        is_supported_reference_image, reference_texture_format, validate_android_reference_rgba8,
    };
    use image::{DynamicImage, ImageBuffer, Rgba, RgbaImage};
    use rust_wgpu_fiber::eframe::wgpu;
    use std::path::Path;

    #[test]
    fn supported_reference_image_extensions_include_exr_and_png() {
        assert!(is_supported_reference_image(Path::new("foo.exr")));
        assert!(is_supported_reference_image(Path::new("foo.png")));
        assert!(!is_supported_reference_image(Path::new("foo.gif")));
    }

    #[test]
    fn alpha_mode_conversion_differs_between_premultiplied_and_straight() {
        let rgba = vec![0.8, 0.6, 0.4, 0.5];
        assert_eq!(
            apply_reference_alpha_mode_to_linear_rgba(&rgba, RefImageAlphaMode::Premultiplied),
            rgba
        );
        assert_eq!(
            apply_reference_alpha_mode_to_linear_rgba(&rgba, RefImageAlphaMode::Straight),
            vec![0.4, 0.3, 0.2, 0.5]
        );
    }

    #[test]
    fn sdr_decode_applies_srgb_to_linear() {
        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([128, 0, 0, 255])));
        let decoded = decode_reference_image(image, RefImageAlphaMode::Premultiplied);
        assert!(decoded.source_linear_rgba[0] < 0.3);
    }

    #[test]
    fn android_reference_rgba8_validation_keeps_frame_bytes() {
        let decoded = validate_android_reference_rgba8(1, 1, vec![128, 0, 255, 255]).unwrap();
        assert_eq!(decoded.rgba8, vec![128, 0, 255, 255]);
    }

    #[test]
    fn u16_decode_uses_high_precision_reference_texture_format() {
        let image =
            DynamicImage::ImageRgba16(ImageBuffer::from_pixel(1, 1, image::Rgba([0, 0, 0, 65535])));
        let decoded = decode_reference_image(image, RefImageAlphaMode::Premultiplied);
        assert_eq!(
            reference_texture_format(&decoded),
            wgpu::TextureFormat::Rgba16Float
        );
    }
}
