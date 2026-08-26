use anyhow::{anyhow, bail};

use super::{
    color::{PngRgbSpace, detect_png_rgb_space, linear_p3_to_linear_srgb},
    iso21496::{self, BaseImageType, GainmapInfo},
    png_chunks::{find_unique_chunk, parse_png_chunks},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GainmapLayout {
    Gray,
    GrayAlpha,
    Rgb,
    Rgba,
}

struct GainmapImage {
    width: u32,
    height: u32,
    layout: GainmapLayout,
    pixels: Vec<f32>,
    space: PngRgbSpace,
}

pub fn reconstruct_if_present(
    png_bytes: &[u8],
    base_linear: &[f32],
    width: u32,
    height: u32,
    base_space: PngRgbSpace,
) -> anyhow::Result<Option<Vec<f32>>> {
    let parsed = parse_png_chunks(png_bytes)?;
    let gmap = find_unique_chunk(&parsed, b"gmAP")?;
    let gdat = find_unique_chunk(&parsed, b"gdAT")?;
    match (gmap, gdat) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => {
            bail!("PNG has an incomplete gmAP/gdAT pair")
        }
        (Some(gmap), Some(gdat)) => {
            iso21496::parse_version(&gmap.data)?;
            let gainmap_png = &gdat.data;
            let nested = parse_png_chunks(gainmap_png)
                .map_err(|error| anyhow!("embedded gdAT PNG is malformed: {error}"))?;
            let nested_gmap = find_unique_chunk(&nested, b"gmAP")?
                .ok_or_else(|| anyhow!("embedded gainmap PNG is missing gmAP metadata"))?;
            let info = iso21496::parse_info(&nested_gmap.data)?;
            let image = decode_gainmap_png(gainmap_png)?;
            Ok(Some(apply_gainmap(
                base_linear,
                width,
                height,
                base_space,
                &image,
                &info,
            )?))
        }
    }
}

fn decode_gainmap_png(bytes: &[u8]) -> anyhow::Result<GainmapImage> {
    let space = detect_png_rgb_space(bytes)?;
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|error| anyhow!("failed to decode gainmap PNG: {error}"))?;
    let mut buf = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| anyhow!("gainmap PNG output buffer is too large"))?
    ];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|error| anyhow!("failed to read gainmap PNG frame: {error}"))?;
    let info = reader.info();
    let width = info.width;
    let height = info.height;
    let (layout, channels) = match (frame.color_type, frame.bit_depth) {
        (png::ColorType::Grayscale, png::BitDepth::Eight) => (GainmapLayout::Gray, 1usize),
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => (GainmapLayout::GrayAlpha, 2),
        (png::ColorType::Rgb, png::BitDepth::Eight) => (GainmapLayout::Rgb, 3),
        (png::ColorType::Rgba, png::BitDepth::Eight) => (GainmapLayout::Rgba, 4),
        (png::ColorType::Grayscale, png::BitDepth::Sixteen) => (GainmapLayout::Gray, 1),
        (png::ColorType::Rgba, png::BitDepth::Sixteen) => (GainmapLayout::Rgba, 4),
        other => bail!("unsupported gainmap PNG color type {other:?}"),
    };
    let layout = match (layout, info.trns.is_some()) {
        (GainmapLayout::Gray, true)
            if channels == 1 && info.color_type == png::ColorType::Grayscale =>
        {
            // Keep gray; tRNS is not used as A8.
            GainmapLayout::Gray
        }
        _ => layout,
    };
    let layout = if info.color_type == png::ColorType::Grayscale
        && channels == 1
        && frame.buffer_size() == (width * height) as usize
    {
        // Distinguish true A8 from gray: PNG has no standalone A8; Android writes gray or gray-alpha.
        layout
    } else {
        layout
    };

    let samples = match frame.bit_depth {
        png::BitDepth::Eight => buf[..frame.buffer_size()]
            .iter()
            .map(|sample| *sample as f32 / 255.0)
            .collect::<Vec<_>>(),
        png::BitDepth::Sixteen => buf[..frame.buffer_size()]
            .chunks_exact(2)
            .map(|sample| u16::from_be_bytes([sample[0], sample[1]]) as f32 / 65_535.0)
            .collect::<Vec<_>>(),
        _ => bail!("unsupported gainmap PNG bit depth"),
    };

    // Treat single-channel gray as A8 when the PNG is grayscale with no colorimetric intent
    // other than a ratio map. Skia uses alpha for A8 and red for gray; both yield the same
    // numeric channel after our expansion.
    let layout = if matches!(info.color_type, png::ColorType::Grayscale) {
        GainmapLayout::Gray
    } else {
        layout
    };

    Ok(GainmapImage {
        width,
        height,
        layout,
        pixels: samples,
        space,
    })
}

fn apply_gainmap(
    base_linear: &[f32],
    width: u32,
    height: u32,
    base_space: PngRgbSpace,
    gainmap: &GainmapImage,
    info: &GainmapInfo,
) -> anyhow::Result<Vec<f32>> {
    if base_linear.len() != width as usize * height as usize * 4 {
        bail!("base image size does not match linear pixel buffer");
    }
    let three_channel =
        info.multi_channel && matches!(gainmap.layout, GainmapLayout::Rgb | GainmapLayout::Rgba);
    let working_space = if three_channel {
        gainmap.space
    } else if info.use_base_colour_space {
        base_space
    } else {
        PngRgbSpace::Srgb
    };

    let mut hdr = vec![0.0; base_linear.len()];
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) as usize * 4;
            let mut s = [base_linear[idx], base_linear[idx + 1], base_linear[idx + 2]];
            s = convert_space(s, base_space, working_space);
            let g = sample_gainmap(gainmap, x, y, width, height, three_channel);
            let mut h = [0.0; 3];
            for channel in 0..3 {
                let sample = if three_channel { g[channel] } else { g[0] };
                let no_gamma = (info.gainmap_gamma[channel] - 1.0).abs() <= 1e-5;
                let mixed = if no_gamma {
                    sample
                } else {
                    sample.powf(info.gainmap_gamma[channel])
                };
                let log_min = info.gainmap_ratio_min[channel].ln();
                let log_max = info.gainmap_ratio_max[channel].ln();
                let l = log_min + (log_max - log_min) * mixed;
                let (epsilon_base, epsilon_other, w) = match info.base_image_type {
                    BaseImageType::Sdr => {
                        (info.epsilon_sdr[channel], info.epsilon_hdr[channel], 1.0)
                    }
                    BaseImageType::Hdr => {
                        (info.epsilon_hdr[channel], info.epsilon_sdr[channel], 0.0)
                    }
                };
                h[channel] = (s[channel] + epsilon_base) * (l * w).exp() - epsilon_other;
                if !h[channel].is_finite() {
                    bail!("gainmap reconstruction produced a non-finite HDR value");
                }
            }
            let linear_srgb = convert_space(h, working_space, PngRgbSpace::Srgb);
            hdr[idx] = linear_srgb[0];
            hdr[idx + 1] = linear_srgb[1];
            hdr[idx + 2] = linear_srgb[2];
            hdr[idx + 3] = base_linear[idx + 3];
            if !hdr[idx + 3].is_finite() {
                bail!("gainmap reconstruction produced a non-finite alpha");
            }
        }
    }
    Ok(hdr)
}

fn convert_space(rgb: [f32; 3], from: PngRgbSpace, to: PngRgbSpace) -> [f32; 3] {
    match (from, to) {
        (PngRgbSpace::DisplayP3, PngRgbSpace::Srgb) => linear_p3_to_linear_srgb(rgb),
        (PngRgbSpace::Srgb, PngRgbSpace::DisplayP3) => linear_srgb_to_linear_p3(rgb),
        _ => rgb,
    }
}

fn linear_srgb_to_linear_p3([r, g, b]: [f32; 3]) -> [f32; 3] {
    [
        0.822_461_97 * r + 0.177_538_03 * g + 0.0 * b,
        0.033_194_2 * r + 0.966_805_8 * g + 0.0 * b,
        0.017_087_95 * r + 0.072_397_44 * g + 0.910_514_6 * b,
    ]
}

fn sample_gainmap(
    gainmap: &GainmapImage,
    x: u32,
    y: u32,
    base_width: u32,
    base_height: u32,
    three_channel: bool,
) -> [f32; 3] {
    let u = (x as f32 + 0.5) * gainmap.width as f32 / base_width as f32 - 0.5;
    let v = (y as f32 + 0.5) * gainmap.height as f32 / base_height as f32 - 0.5;
    let channels = gainmap_channel_count(gainmap.layout);
    let pixel = bilinear(
        &gainmap.pixels,
        gainmap.width,
        gainmap.height,
        channels,
        u,
        v,
    );
    select_gainmap_channels(gainmap.layout, pixel, three_channel)
}

fn gainmap_channel_count(layout: GainmapLayout) -> usize {
    match layout {
        GainmapLayout::Gray => 1,
        GainmapLayout::GrayAlpha => 2,
        GainmapLayout::Rgb => 3,
        GainmapLayout::Rgba => 4,
    }
}

fn select_gainmap_channels(
    layout: GainmapLayout,
    pixel: [f32; 4],
    three_channel: bool,
) -> [f32; 3] {
    match layout {
        GainmapLayout::Gray => [pixel[0], pixel[0], pixel[0]],
        GainmapLayout::GrayAlpha => [pixel[0], pixel[0], pixel[0]],
        GainmapLayout::Rgb if three_channel => [pixel[0], pixel[1], pixel[2]],
        GainmapLayout::Rgb => [pixel[0], pixel[0], pixel[0]],
        GainmapLayout::Rgba if three_channel => [pixel[0], pixel[1], pixel[2]],
        // Android/Skia serializes an A8 single-channel gainmap as unpremultiplied
        // white RGBA: RGB is a binary coverage mask while A carries the gain value.
        GainmapLayout::Rgba => [pixel[3], pixel[3], pixel[3]],
    }
}

fn bilinear(pixels: &[f32], width: u32, height: u32, channels: usize, x: f32, y: f32) -> [f32; 4] {
    let max_x = (width.saturating_sub(1)) as f32;
    let max_y = (height.saturating_sub(1)) as f32;
    let x = x.clamp(0.0, max_x);
    let y = y.clamp(0.0, max_y);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(width.saturating_sub(1));
    let y1 = (y0 + 1).min(height.saturating_sub(1));
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let p00 = fetch(pixels, width, channels, x0, y0);
    let p10 = fetch(pixels, width, channels, x1, y0);
    let p01 = fetch(pixels, width, channels, x0, y1);
    let p11 = fetch(pixels, width, channels, x1, y1);
    let mut out = [0.0; 4];
    for i in 0..4 {
        let top = p00[i] + (p10[i] - p00[i]) * tx;
        let bottom = p01[i] + (p11[i] - p01[i]) * tx;
        out[i] = top + (bottom - top) * ty;
    }
    out
}

fn fetch(pixels: &[f32], width: u32, channels: usize, x: u32, y: u32) -> [f32; 4] {
    let idx = (y * width + x) as usize * channels;
    let mut out = [0.0; 4];
    for channel in 0..channels {
        out[channel] = pixels[idx + channel];
    }
    if channels == 1 {
        out[1] = out[0];
        out[2] = out[0];
        out[3] = 1.0;
    }
    out
}

pub fn has_gainmap_pair(png_bytes: &[u8]) -> anyhow::Result<bool> {
    let parsed = parse_png_chunks(png_bytes)?;
    Ok(find_unique_chunk(&parsed, b"gmAP")?.is_some()
        && find_unique_chunk(&parsed, b"gdAT")?.is_some())
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    fn rgba_png_with_icc(profile: Vec<u8>) -> Vec<u8> {
        let mut info = png::Info::with_size(1, 1);
        info.color_type = png::ColorType::Rgba;
        info.bit_depth = png::BitDepth::Eight;
        info.icc_profile = Some(Cow::Owned(profile));
        let mut bytes = Vec::new();
        let encoder = png::Encoder::with_info(&mut bytes, info).unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0, 0, 0, 255]).unwrap();
        drop(writer);
        bytes
    }

    #[test]
    fn embedded_gainmap_rejects_an_unsupported_icc_profile() {
        let png = rgba_png_with_icc(vec![0; 256]);
        let error = decode_gainmap_png(&png).err().unwrap();
        assert!(
            error
                .to_string()
                .contains("unsupported embedded RGB color profile")
        );
    }
    use crate::reference_image::{
        color::encode_srgb_png,
        iso21496::{self, GainmapInfo},
        png_chunks::encode_png_with_chunks,
    };
    use image::RgbaImage;

    fn encode_gray_png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, width, height);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(pixels).unwrap();
        }
        png
    }

    fn encode_color_png(color: png::ColorType, width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, width, height);
            encoder.set_color(color);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(pixels).unwrap();
        }
        png
    }

    fn gainmap_png(info: &GainmapInfo, image_png: Vec<u8>) -> Vec<u8> {
        encode_png_with_chunks(&image_png, &[(b"gmAP", &iso21496::serialize_info(info))]).unwrap()
    }

    fn ultra_hdr_png(base: &[u8], gainmap: &[u8]) -> Vec<u8> {
        encode_png_with_chunks(
            base,
            &[
                (b"gmAP", &iso21496::serialize_version()),
                (b"gdAT", gainmap),
            ],
        )
        .unwrap()
    }

    #[test]
    fn reconstructs_values_above_one_from_synthetic_gainmap() {
        let mut info = iso21496::tests_sdr_info();
        info.display_ratio_hdr = 4.0;
        info.gainmap_ratio_max = [4.0, 4.0, 4.0];
        let base =
            encode_srgb_png(&RgbaImage::from_raw(1, 1, vec![186, 186, 186, 255]).unwrap()).unwrap();
        let map = gainmap_png(&info, encode_gray_png(1, 1, &[255]));
        let hdr_png = ultra_hdr_png(&base, &map);
        let base_linear = vec![
            crate::color::srgb_to_linear_channel(186.0 / 255.0),
            crate::color::srgb_to_linear_channel(186.0 / 255.0),
            crate::color::srgb_to_linear_channel(186.0 / 255.0),
            1.0,
        ];
        let hdr = reconstruct_if_present(&hdr_png, &base_linear, 1, 1, PngRgbSpace::Srgb)
            .unwrap()
            .unwrap();
        assert!(hdr[0] > 1.0, "r={}", hdr[0]);
        assert!((hdr[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_incomplete_or_duplicate_chunks() {
        let base =
            encode_srgb_png(&RgbaImage::from_raw(1, 1, vec![0, 0, 0, 255]).unwrap()).unwrap();
        let only_gmap =
            encode_png_with_chunks(&base, &[(b"gmAP", &iso21496::serialize_version())]).unwrap();
        assert!(reconstruct_if_present(&only_gmap, &[0.0; 4], 1, 1, PngRgbSpace::Srgb).is_err());

        let duplicates = encode_png_with_chunks(
            &base,
            &[
                (b"gmAP", &iso21496::serialize_version()),
                (b"gmAP", &iso21496::serialize_version()),
            ],
        )
        .unwrap();
        assert!(reconstruct_if_present(&duplicates, &[0.0; 4], 1, 1, PngRgbSpace::Srgb).is_err());
    }

    #[test]
    fn ignores_nested_gdat_inside_gainmap_png() {
        let mut info = iso21496::tests_sdr_info();
        info.gainmap_ratio_max = [4.0, 4.0, 4.0];
        let nested =
            encode_png_with_chunks(&encode_gray_png(1, 1, &[255]), &[(b"gdAT", b"not-a-png")])
                .unwrap();
        let map = gainmap_png(&info, nested);
        let base =
            encode_srgb_png(&RgbaImage::from_raw(1, 1, vec![186, 186, 186, 255]).unwrap()).unwrap();
        let hdr_png = ultra_hdr_png(&base, &map);
        let base_linear = vec![0.5, 0.5, 0.5, 1.0];
        reconstruct_if_present(&hdr_png, &base_linear, 1, 1, PngRgbSpace::Srgb).unwrap();
    }

    #[test]
    fn samples_scaled_gainmap_bilinearly() {
        let mut info = iso21496::tests_sdr_info();
        info.gainmap_ratio_max = [4.0, 4.0, 4.0];
        let map = gainmap_png(&info, encode_gray_png(1, 1, &[255]));
        let base = encode_srgb_png(
            &RgbaImage::from_raw(2, 1, vec![186, 186, 186, 255, 186, 186, 186, 255]).unwrap(),
        )
        .unwrap();
        let hdr_png = ultra_hdr_png(&base, &map);
        let base_linear = vec![0.5, 0.5, 0.5, 1.0, 0.5, 0.5, 0.5, 1.0];
        let hdr = reconstruct_if_present(&hdr_png, &base_linear, 2, 1, PngRgbSpace::Srgb)
            .unwrap()
            .unwrap();
        assert!(hdr[0] > 1.0);
        assert!((hdr[0] - hdr[4]).abs() < 1e-5);
    }

    #[test]
    fn decodes_rgb_and_rgba_gainmap_layouts() {
        let mut info = iso21496::tests_sdr_info();
        info.multi_channel = true;
        info.gainmap_ratio_max = [4.0, 2.0, 1.0];
        for (color, pixels) in [
            (png::ColorType::Rgb, vec![255u8, 255, 255]),
            (png::ColorType::Rgba, vec![255, 255, 255, 255]),
            (png::ColorType::GrayscaleAlpha, vec![255, 255]),
        ] {
            let map = gainmap_png(&info, encode_color_png(color, 1, 1, &pixels));
            let base =
                encode_srgb_png(&RgbaImage::from_raw(1, 1, vec![186, 186, 186, 128]).unwrap())
                    .unwrap();
            let hdr_png = ultra_hdr_png(&base, &map);
            let hdr =
                reconstruct_if_present(&hdr_png, &[0.5, 0.5, 0.5, 0.5], 1, 1, PngRgbSpace::Srgb)
                    .unwrap()
                    .unwrap();
            assert!((hdr[3] - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn reads_single_channel_rgba_gainmap_from_alpha() {
        let info = iso21496::tests_sdr_info();
        let map = gainmap_png(
            &info,
            encode_color_png(png::ColorType::Rgba, 1, 1, &[255, 255, 255, 0]),
        );
        let base =
            encode_srgb_png(&RgbaImage::from_raw(1, 1, vec![186, 186, 186, 255]).unwrap()).unwrap();
        let hdr_png = ultra_hdr_png(&base, &map);
        let hdr = reconstruct_if_present(&hdr_png, &[0.5, 0.5, 0.5, 1.0], 1, 1, PngRgbSpace::Srgb)
            .unwrap()
            .unwrap();

        assert!((hdr[0] - 0.5).abs() < 1e-6, "r={}", hdr[0]);
        assert!((hdr[1] - 0.5).abs() < 1e-6, "g={}", hdr[1]);
        assert!((hdr[2] - 0.5).abs() < 1e-6, "b={}", hdr[2]);
    }
}
