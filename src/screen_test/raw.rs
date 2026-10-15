use super::model::RawMetadata;
use anyhow::{Context, Result, ensure};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct RawFrame {
    pub metadata: RawMetadata,
    pub pixels: Vec<u16>,
}
pub fn helper() -> PathBuf {
    std::env::var_os("NODE_FORGE_SCREEN_TEST_NATIVE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/screen-test/screen-test-native")
        })
}
pub fn native(args: &[&std::ffi::OsStr]) -> Result<Vec<u8>> {
    let mut child=Command::new(helper()).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("Build the measurement helper: tools/screen-test-native/build.sh (requires libraw and opencv)")?;
    let stdout = child.stdout.take().context("Helper stdout unavailable")?;
    let stderr = child.stderr.take().context("Helper stderr unavailable")?;
    let output = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(210_000_000)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .take(1_000_000)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if start.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let output = output
        .join()
        .map_err(|_| anyhow::anyhow!("RAW output reader failed"))??;
    let errors = errors
        .join()
        .map_err(|_| anyhow::anyhow!("RAW error reader failed"))??;
    let status =
        status.context("Measurement helper timed out after 60 seconds; original RAW retained")?;
    ensure!(
        status.success(),
        "Measurement helper: {}",
        String::from_utf8_lossy(&errors)
    );
    ensure!(
        output.len() < 210_000_000,
        "Measurement helper output too large"
    );
    Ok(output)
}
impl RawFrame {
    pub fn decode(path: &Path) -> Result<Self> {
        Self::from_bytes(&native(&[std::ffi::OsStr::new("raw"), path.as_os_str()])?)
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= 8 && &bytes[..4] == b"NFR1",
            "Invalid RAW helper response"
        );
        let n = u32::from_le_bytes(bytes[4..8].try_into()?) as usize;
        ensure!(
            n <= 65536 && bytes.len() >= 8 + n,
            "Invalid RAW header size"
        );
        let metadata: RawMetadata = serde_json::from_slice(&bytes[8..8 + n])?;
        let count = (metadata.width as usize)
            .checked_mul(metadata.height as usize)
            .context("RAW size overflow")?;
        ensure!(
            count > 0 && count <= 100_000_000 && bytes.len() - 8 - n == count * 2,
            "RAW payload size mismatch"
        );
        ensure!(
            metadata.cfa.iter().all(|c| *c < 4) && (0..4).all(|c| metadata.cfa.contains(&c)),
            "Invalid CFA"
        );
        ensure!(
            metadata.exposure.is_finite()
                && metadata.exposure > 0.
                && metadata.white > 0.
                && metadata
                    .black
                    .iter()
                    .all(|b| b.is_finite() && *b < metadata.white),
            "Invalid RAW radiometry"
        );
        let pixels = bytes[8 + n..]
            .chunks_exact(2)
            .map(|v| u16::from_le_bytes([v[0], v[1]]))
            .collect();
        Ok(Self { metadata, pixels })
    }
    /// x/y index the private top-left raster. Coordinates exposed by geometry remain Y-up.
    pub fn sample(&self, x: u32, y: u32) -> (usize, f64, bool) {
        let c = self.metadata.cfa[((y % 2) * 2 + x % 2) as usize] as usize;
        let raw = self.pixels[(y * self.metadata.width + x) as usize] as f64;
        (
            c,
            raw - self.metadata.black[c],
            raw >= self.metadata.white - 16.,
        )
    }
    pub fn detection_image(&self) -> image::GrayImage {
        // Detection has its own exposure range. Mapping the sensor's entire saturation range
        // to u8 destroys marker contrast in dark captures (e.g. a screen exposed for highlights).
        // These are disposable green-plane values; never change the measurement CFA buffer.
        let width = self.metadata.width;
        let height = self.metadata.height;
        let cell_width = width.div_ceil(2);
        let mut cells = Vec::with_capacity((cell_width * height.div_ceil(2)) as usize);
        let mut histogram = vec![0usize; 65536];
        for y in (0..height).step_by(2) {
            for x in (0..width).step_by(2) {
                let mut sum = 0.;
                let mut n = 0.;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let xx = (x + dx).min(width - 1);
                        let yy = (y + dy).min(height - 1);
                        let (c, v, _) = self.sample(xx, yy);
                        if c == 1 || c == 3 {
                            sum += v;
                            n += 1.;
                        }
                    }
                }
                let value = if n > 0. {
                    (sum / n).clamp(0., 65535.)
                } else {
                    0.
                };
                histogram[value.round() as usize] += 1;
                cells.push(value as f32);
            }
        }
        let percentile = |numerator: usize, denominator: usize| {
            let rank = (cells.len() * numerator).div_ceil(denominator).max(1);
            let mut total = 0;
            for (value, count) in histogram.iter().enumerate() {
                total += count;
                if total >= rank {
                    return value as f32;
                }
            }
            0.
        };
        // Ignore a few hot pixels/reflections; a maximum-based gain is not exposure robust.
        let low = percentile(1, 100);
        let high = percentile(995, 1000).max(low + 1.);
        image::GrayImage::from_fn(width, height, |x, y| {
            let value = cells[((y / 2) * cell_width + x / 2) as usize];
            image::Luma([((value - low) * 255. / (high - low))
                .clamp(0., 255.)
                .round() as u8])
        })
    }
    /// Unrectified camera-linear RGB proxy. Never used as measurement input.
    pub fn sensor_preview(&self) -> image::RgbImage {
        let m = &self.metadata;
        let mut image = image::RgbImage::new(m.width, m.height);
        let scale = m.white - m.black.iter().sum::<f64>() / 4.;
        for y in (0..m.height).step_by(2) {
            for x in (0..m.width).step_by(2) {
                let mut rgb = [0.; 3];
                let mut n = [0.; 3];
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (c, value, _) =
                            self.sample((x + dx).min(m.width - 1), (y + dy).min(m.height - 1));
                        let c = if c == 3 { 1 } else { c };
                        rgb[c] += value;
                        n[c] += 1.;
                    }
                }
                let color = image::Rgb(std::array::from_fn(|c| {
                    let v = (rgb[c] / n[c] / scale).clamp(0., 1.);
                    ((if v < 0.0031308 {
                        12.92 * v
                    } else {
                        1.055 * v.powf(1. / 2.4) - 0.055
                    }) * 255.)
                        .round() as u8
                }));
                for dy in 0..2 {
                    for dx in 0..2 {
                        if x + dx < m.width && y + dy < m.height {
                            image.put_pixel(x + dx, y + dy, color);
                        }
                    }
                }
            }
        }
        image
    }
}

#[derive(serde::Deserialize)]
pub struct RawDisplayInfo {
    pub sensor_crop: [f64; 4],
    pub flip: i32,
}
pub fn display_info(path: &Path) -> Result<RawDisplayInfo> {
    Ok(serde_json::from_slice(&native(&[
        std::ffi::OsStr::new("display-info"),
        path.as_os_str(),
    ])?)?)
}
