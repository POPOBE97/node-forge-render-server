use std::path::Path;

use rust_wgpu_fiber::eframe::{egui, egui_wgpu, wgpu};

use crate::app::types::{
    App, RefImageAlphaMode, RefImageMode, RefImageSource, RefImageState, RefImageTransferMode,
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

fn clear_reference_internal(app: &mut App, clear_override: bool) {
    app.canvas.reference.ref_image = None;
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

    clear_reference_internal(app, false);
    app.canvas.reference.ref_image = Some(RefImageState {
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
    app.canvas.invalidation.reference_mode_changed();
    Ok(())
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
        Some(ReferenceDesiredSource::SceneAsset {
            asset_id,
            alpha_mode,
        }) => {
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
                asset_store_revision: app.asset_store.revision(),
            };
            if app.canvas.reference.last_attempt_key.as_ref() == Some(&attempt_key) {
                return;
            }
            app.canvas.reference.last_attempt_key = Some(attempt_key);

            if !app.asset_store.contains(&asset_id) {
                return;
            }

            match app.asset_store.load_image(&asset_id) {
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
            app.canvas.reference.last_attempt_key = None;
            if matches!(
                app.canvas.reference.ref_image.as_ref().map(|r| &r.source),
                Some(
                    RefImageSource::SceneNodePath(_)
                        | RefImageSource::SceneNodeDataUrl(_)
                        | RefImageSource::SceneNodeAssetId(_)
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
        is_supported_reference_image, reference_texture_format,
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
