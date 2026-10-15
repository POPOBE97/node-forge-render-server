//! Count luminous RGB subpixels crossed by a segment, independently in the image and grid.
use super::{grid::subpixel_distance, io::DecodedImage, model::*};
use anyhow::{Result, ensure};
use std::collections::HashSet;

/// Grid candidates only enumerate ideal subpixels; they never seed image detection.
fn candidates(m: Measurement) -> Result<Vec<GridSite>> {
    let count = (length(sub(m.b, m.a)) * 2.).ceil().max(1.) as usize;
    ensure!(count <= 20_000, "Measurement too long; shorten the segment");
    let mut cells = HashSet::new();
    let mut sites = vec![];
    for step in 0..=count {
        let p = world_to_cell(add(m.a, mul(sub(m.b, m.a), step as f64 / count as f64)));
        for i in p[0].floor() as i32 - 1..=p[0].floor() as i32 + 1 {
            for j in p[1].floor() as i32 - 1..=p[1].floor() as i32 + 1 {
                if cells.insert((i, j)) {
                    sites.extend(Subpixel::ALL.map(|kind| GridSite { cell: [i, j], kind }));
                }
            }
        }
    }
    Ok(sites)
}
fn parameter(m: Measurement, p: Point) -> f64 {
    let d = sub(m.b, m.a);
    let q = sub(p, m.a);
    ((d[0] * q[0] + d[1] * q[1]) / (d[0] * d[0] + d[1] * d[1]).max(1e-20)).clamp(0., 1.)
}
/// Intersect the filled RGB footprints used by the grid overlay, not grid lines/cell edges.
/// Each convex subpixel contributes at most one hit, even at A/B or near a grid vertex.
pub fn grid_pixels(m: Measurement) -> Result<Vec<(Point, Subpixel)>> {
    if length(sub(m.b, m.a)) < 1e-12 {
        return Ok(vec![]);
    }
    let mut hits = vec![];
    for site in candidates(m)? {
        let distance = |t| {
            subpixel_distance(
                sub(add(m.a, mul(sub(m.b, m.a), t)), site.world()),
                site.kind,
            )
        };
        // Signed footprint distance is convex along a line. A bounded minimization handles
        // diamonds and either green capsule orientation, without zoom-dependent sampling.
        let (mut lo, mut hi) = (0., 1.);
        for _ in 0..48 {
            let a = lo + (hi - lo) / 3.;
            let b = hi - (hi - lo) / 3.;
            if distance(a) <= distance(b) {
                hi = b;
            } else {
                lo = a;
            }
        }
        if distance((lo + hi) * 0.5)
            .min(distance(0.))
            .min(distance(1.))
            < -1e-9
        {
            hits.push((site.world(), site.kind));
        }
    }
    hits.sort_by(|a, b| parameter(m, a.0).total_cmp(&parameter(m, b.0)));
    Ok(hits)
}

/// Sample the visible measurement line at <= 0.5 original-image px, including curvature.
/// This has no lattice/color/spacing assumptions and works before grid alignment.
fn path_samples(m: Measurement, layer: &ImageLayer) -> Result<Vec<(f64, Point)>> {
    let point = |t| {
        layer
            .sample_position(add(m.a, mul(sub(m.b, m.a), t)))
            .ok_or_else(|| anyhow::anyhow!("Measurement crosses an invalid image mapping"))
    };
    let mut samples = vec![(0., point(0.)?)];
    let mut stack = vec![];
    for i in (0..64).rev() {
        let a = i as f64 / 64.;
        let b = (i + 1) as f64 / 64.;
        stack.push((a, point(a)?, b, point(b)?, 0));
    }
    while let Some((a, pa, b, pb, depth)) = stack.pop() {
        let t = (a + b) * 0.5;
        let pm = point(t)?;
        if length(sub(pb, pa)) > 0.5 || length(sub(pm, mul(add(pa, pb), 0.5))) > 0.1 {
            ensure!(
                depth < 24 && samples.len() + stack.len() < 250_000,
                "Image scan too long or too distorted; shorten the segment"
            );
            stack.push((t, pm, b, pb, depth + 1));
            stack.push((a, pa, t, pm, depth + 1));
        } else {
            samples.push((b, pb));
        }
    }
    Ok(samples)
}
#[derive(Clone, Debug)]
pub struct RgbMeasure {
    /// Ideal grid R, B, G1, G2 counts.
    pub expected: [usize; 4],
    /// Independently detected image R, G, B counts. No green orientation is inferred from grid.
    pub detected: [usize; 3],
    pub centers: Vec<(Point, Subpixel)>,
    pub grid_centers: Vec<(Point, Subpixel)>,
    pub outside: bool,
}
pub fn count_rgb(m: Measurement, layer: &ImageLayer, image: &DecodedImage) -> Result<RgbMeasure> {
    let grid_centers = grid_pixels(m)?;
    let mut out = RgbMeasure {
        expected: [0; 4],
        detected: [0; 3],
        centers: vec![],
        grid_centers,
        outside: false,
    };
    for (_, kind) in &out.grid_centers {
        out.expected[*kind as usize] += 1;
    }
    if length(sub(m.b, m.a)) < 1e-12 {
        return Ok(out);
    }
    let samples = path_samples(m, layer)?;
    let mut distance = vec![0.; samples.len()];
    let mut signals = Vec::with_capacity(samples.len());
    for (i, (_, p)) in samples.iter().enumerate() {
        if i > 0 {
            distance[i] = distance[i - 1] + length(sub(*p, samples[i - 1].1));
        }
        out.outside |=
            p[0] < 0. || p[1] < 0. || p[0] >= image.size[0] as f64 || p[1] >= image.size[1] as f64;
        let rgba = image.sample(*p, false);
        signals.push(std::array::from_fn::<_, 3, _>(|c| {
            (rgba[c] - 1.2 * rgba[(c + 1) % 3].max(rgba[(c + 2) % 3])).max(0.) as f64
        }));
    }
    for channel in 0..3 {
        // Mild half-pixel smoothing plus hysteresis suppresses camera noise. Peaks are
        // separated only by measured dark gaps, never by a guessed grid pitch/count.
        let profile: Vec<f64> = (0..signals.len())
            .map(|i| {
                signals[i.saturating_sub(1)][channel] * 0.25
                    + signals[i][channel] * 0.5
                    + signals[(i + 1).min(signals.len() - 1)][channel] * 0.25
            })
            .collect();
        let peak = profile.iter().copied().fold(0., f64::max);
        if peak < 0.008 {
            continue;
        }
        let high = (peak * m.threshold).max(0.004);
        let low = high * 0.5;
        let mut i = 0;
        while i < profile.len() {
            if profile[i] <= low {
                i += 1;
                continue;
            }
            let start = i;
            let mut end = i;
            let mut best = profile[i];
            let (mut weight, mut weighted_t) = (0., 0.);
            while i < profile.len() {
                if profile[i] > low {
                    end = i;
                    best = best.max(profile[i]);
                    let w = profile[i];
                    weight += w;
                    weighted_t += samples[i].0 * w;
                } else if distance[i] - distance[end] > 0.75 {
                    break;
                }
                i += 1;
            }
            if best >= high
                && (distance[end] - distance[start] >= 1. || start == 0 || end + 1 == profile.len())
            {
                let t = weighted_t / weight;
                out.detected[channel] += 1;
                let kind = [Subpixel::Red, Subpixel::Green1, Subpixel::Blue][channel];
                out.centers.push((add(m.a, mul(sub(m.b, m.a), t)), kind));
            }
        }
    }
    out.centers
        .sort_by(|a, b| parameter(m, a.0).total_cmp(&parameter(m, b.0)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_perfecto::{calibration::Calibration, io::Pixels};

    fn layer(size: [u32; 2]) -> ImageLayer {
        ImageLayer {
            id: 1,
            asset: "test".into(),
            name: "test".into(),
            size,
            bit_depth: 8,
            visible: true,
            locked: false,
            opacity: 1.,
            offset: [0, 0],
            calibration: Calibration::initial(size),
            landmarks: vec![],
        }
    }
    fn synthetic(layer: &ImageLayer, dark: Option<Point>) -> DecodedImage {
        let [w, h] = layer.size;
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        for j in -4..=4 {
            for i in -4..=4 {
                for kind in Subpixel::ALL {
                    let site = GridSite { cell: [i, j], kind };
                    if dark == Some(site.world()) {
                        continue;
                    }
                    let p = layer.sample_position(site.world()).unwrap();
                    for y in -4..=4 {
                        for x in -4..=4 {
                            if x * x + y * y > 16 {
                                continue;
                            }
                            let px = p[0].round() as i32 + x;
                            let py = p[1].round() as i32 + y;
                            if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                                continue;
                            }
                            let index = (((h as i32 - 1 - py) * w as i32 + px) * 4) as usize;
                            let channel = match kind {
                                Subpixel::Red => 0,
                                Subpixel::Blue => 2,
                                _ => 1,
                            };
                            pixels[index + channel] = 255;
                            pixels[index + 3] = 255;
                        }
                    }
                }
            }
        }
        DecodedImage {
            size: layer.size,
            bit_depth: 8,
            pixels: Pixels::Srgb8(pixels),
        }
    }
    #[test]
    fn rgb_counts_have_inclusive_endpoints_color_identity_and_dark_pixel_gaps() {
        let layer = layer([512, 512]);
        let image = synthetic(&layer, Some([0., 0.]));
        let m = Measurement::new([-2., 0.], [2., 0.]);
        let r = count_rgb(m, &layer, &image).unwrap();
        assert_eq!(r.expected, [3, 2, 0, 0]);
        assert_eq!(r.detected, [2, 0, 2]);
        assert_eq!(r.centers.len(), 4);
        let reversed = count_rgb(Measurement::new(m.b, m.a), &layer, &image).unwrap();
        assert_eq!(reversed.detected, r.detected);
        let diagonal = count_rgb(Measurement::new([-2., -2.], [2., 2.]), &layer, &image).unwrap();
        assert_eq!(diagonal.expected, [5, 0, 4, 0]);
        assert_eq!(diagonal.detected, [4, 4, 0]);
        let other = count_rgb(Measurement::new([2., -2.], [-2., 2.]), &layer, &image).unwrap();
        assert_eq!(other.detected, [4, 4, 0]);
        let outside = count_rgb(Measurement::new([20., 20.], [22., 22.]), &layer, &image).unwrap();
        assert!(outside.outside);
        assert_eq!(outside.detected, [0; 3]);
    }
    #[test]
    fn image_and_grid_counts_are_independent_before_alignment() {
        let display = layer([512, 512]);
        let mut photograph = display.clone();
        // The real photograph has a finer pitch than the current grid mapping.
        photograph.calibration.h[0] *= 0.6;
        photograph.calibration.h[4] *= 0.6;
        let image = synthetic(&photograph, None);
        let segment = Measurement::new([-2., 0.], [2., 0.]);
        let result = count_rgb(segment, &display, &image).unwrap();
        assert_eq!(result.expected, [3, 2, 0, 0]);
        assert_eq!(result.detected, [3, 0, 4]);
        assert_eq!(
            result.centers.len(),
            7,
            "Image count must not be limited by 5 grid predictions"
        );
        assert_eq!(result.grid_centers.len(), 5);
        let backwards =
            count_rgb(Measurement::new(segment.b, segment.a), &display, &image).unwrap();
        assert_eq!(backwards.detected, result.detected);
        // Moving the image by an integer cell keeps the same counts at the translated line.
        let mut moved = display.clone();
        moved.offset = [-2, 3];
        let offset = moved.offset_world();
        let translated = Measurement::new(add(segment.a, offset), add(segment.b, offset));
        let after = count_rgb(translated, &moved, &image).unwrap();
        assert_eq!(after.detected, result.detected);
        assert_eq!(after.expected, result.expected);
    }
    #[test]
    fn grid_counts_rgb_footprints_instead_of_mesh_intersections() {
        let segment = Measurement::new([0., 0.], [4., 0.]);
        let hits = grid_pixels(segment).unwrap();
        assert_eq!(hits.len(), 5); // Three red + two blue, not two traversed diamond cells.
        assert_eq!(
            grid_pixels(Measurement::new([-4., 0.], [0., 0.]))
                .unwrap()
                .len(),
            5
        );
        // This line crosses diamond mesh edges, but misses all RGB footprints.
        assert_eq!(
            grid_pixels(Measurement::new([0., 0.34], [4., 0.34]))
                .unwrap()
                .len(),
            0
        );
        assert!(
            grid_pixels(Measurement::new([0., 0.], [0., 0.]))
                .unwrap()
                .is_empty()
        );
        assert!(grid_pixels(Measurement::new([0., 0.], [1e9, 1e9])).is_err());
    }
    #[test]
    fn line_scan_uses_accumulated_rotation_perspective_and_lens_mapping() {
        let reference = layer([512, 512]);
        let mut corrected = reference.clone();
        corrected.calibration.lens = [0.2, -0.02, 0.01, -0.01];
        corrected.calibration.h[6] = 0.004;
        corrected.rotate_image(33., [0., 0.]).unwrap();
        let image = synthetic(&corrected, None);
        let result = count_rgb(Measurement::new([-2., 0.], [2., 0.]), &corrected, &image).unwrap();
        assert_eq!(result.detected, [3, 0, 2]);
        assert_eq!(result.expected, [3, 2, 0, 0]);
        let samples = path_samples(Measurement::new([-2., 0.], [2., 0.]), &corrected).unwrap();
        assert!(
            samples
                .windows(2)
                .all(|w| length(sub(w[1].1, w[0].1)) <= 0.500001)
        );
    }
    #[test]
    #[ignore = "Set PIXEL_PERFECTO_PHOTO to ocean-weather-icon.png for a real-image line scan"]
    fn supplied_photo_line_count() {
        let path = std::env::var("PIXEL_PERFECTO_PHOTO").unwrap();
        let (name, hash, asset) =
            crate::pixel_perfecto::io::import(std::path::Path::new(&path)).unwrap();
        let mut layer = layer(asset.image.size);
        layer.name = name;
        layer.asset = hash.clone();
        // Same two visible red centers as the collinear-anchor regression: 15 red pitches.
        let a = [1492.3386, 1928.6177];
        let b = [1515.6402, 3247.3390];
        let step = mul(sub(b, a), 1. / 30.);
        let norm = layer.size[0].max(layer.size[1]) as f64 * 0.5;
        let center = layer.calibration.center;
        layer.calibration.h = [
            step[1] / norm,
            step[0] / norm,
            (a[0] - center[0]) / norm,
            -step[0] / norm,
            step[1] / norm,
            (a[1] - center[1]) / norm,
            0.,
            0.,
            1.,
        ];
        let m = Measurement::new([0., 0.], [0., 30.]);
        let result = count_rgb(m, &layer, &asset.image).unwrap();
        eprintln!(
            "Ocean line: image {:?} = {}; grid {:?} = {}",
            result.detected,
            result.centers.len(),
            result.expected,
            result.grid_centers.len()
        );
        assert_eq!(result.grid_centers.len(), 31);
        assert!(
            result.detected[0] >= 10,
            "Expected a resolved sequence of red subpixels"
        );
        if let Ok(output) = std::env::var("PIXEL_PERFECTO_MEASURE_FIXTURE") {
            let mut doc = PixelPerfectoDocument::default();
            doc.layers.push(layer);
            doc.repair_selection();
            doc.measurement = Some(m);
            doc.viewport = Viewport {
                center: [0., 15.],
                zoom: 16.,
            };
            doc.grid.outlines = true;
            doc.grid.labels = false;
            let mut assets = crate::pixel_perfecto::io::Assets::new();
            assets.insert(hash, asset);
            crate::pixel_perfecto::io::save(std::path::Path::new(&output), &doc, &assets).unwrap();
        }
    }
}
