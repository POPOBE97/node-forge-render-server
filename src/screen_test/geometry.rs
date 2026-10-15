use super::{
    model::*,
    raw::{RawFrame, native},
};
use crate::pixel_perfecto::calibration::{Calibration, Correspondence, fit_planar_correspondences};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
#[derive(Clone, Deserialize, Serialize)]
pub struct DetectedMarker {
    pub id: i32,
    pub corners: Vec<[f64; 2]>,
}
#[derive(Deserialize)]
struct Detection {
    markers: Vec<DetectedMarker>,
}
pub fn detect(raw: &RawFrame, directory: &Path) -> Result<Vec<DetectedMarker>> {
    let path = directory.join(format!("{}.png", new_id("detect")));
    raw.detection_image().save(&path)?;
    let result = native(&[std::ffi::OsStr::new("detect"), path.as_os_str()]);
    let _ = fs::remove_file(path);
    Ok(serde_json::from_slice::<Detection>(&result?)?.markers)
}
pub fn correspondences(
    manifest: &PatternManifest,
    detected: &[DetectedMarker],
    height: u32,
) -> Result<Vec<(i32, Correspondence)>> {
    ensure!(
        !detected.is_empty(),
        "No chart markers could be read from the RAW capture. Keep the full chart visible and check focus/exposure; the pattern identity could not be verified."
    );
    // Missing markers do not prove that a photograph is stale. Only a decoded different
    // generation nibble is an identity mismatch. Never infer/recover IDs from their position.
    let identity: Vec<_> = manifest.markers.iter().filter(|m| m.id >= 128).collect();
    let mut missing = Vec::new();
    for expected in &identity {
        let slot = (expected.id - 128) / 16;
        let observed: Vec<_> = detected
            .iter()
            .filter(|m| m.id >= 128 && (m.id - 128) / 16 == slot)
            .map(|m| m.id)
            .collect();
        ensure!(
            observed.iter().all(|id| *id == expected.id),
            "Pattern identity mismatch: expected marker {}, read {:?}. The photographed chart differs from the current USB presentation; run calibration again.",
            expected.id,
            observed
        );
        if observed.is_empty() {
            missing.push(expected.id);
        }
    }
    ensure!(
        missing.is_empty(),
        "Pattern identity incomplete: read {} of {} identity markers ({} chart markers detected; missing {:?}). Keep the full chart visible and check focus/exposure; this does not establish that the frame is old.",
        identity.len() - missing.len(),
        identity.len(),
        detected.len(),
        missing
    );
    let mut result = Vec::new();
    for marker in &manifest.markers {
        let Some(found) = detected.iter().find(|d| d.id == marker.id) else {
            continue;
        };
        ensure!(found.corners.len() == 4, "Invalid marker corners");
        ensure!(
            detected.iter().filter(|d| d.id == marker.id).count() == 1,
            "Multiple copies of a chart marker are visible; show one screen chart only"
        );
        ensure!(
            found.corners.iter().flatten().all(|v| v.is_finite()),
            "Invalid marker coordinates"
        );
        let [x, y, w, h] = marker.rect;
        // OpenCV returns canonical marker corner order, decoded from its bit pattern.
        // Corner zero follows the marker's rotation; never sort corners by photo X/Y.
        // RAW stays in sensor coordinates, regardless of its EXIF orientation.
        let corners = [
            [x as f64, y as f64 + h as f64 - 1.],
            [(x + w - 1) as f64, (y + h - 1) as f64],
            [(x + w - 1) as f64, y as f64],
            [x as f64, y as f64],
        ];
        for (target, source) in corners.into_iter().zip(&found.corners) {
            result.push((
                marker.id,
                Correspondence {
                    source: [source[0], height as f64 - 1. - source[1]],
                    target: [
                        target[0] / (manifest.size[0] - 1) as f64,
                        target[1] / (manifest.size[1] - 1) as f64,
                    ],
                    enabled: true,
                },
            ));
        }
    }
    ensure!(
        result.len() >= 48,
        "Need at least 12 visible markers covering the screen"
    );
    ensure!(
        detected
            .iter()
            .filter(|m| (100..=102).contains(&m.id))
            .count()
            >= 2,
        "Need at least two interior markers to constrain distortion"
    );
    for axis in 0..2 {
        let observed_low = result
            .iter()
            .map(|(_, p)| p.target[axis])
            .fold(f64::INFINITY, f64::min);
        let observed_high = result
            .iter()
            .map(|(_, p)| p.target[axis])
            .fold(f64::NEG_INFINITY, f64::max);
        let dimension = (manifest.size[axis] - 1) as f64;
        let expected_low = manifest
            .markers
            .iter()
            .map(|m| m.rect[axis] as f64 / dimension)
            .fold(f64::INFINITY, f64::min);
        let expected_high = manifest
            .markers
            .iter()
            .map(|m| (m.rect[axis] + m.rect[axis + 2] - 1) as f64 / dimension)
            .fold(f64::NEG_INFINITY, f64::max);
        let tolerance = (expected_high - expected_low) * 0.1;
        ensure!(
            observed_low <= expected_low + tolerance && observed_high >= expected_high - tolerance,
            "Markers must cover all four sides of the chart; a cropped cluster cannot calibrate the screen"
        );
    }
    Ok(result)
}
pub fn fit(
    raw: &RawFrame,
    manifest: &PatternManifest,
    detected: &[DetectedMarker],
    capture: &str,
) -> Result<GeometryProfile> {
    let pairs = correspondences(manifest, detected, raw.metadata.height)?;
    let (validation, training): (Vec<_>, Vec<_>) =
        pairs.into_iter().partition(|(id, _)| id % 5 == 0);
    ensure!(validation.len() >= 8, "Need distributed holdout markers");
    let points: Vec<_> = training.into_iter().map(|(_, p)| p).collect();
    let size = [raw.metadata.width, raw.metadata.height];
    let fitted = fit_planar_correspondences(
        &points,
        &vec![1.; points.len()],
        size,
        &Calibration::initial(size),
    )?;
    let rms = (validation
        .iter()
        .map(|(_, p)| {
            let q = fitted
                .calibration
                .map(p.target, size)
                .unwrap_or([f64::INFINITY; 2]);
            (q[0] - p.source[0]).powi(2) + (q[1] - p.source[1]).powi(2)
        })
        .sum::<f64>()
        / validation.len() as f64)
        .sqrt();
    ensure!(
        rms.is_finite() && rms <= 1.,
        "Geometry holdout error {rms:.2} px exceeds 1 px; improve focus/coverage"
    );
    Ok(GeometryProfile {
        id: new_id("geometry"),
        revision: 0,
        created_ms: now_ms(),
        source_capture: capture.into(),
        camera_key: raw.metadata.geometry_key(),
        raw_size: size,
        mapping: fitted.calibration,
        holdout_rms: rms,
        marker_count: detected.len(),
    })
}
pub fn validate_snapshot(
    profile: &GeometryProfile,
    raw: &RawFrame,
    manifest: &PatternManifest,
    detected: &[DetectedMarker],
) -> Result<f64> {
    ensure!(
        profile.holdout_rms.is_finite() && (0.0..=1.0).contains(&profile.holdout_rms),
        "Geometry profile failed holdout validation"
    );
    profile.mapping.validate(profile.raw_size)?;
    ensure!(
        profile.raw_size == [raw.metadata.width, raw.metadata.height]
            && profile.camera_key == raw.metadata.geometry_key(),
        "Camera, lens or focal length changed; run Geometry Calibration"
    );
    let pairs = correspondences(manifest, detected, raw.metadata.height)?;
    let max = pairs
        .iter()
        .map(|(_, p)| {
            profile
                .mapping
                .map(p.target, profile.raw_size)
                .map(|v| ((v[0] - p.source[0]).powi(2) + (v[1] - p.source[1]).powi(2)).sqrt())
        })
        .collect::<Option<Vec<_>>>()
        .context("Invalid geometry mapping")?
        .into_iter()
        .fold(0., f64::max);
    ensure!(
        max <= 3.,
        "Shared geometry no longer matches this phone placement ({max:.1} px). Recalibrate geometry; existing parameters were retained."
    );
    Ok(max)
}

/// The only color-measurement input: optically verified against this RAW and rectified first.
/// Fields are private so callers cannot skip geometry by passing an arbitrary profile.
pub struct RectifiedRaw<'a> {
    raw: &'a RawFrame,
    profile: &'a GeometryProfile,
    manifest: &'a PatternManifest,
    pub max_residual_px: f64,
}
impl<'a> RectifiedRaw<'a> {
    pub(crate) fn parts(&self) -> (&'a RawFrame, &'a GeometryProfile, &'a PatternManifest) {
        (self.raw, self.profile, self.manifest)
    }
}
pub fn rectify_for_color<'a>(
    raw: &'a RawFrame,
    profile: &'a GeometryProfile,
    manifest: &'a PatternManifest,
    folder: &Path,
) -> Result<RectifiedRaw<'a>> {
    std::fs::create_dir_all(folder)?;
    let detected = detect(raw, folder)?;
    let max_residual_px = validate_snapshot(profile, raw, manifest, &detected)?;
    super::measurement::export(raw, profile, manifest, None, folder)?;
    export_overlay(raw, manifest, &detected, profile, folder)?;
    Ok(RectifiedRaw {
        raw,
        profile,
        manifest,
        max_residual_px,
    })
}

pub fn export_overlay(
    raw: &RawFrame,
    manifest: &PatternManifest,
    detected: &[DetectedMarker],
    profile: &GeometryProfile,
    folder: &Path,
) -> Result<()> {
    let mut preview = raw.sensor_preview();
    preview.save(folder.join("sensor-preview.png"))?;
    let mut dot = |x: i32, y: i32, color: image::Rgb<u8>| {
        for dy in -2..=2 {
            for dx in -2..=2 {
                if x + dx >= 0
                    && y + dy >= 0
                    && x + dx < preview.width() as i32
                    && y + dy < preview.height() as i32
                {
                    preview.put_pixel((x + dx) as u32, (y + dy) as u32, color);
                }
            }
        }
    };
    for marker in detected {
        for corner in &marker.corners {
            dot(
                corner[0].round() as i32,
                corner[1].round() as i32,
                image::Rgb([0, 220, 255]),
            );
        }
    }
    for patch in &manifest.patches {
        let [x, y, w, h] = patch.rect;
        let pad = w.min(h) as f64 * 0.2;
        let corners = [
            [x as f64 + pad, y as f64 + pad],
            [(x + w) as f64 - pad, y as f64 + pad],
            [(x + w) as f64 - pad, (y + h) as f64 - pad],
            [x as f64 + pad, (y + h) as f64 - pad],
        ];
        for edge in 0..4 {
            for step in 0..=100 {
                let t = step as f64 / 100.;
                let a = corners[edge];
                let b = corners[(edge + 1) % 4];
                let uv = [
                    (a[0] * (1. - t) + b[0] * t) / (manifest.size[0] - 1) as f64,
                    (a[1] * (1. - t) + b[1] * t) / (manifest.size[1] - 1) as f64,
                ];
                if let Some(p) = profile.mapping.map(uv, profile.raw_size) {
                    dot(
                        p[0].round() as i32,
                        (raw.metadata.height - 1) as i32 - p[1].round() as i32,
                        image::Rgb([255, 190, 40]),
                    );
                }
            }
        }
    }
    preview.save(folder.join("geometry-overlay.png"))?;
    Ok(())
}
