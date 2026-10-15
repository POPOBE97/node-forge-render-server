use super::{
    geometry, measurement, model::*, pattern::Pattern, raw::RawFrame, repository::Repository,
};
use crate::pixel_perfecto::calibration::{Calibration, Correspondence, fit_correspondences};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
fn device(size: [u32; 2]) -> DisplayDevice {
    DisplayDevice {
        id: "iphone-test".into(),
        platform: Platform::Ios,
        name: "iPhone".into(),
        size,
        drawable: size,
        insets: [12, 24, 12, 60],
        modes: vec![
            DisplayMode::Srgb,
            DisplayMode::DisplayP3,
            DisplayMode::LinearHdr,
        ],
        brightness: 0.5,
        headroom: 4.,
        active: true,
        generation: "test".into(),
        brightness_guard: None,
    }
}
fn pattern() -> Pattern {
    Pattern::with_dictionary(
        &device([1080, 2400]),
        DisplayMode::Srgb,
        1.,
        &vec![vec![0; 64]; 250],
    )
    .unwrap()
}
fn raw_meta() -> RawMetadata {
    RawMetadata {
        width: 100,
        height: 120,
        cfa: [0, 1, 3, 2],
        black: [100.; 4],
        white: 16000.,
        exposure: 0.5,
        iso: 100.,
        aperture: 5.6,
        focal_length: 62.,
        camera: "650D".into(),
        lens: "test".into(),
        orientation: 0,
    }
}
fn profile() -> GeometryProfile {
    GeometryProfile {
        id: "g1".into(),
        revision: 0,
        created_ms: 0,
        source_capture: "fixture".into(),
        camera_key: raw_meta().camera_key(),
        raw_size: [100, 120],
        mapping: Calibration {
            h: [1.5, 0., -0.75, 0., 1.8, -0.9, 0., 0., 1.],
            lens: [0.; 4],
            center: [50., 60.],
        },
        holdout_rms: 0.1,
        marker_count: 20,
    }
}
pub(super) fn capture() -> StoredCapture {
    let p = pattern();
    let raw = raw_meta();
    let patches = p
        .manifest
        .patches
        .iter()
        .map(|p| PatchMeasurement {
            id: p.id,
            rgb: p.rgb.map(|v| v as f64 * 10000.),
            cfa_mean: [5000.; 4],
            stddev: [1.; 4],
            count: [100; 4],
            rejected_fraction: 0.,
            valid: true,
        })
        .collect();
    let geometry_verified = Some(GeometryEvidence {
        profile_id: profile().id,
        pattern_generation: p.manifest.generation.clone(),
        raw_sha256: String::new(),
        max_residual_px: 0.2,
        normalized_path: PathBuf::from("unit-test-normalized.exr"),
        normalized_sha256: String::new(),
    });
    StoredCapture {
        id: new_id("fixture"),
        created_ms: now_ms(),
        ticket: CaptureTicket {
            id: new_id("ticket"),
            workflow: Workflow::Reference,
            conditions: CaptureConditions {
                device: device([1080, 2400]),
                brightness: 0.5,
                batch: "a".into(),
                notes: String::new(),
                manifest: p.manifest,
                camera_key: raw.camera_key(),
            },
            geometry: Some(profile()),
            reference: None,
            thresholds: None,
        },
        camera_record: new_id("camera"),
        raw_path: PathBuf::new(),
        raw_sha256: String::new(),
        measurements: Some(Measurements {
            raw,
            patches,
            issues: vec![],
        }),
        analysis_issue: None,
        display_issue: None,
        artifacts: vec![PathBuf::from("unit-test-normalized.exr")],
        geometry_verified,
    }
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(new_id("screen-test"));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn persist(repo: &Repository, mut capture: StoredCapture) -> StoredCapture {
    let p = repo.root.join("original.CR2");
    std::fs::write(&p, b"II1234567890123456789").unwrap();
    let (path, hash) = repo.import_raw(&p).unwrap();
    capture.raw_path = path;
    capture.raw_sha256 = hash;
    let normalized = repo.root.join(format!("{}.exr", capture.id));
    std::fs::write(&normalized, b"test normalized artifact").unwrap();
    use sha2::{Digest, Sha256};
    let proof = capture.geometry_verified.as_mut().unwrap();
    proof.raw_sha256 = capture.raw_sha256.clone();
    proof.normalized_path = normalized.clone();
    proof.normalized_sha256 = format!("{:x}", Sha256::digest(std::fs::read(&normalized).unwrap()));
    capture.artifacts = vec![normalized];
    repo.insert_capture(&capture).unwrap();
    capture
}
#[test]
fn native_resolution_and_cutouts_are_explicit() {
    for size in [[1080, 2400], [1260, 2800], [1179, 2556], [1400, 900]] {
        let p = Pattern::with_dictionary(
            &device(size),
            DisplayMode::DisplayP3,
            1.,
            &vec![vec![0; 64]; 250],
        )
        .unwrap();
        assert_eq!(p.rgba.len(), (size[0] * size[1]) as usize);
        assert_eq!(p.manifest.patches.len(), 60);
        assert_eq!(p.manifest.markers.len(), 39);
        for m in p.manifest.markers {
            let [x, y, w, h] = m.rect;
            assert!(x >= 12 && y >= 24 && x + w <= size[0] - 12 && y + h <= size[1] - 60);
            assert_eq!(w % 8, 0);
        }
    }
    let mut d = device([1080, 2400]);
    d.drawable = [720, 1600];
    assert!(Pattern::with_dictionary(&d, DisplayMode::Srgb, 1., &vec![vec![0; 64]; 250]).is_err());
}
#[test]
fn hdr_pattern_preserves_above_white() {
    let p = Pattern::with_dictionary(
        &device([480, 800]),
        DisplayMode::LinearHdr,
        4.,
        &vec![vec![0; 64]; 250],
    )
    .unwrap();
    assert!(p.rgba.iter().any(|v| v[0] > 1.));
    assert_eq!(p.bytes().len(), 480 * 800 * 8);
}
#[test]
fn raw_black_subtraction_is_linear_and_unclamped() {
    let mut m = raw_meta();
    m.width = 2;
    m.height = 2;
    let header = serde_json::to_vec(&m).unwrap();
    let mut bytes = b"NFR1".to_vec();
    bytes.extend((header.len() as u32).to_le_bytes());
    bytes.extend(header);
    for v in [90u16, 200, 300, 16000] {
        bytes.extend(v.to_le_bytes());
    }
    let raw = RawFrame::from_bytes(&bytes).unwrap();
    assert_eq!(raw.sample(0, 0), (0, -10., false));
    assert_eq!(raw.sample(0, 1), (3, 200., false));
    assert!(raw.sample(1, 1).2);
    bytes.pop();
    assert!(RawFrame::from_bytes(&bytes).is_err());
}
#[test]
fn generic_geometry_has_no_grid_quantization() {
    let size = [1000, 800];
    let c = Calibration {
        h: [1.3, 0.04, -0.65, 0.02, 1.1, -0.55, 0.02, -0.03, 1.],
        lens: [0.04, -0.007, 0.001, -0.002],
        center: [500., 400.],
    };
    let mut points = vec![];
    for i in 0..8 {
        for j in 0..8 {
            let target = [i as f64 / 7., j as f64 / 7.];
            points.push(Correspondence {
                target,
                source: c.map(target, size).unwrap(),
                enabled: true,
            });
        }
    }
    let result = fit_correspondences(
        &points,
        &vec![1.; points.len()],
        size,
        &Calibration::initial(size),
        true,
    )
    .unwrap();
    assert!(result.rms < 0.01);
}
#[test]
fn failed_geometry_does_not_replace_session() {
    let mut store = SessionGeometryStore::default();
    let first = store.publish(profile()).unwrap();
    let mut invalid = profile();
    invalid.holdout_rms = 2.;
    assert!(store.publish(invalid).is_err());
    assert_eq!(store.active.as_ref().unwrap().revision, first.revision);
    let snapshot = store.active.clone().unwrap();
    store.publish(profile()).unwrap();
    assert_eq!(snapshot.revision, 1);
    assert_eq!(store.active.unwrap().revision, 2);
    assert!(SessionGeometryStore::default().active.is_none());
}
#[test]
fn window_controller_does_not_own_geometry_lifetime() {
    let root = Temp::new();
    let shared = Arc::new(Mutex::new(SessionGeometryStore::default()));
    shared.lock().unwrap().publish(profile()).unwrap();
    {
        let controller = super::controller::Controller::at(root.0.clone(), shared.clone());
        assert!(controller.geometry.lock().unwrap().active.is_some());
    }
    let reopened = super::controller::Controller::at(root.0.clone(), shared);
    assert_eq!(
        reopened
            .geometry
            .lock()
            .unwrap()
            .active
            .as_ref()
            .unwrap()
            .revision,
        1
    );
}
#[test]
fn matrix_recovers_linear_reference_without_white_balance() {
    let c = capture();
    let t = measurement::color_transform(
        c.measurements.as_ref().unwrap(),
        &c.ticket.conditions.manifest,
    )
    .unwrap();
    let out = measurement::apply(t.matrix, [5000., 2000., 12000.]);
    assert!((out[0] - 0.5).abs() < 1e-6);
    assert!((out[2] - 1.2).abs() < 1e-6);
}
#[test]
fn android_bias_is_preserved_and_no_threshold_means_no_pass() {
    let source = capture();
    let reference = measurement::reference("base".into(), &[source.clone()]).unwrap();
    let mut test = source;
    test.ticket.workflow = Workflow::Inspection;
    test.ticket.conditions.device.id = "android".into();
    test.ticket.conditions.device.platform = Platform::Android;
    for p in &mut test.measurements.as_mut().unwrap().patches {
        p.rgb = p.rgb.map(|v| v * 1.2);
    }
    let report = measurement::inspect(&test, &reference, None).unwrap();
    assert!((report.white_relative.unwrap() - 0.2).abs() < 1e-8);
    assert!(report.conclusion.contains("no pass/fail"));
    let flagged = measurement::inspect(
        &test,
        &reference,
        Some(Thresholds {
            rgb_fraction_of_white: 0.03,
            white_relative: 0.05,
        }),
    )
    .unwrap();
    assert!(flagged.differences.iter().any(|p| p.flagged));
    assert!(!flagged.issues.is_empty());
}
#[test]
fn different_brightness_or_batches_cannot_silently_merge() {
    let a = capture();
    let mut b = a.clone();
    b.id = new_id("fixture");
    b.ticket.conditions.brightness = 0.8;
    assert!(measurement::reference("bad".into(), &[a.clone(), b.clone()]).is_err());
    b.ticket.conditions.brightness = 0.5;
    b.ticket.conditions.batch = "b".into();
    assert!(measurement::reference("bad".into(), &[a, b]).is_err());
}
#[test]
fn exposure_brackets_only_merge_valid_linear_samples() {
    let a = capture();
    let mut b = a.clone();
    b.id = new_id("bracket");
    b.measurements.as_mut().unwrap().raw.exposure = 0.25;
    let mut a = a;
    a.measurements.as_mut().unwrap().patches[11].valid = false;
    b.measurements.as_mut().unwrap().patches[1].valid = false;
    let r = measurement::reference("bracket".into(), &[a, b]).unwrap();
    assert!(r.measurements.patches.iter().all(|p| p.valid));
    assert_eq!(r.measurements.patches[11].rgb, [10000.; 3]);
}
#[test]
fn database_survives_restart_and_old_report_keeps_baseline_version() {
    let temp = Temp::new();
    let mut repo = Repository::open(&temp.0).unwrap();
    let original = persist(&repo, capture());
    let reference = measurement::reference("old".into(), &[original.clone()]).unwrap();
    repo.insert_reference(&reference).unwrap();
    let mut test = original.clone();
    test.id = new_id("test");
    test.ticket.workflow = Workflow::Inspection;
    repo.insert_capture(&test).unwrap();
    let report = measurement::inspect(&test, &reference, None).unwrap();
    repo.insert_report(&report).unwrap();
    let newer = measurement::reference("new".into(), &[original]).unwrap();
    repo.insert_reference(&newer).unwrap();
    assert!(repo.insert_reference(&reference).is_err());
    drop(repo);
    let reopened = Repository::open(&temp.0).unwrap();
    assert_eq!(reopened.references().unwrap().len(), 2);
    assert_eq!(reopened.reports().unwrap()[0].reference, reference.id);
}
#[test]
fn missing_or_changed_raw_cannot_publish_a_baseline() {
    let temp = Temp::new();
    let mut repo = Repository::open(&temp.0).unwrap();
    let capture = persist(&repo, capture());
    let reference = measurement::reference("base".into(), &[capture.clone()]).unwrap();
    std::fs::write(repo.root.join(&capture.raw_path), b"corrupt").unwrap();
    assert!(repo.insert_reference(&reference).is_err());
    assert!(repo.references().unwrap().is_empty());
}
#[test]
fn stale_chart_identity_is_rejected() {
    let p = pattern();
    let error = geometry::correspondences(&p.manifest, &[], 100)
        .unwrap_err()
        .to_string();
    assert!(error.contains("No chart markers"));
    let mut detected: Vec<_> = p
        .manifest
        .markers
        .iter()
        .map(|m| geometry::DetectedMarker {
            id: m.id,
            corners: vec![[0., 0.]; 4],
        })
        .collect();
    let index = detected.iter().position(|m| m.id >= 128).unwrap();
    let original = detected.remove(index);
    let error = geometry::correspondences(&p.manifest, &detected, 100)
        .unwrap_err()
        .to_string();
    assert!(error.contains("identity incomplete: read 5 of 6"));
    // A readable, different nibble in the same generation slot really is an old/different chart.
    let mut wrong = original.clone();
    wrong.id = 128 + (original.id - 128) / 16 * 16 + (original.id + 1) % 16;
    detected.push(wrong);
    let error = geometry::correspondences(&p.manifest, &detected, 100)
        .unwrap_err()
        .to_string();
    assert!(error.contains("identity mismatch"));
    assert!(error.contains(&format!("expected marker {}", original.id)));
}

#[test]
fn detection_gain_ignores_hot_pixels_without_changing_raw_samples() {
    let mut metadata = raw_meta();
    metadata.width = 200;
    metadata.height = 200;
    metadata.black = [2047., 2047., 2048., 2048.];
    metadata.white = 13645.;
    let mut pixels = Vec::new();
    for y in 0..metadata.height {
        for x in 0..metadata.width {
            let c = metadata.cfa[((y % 2) * 2 + x % 2) as usize] as usize;
            let signal = if x < 100 { -12 } else { 400 };
            pixels.push((metadata.black[c] as i32 + signal) as u16);
        }
    }
    pixels[1] = 65000; // A hot green pixel must not set the normalization range.
    let raw = RawFrame { metadata, pixels };
    let before = raw.pixels.clone();
    let samples = [raw.sample(50, 50), raw.sample(150, 150)];
    let image = raw.detection_image();
    assert_eq!(image.dimensions(), (200, 200));
    assert_eq!(image.get_pixel(50, 50)[0], 0);
    assert_eq!(image.get_pixel(150, 150)[0], 255);
    assert_eq!(raw.pixels, before);
    assert_eq!([raw.sample(50, 50), raw.sample(150, 150)], samples);
    assert_eq!(samples[0].1, -12.);
}

#[test]
#[ignore = "requires built LibRaw/OpenCV helper; synthetic CFA only, no devices"]
fn native_dark_raw_chart_preserves_all_identity_markers() {
    let temp = Temp::new();
    let chart = Pattern::generate(&device([900, 1600]), DisplayMode::Srgb, 1.).unwrap();
    chart.save(&temp.0).unwrap();
    let portrait = image::open(temp.0.join("preview.png")).unwrap().to_luma8();
    let source = image::imageops::rotate90(&portrait);
    for signal in [128., 512., 8192.] {
        let mut metadata = raw_meta();
        metadata.width = source.width();
        metadata.height = source.height();
        metadata.black = [2047., 2047., 2048., 2048.];
        metadata.white = 13645.;
        metadata.orientation = 6;
        let mut pixels = Vec::new();
        for y in 0..metadata.height {
            for x in 0..metadata.width {
                let c = metadata.cfa[((y % 2) * 2 + x % 2) as usize] as usize;
                // Low-exposure chart plus dark surround and a few extreme reflection/hot pixels.
                let value = if x < 2 && y < 2 {
                    11000.
                } else {
                    source.get_pixel(x, y)[0] as f64 / 255. * signal
                };
                pixels.push((metadata.black[c] + value).round() as u16);
            }
        }
        let raw = RawFrame { metadata, pixels };
        let detected = geometry::detect(&raw, &temp.0).unwrap();
        assert_eq!(
            detected.len(),
            chart.manifest.markers.len(),
            "signal {signal}"
        );
        let profile = geometry::fit(&raw, &chart.manifest, &detected, "dark-raw")
            .unwrap_or_else(|e| panic!("signal {signal}: {e:#}"));
        assert!(profile.holdout_rms < 1.);
        if signal == 512. {
            let corrected =
                geometry::rectify_for_color(&raw, &profile, &chart.manifest, &temp.0).unwrap();
            assert!(temp.0.join("normalized.exr").is_file());
            let measured = measurement::measure(&corrected).unwrap();
            assert_eq!(measured.patches.len(), 60);
            assert_eq!(raw.metadata.exposure, measured.raw.exposure);
        }
        let other = Pattern::generate(&device([900, 1600]), DisplayMode::Srgb, 1.).unwrap();
        assert!(
            geometry::correspondences(&other.manifest, &detected, raw.metadata.height)
                .unwrap_err()
                .to_string()
                .contains("identity mismatch")
        );
    }
}

#[test]
#[ignore = "requires built LibRaw/OpenCV helper; no camera or phone access"]
fn native_chart_detection_roundtrip_and_geometry_holdout() {
    let temp = Temp::new();
    let chart = Pattern::generate(&device([1260, 2800]), DisplayMode::Srgb, 1.).unwrap();
    chart.save(&temp.0).unwrap();
    let bytes =
        super::raw::native(&["detect".as_ref(), temp.0.join("preview.png").as_os_str()]).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let detected: Vec<geometry::DetectedMarker> =
        serde_json::from_value(json["markers"].clone()).unwrap();
    assert_eq!(detected.len(), chart.manifest.markers.len());
    let mut metadata = raw_meta();
    metadata.width = 1260;
    metadata.height = 2800;
    let raw = RawFrame {
        metadata,
        pixels: Vec::new(),
    };
    let fitted = geometry::fit(&raw, &chart.manifest, &detected, "roundtrip").unwrap();
    assert!(fitted.holdout_rms < 1.);
    geometry::validate_snapshot(&fitted, &raw, &chart.manifest, &detected).unwrap();
    let cropped: Vec<_> = detected
        .iter()
        .filter(|m| m.corners.iter().all(|p| p[1] < 1960.))
        .cloned()
        .collect();
    assert!(geometry::correspondences(&chart.manifest, &cropped, 2800).is_err());
    let perimeter: Vec<_> = detected
        .iter()
        .filter(|m| !(100..=102).contains(&m.id))
        .cloned()
        .collect();
    assert!(geometry::correspondences(&chart.manifest, &perimeter, 2800).is_err());
    let other = Pattern::generate(&device([1260, 2800]), DisplayMode::Srgb, 1.).unwrap();
    assert!(geometry::validate_snapshot(&fitted, &raw, &other.manifest, &detected).is_err());
}

#[test]
fn inspection_exposure_merge_keeps_source_ids_and_bias() {
    let source = capture();
    let reference = measurement::reference("base".into(), &[source.clone()]).unwrap();
    let mut a = source;
    a.ticket.workflow = Workflow::Inspection;
    a.ticket.reference = Some(reference.id.clone());
    for p in &mut a.measurements.as_mut().unwrap().patches {
        p.rgb = p.rgb.map(|v| v * 1.2);
    }
    let mut b = a.clone();
    b.id = new_id("short-exposure");
    a.measurements.as_mut().unwrap().patches[11].valid = false;
    b.measurements.as_mut().unwrap().patches[1].valid = false;
    let report = measurement::inspect_exposures(&[a.clone(), b.clone()], &reference, None).unwrap();
    assert_eq!(report.source_captures, [a.id, b.id]);
    assert!((report.white_relative.unwrap() - 0.2).abs() < 1e-6);
    assert_eq!(report.differences.len(), 60);
}

#[test]
fn row_quality_rejects_bands_but_preserves_smooth_shading() {
    let smooth: Vec<_> = (0..30)
        .map(|i| (i as f64, 1000. + i as f64 * 4., 100))
        .collect();
    assert!(!measurement::row_banding(&smooth, 1060., 12.));
    let bands: Vec<_> = smooth
        .iter()
        .enumerate()
        .map(|(i, (x, y, n))| (*x, *y + if i % 4 < 2 { 100. } else { -100. }, *n))
        .collect();
    assert!(measurement::row_banding(&bands, 1060., 12.));
}

#[test]
fn floating_image_bracket_uses_unclipped_exposure_without_gain_fitting() {
    let mut long = RawFrame {
        metadata: raw_meta(),
        pixels: vec![16000; 100 * 120],
    };
    long.metadata.exposure = 0.5;
    let mut short = long.clone();
    short.metadata.exposure = 0.25;
    short.pixels.fill(1100);
    let geometry = profile();
    let (rgb, clipped) =
        measurement::fused_sample(&[(&long, &geometry), (&short, &geometry)], [0.5, 0.5]).unwrap();
    assert!(!clipped);
    assert!(rgb.iter().all(|v| (*v - 4000.).abs() < 1e-8));
    long.pixels.fill(2100);
    let (rgb, _) =
        measurement::fused_sample(&[(&long, &geometry), (&short, &geometry)], [0.5, 0.5]).unwrap();
    assert!(rgb.iter().all(|v| (*v - 4000.).abs() < 1e-8));
    short.pixels.fill(90);
    let (rgb, _) = measurement::fused_sample(&[(&short, &geometry)], [0.5, 0.5]).unwrap();
    assert!(rgb.iter().all(|v| *v < 0.));
}

fn rotated_screen_mapping(size: [u32; 2], degrees: f64, lens: bool) -> Calibration {
    let (sin, cos) = degrees.to_radians().sin_cos();
    Calibration {
        h: crate::pixel_perfecto::calibration::mat_mul(
            [cos, -sin, 0., sin, cos, 0., 0., 0., 1.],
            [0.62, 0.02, -0.31, 0.015, 1.20, -0.60, 0.035, -0.025, 1.],
        ),
        lens: if lens {
            [0.035, -0.012, 0.0007, -0.001]
        } else {
            [0.; 4]
        },
        center: [size[0] as f64 / 2., size[1] as f64 / 2.],
    }
}
#[test]
fn geometry_is_rotation_invariant_with_perspective_and_distortion_in_sensor_coordinates() {
    let chart = pattern();
    let size = [3600, 2600];
    for angle in [0., 17., 45., 90., 137., 180., 231., 270., 315., 359.] {
        let truth = rotated_screen_mapping(size, angle, true);
        let detected = chart
            .manifest
            .markers
            .iter()
            .map(|marker| {
                let [x, y, w, h] = marker.rect;
                geometry::DetectedMarker {
                    id: marker.id,
                    corners: [
                        [x, y + h - 1],
                        [x + w - 1, y + h - 1],
                        [x + w - 1, y],
                        [x, y],
                    ]
                    .into_iter()
                    .map(|p| {
                        let q = truth
                            .map(
                                [
                                    p[0] as f64 / (chart.manifest.size[0] - 1) as f64,
                                    p[1] as f64 / (chart.manifest.size[1] - 1) as f64,
                                ],
                                size,
                            )
                            .unwrap();
                        [q[0], size[1] as f64 - 1. - q[1]]
                    })
                    .collect(),
                }
            })
            .rev()
            .collect::<Vec<_>>(); // Neither marker list nor photo corners may be spatially sorted.
        for orientation in [0, 3, 5, 6] {
            let mut metadata = raw_meta();
            metadata.width = size[0];
            metadata.height = size[1];
            metadata.orientation = orientation;
            let raw = RawFrame {
                metadata,
                pixels: vec![],
            };
            let fitted = geometry::fit(&raw, &chart.manifest, &detected, "rotated")
                .unwrap_or_else(|e| panic!("{angle} deg / EXIF {orientation}: {e:#}"));
            assert!(fitted.holdout_rms < 0.01);
            for uv in [[0., 0.], [1., 0.], [0., 1.], [1., 1.], [0.3, 0.7]] {
                let a = fitted.mapping.map(uv, size).unwrap();
                let b = truth.map(uv, size).unwrap();
                assert!(
                    (a[0] - b[0]).hypot(a[1] - b[1]) < 0.1,
                    "orientation at {angle}"
                );
            }
        }
    }
}
#[test]
#[ignore = "requires built OpenCV helper; synthetic images only, no devices"]
fn native_rotated_and_tilted_chart_keeps_canonical_marker_corners() {
    let temp = Temp::new();
    let chart = Pattern::generate(&device([900, 1600]), DisplayMode::Srgb, 1.).unwrap();
    chart.save(&temp.0).unwrap();
    let source = image::open(temp.0.join("preview.png")).unwrap().to_luma8();
    let size = [1800, 1400];
    for angle in [0., 90., 180., 270., 37., -23.] {
        let truth = rotated_screen_mapping(size, angle, false);
        let warped = image::GrayImage::from_fn(size[0], size[1], |x, y| {
            let Some(uv) = truth.unmap([x as f64, (size[1] - 1 - y) as f64], size) else {
                return image::Luma([32]);
            };
            if uv.iter().any(|v| !(0.0..=1.0).contains(v)) {
                return image::Luma([32]);
            }
            let sx = uv[0] * (source.width() - 1) as f64;
            let sy = (1. - uv[1]) * (source.height() - 1) as f64;
            let (ix, iy) = (sx.floor() as u32, sy.floor() as u32);
            let (dx, dy) = (sx - ix as f64, sy - iy as f64);
            let sample = |xx: u32, yy: u32| {
                source.get_pixel(xx.min(source.width() - 1), yy.min(source.height() - 1))[0] as f64
            };
            image::Luma([
                ((1. - dy) * ((1. - dx) * sample(ix, iy) + dx * sample(ix + 1, iy))
                    + dy * ((1. - dx) * sample(ix, iy + 1) + dx * sample(ix + 1, iy + 1)))
                .round() as u8,
            ])
        });
        let path = temp.0.join(format!("rotated-{angle}.png"));
        warped.save(&path).unwrap();
        let bytes = super::raw::native(&["detect".as_ref(), path.as_os_str()]).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let detected: Vec<geometry::DetectedMarker> =
            serde_json::from_value(json["markers"].clone()).unwrap();
        let mut metadata = raw_meta();
        metadata.width = size[0];
        metadata.height = size[1];
        let raw = RawFrame {
            metadata,
            pixels: vec![],
        };
        let fitted = geometry::fit(&raw, &chart.manifest, &detected, "native-rotated")
            .unwrap_or_else(|e| panic!("{angle} deg ({} markers): {e:#}", detected.len()));
        for uv in [[0.1, 0.25], [0.9, 0.25], [0.1, 0.9], [0.9, 0.9]] {
            let a = fitted.mapping.map(uv, size).unwrap();
            let b = truth.map(uv, size).unwrap();
            assert!(
                (a[0] - b[0]).hypot(a[1] - b[1]) < 2.,
                "native orientation at {angle}: {a:?} vs {b:?}"
            );
        }
    }
}
#[test]
fn geometry_cannot_be_inserted_into_the_color_database() {
    let temp = Temp::new();
    let repo = Repository::open(&temp.0).unwrap();
    let mut geometry = capture();
    geometry.ticket.workflow = Workflow::Geometry;
    let error = repo.insert_capture(&geometry).unwrap_err();
    assert!(error.to_string().contains("current session"));
    assert!(repo.captures().unwrap().is_empty());
}

#[test]
fn color_measurement_publication_requires_matching_geometry_evidence() {
    let valid = capture();
    for case in 0..7 {
        let mut c = valid.clone();
        match case {
            0 => c.geometry_verified = None,
            1 => c.geometry_verified.as_mut().unwrap().pattern_generation = "old-pattern".into(),
            2 => c.geometry_verified.as_mut().unwrap().raw_sha256 = "different-raw".into(),
            3 => c.geometry_verified.as_mut().unwrap().profile_id = "other-placement".into(),
            4 => c.geometry_verified.as_mut().unwrap().max_residual_px = 4.,
            5 => c.geometry_verified.as_mut().unwrap().max_residual_px = f64::NAN,
            _ => c.artifacts.clear(),
        }
        assert!(
            measurement::reference("must reject".into(), &[c]).is_err(),
            "case {case}"
        );
    }
    let baseline = measurement::reference("valid".into(), &[valid.clone()]).unwrap();
    let mut test = valid;
    test.geometry_verified = None;
    assert!(measurement::inspect(&test, &baseline, None).is_err());
}

#[test]
fn normalized_artifact_integrity_is_checked_at_publication_and_loading() {
    let temp = Temp::new();
    let mut repo = Repository::open(&temp.0).unwrap();
    let capture = persist(&repo, capture());
    let baseline = measurement::reference("base".into(), &[capture.clone()]).unwrap();
    repo.insert_reference(&baseline).unwrap();
    let path = &capture.geometry_verified.as_ref().unwrap().normalized_path;
    std::fs::write(path, b"changed image").unwrap();
    assert!(repo.reference(&baseline.id).is_err());
    let next = measurement::reference("next".into(), &[capture]).unwrap();
    assert!(repo.insert_reference(&next).is_err());
    assert_eq!(repo.references().unwrap().len(), 1);
}

#[test]
fn sweep_progress_survives_restart_without_turning_failures_into_baselines() {
    let temp = Temp::new();
    let repo = Repository::open(&temp.0).unwrap();
    let mut run = super::batch::SweepRun::planned();
    repo.save_sweep(&run).unwrap();
    run.cells[0].state = super::batch::CellState::Failed;
    run.cells[0].issue = Some("No matching geometry".into());
    repo.save_sweep(&run).unwrap();
    drop(repo);
    let repo = Repository::open(&temp.0).unwrap();
    let runs = repo.sweeps().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].cells.len(), 33);
    assert_eq!(runs[0].saved(), 0);
    assert_eq!(
        runs[0].cells[0].issue.as_deref(),
        Some("No matching geometry")
    );
    assert!(repo.references().unwrap().is_empty());
}
