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

/// Ensure the image is RGBA8 for GPU upload.
///
/// rust-wgpu-fiber's image texture path selects wgpu texture format based on `image.color()`.
/// For RGB images it maps to RGBA formats (because wgpu has no RGB8), so we must ensure
/// the pixel buffer is actually RGBA to keep `bytes_per_row` consistent.
pub(crate) fn ensure_rgba8(image: Arc<DynamicImage>) -> Arc<DynamicImage> {
    if image.color() == image::ColorType::Rgba8 {
        return image;
    }
    Arc::new(DynamicImage::ImageRgba8(image.as_ref().to_rgba8()))
}

/// Load an image from a data URL string (legacy path).
pub(crate) fn load_image_from_data_url_checked(
    data_url: &str,
    node_id: &str,
) -> Result<Arc<DynamicImage>> {
    match load_image_from_data_url(data_url) {
        Ok(img) => Ok(ensure_rgba8(Arc::new(img))),
        Err(e) => bail!(
            "ImageTexture node '{node_id}': failed to load image from dataUrl: {e}"
        ),
    }
}
