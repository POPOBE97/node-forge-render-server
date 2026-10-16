//! Geometrically corrected, still-mosaic RAW output.
//!
//! CR2 is a proprietary camera container, so corrected captures are written as DNG. Each output
//! CFA value is resampled only from source values of the same color; RGB channels are never mixed.
use super::{
    model::{DisplayGeometry, RawMetadata},
    raw::RawFrame,
};
use anyhow::{Context, Result, ensure};
use std::path::Path;
use tiff::{
    encoder::{Ifd, Rational, SRational, TiffEncoder},
    tags::Tag,
};

pub fn rectify_raw(raw: &RawFrame, geometry: &DisplayGeometry) -> Result<RawFrame> {
    let source = &raw.metadata;
    let profile = &geometry.profile;
    ensure!(
        profile.raw_size == [source.width, source.height]
            && profile.camera_key == source.geometry_key(),
        "Camera, lens, focal length or RAW dimensions changed; recalibrate before saving RAW"
    );
    let [width, height] = geometry.screen_size;
    ensure!(
        width > 1 && height > 1 && u64::from(width) * u64::from(height) <= 40_000_000,
        "Invalid corrected RAW dimensions"
    );
    profile.mapping.validate(profile.raw_size)?;

    let mut pixels = vec![0; width as usize * height as usize];
    for y in 0..height {
        for x in 0..width {
            let channel = source.cfa[((y % 2) * 2 + x % 2) as usize];
            let color = if channel == 3 { 1 } else { channel };
            let target = [
                (x as f64 + 0.5) / width as f64,
                1. - (y as f64 + 0.5) / height as f64,
            ];
            let value = profile
                .mapping
                .map(target, profile.raw_size)
                .filter(|p| {
                    p[0] >= 0.
                        && p[1] >= 0.
                        && p[0] <= (source.width - 1) as f64
                        && p[1] <= (source.height - 1) as f64
                })
                .map(|p| sample_channel(raw, p[0], (source.height - 1) as f64 - p[1], color))
                .unwrap_or(source.black[channel as usize]);
            pixels[(y * width + x) as usize] = value.round().clamp(0., u16::MAX as f64) as u16;
        }
    }
    let mut metadata = source.clone();
    metadata.width = width;
    metadata.height = height;
    // The output array is already upright and must not receive the source camera rotation again.
    metadata.orientation = 0;
    Ok(RawFrame { metadata, pixels })
}

/// Bilinearly samples one color without ever using a different color's photosites.
fn sample_channel(raw: &RawFrame, x: f64, y: f64, color: u8) -> f64 {
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(raw.metadata.width - 1);
    let y1 = (y0 + 1).min(raw.metadata.height - 1);
    let fx = x - x0 as f64;
    let fy = y - y0 as f64;
    let top = color_at(raw, x0, y0, color) * (1. - fx) + color_at(raw, x1, y0, color) * fx;
    let bottom = color_at(raw, x0, y1, color) * (1. - fx) + color_at(raw, x1, y1, color) * fx;
    top * (1. - fy) + bottom * fy
}

/// Standard bilinear color-plane reconstruction at one photosite. Only the eight immediate
/// neighbors are needed for a Bayer pattern: same-color sites are orthogonal or diagonal.
fn color_at(raw: &RawFrame, x: u32, y: u32, color: u8) -> f64 {
    let m = &raw.metadata;
    let actual = m.cfa[((y % 2) * 2 + x % 2) as usize];
    if actual == color || actual == 3 && color == 1 {
        return raw.pixels[(y * m.width + x) as usize] as f64;
    }
    let mut sum = 0.;
    let mut count = 0;
    for dy in -1..=1_i32 {
        for dx in -1..=1_i32 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= m.width as i32 || ny >= m.height as i32 {
                continue;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            let candidate = m.cfa[((ny % 2) * 2 + nx % 2) as usize];
            if candidate == color || candidate == 3 && color == 1 {
                sum += raw.pixels[(ny * m.width + nx) as usize] as f64;
                count += 1;
            }
        }
    }
    ensure_color_average(sum, count, raw.pixels[(y * m.width + x) as usize] as f64)
}

fn ensure_color_average(sum: f64, count: usize, fallback: f64) -> f64 {
    if count == 0 {
        fallback
    } else {
        sum / count as f64
    }
}

pub fn write_dng(path: &Path, raw: &RawFrame) -> Result<()> {
    validate_dng(raw)?;
    let mut file = std::fs::File::create(path)
        .with_context(|| format!("Unable to create corrected RAW {}", path.display()))?;
    {
        let mut tiff = TiffEncoder::new(&mut file)?;
        let exif_offset = {
            let mut exif = tiff.extra_directory()?;
            exif.write_tag(
                Tag::from_u16_exhaustive(33434),
                rational(raw.metadata.exposure),
            )?;
            exif.write_tag(
                Tag::from_u16_exhaustive(33437),
                rational(raw.metadata.aperture),
            )?;
            exif.write_tag(
                Tag::from_u16_exhaustive(34855),
                raw.metadata.iso.round().clamp(0., u16::MAX as f64) as u16,
            )?;
            exif.write_tag(
                Tag::from_u16_exhaustive(37386),
                rational(raw.metadata.focal_length),
            )?;
            pad_even(&mut exif)?;
            exif.finish_with_offsets()?.offset
        };
        let mut ifd = tiff.image_directory()?;
        let m = &raw.metadata;
        let make = ascii(&m.make);
        let model = ascii(&m.camera);
        let unique_model = ascii(&format!("{} {}", m.make, m.camera));
        ifd.write_tag(Tag::ImageWidth, m.width)?;
        ifd.write_tag(Tag::ImageLength, m.height)?;
        ifd.write_tag(Tag::BitsPerSample, 16_u16)?;
        ifd.write_tag(Tag::Compression, 1_u16)?;
        ifd.write_tag(Tag::PhotometricInterpretation, 32803_u16)?; // CFA
        ifd.write_tag(Tag::Make, make.as_str())?;
        ifd.write_tag(Tag::Model, model.as_str())?;
        ifd.write_tag(Tag::Orientation, 1_u16)?;
        ifd.write_tag(Tag::SamplesPerPixel, 1_u16)?;
        ifd.write_tag(Tag::RowsPerStrip, m.height)?;
        ifd.write_tag(Tag::PlanarConfiguration, 1_u16)?;
        ifd.write_tag(Tag::SampleFormat, 1_u16)?;
        ifd.write_tag(Tag::Software, "Node Forge Camera Studio")?;
        ifd.write_tag(
            Tag::ImageDescription,
            "Geometrically rectified 16-bit Bayer CFA; original camera RAW retained separately",
        )?;
        ifd.write_tag(Tag::ExifDirectory, Ifd(exif_offset))?;

        ifd.write_tag(Tag::from_u16_exhaustive(50706), &[1_u8, 4, 0, 0][..])?;
        ifd.write_tag(Tag::from_u16_exhaustive(50707), &[1_u8, 1, 0, 0][..])?;
        ifd.write_tag(Tag::from_u16_exhaustive(50708), unique_model.as_str())?;
        ifd.write_tag(Tag::from_u16_exhaustive(33421), &[2_u16, 2][..])?;
        let cfa = m.cfa.map(|c| if c == 3 { 1 } else { c });
        ifd.write_tag(Tag::from_u16_exhaustive(33422), &cfa[..])?;
        ifd.write_tag(Tag::from_u16_exhaustive(50710), &[0_u8, 1, 2][..])?;
        ifd.write_tag(Tag::from_u16_exhaustive(50711), 1_u16)?;
        ifd.write_tag(Tag::from_u16_exhaustive(50713), &[2_u16, 2][..])?;
        let black = m.black.map(rational);
        ifd.write_tag(Tag::from_u16_exhaustive(50714), &black[..])?;
        ifd.write_tag(
            Tag::from_u16_exhaustive(50717),
            m.white.round().clamp(1., u32::MAX as f64) as u32,
        )?;
        ifd.write_tag(
            Tag::from_u16_exhaustive(50718),
            &[Rational { n: 1, d: 1 }, Rational { n: 1, d: 1 }][..],
        )?;
        ifd.write_tag(Tag::from_u16_exhaustive(50719), &[0_u32, 0][..])?;
        ifd.write_tag(Tag::from_u16_exhaustive(50720), &[m.width, m.height][..])?;
        let color_matrix = m.color_matrix.map(signed_rational);
        ifd.write_tag(Tag::from_u16_exhaustive(50721), &color_matrix[..])?;
        let neutral = m.as_shot_neutral.map(rational);
        ifd.write_tag(Tag::from_u16_exhaustive(50728), &neutral[..])?;
        ifd.write_tag(Tag::from_u16_exhaustive(50730), signed_rational(0.))?;
        ifd.write_tag(Tag::from_u16_exhaustive(50731), rational(1.))?;
        ifd.write_tag(Tag::from_u16_exhaustive(50732), rational(1.))?;
        ifd.write_tag(Tag::from_u16_exhaustive(50734), rational(1.))?;
        ifd.write_tag(Tag::from_u16_exhaustive(50778), 21_u16)?; // D65

        pad_even(&mut ifd)?;
        let pixels_offset = ifd.write_data(&raw.pixels[..])?;
        ifd.write_tag(Tag::StripOffsets, u32::try_from(pixels_offset)?)?;
        ifd.write_tag(Tag::StripByteCounts, u32::try_from(raw.pixels.len() * 2)?)?;
        pad_even(&mut ifd)?;
        ifd.finish()?;
    }
    file.sync_all()?;
    Ok(())
}

fn pad_even<W: std::io::Write + std::io::Seek, K: tiff::encoder::TiffKind>(
    directory: &mut tiff::encoder::DirectoryEncoder<'_, W, K>,
) -> Result<()> {
    let empty: &[u8] = &[];
    if directory.write_data(empty)? % 2 != 0 {
        directory.write_data(&[0_u8][..])?;
    }
    Ok(())
}

fn validate_dng(raw: &RawFrame) -> Result<()> {
    let m = &raw.metadata;
    let count = (m.width as usize)
        .checked_mul(m.height as usize)
        .context("Corrected RAW size overflow")?;
    ensure!(
        m.width > 1 && m.height > 1 && count == raw.pixels.len(),
        "Corrected RAW dimensions do not match its pixels"
    );
    ensure!(
        m.cfa.iter().all(|c| *c < 4)
            && m.color_matrix.iter().all(|v| v.is_finite())
            && m.as_shot_neutral.iter().all(|v| v.is_finite() && *v > 0.)
            && m.black.iter().all(|v| v.is_finite() && *v >= 0.)
            && m.white.is_finite()
            && m.white > 0.,
        "Invalid corrected RAW metadata"
    );
    Ok(())
}

fn ascii(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|c| if c.is_ascii() && c != '\0' { c } else { '?' })
        .collect();
    if value.trim().is_empty() {
        "Unknown".into()
    } else {
        value
    }
}

fn rational(value: f64) -> Rational {
    let denominator = 1_000_000_u32;
    Rational {
        n: (value * denominator as f64)
            .round()
            .clamp(0., u32::MAX as f64) as u32,
        d: denominator,
    }
}

fn signed_rational(value: f64) -> SRational {
    let denominator = 1_000_000_i32;
    SRational {
        n: (value * denominator as f64)
            .round()
            .clamp(i32::MIN as f64, i32::MAX as f64) as i32,
        d: denominator,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pixel_perfecto::calibration::Calibration, screen_test::model::GeometryProfile};

    fn metadata(width: u32, height: u32) -> RawMetadata {
        RawMetadata {
            width,
            height,
            cfa: [0, 1, 3, 2],
            black: [100.; 4],
            white: 16383.,
            exposure: 0.01,
            iso: 100.,
            aperture: 5.6,
            focal_length: 50.,
            make: "Canon".into(),
            camera: "EOS 650D".into(),
            lens: "Test".into(),
            orientation: 0,
            color_matrix: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
            as_shot_neutral: [1.; 3],
        }
    }

    #[test]
    fn identity_raw_rectification_preserves_each_cfa_value() {
        let raw = RawFrame {
            metadata: metadata(4, 4),
            pixels: (0..16).map(|v| 1000 + v).collect(),
        };
        let geometry = DisplayGeometry {
            profile: GeometryProfile {
                id: "g".into(),
                revision: 0,
                created_ms: 0,
                source_capture: "test".into(),
                camera_key: raw.metadata.geometry_key(),
                raw_size: [4, 4],
                mapping: Calibration {
                    h: [2., 0., -0.25, 0., 2., -0.25, 0., 0., 1.],
                    lens: [0.; 4],
                    center: [0.; 2],
                },
                holdout_rms: 0.,
                marker_count: 39,
            },
            screen_size: [4, 4],
            sensor_crop: [0., 0., 4., 4.],
        };
        let corrected = rectify_raw(&raw, &geometry).unwrap();
        assert_eq!(corrected.pixels, raw.pixels);
        assert_eq!(corrected.metadata.width, 4);
        assert_eq!(corrected.metadata.orientation, 0);
    }

    #[test]
    fn dng_is_a_single_channel_sixteen_bit_cfa_tiff() {
        let directory = std::env::temp_dir().join(super::super::model::new_id("dng-test"));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("rectified.dng");
        let raw = RawFrame {
            metadata: metadata(4, 4),
            pixels: (0..16).map(|v| 1000 + v).collect(),
        };
        write_dng(&path, &raw).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"II\x2a\0");
        let ifd = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let count = u16::from_le_bytes(bytes[ifd..ifd + 2].try_into().unwrap()) as usize;
        let entry = |wanted: u16| {
            (0..count)
                .map(|i| {
                    let start = ifd + 2 + i * 12;
                    &bytes[start..start + 12]
                })
                .find(|entry| u16::from_le_bytes(entry[..2].try_into().unwrap()) == wanted)
                .unwrap()
                .to_vec()
        };
        let short = |tag| {
            let entry = entry(tag);
            u16::from_le_bytes(entry[8..10].try_into().unwrap())
        };
        let long = |tag| {
            let entry = entry(tag);
            u32::from_le_bytes(entry[8..12].try_into().unwrap())
        };
        assert_eq!(short(262), 32803); // PhotometricInterpretation = CFA
        assert_eq!(short(258), 16); // BitsPerSample
        assert_eq!(long(256), 4);
        assert_eq!(long(257), 4);
        let version = entry(50706);
        assert_eq!(&version[8..12], &[1, 4, 0, 0]);
        let pixels_offset = long(273) as usize;
        let pixels_bytes = long(279) as usize;
        let expected = raw
            .pixels
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(pixels_bytes, expected.len());
        assert_eq!(
            &bytes[pixels_offset..pixels_offset + pixels_bytes],
            expected
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    #[ignore = "requires the locally built LibRaw helper"]
    fn libraw_can_decode_the_corrected_dng() {
        let directory = std::env::temp_dir().join(super::super::model::new_id("dng-libraw"));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("rectified.dng");
        let raw = RawFrame {
            metadata: metadata(64, 64),
            pixels: (0..4096).map(|v| 1000 + v).collect(),
        };
        write_dng(&path, &raw).unwrap();
        let decoded = RawFrame::decode(&path).unwrap();
        assert_eq!(decoded.metadata.width, 64);
        assert_eq!(decoded.metadata.height, 64);
        assert_eq!(decoded.pixels, raw.pixels);
        let _ = std::fs::remove_dir_all(directory);
    }
}
