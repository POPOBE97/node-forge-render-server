use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufWriter, Cursor, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, ensure};
use image::ImageDecoder;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

use super::model::*;
use crate::reference_image::color::{
    PngRgbSpace, detect_icc_rgb_space, detect_png_rgb_space, linear_p3_to_linear_srgb,
};

#[derive(Debug)]
pub enum Pixels {
    Srgb8(Vec<u8>),
    Linear(Vec<f32>),
}
#[derive(Debug)]
pub struct DecodedImage {
    pub size: [u32; 2],
    pub bit_depth: u8,
    pub pixels: Pixels,
}
#[derive(Debug)]
pub struct Asset {
    pub bytes: Arc<Vec<u8>>,
    pub image: Arc<DecodedImage>,
}
pub type Assets = HashMap<String, Arc<Asset>>;
pub fn content_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn decode(bytes: &[u8]) -> Result<DecodedImage> {
    let format = image::guess_format(bytes)?;
    ensure!(
        matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg),
        "Pixel Perfecto supports PNG and JPEG images"
    );
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1024 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let (w, h) = decoder.dimensions();
    ensure!(
        w > 0 && h > 0 && u64::from(w) * u64::from(h) <= 100_000_000,
        "Image exceeds 100 megapixels"
    );
    let orientation = decoder.orientation()?;
    let space = if format == image::ImageFormat::Png {
        detect_png_rgb_space(bytes)?
    } else {
        match decoder.icc_profile()? {
            Some(profile) => detect_icc_rgb_space(&profile)
                .context("JPEG has an unsupported color profile; convert to sRGB or Display P3")?,
            None => PngRgbSpace::Srgb,
        }
    };
    let mut image = image::DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    let size = [image.width(), image.height()];
    let bit_depth = if image.color().bits_per_pixel() / image.color().channel_count() as u16 > 8 {
        16
    } else {
        8
    };
    let pixels = if bit_depth == 8 && space == PngRgbSpace::Srgb {
        Pixels::Srgb8(image.into_rgba8().into_raw())
    } else {
        let mut rgba = image.into_rgba32f().into_raw();
        for p in rgba.chunks_exact_mut(4) {
            let mut rgb = [
                crate::color::srgb_to_linear_channel(p[0]),
                crate::color::srgb_to_linear_channel(p[1]),
                crate::color::srgb_to_linear_channel(p[2]),
            ];
            if space == PngRgbSpace::DisplayP3 {
                rgb = linear_p3_to_linear_srgb(rgb);
            }
            for i in 0..3 {
                p[i] = rgb[i] * p[3];
            }
        }
        Pixels::Linear(rgba)
    };
    Ok(DecodedImage {
        size,
        bit_depth,
        pixels,
    })
}
pub fn import(path: &Path) -> Result<(String, String, Arc<Asset>)> {
    let bytes = fs::read(path).with_context(|| format!("Read {}", path.display()))?;
    let image = decode(&bytes).with_context(|| format!("Decode {}", path.display()))?;
    Ok((
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        content_hash(&bytes),
        Arc::new(Asset {
            bytes: Arc::new(bytes),
            image: Arc::new(image),
        }),
    ))
}
impl DecodedImage {
    pub fn pixel(&self, x: i32, y: i32) -> [f32; 4] {
        if x < 0 || y < 0 || x >= self.size[0] as i32 || y >= self.size[1] as i32 {
            return [0.; 4];
        }
        let i = (y as usize * self.size[0] as usize + x as usize) * 4;
        match &self.pixels {
            Pixels::Srgb8(data) => {
                static LUT: OnceLock<[f32; 256]> = OnceLock::new();
                let lut = LUT.get_or_init(|| {
                    std::array::from_fn(|i| crate::color::srgb_to_linear_channel(i as f32 / 255.))
                });
                let a = data[i + 3] as f32 / 255.;
                [
                    lut[data[i] as usize] * a,
                    lut[data[i + 1] as usize] * a,
                    lut[data[i + 2] as usize] * a,
                    a,
                ]
            }
            Pixels::Linear(data) => [data[i], data[i + 1], data[i + 2], data[i + 3]],
        }
    }
    pub fn sample(&self, p: Point, nearest: bool) -> [f32; 4] {
        if p[0] < 0. || p[1] < 0. || p[0] >= self.size[0] as f64 || p[1] >= self.size[1] as f64 {
            return [0.; 4];
        }
        // Raster row order is private to image storage; public coordinates remain Y-up.
        let x = p[0] - 0.5;
        let y = self.size[1] as f64 - p[1] - 0.5;
        if nearest {
            return self.pixel(x.round() as i32, y.round() as i32);
        }
        let ix = x.floor() as i32;
        let iy = y.floor() as i32;
        let fx = (x - x.floor()) as f32;
        let fy = (y - y.floor()) as f32;
        let samples = [
            self.pixel(ix, iy),
            self.pixel(ix + 1, iy),
            self.pixel(ix, iy + 1),
            self.pixel(ix + 1, iy + 1),
        ];
        std::array::from_fn(|c| {
            (samples[0][c] * (1. - fx) + samples[1][c] * fx) * (1. - fy)
                + (samples[2][c] * (1. - fx) + samples[3][c] * fx) * fy
        })
    }
}

const APPLICATION_ID: i32 = 0x50585046;
pub struct TemporaryOutput {
    pub path: PathBuf,
    destination: PathBuf,
}
impl TemporaryOutput {
    pub fn new(path: &Path) -> Result<Self> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let name = path
            .file_name()
            .context("Output has no filename")?
            .to_string_lossy();
        let temporary = path.with_file_name(format!(
            ".{name}.pp-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        Ok(Self {
            path: temporary,
            destination: path.into(),
        })
    }
    pub fn commit(&self) -> Result<()> {
        File::open(&self.path)?.sync_all()?;
        fs::rename(&self.path, &self.destination).context("Replace output file")
    }
}
impl Drop for TemporaryOutput {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn save(path: &Path, doc: &PixelPerfectoDocument, assets: &Assets) -> Result<()> {
    doc.validate()?;
    let json = serde_json::to_string(doc)?;
    let output = TemporaryOutput::new(path)?;
    let mut db = Connection::open(&output.path)?;
    db.pragma_update(None, "application_id", APPLICATION_ID)?;
    let tx = db.transaction()?;
    tx.execute_batch("CREATE TABLE document (id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL); CREATE TABLE assets (hash TEXT PRIMARY KEY, bytes BLOB NOT NULL);")?;
    tx.execute("INSERT INTO document VALUES (1, ?1)", params![json])?;
    let mut written = std::collections::HashSet::new();
    for layer in &doc.layers {
        if written.insert(&layer.asset) {
            let asset = assets
                .get(&layer.asset)
                .context("An image is unavailable; project was not saved")?;
            ensure!(
                content_hash(&asset.bytes) == layer.asset,
                "Image content hash mismatch"
            );
            tx.execute(
                "INSERT INTO assets VALUES (?1,?2)",
                params![layer.asset, asset.bytes.as_slice()],
            )?;
        }
    }
    tx.commit()?;
    db.close().map_err(|(_, e)| e)?;
    output.commit()
}
pub fn open(path: &Path) -> Result<(PixelPerfectoDocument, Assets)> {
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let id: i32 = db.pragma_query_value(None, "application_id", |r| r.get(0))?;
    ensure!(id == APPLICATION_ID, "This is not a Pixel Perfecto project");
    let json: String = db.query_row("SELECT json FROM document WHERE id=1", [], |r| r.get(0))?;
    let mut doc: PixelPerfectoDocument = serde_json::from_str(&json)?;
    doc.validate()?;
    let mut assets = Assets::new();
    for layer in &doc.layers {
        if !assets.contains_key(&layer.asset) {
            let bytes: Vec<u8> = db.query_row(
                "SELECT bytes FROM assets WHERE hash=?1",
                params![layer.asset],
                |r| r.get(0),
            )?;
            ensure!(
                content_hash(&bytes) == layer.asset,
                "Embedded image is corrupt"
            );
            let image = decode(&bytes)?;
            ensure!(
                image.size == layer.size && image.bit_depth == layer.bit_depth,
                "Embedded image metadata does not match its layer"
            );
            assets.insert(
                layer.asset.clone(),
                Arc::new(Asset {
                    bytes: Arc::new(bytes),
                    image: Arc::new(image),
                }),
            );
        }
    }
    doc.repair_selection();
    Ok((doc, assets))
}

pub fn over(top: [f32; 4], bottom: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|c| top[c] + bottom[c] * (1. - top[3]))
}
pub fn sample_layer(
    layer: &ImageLayer,
    assets: &Assets,
    world: Point,
    nearest: bool,
    opacity: bool,
) -> [f32; 4] {
    let Some(p) = layer.sample_position(world) else {
        return [0.; 4];
    };
    let Some(asset) = assets.get(&layer.asset) else {
        return [0.; 4];
    };
    let value = asset.image.sample(p, nearest);
    let alpha = if opacity { layer.opacity } else { 1. };
    value.map(|v| v * alpha)
}
pub fn composite(
    doc: &PixelPerfectoDocument,
    assets: &Assets,
    world: Point,
    uv: Point,
    selected_only: bool,
) -> [f32; 4] {
    if selected_only {
        return doc
            .selected_layer()
            .map(|l| sample_layer(l, assets, world, doc.nearest, false))
            .unwrap_or([0.; 4]);
    }
    let c = &doc.comparison;
    if c.mode == CompareMode::Overlay {
        return doc
            .layers
            .iter()
            .filter(|l| l.visible)
            .fold([0.; 4], |out, l| {
                over(sample_layer(l, assets, world, doc.nearest, true), out)
            });
    }
    let layer = |id| doc.layers.iter().find(|l| Some(l.id) == id);
    let sample = |id| {
        layer(id)
            .map(|l| sample_layer(l, assets, world, doc.nearest, false))
            .unwrap_or([0.; 4])
    };
    let a = sample(c.a);
    let b = sample(c.b);
    match c.mode {
        CompareMode::Difference => {
            if a[3] <= 0. || b[3] <= 0. {
                return [0.; 4];
            }
            [
                (a[0] / a[3] - b[0] / b[3]).abs() * c.gain,
                (a[1] / a[3] - b[1] / b[3]).abs() * c.gain,
                (a[2] / a[3] - b[2] / b[3]).abs() * c.gain,
                1.,
            ]
        }
        CompareMode::Ab => {
            if c.show_b {
                b
            } else {
                a
            }
        }
        CompareMode::Wipe => {
            if uv[if c.horizontal { 1 } else { 0 }] < c.wipe as f64 {
                a
            } else {
                b
            }
        }
        CompareMode::Overlay => unreachable!(),
    }
}

pub fn export_png(
    path: &Path,
    doc: &PixelPerfectoDocument,
    assets: &Assets,
    progress: &AtomicU64,
) -> Result<()> {
    let e = &doc.export;
    let [w, h] = e.dimensions()?;
    ensure!(!doc.layers.is_empty(), "Import an image before exporting");
    let output = TemporaryOutput::new(path)?;
    let file = BufWriter::new(File::create(&output.path)?);
    let mut encoder = png::Encoder::new(file, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(if e.bit_depth == 16 {
        png::BitDepth::Sixteen
    } else {
        png::BitDepth::Eight
    });
    encoder.set_source_srgb(png::SrgbRenderingIntent::RelativeColorimetric);
    let mut writer = encoder.write_header()?;
    {
        let mut stream = writer.stream_writer()?;
        let stride = if e.bit_depth == 16 { 8 } else { 4 };
        let mut row = vec![0u8; w as usize * stride];
        for y in 0..h {
            for x in 0..w {
                let world = [
                    e.bounds[0] + (x as f64 + 0.5) / e.pixels_per_unit,
                    e.bounds[3] - (y as f64 + 0.5) / e.pixels_per_unit,
                ];
                let uv = [(x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64];
                let mut rgba = composite(doc, assets, world, uv, e.selected_only);
                if e.grid && doc.grid.visible {
                    rgba = over(
                        super::grid::sample_grid(world, &doc.grid, e.pixels_per_unit),
                        rgba,
                    );
                }
                let a = rgba[3].clamp(0., 1.);
                for c in 0..4 {
                    let v = if c == 3 {
                        a
                    } else if a > 1e-8 {
                        crate::color::linear_to_srgb_channel(rgba[c] / a).clamp(0., 1.)
                    } else {
                        0.
                    };
                    if stride == 8 {
                        let bytes = ((v * 65535.).round() as u16).to_be_bytes();
                        let i = x as usize * 8 + c * 2;
                        row[i..i + 2].copy_from_slice(&bytes);
                    } else {
                        row[x as usize * 4 + c] = (v * 255.).round() as u8;
                    }
                }
            }
            stream.write_all(&row)?;
            progress.store((y as u64 + 1) * 100 / h as u64, Ordering::Relaxed);
        }
        stream.finish()?;
    }
    writer.finish()?;
    output.commit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    pub fn png16() -> Vec<u8> {
        let mut data = vec![];
        let pixels = [12000u16, 20000, 30000, 65535, 12001, 20001, 30001, 65535];
        let bytes: Vec<u8> = pixels.iter().flat_map(|v| v.to_ne_bytes()).collect();
        image::codecs::png::PngEncoder::new(&mut data)
            .write_image(&bytes, 2, 1, image::ExtendedColorType::Rgba16)
            .unwrap();
        data
    }
    fn fixture() -> (PixelPerfectoDocument, Assets) {
        let bytes = png16();
        let key = content_hash(&bytes);
        let image = decode(&bytes).unwrap();
        let mut assets = Assets::new();
        assets.insert(
            key.clone(),
            Arc::new(Asset {
                bytes: Arc::new(bytes),
                image: Arc::new(image),
            }),
        );
        let mut doc = PixelPerfectoDocument::default();
        doc.layers.push(ImageLayer {
            id: 1,
            asset: key,
            name: "16-bit.png".into(),
            size: [2, 1],
            bit_depth: 16,
            visible: true,
            locked: false,
            opacity: 1.,
            offset: [-2, 4],
            calibration: super::super::calibration::Calibration::initial([2, 1]),
            landmarks: vec![],
        });
        doc.repair_selection();
        (doc, assets)
    }
    #[test]
    fn sixteen_bit_source_is_not_truncated() {
        let im = decode(&png16()).unwrap();
        assert_eq!(im.bit_depth, 16);
        let a = im.pixel(0, 0);
        let b = im.pixel(1, 0);
        assert!(b[0] > a[0]);
        assert!(b[0] - a[0] < 0.0001);
    }
    #[test]
    fn project_roundtrip_embeds_originals_and_failed_save_preserves_file() {
        let (mut doc, assets) = fixture();
        doc.measurement = Some(Measurement::new([-3.25, 1.], [5., -2.]));
        doc.rulers_mut().guides = vec![
            Guide {
                id: 1,
                axis: GuideAxis::Horizontal,
                position: -12.375,
            },
            Guide {
                id: 2,
                axis: GuideAxis::Vertical,
                position: 8.125,
            },
        ];
        doc.rulers_mut().locked = true;
        let path = std::env::temp_dir().join(format!(
            "pixel-perfecto-test-{}.pixelperfecto",
            std::process::id()
        ));
        save(&path, &doc, &assets).unwrap();
        let original = fs::read(&path).unwrap();
        let (loaded, images) = open(&path).unwrap();
        assert_eq!(loaded, doc);
        assert_eq!(
            images.values().next().unwrap().bytes,
            assets.values().next().unwrap().bytes
        );
        assert!(save(&path, &doc, &Assets::new()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn difference_only_uses_valid_overlap_and_nearest_preserves_centers() {
        let im = decode(&png16()).unwrap();
        assert_eq!(im.sample([0.5, 0.5], false), im.pixel(0, 0));
        assert_eq!(im.sample([-0.1, 0.5], false), [0.; 4]);
        let (mut d, a) = fixture();
        d.comparison.mode = CompareMode::Difference;
        assert_eq!(
            composite(&d, &a, [10000., 10000.], [0., 0.], false),
            [0.; 4]
        );
    }
}

#[cfg(test)]
mod export_tests {
    use super::*;
    use crate::pixel_perfecto::calibration::Calibration;
    #[test]
    fn export_preserves_16_bit_source_and_uses_y_up_crop() {
        use image::ImageEncoder;
        let values: Vec<u16> = (0..24).map(|i| 1000 + i * 1537).collect();
        let mut bytes = vec![];
        let mut pixels = values.clone();
        for i in (3..24).step_by(4) {
            pixels[i] = 65535;
        }
        let native: Vec<_> = pixels.iter().flat_map(|v| v.to_ne_bytes()).collect();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&native, 3, 2, image::ExtendedColorType::Rgba16)
            .unwrap();
        let image = decode(&bytes).unwrap();
        let key = content_hash(&bytes);
        let mut assets = Assets::new();
        assets.insert(
            key.clone(),
            Arc::new(Asset {
                bytes: Arc::new(bytes),
                image: Arc::new(image),
            }),
        );
        let mut doc = PixelPerfectoDocument::default();
        doc.grid.visible = false;
        // Saved editing annotations must never enter exported image pixels.
        doc.measurement = Some(Measurement::new([0., 0.], [3., 2.]));
        doc.layers.push(ImageLayer {
            id: 1,
            asset: key,
            name: "source".into(),
            size: [3, 2],
            bit_depth: 16,
            visible: true,
            locked: false,
            opacity: 1.,
            offset: [0, 0],
            calibration: Calibration {
                h: [1. / 1.5, 0., -1., 0., 1. / 1.5, -1. / 1.5, 0., 0., 1.],
                lens: [0.; 4],
                center: [1.5, 1.],
            },
            landmarks: vec![],
        });
        doc.repair_selection();
        doc.export = ExportSettings {
            bounds: [0., 0., 3., 2.],
            pixels_per_unit: 1.,
            bit_depth: 16,
            selected_only: true,
            grid: false,
        };
        let path = std::env::temp_dir().join(format!("pp-export-{}.png", std::process::id()));
        let progress = AtomicU64::new(0);
        export_png(&path, &doc, &assets, &progress).unwrap();
        let output = image::open(&path).unwrap().to_rgba16().into_raw();
        for (expected, actual) in pixels.iter().zip(output) {
            assert!(
                (*expected as i32 - actual as i32).abs() <= 1,
                "{expected} vs {actual}"
            );
        }
        assert_eq!(progress.load(Ordering::Relaxed), 100);
        fs::remove_file(path).unwrap();
    }
}
