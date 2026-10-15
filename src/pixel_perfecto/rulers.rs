//! Shared horizontal/vertical rulers and editor-only alignment guides.
use super::model::*;
use rust_wgpu_fiber::eframe::egui;

pub const THICKNESS: f32 = 24.;
#[derive(Clone, Copy)]
pub struct Layout {
    pub canvas: egui::Rect,
    pub top: egui::Rect,
    pub left: egui::Rect,
    pub visible: bool,
}
impl Layout {
    pub fn new(body: egui::Rect, visible: bool) -> Self {
        let size = if visible { THICKNESS } else { 0. };
        let canvas = egui::Rect::from_min_max(body.min + egui::vec2(size, size), body.max);
        let top =
            egui::Rect::from_min_max(egui::pos2(canvas.left(), body.top()), canvas.right_top());
        let left =
            egui::Rect::from_min_max(egui::pos2(body.left(), canvas.top()), canvas.left_bottom());
        Self {
            canvas,
            top,
            left,
            visible,
        }
    }
}
fn size(rect: egui::Rect) -> Point {
    [rect.width() as f64, rect.height() as f64]
}
pub fn world_at(view: &Viewport, rect: egui::Rect, position: egui::Pos2) -> Point {
    view.screen_to_world(
        [
            (position.x - rect.left()) as f64,
            (position.y - rect.top()) as f64,
        ],
        size(rect),
    )
}
pub fn screen(view: &Viewport, rect: egui::Rect, point: Point) -> egui::Pos2 {
    let p = view.world_to_screen(point, size(rect));
    rect.min + egui::vec2(p[0] as f32, p[1] as f32)
}
pub fn major_step(zoom: f64) -> f64 {
    let needed = 72. / zoom;
    let power = 10_f64.powf(needed.log10().floor());
    [1., 2., 5., 10.]
        .into_iter()
        .find(|v| v * power >= needed)
        .unwrap_or(10.)
        * power
}
pub fn label(value: f64, step: f64) -> String {
    let value = if value.abs() < step * 1e-7 { 0. } else { value };
    let decimals = (-step.log10().floor()).max(0.).min(6.) as usize;
    format!("{value:.decimals$}")
}
pub fn hit(
    settings: &RulerSettings,
    view: &Viewport,
    rect: egui::Rect,
    p: egui::Pos2,
) -> Option<u64> {
    if !settings.visible || !settings.guides_visible || settings.locked || !rect.contains(p) {
        return None;
    }
    settings
        .guides
        .iter()
        .filter_map(|guide| {
            let q = screen(
                view,
                rect,
                if guide.axis == GuideAxis::Vertical {
                    [guide.position, 0.]
                } else {
                    [0., guide.position]
                },
            );
            let distance = if guide.axis == GuideAxis::Vertical {
                (p.x - q.x).abs()
            } else {
                (p.y - q.y).abs()
            };
            (distance <= 5.).then_some((guide.id, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|p| p.0)
}

pub fn paint_rulers(
    painter: &egui::Painter,
    layout: Layout,
    view: &Viewport,
    pointer: Option<egui::Pos2>,
) {
    if !layout.visible {
        return;
    }
    let canvas = layout.canvas;
    let corner =
        egui::Rect::from_min_max(egui::pos2(layout.left.left(), layout.top.top()), canvas.min);
    let bg = egui::Color32::from_gray(32);
    let ink = egui::Color32::from_gray(165);
    for rect in [layout.top, layout.left, corner] {
        painter.rect_filled(rect, 0., bg);
    }
    painter.text(
        corner.center(),
        egui::Align2::CENTER_CENTER,
        "xy",
        egui::FontId::monospace(10.),
        ink,
    );
    let step = major_step(view.zoom);
    let minor = step / 5.;
    let bounds = view.bounds(size(canvas));
    for axis in [GuideAxis::Vertical, GuideAxis::Horizontal] {
        let ruler = if axis == GuideAxis::Vertical {
            layout.top
        } else {
            layout.left
        };
        let painter = painter.with_clip_rect(ruler);
        let coordinate = axis.coordinate();
        let lo = bounds[coordinate];
        let hi = bounds[coordinate + 2];
        let first = (lo / minor).ceil() as i64;
        let last = (hi / minor).floor() as i64;
        for index in first..=last.min(first.saturating_add(2000)) {
            let value = index as f64 * minor;
            let p = screen(
                view,
                canvas,
                if coordinate == 0 {
                    [value, 0.]
                } else {
                    [0., value]
                },
            );
            let major = index.rem_euclid(5) == 0;
            let length = if major { 9. } else { 4. };
            if coordinate == 0 {
                painter.line_segment(
                    [
                        egui::pos2(p.x, ruler.bottom() - length),
                        egui::pos2(p.x, ruler.bottom()),
                    ],
                    egui::Stroke::new(1.0_f32, ink),
                );
                if major {
                    painter.text(
                        egui::pos2(p.x + 3., ruler.top() + 2.),
                        egui::Align2::LEFT_TOP,
                        label(value, step),
                        egui::FontId::monospace(10.),
                        ink,
                    );
                }
            } else {
                painter.line_segment(
                    [
                        egui::pos2(ruler.right() - length, p.y),
                        egui::pos2(ruler.right(), p.y),
                    ],
                    egui::Stroke::new(1.0_f32, ink),
                );
                if major {
                    let galley = painter.layout_no_wrap(
                        label(value, step),
                        egui::FontId::monospace(10.),
                        ink,
                    );
                    let mut shape = egui::epaint::TextShape::new(
                        egui::pos2(ruler.left() + 3., p.y - 3.),
                        galley,
                        ink,
                    );
                    shape.angle = -std::f32::consts::FRAC_PI_2;
                    painter.add(shape);
                }
            }
        }
    }
    painter.line_segment(
        [canvas.left_top(), canvas.right_top()],
        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(65)),
    );
    painter.line_segment(
        [canvas.left_top(), canvas.left_bottom()],
        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(65)),
    );
    if let Some(p) = pointer.filter(|p| canvas.contains(*p)) {
        let color = egui::Color32::from_rgb(80, 180, 255);
        painter.line_segment(
            [
                egui::pos2(p.x, layout.top.top()),
                egui::pos2(p.x, layout.top.bottom()),
            ],
            egui::Stroke::new(1.0_f32, color),
        );
        painter.line_segment(
            [
                egui::pos2(layout.left.left(), p.y),
                egui::pos2(layout.left.right(), p.y),
            ],
            egui::Stroke::new(1.0_f32, color),
        );
    }
}
fn badge(painter: &egui::Painter, position: egui::Pos2, text: String, color: egui::Color32) {
    let galley = painter.layout_no_wrap(text, egui::FontId::monospace(11.), egui::Color32::WHITE);
    let rect = egui::Rect::from_min_size(position, galley.size() + egui::vec2(8., 4.));
    painter.rect_filled(rect, 2., color);
    painter.galley(rect.min + egui::vec2(4., 2.), galley, egui::Color32::WHITE);
}
pub fn paint_guides(
    painter: &egui::Painter,
    rect: egui::Rect,
    view: &Viewport,
    settings: &RulerSettings,
    selected: Option<u64>,
) {
    if !settings.visible || !settings.guides_visible {
        return;
    }
    let painter = painter.with_clip_rect(rect);
    for guide in &settings.guides {
        let p = screen(
            view,
            rect,
            if guide.axis == GuideAxis::Vertical {
                [guide.position, 0.]
            } else {
                [0., guide.position]
            },
        );
        let active = selected == Some(guide.id);
        let color = if active {
            egui::Color32::from_rgb(90, 190, 255)
        } else {
            egui::Color32::from_rgba_unmultiplied(255, 95, 110, 185)
        };
        let (a, b, label_pos) = if guide.axis == GuideAxis::Vertical {
            (
                egui::pos2(p.x, rect.top()),
                egui::pos2(p.x, rect.bottom()),
                egui::pos2(p.x + 4., rect.top() + 4.),
            )
        } else {
            (
                egui::pos2(rect.left(), p.y),
                egui::pos2(rect.right(), p.y),
                egui::pos2(rect.left() + 4., p.y + 4.),
            )
        };
        painter.line_segment(
            [a, b],
            egui::Stroke::new(if active { 1.5_f32 } else { 1.0_f32 }, color),
        );
        if active {
            badge(
                &painter,
                label_pos,
                format!("{} {}", guide.axis.label(), label(guide.position, 0.001)),
                egui::Color32::from_rgb(35, 110, 180),
            );
        }
    }
    // A shared-coordinate distance readout makes guide pairs useful across different images.
    if let Some(guide) = settings.guides.iter().find(|g| Some(g.id) == selected) {
        if let Some(other) = settings
            .guides
            .iter()
            .filter(|g| g.id != guide.id && g.axis == guide.axis)
            .min_by(|a, b| {
                (a.position - guide.position)
                    .abs()
                    .total_cmp(&(b.position - guide.position).abs())
            })
        {
            let (a, b) = if guide.axis == GuideAxis::Vertical {
                (
                    screen(view, rect, [guide.position, view.center[1]]),
                    screen(view, rect, [other.position, view.center[1]]),
                )
            } else {
                (
                    screen(view, rect, [view.center[0], guide.position]),
                    screen(view, rect, [view.center[0], other.position]),
                )
            };
            let color = egui::Color32::from_rgb(255, 125, 135);
            painter.arrow(a, b - a, egui::Stroke::new(1.0_f32, color));
            painter.arrow(b, a - b, egui::Stroke::new(1.0_f32, color));
            badge(
                &painter,
                a.lerp(b, 0.5) + egui::vec2(5., 5.),
                format!(
                    "Δ{} {}",
                    guide.axis.label().to_lowercase(),
                    label((other.position - guide.position).abs(), 0.001)
                ),
                egui::Color32::from_rgb(150, 50, 70),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_ticks_and_guide_hits_follow_shared_y_up_coordinates_at_every_zoom() {
        let rect = egui::Rect::from_min_size(egui::pos2(130., 90.), egui::vec2(800., 600.));
        for zoom in [0.02, 0.25, 24., 96., 1024., 4096.] {
            let mut view = Viewport {
                center: [-12.5, 9.25],
                zoom,
            };
            let step = major_step(zoom);
            assert!((72. - 1e-9..=180. + 1e-9).contains(&(step * zoom)));
            assert_eq!(label(-step, step), format!("-{}", label(step, step)));
            assert!(!label(-1e-15, step).starts_with('-'));
            let world = world_at(&view, rect, rect.center() + egui::vec2(-80., -60.));
            assert!(world[0] < view.center[0] && world[1] > view.center[1]);
            assert!(
                screen(&view, rect, world).distance(rect.center() + egui::vec2(-80., -60.)) < 0.001
            );
            let mut settings = RulerSettings {
                guides: vec![Guide {
                    id: 1,
                    axis: GuideAxis::Horizontal,
                    position: world[1],
                }],
                ..Default::default()
            };
            let point = screen(&view, rect, world);
            assert_eq!(
                hit(&settings, &view, rect, point + egui::vec2(40., 4.9)),
                Some(1)
            );
            assert_eq!(
                hit(&settings, &view, rect, point + egui::vec2(40., 5.1)),
                None
            );
            view.zoom_at(
                1.7,
                [
                    (point.x - rect.left()) as f64,
                    (point.y - rect.top()) as f64,
                ],
                size(rect),
            );
            assert!(screen(&view, rect, world).distance(point) < 0.001);
            assert_eq!(hit(&settings, &view, rect, point), Some(1));
            settings.locked = true;
            assert_eq!(hit(&settings, &view, rect, point), None);
            settings.locked = false;
            settings.visible = false;
            assert_eq!(hit(&settings, &view, rect, point), None);
        }
    }
}
