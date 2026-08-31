use anyhow::{anyhow, bail};

pub const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PngChunk {
    pub name: [u8; 4],
    pub data: Vec<u8>,
}

impl PngChunk {
    pub fn name_str(&self) -> &str {
        std::str::from_utf8(&self.name).unwrap_or("????")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedPng {
    pub chunks: Vec<PngChunk>,
}

pub fn parse_png_chunks(bytes: &[u8]) -> anyhow::Result<ParsedPng> {
    if bytes.len() < 8 || bytes[0..8] != PNG_SIGNATURE {
        bail!("not a PNG: missing signature");
    }
    let mut offset = 8usize;
    let mut chunks = Vec::new();
    let mut saw_ihdr = false;
    let mut saw_idat = false;
    let mut saw_iend = false;
    while offset < bytes.len() {
        if saw_iend {
            bail!("PNG has data after IEND");
        }
        if bytes.len() - offset < 12 {
            bail!("PNG chunk header is truncated");
        }
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let name: [u8; 4] = bytes[offset + 4..offset + 8].try_into().unwrap();
        let data_start = offset + 8;
        let data_end = data_start
            .checked_add(length)
            .ok_or_else(|| anyhow!("PNG chunk length overflow"))?;
        let crc_end = data_end
            .checked_add(4)
            .ok_or_else(|| anyhow!("PNG CRC offset overflow"))?;
        if crc_end > bytes.len() {
            bail!(
                "PNG chunk {:?} is truncated",
                std::str::from_utf8(&name).unwrap_or("????")
            );
        }
        let data = bytes[data_start..data_end].to_vec();
        let expected_crc = u32::from_be_bytes(bytes[data_end..crc_end].try_into().unwrap());
        let actual_crc = crc32(&bytes[offset + 4..data_end]);
        if expected_crc != actual_crc {
            bail!(
                "PNG chunk {:?} has CRC mismatch",
                std::str::from_utf8(&name).unwrap_or("????")
            );
        }
        match &name {
            b"IHDR" => {
                if saw_ihdr || !chunks.is_empty() {
                    bail!("PNG IHDR must be the first chunk and appear once");
                }
                saw_ihdr = true;
            }
            b"IDAT" => {
                if !saw_ihdr {
                    bail!("PNG IDAT appeared before IHDR");
                }
                saw_idat = true;
            }
            b"IEND" => {
                if !saw_ihdr || !saw_idat {
                    bail!("PNG IEND is missing IHDR or IDAT");
                }
                if length != 0 {
                    bail!("PNG IEND must be empty");
                }
                saw_iend = true;
            }
            _ => {
                if !saw_ihdr {
                    bail!("PNG ancillary chunk appeared before IHDR");
                }
            }
        }
        chunks.push(PngChunk { name, data });
        offset = crc_end;
    }
    if !saw_iend {
        bail!("PNG is missing IEND");
    }
    if offset != bytes.len() {
        bail!("PNG chunk stream did not consume the file");
    }
    Ok(ParsedPng { chunks })
}

pub fn find_unique_chunk<'a>(
    png: &'a ParsedPng,
    name: &[u8; 4],
) -> anyhow::Result<Option<&'a PngChunk>> {
    let mut found = None;
    for chunk in &png.chunks {
        if &chunk.name == name {
            if found.is_some() {
                bail!(
                    "PNG contains duplicate {} chunks",
                    std::str::from_utf8(name).unwrap_or("????")
                );
            }
            found = Some(chunk);
        }
    }
    Ok(found)
}

pub fn encode_png_with_chunks(
    base_png: &[u8],
    extra: &[(&[u8; 4], &[u8])],
) -> anyhow::Result<Vec<u8>> {
    let parsed = parse_png_chunks(base_png)?;
    let mut out = PNG_SIGNATURE.to_vec();
    for chunk in &parsed.chunks {
        if chunk.name == *b"IEND" {
            for (name, data) in extra {
                write_chunk(&mut out, name, data);
            }
        }
        write_chunk(&mut out, &chunk.name, &chunk.data);
    }
    Ok(out)
}

pub fn write_chunk(out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(data);
    let crc = crc32(&out[out.len() - data.len() - 4..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_rgba_png() -> Vec<u8> {
        crate::reference_image::color::encode_srgb_png(
            &image::RgbaImage::from_raw(1, 1, vec![10, 20, 30, 40]).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn parses_ordinary_png_and_rejects_bad_crc() {
        let png = tiny_rgba_png();
        let parsed = parse_png_chunks(&png).unwrap();
        assert_eq!(parsed.chunks.first().unwrap().name, *b"IHDR");
        assert_eq!(parsed.chunks.last().unwrap().name, *b"IEND");

        let mut corrupt = png;
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0xff;
        assert!(
            parse_png_chunks(&corrupt)
                .unwrap_err()
                .to_string()
                .contains("CRC")
        );
    }

    #[test]
    fn rejects_trailing_bytes_and_missing_iend() {
        let mut png = tiny_rgba_png();
        png.push(0);
        assert!(
            parse_png_chunks(&png)
                .unwrap_err()
                .to_string()
                .contains("after IEND")
        );
    }

    #[test]
    fn injects_ancillary_chunks_before_iend() {
        let png = encode_png_with_chunks(&tiny_rgba_png(), &[(b"gmAP", &[0, 0, 0, 0])]).unwrap();
        let parsed = parse_png_chunks(&png).unwrap();
        assert!(parsed.chunks.iter().any(|chunk| chunk.name == *b"gmAP"));
        assert_eq!(parsed.chunks.last().unwrap().name, *b"IEND");
    }
}
