//! Image measurements for assisted calibration. All exported points are source pixels, Y-up.
//! Automatic correspondences are disposable measurements; only the fit and user anchors persist.
use std::collections::HashSet;

use anyhow::{Result, ensure};

use super::{
    calibration::{self, Calibration},
    io::DecodedImage,
    model::*,
};

#[derive(Clone, Debug)]
pub struct Center {
    pub position: Point,
    pub radius: f64,
}
#[derive(Clone, Debug)]
pub struct Assistance {
    pub calibration: Calibration,
    pub anchors: Vec<Landmark>,
    pub model: FitModel,
    pub automatic: Vec<Landmark>,
    pub held_out: Vec<Landmark>,
    pub validation_rms: Option<f64>,
    pub fitted_rms: f64,
    pub used_lens: bool,
    pub note: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitModel {
    Unchanged,
    Translation,
    Similarity,
    Affine,
    Perspective,
    Lens,
}
impl FitModel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "Current alignment",
            Self::Translation => "Translation",
            Self::Similarity => "Translation + rotation + uniform scale",
            Self::Affine => "Affine",
            Self::Perspective => "Perspective",
            Self::Lens => "Perspective + lens",
        }
    }
}

fn channel(kind: Subpixel) -> usize {
    match kind {
        Subpixel::Red => 0,
        Subpixel::Blue => 2,
        _ => 1,
    }
}
fn signal(p: [f32; 4], c: usize) -> f64 {
    (p[c] - 1.4 * p[(c + 1) % 3].max(p[(c + 2) % 3])).max(0.) as f64
}

/// Connected components, with intensity centroids instead of the brightest camera sample.
/// Raster coordinates remain private to this measurement boundary.
fn components(values: &[f64], width: usize, height: usize, threshold: f64) -> Vec<Center> {
    let mut visited = vec![false; values.len()];
    let mut centers = Vec::new();
    let mut queue = Vec::new();
    for start in 0..values.len() {
        if visited[start] || values[start] <= threshold {
            continue;
        }
        queue.clear();
        queue.push(start);
        visited[start] = true;
        let (mut total, mut x_sum, mut y_sum) = (0., 0., 0.);
        let mut boundary = false;
        let mut head = 0;
        while head < queue.len() {
            let i = queue[head];
            head += 1;
            let (x, y) = (i % width, i / width);
            boundary |= x == 0 || y == 0 || x + 1 == width || y + 1 == height;
            let weight = values[i];
            total += weight;
            x_sum += (x as f64 + 0.5) * weight;
            y_sum += (y as f64 + 0.5) * weight;
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx < width && ny < height {
                    let j = ny * width + nx;
                    if !visited[j] && values[j] > threshold {
                        visited[j] = true;
                        queue.push(j);
                    }
                }
            }
        }
        if !boundary && queue.len() >= 3 && total > threshold * 4. {
            centers.push(Center {
                position: [x_sum / total, y_sum / total],
                radius: (queue.len() as f64 / std::f64::consts::PI).sqrt(),
            });
        }
    }
    centers
}

/// A strict center-distance gate prevents local searches from hopping to the next repeat.
/// The larger measurement window avoids clipping the chosen blob at that gate.
pub fn snap_center(
    image: &DecodedImage,
    near: Point,
    kind: Subpixel,
    radius: f64,
) -> Option<Center> {
    if !near.iter().all(|v| v.is_finite()) || !radius.is_finite() || radius < 2. {
        return None;
    }
    let radius = radius.min(128.);
    let extent = (radius * 2.).ceil() as i32;
    let x0 = near[0].floor() as i32 - extent;
    let y0 = (image.size[1] as f64 - near[1]).floor() as i32 - extent;
    let width = (extent * 2 + 1) as usize;
    let mut values = Vec::with_capacity(width * width);
    let mut peak: f64 = 0.;
    for y in 0..width {
        for x in 0..width {
            let v = signal(image.pixel(x0 + x as i32, y0 + y as i32), channel(kind));
            peak = peak.max(v);
            values.push(v);
        }
    }
    if peak < 0.015 {
        return None;
    }
    components(&values, width, width, (peak * 0.18).max(0.004))
        .into_iter()
        .map(|mut c| {
            c.position = [
                x0 as f64 + c.position[0],
                image.size[1] as f64 - y0 as f64 - c.position[1],
            ];
            c
        })
        .filter(|c| {
            c.radius < radius * 0.9
                && length(sub(c.position, near)) <= radius
                && c.position[0] - c.radius > 0.
                && c.position[1] - c.radius > 0.
                && c.position[0] + c.radius < image.size[0] as f64
                && c.position[1] + c.radius < image.size[1] as f64
        })
        .min_by(|a, b| length(sub(a.position, near)).total_cmp(&length(sub(b.position, near))))
}

pub fn search_radius(c: &Calibration, size: [u32; 2], site: GridSite) -> f64 {
    let q = site.world();
    let Some(p) = c.map(q, size) else {
        return 0.;
    };
    [[1., 1.], [-1., 1.], [1., -1.], [-1., -1.]]
        .into_iter()
        .filter_map(|d| c.map(add(q, d), size).map(|v| length(sub(v, p))))
        .fold(128. / 0.28, f64::min)
        * 0.28
}

fn red_centers(image: &DecodedImage) -> Result<Vec<Center>> {
    let step = image.size[0].max(image.size[1]).div_ceil(1800).max(1) as usize;
    let w = image.size[0] as usize / step;
    let h = image.size[1] as usize / step;
    let mut values = Vec::with_capacity(w * h);
    let mut peak: f64 = 0.;
    for y in 0..h {
        for x in 0..w {
            let v = signal(
                image.pixel((x * step + step / 2) as i32, (y * step + step / 2) as i32),
                0,
            );
            peak = peak.max(v);
            values.push(v);
        }
    }
    ensure!(
        peak > 0.025,
        "No clear red subpixels found; use Fine alignment and drag grid points manually"
    );
    let result: Vec<_> = components(&values, w, h, (peak * 0.16).max(0.008))
        .into_iter()
        .filter(|c| c.radius >= 0.9 && c.radius < w.min(h) as f64 * 0.04)
        .map(|c| Center {
            position: [
                c.position[0] * step as f64,
                image.size[1] as f64 - c.position[1] * step as f64,
            ],
            radius: c.radius * step as f64,
        })
        .collect();
    ensure!(
        result.len() >= 20 && result.len() <= 20_000,
        "Need a clear repeating red lattice; found {} candidate centers",
        result.len()
    );
    Ok(result)
}

fn lattice_seed(image: &DecodedImage) -> Result<Calibration> {
    let centers = red_centers(image)?;
    let stride = centers.len().div_ceil(600);
    let mut nearest = Vec::new();
    for (i, c) in centers.iter().enumerate().step_by(stride) {
        let distance = centers
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, d)| length(sub(c.position, d.position)))
            .fold(f64::INFINITY, f64::min);
        nearest.push(distance);
    }
    nearest.sort_by(f64::total_cmp);
    let pitch = nearest[nearest.len() / 2];
    ensure!(
        pitch >= 8.,
        "Subpixels are too small for reliable automatic alignment; zoom the microscope or align manually"
    );
    let mut vectors = Vec::new();
    let mut histogram = [0usize; 180];
    for c in centers.iter().step_by(stride) {
        for d in &centers {
            let mut v = sub(d.position, c.position);
            let len = length(v);
            if !(pitch * 0.78..pitch * 1.27).contains(&len) {
                continue;
            }
            if v[1] < 0. {
                v = mul(v, -1.);
            }
            let angle = v[1].atan2(v[0]).to_degrees().rem_euclid(180.);
            let bin = angle.floor() as usize;
            histogram[bin.min(179)] += 1;
            vectors.push((angle, v));
        }
    }
    let strength = |i: usize| {
        (0..9)
            .map(|k| histogram[(i + 176 + k) % 180])
            .sum::<usize>()
    };
    let first = (0..180).max_by_key(|i| strength(*i)).unwrap_or(45);
    let second = (0..180)
        .filter(|i| {
            let d = (*i as f64 - first as f64).abs();
            (50.0..130.0).contains(&d)
        })
        .max_by_key(|i| strength(*i))
        .unwrap_or((first + 90) % 180);
    ensure!(
        strength(first) > 20 && strength(second) > 20,
        "No two reliable lattice directions; adjust the grid manually"
    );
    let mean_vector = |angle: usize| {
        let direction = [
            (angle as f64).to_radians().cos(),
            (angle as f64).to_radians().sin(),
        ];
        let (sum, n) = vectors
            .iter()
            .filter(|(a, _)| {
                let d = (*a - angle as f64).abs();
                d.min(180. - d) < 7.
            })
            .fold(([0., 0.], 0.), |(sum, n), (_, v)| {
                let v = if v[0] * direction[0] + v[1] * direction[1] < 0. {
                    mul(*v, -1.)
                } else {
                    *v
                };
                (add(sum, v), n + 1.)
            });
        mul(sum, 1. / n)
    };
    let mut u = mean_vector(first);
    let mut v = mean_vector(second);
    if u[0] * v[1] - u[1] * v[0] < 0. {
        std::mem::swap(&mut u, &mut v);
    }
    let middle = mul([image.size[0] as f64, image.size[1] as f64], 0.5);
    let origin = centers
        .iter()
        .min_by(|a, b| length(sub(a.position, middle)).total_cmp(&length(sub(b.position, middle))))
        .unwrap()
        .position;
    let s = Calibration::scale(image.size);
    let c = Calibration {
        h: [
            (u[0] - v[0]) * 0.5 / s,
            (u[0] + v[0]) * 0.5 / s,
            (origin[0] - middle[0]) / s,
            (u[1] - v[1]) * 0.5 / s,
            (u[1] + v[1]) * 0.5 / s,
            (origin[1] - middle[1]) / s,
            0.,
            0.,
            1.,
        ],
        lens: [0.; 4],
        center: middle,
    };
    c.validate(image.size)?;
    // Reject random colored blobs even if they happen to yield two histogram peaks.
    let matched = centers
        .iter()
        .filter(|p| {
            c.unmap(p.position, image.size).is_some_and(|q| {
                let site = GridSite::nearest(q, Subpixel::Red);
                c.map(site.world(), image.size)
                    .is_some_and(|v| length(sub(v, p.position)) < pitch * 0.18)
            })
        })
        .count();
    ensure!(
        matched * 3 >= centers.len(),
        "The image does not contain a reliable diamond lattice; align a few points manually"
    );
    Ok(c)
}

/// Bounded lattice enumeration, also shared by the projected-grid overlay.
pub fn sites_in_bounds(
    c: &Calibration,
    size: [u32; 2],
    bounds: [f64; 4],
    max_cells: usize,
) -> Vec<GridSite> {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for y in 0..=4 {
        for x in 0..=4 {
            let p = [
                bounds[0] + (bounds[2] - bounds[0]) * x as f64 / 4.,
                bounds[1] + (bounds[3] - bounds[1]) * y as f64 / 4.,
            ];
            if let Some(q) = c.unmap(p, size) {
                let q = world_to_cell(q);
                for a in 0..2 {
                    lo[a] = lo[a].min(q[a]);
                    hi[a] = hi[a].max(q[a]);
                }
            }
        }
    }
    if !lo
        .iter()
        .chain(&hi)
        .all(|v| v.is_finite() && v.abs() < 1_000_000.)
    {
        return vec![];
    }
    let a = lo.map(|v| v.floor() as i32 - 1);
    let b = hi.map(|v| v.ceil() as i32 + 1);
    let count = (b[0] - a[0] + 1) as f64 * (b[1] - a[1] + 1) as f64;
    let step = (count / max_cells.max(1) as f64).sqrt().ceil().max(1.) as usize;
    let mut out = Vec::new();
    for i in (a[0]..=b[0]).step_by(step) {
        for j in (a[1]..=b[1]).step_by(step) {
            for kind in Subpixel::ALL {
                out.push(GridSite { cell: [i, j], kind });
            }
        }
    }
    out
}

pub fn coverage(points: &[Landmark], size: [u32; 2]) -> [usize; 9] {
    let mut bins = [0; 9];
    for p in points.iter().filter(|p| p.enabled) {
        if p.source[0] < 0.
            || p.source[1] < 0.
            || p.source[0] >= size[0] as f64
            || p.source[1] >= size[1] as f64
        {
            continue;
        }
        let x = (p.source[0] / size[0] as f64 * 3.) as usize;
        let y = (p.source[1] / size[1] as f64 * 3.) as usize;
        bins[y * 3 + x] += 1;
    }
    bins
}
pub const REGIONS: [&str; 9] = [
    "bottom left",
    "bottom edge",
    "bottom right",
    "left edge",
    "center",
    "right edge",
    "top left",
    "top edge",
    "top right",
];
pub fn coverage_hint(points: &[Landmark], size: [u32; 2]) -> String {
    let bins = coverage(points, size);
    let missing: Vec<_> = REGIONS
        .iter()
        .enumerate()
        .filter(|(i, _)| bins[*i] == 0)
        .map(|(_, s)| *s)
        .collect();
    if missing.is_empty() {
        "All nine regions have measurements. Check the residual arrows and held-out error.".into()
    } else {
        format!(
            "Add an anchor near: {}",
            missing.into_iter().take(3).collect::<Vec<_>>().join(", ")
        )
    }
}
fn lens_ready(points: &[Landmark], size: [u32; 2]) -> bool {
    let bins = coverage(points, size);
    points.iter().filter(|p| p.enabled).count() >= 12
        && bins[4] > 0
        && [0, 2, 6, 8].iter().filter(|i| bins[**i] > 0).count() >= 3
        && bins.iter().filter(|n| **n > 0).count() >= 7
}

/// Test the two principal spreads, independent of position, rotation and coordinate scale.
/// A very thin strip cannot reliably constrain a two-dimensional warp.
fn spans_area(points: &[Point]) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mean = mul(
        points.iter().fold([0., 0.], |a, p| add(a, *p)),
        1. / points.len() as f64,
    );
    let [mut xx, mut xy, mut yy] = [0.; 3];
    for p in points {
        let [x, y] = sub(*p, mean);
        xx += x * x;
        xy += x * y;
        yy += y * y;
    }
    let trace = xx + yy;
    let major = (trace + (xx - yy).hypot(2. * xy)) * 0.5;
    let minor = (xx * yy - xy * xy).max(0.) / major.max(1e-30);
    major > 1e-12 && minor / major > 1e-4
}

pub fn anchor_geometry_hint(points: &[Landmark]) -> Option<String> {
    let grid: Vec<_> = points
        .iter()
        .filter(|p| p.enabled)
        .map(|p| p.target.world())
        .collect();
    if grid.len() < 3 || spans_area(&grid) {
        return None;
    }
    if grid.iter().all(|p| length(sub(*p, grid[0])) < 1e-8) {
        return Some(
            "These anchors share one grid point. Assign distinct grid points before fitting."
                .into(),
        );
    }
    let column = grid.iter().all(|p| (p[0] - grid[0][0]).abs() < 1e-8);
    let row = grid.iter().all(|p| (p[1] - grid[0][1]).abs() < 1e-8);
    let location = if column {
        "the same grid column"
    } else if row {
        "the same grid row"
    } else {
        "one line or a very narrow strip"
    };
    Some(format!(
        "{} manual anchors lie on {location}. All points can refine position, rotation and uniform scale. For perspective, add points well away from this line in two different areas.",
        grid.len()
    ))
}

fn compose_anchor_fit(
    current: &Calibration,
    transform: [f64; 9],
    size: [u32; 2],
) -> Result<Calibration> {
    let mut result = current.clone();
    result.h = calibration::mat_mul(current.h, transform);
    let w = result.h[8];
    ensure!(w.abs() > 1e-10, "Grid origin lies on perspective horizon");
    for value in &mut result.h {
        *value /= w;
    }
    result.validate(size)?;
    Ok(result)
}

/// Select a supported model by geometry, not just by anchor count. Lower-dimensional
/// updates compose with the current map so existing lens/perspective information survives.
fn anchor_fit(
    points: &[Landmark],
    size: [u32; 2],
    current: &Calibration,
) -> Result<(Calibration, FitModel, String)> {
    let enabled: Vec<_> = points.iter().filter(|p| p.enabled).collect();
    if enabled.is_empty() {
        return Ok((current.clone(), FitModel::Unchanged, String::new()));
    }
    let pairs: Vec<_> = enabled
        .iter()
        .map(|p| {
            current
                .unmap(p.source, size)
                .map(|v| (p.target.world(), v))
                .ok_or_else(|| anyhow::anyhow!("Anchor is outside the valid projection"))
        })
        .collect::<Result<_>>()?;
    let mut note = String::new();
    let area = spans_area(&pairs.iter().map(|(q, _)| *q).collect::<Vec<_>>())
        && spans_area(&pairs.iter().map(|(_, p)| *p).collect::<Vec<_>>());
    if area && pairs.len() >= 4 {
        match calibration::fit(points, size, current, false) {
            Ok(result) => return Ok((result.calibration, FitModel::Perspective, note)),
            Err(e) => note.push_str(&format!(
                "Perspective deferred: {e}. Using a simpler supported model. "
            )),
        }
    }
    let n = pairs.len() as f64;
    let q_mean = mul(pairs.iter().fold([0., 0.], |a, (q, _)| add(a, *q)), 1. / n);
    let p_mean = mul(pairs.iter().fold([0., 0.], |a, (_, p)| add(a, *p)), 1. / n);
    if pairs.len() == 1 {
        let d = sub(p_mean, q_mean);
        return Ok((
            compose_anchor_fit(current, [1., 0., d[0], 0., 1., d[1], 0., 0., 1.], size)?,
            FitModel::Translation,
            note,
        ));
    }
    let den = pairs
        .iter()
        .map(|(q, _)| length(sub(*q, q_mean)).powi(2))
        .sum::<f64>();
    ensure!(
        den > 1e-8,
        "Choose different grid points; these anchors coincide"
    );
    if area {
        // Center and scale before QR: cell indices can be far from the grid origin.
        let radius = (den / n).sqrt();
        let matrix: Vec<_> = pairs
            .iter()
            .map(|(q, _)| {
                let q = mul(sub(*q, q_mean), 1. / radius);
                vec![q[0], q[1], 1.]
            })
            .collect();
        let affine = (|| -> Result<Calibration> {
            let x = calibration::least_squares(
                &matrix,
                &pairs
                    .iter()
                    .map(|(_, p)| p[0] - p_mean[0])
                    .collect::<Vec<_>>(),
            )?;
            let y = calibration::least_squares(
                &matrix,
                &pairs
                    .iter()
                    .map(|(_, p)| p[1] - p_mean[1])
                    .collect::<Vec<_>>(),
            )?;
            compose_anchor_fit(
                current,
                [
                    x[0] / radius,
                    x[1] / radius,
                    p_mean[0] + x[2] - (x[0] * q_mean[0] + x[1] * q_mean[1]) / radius,
                    y[0] / radius,
                    y[1] / radius,
                    p_mean[1] + y[2] - (y[0] * q_mean[0] + y[1] * q_mean[1]) / radius,
                    0.,
                    0.,
                    1.,
                ],
                size,
            )
        })();
        match affine {
            Ok(c) => return Ok((c, FitModel::Affine, note)),
            Err(e) => note.push_str(&format!("Affine deferred: {e}. ")),
        }
    }
    // Least-squares similarity uses EVERY enabled anchor, including collinear additions.
    let mut a = 0.;
    let mut b = 0.;
    for (q, p) in &pairs {
        let q = sub(*q, q_mean);
        let p = sub(*p, p_mean);
        a += q[0] * p[0] + q[1] * p[1];
        b += q[0] * p[1] - q[1] * p[0];
    }
    a /= den;
    b /= den;
    ensure!(
        a.hypot(b) > 1e-8,
        "Measured centers collapse onto one point; check the correspondences"
    );
    let c = compose_anchor_fit(
        current,
        [
            a,
            -b,
            p_mean[0] - a * q_mean[0] + b * q_mean[1],
            b,
            a,
            p_mean[1] - b * q_mean[0] - a * q_mean[1],
            0.,
            0.,
            1.,
        ],
        size,
    )?;
    if let Some(hint) = anchor_geometry_hint(points) {
        note.push_str(&hint);
        note.push(' ');
    }
    Ok((c, FitModel::Similarity, note))
}

fn measure(image: &DecodedImage, c: &Calibration, anchors: &[Landmark]) -> Vec<Landmark> {
    let size = image.size;
    let sites = sites_in_bounds(c, size, [0., 0., size[0] as f64, size[1] as f64], 180);
    // Disabled user anchors also suppress automatic measurements for that identity.
    let manual: HashSet<_> = anchors
        .iter()
        .map(|p| (p.target.cell, channel(p.target.kind), p.target.kind as u8))
        .collect();
    let mut counts = [0; 9];
    let mut out: Vec<Landmark> = Vec::new();
    for site in sites {
        if manual.contains(&(site.cell, channel(site.kind), site.kind as u8)) {
            continue;
        }
        let Some(predicted) = c.map(site.world(), size) else {
            continue;
        };
        let r = search_radius(c, size, site);
        if predicted[0] < r
            || predicted[1] < r
            || predicted[0] >= size[0] as f64 - r
            || predicted[1] >= size[1] as f64 - r
        {
            continue;
        }
        let bin = ((predicted[1] / size[1] as f64 * 3.) as usize) * 3
            + (predicted[0] / size[0] as f64 * 3.) as usize;
        if counts[bin] >= 32 {
            continue;
        }
        let Some(center) = snap_center(image, predicted, site.kind, r) else {
            continue;
        };
        // One detected blob cannot be assigned to two grid identities, including G1/G2.
        if anchors
            .iter()
            .filter(|p| p.enabled)
            .any(|p| length(sub(p.source, center.position)) < center.radius)
            || out
                .iter()
                .any(|p| length(sub(p.source, center.position)) < center.radius)
        {
            continue;
        }
        counts[bin] += 1;
        out.push(Landmark {
            source: center.position,
            target: site,
            enabled: true,
        });
    }
    out
}

fn weighted_fit(
    anchors: &[Landmark],
    automatic: &[Landmark],
    size: [u32; 2],
    current: &Calibration,
    lens: bool,
) -> Result<Calibration> {
    let mut points = anchors.to_vec();
    points.extend_from_slice(automatic);
    let anchor_count = anchors.iter().filter(|p| p.enabled).count().max(1);
    let mut weights = vec![(automatic.len() as f64 / anchor_count as f64).max(8.); anchors.len()];
    weights.extend(vec![1.; automatic.len()]);
    Ok(calibration::fit_weighted(&points, &weights, size, current, lens)?.calibration)
}
fn rms(c: &Calibration, points: &[Landmark], size: [u32; 2]) -> Option<f64> {
    let errors: Vec<_> = c.residuals(points, size).into_iter().flatten().collect();
    (!errors.is_empty()).then(|| {
        (errors.iter().map(|p| length(*p).powi(2)).sum::<f64>() / errors.len() as f64).sqrt()
    })
}

pub fn refine(
    image: &DecodedImage,
    anchors: &[Landmark],
    current: &Calibration,
    allow_lens: bool,
    complete: bool,
) -> Result<Assistance> {
    let size = image.size;
    let (anchor_calibration, anchor_model, mut note) = anchor_fit(anchors, size, current)?;
    let mut c = anchor_calibration.clone();
    let mut model = anchor_model;
    let mut automatic = if complete {
        measure(image, &c, anchors)
    } else {
        vec![]
    };
    // Deterministically hold out spatially distributed sites. They never enter the solver.
    let mut held_out = Vec::new();
    automatic.retain(|p| {
        let [i, j] = p.target.cell;
        if (i
            .wrapping_mul(31)
            .wrapping_add(j.wrapping_mul(17))
            .wrapping_add(p.target.kind as i32))
        .rem_euclid(5)
            == 0
        {
            held_out.push(p.clone());
            false
        } else {
            true
        }
    });
    let automatic_only = complete && !anchors.iter().any(|p| p.enabled);
    let mut used_lens = false;
    // Automatic completion is an optional extension of the already-valid manual fit.
    // It must not make collinear manual anchors fail or discard their latest correction.
    for pass in 0..2 {
        let mut all = anchors.to_vec();
        all.extend_from_slice(&automatic);
        used_lens = allow_lens && lens_ready(&all, size);
        let grid: Vec<_> = all
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.target.world())
            .collect();
        if (automatic.is_empty() && !used_lens) || grid.len() < 4 || !spans_area(&grid) {
            if !automatic.is_empty() {
                note.push_str("Automatic matches do not yet span a stable 2D region. ");
            }
            automatic.clear();
            c = anchor_calibration.clone();
            model = anchor_model;
            used_lens = false;
            break;
        }
        let mut fit = weighted_fit(anchors, &automatic, size, &c, used_lens);
        if fit.is_err() && used_lens {
            note.push_str(&format!(
                "Lens fit deferred: {}. ",
                fit.as_ref().unwrap_err()
            ));
            used_lens = false;
            fit = weighted_fit(anchors, &automatic, size, &c, false);
        }
        match fit {
            Ok(next) => {
                c = next;
                model = if used_lens {
                    FitModel::Lens
                } else {
                    FitModel::Perspective
                };
            }
            Err(e) if automatic_only => return Err(e),
            Err(e) => {
                note.push_str(&format!(
                    "Additional fit deferred: {e}. Applied all manual anchors with {}. ",
                    anchor_model.label()
                ));
                c = anchor_calibration.clone();
                model = anchor_model;
                used_lens = false;
                automatic.clear();
                break;
            }
        }
        if pass == 1 {
            break;
        }
        let old_len = automatic.len();
        automatic.retain(|p| {
            c.map(p.target.world(), size)
                .is_some_and(|q| length(sub(q, p.source)) <= 2.5)
        });
        if automatic.len() == old_len {
            break;
        }
        note.push_str(&format!(
            "Rejected {} automatic matches. ",
            old_len - automatic.len()
        ));
    }
    if automatic_only {
        ensure!(
            automatic.len() >= 16,
            "Too few reliable matches; drag a few grid points to establish alignment"
        );
    }
    c.validate(size)?;
    let validation_rms = rms(&c, &held_out, size);
    // A bad automatic proposal must never replace the last valid transform.
    if automatic_only {
        ensure!(
            validation_rms.is_some_and(|v| v < 2.5),
            "Automatic alignment failed the held-out check; use manual anchors"
        );
    }
    let mut all = anchors.to_vec();
    all.extend_from_slice(&automatic);
    let fitted_rms = rms(&c, &all, size).unwrap_or(0.);
    if !used_lens && allow_lens {
        note.push_str("Lens fitting waits for center and edge coverage. ");
    }
    note.push_str(&coverage_hint(&all, size));
    Ok(Assistance {
        calibration: c,
        anchors: anchors.to_vec(),
        model,
        automatic,
        held_out,
        validation_rms,
        fitted_rms,
        used_lens,
        note,
    })
}

pub fn auto_align(image: &DecodedImage) -> Result<Assistance> {
    let seed = lattice_seed(image)?;
    let first = refine(image, &[], &seed, true, true)?;
    let mut result = refine(image, &[], &first.calibration, true, true)?;
    result.note = format!(
        "Provisional cell origin: verify a common anchor across photos. {}",
        result.note
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_perfecto::io::Pixels;

    pub(super) fn photograph(lens: bool) -> (DecodedImage, Calibration) {
        let size = [800, 640];
        let s = Calibration::scale(size);
        let angle = 0.07_f64;
        let truth = Calibration {
            h: [
                24. * angle.cos() / s,
                -24. * angle.sin() / s,
                0.017,
                24. * angle.sin() / s,
                24. * angle.cos() / s,
                -0.023,
                0.0003,
                -0.0002,
                1.,
            ],
            lens: if lens {
                [0.035, -0.008, 0.001, -0.0015]
            } else {
                [0.; 4]
            },
            center: [400., 320.],
        };
        let mut pixels = vec![0.0_f32; size[0] as usize * size[1] as usize * 4];
        for p in pixels.chunks_exact_mut(4) {
            p[3] = 1.;
        }
        for site in sites_in_bounds(&truth, size, [0., 0., 800., 640.], 10_000) {
            // Dark blue pixels and varying brightness imitate changing screen content.
            if site.kind == Subpixel::Blue && (site.cell[0] + site.cell[1]).rem_euclid(3) == 0 {
                continue;
            }
            let Some(center) = truth.map(site.world(), size) else {
                continue;
            };
            let r = if site.kind == Subpixel::Blue {
                4.0
            } else {
                2.7
            };
            for y in (center[1] - r * 4.) as i32..=(center[1] + r * 4.) as i32 {
                for x in (center[0] - r * 4.) as i32..=(center[0] + r * 4.) as i32 {
                    if x < 0 || y < 0 || x >= 800 || y >= 640 {
                        continue;
                    }
                    let d = sub([x as f64 + 0.5, y as f64 + 0.5], center);
                    let value = (-0.5 * (d[0] * d[0] + d[1] * d[1]) / (r * r)).exp() as f32;
                    let i = ((639 - y) as usize * 800 + x as usize) * 4 + channel(site.kind);
                    pixels[i] = pixels[i].max(value * 0.8);
                }
            }
        }
        (
            DecodedImage {
                size,
                bit_depth: 16,
                pixels: Pixels::Linear(pixels),
            },
            truth,
        )
    }
    #[test]
    fn automatic_alignment_recovers_rotated_distorted_lattice_with_held_out_sites() {
        let (image, truth) = photograph(true);
        let result = auto_align(&image).unwrap();
        assert!(result.used_lens, "{}", result.note);
        assert!(result.automatic.len() > 60);
        assert!(result.held_out.len() > 12);
        assert!(result.validation_rms.unwrap() < 0.35, "{:?}", result);
        for p in &result.automatic {
            let actual = truth.unmap(p.source, image.size).unwrap();
            let nearest = GridSite::nearest(actual, p.target.kind);
            assert!(
                length(sub(actual, nearest.world())) < 0.025,
                "{:?} {:?}",
                p,
                actual
            );
            assert!(!result.held_out.iter().any(|q| q.target == p.target));
        }
    }
    #[test]
    fn local_snap_keeps_color_and_rejects_dark_or_distant_sites() {
        let (image, c) = photograph(false);
        for kind in Subpixel::ALL {
            let site = GridSite { cell: [2, 1], kind };
            let center = c.map(site.world(), image.size).unwrap();
            if kind == Subpixel::Blue {
                // fixture deliberately leaves this site dark
                assert!(snap_center(&image, center, kind, 6.).is_none());
                continue;
            }
            let measured = snap_center(&image, add(center, [1.3, -1.7]), kind, 6.).unwrap();
            assert!(length(sub(measured.position, center)) < 0.15);
            assert!(snap_center(&image, add(center, [13., 0.]), kind, 4.).is_none());
        }
        let blank = DecodedImage {
            size: [80, 80],
            bit_depth: 8,
            pixels: Pixels::Srgb8(vec![0; 80 * 80 * 4]),
        };
        assert!(auto_align(&blank).is_err());
        assert!(snap_center(&blank, [40., 40.], Subpixel::Red, 8.).is_none());
    }
    #[test]
    fn incremental_anchors_preserve_identity_and_fit_only_constrained_parameters() {
        let (image, truth) = photograph(false);
        let mut guess = truth.clone();
        guess.adjust([0.21, -0.31], 2., [1.02, 1.02]);
        let points: Vec<_> = [[0, 0], [5, 1], [-2, 5], [-5, -3]]
            .into_iter()
            .map(|cell| {
                let target = GridSite {
                    cell,
                    kind: Subpixel::Red,
                };
                Landmark {
                    source: truth.map(target.world(), image.size).unwrap(),
                    target,
                    enabled: true,
                }
            })
            .collect();
        for n in 1..=4 {
            let result = refine(&image, &points[..n], &guess, true, false).unwrap();
            for point in &points[..n] {
                assert!(
                    length(sub(
                        result
                            .calibration
                            .map(point.target.world(), image.size)
                            .unwrap(),
                        point.source
                    )) < 0.02
                );
            }
            assert!(!result.used_lens);
        }
        let unchanged = refine(&image, &[], &truth, true, false).unwrap();
        assert_eq!(unchanged.calibration, truth);
        assert!(unchanged.automatic.is_empty());
        let result = refine(&image, &points, &guess, true, true).unwrap();
        assert!(result.validation_rms.unwrap() < 0.3);
        for point in &points {
            assert!(
                length(sub(
                    result
                        .calibration
                        .map(point.target.world(), image.size)
                        .unwrap(),
                    point.source
                )) < 0.2
            );
        }
        let mut disabled = points.clone();
        disabled[0].enabled = false;
        let result = refine(&image, &disabled, &truth, true, true).unwrap();
        assert!(
            !result
                .automatic
                .iter()
                .chain(&result.held_out)
                .any(|p| p.target == disabled[0].target)
        );
    }
    #[test]
    fn coverage_requires_center_and_corners_and_coincident_anchors_fail() {
        let (image, c) = photograph(false);
        let points: Vec<_> = (0..16)
            .map(|i| Landmark {
                source: [300. + i as f64, 310.],
                target: GridSite {
                    cell: [i, 0],
                    kind: Subpixel::Red,
                },
                enabled: true,
            })
            .collect();
        assert!(!lens_ready(&points, image.size));
        assert!(coverage_hint(&points, image.size).contains("bottom"));
        let result = refine(&image, &points, &c, true, false).unwrap();
        assert_eq!(result.model, FitModel::Similarity);
        assert!(!result.used_lens);
        let mut duplicates = points;
        for p in &mut duplicates {
            p.target = GridSite {
                cell: [0, 0],
                kind: Subpixel::Red,
            };
        }
        assert!(refine(&image, &duplicates, &c, true, true).is_err());
    }
    #[test]
    fn screenshot_column_keeps_fitting_all_anchors_and_upgrades_only_with_spatial_constraints() {
        let size = [1600, 1200];
        let image = DecodedImage {
            size,
            bit_depth: 8,
            pixels: Pixels::Srgb8(vec![0; 1600 * 1200 * 4]),
        };
        let mut truth = Calibration::initial(size);
        truth.h[0] = 12. / Calibration::scale(size);
        truth.h[4] = truth.h[0];
        truth.lens = [0.04, -0.005, 0.001, -0.001];
        truth.h[6] = 0.0006;
        truth.h[7] = -0.0004;
        let points: Vec<_> = [
            [-26, -3],
            [-27, -4],
            [-17, 6],
            [-12, 11],
            [-16, -3],
            [-20, 9],
        ]
        .into_iter()
        .map(|cell| {
            let target = GridSite {
                cell,
                kind: Subpixel::Red,
            };
            Landmark {
                source: truth.map(target.world(), size).unwrap(),
                target,
                enabled: true,
            }
        })
        .collect();
        assert!(
            anchor_geometry_hint(&points[..4])
                .unwrap()
                .contains("same grid column")
        );
        // A little picking noise in two adjacent anchors extrapolates to large errors far away.
        let mut measured = points.clone();
        measured[1].source = add(measured[1].source, [1.8, 0.5]);
        let two = refine(&image, &measured[..2], &truth, true, false).unwrap();
        let stale_error = rms(&two.calibration, &measured[..4], size).unwrap();
        assert!(
            stale_error > 8.,
            "fixture must reproduce the stale two-anchor extrapolation"
        );
        for complete in [false, true] {
            let mut current = two.calibration.clone();
            for n in 3..=4 {
                let result = refine(&image, &measured[..n], &current, true, complete).unwrap();
                assert_eq!(result.model, FitModel::Similarity);
                assert!(result.fitted_rms < 1.5, "{}", result.fitted_rms);
                assert_eq!(result.calibration.lens, truth.lens);
                assert_eq!(result.calibration.center, truth.center);
                assert_eq!(result.anchors, measured[..n]);
                assert!(!result.used_lens);
                current = result.calibration;
            }
        }
        // Four collinear + one off-line point still cannot define a full homography.
        // Keep the valid affine update until another off-line point constrains perspective.
        let mut current = truth.clone();
        current.adjust([0.21, -0.31], 2., [1.02, 1.02]);
        for (n, model) in [
            (3, FitModel::Similarity),
            (4, FitModel::Similarity),
            (5, FitModel::Affine),
            (6, FitModel::Perspective),
        ] {
            let result = refine(&image, &points[..n], &current, true, false).unwrap();
            assert_eq!(result.model, model, "{n} anchors");
            assert!(
                result.fitted_rms < 0.02,
                "{n} anchors: {}",
                result.fitted_rms
            );
            current = result.calibration;
        }
    }
    #[test]
    fn nearly_collinear_anchors_do_not_create_unstable_perspective_or_affine_updates() {
        let size = [1600, 1200];
        let image = DecodedImage {
            size,
            bit_depth: 8,
            pixels: Pixels::Srgb8(vec![]),
        };
        let mut truth = Calibration::initial(size);
        truth.h[0] = 0.001;
        truth.h[4] = 0.001;
        let points: Vec<_> = [[0, 0], [100, 100], [200, 201], [300, 300]]
            .into_iter()
            .map(|cell| {
                let target = GridSite {
                    cell,
                    kind: Subpixel::Red,
                };
                Landmark {
                    source: truth.map(target.world(), size).unwrap(),
                    target,
                    enabled: true,
                }
            })
            .collect();
        let mut current = truth.clone();
        current.adjust([0.21, -0.31], 2., [1.02, 1.02]);
        let result = refine(&image, &points, &current, false, false).unwrap();
        assert_eq!(result.model, FitModel::Similarity);
        assert!(result.fitted_rms < 1e-6);
        result.calibration.validate(size).unwrap();
    }
    #[test]
    #[ignore = "Set PIXEL_PERFECTO_PHOTO to ocean-weather-icon.png for real-image manual acceptance"]
    fn supplied_photo_collinear_anchors() {
        let path = std::env::var("PIXEL_PERFECTO_PHOTO").expect("Set PIXEL_PERFECTO_PHOTO");
        let (_, _, asset) = crate::pixel_perfecto::io::import(std::path::Path::new(&path)).unwrap();
        let image = &asset.image;
        assert_eq!(
            image.size,
            [3024, 4032],
            "This manual fixture is for ocean-weather-icon.png"
        );
        // Coarse, manually selected red centers on one column of the supplied photograph.
        // Refine each at original resolution; no successful automatic lattice seed is required.
        let centers = red_centers(image).unwrap();
        let probes = [
            [1494., 2015.],
            [1493., 1929.],
            [1504., 2794.],
            [1510., 3227.],
        ];
        let points: Vec<_> = [[-26, -3], [-27, -4], [-17, 6], [-12, 11]]
            .into_iter()
            .zip(probes)
            .map(|(cell, probe)| {
                let approximate = centers
                    .iter()
                    .min_by(|a, b| {
                        length(sub(a.position, probe)).total_cmp(&length(sub(b.position, probe)))
                    })
                    .unwrap();
                assert!(
                    length(sub(approximate.position, probe)) < 35.,
                    "Visible center near {probe:?}"
                );
                let center = snap_center(image, approximate.position, Subpixel::Red, 18.).unwrap();
                println!(
                    "Manual photo correspondence {cell:?} -> {:?}",
                    center.position
                );
                Landmark {
                    source: center.position,
                    target: GridSite {
                        cell,
                        kind: Subpixel::Red,
                    },
                    enabled: true,
                }
            })
            .collect();
        let mut current = Calibration::initial(image.size);
        let mut stale_error = 0.;
        for n in 1..=4 {
            let before = rms(&current, &points[..n], image.size).unwrap();
            let result = refine(image, &points[..n], &current, true, false).unwrap();
            println!(
                "Real photograph: {n} column anchors, {}, RMS {:.4} source px (before {:.4})",
                result.model.label(),
                result.fitted_rms,
                before
            );
            assert!(result.fitted_rms <= before + 1e-6);
            if n >= 2 {
                assert_eq!(result.model, FitModel::Similarity);
            }
            current = result.calibration;
            if n == 2 {
                stale_error = rms(&current, &points, image.size).unwrap();
            }
        }
        assert!(rms(&current, &points, image.size).unwrap() < stale_error);
        let assisted = refine(image, &points, &current, true, true).unwrap();
        println!(
            "Automatic completion after four column anchors: {} points, {}, manual RMS {:.4}",
            assisted.automatic.len(),
            assisted.model.label(),
            rms(&assisted.calibration, &points, image.size).unwrap()
        );
        assisted.calibration.validate(image.size).unwrap();
    }
    #[test]
    #[ignore = "Set PIXEL_PERFECTO_PHOTO to a real microscope photo for independent acceptance"]
    fn supplied_photo_automatic_acceptance() {
        let path = std::env::var("PIXEL_PERFECTO_PHOTO").expect("Set PIXEL_PERFECTO_PHOTO");
        let (_, _, asset) = crate::pixel_perfecto::io::import(std::path::Path::new(&path)).unwrap();
        let started = std::time::Instant::now();
        let result = auto_align(&asset.image).unwrap();
        println!(
            "Automatic real-photo calibration: {} fit points, {} held out; fit RMS {:.4}, held-out RMS {:.4}, lens {}, elapsed {:?}",
            result.automatic.len(),
            result.held_out.len(),
            result.fitted_rms,
            result.validation_rms.unwrap(),
            result.used_lens,
            started.elapsed()
        );
        assert!(result.automatic.len() > 40);
        assert!(result.validation_rms.unwrap() < 1.0);
    }
}
