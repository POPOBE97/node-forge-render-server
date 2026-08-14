use std::{io::Write, path::Path};

use anyhow::{Result, anyhow, bail};
use rust_wgpu_fiber::HeadlessRenderer;
use rust_wgpu_fiber::HeadlessRendererConfig;
use rust_wgpu_fiber::eframe::wgpu::TextureFormat;
use rust_wgpu_fiber::shader_space::{
    PASS_CAPTURE_OUTPUT_TEXTURE_NAME, PassCaptureMode, PassCaptureRequest,
};

use crate::asset_store::AssetStore;
use crate::dsl::SceneDSL;
use crate::profile::{self, ProfileAccumulator, ProfileRunConfig, ProfileWriter};
use crate::ui::resource_tree::ResourceSnapshot;

use super::api::{ShaderSpaceBuildOptions, ShaderSpaceBuilder, ShaderSpacePresentationMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeadlessOutputKind {
    Png,
    Exr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeadlessTextureOutputKind {
    Exr,
    RawRgba16F,
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

fn route_headless_texture_output(
    format: TextureFormat,
    output_path: &Path,
) -> Result<HeadlessTextureOutputKind> {
    let ext = output_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match (format, ext.as_deref()) {
        (TextureFormat::Rgba16Float, Some("rgba16f")) => Ok(HeadlessTextureOutputKind::RawRgba16F),
        (TextureFormat::Rgba16Float, Some("exr")) => Ok(HeadlessTextureOutputKind::Exr),
        (TextureFormat::Rgba16Float, _) => bail!(
            "RGBA16Float texture export requires .rgba16f (exact half bits) or .exr; got {}",
            output_path.display()
        ),
        (other, _) => bail!(
            "headless texture export unsupported for format {other:?}; only Rgba16Float is supported"
        ),
    }
}

/// Write exact RGBA half-float texels using a small self-describing binary container.
///
/// Layout (little-endian): `NF16`, version u32, width u32, height u32, channels u32,
/// followed by row-major RGBA f16 bits. `read_texture_rgba16f` expands each GPU half to
/// an exactly representable f32; converting it back to f16 therefore preserves every
/// finite source bit without the precision loss of an 8-bit image path.
fn save_rgba16f_raw(output_path: &Path, width: u32, height: u32, channels: &[f32]) -> Result<()> {
    let expected_len = width as usize * height as usize * 4;
    if channels.len() != expected_len {
        bail!(
            "invalid RGBA16F readback shape: expected {expected_len} channels, got {}",
            channels.len()
        );
    }

    let file = std::fs::File::create(output_path)
        .map_err(|error| anyhow!("failed to create {}: {error}", output_path.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    writer.write_all(b"NF16")?;
    writer.write_all(&1u32.to_le_bytes())?;
    writer.write_all(&width.to_le_bytes())?;
    writer.write_all(&height.to_le_bytes())?;
    writer.write_all(&4u32.to_le_bytes())?;
    for value in channels {
        writer.write_all(&half::f16::from_f32(*value).to_bits().to_le_bytes())?;
    }
    writer.flush()?;
    Ok(())
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

/// Render one frame at scene time zero and export a named internal RGBA16F texture.
///
/// `.rgba16f` preserves the exact half-float payload and is the preferred format for
/// pixel equality. `.exr` is also supported for visual inspection and standard tools.
pub fn render_scene_texture_to_file_headless(
    scene: &SceneDSL,
    texture_name: &str,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
) -> Result<()> {
    let output_path = output_path.as_ref();
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default())
        .map_err(|error| anyhow!("failed to create headless renderer: {error}"))?;
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

    let texture_info = result
        .shader_space
        .texture_info(texture_name)
        .ok_or_else(|| anyhow!("named headless texture not found: {texture_name}"))?;
    match route_headless_texture_output(texture_info.format, output_path)? {
        HeadlessTextureOutputKind::RawRgba16F => {
            let image = result
                .shader_space
                .read_texture_rgba16f(texture_name)
                .map_err(|error| {
                    anyhow!("failed to read RGBA16F texture {texture_name}: {error}")
                })?;
            save_rgba16f_raw(output_path, image.width, image.height, &image.channels)?;
        }
        HeadlessTextureOutputKind::Exr => result
            .shader_space
            .save_texture_exr(texture_name, output_path)
            .map_err(|error| anyhow!("failed to save EXR texture {texture_name}: {error}"))?,
    }
    Ok(())
}

/// Render one frame at scene time zero and export one render pass in Solo mode.
///
/// Solo redirects the selected draw into a transparent attachment with the same size and format as
/// its production target. This exposes the pass's pre-composite RGBA16F result even when several
/// passes normally share the scene output texture.
pub fn render_scene_pass_to_file_headless(
    scene: &SceneDSL,
    pass_name: &str,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
) -> Result<()> {
    let output_path = output_path.as_ref();
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default())
        .map_err(|error| anyhow!("failed to create headless renderer: {error}"))?;
    let mut builder = ShaderSpaceBuilder::new(renderer.device.clone(), renderer.queue.clone())
        .with_adapter(renderer.adapter.clone())
        .with_options(ShaderSpaceBuildOptions {
            presentation_mode: ShaderSpacePresentationMode::UiSdrDisplayEncode,
            ..Default::default()
        });
    if let Some(store) = asset_store {
        builder = builder.with_asset_store(store.clone());
    }
    let mut result = builder.build(scene)?;
    let request = PassCaptureRequest::new(pass_name, PassCaptureMode::Solo);
    let capture = result
        .shader_space
        .prepare_pass_capture(&request)
        .map_err(|error| anyhow!("cannot capture pass {pass_name}: {error}"))?;
    let _ = result
        .shader_space
        .render_profiled_with_pass_capture(true, &request);

    match route_headless_texture_output(capture.format, output_path)? {
        HeadlessTextureOutputKind::RawRgba16F => {
            let image = result
                .shader_space
                .read_texture_rgba16f(PASS_CAPTURE_OUTPUT_TEXTURE_NAME)
                .map_err(|error| {
                    anyhow!("failed to read RGBA16F pass capture {pass_name}: {error}")
                })?;
            save_rgba16f_raw(output_path, image.width, image.height, &image.channels)?;
        }
        HeadlessTextureOutputKind::Exr => result
            .shader_space
            .save_texture_exr(PASS_CAPTURE_OUTPUT_TEXTURE_NAME, output_path)
            .map_err(|error| anyhow!("failed to save pass capture {pass_name}: {error}"))?,
    }
    Ok(())
}

pub fn render_scene_to_file_headless_profiled(
    scene: &SceneDSL,
    output_path: impl AsRef<Path>,
    asset_store: Option<&AssetStore>,
    profile_config: &ProfileRunConfig,
    writer: &mut ProfileWriter,
) -> Result<()> {
    let output_path = output_path.as_ref();
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default())
        .map_err(|e| anyhow!("failed to create headless renderer: {e}"))?;

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
    let snapshot = ResourceSnapshot::capture(
        &result.shader_space,
        &result.pass_bindings,
        Some(result.present_output_texture.as_str()),
        Some(scene),
    );

    let run_id = profile::run_id();
    let output_path_text = output_path.display().to_string();
    writer.emit(&profile::run_start_event(
        &run_id,
        profile_config,
        &output_path_text,
    ))?;
    writer.emit(&profile::adapter_info_event(
        &run_id,
        &renderer.adapter,
        &renderer.device,
    ))?;
    writer.emit(&profile::scene_info_event(
        &run_id,
        result.resolution,
        result.present_output_texture.as_str(),
        result.export_output_texture.as_str(),
        &snapshot,
    ))?;
    for _ in 0..profile_config.warmup_frames {
        let _ = result.shader_space.render_profiled(true);
    }

    let pass_info = profile::pass_info_by_name(&snapshot);
    let mut accumulator = ProfileAccumulator::default();
    let measured_frames = profile_config.frames.max(1);
    for frame_index in 0..measured_frames {
        let frame_profile = result.shader_space.render_profiled(true);
        accumulator.observe_frame(&frame_profile);
        writer.emit(&profile::frame_sample_event(
            &run_id,
            frame_index,
            &frame_profile,
        ))?;
        for pass_sample in &frame_profile.passes {
            writer.emit(&profile::pass_sample_event(
                &run_id,
                frame_index,
                pass_sample,
                pass_info.get(pass_sample.pass_name.as_str()).copied(),
            ))?;
        }
    }

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

    writer.emit(&profile::run_end_event(
        &run_id,
        &output_path_text,
        &accumulator,
    ))?;
    writer.flush()?;
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

    #[test]
    fn route_named_rgba16f_texture_prefers_exact_raw_extension() {
        assert_eq!(
            route_headless_texture_output(
                TextureFormat::Rgba16Float,
                Path::new("/tmp/intelligent-light.rgba16f")
            )
            .unwrap(),
            HeadlessTextureOutputKind::RawRgba16F
        );
    }

    #[test]
    fn raw_rgba16f_container_preserves_half_bits() {
        let path = std::env::temp_dir().join(format!(
            "node-forge-headless-{}-{}.rgba16f",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let values = [0.0, 1.0, -2.0, 0.5, 65504.0, 1.0 / 3.0, 4.0, 8.0];
        save_rgba16f_raw(&path, 2, 1, &values).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"NF16");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 4);
        let payload_bits = bytes[20..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        let expected_bits = values
            .into_iter()
            .map(|value| half::f16::from_f32(value).to_bits())
            .collect::<Vec<_>>();
        assert_eq!(payload_bits, expected_bits);
        std::fs::remove_file(path).unwrap();
    }
}
