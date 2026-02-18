use rust_wgpu_fiber::eframe::wgpu;
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
use wgpu::{AstcBlock, AstcChannel};

fn all_texture_formats() -> Vec<wgpu::TextureFormat> {
    let mut formats = vec![
        wgpu::TextureFormat::R8Unorm,
        wgpu::TextureFormat::R8Snorm,
        wgpu::TextureFormat::R8Uint,
        wgpu::TextureFormat::R8Sint,
        wgpu::TextureFormat::R16Uint,
        wgpu::TextureFormat::R16Sint,
        wgpu::TextureFormat::R16Unorm,
        wgpu::TextureFormat::R16Snorm,
        wgpu::TextureFormat::R16Float,
        wgpu::TextureFormat::Rg8Unorm,
        wgpu::TextureFormat::Rg8Snorm,
        wgpu::TextureFormat::Rg8Uint,
        wgpu::TextureFormat::Rg8Sint,
        wgpu::TextureFormat::R32Uint,
        wgpu::TextureFormat::R32Sint,
        wgpu::TextureFormat::R32Float,
        wgpu::TextureFormat::Rg16Uint,
        wgpu::TextureFormat::Rg16Sint,
        wgpu::TextureFormat::Rg16Unorm,
        wgpu::TextureFormat::Rg16Snorm,
        wgpu::TextureFormat::Rg16Float,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Rgba8Snorm,
        wgpu::TextureFormat::Rgba8Uint,
        wgpu::TextureFormat::Rgba8Sint,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgb9e5Ufloat,
        wgpu::TextureFormat::Rgb10a2Uint,
        wgpu::TextureFormat::Rgb10a2Unorm,
        wgpu::TextureFormat::Rg11b10Ufloat,
        wgpu::TextureFormat::R64Uint,
        wgpu::TextureFormat::Rg32Uint,
        wgpu::TextureFormat::Rg32Sint,
        wgpu::TextureFormat::Rg32Float,
        wgpu::TextureFormat::Rgba16Uint,
        wgpu::TextureFormat::Rgba16Sint,
        wgpu::TextureFormat::Rgba16Unorm,
        wgpu::TextureFormat::Rgba16Snorm,
        wgpu::TextureFormat::Rgba16Float,
        wgpu::TextureFormat::Rgba32Uint,
        wgpu::TextureFormat::Rgba32Sint,
        wgpu::TextureFormat::Rgba32Float,
        wgpu::TextureFormat::Stencil8,
        wgpu::TextureFormat::Depth16Unorm,
        wgpu::TextureFormat::Depth24Plus,
        wgpu::TextureFormat::Depth24PlusStencil8,
        wgpu::TextureFormat::Depth32Float,
        wgpu::TextureFormat::Depth32FloatStencil8,
        wgpu::TextureFormat::NV12,
        wgpu::TextureFormat::P010,
        wgpu::TextureFormat::Bc1RgbaUnorm,
        wgpu::TextureFormat::Bc1RgbaUnormSrgb,
        wgpu::TextureFormat::Bc2RgbaUnorm,
        wgpu::TextureFormat::Bc2RgbaUnormSrgb,
        wgpu::TextureFormat::Bc3RgbaUnorm,
        wgpu::TextureFormat::Bc3RgbaUnormSrgb,
        wgpu::TextureFormat::Bc4RUnorm,
        wgpu::TextureFormat::Bc4RSnorm,
        wgpu::TextureFormat::Bc5RgUnorm,
        wgpu::TextureFormat::Bc5RgSnorm,
        wgpu::TextureFormat::Bc6hRgbUfloat,
        wgpu::TextureFormat::Bc6hRgbFloat,
        wgpu::TextureFormat::Bc7RgbaUnorm,
        wgpu::TextureFormat::Bc7RgbaUnormSrgb,
        wgpu::TextureFormat::Etc2Rgb8Unorm,
        wgpu::TextureFormat::Etc2Rgb8UnormSrgb,
        wgpu::TextureFormat::Etc2Rgb8A1Unorm,
        wgpu::TextureFormat::Etc2Rgb8A1UnormSrgb,
        wgpu::TextureFormat::Etc2Rgba8Unorm,
        wgpu::TextureFormat::Etc2Rgba8UnormSrgb,
        wgpu::TextureFormat::EacR11Unorm,
        wgpu::TextureFormat::EacR11Snorm,
        wgpu::TextureFormat::EacRg11Unorm,
        wgpu::TextureFormat::EacRg11Snorm,
    ];

    let astc_blocks = [
        AstcBlock::B4x4,
        AstcBlock::B5x4,
        AstcBlock::B5x5,
        AstcBlock::B6x5,
        AstcBlock::B6x6,
        AstcBlock::B8x5,
        AstcBlock::B8x6,
        AstcBlock::B8x8,
        AstcBlock::B10x5,
        AstcBlock::B10x6,
        AstcBlock::B10x8,
        AstcBlock::B10x10,
        AstcBlock::B12x10,
        AstcBlock::B12x12,
    ];
    let astc_channels = [AstcChannel::Unorm, AstcChannel::UnormSrgb, AstcChannel::Hdr];

    for block in astc_blocks {
        for channel in astc_channels {
            formats.push(wgpu::TextureFormat::Astc { block, channel });
        }
    }

    formats
}

#[test]
#[ignore = "adapter-dependent diagnostic test; run manually with --ignored --nocapture"]
fn list_texture_formats_supporting_8x_msaa() {
    let renderer = match HeadlessRenderer::new(HeadlessRendererConfig::default()) {
        Ok(renderer) => renderer,
        Err(err) => {
            eprintln!("No adapter available to probe 8x MSAA support: {err:?}");
            return;
        }
    };
    let adapter = renderer.adapter;

    let info = adapter.get_info();
    eprintln!(
        "Adapter: {} ({:?}, {:?})",
        info.name, info.backend, info.device_type
    );
    eprintln!(
        "Adapter supports TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES: {}",
        adapter
            .features()
            .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
    );

    let common_formats = [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba16Float,
        wgpu::TextureFormat::Depth24Plus,
        wgpu::TextureFormat::Depth24PlusStencil8,
        wgpu::TextureFormat::Depth32Float,
    ];
    eprintln!("Sample counts for common formats:");
    for format in common_formats {
        let counts = adapter
            .get_texture_format_features(format)
            .flags
            .supported_sample_counts();
        eprintln!(" - {format:?}: {counts:?}");
    }

    let mut formats_with_8x: Vec<wgpu::TextureFormat> = all_texture_formats()
        .into_iter()
        .filter(|format| {
            let counts = adapter
                .get_texture_format_features(*format)
                .flags
                .supported_sample_counts();
            counts.contains(&8)
        })
        .collect();

    formats_with_8x.sort_by_key(|f| format!("{f:?}"));
    eprintln!("8x MSAA supported formats ({}):", formats_with_8x.len());
    for format in formats_with_8x {
        eprintln!(" - {format:?}");
    }
}
