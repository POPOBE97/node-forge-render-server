pub const PRIVATE_TYPE: &str = "com.node-forge.reference-image.v1";

pub trait Pasteboard {
    fn write(&mut self, private_envelope: &[u8], public_png: &[u8]) -> anyhow::Result<()>;
    fn read_private(&self) -> anyhow::Result<Option<Vec<u8>>>;
    fn read_png(&self) -> anyhow::Result<Option<Vec<u8>>>;
}

#[derive(Clone, Debug, Default)]
pub struct MemoryPasteboard {
    pub private: Option<Vec<u8>>,
    pub png: Option<Vec<u8>>,
    pub writes: usize,
}

impl Pasteboard for MemoryPasteboard {
    fn write(&mut self, private_envelope: &[u8], public_png: &[u8]) -> anyhow::Result<()> {
        self.private = Some(private_envelope.to_vec());
        self.png = Some(public_png.to_vec());
        self.writes += 1;
        Ok(())
    }

    fn read_private(&self) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(self.private.clone())
    }

    fn read_png(&self) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(self.png.clone())
    }
}

#[cfg(target_os = "macos")]
pub fn write_macos(private_envelope: &[u8], public_png: &[u8]) -> anyhow::Result<()> {
    write_macos_pasteboard(
        &objc2_app_kit::NSPasteboard::generalPasteboard(),
        private_envelope,
        public_png,
    )
}

#[cfg(target_os = "macos")]
pub fn write_macos_pasteboard(
    pasteboard: &objc2_app_kit::NSPasteboard,
    private_envelope: &[u8],
    public_png: &[u8],
) -> anyhow::Result<()> {
    use objc2_app_kit::NSPasteboardTypePNG;
    use objc2_foundation::{NSData, NSString};

    let png_data = NSData::with_bytes(public_png);
    let private_data = NSData::with_bytes(private_envelope);
    pasteboard.clearContents();
    // SAFETY: NSPasteboardTypePNG is a process-lifetime AppKit constant.
    let png_type = unsafe { NSPasteboardTypePNG };
    if !pasteboard.setData_forType(Some(&png_data), png_type) {
        anyhow::bail!("NSPasteboard rejected public PNG data");
    }
    let private_type = NSString::from_str(PRIVATE_TYPE);
    if !pasteboard.setData_forType(Some(&private_data), &private_type) {
        anyhow::bail!("NSPasteboard rejected private reference image data");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn read_macos_private() -> anyhow::Result<Option<Vec<u8>>> {
    read_macos_type(PRIVATE_TYPE)
}

#[cfg(target_os = "macos")]
pub fn read_macos_png() -> anyhow::Result<Option<Vec<u8>>> {
    use objc2_app_kit::NSPasteboardTypePNG;
    let pasteboard = objc2_app_kit::NSPasteboard::generalPasteboard();
    // SAFETY: NSPasteboardTypePNG is a process-lifetime AppKit constant.
    let png_type = unsafe { NSPasteboardTypePNG };
    Ok(pasteboard.dataForType(png_type).map(|data| data.to_vec()))
}

#[cfg(target_os = "macos")]
fn read_macos_type(ty: &str) -> anyhow::Result<Option<Vec<u8>>> {
    use objc2_foundation::NSString;
    let pasteboard = objc2_app_kit::NSPasteboard::generalPasteboard();
    let ty = NSString::from_str(ty);
    Ok(pasteboard.dataForType(&ty).map(|data| data.to_vec()))
}

#[cfg(not(target_os = "macos"))]
pub fn write_macos(_private_envelope: &[u8], _public_png: &[u8]) -> anyhow::Result<()> {
    anyhow::bail!("private reference clipboard flavor is macOS-only")
}

pub fn write_public_and_private(
    pasteboard: &mut dyn Pasteboard,
    private_envelope: &[u8],
    public_png: &[u8],
) -> anyhow::Result<()> {
    pasteboard.write(private_envelope, public_png)
}

#[cfg(target_os = "macos")]
pub fn write_system_clipboard(private_envelope: &[u8], public_png: &[u8]) -> anyhow::Result<()> {
    write_macos(private_envelope, public_png)
}

#[cfg(not(target_os = "macos"))]
pub fn write_system_clipboard(_private_envelope: &[u8], public_png: &[u8]) -> anyhow::Result<()> {
    let decoded = image::load_from_memory(public_png)
        .map_err(|error| anyhow::anyhow!("failed to decode public PNG for clipboard: {error}"))?;
    let rgba = decoded.to_rgba8();
    arboard::Clipboard::new()
        .and_then(|mut clipboard| {
            clipboard.set_image(arboard::ImageData {
                width: rgba.width() as usize,
                height: rgba.height() as usize,
                bytes: std::borrow::Cow::Owned(rgba.into_raw()),
            })
        })
        .map_err(|error| anyhow::anyhow!("failed to copy Android screencap to clipboard: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference_image::{encode_srgb_png, envelope_from_raw_fp16};
    use image::RgbaImage;

    #[test]
    fn private_flavor_wins_on_memory_pasteboard() {
        let png = encode_srgb_png(&RgbaImage::from_raw(1, 1, vec![1, 2, 3, 4]).unwrap()).unwrap();
        let envelope = envelope_from_raw_fp16(1, 1, &[1.5, 0.0, 0.0, 1.0], true, Some(png.clone()))
            .unwrap()
            .encode()
            .unwrap();
        let mut pasteboard = MemoryPasteboard::default();
        write_public_and_private(&mut pasteboard, &envelope, &png).unwrap();
        assert_eq!(pasteboard.read_private().unwrap().unwrap(), envelope);
        assert_eq!(pasteboard.read_png().unwrap().unwrap(), png);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_unique_pasteboard_writes_both_flavors() {
        use objc2_app_kit::NSPasteboard;
        let png = encode_srgb_png(&RgbaImage::from_raw(1, 1, vec![9, 8, 7, 6]).unwrap()).unwrap();
        let envelope = envelope_from_raw_fp16(1, 1, &[1.25, 0.0, 0.0, 1.0], true, Some(png.clone()))
            .unwrap()
            .encode()
            .unwrap();
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();
        write_macos_pasteboard(&pasteboard, &envelope, &png).unwrap();
        let png_type = unsafe { objc2_app_kit::NSPasteboardTypePNG };
        assert_eq!(pasteboard.dataForType(png_type).unwrap().to_vec(), png);
        let private = objc2_foundation::NSString::from_str(PRIVATE_TYPE);
        assert_eq!(pasteboard.dataForType(&private).unwrap().to_vec(), envelope);
    }
}
