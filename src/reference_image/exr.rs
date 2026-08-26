use anyhow::{anyhow, bail};
use exr::prelude::*;
use half::f16;

use super::decode::{DecodedReferencePixels, ReferencePayloadKind, decode_linear_f32};
use super::envelope::Alpha;

pub const EXR_MIME: &str = "image/x-exr";
pub const PNG_MIME: &str = "image/png";

pub fn encode_premultiplied_linear_exr(
    width: u32,
    height: u32,
    linear_rgba: &[f32],
) -> anyhow::Result<Vec<u8>> {
    let expected = width as usize * height as usize * 4;
    if linear_rgba.len() != expected {
        bail!(
            "EXR source has {} values, expected {expected}",
            linear_rgba.len()
        );
    }
    let pixels = linear_rgba.to_vec();
    let channels = SpecificChannels::rgba(move |pos: Vec2<usize>| {
        let index = (pos.y() * width as usize + pos.x()) * 4;
        (
            f16::from_f32(pixels[index]),
            f16::from_f32(pixels[index + 1]),
            f16::from_f32(pixels[index + 2]),
            f16::from_f32(pixels[index + 3]),
        )
    });
    let image = Image::from_channels((width as usize, height as usize), channels);
    let mut bytes = Vec::new();
    image
        .write()
        .to_buffered(std::io::Cursor::new(&mut bytes))
        .map_err(|error| anyhow!("failed to encode linear EXR: {error}"))?;
    Ok(bytes)
}

pub fn decode_premultiplied_linear_exr(bytes: &[u8]) -> anyhow::Result<DecodedReferencePixels> {
    let decoded = image::load_from_memory(bytes)
        .map_err(|error| anyhow!("failed to decode linear EXR: {error}"))?;
    let rgba = decoded.to_rgba32f();
    decode_linear_f32(
        rgba.width(),
        rgba.height(),
        rgba.into_raw(),
        Alpha::Premultiplied,
        ReferencePayloadKind::RawFp16,
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exr_round_trip_preserves_hdr_half_float() {
        let source = vec![1.5f32, -0.25, 0.5, 1.0, 0.125, 0.25, 0.75, 0.5];
        let bytes = encode_premultiplied_linear_exr(1, 2, &source).unwrap();
        let decoded = decode_premultiplied_linear_exr(&bytes).unwrap();
        assert!(decoded.max_rgb() > 1.0);
        assert!((decoded.source_linear_rgba[0] - 1.5).abs() < 1e-3);
        assert!((decoded.source_linear_rgba[4] - 0.125).abs() < 1e-3);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.alpha, Alpha::Premultiplied);
    }
}
