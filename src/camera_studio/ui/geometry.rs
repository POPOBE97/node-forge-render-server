//! Display adapter for the session's RAW geometry. Measurement continues to use original CFA data.
use super::*;
use crate::screen_test::model::SessionGeometryStore;
use std::sync::{Arc, Mutex};

impl Workspace {
    pub(super) fn geometry_store(&self) -> Arc<Mutex<SessionGeometryStore>> {
        self.screen_test
            .as_ref()
            .map(|p| p.controller.geometry.clone())
            .unwrap_or_else(session_geometry)
    }
    pub(super) fn sync_display_geometry(&mut self) {
        let next = self
            .geometry_store()
            .lock()
            .ok()
            .and_then(|s| s.active_display());
        if next.as_ref().map(|g| &g.profile.id)
            != self.display_geometry.as_ref().map(|g| &g.profile.id)
        {
            self.display_geometry = next;
            self.geometry_enabled = true;
            self.live_view = ImageTransform::default();
            self.photo_view = ImageTransform::default();
        }
    }
    pub(super) fn correction_for_path(&self, path: &Path) -> Option<Arc<DisplayGeometry>> {
        // Analysis outputs are already rectified. Historical/imported files without a session
        // association must not silently inherit a calibration from a different phone placement.
        let record = self
            .records
            .iter()
            .find(|r| r.files.iter().any(|f| f.path == path))?;
        let id = record.context.get("display_geometry_id")?;
        self.geometry_store().lock().ok()?.display(id)
    }
    pub(super) fn canvas_geometry(&self) -> Option<Arc<DisplayGeometry>> {
        if self.calibration_running {
            return None;
        }
        match &self.source {
            CanvasSource::Live => self.display_geometry.clone(),
            CanvasSource::Photo(path) => self.correction_for_path(path),
            CanvasSource::Record(_) => None,
        }
    }
}

/// Convert camera pixel coordinates only at the egui texture boundary. The shared mapping
/// remains Y-up and is evaluated in full active RAW pixels, never in preview dimensions.
pub(super) fn sampling_uv(
    g: &DisplayGeometry,
    size: [u32; 2],
    orientation: Orientation,
    target: [f64; 2],
) -> Option<egui::Pos2> {
    let source = g.profile.mapping.map(target, g.profile.raw_size)?;
    let swap = matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    );
    let [width, height] = if swap { [size[1], size[0]] } else { size };
    if width == 0 || height == 0 {
        return None;
    }
    let [mut x, mut y, mut w, mut h] = g.sensor_crop;
    // Camera JPEG aspect-ratio modes crop the native image area centrally. Live View and
    // full-size photographs can have different sizes/aspects but share the same RAW fit.
    let ratio = width as f64 / height as f64;
    if w / h > ratio {
        let next = h * ratio;
        x += (w - next) * 0.5;
        w = next;
    } else {
        let next = w / ratio;
        y += (h - next) * 0.5;
        h = next;
    }
    let u = (source[0] - x + 0.5) / w;
    let v = 1. - (source[1] - y + 0.5) / h;
    let [u, v] = match orientation {
        Orientation::NoTransforms => [u, v],
        Orientation::Rotate90 => [1. - v, u],
        Orientation::Rotate180 => [1. - u, 1. - v],
        Orientation::Rotate270 => [v, 1. - u],
        Orientation::FlipHorizontal => [1. - u, v],
        Orientation::FlipVertical => [u, 1. - v],
        Orientation::Rotate90FlipH => [v, u],
        Orientation::Rotate270FlipH => [1. - v, 1. - u],
    };
    (u.is_finite() && v.is_finite()).then(|| egui::pos2(u as f32, v as f32))
}

/// Tessellate the inverse sampling map, including radial/tangential lens distortion.
/// Unit positions and texture coordinates are cached across live frames; pixels stay on GPU.
pub(super) fn unit_mesh(
    g: &DisplayGeometry,
    size: [u32; 2],
    orientation: Orientation,
    steps: u32,
) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    for y in 0..=steps {
        for x in 0..=steps {
            let u = x as f64 / steps as f64;
            let v = y as f64 / steps as f64;
            let uv = sampling_uv(g, size, orientation, [u, 1. - v]).unwrap_or(egui::pos2(-1., -1.));
            mesh.vertices.push(egui::epaint::Vertex {
                pos: egui::pos2(u as f32, v as f32),
                uv,
                color: Color32::WHITE,
            });
        }
    }
    for y in 0..steps {
        for x in 0..steps {
            let a = y * (steps + 1) + x;
            for triangle in [[a, a + 1, a + steps + 2], [a, a + steps + 2, a + steps + 1]] {
                if triangle.iter().all(|i| {
                    let uv = mesh.vertices[*i as usize].uv;
                    (0.0..=1.0).contains(&uv.x) && (0.0..=1.0).contains(&uv.y)
                }) {
                    mesh.indices.extend(triangle);
                }
            }
        }
    }
    mesh
}

pub(super) fn place_mesh(
    mut mesh: egui::Mesh,
    texture: egui::TextureId,
    rect: egui::Rect,
    turns: u8,
) -> egui::Mesh {
    mesh.texture_id = texture;
    for vertex in &mut mesh.vertices {
        let egui::Pos2 { x, y } = vertex.pos;
        let point = match turns % 4 {
            1 => egui::vec2(1. - y, x),
            2 => egui::vec2(1. - x, 1. - y),
            3 => egui::vec2(y, 1. - x),
            _ => egui::vec2(x, y),
        };
        vertex.pos = rect.min + point * rect.size();
    }
    mesh
}

#[derive(Clone, Debug)]
pub(super) struct WarpCache {
    id: String,
    size: [u32; 2],
    orientation: Orientation,
    mesh: egui::Mesh,
}
impl WarpCache {
    pub fn mesh(cache: &mut Option<Self>, g: &DisplayGeometry, view: &ImageView) -> egui::Mesh {
        if cache.as_ref().is_none_or(|c| {
            c.id != g.profile.id || c.size != view.size || c.orientation != view.orientation
        }) {
            *cache = Some(Self {
                id: g.profile.id.clone(),
                size: view.size,
                orientation: view.orientation,
                mesh: unit_mesh(g, view.size, view.orientation, 64),
            });
        }
        cache.as_ref().unwrap().mesh.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_perfecto::calibration::Calibration;
    use crate::screen_test::model::GeometryProfile;

    fn geometry(size: [u32; 2], crop: [f64; 4]) -> DisplayGeometry {
        let s = Calibration::scale(size);
        DisplayGeometry {
            profile: GeometryProfile {
                id: "geometry-a".into(),
                revision: 1,
                created_ms: 0,
                source_capture: "calibration".into(),
                camera_key: "650D".into(),
                raw_size: size,
                mapping: Calibration {
                    h: [
                        (crop[2] - 1.) / s,
                        0.,
                        crop[0] / s,
                        0.,
                        (crop[3] - 1.) / s,
                        crop[1] / s,
                        0.,
                        0.,
                        1.,
                    ],
                    lens: [0.; 4],
                    center: [0.; 2],
                },
                holdout_rms: 0.2,
                marker_count: 39,
            },
            screen_size: [1260, 2800],
            sensor_crop: crop,
        }
    }
    #[test]
    fn live_and_full_photo_sample_same_sensor_area_including_canon_border_crop() {
        let g = geometry([5208, 3476], [12., 8., 5184., 3456.]);
        for p in [[0., 0.], [0.2, 0.7], [1., 1.]] {
            let live = sampling_uv(&g, [960, 640], Orientation::NoTransforms, p).unwrap();
            let photo = sampling_uv(&g, [5184, 3456], Orientation::NoTransforms, p).unwrap();
            assert_eq!(live, photo);
        }
        let origin = sampling_uv(&g, [960, 640], Orientation::NoTransforms, [0., 0.]).unwrap();
        assert!((origin.x - 0.5 / 5184.).abs() < 1e-6);
        assert!((origin.y - (1. - 0.5 / 3456.)).abs() < 1e-6);
    }
    #[test]
    fn exif_rotations_and_mirrors_match_the_image_decoder_for_every_pixel() {
        let g = geometry([3, 2], [0., 0., 3., 2.]);
        let original =
            image::RgbaImage::from_fn(3, 2, |x, y| image::Rgba([(x + y * 3) as u8, 0, 0, 255]));
        for exif in 1..=8 {
            let orientation = Orientation::from_exif(exif).unwrap();
            let mut decoded = image::DynamicImage::ImageRgba8(original.clone());
            decoded.apply_orientation(orientation);
            let decoded = decoded.into_rgba8();
            for y in 0..2 {
                for x in 0..3 {
                    let uv = sampling_uv(
                        &g,
                        [decoded.width(), decoded.height()],
                        orientation,
                        [x as f64 / 2., 1. - y as f64],
                    )
                    .unwrap();
                    let sample = decoded.get_pixel(
                        (uv.x * decoded.width() as f32) as u32,
                        (uv.y * decoded.height() as f32) as u32,
                    );
                    assert_eq!(
                        sample,
                        original.get_pixel(x, y),
                        "EXIF {exif} pixel {x},{y}"
                    );
                }
            }
        }
    }
    #[test]
    fn mesh_preserves_non_affine_geometry_and_masks_outside_the_source() {
        let mut g = geometry([5208, 3476], [12., 8., 5184., 3456.]);
        g.profile.mapping = Calibration {
            h: [0.18, -0.45, 0.15, 0.65, 0.04, -0.34, 0.03, 0.02, 1.],
            lens: [0.02, -0.002, 0.001, -0.001],
            center: [2604., 1738.],
        };
        let mesh = unit_mesh(&g, [5184, 3456], Orientation::NoTransforms, 64);
        assert_eq!(mesh.indices.len(), 64 * 64 * 6);
        for triangle in mesh.indices.chunks_exact(3) {
            let vertices = triangle
                .iter()
                .map(|i| mesh.vertices[*i as usize])
                .collect::<Vec<_>>();
            let target = vertices
                .iter()
                .fold(egui::Vec2::ZERO, |p, v| p + v.pos.to_vec2())
                / 3.;
            let sampled = vertices
                .iter()
                .fold(egui::Vec2::ZERO, |p, v| p + v.uv.to_vec2())
                / 3.;
            let exact = sampling_uv(
                &g,
                [5184, 3456],
                Orientation::NoTransforms,
                [target.x as f64, 1. - target.y as f64],
            )
            .unwrap();
            let error = (sampled - exact.to_vec2()) * egui::vec2(5184., 3456.);
            assert!(error.length() < 0.25, "mesh error: {error:?}");
        }
        g.profile.mapping.center = [-20000., -20000.];
        assert!(
            unit_mesh(&g, [960, 640], Orientation::NoTransforms, 8)
                .indices
                .is_empty()
        );
    }
    #[test]
    fn successful_calibration_applies_immediately_and_capture_keeps_its_own_revision() {
        let mut w = Workspace::new();
        w.screen_test = Some(crate::screen_test::ui::Panel::preview(
            crate::screen_test::model::Workflow::Geometry,
        ));
        let store = w.geometry_store();
        let mut g = geometry([5208, 3476], [12., 8., 5184., 3456.]);
        store
            .lock()
            .unwrap()
            .publish_display(g.profile.clone(), g.screen_size, g.sensor_crop)
            .unwrap();
        w.live_view.turns = 3;
        w.geometry_enabled = false;
        w.sync_display_geometry();
        assert!(w.geometry_enabled);
        assert_eq!(w.live_view.turns, 0);
        assert_eq!(w.canvas_geometry().unwrap().screen_size, [1260, 2800]);
        w.records.push(CaptureRecord {
            id: "photo".into(),
            kind: CaptureKind::Photo,
            started_unix_ms: 1,
            camera: CameraInfo::default(),
            context: w.context(),
            files: vec![CapturedFile {
                remote: RemoteFile {
                    folder: "/".into(),
                    name: "a.jpg".into(),
                },
                path: "a.jpg".into(),
                bytes: 0,
                sha256: String::new(),
            }],
            pending_files: vec![],
            directory: PathBuf::new(),
        });
        g.profile.id = "geometry-b".into();
        store
            .lock()
            .unwrap()
            .publish_display(g.profile.clone(), g.screen_size, g.sensor_crop)
            .unwrap();
        w.sync_display_geometry();
        assert_eq!(w.canvas_geometry().unwrap().profile.id, "geometry-b");
        w.source = CanvasSource::Photo("a.jpg".into());
        assert_eq!(w.canvas_geometry().unwrap().profile.id, "geometry-a");
        w.source = CanvasSource::Photo("normalized-output.png".into());
        assert!(w.canvas_geometry().is_none());
        let mut reopened = Workspace::new();
        let mut panel =
            crate::screen_test::ui::Panel::preview(crate::screen_test::model::Workflow::Geometry);
        panel.controller.geometry = store;
        reopened.screen_test = Some(panel);
        reopened.sync_display_geometry();
        assert_eq!(reopened.canvas_geometry().unwrap().profile.id, "geometry-b");
        assert!(SessionGeometryStore::default().active_display().is_none());
    }
}
