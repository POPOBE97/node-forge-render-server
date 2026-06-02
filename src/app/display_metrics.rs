use rust_wgpu_fiber::eframe::egui;

pub const DEFAULT_DISPLAY_PPI: f32 = 160.0;
pub const MIN_DISPLAY_PPI: f32 = 50.0;
pub const MAX_DISPLAY_PPI: f32 = 1000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurrentDisplayMetrics {
    pub display_ppi: Option<f32>,
    pub pixels_per_point: f32,
}

pub fn clamp_display_ppi(ppi: f32) -> f32 {
    if ppi.is_finite() {
        ppi.clamp(MIN_DISPLAY_PPI, MAX_DISPLAY_PPI)
    } else {
        DEFAULT_DISPLAY_PPI
    }
}

pub fn simulation_zoom(
    current_display_ppi: f32,
    target_ppi: f32,
    pixels_per_point: f32,
) -> Option<f32> {
    if !current_display_ppi.is_finite()
        || !target_ppi.is_finite()
        || !pixels_per_point.is_finite()
        || current_display_ppi <= 0.0
        || target_ppi <= 0.0
        || pixels_per_point <= 0.0
    {
        return None;
    }

    Some(current_display_ppi / target_ppi / pixels_per_point)
}

pub fn ppi_from_pixels_and_mm(
    width_px: f64,
    height_px: f64,
    width_mm: f64,
    height_mm: f64,
) -> Option<f32> {
    if width_px <= 0.0 || height_px <= 0.0 || width_mm <= 0.0 || height_mm <= 0.0 {
        return None;
    }

    let pixel_diagonal = width_px.hypot(height_px);
    let inch_diagonal = width_mm.hypot(height_mm) / 25.4;
    if inch_diagonal <= 0.0 {
        return None;
    }

    let ppi = pixel_diagonal / inch_diagonal;
    ppi.is_finite().then_some(ppi as f32)
}

pub fn current_display_metrics(ctx: &egui::Context) -> CurrentDisplayMetrics {
    let pixels_per_point = ctx.pixels_per_point();
    CurrentDisplayMetrics {
        display_ppi: current_display_ppi_for_viewport(ctx, pixels_per_point),
        pixels_per_point,
    }
}

fn monitor_pixel_size_from_points(
    monitor_size_points: egui::Vec2,
    pixels_per_point: f32,
) -> Option<(f64, f64)> {
    if !monitor_size_points.x.is_finite()
        || !monitor_size_points.y.is_finite()
        || !pixels_per_point.is_finite()
        || monitor_size_points.x <= 0.0
        || monitor_size_points.y <= 0.0
        || pixels_per_point <= 0.0
    {
        return None;
    }

    Some((
        (monitor_size_points.x * pixels_per_point) as f64,
        (monitor_size_points.y * pixels_per_point) as f64,
    ))
}

#[cfg(target_os = "macos")]
fn current_display_ppi_for_viewport(ctx: &egui::Context, pixels_per_point: f32) -> Option<f32> {
    platform::current_display_ppi(ctx, pixels_per_point)
}

#[cfg(not(target_os = "macos"))]
fn current_display_ppi_for_viewport(_ctx: &egui::Context, _pixels_per_point: f32) -> Option<f32> {
    None
}

#[cfg(target_os = "macos")]
mod platform {
    use std::cmp::Ordering;

    use core_graphics::display::{CGDirectDisplayID, CGDisplay};
    use rust_wgpu_fiber::eframe::egui;

    use super::{monitor_pixel_size_from_points, ppi_from_pixels_and_mm};

    #[derive(Clone, Copy, Debug)]
    struct ViewportDisplaySource {
        monitor_pixel_size: Option<(f64, f64)>,
        native_pixels_per_point: Option<f32>,
        viewport_center: Option<egui::Pos2>,
        pixels_per_point: f32,
    }

    pub fn current_display_ppi(ctx: &egui::Context, pixels_per_point: f32) -> Option<f32> {
        let source = viewport_display_source(ctx, pixels_per_point);

        let display = display_matching_viewport_source(source).unwrap_or_else(CGDisplay::main);

        ppi_for_display(display)
    }

    fn viewport_display_source(
        ctx: &egui::Context,
        pixels_per_point: f32,
    ) -> ViewportDisplaySource {
        ctx.input(|i| {
            let viewport = i.viewport();
            ViewportDisplaySource {
                monitor_pixel_size: viewport
                    .monitor_size
                    .and_then(|size| monitor_pixel_size_from_points(size, pixels_per_point)),
                native_pixels_per_point: viewport.native_pixels_per_point,
                viewport_center: viewport
                    .inner_rect
                    .or(viewport.outer_rect)
                    .map(|rect| rect.center()),
                pixels_per_point,
            }
        })
    }

    fn display_matching_viewport_source(source: ViewportDisplaySource) -> Option<CGDisplay> {
        display_matching_monitor(source).or_else(|| {
            source
                .viewport_center
                .and_then(|point| display_containing_viewport_point(point, source.pixels_per_point))
        })
    }

    fn display_matching_monitor(source: ViewportDisplaySource) -> Option<CGDisplay> {
        let monitor_pixel_size = source.monitor_pixel_size?;
        let displays = CGDisplay::active_displays().ok()?;

        displays
            .into_iter()
            .filter_map(|id| {
                let display = CGDisplay::new(id);
                display_match_score(display, monitor_pixel_size, source)
                    .map(|score| (score, display))
            })
            .min_by(|(score_a, _), (score_b, _)| {
                score_a.partial_cmp(score_b).unwrap_or(Ordering::Equal)
            })
            .map(|(_, display)| display)
    }

    fn display_match_score(
        display: CGDisplay,
        monitor_pixel_size: (f64, f64),
        source: ViewportDisplaySource,
    ) -> Option<f64> {
        let (width_px, height_px) = display_pixel_size(display);
        let pixel_delta = (width_px as f64 - monitor_pixel_size.0).abs()
            + (height_px as f64 - monitor_pixel_size.1).abs();
        if pixel_delta > 2.0 {
            return None;
        }

        let scale_delta = source
            .native_pixels_per_point
            .and_then(|native_pixels_per_point| {
                display_backing_scale(display)
                    .map(|scale| (scale - native_pixels_per_point as f64).abs())
            })
            .unwrap_or(0.0);
        let center_bonus = source
            .viewport_center
            .is_some_and(|point| {
                display_contains_viewport_point(display, point, source.pixels_per_point)
            })
            .then_some(-10.0)
            .unwrap_or(0.0);

        Some(pixel_delta + scale_delta * 100.0 + center_bonus)
    }

    fn display_containing_viewport_point(
        point: egui::Pos2,
        pixels_per_point: f32,
    ) -> Option<CGDisplay> {
        let displays = CGDisplay::active_displays().ok()?;
        displays.into_iter().find_map(|id| {
            let display = CGDisplay::new(id);
            display_contains_viewport_point(display, point, pixels_per_point).then_some(display)
        })
    }

    fn display_contains_viewport_point(
        display: CGDisplay,
        point: egui::Pos2,
        pixels_per_point: f32,
    ) -> bool {
        display_contains_point(display, point)
            || (pixels_per_point.is_finite()
                && pixels_per_point > 0.0
                && display_contains_point(
                    display,
                    egui::pos2(point.x * pixels_per_point, point.y * pixels_per_point),
                ))
    }

    fn display_contains_point(display: CGDisplay, point: egui::Pos2) -> bool {
        let bounds = display.bounds();
        let min_x = bounds.origin.x as f32;
        let min_y = bounds.origin.y as f32;
        let max_x = min_x + bounds.size.width as f32;
        let max_y = min_y + bounds.size.height as f32;

        point.x >= min_x && point.x <= max_x && point.y >= min_y && point.y <= max_y
    }

    fn display_backing_scale(display: CGDisplay) -> Option<f64> {
        let bounds = display.bounds();
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return None;
        }

        let (width_px, height_px) = display_pixel_size(display);
        let scale_x = width_px as f64 / bounds.size.width;
        let scale_y = height_px as f64 / bounds.size.height;
        let scale = (scale_x + scale_y) * 0.5;
        scale.is_finite().then_some(scale)
    }

    fn ppi_for_display(display: CGDisplay) -> Option<f32> {
        let screen_size = display.screen_size();
        let (width_px, height_px) = display_pixel_size(display);

        ppi_from_pixels_and_mm(
            width_px as f64,
            height_px as f64,
            screen_size.width,
            screen_size.height,
        )
    }

    fn display_pixel_size(display: CGDisplay) -> (u64, u64) {
        display
            .display_mode()
            .map(|mode| (mode.pixel_width(), mode.pixel_height()))
            .filter(|(w, h)| *w > 0 && *h > 0)
            .unwrap_or_else(|| (display.pixels_wide(), display.pixels_high()))
    }

    #[allow(dead_code)]
    fn _display_from_id(id: CGDirectDisplayID) -> CGDisplay {
        CGDisplay::new(id)
    }
}

#[cfg(test)]
mod tests {
    use rust_wgpu_fiber::eframe::egui;

    use super::{
        clamp_display_ppi, monitor_pixel_size_from_points, ppi_from_pixels_and_mm, simulation_zoom,
    };

    #[test]
    fn simulation_zoom_matches_one_to_one_when_ppi_matches() {
        let zoom = simulation_zoom(220.0, 220.0, 2.0).unwrap();
        assert!((zoom - 0.5).abs() < 1e-6);
    }

    #[test]
    fn simulation_zoom_shrinks_for_higher_target_ppi() {
        let zoom = simulation_zoom(220.0, 440.0, 2.0).unwrap();
        assert!((zoom - 0.25).abs() < 1e-6);
    }

    #[test]
    fn simulation_zoom_rejects_invalid_inputs() {
        assert_eq!(simulation_zoom(0.0, 220.0, 2.0), None);
        assert_eq!(simulation_zoom(220.0, -1.0, 2.0), None);
        assert_eq!(simulation_zoom(220.0, 220.0, 0.0), None);
    }

    #[test]
    fn clamp_display_ppi_handles_invalid_and_out_of_range_values() {
        assert_eq!(clamp_display_ppi(f32::NAN), 160.0);
        assert_eq!(clamp_display_ppi(10.0), 50.0);
        assert_eq!(clamp_display_ppi(1200.0), 1000.0);
        assert_eq!(clamp_display_ppi(264.0), 264.0);
    }

    #[test]
    fn ppi_from_pixels_and_mm_uses_diagonals() {
        let ppi = ppi_from_pixels_and_mm(1920.0, 1080.0, 508.0, 285.75).unwrap();
        assert!((ppi - 96.0).abs() < 0.2);
    }

    #[test]
    fn monitor_pixel_size_uses_window_pixels_per_point() {
        let size = monitor_pixel_size_from_points(egui::vec2(720.0, 450.0), 2.0).unwrap();
        assert!((size.0 - 1440.0).abs() < f64::EPSILON);
        assert!((size.1 - 900.0).abs() < f64::EPSILON);
    }

    #[test]
    fn monitor_pixel_size_rejects_invalid_values() {
        assert_eq!(
            monitor_pixel_size_from_points(egui::vec2(720.0, 450.0), 0.0),
            None
        );
        assert_eq!(
            monitor_pixel_size_from_points(egui::vec2(f32::NAN, 450.0), 2.0),
            None
        );
    }
}
