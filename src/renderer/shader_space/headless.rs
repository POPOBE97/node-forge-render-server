use std::path::Path;

use anyhow::{Result, anyhow, bail};
use rust_wgpu_fiber::HeadlessRenderer;
use rust_wgpu_fiber::HeadlessRendererConfig;
use rust_wgpu_fiber::eframe::wgpu::TextureFormat;

use crate::asset_store::AssetStore;
use crate::dsl::SceneDSL;

use super::api::{ShaderSpaceBuildOptions, ShaderSpacePresentationMode, ShaderSpaceBuilder};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeadlessOutputKind {
    Png,
    Exr,
}

fn route_headless_output(format: TextureFormat, output_path: &Path) -> Result<HeadlessOutputKind> {
    match format {
        TextureFormat::Rgba16Float => {
            let ext = output_path
                .extension()
                .and_then(|v| v.to_str())
                .map(|v| v.to_ascii_lowercase());
            if ext.as_deref() != Some("exr") {
                bail!(
                    "scene output format {:?}: .exr required for HDR output; got {}",
                    format,
                    output_path.display()
                );
            }
            Ok(HeadlessOutputKind::Exr)
        }
        TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb => Ok(HeadlessOutputKind::Png),
        other => bail!(
            "headless file export unsupported for scene output format {other:?}; supported: Rgba8Unorm/Rgba8UnormSrgb (png), Rgba16Float (exr)"
        ),
    }
}

pub fn render_scene_to_file_headless(
    scene: &SceneDSL,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
) -> Result<()> {
    let output_path = output_path.as_ref();
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default())
        .map_err(|e| anyhow!("failed to create headless renderer: {e}"))?;

    // Use UiSdrDisplayEncode so the assembler creates a display-encode pass
    // that bakes linear→sRGB into a presentation texture.  PNG export reads
    // that texture for correct gamma.  EXR stays on the raw scene output.
    let mut builder = ShaderSpaceBuilder::new(renderer.device.clone(), renderer.queue.clone())
        .with_adapter(renderer.adapter.clone())
        .with_options(ShaderSpaceBuildOptions {
            presentation_mode: ShaderSpacePresentationMode::UiSdrDisplayEncode,
            ..Default::default()
        });
    if let Some(store) = asset_store {
        builder = builder.with_asset_store(store.clone());
    }
    let result = builder.build(scene)?;

    result.shader_space.render();
    let output_info = result
        .shader_space
        .texture_info(result.scene_output_texture.as_str())
        .ok_or_else(|| {
            anyhow!(
                "missing scene output texture info: {}",
                result.scene_output_texture
            )
        })?;
    match route_headless_output(output_info.format, output_path)? {
        HeadlessOutputKind::Png => {
            // Read from the display-encode export texture (sRGB-encoded bytes)
            // so the PNG contains correct gamma.
            let tex_name = result.export_output_texture.as_str();
            result
                .shader_space
                .save_texture_png(tex_name, output_path)
                .map_err(|e| anyhow!("failed to save png: {e}"))?
        }
        HeadlessOutputKind::Exr => result
            .shader_space
            .save_texture_exr(result.scene_output_texture.as_str(), output_path)
            .map_err(|e| anyhow!("failed to save exr: {e}"))?,
    }
    Ok(())
}

pub fn render_scene_to_png_headless(
    scene: &SceneDSL,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
) -> Result<()> {
    render_scene_to_file_headless(scene, output_path, asset_store)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_headless_output_accepts_hdr_exr() {
        let out = route_headless_output(TextureFormat::Rgba16Float, Path::new("/tmp/out.exr"))
            .expect("rgba16float + exr should be accepted");
        assert_eq!(out, HeadlessOutputKind::Exr);
    }

    #[test]
    fn route_headless_output_rejects_hdr_non_exr() {
        let err = route_headless_output(TextureFormat::Rgba16Float, Path::new("/tmp/out.png"))
            .expect_err("rgba16float + png should fail");
        let msg = err.to_string();
        assert!(msg.contains(".exr required"));
    }

    #[test]
    fn route_headless_output_routes_rgba8_to_png() {
        assert_eq!(
            route_headless_output(TextureFormat::Rgba8Unorm, Path::new("/tmp/out.png")).unwrap(),
            HeadlessOutputKind::Png
        );
        assert_eq!(
            route_headless_output(TextureFormat::Rgba8UnormSrgb, Path::new("/tmp/out")).unwrap(),
            HeadlessOutputKind::Png
        );
    }

    #[test]
    fn route_headless_output_rejects_unsupported_format() {
        let err = route_headless_output(TextureFormat::Bgra8Unorm, Path::new("/tmp/out.png"))
            .expect_err("unsupported output format should fail");
        assert!(
            err.to_string()
                .contains("headless file export unsupported for scene output format")
        );
    }
}
