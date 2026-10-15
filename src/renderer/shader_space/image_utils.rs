//! Image loading utilities.
//!
//! Handles loading images from asset stores, data URLs, and file paths, plus
//! format normalisation for GPU upload.

use std::{io::Cursor, path::PathBuf, sync::Arc};

use anyhow::{Result, bail};
use image::DynamicImage;

use crate::renderer::utils::{decode_data_url, load_image_from_data_url};

// ── Image dimension probing ──────────────────────────────────────────────

pub(crate) fn image_node_dimensions(
    node: &crate::dsl::Node,
    asset_store: Option<&crate::asset_store::AssetStore>,
) -> Option<[u32; 2]> {
    // Prefer assetId → asset_store lookup.
    if let Some(asset_id) = node
        .params
        .get("assetId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        if let Some(store) = asset_store {
            if let Some(data) = store.get(asset_id) {
                let reader = image::ImageReader::new(Cursor::new(&data.bytes))
                    .with_guessed_format()
                    .ok()?;
                return reader.into_dimensions().ok().map(|(w, h)| [w, h]);
            }
        }
    }

    // Legacy fallback: dataUrl.
    let data_url = node
        .params
        .get("dataUrl")
        .and_then(|v| v.as_str())
        .or_else(|| node.params.get("data_url").and_then(|v| v.as_str()));

    if let Some(s) = data_url.filter(|s| !s.trim().is_empty()) {
        let bytes = decode_data_url(s).ok()?;
        let reader = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        return reader.into_dimensions().ok().map(|(w, h)| [w, h]);
    }

    // Legacy fallback: file path.
    let rel_base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = node.params.get("path").and_then(|v| v.as_str());
    let p = path.filter(|s| !s.trim().is_empty())?;

    let candidates: Vec<PathBuf> = {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            vec![pb]
        } else {
            vec![
                pb.clone(),
                rel_base.join(&pb),
                rel_base.join("assets").join(&pb),
            ]
        }
    };

    for cand in &candidates {
        if let Ok((w, h)) = image::image_dimensions(cand) {
            return Some([w, h]);
        }
    }

    None
}

// ── Image loading ────────────────────────────────────────────────────────

pub(crate) fn load_image_from_path(
    rel_base: &PathBuf,
    path: Option<&str>,
    node_id: &str,
) -> Result<Arc<DynamicImage>> {
    let Some(p) = path.filter(|s| !s.trim().is_empty()) else {
        bail!("ImageTexture node '{node_id}' has no path specified");
    };

    let candidates: Vec<PathBuf> = {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            vec![pb]
        } else {
            vec![
                pb.clone(),
                rel_base.join(&pb),
                rel_base.join("assets").join(&pb),
            ]
        }
    };

    for cand in &candidates {
        if let Ok(img) = image::open(cand) {
            return Ok(Arc::new(img));
        }
    }

    bail!(
        "ImageTexture node '{node_id}': failed to load image from path '{}'. Tried: {:?}",
        p,
        candidates
    );
}

/// Expand grayscale/RGB to RGBA without changing the source component precision.
/// The upload buffer must have the same channel layout as its GPU texture format.
pub(crate) fn normalize_image_channels(image: Arc<DynamicImage>) -> Result<Arc<DynamicImage>> {
    use image::ColorType;
    Ok(match image.color() {
        ColorType::Rgba8 | ColorType::Rgba16 | ColorType::Rgba32F => image,
        ColorType::L8 | ColorType::La8 | ColorType::Rgb8 => {
            Arc::new(DynamicImage::ImageRgba8(image.to_rgba8()))
        }
        ColorType::L16 | ColorType::La16 | ColorType::Rgb16 => {
            Arc::new(DynamicImage::ImageRgba16(image.to_rgba16()))
        }
        ColorType::Rgb32F => Arc::new(DynamicImage::ImageRgba32F(image.to_rgba32f())),
        other => bail!("unsupported image component format: {other:?}"),
    })
}

/// Prepare high precision images before filtering. There is no 16/32-bit sRGB GPU
/// format: applying an sRGB suffix would leave these encoded samples undecoded.
/// Use f32 for transfer/primary conversion and premultiplication, retaining all
/// 16-bit source levels and the range/precision of floating-point sources.
pub(crate) fn prepare_high_precision_image(
    image: &DynamicImage,
    decode_srgb_transfer: bool,
    display_p3: bool,
    premultiply: bool,
) -> Arc<DynamicImage> {
    let mut rgba = image.to_rgba32f();
    for pixel in rgba.pixels_mut() {
        let [r, g, b, alpha] = pixel.0;
        let mut rgb = [r, g, b];
        if decode_srgb_transfer {
            rgb = rgb.map(|value| {
                value.signum() * crate::color::srgb_to_linear_channel(value.abs())
            });
        }
        if display_p3 {
            rgb = crate::color::linear_p3_to_linear_srgb(rgb);
        }
        if premultiply {
            rgb = rgb.map(|value| value * alpha);
        }
        // In particular, never clamp negative or >1 floating-point values.
        pixel.0 = [rgb[0], rgb[1], rgb[2], alpha];
    }
    Arc::new(DynamicImage::ImageRgba32F(rgba))
}

/// Load an image from a data URL string (legacy path).
pub(crate) fn load_image_from_data_url_checked(
    data_url: &str,
    node_id: &str,
) -> Result<Arc<DynamicImage>> {
    match load_image_from_data_url(data_url) {
        Ok(img) => Ok(Arc::new(img)),
        Err(e) => bail!("ImageTexture node '{node_id}': failed to load image from dataUrl: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, LumaA, Rgb};

    #[test]
    fn image_channel_expansion_preserves_component_depth_and_alpha() {
        let gray = DynamicImage::ImageLumaA16(
            ImageBuffer::from_raw(2, 1, vec![32768, 12345, 32769, 54321]).unwrap(),
        );
        let expanded = normalize_image_channels(Arc::new(gray)).unwrap();
        assert_eq!(expanded.color(), image::ColorType::Rgba16);
        assert_eq!(
            expanded.as_rgba16().unwrap().as_raw(),
            &[32768, 32768, 32768, 12345, 32769, 32769, 32769, 54321]
        );
        let gray8 = DynamicImage::ImageLumaA8(ImageBuffer::from_pixel(1, 1, LumaA([17, 128])));
        let expanded = normalize_image_channels(Arc::new(gray8)).unwrap();
        assert_eq!(expanded.as_rgba8().unwrap().as_raw(), &[17, 17, 17, 128]);
        let rgb = DynamicImage::ImageRgb32F(ImageBuffer::from_pixel(
            1, 1, Rgb([-0.25, 0.500_123_4, 100_000.0]),
        ));
        let expanded = normalize_image_channels(Arc::new(rgb)).unwrap();
        assert_eq!(expanded.color(), image::ColorType::Rgba32F);
        assert_eq!(
            expanded.as_rgba32f().unwrap().as_raw(),
            &[-0.25, 0.500_123_4, 100_000.0, 1.0]
        );
    }

    #[test]
    fn image_u16_transfer_keeps_all_65536_source_levels_distinct() {
        let image = DynamicImage::ImageLuma16(ImageBuffer::from_fn(65536, 1, |x, _| {
            image::Luma([x as u16])
        }));
        let prepared = prepare_high_precision_image(&image, true, false, false);
        let pixels = prepared.as_rgba32f().unwrap();
        let mut previous = -1.0;
        for pixel in pixels.pixels() {
            assert!(pixel[0] > previous);
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[1], pixel[2]);
            assert_eq!(pixel[3], 1.0);
            previous = pixel[0];
        }
    }

    #[test]
    fn image_linear_float_upload_preserves_bits_and_unbounded_values() {
        let values = vec![-0.25, 0.500_123_4, 100_000.0, 1.5];
        let image = DynamicImage::ImageRgba32F(ImageBuffer::from_raw(1, 1, values.clone()).unwrap());
        let prepared = prepare_high_precision_image(&image, false, false, false);
        assert_eq!(prepared.as_rgba32f().unwrap().as_raw(), &values);
        let prepared = prepare_high_precision_image(&image, false, false, true);
        assert_eq!(
            prepared.as_rgba32f().unwrap().as_raw(),
            &[-0.375, values[1] * 1.5, 150_000.0, 1.5]
        );
    }
}
