use super::{model::*, raw::RawFrame};
use anyhow::{Context, Result, ensure};

pub fn compatible(
    reference: &CaptureConditions,
    test: &CaptureConditions,
    same_reference_group: bool,
) -> Result<()> {
    ensure!(
        reference.manifest.version == test.manifest.version
            && reference.manifest.mode == test.manifest.mode
            && reference.manifest.peak == test.manifest.peak,
        "Reference chart or display mode/range differs"
    );
    ensure!(
        reference.camera_key == test.camera_key,
        "Reference camera, lens, ISO, aperture or focal length differs"
    );
    ensure!(
        reference.manifest.patches.len() == test.manifest.patches.len()
            && reference
                .manifest
                .patches
                .iter()
                .zip(&test.manifest.patches)
                .all(|(a, b)| a.id == b.id && a.rgb == b.rgb),
        "Reference patch semantics differ"
    );
    if same_reference_group {
        ensure!(
            reference.device.id == test.device.id
                && (reference.brightness - test.brightness).abs() < 0.001
                && reference.batch == test.batch
                && reference.notes == test.notes
                && (reference.device.headroom - test.device.headroom).abs()
                    < 0.05 * reference.device.headroom.max(1.),
            "Reference captures have different device, brightness, batch or conditions"
        );
    }
    Ok(())
}
pub fn measure(rectified: &super::geometry::RectifiedRaw<'_>) -> Result<Measurements> {
    let (raw, geometry, manifest) = rectified.parts();
    let m = &raw.metadata;
    ensure!(
        geometry.raw_size == [m.width, m.height],
        "RAW dimensions changed"
    );
    let mut patches = Vec::new();
    let mut issues = Vec::new();
    for patch in &manifest.patches {
        let [x, y, w, h] = patch.rect;
        let margin = (w.min(h) as f64 * 0.2).max(3.);
        let low = [
            (x as f64 + margin) / (manifest.size[0] - 1) as f64,
            (y as f64 + margin) / (manifest.size[1] - 1) as f64,
        ];
        let high = [
            ((x + w) as f64 - margin) / (manifest.size[0] - 1) as f64,
            ((y + h) as f64 - margin) / (manifest.size[1] - 1) as f64,
        ];
        let corners = [
            [low[0], low[1]],
            [low[0], high[1]],
            [high[0], low[1]],
            [high[0], high[1]],
        ]
        .map(|v| geometry.mapping.map(v, geometry.raw_size));
        let corners = corners
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .context("Patch outside geometric transform")?;
        let minx = corners
            .iter()
            .map(|p| p[0])
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.) as u32;
        let maxx = corners
            .iter()
            .map(|p| p[0])
            .fold(0., f64::max)
            .ceil()
            .min((m.width - 1) as f64) as u32;
        let miny = corners
            .iter()
            .map(|p| p[1])
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.) as u32;
        let maxy = corners
            .iter()
            .map(|p| p[1])
            .fold(0., f64::max)
            .ceil()
            .min((m.height - 1) as f64) as u32;
        let mut values: [Vec<f64>; 4] = std::array::from_fn(|_| Vec::new());
        let mut rows = std::collections::BTreeMap::<u32, (f64, u64)>::new();
        let mut total = 0u64;
        let mut rejected = 0u64;
        for yy in miny..=maxy {
            for xx in minx..=maxx {
                let Some(uv) = geometry
                    .mapping
                    .unmap([xx as f64, yy as f64], geometry.raw_size)
                else {
                    continue;
                };
                if uv[0] < low[0] || uv[0] > high[0] || uv[1] < low[1] || uv[1] > high[1] {
                    continue;
                }
                total += 1;
                let (c, v, saturated) = raw.sample(xx, m.height - 1 - yy);
                if saturated {
                    rejected += 1
                } else {
                    values[c].push(v);
                    if c == 1 || c == 3 {
                        let row = rows.entry(yy / 8).or_default();
                        row.0 += v;
                        row.1 += 1;
                    }
                }
            }
        }
        let mut mean = [0.; 4];
        let mut dev = [0.; 4];
        let mut count = [0; 4];
        for c in 0..4 {
            values[c].sort_by(f64::total_cmp);
            let trim = values[c].len() / 100;
            let samples = &values[c][trim..values[c].len().saturating_sub(trim)];
            count[c] = samples.len() as u64;
            if !samples.is_empty() {
                mean[c] = samples.iter().sum::<f64>() / samples.len() as f64;
                dev[c] = (samples.iter().map(|v| (v - mean[c]).powi(2)).sum::<f64>()
                    / samples.len() as f64)
                    .sqrt();
            }
        }
        let fraction = if total > 0 {
            rejected as f64 / total as f64
        } else {
            1.
        };
        let mut valid = count.iter().all(|n| *n >= 16) && fraction < 0.001;
        if !valid {
            issues.push(format!(
                "Patch {}: insufficient samples or sensor clipping ({:.2}%)",
                patch.id,
                fraction * 100.
            ));
        }
        let green = (mean[1] * count[1] as f64 + mean[3] * count[3] as f64)
            / (count[1] + count[3]).max(1) as f64;
        let dominant = (0..4).max_by(|a, b| mean[*a].total_cmp(&mean[*b])).unwrap();
        let mean_error = dev[dominant] / (count[dominant].max(1) as f64).sqrt();
        if patch.display_rgb.iter().any(|v| *v > 0.)
            && (mean[dominant] < 4. || mean[dominant] < 10. * mean_error)
        {
            valid = false;
            issues.push(format!(
                "Patch {}: insufficient shadow signal; add a longer shutter exposure",
                patch.id
            ));
        }
        let lines: Vec<_> = rows
            .into_iter()
            .filter(|(_, (_, n))| *n >= 8)
            .map(|(y, (sum, n))| (y as f64, sum / n as f64, n))
            .collect();
        if row_banding(&lines, green, (dev[1] + dev[3]) * 0.5) {
            valid = false;
            issues.push(format!(
                "Patch {}: row banding/nonuniformity; use a shutter that averages display refresh",
                patch.id
            ));
        }
        patches.push(PatchMeasurement {
            id: patch.id,
            rgb: [
                mean[0] / m.exposure,
                green / m.exposure,
                mean[2] / m.exposure,
            ],
            cfa_mean: mean,
            stddev: dev,
            count,
            rejected_fraction: fraction,
            valid,
        });
    }
    Ok(Measurements {
        raw: m.clone(),
        patches,
        issues,
    })
}
/// Remove a smooth linear illumination gradient before checking coherent row variation.
/// This is an acquisition quality heuristic, not a calibrated display-uniformity metric.
pub(super) fn row_banding(rows: &[(f64, f64, u64)], signal: f64, pixel_noise: f64) -> bool {
    if rows.len() < 8 || signal < 32. {
        return false;
    }
    let n = rows.len() as f64;
    let x = rows.iter().map(|r| r.0).sum::<f64>() / n;
    let y = rows.iter().map(|r| r.1).sum::<f64>() / n;
    let xx = rows.iter().map(|r| (r.0 - x).powi(2)).sum::<f64>();
    let slope = rows.iter().map(|r| (r.0 - x) * (r.1 - y)).sum::<f64>() / xx.max(1e-9);
    let residual = (rows
        .iter()
        .map(|r| (r.1 - y - slope * (r.0 - x)).powi(2))
        .sum::<f64>()
        / n)
        .sqrt();
    let noise = pixel_noise / (rows.iter().map(|r| r.2).min().unwrap_or(1) as f64).sqrt();
    residual > signal.abs() * 0.025 && residual > noise * 6.
}
fn solve3(mut a: [[f64; 3]; 3], mut b: [f64; 3]) -> Result<[f64; 3]> {
    for i in 0..3 {
        let pivot = (i..3)
            .max_by(|x, y| a[*x][i].abs().total_cmp(&a[*y][i].abs()))
            .unwrap();
        a.swap(i, pivot);
        b.swap(i, pivot);
        ensure!(a[i][i].abs() > 1e-12, "Color matrix is singular");
        let d = a[i][i];
        for k in i..3 {
            a[i][k] /= d;
        }
        b[i] /= d;
        for j in 0..3 {
            if j == i {
                continue;
            }
            let f = a[j][i];
            for k in i..3 {
                a[j][k] -= f * a[i][k];
            }
            b[j] -= f * b[i];
        }
    }
    Ok(b)
}
pub fn apply(matrix: [[f64; 3]; 3], rgb: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| row.iter().zip(rgb).map(|(a, b)| a * b).sum())
}
pub fn color_transform(
    measurements: &Measurements,
    manifest: &PatternManifest,
) -> Result<ColorTransform> {
    let white = measurements
        .patches
        .iter()
        .find(|p| p.id == 11 && p.valid)
        .context("Reference white is missing/clipped")?
        .rgb;
    let scale = white.iter().copied().fold(0., f64::max);
    ensure!(scale > 0., "Reference white is not measurable");
    let mut normal = [[0.; 3]; 3];
    let mut rhs = [[0.; 3]; 3];
    let mut validation = Vec::new();
    let mut count = 0;
    for p in measurements.patches.iter().filter(|p| p.valid) {
        let Some(target) = manifest.patches.iter().find(|v| v.id == p.id) else {
            continue;
        };
        let x = p.rgb.map(|v| v / scale);
        if p.id % 5 == 0 {
            validation.push((x, target.rgb.map(f64::from)));
            continue;
        }
        count += 1;
        for i in 0..3 {
            for j in 0..3 {
                normal[i][j] += x[i] * x[j];
            }
            for c in 0..3 {
                rhs[c][i] += x[i] * target.rgb[c] as f64;
            }
        }
    }
    ensure!(
        count >= 12 && validation.len() >= 4,
        "Not enough valid reference colors"
    );
    let mut matrix = [[0.; 3]; 3];
    for c in 0..3 {
        matrix[c] = solve3(normal, rhs[c])?;
    }
    let error = (validation
        .iter()
        .map(|(x, target)| {
            let out = apply(matrix, *x);
            (0..3).map(|c| (out[c] - target[c]).powi(2)).sum::<f64>()
        })
        .sum::<f64>()
        / (validation.len() * 3) as f64)
        .sqrt();
    ensure!(
        error.is_finite() && error <= 0.03,
        "Reference transform holdout RMS {error:.4} exceeds 0.03; RAW reference remains available"
    );
    for row in &mut matrix {
        for value in row {
            *value /= scale;
        }
    }
    Ok(ColorTransform {
        matrix,
        validation_rms: error,
        reference_white: scale,
    })
}
pub fn reference(name: String, captures: &[StoredCapture]) -> Result<ReferenceSetVersion> {
    let first = captures.first().context("Select reference captures")?;
    ensure!(
        first.ticket.workflow == Workflow::Reference,
        "Not an iPhone reference capture"
    );
    let mut measurements = first
        .measurements
        .clone()
        .context("Reference requires geometry analysis")?;
    let mut dispersion = Vec::new();
    for capture in captures {
        capture.ensure_geometry_verified()?;
        ensure!(
            capture.ticket.workflow == Workflow::Reference,
            "Not a reference"
        );
        compatible(&first.ticket.conditions, &capture.ticket.conditions, true)?;
        ensure!(
            capture.measurements.is_some(),
            "Unanalyzed reference member"
        );
    }
    for patch in &mut measurements.patches {
        let mut samples = Vec::new();
        for capture in captures {
            if let Some(p) = capture
                .measurements
                .as_ref()
                .unwrap()
                .patches
                .iter()
                .find(|p| p.id == patch.id && p.valid)
            {
                samples.push(p.rgb);
            }
        }
        ensure!(
            !samples.is_empty(),
            "Patch {} has no valid exposure. Add a shorter/longer RAW with identical display conditions.",
            patch.id
        );
        patch.valid = true;
        patch.rgb = std::array::from_fn(|c| {
            samples.iter().map(|v| v[c]).sum::<f64>() / samples.len() as f64
        });
        dispersion.push(std::array::from_fn(|c| {
            (samples
                .iter()
                .map(|v| (v[c] - patch.rgb[c]).powi(2))
                .sum::<f64>()
                / samples.len() as f64)
                .sqrt()
        }));
    }
    measurements.issues.clear();
    let transform = color_transform(&measurements, &first.ticket.conditions.manifest).ok();
    Ok(ReferenceSetVersion {
        id: new_id("reference"),
        name,
        created_ms: now_ms(),
        members: captures.iter().map(|c| c.id.clone()).collect(),
        conditions: first.ticket.conditions.clone(),
        measurements,
        transform,
        dispersion,
    })
}
pub fn inspect(
    capture: &StoredCapture,
    reference: &ReferenceSetVersion,
    thresholds: Option<Thresholds>,
) -> Result<InspectionReport> {
    capture.ensure_geometry_verified()?;
    compatible(&reference.conditions, &capture.ticket.conditions, false)?;
    let test = capture
        .measurements
        .as_ref()
        .context("Test requires valid geometry/RAW analysis")?;
    let white = reference
        .measurements
        .patches
        .iter()
        .find(|p| p.id == 11 && p.valid)
        .context("Reference white unavailable")?
        .rgb;
    let scale = white.iter().copied().fold(0., f64::max);
    ensure!(scale > 0., "Invalid reference white");
    if let Some(t) = &thresholds {
        ensure!(
            t.rgb_fraction_of_white.is_finite()
                && t.rgb_fraction_of_white > 0.
                && t.white_relative.is_finite()
                && t.white_relative > 0.,
            "Invalid report thresholds"
        );
    }
    let mut differences = Vec::new();
    let mut issues = test.issues.clone();
    for p in &test.patches {
        let Some(r) = reference
            .measurements
            .patches
            .iter()
            .find(|r| r.id == p.id && r.valid)
        else {
            issues.push(format!("Patch {}: reference unavailable", p.id));
            continue;
        };
        if !p.valid {
            continue;
        }
        let delta = std::array::from_fn(|c| p.rgb[c] - r.rgb[c]);
        let error = delta.iter().map(|v| v.abs() / scale).fold(0., f64::max);
        differences.push(PatchDifference {
            id: p.id,
            rect: capture
                .ticket
                .conditions
                .manifest
                .patches
                .iter()
                .find(|v| v.id == p.id)
                .context("Missing patch location")?
                .rect,
            delta,
            max_fraction_of_white: error,
            flagged: thresholds
                .as_ref()
                .is_some_and(|t| error > t.rgb_fraction_of_white),
        });
    }
    let white_relative = test
        .patches
        .iter()
        .find(|p| p.id == 11 && p.valid)
        .map(|p| p.rgb.iter().sum::<f64>() / white.iter().sum::<f64>() - 1.);
    if thresholds
        .as_ref()
        .zip(white_relative)
        .is_some_and(|(t, v)| v.abs() > t.white_relative)
    {
        issues.push("Reference-relative white response exceeds threshold".into());
    }
    let conclusion =
        if !issues.is_empty() || differences.len() != reference.measurements.patches.len() {
            "Needs attention"
        } else if thresholds.is_none() {
            "Measured — no pass/fail thresholds configured"
        } else if differences.iter().any(|p| p.flagged) {
            "Outside configured thresholds"
        } else {
            "Within configured thresholds"
        };
    Ok(InspectionReport {
        id: new_id("report"),
        capture: capture.id.clone(),
        reference: reference.id.clone(),
        created_ms: now_ms(),
        source_captures: vec![capture.id.clone()],
        test_conditions: capture.ticket.conditions.clone(),
        reference_conditions: reference.conditions.clone(),
        reference_members: reference.members.clone(),
        geometry: capture.ticket.geometry.clone(),
        thresholds,
        differences,
        white_relative,
        issues,
        conclusion: conclusion.into(),
    })
}

/// Export is downstream of CFA measurements. Its pixels are never fed back into statistics.
pub fn export(
    raw: &RawFrame,
    profile: &GeometryProfile,
    manifest: &PatternManifest,
    matrix: Option<[[f64; 3]; 3]>,
    folder: &std::path::Path,
) -> Result<()> {
    export_exposures(&[(raw, profile)], manifest, matrix, folder)
}

/// Use the longest unclipped exposure at each output sample, in fixed DN/second units.
/// No per-image gain or white balance is estimated. Patch statistics remain a separate CFA path.
pub fn export_exposures(
    sources: &[(&RawFrame, &GeometryProfile)],
    manifest: &PatternManifest,
    matrix: Option<[[f64; 3]; 3]>,
    folder: &std::path::Path,
) -> Result<()> {
    let (first, _) = sources
        .first()
        .context("No RAW exposure for normalized output")?;
    ensure!(
        sources
            .iter()
            .all(|(raw, _)| raw.metadata.camera_key() == first.metadata.camera_key()),
        "Exposure camera conditions differ"
    );
    let [w, h] = manifest.size;
    let mut pixels = vec![[0f32; 3]; (w * h) as usize];
    let mut mask = image::GrayImage::new(w, h);
    let mut preview = image::RgbImage::new(w, h);
    let [left, bottom, right, top] = manifest.insets;
    for y in bottom..h - top {
        for x in left..w - right {
            let uv = [x as f64 / (w - 1) as f64, y as f64 / (h - 1) as f64];
            let Some((rgb, clipped)) = fused_sample(sources, uv) else {
                continue;
            };
            let calibrated = matrix.map(|t| apply(t, rgb)).unwrap_or(rgb);
            let index = ((h - 1 - y) * w + x) as usize;
            pixels[index] = calibrated.map(|v| v as f32);
            mask.put_pixel(x, h - 1 - y, image::Luma([if clipped { 128 } else { 255 }]));
            let display = if matrix.is_some() {
                calibrated
            } else {
                // A single explicit scale for the SDR proxy, including bracket output.
                let m = &first.metadata;
                rgb.map(|v| v * m.exposure / (m.white - m.black.iter().sum::<f64>() / 4.))
            };
            preview.put_pixel(
                x,
                h - 1 - y,
                image::Rgb(display.map(|v| {
                    let v = v.clamp(0., 1.);
                    ((if v < 0.0031308 {
                        12.92 * v
                    } else {
                        1.055 * v.powf(1. / 2.4) - 0.055
                    }) * 255.)
                        .round() as u8
                })),
            );
        }
    }
    exr::prelude::write_rgb_file(
        folder.join("normalized.exr"),
        w as usize,
        h as usize,
        |x, y| {
            let p = pixels[y * w as usize + x];
            (p[0], p[1], p[2])
        },
    )?;
    preview.save(folder.join("preview.png"))?;
    mask.save(folder.join("valid-mask.png"))?;
    std::fs::write(
        folder.join("encoding.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"encoding":if matrix.is_some(){"extended-linear-sRGB"}else{"camera-linear-DN-per-second"},"preview":"SDR proxy only; never measurement input","sources":sources.iter().map(|(raw,profile)| serde_json::json!({"raw":raw.metadata,"geometry":profile})).collect::<Vec<_>>(),"manifest":manifest,"fusion":"longest unclipped exposure per sample, fixed DN/second","sampling":"Output pixel density does not imply resolved screen-pixel detail"}),
        )?,
    )?;
    Ok(())
}

pub(super) fn fused_sample(
    sources: &[(&RawFrame, &GeometryProfile)],
    uv: [f64; 2],
) -> Option<([f64; 3], bool)> {
    let mut best: Option<([f64; 3], bool, f64)> = None;
    for (raw, profile) in sources {
        let m = &raw.metadata;
        let Some(p) = profile.mapping.map(uv, profile.raw_size) else {
            continue;
        };
        let (sx, sy) = (p[0], (m.height - 1) as f64 - p[1]);
        if sx < 2. || sy < 2. || sx >= (m.width - 3) as f64 || sy >= (m.height - 3) as f64 {
            continue;
        }
        let mut sums = [0.; 3];
        let mut weights = [0.; 3];
        let mut clipped = false;
        for yy in sy.floor() as u32 - 1..=sy.floor() as u32 + 2 {
            for xx in sx.floor() as u32 - 1..=sx.floor() as u32 + 2 {
                let (c, v, saturated) = raw.sample(xx, yy);
                let c = if c == 3 { 1 } else { c };
                let weight =
                    (2. - (xx as f64 - sx).abs()).max(0.) * (2. - (yy as f64 - sy).abs()).max(0.);
                clipped |= saturated && weight > 0.;
                sums[c] += v * weight;
                weights[c] += weight;
            }
        }
        let rgb = std::array::from_fn(|c| sums[c] / weights[c] / m.exposure);
        if best.as_ref().is_none_or(|(_, old_clipped, exposure)| {
            (*old_clipped && !clipped)
                || (*old_clipped == clipped
                    && if clipped {
                        m.exposure < *exposure
                    } else {
                        m.exposure > *exposure
                    })
        }) {
            best = Some((rgb, clipped, m.exposure));
        }
    }
    best.map(|(rgb, clipped, _)| (rgb, clipped))
}

/// Fuse measured patch responses, never tone-mapped images. Sources remain immutable.
pub fn inspect_exposures(
    captures: &[StoredCapture],
    reference: &ReferenceSetVersion,
    thresholds: Option<Thresholds>,
) -> Result<InspectionReport> {
    let first = captures.first().context("Select test exposures")?;
    let mut merged = first.clone();
    let measured = merged
        .measurements
        .as_mut()
        .context("All exposures need geometric analysis")?;
    for source in captures {
        source.ensure_geometry_verified()?;
        ensure!(
            source.ticket.workflow == Workflow::Inspection && source.analysis_issue.is_none(),
            "Select analyzed inspection captures only"
        );
        ensure!(
            source.ticket.reference.as_deref() == Some(reference.id.as_str()),
            "Exposure captures selected different baseline versions"
        );
        compatible(&first.ticket.conditions, &source.ticket.conditions, true)?;
        ensure!(source.measurements.is_some(), "Exposure lacks measurements");
    }
    measured.issues.clear();
    for patch in &mut measured.patches {
        let sources: Vec<_> = captures
            .iter()
            .filter_map(|c| {
                c.measurements
                    .as_ref()?
                    .patches
                    .iter()
                    .find(|p| p.id == patch.id && p.valid)
            })
            .collect();
        patch.valid = !sources.is_empty();
        if sources.is_empty() {
            measured
                .issues
                .push(format!("Patch {} has no valid exposure", patch.id));
            continue;
        }
        patch.rgb = std::array::from_fn(|i| {
            sources.iter().map(|p| p.rgb[i]).sum::<f64>() / sources.len() as f64
        });
    }
    let mut report = inspect(&merged, reference, thresholds)?;
    report.source_captures = captures.iter().map(|c| c.id.clone()).collect();
    Ok(report)
}
