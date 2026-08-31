use anyhow::{Context, anyhow, bail};
use half::f16;

pub const MAGIC: [u8; 8] = *b"NFRCLIP\0";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: u16 = 64;
pub const MAX_DIMENSION: u32 = 16_384;
pub const MAX_PAYLOAD_BYTES: u64 = 512 * 1024 * 1024;

pub const FLAG_CAPTURED_HDR_LAYERS: u32 = 1 << 0;
pub const FLAG_HAS_SOURCE_PNG: u32 = 1 << 1;
const KNOWN_FLAGS: u32 = FLAG_CAPTURED_HDR_LAYERS | FLAG_HAS_SOURCE_PNG;

pub const PAYLOAD_LINEAR_RGBA16F: u16 = 1;
pub const PAYLOAD_ENCODED_PNG: u16 = 2;
pub const TRANSFER_LINEAR_SCRGB: u16 = 1;
pub const TRANSFER_ENCODED_PROFILED: u16 = 2;
pub const ALPHA_PREMULTIPLIED: u16 = 1;
pub const ALPHA_STRAIGHT: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadKind {
    LinearRgba16F,
    EncodedPng,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    LinearScRgb,
    EncodedProfiled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpha {
    Premultiplied,
    Straight,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceImageEnvelope {
    pub captured_hdr_layers: bool,
    pub payload_kind: PayloadKind,
    pub transfer: Transfer,
    pub alpha: Alpha,
    pub width: u32,
    pub height: u32,
    pub bytes_per_row: u32,
    pub primary: Vec<u8>,
    pub source_png: Option<Vec<u8>>,
}

impl ReferenceImageEnvelope {
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        validate_logical(self)?;
        let primary_len = u64::try_from(self.primary.len()).context("primary payload too large")?;
        let png_len = u64::try_from(self.source_png.as_ref().map(Vec::len).unwrap_or(0))
            .context("source PNG too large")?;
        let total = primary_len
            .checked_add(png_len)
            .filter(|len| *len <= MAX_PAYLOAD_BYTES)
            .ok_or_else(|| anyhow!("envelope payload exceeds 512 MiB"))?;
        let mut flags = 0u32;
        if self.captured_hdr_layers {
            flags |= FLAG_CAPTURED_HDR_LAYERS;
        }
        if self.source_png.is_some() {
            flags |= FLAG_HAS_SOURCE_PNG;
        }

        let mut bytes = Vec::with_capacity(HEADER_LEN as usize + total as usize);
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&HEADER_LEN.to_le_bytes());
        bytes.extend_from_slice(&flags.to_le_bytes());
        bytes.extend_from_slice(&payload_kind_code(self.payload_kind).to_le_bytes());
        bytes.extend_from_slice(&transfer_code(self.transfer).to_le_bytes());
        bytes.extend_from_slice(&alpha_code(self.alpha).to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&self.width.to_le_bytes());
        bytes.extend_from_slice(&self.height.to_le_bytes());
        bytes.extend_from_slice(&self.bytes_per_row.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&primary_len.to_le_bytes());
        bytes.extend_from_slice(&png_len.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&self.primary);
        if let Some(png) = &self.source_png {
            bytes.extend_from_slice(png);
        }
        Ok(bytes)
    }
}

pub fn parse_envelope(bytes: &[u8]) -> anyhow::Result<ReferenceImageEnvelope> {
    if bytes.len() < HEADER_LEN as usize {
        bail!("NodeForgeReferenceImageV1 header is truncated");
    }
    if bytes[0..8] != MAGIC {
        bail!("NodeForgeReferenceImageV1 magic mismatch");
    }
    let version = read_u16(bytes, 8)?;
    if version != VERSION {
        bail!("unsupported NodeForgeReferenceImageV1 version {version}");
    }
    let header_len = read_u16(bytes, 10)?;
    if header_len != HEADER_LEN {
        bail!("unsupported NodeForgeReferenceImageV1 header length {header_len}");
    }
    let flags = read_u32(bytes, 12)?;
    if flags & !KNOWN_FLAGS != 0 {
        bail!("unsupported NodeForgeReferenceImageV1 flags {flags:#x}");
    }
    let captured_hdr_layers = flags & FLAG_CAPTURED_HDR_LAYERS != 0;
    let has_source_png = flags & FLAG_HAS_SOURCE_PNG != 0;
    let payload_kind = parse_payload_kind(read_u16(bytes, 16)?)?;
    let transfer = parse_transfer(read_u16(bytes, 18)?)?;
    let alpha = parse_alpha(read_u16(bytes, 20)?)?;
    let reserved_u16 = read_u16(bytes, 22)?;
    if reserved_u16 != 0 {
        bail!("NodeForgeReferenceImageV1 reserved u16 must be 0");
    }
    let width = read_u32(bytes, 24)?;
    let height = read_u32(bytes, 28)?;
    let bytes_per_row = read_u32(bytes, 32)?;
    let reserved_u32 = read_u32(bytes, 36)?;
    if reserved_u32 != 0 {
        bail!("NodeForgeReferenceImageV1 reserved u32 must be 0");
    }
    let primary_len = read_u64(bytes, 40)?;
    let source_png_len = read_u64(bytes, 48)?;
    let reserved_u64 = read_u64(bytes, 56)?;
    if reserved_u64 != 0 {
        bail!("NodeForgeReferenceImageV1 reserved u64 must be 0");
    }
    if has_source_png != (source_png_len > 0) {
        bail!("NodeForgeReferenceImageV1 has_source_png flag does not match PNG length");
    }
    let total_len = primary_len
        .checked_add(source_png_len)
        .ok_or_else(|| anyhow!("NodeForgeReferenceImageV1 payload length overflow"))?;
    if total_len > MAX_PAYLOAD_BYTES {
        bail!("NodeForgeReferenceImageV1 payload exceeds 512 MiB");
    }
    let expected_len = (HEADER_LEN as u64)
        .checked_add(total_len)
        .ok_or_else(|| anyhow!("NodeForgeReferenceImageV1 envelope length overflow"))?;
    let actual_len = u64::try_from(bytes.len()).context("envelope too large")?;
    if actual_len < expected_len {
        bail!("NodeForgeReferenceImageV1 payload is truncated");
    }
    if actual_len > expected_len {
        bail!("NodeForgeReferenceImageV1 has trailing data");
    }

    let primary_end =
        HEADER_LEN as usize + usize::try_from(primary_len).context("primary payload too large")?;
    let png_end = primary_end + usize::try_from(source_png_len).context("source PNG too large")?;
    let primary = bytes[HEADER_LEN as usize..primary_end].to_vec();
    let source_png = has_source_png.then(|| bytes[primary_end..png_end].to_vec());

    let envelope = ReferenceImageEnvelope {
        captured_hdr_layers,
        payload_kind,
        transfer,
        alpha,
        width,
        height,
        bytes_per_row,
        primary,
        source_png,
    };
    validate_logical(&envelope)?;
    if envelope.payload_kind == PayloadKind::LinearRgba16F {
        validate_linear_rgba16f_pixels(&envelope)?;
    }
    Ok(envelope)
}

fn validate_logical(envelope: &ReferenceImageEnvelope) -> anyhow::Result<()> {
    if envelope.width == 0 || envelope.height == 0 {
        bail!("NodeForgeReferenceImageV1 dimensions must be non-zero");
    }
    if envelope.width > MAX_DIMENSION || envelope.height > MAX_DIMENSION {
        bail!("NodeForgeReferenceImageV1 exceeds 16K per-axis limit");
    }
    match envelope.payload_kind {
        PayloadKind::LinearRgba16F => {
            if envelope.transfer != Transfer::LinearScRgb {
                bail!("LinearRgba16F requires LinearScRgb transfer");
            }
            if envelope.alpha != Alpha::Premultiplied {
                bail!("LinearRgba16F requires premultiplied alpha");
            }
            let expected_bpr = envelope
                .width
                .checked_mul(8)
                .ok_or_else(|| anyhow!("LinearRgba16F row stride overflow"))?;
            if envelope.bytes_per_row != expected_bpr {
                bail!(
                    "LinearRgba16F bytes-per-row is {}, expected {}",
                    envelope.bytes_per_row,
                    expected_bpr
                );
            }
            let expected_len = (envelope.bytes_per_row as u64)
                .checked_mul(u64::from(envelope.height))
                .ok_or_else(|| anyhow!("LinearRgba16F payload size overflow"))?;
            if envelope.primary.len() as u64 != expected_len {
                bail!(
                    "LinearRgba16F payload has {} bytes, expected {expected_len}",
                    envelope.primary.len()
                );
            }
        }
        PayloadKind::EncodedPng => {
            if envelope.transfer != Transfer::EncodedProfiled {
                bail!("EncodedPng requires EncodedProfiled transfer");
            }
            if envelope.alpha != Alpha::Straight {
                bail!("EncodedPng requires straight alpha");
            }
            if envelope.bytes_per_row != 0 {
                bail!("EncodedPng bytes-per-row must be 0");
            }
            if envelope.primary.is_empty() {
                bail!("EncodedPng payload is empty");
            }
        }
    }
    Ok(())
}

fn validate_linear_rgba16f_pixels(envelope: &ReferenceImageEnvelope) -> anyhow::Result<()> {
    let pixels = envelope.primary.chunks_exact(8);
    if !envelope.primary.chunks_exact(8).remainder().is_empty() {
        bail!("LinearRgba16F payload is not aligned to RGBA half-float pixels");
    }
    for pixel in pixels {
        let r = f16::from_le_bytes(pixel[0..2].try_into().unwrap());
        let g = f16::from_le_bytes(pixel[2..4].try_into().unwrap());
        let b = f16::from_le_bytes(pixel[4..6].try_into().unwrap());
        let a = f16::from_le_bytes(pixel[6..8].try_into().unwrap());
        if !r.is_finite() || !g.is_finite() || !b.is_finite() || !a.is_finite() {
            bail!("LinearRgba16F payload contains a non-finite half value");
        }
        let alpha = a.to_f32();
        if !(0.0..=1.0).contains(&alpha) {
            bail!("LinearRgba16F alpha {alpha} is outside [0,1]");
        }
    }
    Ok(())
}

pub fn rgba16f_to_f32(bytes: &[u8]) -> anyhow::Result<Vec<f32>> {
    if bytes.len() % 8 != 0 {
        bail!("RGBA16F payload is not aligned to pixels");
    }
    Ok(bytes
        .chunks_exact(8)
        .flat_map(|pixel| {
            [
                f16::from_le_bytes(pixel[0..2].try_into().unwrap()).to_f32(),
                f16::from_le_bytes(pixel[2..4].try_into().unwrap()).to_f32(),
                f16::from_le_bytes(pixel[4..6].try_into().unwrap()).to_f32(),
                f16::from_le_bytes(pixel[6..8].try_into().unwrap()).to_f32(),
            ]
        })
        .collect())
}

pub fn f32_to_rgba16f(pixels: &[f32]) -> anyhow::Result<Vec<u8>> {
    if pixels.len() % 4 != 0 {
        bail!("linear RGBA f32 length must be a multiple of 4");
    }
    let mut bytes = Vec::with_capacity(pixels.len() * 2);
    for chunk in pixels.chunks_exact(4) {
        for value in chunk {
            bytes.extend_from_slice(&f16::from_f32(*value).to_le_bytes());
        }
    }
    Ok(bytes)
}

fn payload_kind_code(kind: PayloadKind) -> u16 {
    match kind {
        PayloadKind::LinearRgba16F => PAYLOAD_LINEAR_RGBA16F,
        PayloadKind::EncodedPng => PAYLOAD_ENCODED_PNG,
    }
}

fn transfer_code(transfer: Transfer) -> u16 {
    match transfer {
        Transfer::LinearScRgb => TRANSFER_LINEAR_SCRGB,
        Transfer::EncodedProfiled => TRANSFER_ENCODED_PROFILED,
    }
}

fn alpha_code(alpha: Alpha) -> u16 {
    match alpha {
        Alpha::Premultiplied => ALPHA_PREMULTIPLIED,
        Alpha::Straight => ALPHA_STRAIGHT,
    }
}

fn parse_payload_kind(code: u16) -> anyhow::Result<PayloadKind> {
    match code {
        PAYLOAD_LINEAR_RGBA16F => Ok(PayloadKind::LinearRgba16F),
        PAYLOAD_ENCODED_PNG => Ok(PayloadKind::EncodedPng),
        other => bail!("unsupported NodeForgeReferenceImageV1 payload kind {other}"),
    }
}

fn parse_transfer(code: u16) -> anyhow::Result<Transfer> {
    match code {
        TRANSFER_LINEAR_SCRGB => Ok(Transfer::LinearScRgb),
        TRANSFER_ENCODED_PROFILED => Ok(Transfer::EncodedProfiled),
        other => bail!("unsupported NodeForgeReferenceImageV1 transfer {other}"),
    }
}

fn parse_alpha(code: u16) -> anyhow::Result<Alpha> {
    match code {
        ALPHA_PREMULTIPLIED => Ok(Alpha::Premultiplied),
        ALPHA_STRAIGHT => Ok(Alpha::Straight),
        other => bail!("unsupported NodeForgeReferenceImageV1 alpha {other}"),
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> anyhow::Result<u16> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or_else(|| anyhow!("truncated u16 at {offset}"))?
            .try_into()
            .unwrap(),
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> anyhow::Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| anyhow!("truncated u32 at {offset}"))?
            .try_into()
            .unwrap(),
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> anyhow::Result<u64> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or_else(|| anyhow!("truncated u64 at {offset}"))?
            .try_into()
            .unwrap(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_hdr_envelope(with_png: bool) -> ReferenceImageEnvelope {
        let pixels = f32_to_rgba16f(&[1.5, -0.25, 0.5, 1.0]).unwrap();
        ReferenceImageEnvelope {
            captured_hdr_layers: true,
            payload_kind: PayloadKind::LinearRgba16F,
            transfer: Transfer::LinearScRgb,
            alpha: Alpha::Premultiplied,
            width: 1,
            height: 1,
            bytes_per_row: 8,
            primary: pixels,
            source_png: with_png.then(|| b"\x89PNG".to_vec()),
        }
    }

    fn sdr_png_envelope() -> ReferenceImageEnvelope {
        ReferenceImageEnvelope {
            captured_hdr_layers: false,
            payload_kind: PayloadKind::EncodedPng,
            transfer: Transfer::EncodedProfiled,
            alpha: Alpha::Straight,
            width: 2,
            height: 2,
            bytes_per_row: 0,
            primary: b"\x89PNG\r\n\x1a\n".to_vec(),
            source_png: None,
        }
    }

    #[test]
    fn round_trips_raw_only_and_raw_plus_png() {
        for with_png in [false, true] {
            let encoded = raw_hdr_envelope(with_png).encode().unwrap();
            let parsed = parse_envelope(&encoded).unwrap();
            assert_eq!(parsed, raw_hdr_envelope(with_png));
            assert_eq!(encoded[0..8], MAGIC);
            assert_eq!(read_u16(&encoded, 8).unwrap(), 1);
            assert_eq!(read_u16(&encoded, 10).unwrap(), 64);
            assert_eq!(read_u32(&encoded, 24).unwrap(), 1);
            assert_eq!(read_u32(&encoded, 28).unwrap(), 1);
            assert_eq!(read_u32(&encoded, 32).unwrap(), 8);
        }
    }

    #[test]
    fn round_trips_encoded_png_stock_fallback() {
        let encoded = sdr_png_envelope().encode().unwrap();
        assert_eq!(parse_envelope(&encoded).unwrap(), sdr_png_envelope());
        assert_eq!(read_u16(&encoded, 16).unwrap(), PAYLOAD_ENCODED_PNG);
        assert_eq!(read_u32(&encoded, 32).unwrap(), 0);
    }

    #[test]
    fn accepts_negative_finite_rgb() {
        parse_envelope(&raw_hdr_envelope(false).encode().unwrap()).unwrap();
    }

    #[test]
    fn rejects_unknown_version_flags_enums_and_reserved() {
        let mut encoded = raw_hdr_envelope(false).encode().unwrap();
        encoded[8..10].copy_from_slice(&2u16.to_le_bytes());
        assert!(
            parse_envelope(&encoded)
                .unwrap_err()
                .to_string()
                .contains("version")
        );

        encoded = raw_hdr_envelope(false).encode().unwrap();
        encoded[12..16].copy_from_slice(&(FLAG_HAS_SOURCE_PNG << 2).to_le_bytes());
        assert!(
            parse_envelope(&encoded)
                .unwrap_err()
                .to_string()
                .contains("flags")
        );

        encoded = raw_hdr_envelope(false).encode().unwrap();
        encoded[16..18].copy_from_slice(&99u16.to_le_bytes());
        assert!(
            parse_envelope(&encoded)
                .unwrap_err()
                .to_string()
                .contains("payload kind")
        );

        encoded = raw_hdr_envelope(false).encode().unwrap();
        encoded[22..24].copy_from_slice(&1u16.to_le_bytes());
        assert!(
            parse_envelope(&encoded)
                .unwrap_err()
                .to_string()
                .contains("reserved")
        );

        encoded = raw_hdr_envelope(false).encode().unwrap();
        encoded[36..40].copy_from_slice(&1u32.to_le_bytes());
        assert!(
            parse_envelope(&encoded)
                .unwrap_err()
                .to_string()
                .contains("reserved")
        );

        encoded = raw_hdr_envelope(false).encode().unwrap();
        encoded[56..64].copy_from_slice(&1u64.to_le_bytes());
        assert!(
            parse_envelope(&encoded)
                .unwrap_err()
                .to_string()
                .contains("reserved")
        );
    }

    #[test]
    fn rejects_wrong_format_transfer_alpha_combinations() {
        let mut envelope = raw_hdr_envelope(false);
        envelope.transfer = Transfer::EncodedProfiled;
        assert!(envelope.encode().is_err());

        envelope = raw_hdr_envelope(false);
        envelope.alpha = Alpha::Straight;
        assert!(envelope.encode().is_err());

        envelope = sdr_png_envelope();
        envelope.bytes_per_row = 8;
        assert!(envelope.encode().is_err());

        envelope = sdr_png_envelope();
        envelope.alpha = Alpha::Premultiplied;
        assert!(envelope.encode().is_err());
    }

    #[test]
    fn rejects_truncation_trailing_data_and_row_mismatch() {
        let encoded = raw_hdr_envelope(true).encode().unwrap();
        assert!(parse_envelope(&encoded[..encoded.len() - 1]).is_err());
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(
            parse_envelope(&trailing)
                .unwrap_err()
                .to_string()
                .contains("trailing")
        );

        let mut envelope = raw_hdr_envelope(false);
        envelope.bytes_per_row = 16;
        assert!(envelope.encode().is_err());
    }

    #[test]
    fn rejects_dimension_and_payload_overflow() {
        let mut envelope = raw_hdr_envelope(false);
        envelope.width = MAX_DIMENSION + 1;
        assert!(envelope.encode().is_err());

        let mut encoded = raw_hdr_envelope(false).encode().unwrap();
        encoded[40..48].copy_from_slice(&(MAX_PAYLOAD_BYTES + 1).to_le_bytes());
        assert!(
            parse_envelope(&encoded)
                .unwrap_err()
                .to_string()
                .contains("512 MiB")
        );
    }

    fn pack_rgba16f(r: f16, g: f16, b: f16, a: f16) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&r.to_le_bytes());
        bytes.extend_from_slice(&g.to_le_bytes());
        bytes.extend_from_slice(&b.to_le_bytes());
        bytes.extend_from_slice(&a.to_le_bytes());
        bytes
    }

    #[test]
    fn rejects_nan_inf_and_invalid_alpha() {
        for value in [f16::NAN, f16::INFINITY, f16::NEG_INFINITY] {
            let mut bytes = raw_hdr_envelope(false).encode().unwrap();
            let primary = pack_rgba16f(
                value,
                f16::from_f32(0.0),
                f16::from_f32(0.0),
                f16::from_f32(1.0),
            );
            bytes.splice(64.., primary);
            assert!(
                parse_envelope(&bytes).is_err(),
                "expected rejection for {value:?}"
            );
        }

        let mut bytes = raw_hdr_envelope(false).encode().unwrap();
        bytes[64 + 6..64 + 8].copy_from_slice(&f16::from_f32(1.5).to_le_bytes());
        assert!(
            parse_envelope(&bytes)
                .unwrap_err()
                .to_string()
                .contains("alpha")
        );
    }

    #[test]
    fn little_endian_header_fields_cover_every_offset() {
        let encoded = raw_hdr_envelope(true).encode().unwrap();
        assert_eq!(&encoded[0..8], &MAGIC);
        assert_eq!(
            u16::from_le_bytes(encoded[8..10].try_into().unwrap()),
            VERSION
        );
        assert_eq!(
            u16::from_le_bytes(encoded[10..12].try_into().unwrap()),
            HEADER_LEN
        );
        assert_eq!(
            u32::from_le_bytes(encoded[12..16].try_into().unwrap()),
            FLAG_CAPTURED_HDR_LAYERS | FLAG_HAS_SOURCE_PNG
        );
        assert_eq!(
            u16::from_le_bytes(encoded[16..18].try_into().unwrap()),
            PAYLOAD_LINEAR_RGBA16F
        );
        assert_eq!(
            u16::from_le_bytes(encoded[18..20].try_into().unwrap()),
            TRANSFER_LINEAR_SCRGB
        );
        assert_eq!(
            u16::from_le_bytes(encoded[20..22].try_into().unwrap()),
            ALPHA_PREMULTIPLIED
        );
        assert_eq!(u16::from_le_bytes(encoded[22..24].try_into().unwrap()), 0);
        assert_eq!(u32::from_le_bytes(encoded[24..28].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(encoded[28..32].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(encoded[32..36].try_into().unwrap()), 8);
        assert_eq!(u32::from_le_bytes(encoded[36..40].try_into().unwrap()), 0);
        assert_eq!(u64::from_le_bytes(encoded[40..48].try_into().unwrap()), 8);
        assert_eq!(u64::from_le_bytes(encoded[48..56].try_into().unwrap()), 4);
        assert_eq!(u64::from_le_bytes(encoded[56..64].try_into().unwrap()), 0);
    }
}
