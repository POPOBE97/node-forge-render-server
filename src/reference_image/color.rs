use anyhow::{anyhow, bail};
use image::RgbaImage;

use super::envelope::Alpha;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngRgbSpace {
    DisplayP3,
    Srgb,
}


pub fn detect_png_rgb_space(png_bytes: &[u8]) -> anyhow::Result<PngRgbSpace> {
    Ok(inspect_png_rgb_space(png_bytes)?.unwrap_or(PngRgbSpace::Srgb))
}

pub fn require_png_rgb_space(png_bytes: &[u8]) -> anyhow::Result<PngRgbSpace> {
    inspect_png_rgb_space(png_bytes)?
        .ok_or_else(|| anyhow!("PNG has no recognized sRGB marker or embedded ICC profile"))
}

pub fn inspect_png_rgb_space(png_bytes: &[u8]) -> anyhow::Result<Option<PngRgbSpace>> {
    let reader = png::Decoder::new(std::io::Cursor::new(png_bytes))
        .read_info()
        .map_err(|error| anyhow!("failed to inspect PNG color profile: {error}"))?;
    let info = reader.info();
    if info.srgb.is_some() {
        return Ok(Some(PngRgbSpace::Srgb));
    }
    let Some(profile) = info.icc_profile.as_deref() else {
        return Ok(None);
    };
    detect_icc_rgb_space(profile)
        .map(Some)
        .ok_or_else(|| anyhow!("PNG uses an unsupported embedded RGB color profile"))
}

pub fn detect_icc_rgb_space(profile: &[u8]) -> Option<PngRgbSpace> {
    let red = icc_xyz_tag(profile, b"rXYZ")?;
    let green = icc_xyz_tag(profile, b"gXYZ")?;
    let blue = icc_xyz_tag(profile, b"bXYZ")?;

    const DISPLAY_P3: [[f32; 3]; 3] = [
        [0.5151, 0.2412, -0.0011],
        [0.2920, 0.6922, 0.0419],
        [0.1571, 0.0666, 0.7842],
    ];
    const SRGB: [[f32; 3]; 3] = [
        [0.4361, 0.2225, 0.0139],
        [0.3851, 0.7169, 0.0971],
        [0.1431, 0.0606, 0.7142],
    ];
    let matrix = [red, green, blue];
    if xyz_matrix_close(matrix, DISPLAY_P3) {
        Some(PngRgbSpace::DisplayP3)
    } else if xyz_matrix_close(matrix, SRGB) {
        Some(PngRgbSpace::Srgb)
    } else {
        None
    }
}

fn icc_xyz_tag(profile: &[u8], signature: &[u8; 4]) -> Option<[f32; 3]> {
    let tag_count = usize::try_from(read_be_u32(profile, 128)?).ok()?;
    let table_byte_len = tag_count.checked_mul(12)?;
    let table_end = 132usize.checked_add(table_byte_len)?;
    if table_end > profile.len() {
        return None;
    }
    for index in 0..tag_count {
        let entry = 132 + index * 12;
        if &profile[entry..entry + 4] != signature {
            continue;
        }
        let offset = usize::try_from(read_be_u32(profile, entry + 4)?).ok()?;
        let size = usize::try_from(read_be_u32(profile, entry + 8)?).ok()?;
        let end = offset.checked_add(size)?;
        if size < 20 || end > profile.len() || &profile[offset..offset + 4] != b"XYZ " {
            return None;
        }
        return Some([
            read_s15_fixed16(profile, offset + 8)?,
            read_s15_fixed16(profile, offset + 12)?,
            read_s15_fixed16(profile, offset + 16)?,
        ]);
    }
    None
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let raw: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(u32::from_be_bytes(raw))
}

fn read_s15_fixed16(bytes: &[u8], offset: usize) -> Option<f32> {
    let raw: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(i32::from_be_bytes(raw) as f32 / 65_536.0)
}

fn xyz_matrix_close(actual: [[f32; 3]; 3], expected: [[f32; 3]; 3]) -> bool {
    actual
        .iter()
        .flatten()
        .zip(expected.iter().flatten())
        .all(|(actual, expected)| (actual - expected).abs() <= 0.02)
}

pub use crate::color::linear_p3_to_linear_srgb;

pub fn display_p3_rgba8_to_srgb(rgba: &mut RgbaImage) {
    let decode_lut: [f32; 256] =
        std::array::from_fn(|value| crate::color::srgb_to_linear_channel(value as f32 / 255.0));
    for pixel in rgba.pixels_mut() {
        let p3 = [
            decode_lut[pixel[0] as usize],
            decode_lut[pixel[1] as usize],
            decode_lut[pixel[2] as usize],
        ];
        let srgb = linear_p3_to_linear_srgb(p3);
        pixel[0] = linear_srgb_to_u8(srgb[0]);
        pixel[1] = linear_srgb_to_u8(srgb[1]);
        pixel[2] = linear_srgb_to_u8(srgb[2]);
    }
}

pub fn linear_srgb_to_u8(linear: f32) -> u8 {
    let encoded = crate::color::linear_to_srgb_channel(linear.clamp(0.0, 1.0));
    (encoded * 255.0).round() as u8
}

pub fn encode_srgb_png(rgba: &RgbaImage) -> anyhow::Result<Vec<u8>> {
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, rgba.width(), rgba.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut writer = encoder
            .write_header()
            .map_err(|error| anyhow!("failed to encode sRGB PNG: {error}"))?;
        writer
            .write_image_data(rgba.as_raw())
            .map_err(|error| anyhow!("failed to write sRGB PNG: {error}"))?;
    }
    Ok(png_bytes)
}

pub fn encode_srgb_png_from_linear(
    width: u32,
    height: u32,
    linear_rgba: &[f32],
    alpha_mode: Alpha,
) -> anyhow::Result<Vec<u8>> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("PNG size overflow"))? as usize;
    if linear_rgba.len() != expected {
        bail!(
            "linear PNG source has {} values, expected {expected}",
            linear_rgba.len()
        );
    }
    let mut rgba = Vec::with_capacity(expected);
    for pixel in linear_rgba.chunks_exact(4) {
        let alpha = pixel[3].clamp(0.0, 1.0);
        let rgb = straight_linear_rgb(pixel, alpha_mode, alpha);
        rgba.push(linear_srgb_to_u8(rgb[0]));
        rgba.push(linear_srgb_to_u8(rgb[1]));
        rgba.push(linear_srgb_to_u8(rgb[2]));
        rgba.push((alpha * 255.0).round() as u8);
    }
    let image = RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| anyhow!("failed to pack sRGB PNG pixels"))?;
    encode_srgb_png(&image)
}

pub fn preview_rgba8_from_linear(linear_rgba: &[f32], alpha_mode: Alpha) -> Vec<u8> {
    let mut preview = Vec::with_capacity(linear_rgba.len());
    for pixel in linear_rgba.chunks_exact(4) {
        let alpha = pixel[3].clamp(0.0, 1.0);
        let rgb = straight_linear_rgb(pixel, alpha_mode, alpha);
        preview.push(linear_srgb_to_u8(rgb[0]));
        preview.push(linear_srgb_to_u8(rgb[1]));
        preview.push(linear_srgb_to_u8(rgb[2]));
        preview.push((alpha * 255.0).round() as u8);
    }
    preview
}

fn straight_linear_rgb(pixel: &[f32], alpha_mode: Alpha, alpha: f32) -> [f32; 3] {
    if alpha_mode == Alpha::Premultiplied {
        if alpha > 0.0 {
            [pixel[0] / alpha, pixel[1] / alpha, pixel[2] / alpha]
        } else {
            [0.0, 0.0, 0.0]
        }
    } else {
        [pixel[0], pixel[1], pixel[2]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_p3_rgba8_conversion_matches_reference_values() {
        let mut rgba =
            image::RgbaImage::from_raw(2, 1, vec![128, 128, 128, 255, 255, 106, 68, 17]).unwrap();
        display_p3_rgba8_to_srgb(&mut rgba);
        assert_eq!(rgba.as_raw(), &[128, 128, 128, 255, 255, 92, 51, 17]);
    }

    #[test]
    fn figma_srgb_png_has_an_srgb_chunk_and_no_icc_profile() {
        let rgba = image::RgbaImage::from_raw(1, 1, vec![255, 92, 51, 255]).unwrap();
        let png_bytes = encode_srgb_png(&rgba).unwrap();
        let reader = png::Decoder::new(std::io::Cursor::new(&png_bytes))
            .read_info()
            .unwrap();
        assert_eq!(
            reader.info().srgb,
            Some(png::SrgbRenderingIntent::Perceptual)
        );
        assert!(reader.info().icc_profile.is_none());
        assert!(png_bytes.windows(4).any(|bytes| bytes == b"sRGB"));
        assert!(!png_bytes.windows(4).any(|bytes| bytes == b"iCCP"));
    }

    #[test]
    fn linear_p3_red_is_outside_srgb_without_clamping() {
        let [r, g, b] = linear_p3_to_linear_srgb([1.0, 0.0, 0.0]);
        assert!(r > 1.0, "r={r}");
        assert!(g < 0.0, "g={g}");
        assert!(b < 0.0, "b={b}");
    }

    #[test]
    fn premultiplied_linear_output_is_unpremultiplied_for_straight_png_and_preview() {
        let source = [0.25, 0.125, 0.0, 0.5];
        let preview = preview_rgba8_from_linear(&source, Alpha::Premultiplied);
        assert_eq!(preview, vec![188, 137, 0, 128]);

        let png = encode_srgb_png_from_linear(1, 1, &source, Alpha::Premultiplied).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().unwrap();
        let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut bytes).unwrap();
        assert_eq!(&bytes[..frame.buffer_size()], preview.as_slice());
    }

    #[test]
    fn zero_alpha_premultiplied_output_has_deterministic_black_rgb() {
        assert_eq!(
            preview_rgba8_from_linear(&[10.0, -2.0, 1.0, 0.0], Alpha::Premultiplied),
            vec![0, 0, 0, 0]
        );
    }
}
