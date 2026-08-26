use anyhow::{anyhow, bail};

const MULTI_CHANNEL: u8 = 1 << 7;
const USE_BASE_COLOUR_SPACE: u8 = 1 << 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseImageType {
    Sdr,
    Hdr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GainmapInfo {
    pub base_image_type: BaseImageType,
    pub display_ratio_sdr: f32,
    pub display_ratio_hdr: f32,
    pub gainmap_ratio_min: [f32; 3],
    pub gainmap_ratio_max: [f32; 3],
    pub gainmap_gamma: [f32; 3],
    pub epsilon_sdr: [f32; 3],
    pub epsilon_hdr: [f32; 3],
    pub use_base_colour_space: bool,
    pub multi_channel: bool,
}

pub fn parse_version(data: &[u8]) -> anyhow::Result<()> {
    let mut cursor = Cursor::new(data);
    read_iso_version(&mut cursor)?;
    Ok(())
}

pub fn parse_info(data: &[u8]) -> anyhow::Result<GainmapInfo> {
    let mut cursor = Cursor::new(data);
    read_iso_version(&mut cursor)?;
    let flags = cursor.read_u8()?;
    let multi_channel = flags & MULTI_CHANNEL != 0;
    let use_base_colour_space = flags & USE_BASE_COLOUR_SPACE != 0;
    let base_hdr_headroom = cursor.read_positive_rational()?;
    let altr_hdr_headroom = cursor.read_positive_rational()?;
    let channel_count = if multi_channel { 3 } else { 1 };
    let mut gain_min = [0.0; 3];
    let mut gain_max = [0.0; 3];
    let mut gamma = [0.0; 3];
    let mut base_offset = [0.0; 3];
    let mut altr_offset = [0.0; 3];
    for i in 0..channel_count {
        gain_min[i] = cursor.read_rational()?;
        gain_max[i] = cursor.read_rational()?;
        gamma[i] = cursor.read_positive_rational()?;
        if gamma[i] <= 0.0 || !gamma[i].is_finite() {
            bail!("ISO 21496-1 gamma must be finite and positive");
        }
        base_offset[i] = cursor.read_rational()?;
        altr_offset[i] = cursor.read_rational()?;
    }
    if !multi_channel {
        gain_min = [gain_min[0]; 3];
        gain_max = [gain_max[0]; 3];
        gamma = [gamma[0]; 3];
        base_offset = [base_offset[0]; 3];
        altr_offset = [altr_offset[0]; 3];
    }

    let mut info = GainmapInfo {
        base_image_type: BaseImageType::Sdr,
        display_ratio_sdr: 1.0,
        display_ratio_hdr: 1.0,
        gainmap_ratio_min: [0.0; 3],
        gainmap_ratio_max: [0.0; 3],
        gainmap_gamma: [0.0; 3],
        epsilon_sdr: [0.0; 3],
        epsilon_hdr: [0.0; 3],
        use_base_colour_space,
        multi_channel,
    };
    if base_hdr_headroom < altr_hdr_headroom {
        info.base_image_type = BaseImageType::Sdr;
        info.display_ratio_sdr = exp2(base_hdr_headroom)?;
        info.display_ratio_hdr = exp2(altr_hdr_headroom)?;
        info.epsilon_sdr = base_offset;
        info.epsilon_hdr = altr_offset;
    } else {
        info.base_image_type = BaseImageType::Hdr;
        info.display_ratio_hdr = exp2(base_hdr_headroom)?;
        info.display_ratio_sdr = exp2(altr_hdr_headroom)?;
        info.epsilon_hdr = base_offset;
        info.epsilon_sdr = altr_offset;
    }
    for i in 0..3 {
        info.gainmap_ratio_min[i] = exp2(gain_min[i])?;
        info.gainmap_ratio_max[i] = exp2(gain_max[i])?;
        info.gainmap_gamma[i] = 1.0 / gamma[i];
    }
    Ok(info)
}

pub fn serialize_version() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out
}

pub fn serialize_info(info: &GainmapInfo) -> Vec<u8> {
    let mut out = serialize_version();
    let mut flags = 0u8;
    if info.use_base_colour_space {
        flags |= USE_BASE_COLOUR_SPACE;
    }
    if info.multi_channel {
        flags |= MULTI_CHANNEL;
    }
    out.push(flags);
    match info.base_image_type {
        BaseImageType::Sdr => {
            write_positive_rational(&mut out, info.display_ratio_sdr.log2());
            write_positive_rational(&mut out, info.display_ratio_hdr.log2());
        }
        BaseImageType::Hdr => {
            write_positive_rational(&mut out, info.display_ratio_hdr.log2());
            write_positive_rational(&mut out, info.display_ratio_sdr.log2());
        }
    }
    let channels = if info.multi_channel { 3 } else { 1 };
    for i in 0..channels {
        write_rational(&mut out, info.gainmap_ratio_min[i].log2());
        write_rational(&mut out, info.gainmap_ratio_max[i].log2());
        write_positive_rational(&mut out, 1.0 / info.gainmap_gamma[i]);
        match info.base_image_type {
            BaseImageType::Sdr => {
                write_rational(&mut out, info.epsilon_sdr[i]);
                write_rational(&mut out, info.epsilon_hdr[i]);
            }
            BaseImageType::Hdr => {
                write_rational(&mut out, info.epsilon_hdr[i]);
                write_rational(&mut out, info.epsilon_sdr[i]);
            }
        }
    }
    out
}

fn read_iso_version(cursor: &mut Cursor<'_>) -> anyhow::Result<()> {
    let minimum_version = cursor.read_u16()?;
    if minimum_version != 0 {
        bail!("unsupported ISO 21496-1 minimum reader version {minimum_version}");
    }
    let _writer_version = cursor.read_u16()?;
    Ok(())
}

fn exp2(value: f32) -> anyhow::Result<f32> {
    let result = value.exp2();
    if !result.is_finite() || result <= 0.0 {
        bail!("ISO 21496-1 headroom/ratio is not a finite positive value");
    }
    Ok(result)
}

fn write_rational(out: &mut Vec<u8>, value: f32) {
    let denominator: u32 = if value.abs() > 1.0 { 0x1000 } else { 0x1000_0000 };
    let numerator = (f64::from(value) * f64::from(denominator)).round() as i32;
    out.extend_from_slice(&numerator.to_be_bytes());
    out.extend_from_slice(&denominator.to_be_bytes());
}

fn write_positive_rational(out: &mut Vec<u8>, value: f32) {
    let denominator: u32 = if value > 1.0 { 0x1000 } else { 0x1000_0000 };
    let numerator = (f64::from(value) * f64::from(denominator)).round() as u32;
    out.extend_from_slice(&numerator.to_be_bytes());
    out.extend_from_slice(&denominator.to_be_bytes());
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read_exact<const N: usize>(&mut self) -> anyhow::Result<[u8; N]> {
        let end = self
            .offset
            .checked_add(N)
            .ok_or_else(|| anyhow!("ISO 21496-1 metadata is truncated"))?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| anyhow!("ISO 21496-1 metadata is truncated"))?;
        self.offset = end;
        Ok(slice.try_into().unwrap())
    }

    fn read_u8(&mut self) -> anyhow::Result<u8> {
        Ok(self.read_exact::<1>()?[0])
    }

    fn read_u16(&mut self) -> anyhow::Result<u16> {
        Ok(u16::from_be_bytes(self.read_exact()?))
    }

    fn read_u32(&mut self) -> anyhow::Result<u32> {
        Ok(u32::from_be_bytes(self.read_exact()?))
    }

    fn read_i32(&mut self) -> anyhow::Result<i32> {
        Ok(i32::from_be_bytes(self.read_exact()?))
    }

    fn read_rational(&mut self) -> anyhow::Result<f32> {
        let numerator = self.read_i32()?;
        let denominator = self.read_u32()?;
        if denominator == 0 {
            bail!("ISO 21496-1 rational denominator is 0");
        }
        Ok(numerator as f32 / denominator as f32)
    }

    fn read_positive_rational(&mut self) -> anyhow::Result<f32> {
        let numerator = self.read_u32()?;
        let denominator = self.read_u32()?;
        if denominator == 0 {
            bail!("ISO 21496-1 positive rational denominator is 0");
        }
        Ok(numerator as f32 / denominator as f32)
    }
}

#[cfg(test)]
pub(crate) fn tests_sdr_info() -> GainmapInfo {
    tests::sdr_info()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sdr_info() -> GainmapInfo {
        GainmapInfo {
            base_image_type: BaseImageType::Sdr,
            display_ratio_sdr: 1.0,
            display_ratio_hdr: 4.0,
            gainmap_ratio_min: [1.0, 1.0, 1.0],
            gainmap_ratio_max: [4.0, 4.0, 4.0],
            gainmap_gamma: [1.0, 1.0, 1.0],
            epsilon_sdr: [0.0, 0.0, 0.0],
            epsilon_hdr: [0.0, 0.0, 0.0],
            use_base_colour_space: true,
            multi_channel: false,
        }
    }

    #[test]
    fn parse_version_requires_minimum_zero() {
        parse_version(&serialize_version()).unwrap();
        let mut unsupported = serialize_version();
        unsupported[0..2].copy_from_slice(&1u16.to_be_bytes());
        assert!(parse_version(&unsupported).is_err());
    }

    #[test]
    fn round_trips_single_channel_sdr_metadata() {
        let bytes = serialize_info(&sdr_info());
        let parsed = parse_info(&bytes).unwrap();
        assert_eq!(parsed.base_image_type, BaseImageType::Sdr);
        assert!((parsed.display_ratio_hdr - 4.0).abs() < 1e-3);
        assert!((parsed.gainmap_ratio_max[0] - 4.0).abs() < 1e-3);
        assert!(parsed.use_base_colour_space);
        assert!(!parsed.multi_channel);
    }
}
