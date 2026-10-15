use super::{io::over, model::*};

pub fn line_step(grid: &GridSettings, density: f64) -> f64 {
    let ratio = grid.min_spacing as f64 / (density * 2_f64.sqrt());
    2_f64.powf(ratio.max(1.).log2().ceil())
}
fn coverage(distance: f64, width: f64, density: f64) -> f32 {
    ((width * 0.5 + 0.5 / density - distance) * density).clamp(0., 1.) as f32
}
/// Signed distance to an ideal RGB footprint; negative inside. Shared with line counting.
pub fn subpixel_distance(p: Point, kind: Subpixel) -> f64 {
    match kind {
        Subpixel::Red | Subpixel::Blue => {
            (p[0].abs() + p[1].abs() - if kind == Subpixel::Red { 0.2 } else { 0.29 })
                / 2_f64.sqrt()
        }
        _ => {
            let sign = if kind == Subpixel::Green1 { 1. } else { -1. };
            let x = (p[0] + sign * p[1]) / 2_f64.sqrt();
            let y = (-sign * p[0] + p[1]) / 2_f64.sqrt();
            (x.abs() - 0.08).max(0.).hypot(y) - 0.055
        }
    }
}
/// Procedural overlay shared with grid_color in render.wgsl. Labels are editor-only.
pub fn sample_grid(world: Point, grid: &GridSettings, density: f64) -> [f32; 4] {
    if !grid.visible {
        return [0.; 4];
    }
    let cell = world_to_cell(world);
    let step = line_step(grid, density);
    let mut result = [0.; 4];
    if grid.lines {
        let d = ((cell[0] / step - (cell[0] / step).round())
            .abs()
            .min((cell[1] / step - (cell[1] / step).round()).abs()))
            * step
            * 2_f64.sqrt();
        let a = coverage(d, 1. / density, density) * grid.opacity * 0.45;
        result = [0.4 * a, 0.48 * a, 0.58 * a, a];
    }
    if density < grid.min_spacing as f64 {
        return result;
    }
    for kind in Subpixel::ALL {
        let p = sub(world, GridSite::nearest(world, kind).world());
        let mut a = if grid.centers {
            coverage(length(p), 3. / density, density)
        } else {
            0.
        };
        if grid.outlines {
            let d = subpixel_distance(p, kind).abs();
            a = a.max(coverage(d, 1. / density, density));
        }
        a *= grid.opacity;
        let rgb = kind.color();
        result = over([rgb[0] * a, rgb[1] * a, rgb[2] * a, a], result);
    }
    result
}
