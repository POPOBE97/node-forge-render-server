use super::{model::*, raw::native};
use anyhow::{Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
#[derive(Deserialize)]
struct Dictionary {
    markers: Vec<Vec<u8>>,
}
pub struct Pattern {
    pub manifest: PatternManifest,
    pub rgba: Vec<[f32; 4]>,
}
fn canonical(rgb: [f32; 3], mode: DisplayMode) -> [f32; 3] {
    if mode != DisplayMode::DisplayP3 {
        return rgb;
    }
    let [r, g, b] = rgb;
    [
        1.224745 * r - 0.224904 * g,
        -0.042058 * r + 1.042081 * g,
        -0.019642 * r - 0.078655 * g + 1.098537 * b,
    ]
}
impl Pattern {
    pub fn generate(device: &DisplayDevice, mode: DisplayMode, peak: f32) -> Result<Self> {
        let dict: Dictionary =
            serde_json::from_slice(&native(&[std::ffi::OsStr::new("dictionary")])?)?;
        Self::with_dictionary(device, mode, peak, &dict.markers)
    }
    pub fn with_dictionary(
        device: &DisplayDevice,
        mode: DisplayMode,
        peak: f32,
        dictionary: &[Vec<u8>],
    ) -> Result<Self> {
        let [w, h] = device.drawable;
        ensure!(
            device.size == device.drawable,
            "Display buffer differs from physical display; disable compatibility scaling"
        );
        ensure!(
            w >= 480
                && h >= 640
                && w <= 8192
                && h <= 8192
                && u64::from(w) * u64::from(h) <= 20_000_000,
            "Unsupported chart dimensions"
        );
        ensure!(
            peak.is_finite() && peak >= 1. && peak <= 8.,
            "Invalid HDR peak"
        );
        ensure!(
            dictionary.len() >= 224 && dictionary.iter().all(|m| m.len() == 64),
            "Invalid ArUco dictionary"
        );
        let [left, bottom, right, top] = device.insets;
        ensure!(
            left + right + 320 < w && top + bottom + 480 < h,
            "No usable chart viewport"
        );
        let generation = new_id("pattern");
        let digest = Sha256::digest(generation.as_bytes());
        let margin = (w / 80).max(8);
        let x0 = left + margin;
        let y0 = bottom + margin;
        let cw = (w - left - right - 2 * margin) * 10 / 86;
        let center_gap = cw * 6 / 10;
        let ch = (h - top - bottom - 2 * margin) * 4 / 5 / 12;
        let board_y = y0 + (h - top - bottom - 2 * margin) / 5;
        let mut result = Self {
            manifest: PatternManifest {
                version: "screen-chart-2".into(),
                generation,
                size: [w, h],
                insets: device.insets,
                mode,
                peak: if mode == DisplayMode::LinearHdr {
                    peak
                } else {
                    1.
                },
                patches: vec![],
                markers: vec![],
                sha256: String::new(),
            },
            rgba: vec![[0.18, 0.18, 0.18, 1.]; (w * h) as usize],
        };
        let mut marker_index = 0;
        let mut patch_id = 0;
        for row in 0..12 {
            for col in 0..8 {
                let x = x0 + col * cw + if col >= 4 { center_gap } else { 0 };
                let y = board_y + row * ch;
                if row == 0 || row == 11 || col == 0 || col == 7 {
                    let id = if marker_index < 30 {
                        marker_index as i32
                    } else {
                        let i = marker_index - 30;
                        128 + i as i32 * 16 + ((digest[i / 2] >> ((i % 2) * 4)) & 15) as i32
                    };
                    marker_index += 1;
                    let side = (cw.min(ch) * 3 / 4 / 8) * 8;
                    let px = x + (cw - side) / 2;
                    let py = y + (ch - side) / 2;
                    result.rect([x, y, cw, ch], [0.5; 3]);
                    for yy in 0..side {
                        for xx in 0..side {
                            let bit = dictionary[id as usize]
                                [((7 - yy / (side / 8)) * 8 + xx / (side / 8)) as usize];
                            result.pixel(px + xx, py + yy, [if bit == 0 { 0.01 } else { 0.5 }; 3]);
                        }
                    }
                    result.manifest.markers.push(Marker {
                        id,
                        rect: [px, py, side, side],
                    });
                } else {
                    let mut rgb = if patch_id < 12 {
                        let g = patch_id as f32 / 11.;
                        [g.powf(2.2); 3]
                    } else {
                        let i = patch_id - 12;
                        let level = [0.04, 0.12, 0.3, 0.65, 1., 0.2, 0.5, 0.85][(i / 6) as usize];
                        match i % 6 {
                            0 => [level, 0., 0.],
                            1 => [0., level, 0.],
                            2 => [0., 0., level],
                            3 => [level, level, 0.],
                            4 => [0., level, level],
                            _ => [level, 0., level],
                        }
                    };
                    let repeated_neutral = [12, 23, 25, 34, 38, 47].contains(&patch_id);
                    if repeated_neutral {
                        rgb = [0.18; 3];
                    }
                    if mode == DisplayMode::LinearHdr && patch_id >= 42 && !repeated_neutral {
                        for c in &mut rgb {
                            *c *= peak;
                        }
                    }
                    let pad = (cw.min(ch) / 10).max(3);
                    let rect = [x + pad, y + pad, cw - 2 * pad, ch - 2 * pad];
                    result.rect(rect, rgb);
                    result.manifest.patches.push(Patch {
                        id: patch_id,
                        rect,
                        rgb: canonical(rgb, mode),
                        display_rgb: rgb,
                    });
                    patch_id += 1;
                }
            }
        }
        // Interior correspondences constrain lens distortion, in addition to the perimeter.
        for (id, row) in [(100, 2), (101, 5), (102, 8)] {
            let side = (center_gap.min(ch) * 3 / 4 / 8) * 8;
            let px = x0 + 4 * cw + (center_gap - side) / 2;
            let py = board_y + row * ch + (ch - side) / 2;
            result.rect(
                [x0 + 4 * cw, py - side / 8, center_gap, side + side / 4],
                [0.5; 3],
            );
            for yy in 0..side {
                for xx in 0..side {
                    let bit = dictionary[id as usize]
                        [((7 - yy / (side / 8)) * 8 + xx / (side / 8)) as usize];
                    result.pixel(px + xx, py + yy, [if bit == 0 { 0.01 } else { 0.5 }; 3]);
                }
            }
            result.manifest.markers.push(Marker {
                id,
                rect: [px, py, side, side],
            });
        }
        // Baseline-inspired native-pixel resolution targets, circle and continuous ramps.
        let band = (board_y - y0).saturating_sub(margin);
        for y in y0..y0 + band {
            for x in x0..w - right - margin {
                let u = (x - x0) as f32 / (w - right - margin - x0) as f32;
                let v = (y - y0) as f32 / band as f32;
                let rgb = if v < 0.2 {
                    [u; 3]
                } else if u < 0.5 {
                    let stripe = 1u32 << ((u * 10.) as u32).min(4);
                    [if (x / stripe) % 2 == 0 { 0. } else { 1. }; 3]
                } else {
                    let width = (w - right - margin - x0) as f32;
                    let d = (((x - x0) as f32 - width * 0.75).powi(2)
                        + ((y - y0) as f32 - band as f32 * 0.6).powi(2))
                    .sqrt();
                    let radius = (width * 0.21).min(band as f32 * 0.32);
                    [if (d - radius).abs() < 1.5 { 1. } else { 0.18 }; 3]
                };
                result.pixel(x, y, rgb);
            }
        }
        result.text(
            x0 + 4 * cw + center_gap,
            y0 + band * 3 / 5,
            "NODE FORGE",
            (cw / 40).max(1),
        );
        let bytes = result.bytes();
        result.manifest.sha256 = format!("{:x}", Sha256::digest(&bytes));
        Ok(result)
    }
    fn pixel(&mut self, x: u32, y: u32, rgb: [f32; 3]) {
        let [w, h] = self.manifest.size;
        if x < w && y < h {
            self.rgba[((h - 1 - y) * w + x) as usize] = [rgb[0], rgb[1], rgb[2], 1.];
        }
    }
    fn rect(&mut self, rect: [u32; 4], rgb: [f32; 3]) {
        let [x, y, w, h] = rect;
        for yy in y..y + h {
            for xx in x..x + w {
                self.pixel(xx, yy, rgb);
            }
        }
    }
    fn text(&mut self, x: u32, y: u32, text: &str, scale: u32) {
        // Pixel-font targets are redrawn at integer device pixels, never rescaled bitmaps.
        for (index, character) in text.chars().enumerate() {
            let rows = match character {
                'N' => [17, 25, 25, 21, 19, 19, 17],
                'O' => [14, 17, 17, 17, 17, 17, 14],
                'D' => [30, 17, 17, 17, 17, 17, 30],
                'E' => [31, 16, 16, 30, 16, 16, 31],
                'F' => [31, 16, 16, 30, 16, 16, 16],
                'R' => [30, 17, 17, 30, 20, 18, 17],
                'G' => [14, 17, 16, 23, 17, 17, 14],
                _ => [0; 7],
            };
            for (row, bits) in rows.into_iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        self.rect(
                            [
                                x + (index as u32 * 6 + col) * scale,
                                y + (6 - row as u32) * scale,
                                scale,
                                scale,
                            ],
                            [1.; 3],
                        );
                    }
                }
            }
        }
    }
    pub fn bytes(&self) -> Vec<u8> {
        self.rgba
            .iter()
            .flat_map(|p| {
                p.iter()
                    .flat_map(|v| half::f16::from_f32(*v).to_bits().to_le_bytes())
            })
            .collect()
    }
    pub fn save(&self, directory: &std::path::Path) -> Result<()> {
        std::fs::create_dir_all(directory)?;
        std::fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec_pretty(&self.manifest)?,
        )?;
        std::fs::write(directory.join("pattern.rgba16f"), self.bytes())?;
        let [w, h] = self.manifest.size;
        let mut preview = image::RgbImage::new(w, h);
        for (pixel, value) in preview.pixels_mut().zip(&self.rgba) {
            let rgb = canonical([value[0], value[1], value[2]], self.manifest.mode);
            *pixel = image::Rgb(rgb.map(|v| {
                let v = v.clamp(0., 1.);
                ((if v < 0.0031308 {
                    12.92 * v
                } else {
                    1.055 * v.powf(1. / 2.4) - 0.055
                }) * 255.)
                    .round() as u8
            }));
        }
        preview.save(directory.join("preview.png"))?;
        Ok(())
    }
}
