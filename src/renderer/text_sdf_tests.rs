use super::*;

fn request(text: &str) -> TextRequest {
    TextRequest {
        text: text.into(),
        font_size: 64.0,
        weight: 0,
        line_height: 80.0,
        letter_spacing: 0.0,
        size: None,
        align: "left".into(),
        vertical_align: "top".into(),
        wrap: "none".into(),
        hinting: false,
        density: 1.0,
        phase: [0.0; 2],
        measured_frame: None,
    }
}

fn fonts() -> Vec<FontSource> {
    vec![FontSource {
        bytes: include_bytes!("../../assets/fonts/GeistMono/GeistMono-Regular.ttf").to_vec(),
        face_index: 0,
    }]
}

fn record(image: &Rgba32FImage, index: u32) -> [f32; 4] {
    image
        .get_pixel(index % image.width(), index / image.width())
        .0
}

fn median(v: [f32; 4]) -> f32 {
    v[0].min(v[1]).max(v[0].max(v[1]).min(v[2]))
}

fn sample_tile(tile: &GlyphTile, x: f32, y: f32) -> [f32; 4] {
    Bitmap::<4>::from_data(
        tile.image.width(),
        tile.image.height(),
        tile.image.as_raw().clone(),
    )
    .sample(
        f64::from(x - tile.origin[0]),
        f64::from(tile.image.height() as f32 - (y - tile.origin[1])),
    )
}

// Reference reader for the private atlas ABI, including arbitrary shifted LocalPx samples.
fn sample_layout(image: &Rgba32FImage, point: [f32; 2]) -> [f32; 2] {
    let header = record(image, 0);
    let p = [
        point[0].clamp(0.0, header[1]),
        point[1].clamp(0.0, header[2]),
    ];
    let mut result = [1e20_f32; 2];
    for i in 0..header[0] as u32 {
        let plane = record(image, 1 + 2 * i);
        let tile = record(image, 2 + 2 * i);
        let scale = (plane[2] - plane[0]) / tile[2];
        let raster = [
            (p[0] - plane[0]) / scale - 0.5,
            (plane[3] - p[1]) / scale - 0.5,
        ];
        let b = [
            raster[0].clamp(0.0, tile[2] - 1.0),
            raster[1].clamp(0.0, tile[3] - 1.0),
        ];
        let lo = [b[0].floor(), b[1].floor()];
        let hi = [
            (lo[0] + 1.0).min(tile[2] - 1.0),
            (lo[1] + 1.0).min(tile[3] - 1.0),
        ];
        let at = |x: f32, y: f32| {
            image
                .get_pixel((tile[0] + x) as u32, (tile[1] + y) as u32)
                .0
        };
        let a = at(lo[0], lo[1]);
        let c = at(hi[0], lo[1]);
        let d = at(lo[0], hi[1]);
        let e = at(hi[0], hi[1]);
        let v = std::array::from_fn(|j| {
            let top = a[j] + (c[j] - a[j]) * (b[0] - lo[0]);
            let bottom = d[j] + (e[j] - d[j]) * (b[0] - lo[0]);
            (top + (bottom - top) * (b[1] - lo[1])) * scale
        });
        let outside = (raster[0] - b[0]).hypot(raster[1] - b[1]) * scale;
        for (j, value) in [v[3], median(v)].into_iter().enumerate() {
            result[j] = result[j].min(if outside > 0.0 {
                value.max(0.0) + outside
            } else {
                value
            });
        }
    }
    let outside = (point[0] - p[0]).hypot(point[1] - p[1]);
    if outside > 0.0 {
        result = result.map(|v| v.max(0.0) + outside);
    }
    let dx = (point[0] - header[1] * 0.5).abs() - header[1] * 0.5;
    let dy = (point[1] - header[2] * 0.5).abs() - header[2] * 0.5;
    let clip_distance = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0);
    result.map(|v| v.max(clip_distance))
}

#[test]
fn mtsdf_preserves_sharp_corners_and_true_euclidean_distance() {
    let commands = [
        Command::MoveTo((0.2, 0.3).into()),
        Command::LineTo((30.2, 0.3).into()),
        Command::LineTo((30.2, 40.3).into()),
        Command::LineTo((0.2, 40.3).into()),
        Command::Close,
    ];
    let tile = glyph_tile(
        Sha256::digest(b"test rectangle 0.2 0.3 30.2 40.3").into(),
        &commands,
    )
    .unwrap();
    assert!(tile.image.pixels().any(|v| (v[0] - v[1]).abs() > 1.0));
    for (x, y, value) in tile.image.enumerate_pixels() {
        let px = tile.origin[0] + x as f32 + 0.5;
        let py = tile.origin[1] + tile.image.height() as f32 - y as f32 - 0.5;
        let dx = (px - 15.2).abs() - 15.0;
        let dy = (py - 20.3).abs() - 20.0;
        let exact = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0);
        assert!(
            (value[3] - exact).abs() < 0.001,
            "({px},{py}): {} vs {exact}",
            value[3]
        );
    }
    // Bilinear scalar SDF rounds an off-grid corner. RGB median retains the intersection.
    let corner = sample_tile(&tile, 30.15, 40.25);
    assert!(median(corner) < 0.0, "inside sharp corner: {corner:?}");
    assert!((median(corner) + 0.05).abs() < 0.01);
    assert!(
        corner[3] > median(corner) + 0.04,
        "true and perpendicular distances must differ"
    );
}

#[test]
fn glyph_counters_and_multiline_y_up_layout_survive() {
    let req = request("O\nO");
    let source = fonts();
    let layout = layout_outlines(&req, &source, EM_SIZE).unwrap();
    let tile = glyph_tile(layout.glyphs[0].key, &layout.glyphs[0].commands).unwrap();
    let [left, bottom, right, top] = tile.bounds;
    assert!(sample_tile(&tile, (left + right) * 0.5, (bottom + top) * 0.5)[3] > 0.0);
    assert!(sample_tile(&tile, left + 2.0, (bottom + top) * 0.5)[3] < 0.0);
    let atlas = prepare(&req, &source).unwrap();
    let image = atlas.as_rgba32f().unwrap();
    assert_eq!(record(image, 0)[0], 2.0);
    let first = record(image, 1);
    let second = record(image, 3);
    assert!((first[1] - second[1] - req.line_height).abs() < 0.001);
    assert_eq!(
        record(image, 2),
        record(image, 4),
        "repeated glyph shares one tile"
    );
    for plane in [first, second] {
        let p = [
            plane[0] + left + 2.0 - tile.origin[0],
            plane[1] + (bottom + top) * 0.5 - tile.origin[1],
        ];
        assert!(sample_layout(image, p)[0] < 0.0);
    }
}

#[test]
fn dpi_changes_only_instances_and_preserves_cached_glyphs() {
    let source = fonts();
    let mut req = request("9\n8\n7\n6\n5\n4\n3\n2\n1\n0");
    req.font_size = 40.0;
    req.line_height = 75.0;
    req.size = Some([50.0, 750.0]);
    let original = prepare(&req, &source).unwrap();
    let original_image = original.as_rgba32f().unwrap();
    let shaped = layout_outlines(&req, &source, EM_SIZE).unwrap();
    let first = glyph_tile(shaped.glyphs[0].key, &shaped.glyphs[0].commands).unwrap();
    for dpi in [3.0, 6.0, 12.0] {
        let mut scaled = req.clone();
        scaled.font_size *= dpi;
        scaled.line_height *= dpi;
        scaled.size = req.size.map(|s| s.map(|v| v * dpi));
        let atlas = prepare(&scaled, &source).unwrap();
        let image = atlas.as_rgba32f().unwrap();
        assert_eq!(image.dimensions(), original_image.dimensions());
        assert!(u64::from(image.width()) * u64::from(image.height()) < 128 * 1024);
        let metadata_end = record(image, 0)[3] as usize * image.width() as usize * 4;
        assert_eq!(
            &image.as_raw()[metadata_end..],
            &original_image.as_raw()[metadata_end..]
        );
        for i in 0..10 {
            let plane = record(image, 1 + 2 * i);
            let base = record(original_image, 1 + 2 * i);
            for c in 0..4 {
                assert!((plane[c] - base[c] * dpi).abs() < 0.01);
            }
        }
        let changed = layout_outlines(&scaled, &source, EM_SIZE).unwrap();
        let reused = glyph_tile(changed.glyphs[0].key, &changed.glyphs[0].commands).unwrap();
        assert!(Arc::ptr_eq(&first, &reused));
        for p in [[20.0, 20.0], [10.0, 55.0], [28.0, 200.0], [48.0, 749.0]] {
            let a = sample_layout(original_image, p);
            let b = sample_layout(image, p.map(|v| v * dpi));
            for c in 0..2 {
                assert!((a[c] * dpi - b[c]).abs() < 0.02);
            }
        }
    }
}

#[test]
fn empty_text_and_whitespace_have_no_ink() {
    for text in ["", " \n\t "] {
        let atlas = prepare(&request(text), &fonts()).unwrap();
        let image = atlas.as_rgba32f().unwrap();
        assert_eq!(record(image, 0)[0], 0.0);
        assert!(
            sample_layout(image, [0.0, 0.0])
                .iter()
                .all(|v| v.is_finite() && *v > 0.0)
        );
    }
}

#[test]
fn clipped_ink_has_a_continuous_distance_at_the_layout_edge() {
    let mut req = request("I");
    req.size = Some([20.0, 80.0]);
    let atlas = prepare(&req, &fonts()).unwrap();
    let image = atlas.as_rgba32f().unwrap();
    let y = (1..80).map(|y| y as f32).find(|&y| {
        sample_layout(image, [19.5, y])[0] < -0.49
    }).expect("frame must cut through the glyph body");
    for (x, expected) in [(19.9, -0.1), (20.0, 0.0), (20.1, 0.1)] {
        for distance in sample_layout(image, [x, y]) {
            assert!((distance - expected).abs() < 0.001, "x={x}: {distance}");
        }
    }
}

#[test]
fn reconstructed_coverage_matches_independent_swash_rasterization() {
    let mut req = request("Ag O5\njÁ");
    req.font_size = 96.0;
    req.line_height = 125.0;
    req.size = Some([420.0, 270.0]);
    req.align = "center".into();
    let source = fonts();
    let atlas = prepare(&req, &source).unwrap();
    let atlas = atlas.as_rgba32f().unwrap();
    // Supersample the independent raster reference so its quantized subpixel phase
    // cannot dominate this comparison with continuously positioned vector outlines.
    let mut raster_request = req.clone();
    raster_request.density = 8.0;
    let raster = super::super::text::prepare_text(&raster_request, &source, 4096).unwrap();
    let vertices: &[f32] = bytemuck::cast_slice(&raster.vertices);
    let raster_atlas = raster.atlas.as_rgba8().unwrap();
    let mut disagreement = 0;
    let mut max_disagreement_distance = 0.0_f32;
    let mut ink = 0;
    let mut preview = image::RgbaImage::new(420, 270);
    for y in 0..270 {
        for x in 0..420 {
            let point = [x as f32 + 0.5, 270.0 - y as f32 - 0.5];
            let mut expected = false;
            for quad in vertices.chunks_exact(30) {
                let left = quad[0] + 210.0;
                let bottom = quad[1] + 135.0;
                let right = quad[5] + 210.0;
                let top = quad[11] + 135.0;
                if point[0] >= left && point[0] < right && point[1] >= bottom && point[1] < top {
                    let u = quad[3] + (quad[8] - quad[3]) * (point[0] - left) / (right - left);
                    let v = quad[14] + (quad[4] - quad[14]) * (top - point[1]) / (top - bottom);
                    let a = raster_atlas.get_pixel((u * raster_atlas.width() as f32) as u32,
                        (v * raster_atlas.height() as f32) as u32)[3];
                    expected |= a >= 128;
                }
            }
            let distance = sample_layout(atlas, point)[1];
            let actual = distance <= 0.0;
            ink += usize::from(expected);
            disagreement += usize::from(expected != actual);
            if expected != actual {
                max_disagreement_distance = max_disagreement_distance.max(distance.abs());
            }
            let value = ((0.5 - distance).clamp(0.0, 1.0) * 255.0).round() as u8;
            preview.put_pixel(x, y, Rgba([value, value, value, 255]));
        }
    }
    if let Some(path) = std::env::var_os("NF_TEXT_SDF_PREVIEW") {
        preview.save(path).unwrap();
    }
    assert!(ink > 5000);
    // Swash quantizes and rasterizes a phase-shifted outline. At 8x the remaining
    // sampling/phase uncertainty is below 0.25 LocalPx; allow 0.10 for the MTSDF
    // curve interpolation. All interior pixels beyond that edge band must agree.
    assert!(max_disagreement_distance < 0.35,
        "MTSDF/reference mismatch: {disagreement} / {ink} pixels; furthest {max_disagreement_distance} px from edge");
}

#[test]
fn layout_cache_and_font_identity_are_distinct() {
    let mut req = request("A");
    let source = fonts();
    let a = prepare(&req, &source).unwrap();
    assert!(Arc::ptr_eq(&a, &prepare(&req, &source).unwrap()));
    req.letter_spacing = 3.0;
    assert!(!Arc::ptr_eq(&a, &prepare(&req, &source).unwrap()));
    req.text = "B".into();
    assert!(!Arc::ptr_eq(&a, &prepare(&req, &source).unwrap()));
    let another = vec![FontSource {
        bytes: include_bytes!("../../tests/fixtures/fonts/Michroma-Regular.ttf").to_vec(),
        face_index: 0,
    }];
    let ga = layout_outlines(&req, &source, EM_SIZE).unwrap();
    let gb = layout_outlines(&req, &another, EM_SIZE).unwrap();
    assert_ne!(ga.glyphs[0].key, gb.glyphs[0].key);
    assert_ne!(
        prepare(&req, &source).unwrap().as_bytes(),
        prepare(&req, &another).unwrap().as_bytes()
    );
    req.weight = 999;
    assert!(prepare(&req, &source).is_err());
}

#[test]
fn fallback_auto_measurement_and_explicit_frame_remain_valid() {
    let mut req = request("jÁgyp 中文");
    let mut source = fonts();
    assert!(prepare(&req, &source).is_err());
    source.push(FontSource {
        bytes: include_bytes!("../../assets/fonts/MiSans/MiSans-Regular.ttf").to_vec(),
        face_index: 0,
    });
    for align in ["left", "center", "right"] {
        req.align = align.into();
        let image = prepare(&req, &source).unwrap();
        let image = image.as_rgba32f().unwrap();
        let header = record(image, 0);
        assert_eq!(header[0], 7.0);
        assert!(header[1] > 100.0 && header[2] >= 80.0);
        assert!(image.as_raw().iter().all(|v| v.is_finite()));
        for i in 0..header[0] as u32 {
            let tile = record(image, 2 + i * 2);
            assert!(tile[1] > header[3]);
            assert!(tile[0] + tile[2] < image.width() as f32);
            assert!(tile[1] + tile[3] < image.height() as f32);
        }
    }
    req.size = Some([10.0, 20.0]);
    let atlas = prepare(&req, &source).unwrap();
    let image = atlas.as_rgba32f().unwrap();
    assert_eq!(&record(image, 0)[1..3], &[10.0, 20.0]);
    for p in [[-1.0, 10.0], [11.0, 10.0], [5.0, -1.0], [5.0, 21.0]] {
        assert!(sample_layout(image, p).iter().all(|v| *v > 0.0));
    }
}
