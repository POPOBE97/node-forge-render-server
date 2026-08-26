use anyhow::{anyhow, bail};
use image::RgbaImage;

use crate::color::srgb_to_linear_channel;

use super::{
    color::{
        PngRgbSpace, detect_png_rgb_space, encode_srgb_png, linear_p3_to_linear_srgb,
        preview_rgba8_from_linear,
    },
    envelope::{
        Alpha, PayloadKind, ReferenceImageEnvelope, Transfer, f32_to_rgba16f, parse_envelope,
        rgba16f_to_f32,
    },
    gainmap,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferencePayloadKind {
    RawFp16,
    GainmapPng,
    SdrPng,
}

#[derive(Clone, Debug)]
pub struct DecodedReferencePixels {
    pub width: u32,
    pub height: u32,
    pub source_linear_rgba: Vec<f32>,
    pub preview_rgba8: Vec<u8>,
    pub alpha: Alpha,
    pub high_precision: bool,
    pub payload_kind: ReferencePayloadKind,
    pub encoded_source: Option<Vec<u8>>,
}

impl DecodedReferencePixels {
    pub fn linear_premul_rgba(&self) -> Vec<f32> {
        apply_alpha_mode(&self.source_linear_rgba, self.alpha)
    }

    pub fn max_rgb(&self) -> f32 {
        self.source_linear_rgba
            .chunks_exact(4)
            .flat_map(|pixel| pixel[..3].iter().copied())
            .fold(f32::NEG_INFINITY, f32::max)
    }
}

pub fn apply_alpha_mode(source_linear_rgba: &[f32], alpha: Alpha) -> Vec<f32> {
    let mut linear_premul_rgba = source_linear_rgba.to_vec();
    if matches!(alpha, Alpha::Straight) {
        for rgba in linear_premul_rgba.chunks_exact_mut(4) {
            rgba[0] *= rgba[3];
            rgba[1] *= rgba[3];
            rgba[2] *= rgba[3];
        }
    }
    linear_premul_rgba
}

pub fn decode_linear_f32(
    width: u32,
    height: u32,
    source_linear_rgba: Vec<f32>,
    alpha: Alpha,
    payload_kind: ReferencePayloadKind,
    high_precision: bool,
) -> anyhow::Result<DecodedReferencePixels> {
    let expected = pixel_count(width, height)?;
    if source_linear_rgba.len() != expected {
        bail!(
            "linear reference has {} values, expected {expected}",
            source_linear_rgba.len()
        );
    }
    for (index, value) in source_linear_rgba.iter().enumerate() {
        if !value.is_finite() {
            bail!("linear reference contains a non-finite value at {index}");
        }
    }
    Ok(DecodedReferencePixels {
        width,
        height,
        preview_rgba8: preview_rgba8_from_linear(&source_linear_rgba, alpha),
        alpha,
        high_precision,
        source_linear_rgba,
        payload_kind,
        encoded_source: None,
    })
}

pub fn quantize_helper_sdr(
    decoded: DecodedReferencePixels,
) -> anyhow::Result<DecodedReferencePixels> {
    let mut quantized = Vec::with_capacity(decoded.source_linear_rgba.len());
    for pixel in decoded.source_linear_rgba.chunks_exact(4) {
        quantized.push(quantize_u8(pixel[0].clamp(0.0, 1.0)));
        quantized.push(quantize_u8(pixel[1].clamp(0.0, 1.0)));
        quantized.push(quantize_u8(pixel[2].clamp(0.0, 1.0)));
        quantized.push(pixel[3].clamp(0.0, 1.0));
    }
    decode_linear_f32(
        decoded.width,
        decoded.height,
        quantized,
        decoded.alpha,
        ReferencePayloadKind::SdrPng,
        false,
    )
}

fn quantize_u8(linear: f32) -> f32 {
    (linear * 255.0).round() / 255.0
}

pub fn decode_private_envelope(bytes: &[u8]) -> anyhow::Result<DecodedReferencePixels> {
    let envelope = parse_envelope(bytes)?;
    match envelope.payload_kind {
        PayloadKind::LinearRgba16F => decode_raw_envelope(envelope),
        PayloadKind::EncodedPng => decode_png_bytes(&envelope.primary),
    }
}

fn decode_raw_envelope(envelope: ReferenceImageEnvelope) -> anyhow::Result<DecodedReferencePixels> {
    let linear = rgba16f_to_f32(&envelope.primary)?;
    let decoded = decode_linear_f32(
        envelope.width,
        envelope.height,
        linear,
        envelope.alpha,
        ReferencePayloadKind::RawFp16,
        true,
    )?;
    if envelope.captured_hdr_layers {
        Ok(decoded)
    } else {
        quantize_helper_sdr(decoded)
    }
}

pub fn decode_png_bytes(bytes: &[u8]) -> anyhow::Result<DecodedReferencePixels> {
    let space = detect_png_rgb_space(bytes)?;
    let (width, height, encoded_rgba) = decode_png_rgba(bytes)?;
    let mut linear = encoded_rgba_to_linear(&encoded_rgba, space);
    if let Some(hdr) = gainmap::reconstruct_if_present(bytes, &linear, width, height, space)? {
        linear = hdr;
        let mut decoded = decode_linear_f32(
            width,
            height,
            linear,
            Alpha::Straight,
            ReferencePayloadKind::GainmapPng,
            true,
        )?;
        decoded.encoded_source = Some(bytes.to_vec());
        return Ok(decoded);
    }
    let mut quantized = Vec::with_capacity(linear.len());
    for pixel in linear.chunks_exact(4) {
        quantized.push(quantize_u8(pixel[0].clamp(0.0, 1.0)));
        quantized.push(quantize_u8(pixel[1].clamp(0.0, 1.0)));
        quantized.push(quantize_u8(pixel[2].clamp(0.0, 1.0)));
        quantized.push(quantize_u8(pixel[3].clamp(0.0, 1.0)));
    }
    let mut decoded = decode_linear_f32(
        width,
        height,
        quantized,
        Alpha::Straight,
        ReferencePayloadKind::SdrPng,
        false,
    )?;
    decoded.encoded_source = Some(bytes.to_vec());
    Ok(decoded)
}

pub fn decode_png_sdr_base_rgba8(bytes: &[u8]) -> anyhow::Result<(RgbaImage, PngRgbSpace)> {
    let space = super::color::require_png_rgb_space(bytes)?;
    let (width, height, encoded_rgba) = decode_png_rgba(bytes)?;
    let rgba8 = encoded_rgba
        .into_iter()
        .map(|sample| (sample.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    let image = RgbaImage::from_raw(width, height, rgba8)
        .ok_or_else(|| anyhow!("failed to pack SDR base PNG pixels"))?;
    Ok((image, space))
}

pub fn figma_srgb_png_from_source(
    source_png: &[u8],
    raw_linear: Option<&DecodedReferencePixels>,
) -> anyhow::Result<Vec<u8>> {
    match decode_png_sdr_base_rgba8(source_png) {
        Ok((mut rgba, space)) => {
            if space == PngRgbSpace::DisplayP3 {
                super::color::display_p3_rgba8_to_srgb(&mut rgba);
            }
            encode_srgb_png(&rgba)
        }
        Err(error) => {
            if let Some(raw) = raw_linear {
                eprintln!(
                    "[android-screencap] public sRGB PNG synthesized from raw capture after unsupported profile: {error:#}"
                );
                super::color::encode_srgb_png_from_linear(
                    raw.width,
                    raw.height,
                    &raw.source_linear_rgba,
                    raw.alpha,
                )
            } else {
                Err(error)
            }
        }
    }
}

fn decode_png_rgba(bytes: &[u8]) -> anyhow::Result<(u32, u32, Vec<f32>)> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|error| anyhow!("failed to decode PNG: {error}"))?;
    let mut buf = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| anyhow!("PNG output buffer is too large"))?
    ];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|error| anyhow!("failed to read PNG frame: {error}"))?;
    let info = reader.info();
    let width = info.width;
    let height = info.height;
    let rgba = expand_png_frame_to_rgba_f32(&buf[..frame.buffer_size()], frame, info)?;
    Ok((width, height, rgba))
}

fn expand_png_frame_to_rgba_f32(
    buffer: &[u8],
    frame: png::OutputInfo,
    info: &png::Info,
) -> anyhow::Result<Vec<f32>> {
    let pixel_count = frame.width as usize * frame.height as usize;
    match (frame.color_type, frame.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => {
            Ok(buffer.iter().map(|sample| *sample as f32 / 255.0).collect())
        }
        (png::ColorType::Rgb, png::BitDepth::Eight) => Ok(buffer
            .chunks_exact(3)
            .flat_map(|pixel| {
                [
                    pixel[0] as f32 / 255.0,
                    pixel[1] as f32 / 255.0,
                    pixel[2] as f32 / 255.0,
                    1.0,
                ]
            })
            .collect()),
        (png::ColorType::Grayscale, png::BitDepth::Eight) => Ok(buffer
            .iter()
            .flat_map(|sample| {
                let sample = *sample as f32 / 255.0;
                [sample, sample, sample, 1.0]
            })
            .collect()),
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => Ok(buffer
            .chunks_exact(2)
            .flat_map(|pixel| {
                let gray = pixel[0] as f32 / 255.0;
                [gray, gray, gray, pixel[1] as f32 / 255.0]
            })
            .collect()),
        (png::ColorType::Rgba, png::BitDepth::Sixteen) => Ok(buffer
            .chunks_exact(8)
            .flat_map(|pixel| {
                [
                    u16::from_be_bytes([pixel[0], pixel[1]]) as f32 / 65_535.0,
                    u16::from_be_bytes([pixel[2], pixel[3]]) as f32 / 65_535.0,
                    u16::from_be_bytes([pixel[4], pixel[5]]) as f32 / 65_535.0,
                    u16::from_be_bytes([pixel[6], pixel[7]]) as f32 / 65_535.0,
                ]
            })
            .collect()),
        (png::ColorType::Rgb, png::BitDepth::Sixteen) => Ok(buffer
            .chunks_exact(6)
            .flat_map(|pixel| {
                [
                    u16::from_be_bytes([pixel[0], pixel[1]]) as f32 / 65_535.0,
                    u16::from_be_bytes([pixel[2], pixel[3]]) as f32 / 65_535.0,
                    u16::from_be_bytes([pixel[4], pixel[5]]) as f32 / 65_535.0,
                    1.0,
                ]
            })
            .collect()),
        (png::ColorType::Grayscale, png::BitDepth::Sixteen) => Ok(buffer
            .chunks_exact(2)
            .flat_map(|sample| {
                let gray = u16::from_be_bytes([sample[0], sample[1]]) as f32 / 65_535.0;
                [gray, gray, gray, 1.0]
            })
            .collect()),
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Sixteen) => Ok(buffer
            .chunks_exact(4)
            .flat_map(|pixel| {
                let gray = u16::from_be_bytes([pixel[0], pixel[1]]) as f32 / 65_535.0;
                let alpha = u16::from_be_bytes([pixel[2], pixel[3]]) as f32 / 65_535.0;
                [gray, gray, gray, alpha]
            })
            .collect()),
        (png::ColorType::Indexed, png::BitDepth::Eight) => {
            let palette = info
                .palette
                .as_ref()
                .ok_or_else(|| anyhow!("indexed PNG is missing a palette"))?;
            let mut rgba = Vec::with_capacity(pixel_count * 4);
            for index in buffer {
                let entry = usize::from(*index) * 3;
                let alpha = info
                    .trns
                    .as_deref()
                    .and_then(|alpha| alpha.get(usize::from(*index)))
                    .copied()
                    .unwrap_or(255);
                rgba.extend_from_slice(&[
                    palette[entry] as f32 / 255.0,
                    palette[entry + 1] as f32 / 255.0,
                    palette[entry + 2] as f32 / 255.0,
                    alpha as f32 / 255.0,
                ]);
            }
            Ok(rgba)
        }
        other => bail!("unsupported PNG color type {other:?}"),
    }
}

fn encoded_rgba_to_linear(encoded: &[f32], space: PngRgbSpace) -> Vec<f32> {
    encoded
        .chunks_exact(4)
        .flat_map(|pixel| {
            let encoded_rgb = [pixel[0], pixel[1], pixel[2]];
            let linear = [
                srgb_to_linear_channel(encoded_rgb[0]),
                srgb_to_linear_channel(encoded_rgb[1]),
                srgb_to_linear_channel(encoded_rgb[2]),
            ];
            let rgb = match space {
                PngRgbSpace::Srgb => linear,
                PngRgbSpace::DisplayP3 => linear_p3_to_linear_srgb(linear),
            };
            [rgb[0], rgb[1], rgb[2], pixel[3]]
        })
        .collect()
}

fn pixel_count(width: u32, height: u32) -> anyhow::Result<usize> {
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .map(|count| count as usize)
        .ok_or_else(|| anyhow!("reference image is too large"))
}

pub fn envelope_from_raw_fp16(
    width: u32,
    height: u32,
    linear_premul: &[f32],
    captured_hdr_layers: bool,
    source_png: Option<Vec<u8>>,
) -> anyhow::Result<ReferenceImageEnvelope> {
    Ok(ReferenceImageEnvelope {
        captured_hdr_layers,
        payload_kind: PayloadKind::LinearRgba16F,
        transfer: Transfer::LinearScRgb,
        alpha: Alpha::Premultiplied,
        width,
        height,
        bytes_per_row: width
            .checked_mul(8)
            .ok_or_else(|| anyhow!("raw envelope row stride overflow"))?,
        primary: f32_to_rgba16f(linear_premul)?,
        source_png,
    })
}

pub fn envelope_from_encoded_png(
    width: u32,
    height: u32,
    png: Vec<u8>,
    captured_hdr_layers: bool,
) -> ReferenceImageEnvelope {
    ReferenceImageEnvelope {
        captured_hdr_layers,
        payload_kind: PayloadKind::EncodedPng,
        transfer: Transfer::EncodedProfiled,
        alpha: Alpha::Straight,
        width,
        height,
        bytes_per_row: 0,
        primary: png,
        source_png: None,
    }
}

pub fn payload_kind_label(kind: ReferencePayloadKind) -> &'static str {
    match kind {
        ReferencePayloadKind::RawFp16 => "raw-fp16",
        ReferencePayloadKind::GainmapPng => "gainmap-png",
        ReferencePayloadKind::SdrPng => "sdr-png",
    }
}

pub fn is_png(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes[0..8] == super::png_chunks::PNG_SIGNATURE
}

pub fn is_exr(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0..4] == [0x76, 0x2f, 0x31, 0x01]
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::reference_image::color::encode_srgb_png;
    use image::RgbaImage;

    fn display_p3_icc_profile() -> Vec<u8> {
        let mut profile = vec![0u8; 228];
        let profile_len = profile.len() as u32;
        profile[0..4].copy_from_slice(&profile_len.to_be_bytes());
        profile[128..132].copy_from_slice(&3u32.to_be_bytes());
        let tags = [
            (b"rXYZ", 168usize, [0.5151f32, 0.2412, -0.0011]),
            (b"gXYZ", 188usize, [0.2920f32, 0.6922, 0.0419]),
            (b"bXYZ", 208usize, [0.1571f32, 0.0666, 0.7842]),
        ];
        for (index, (signature, offset, xyz)) in tags.into_iter().enumerate() {
            let entry = 132 + index * 12;
            profile[entry..entry + 4].copy_from_slice(signature);
            profile[entry + 4..entry + 8].copy_from_slice(&(offset as u32).to_be_bytes());
            profile[entry + 8..entry + 12].copy_from_slice(&20u32.to_be_bytes());
            profile[offset..offset + 4].copy_from_slice(b"XYZ ");
            for (channel, value) in xyz.into_iter().enumerate() {
                let fixed = (value * 65_536.0).round() as i32;
                let start = offset + 8 + channel * 4;
                profile[start..start + 4].copy_from_slice(&fixed.to_be_bytes());
            }
        }
        profile
    }

    fn display_p3_red_png() -> Vec<u8> {
        let mut info = png::Info::with_size(1, 1);
        info.color_type = png::ColorType::Rgba;
        info.bit_depth = png::BitDepth::Eight;
        info.icc_profile = Some(Cow::Owned(display_p3_icc_profile()));
        let mut bytes = Vec::new();
        let encoder = png::Encoder::with_info(&mut bytes, info).unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[255, 0, 0, 255]).unwrap();
        drop(writer);
        bytes
    }

    #[test]
    fn raw_hdr_envelope_preserves_values_above_one() {
        let linear = vec![1.5, -0.25, 0.4, 1.0];
        let envelope = envelope_from_raw_fp16(1, 1, &linear, true, None)
            .unwrap()
            .encode()
            .unwrap();
        let decoded = decode_private_envelope(&envelope).unwrap();
        assert_eq!(decoded.payload_kind, ReferencePayloadKind::RawFp16);
        assert!(decoded.high_precision);
        assert!(decoded.source_linear_rgba[0] > 1.0);
        assert!(decoded.source_linear_rgba[1] < 0.0);
        assert_eq!(decoded.alpha, Alpha::Premultiplied);
    }

    #[test]
    fn helper_sdr_quantizes_the_same_raw_frame() {
        let linear = vec![1.5, 0.2, 0.0, 0.5];
        let envelope = envelope_from_raw_fp16(1, 1, &linear, false, None)
            .unwrap()
            .encode()
            .unwrap();
        let decoded = decode_private_envelope(&envelope).unwrap();
        assert!(!decoded.high_precision);
        assert!(decoded.source_linear_rgba[0] <= 1.0);
        assert_eq!(decoded.alpha, Alpha::Premultiplied);
    }

    #[test]
    fn ordinary_srgb_png_is_unchanged_8bit_path() {
        let png =
            encode_srgb_png(&RgbaImage::from_raw(1, 1, vec![128, 0, 0, 255]).unwrap()).unwrap();
        let decoded = decode_png_bytes(&png).unwrap();
        assert_eq!(decoded.payload_kind, ReferencePayloadKind::SdrPng);
        assert!(!decoded.high_precision);
        assert!(decoded.source_linear_rgba[0] < 0.3);
    }

    #[test]
    fn ordinary_display_p3_sdr_stays_on_the_8bit_path() {
        let decoded = decode_png_bytes(&display_p3_red_png()).unwrap();
        assert_eq!(decoded.payload_kind, ReferencePayloadKind::SdrPng);
        assert!(!decoded.high_precision);
        assert!(
            decoded.source_linear_rgba[..3]
                .iter()
                .all(|channel| (0.0..=1.0).contains(channel))
        );
    }

    #[test]
    fn png16_samples_keep_low_bits_until_the_explicit_sdr_quantization_stage() {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Sixteen);
            encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
            let mut writer = encoder.write_header().unwrap();
            let samples = [0x1200u16, 0, 0, 0xffff, 0x1201, 0, 0, 0xffff];
            let bytes = samples
                .into_iter()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<_>>();
            writer.write_image_data(&bytes).unwrap();
        }

        let (_, _, encoded) = decode_png_rgba(&png).unwrap();
        assert_ne!(encoded[0], encoded[4]);
        assert!((encoded[4] - encoded[0] - 1.0 / 65_535.0).abs() < 1e-6);

        let decoded = decode_png_bytes(&png).unwrap();
        assert_eq!(decoded.payload_kind, ReferencePayloadKind::SdrPng);
        assert!(!decoded.high_precision);
    }

    #[test]
    fn alpha_mode_toggle_does_not_double_premultiply_from_source() {
        let source = vec![0.8, 0.6, 0.4, 0.5];
        assert_eq!(apply_alpha_mode(&source, Alpha::Premultiplied), source);
        assert_eq!(
            apply_alpha_mode(&source, Alpha::Straight),
            vec![0.4, 0.3, 0.2, 0.5]
        );
        let again = apply_alpha_mode(&source, Alpha::Straight);
        assert_eq!(again, vec![0.4, 0.3, 0.2, 0.5]);
    }

    #[test]
    fn malformed_private_envelope_fails_closed() {
        assert!(decode_private_envelope(b"not-an-envelope").is_err());
    }
}
