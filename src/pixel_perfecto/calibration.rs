//! Grid -> undistorted normalized image -> Brown-Conrady -> original image.
//! The mapping is deliberately in the sampling direction: no iterative inverse in the shader.
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use super::model::{Landmark, Point, add, length, mul, sub};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    /// Row-major homography, mapping world grid coordinates to normalized image coordinates.
    pub h: [f64; 9],
    /// k1, k2, p1, p2. The normalization scale is max(width,height)/2.
    pub lens: [f64; 4],
    /// Public source pixel coordinates, bottom-left/Y-up.
    pub center: Point,
}
#[derive(Clone, Debug)]
pub struct CalibrationResult {
    pub calibration: Calibration,
    pub residuals: Vec<Option<Point>>,
    pub rms: f64,
    pub iterations: usize,
}

pub fn project(h: &[f64; 9], p: Point) -> Option<Point> {
    let w = h[6] * p[0] + h[7] * p[1] + h[8];
    if !w.is_finite() || w.abs() < 1e-10 {
        return None;
    }
    let out = [
        (h[0] * p[0] + h[1] * p[1] + h[2]) / w,
        (h[3] * p[0] + h[4] * p[1] + h[5]) / w,
    ];
    out.iter().all(|v| v.is_finite()).then_some(out)
}
pub fn mat_mul(a: [f64; 9], b: [f64; 9]) -> [f64; 9] {
    let mut r = [0.; 9];
    for i in 0..3 {
        for j in 0..3 {
            for k in 0..3 {
                r[i * 3 + j] += a[i * 3 + k] * b[k * 3 + j];
            }
        }
    }
    r
}
fn inverse(h: [f64; 9]) -> Option<[f64; 9]> {
    let [a, b, c, d, e, f, g, h, i] = h;
    let cof = [
        e * i - f * h,
        c * h - b * i,
        b * f - c * e,
        f * g - d * i,
        a * i - c * g,
        c * d - a * f,
        d * h - e * g,
        b * g - a * h,
        a * e - b * d,
    ];
    let det = a * cof[0] + b * cof[3] + c * cof[6];
    if det.abs() < 1e-14 || !det.is_finite() {
        None
    } else {
        Some(cof.map(|v| v / det))
    }
}
pub fn distort(p: Point, l: [f64; 4]) -> Point {
    let [x, y] = p;
    let [k1, k2, p1, p2] = l;
    let r = x * x + y * y;
    let radial = 1. + k1 * r + k2 * r * r;
    [
        x * radial + 2. * p1 * x * y + p2 * (r + 2. * x * x),
        y * radial + p1 * (r + 2. * y * y) + 2. * p2 * x * y,
    ]
}
fn lens_jacobian(p: Point, l: [f64; 4]) -> [f64; 4] {
    let [x, y] = p;
    let [k1, k2, p1, p2] = l;
    let r = x * x + y * y;
    let a = 1. + k1 * r + k2 * r * r;
    let d = 2. * k1 + 4. * k2 * r;
    [
        a + x * x * d + 2. * p1 * y + 6. * p2 * x,
        x * y * d + 2. * p1 * x + 2. * p2 * y,
        x * y * d + 2. * p1 * x + 2. * p2 * y,
        a + y * y * d + 6. * p1 * y + 2. * p2 * x,
    ]
}
impl Calibration {
    pub fn initial(size: [u32; 2]) -> Self {
        let s = size[0].max(size[1]) as f64 * 0.5;
        Self {
            h: [32. / s, 0., 0., 0., 32. / s, 0., 0., 0., 1.],
            lens: [0.; 4],
            center: [size[0] as f64 * 0.5, size[1] as f64 * 0.5],
        }
    }
    pub fn scale(size: [u32; 2]) -> f64 {
        size[0].max(size[1]) as f64 * 0.5
    }
    pub fn map(&self, p: Point, size: [u32; 2]) -> Option<Point> {
        let p = distort(project(&self.h, p)?, self.lens);
        let p = add(self.center, mul(p, Self::scale(size)));
        p.iter().all(|v| v.is_finite()).then_some(p)
    }
    pub fn unmap(&self, source: Point, size: [u32; 2]) -> Option<Point> {
        let target = mul(sub(source, self.center), 1. / Self::scale(size));
        let mut p = target;
        for _ in 0..30 {
            let err = sub(distort(p, self.lens), target);
            if length(err) < 1e-10 {
                return project(&inverse(self.h)?, p);
            }
            let j = lens_jacobian(p, self.lens);
            let det = j[0] * j[3] - j[1] * j[2];
            if det.abs() < 1e-10 {
                return None;
            }
            p = sub(
                p,
                [
                    (j[3] * err[0] - j[1] * err[1]) / det,
                    (-j[2] * err[0] + j[0] * err[1]) / det,
                ],
            );
            if length(p) > 100. {
                return None;
            }
        }
        None
    }
    pub fn density_at(&self, p: Point, size: [u32; 2]) -> f64 {
        let Some(a) = self.map(p, size) else {
            return 32.;
        };
        let Some(x) = self.map(add(p, [1e-3, 0.]), size) else {
            return 32.;
        };
        let Some(y) = self.map(add(p, [0., 1e-3]), size) else {
            return 32.;
        };
        let x = mul(sub(x, a), 1000.);
        let y = mul(sub(y, a), 1000.);
        (x[0] * y[1] - x[1] * y[0]).abs().sqrt().clamp(0.1, 10000.)
    }
    /// Displayed orientation of the original image's X axis at the optical center.
    /// Positive angles are counterclockwise in the public Y-up coordinate system.
    pub fn image_rotation_degrees(&self, size: [u32; 2]) -> Option<f64> {
        let a = self.unmap(self.center, size)?;
        let b = self.unmap(add(self.center, [0.01, 0.]), size)?;
        let d = sub(b, a);
        (length(d) > 1e-12).then(|| d[1].atan2(d[0]).to_degrees())
    }
    /// Rotate the displayed image around a corrected-grid pivot. Sampling still reads
    /// the original, and lens coefficients/optical center remain in original coordinates.
    pub fn rotated_about(&self, degrees: f64, pivot: Point, size: [u32; 2]) -> Result<Self> {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let inverse_pose = [
            cos,
            sin,
            pivot[0] - cos * pivot[0] - sin * pivot[1],
            -sin,
            cos,
            pivot[1] + sin * pivot[0] - cos * pivot[1],
            0.,
            0.,
            1.,
        ];
        let mut next = self.clone();
        next.h = mat_mul(self.h, inverse_pose);
        let denominator = next.h[8];
        ensure!(
            denominator.abs() > 1e-10,
            "Rotation places the grid origin on a perspective horizon"
        );
        for value in &mut next.h {
            *value /= denominator;
        }
        next.validate(size)?;
        Ok(next)
    }
    /// Compose a continuous fine adjustment in corrected grid space.
    pub fn adjust(&mut self, translation: Point, rotation: f64, scale: Point) {
        let (sin, cos) = rotation.to_radians().sin_cos();
        let inverse_pose = [
            cos / scale[0],
            sin / scale[0],
            -(cos * translation[0] + sin * translation[1]) / scale[0],
            -sin / scale[1],
            cos / scale[1],
            (sin * translation[0] - cos * translation[1]) / scale[1],
            0.,
            0.,
            1.,
        ];
        self.h = mat_mul(self.h, inverse_pose);
    }
    pub fn validate(&self, size: [u32; 2]) -> Result<()> {
        ensure!(
            self.h
                .iter()
                .chain(self.lens.iter())
                .chain(self.center.iter())
                .all(|v| v.is_finite()),
            "Calibration contains non-finite values"
        );
        ensure!(
            inverse(self.h).is_some(),
            "Perspective transform is singular"
        );
        ensure!(size[0] > 0 && size[1] > 0, "Empty image");
        // Verify lens orientation throughout the image, not only at calibration landmarks.
        let s = Self::scale(size);
        for iy in 0..=16 {
            for ix in 0..=16 {
                let source = [
                    size[0] as f64 * ix as f64 / 16.,
                    size[1] as f64 * iy as f64 / 16.,
                ];
                let p = mul(sub(source, self.center), 1. / s);
                let j = lens_jacobian(p, self.lens);
                ensure!(
                    j[0] * j[3] - j[1] * j[2] > 0.02,
                    "Lens mapping folds or becomes singular near the image edge"
                );
                let world = self.unmap(source, size).ok_or_else(|| {
                    anyhow::anyhow!("Image edge cannot be mapped back to the grid")
                })?;
                let w = self.h[6] * world[0] + self.h[7] * world[1] + self.h[8];
                ensure!(
                    w.is_finite() && w.abs() > 1e-6,
                    "Perspective horizon crosses image"
                );
            }
        }
        // Inverse projective denominator must not cross zero over the undistorted image.
        let inv = inverse(self.h).unwrap();
        let mut sign = 0.0_f64;
        for iy in 0..=8 {
            for ix in 0..=8 {
                let p = mul(
                    sub(
                        [
                            size[0] as f64 * ix as f64 / 8.,
                            size[1] as f64 * iy as f64 / 8.,
                        ],
                        self.center,
                    ),
                    1. / s,
                );
                let w = inv[6] * p[0] + inv[7] * p[1] + inv[8];
                ensure!(
                    w.abs() > 1e-7 && (sign == 0. || sign * w > 0.),
                    "Perspective horizon crosses image"
                );
                sign = w.signum();
            }
        }
        Ok(())
    }
    pub fn residuals(&self, points: &[Landmark], size: [u32; 2]) -> Vec<Option<Point>> {
        points
            .iter()
            .map(|p| {
                p.enabled
                    .then(|| self.map(p.target.world(), size).map(|v| sub(v, p.source)))
                    .flatten()
            })
            .collect()
    }
}

// Partial-pivoted small dense system, used for damped normal equations (at most 12 unknowns).
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for k in 0..n {
        let pivot = (k..n).max_by(|&i, &j| a[i][k].abs().total_cmp(&a[j][k].abs()))?;
        if a[pivot][k].abs() < 1e-15 {
            return None;
        }
        a.swap(k, pivot);
        b.swap(k, pivot);
        for i in k + 1..n {
            let f = a[i][k] / a[k][k];
            for j in k..n {
                a[i][j] -= f * a[k][j];
            }
            b[i] -= f * b[k];
        }
    }
    let mut x = vec![0.; n];
    for i in (0..n).rev() {
        x[i] = (b[i] - (i + 1..n).map(|j| a[i][j] * x[j]).sum::<f64>()) / a[i][i];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}
// Reorthogonalized QR for the homography initializer; reject deficient point configurations.
pub(super) fn least_squares(a: &[Vec<f64>], b: &[f64]) -> Result<Vec<f64>> {
    let n = a[0].len();
    let m = a.len();
    let mut q = vec![vec![0.; m]; n];
    let mut r = vec![vec![0.; n]; n];
    for k in 0..n {
        let mut v: Vec<f64> = (0..m).map(|i| a[i][k]).collect();
        for _ in 0..2 {
            for j in 0..k {
                let dot = v.iter().zip(&q[j]).map(|(a, b)| a * b).sum::<f64>();
                r[j][k] += dot;
                for i in 0..m {
                    v[i] -= dot * q[j][i];
                }
            }
        }
        let norm = v.iter().map(|v| v * v).sum::<f64>().sqrt();
        ensure!(
            norm > 1e-8,
            "Calibration points are collinear, duplicated, or ill-conditioned"
        );
        r[k][k] = norm;
        q[k] = v.into_iter().map(|v| v / norm).collect();
    }
    let rhs = q
        .iter()
        .map(|c| c.iter().zip(b).map(|(x, y)| x * y).sum())
        .collect();
    solve(r, rhs).ok_or_else(|| anyhow::anyhow!("Cannot solve perspective transform"))
}
fn evaluate(params: &[f64], q: Point, lens: bool, fixed: [f64; 4]) -> Option<Point> {
    let h = [
        params[0], params[1], params[2], params[3], params[4], params[5], params[6], params[7], 1.,
    ];
    let l = if lens {
        [params[8], params[9], params[10], params[11]]
    } else {
        fixed
    };
    Some(distort(project(&h, q)?, l))
}
fn huber_cost(
    params: &[f64],
    samples: &[(Point, Point)],
    lens: bool,
    fixed: [f64; 4],
    threshold: f64,
    weights: &[f64],
) -> f64 {
    samples
        .iter()
        .zip(weights)
        .map(|((q, p), weight)| {
            let Some(v) = evaluate(params, *q, lens, fixed) else {
                return f64::INFINITY;
            };
            let r = length(sub(v, *p));
            if r <= threshold {
                weight * r * r * 0.5
            } else {
                weight * threshold * (r - threshold * 0.5)
            }
        })
        .sum()
}

pub fn fit(
    points: &[Landmark],
    size: [u32; 2],
    current: &Calibration,
    fit_lens: bool,
) -> Result<CalibrationResult> {
    fit_weighted(points, &vec![1.; points.len()], size, current, fit_lens)
}

/// Explicit weights let measured anchors retain influence over automatic detections.
pub(super) fn fit_weighted(
    points: &[Landmark],
    weights: &[f64],
    size: [u32; 2],
    current: &Calibration,
    fit_lens: bool,
) -> Result<CalibrationResult> {
    let points: Vec<_> = points.iter().map(|p| Correspondence {
        source: p.source, target: p.target.world(), enabled: p.enabled,
    }).collect();
    fit_correspondences(&points, weights, size, current, fit_lens)
}

/// Arbitrary named-target pixel/UV correspondences, independent of the subpixel grid UI.
#[derive(Clone, Debug)]
pub struct Correspondence {
    pub source: Point,
    pub target: Point,
    pub enabled: bool,
}

pub fn fit_correspondences(
    points: &[Correspondence], weights: &[f64], size: [u32; 2],
    current: &Calibration, fit_lens: bool,
) -> Result<CalibrationResult> {
    fit_correspondences_impl(points, weights, size, current, fit_lens, false)
}

/// A phone is a narrow planar target, possibly at any rotation in the sensor frame.
/// Check its principal spans instead of requiring coverage along the sensor X/Y axes.
/// Callers must also verify coverage in the target's own coordinates.
pub fn fit_planar_correspondences(
    points: &[Correspondence], weights: &[f64], size: [u32; 2],
    current: &Calibration,
) -> Result<CalibrationResult> {
    fit_correspondences_impl(points, weights, size, current, true, true)
}

fn fit_correspondences_impl(
    points: &[Correspondence], weights: &[f64], size: [u32; 2],
    current: &Calibration, fit_lens: bool, oriented_plane: bool,
) -> Result<CalibrationResult> {
    ensure!(
        points.len() == weights.len() && weights.iter().all(|w| w.is_finite() && *w > 0.),
        "Invalid calibration weights"
    );
    let weights: Vec<_> = points
        .iter()
        .zip(weights)
        .filter(|(p, _)| p.enabled)
        .map(|(_, w)| *w)
        .collect();
    let pts: Vec<_> = points.iter().filter(|p| p.enabled).collect();
    ensure!(
        pts.len() >= if fit_lens { 12 } else { 4 },
        "Need at least {} enabled points",
        if fit_lens { 12 } else { 4 }
    );
    let n = pts.len() as f64;
    let s = Calibration::scale(size);
    let mean = mul(
        pts.iter().fold([0., 0.], |a, p| add(a, p.target)),
        1. / n,
    );
    let radius = (pts
        .iter()
        .map(|p| length(sub(p.target, mean)).powi(2))
        .sum::<f64>()
        / n)
        .sqrt();
    ensure!(
        radius > 1e-6,
        "Grid points must cover a two-dimensional region"
    );
    if fit_lens && oriented_plane {
        let center = mul(pts.iter().fold([0., 0.], |a, p| add(a, p.source)), 1. / n);
        let mut covariance = [0.; 3];
        for p in &pts {
            let [x, y] = sub(p.source, center);
            covariance[0] += x * x;
            covariance[1] += x * y;
            covariance[2] += y * y;
        }
        let angle = 0.5 * (2. * covariance[1]).atan2(covariance[0] - covariance[2]);
        let (sin, cos) = angle.sin_cos();
        let mut spans = [[cos, sin], [-sin, cos]].map(|axis| {
            let values = pts.iter().map(|p| {
                let q = sub(p.source, center);
                q[0] * axis[0] + q[1] * axis[1]
            });
            values.clone().fold(f64::NEG_INFINITY, f64::max)
                - values.fold(f64::INFINITY, f64::min)
        });
        spans.sort_by(f64::total_cmp);
        ensure!(spans[0] >= size[0].min(size[1]) as f64 * 0.20
            && spans[1] >= size[0].max(size[1]) as f64 * 0.35,
            "Screen is too small or too oblique for lens correction. Move closer while keeping all chart markers visible");
    } else if fit_lens {
        for axis in 0..2 {
            let lo = pts
                .iter()
                .map(|p| p.source[axis])
                .fold(f64::INFINITY, f64::min);
            let hi = pts
                .iter()
                .map(|p| p.source[axis])
                .fold(f64::NEG_INFINITY, f64::max);
            ensure!(
                hi - lo >= size[axis] as f64 * 0.35,
                "Lens fit needs points spread across the image, including its edges"
            );
        }
    }
    let samples: Vec<_> = pts
        .iter()
        .map(|p| {
            (
                mul(sub(p.target, mean), 1. / radius),
                mul(sub(p.source, current.center), 1. / s),
            )
        })
        .collect();
    let mut a = vec![];
    let mut b = vec![];
    for &([x, y], [u, v]) in &samples {
        a.push(vec![x, y, 1., 0., 0., 0., -u * x, -u * y]);
        b.push(u);
        a.push(vec![0., 0., 0., x, y, 1., -v * x, -v * y]);
        b.push(v);
    }
    let mut params = least_squares(&a, &b)?;
    // Starting from a fresh undistorted initializer avoids carrying a bad manual lens guess into a fit.
    let fixed = if fit_lens { [0.; 4] } else { current.lens };
    if fit_lens {
        params.extend([0.; 4]);
    }
    let threshold = 2. / s;
    let mut cost = huber_cost(&params, &samples, fit_lens, fixed, threshold, &weights);
    let mut lambda = 1e-3;
    let mut converged = false;
    let mut iterations = 0;
    for iteration in 0..180 {
        iterations = iteration + 1;
        let m = params.len();
        let mut normal = vec![vec![0.; m]; m];
        let mut rhs = vec![0.; m];
        for (&(q, target), point_weight) in samples.iter().zip(&weights) {
            let predicted = evaluate(&params, q, fit_lens, fixed)
                .ok_or_else(|| anyhow::anyhow!("Invalid perspective during fitting"))?;
            let residual = sub(target, predicted);
            let norm = length(residual);
            let weight = point_weight
                * if norm <= threshold {
                    1.
                } else {
                    threshold / norm
                };
            let mut jac = vec![[0.; 2]; m];
            for k in 0..m {
                let eps = 1e-6 * (params[k].abs() + 1.);
                let mut p = params.clone();
                p[k] += eps;
                let plus = evaluate(&p, q, fit_lens, fixed).unwrap_or(predicted);
                p[k] -= 2. * eps;
                let minus = evaluate(&p, q, fit_lens, fixed).unwrap_or(predicted);
                jac[k] = mul(sub(plus, minus), 0.5 / eps);
            }
            for i in 0..m {
                rhs[i] += weight * (jac[i][0] * residual[0] + jac[i][1] * residual[1]);
                for j in 0..m {
                    normal[i][j] += weight * (jac[i][0] * jac[j][0] + jac[i][1] * jac[j][1]);
                }
            }
        }
        if rhs.iter().map(|v| v.abs()).fold(0., f64::max) < 1e-11 {
            converged = true;
            break;
        }
        for (i, row) in normal.iter_mut().enumerate() {
            row[i] += lambda * (row[i].abs() + 1e-9);
        }
        let Some(step) = solve(normal, rhs) else {
            bail!("Calibration system is singular");
        };
        let candidate: Vec<_> = params.iter().zip(&step).map(|(p, s)| p + s).collect();
        let next = huber_cost(&candidate, &samples, fit_lens, fixed, threshold, &weights);
        if next.is_finite() && next < cost {
            let improvement = cost - next;
            params = candidate;
            cost = next;
            lambda = (lambda * 0.3).max(1e-12);
            if improvement < 1e-14 || step.iter().map(|v| v * v).sum::<f64>() < 1e-18 {
                converged = true;
                break;
            }
        } else {
            lambda *= 10.;
            if lambda > 1e12 {
                break;
            }
        }
    }
    ensure!(
        converged,
        "Calibration did not converge; check point correspondences and coverage"
    );
    let h = [
        params[0], params[1], params[2], params[3], params[4], params[5], params[6], params[7], 1.,
    ];
    let norm = [
        1. / radius,
        0.,
        -mean[0] / radius,
        0.,
        1. / radius,
        -mean[1] / radius,
        0.,
        0.,
        1.,
    ];
    let mut h = mat_mul(h, norm);
    let w = h[8];
    ensure!(w.abs() > 1e-10, "Grid origin lies on perspective horizon");
    for v in &mut h {
        *v /= w;
    }
    let calibration = Calibration {
        h,
        lens: if fit_lens {
            [params[8], params[9], params[10], params[11]]
        } else {
            fixed
        },
        center: current.center,
    };
    calibration.validate(size)?;
    let residuals: Vec<_> = points.iter().map(|p| if p.enabled { calibration.map(p.target, size).map(|v| sub(p.source, v)) } else { None }).collect();
    let rms = (residuals
        .iter()
        .flatten()
        .map(|r| r[0] * r[0] + r[1] * r[1])
        .sum::<f64>()
        / n)
        .sqrt();
    Ok(CalibrationResult {
        calibration,
        residuals,
        rms,
        iterations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_perfecto::model::{GridSite, Subpixel};
    fn fixture(lens: bool) -> (Calibration, Vec<Landmark>, [u32; 2]) {
        let size = [1600, 1200];
        let mut c = Calibration::initial(size);
        c.h = [
            0.043, 0.007, 0.02, -0.004, 0.042, -0.03, 0.0007, -0.0004, 1.,
        ];
        if lens {
            c.lens = [0.09, -0.018, 0.002, -0.003];
        }
        let mut points = vec![];
        for i in -8..=8 {
            for j in -6..=6 {
                let target = GridSite {
                    cell: [i, j],
                    kind: Subpixel::Red,
                };
                let source = c.map(target.world(), size).unwrap();
                if source[0] > 10. && source[1] > 10. && source[0] < 1590. && source[1] < 1190. {
                    points.push(Landmark {
                        target,
                        source,
                        enabled: true,
                    });
                }
            }
        }
        (c, points, size)
    }
    #[test]
    fn fits_perspective_and_lens_and_roundtrips() {
        for lens in [false, true] {
            let (truth, points, size) = fixture(lens);
            let result = fit(&points, size, &Calibration::initial(size), lens).unwrap();
            assert!(result.rms < 0.01, "RMS {}", result.rms);
            for p in [[0., 0.], [-6.2, 9.], [8., -4.3]] {
                let source = truth.map(p, size).unwrap();
                assert!(length(sub(result.calibration.map(p, size).unwrap(), source)) < 0.02);
                assert!(length(sub(truth.unmap(source, size).unwrap(), p)) < 1e-6);
            }
        }
    }
    #[test]
    fn image_rotation_keeps_pivot_and_lens_and_matches_displayed_direction() {
        let (original, _, size) = fixture(true);
        let pivot = [3.1, -2.4];
        let pivot_source = original.map(pivot, size).unwrap();
        let orientation = original.image_rotation_degrees(size).unwrap();
        for degrees in [-135., -90., -17.25, 37., 90., 180.] {
            let rotated = original.rotated_about(degrees, pivot, size).unwrap();
            assert_eq!(rotated.lens, original.lens);
            assert_eq!(rotated.center, original.center);
            assert!(length(sub(rotated.map(pivot, size).unwrap(), pivot_source)) < 1e-8);
            let (sin, cos) = degrees.to_radians().sin_cos();
            for point in [[-4., 2.], [0., 0.], [6., -3.]] {
                let source = original.map(point, size).unwrap();
                let d = sub(point, pivot);
                let expected = add(pivot, [cos * d[0] - sin * d[1], sin * d[0] + cos * d[1]]);
                assert!(length(sub(rotated.unmap(source, size).unwrap(), expected)) < 1e-6);
            }
            let actual = rotated.image_rotation_degrees(size).unwrap();
            let error = (actual - orientation - degrees + 180.).rem_euclid(360.) - 180.;
            assert!(error.abs() < 1e-5, "rotation {degrees}: error {error}");
            let restored = rotated.rotated_about(-degrees, pivot, size).unwrap();
            assert!(
                length(sub(
                    restored.map([2., 3.], size).unwrap(),
                    original.map([2., 3.], size).unwrap()
                )) < 1e-8
            );
        }
    }
    #[test]
    fn robust_fit_limits_single_bad_correspondence() {
        let (_, mut points, size) = fixture(true);
        points[7].source[0] += 80.;
        let result = fit(&points, size, &Calibration::initial(size), true).unwrap();
        let mut errors: Vec<_> = result
            .residuals
            .iter()
            .flatten()
            .map(|r| length(*r))
            .collect();
        errors.sort_by(f64::total_cmp);
        assert!(
            errors[errors.len() / 2] < 0.2,
            "median error {}",
            errors[errors.len() / 2]
        );
        assert!(length(result.residuals[7].unwrap()) > 70.);
    }
    #[test]
    fn rejects_degenerate_undercovered_and_folded_models() {
        let (mut c, mut points, size) = fixture(false);
        points.truncate(4);
        assert!(fit(&points, size, &c, true).is_err());
        for p in &mut points {
            p.target.cell[1] = 0;
        }
        assert!(fit(&points, size, &c, false).is_err());
        c.lens = [-2., 0., 0., 0.];
        assert!(c.validate(size).is_err());
    }
}
