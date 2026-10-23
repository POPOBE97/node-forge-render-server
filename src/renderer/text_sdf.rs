//! Text outlines are shaped/rasterized by the same font pipeline as TextPass, then
//! converted to a signed Euclidean distance field (negative inside, LocalPx units).
use super::text::{FontSource, TextMesh, TextRequest, prepare_text};
use anyhow::{Result, anyhow, ensure};
use image::{DynamicImage, Rgba, Rgba32FImage};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

const DENSITY: f32 = 2.0;
const BUDGET: usize = 64 * 1024 * 1024;
type Cache = VecDeque<(Arc<TextMesh>, Arc<DynamicImage>, usize)>;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

pub(crate) fn prepare(request: &TextRequest, fonts: &[FontSource]) -> Result<Arc<DynamicImage>> {
    let mut request = request.clone();
    request.density = DENSITY;
    let mesh = prepare_text(&request, fonts, 8192)?;
    let cache = CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
    {
        let mut cache = cache
            .lock()
            .map_err(|_| anyhow!("text SDF cache poisoned"))?;
        if let Some(i) = cache.iter().position(|entry| Arc::ptr_eq(&entry.0, &mesh)) {
            let entry = cache.remove(i).expect("cache entry exists");
            let image = entry.1.clone();
            cache.push_back(entry);
            return Ok(image);
        }
    }
    let [width, height] = mesh.size.map(|x| (x * DENSITY).ceil() as u32);
    ensure!(
        width > 0
            && height > 0
            && width <= 8192
            && height <= 8192
            && u64::from(width) * u64::from(height) <= 4 * 1024 * 1024,
        "TextSdf layout exceeds 4 megapixels at 2x density"
    );
    let atlas = mesh
        .atlas
        .as_rgba8()
        .ok_or_else(|| anyhow!("expected glyph coverage atlas"))?;
    let mut mask = vec![0.0_f64; (width * height) as usize];
    // TextMesh quads are centered Y-up. Compose into a private top-down raster.
    let vertices: Vec<f32> = mesh
        .vertices
        .chunks_exact(4)
        .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    for quad in vertices.chunks_exact(30) {
        let left = ((quad[0] + mesh.size[0] * 0.5) * DENSITY).round() as i32;
        let top = ((mesh.size[1] * 0.5 - quad[11]) * DENSITY).round() as i32;
        let ax = (quad[3] * atlas.width() as f32).round() as u32;
        let ay = (quad[14] * atlas.height() as f32).round() as u32;
        let w = ((quad[5] - quad[0]) * DENSITY).round().max(0.0) as u32;
        let h = ((quad[11] - quad[1]) * DENSITY).round().max(0.0) as u32;
        for y in 0..h {
            for x in 0..w {
                let (tx, ty) = (left + x as i32, top + y as i32);
                if tx < 0 || ty < 0 || tx >= width as i32 || ty >= height as i32 {
                    continue;
                }
                let alpha = f64::from(atlas.get_pixel(ax + x, ay + y)[3]) / 255.0;
                let dst = &mut mask[(ty as u32 * width + tx as u32) as usize];
                *dst = alpha + *dst * (1.0 - alpha);
            }
        }
    }
    let field = signed_distance(&mask, width as usize, height as usize);
    let mut image = Rgba32FImage::new(width, height);
    for (pixel, d) in image.pixels_mut().zip(field) {
        // Metadata keeps auto-sized and fractional layouts exact without GPU uniforms.
        *pixel = Rgba([d / DENSITY, mesh.size[0], mesh.size[1], DENSITY]);
    }
    let image = Arc::new(DynamicImage::ImageRgba32F(image));
    let bytes = image.as_bytes().len() + mesh.atlas.as_bytes().len() + mesh.vertices.len();
    if bytes <= BUDGET {
        let mut cache = cache
            .lock()
            .map_err(|_| anyhow!("text SDF cache poisoned"))?;
        while cache.iter().map(|entry| entry.2).sum::<usize>() + bytes > BUDGET {
            cache.pop_front();
        }
        cache.push_back((mesh, image.clone(), bytes));
    }
    Ok(image)
}

fn signed_distance(alpha: &[f64], width: usize, height: usize) -> Vec<f32> {
    const FAR: f64 = 1e12;
    let mut outside: Vec<f64> = alpha
        .iter()
        .map(|&a| {
            if a == 0.0 {
                FAR
            } else {
                (0.5 - a).max(0.0).powi(2)
            }
        })
        .collect();
    let mut inside: Vec<f64> = alpha
        .iter()
        .map(|&a| {
            if a == 1.0 {
                FAR
            } else {
                (a - 0.5).max(0.0).powi(2)
            }
        })
        .collect();
    edt(&mut outside, width, height);
    edt(&mut inside, width, height);
    outside
        .iter()
        .zip(inside)
        .map(|(a, b)| (a.sqrt() - b.sqrt()) as f32)
        .collect()
}

// Exact separable squared Euclidean transform, O(width * height).
fn edt(grid: &mut [f64], width: usize, height: usize) {
    let n = width.max(height);
    let mut f = vec![0.0; n];
    let mut d = vec![0.0; n];
    let mut v = vec![0; n];
    let mut z = vec![0.0; n + 1];
    for x in 0..width {
        for y in 0..height {
            f[y] = grid[y * width + x];
        }
        edt_line(&f[..height], &mut d, &mut v, &mut z);
        for y in 0..height {
            grid[y * width + x] = d[y];
        }
    }
    for row in grid.chunks_exact_mut(width) {
        f[..width].copy_from_slice(row);
        edt_line(&f[..width], &mut d, &mut v, &mut z);
        row.copy_from_slice(&d[..width]);
    }
}

fn edt_line(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let mut k = 0;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..f.len() {
        let mut s;
        loop {
            let p = v[k];
            s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * (q - p) as f64);
            if s > z[k] {
                break;
            }
            k -= 1;
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for q in 0..f.len() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        d[q] = (q as f64 - v[k] as f64).powi(2) + f[v[k]];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn font_layout_cache_preserves_counters_and_invalidates_on_edits() {
        let fonts = vec![FontSource {
            bytes: include_bytes!("../../assets/fonts/GeistMono/GeistMono-Regular.ttf").to_vec(),
            face_index: 0,
        }];
        let mut request = TextRequest {
            text: "O".into(),
            font_size: 80.0,
            weight: 0,
            line_height: 96.0,
            letter_spacing: 0.0,
            size: Some([100.0, 100.0]),
            align: "center".into(),
            vertical_align: "center".into(),
            wrap: "none".into(),
            hinting: false,
            density: 1.0,
            phase: [0.0; 2],
            measured_frame: None,
        };
        let a = prepare(&request, &fonts).unwrap();
        assert!(Arc::ptr_eq(&a, &prepare(&request, &fonts).unwrap()));
        let image = a.as_rgba32f().unwrap();
        assert!(image.pixels().all(|p| p[0].is_finite()));
        // An O has two disjoint interior intervals on a middle scanline.
        assert!((0..image.height()).any(|y| {
            let mut starts = 0;
            let mut was_inside = false;
            for x in 0..image.width() {
                let inside = image.get_pixel(x, y)[0] < 0.0;
                if inside && !was_inside {
                    starts += 1;
                }
                was_inside = inside;
            }
            starts == 2
        }));
        request.text = "I".into();
        let b = prepare(&request, &fonts).unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
        assert_ne!(a.as_bytes(), b.as_bytes());
        request.size = Some([110.0, 120.0]);
        let c = prepare(&request, &fonts).unwrap();
        assert_eq!([c.width(), c.height()], [220, 240]);
        let different_font = vec![FontSource {
            bytes: include_bytes!("../../assets/fonts/MiSans/MiSans-Regular.ttf").to_vec(),
            face_index: 0,
        }];
        assert_ne!(
            c.as_bytes(),
            prepare(&request, &different_font).unwrap().as_bytes()
        );
    }

    #[test]
    fn exact_euclidean_distance_holes_and_subpixel_edges() {
        let mut mask = vec![0.0; 49];
        for y in 1..6 {
            for x in 1..6 {
                mask[y * 7 + x] = 1.0;
            }
        }
        mask[24] = 0.0;
        let sdf = signed_distance(&mask, 7, 7);
        assert!((sdf[0] - 2.0_f32.sqrt()).abs() < 1e-5);
        assert_eq!(sdf[24], 1.0); // counter remains outside
        assert_eq!(sdf[8], -1.0);
        for (i, &d) in sdf.iter().enumerate() {
            let reference = (0..49)
                .filter(|&j| mask[j] != mask[i])
                .map(|j| ((i % 7) as f32 - (j % 7) as f32).hypot((i / 7) as f32 - (j / 7) as f32))
                .fold(f32::INFINITY, f32::min);
            assert!((d.abs() - reference).abs() < 1e-5);
        }
        assert_eq!(signed_distance(&[0.25, 0.75], 2, 1), vec![0.25, -0.25]);
        assert!(
            signed_distance(&[0.0; 16], 4, 4)
                .iter()
                .all(|v| v.is_finite() && *v > 0.0)
        );
    }
}
